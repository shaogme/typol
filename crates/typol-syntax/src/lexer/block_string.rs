use crate::{
    InputLimits, StreamLimits,
    error::SyntaxDiagnostic,
    input::Input,
    token::{Token, TokenKind},
};
use opeo::{OResult, Out};
use typol_core::{
    ChargedString, Diagnostic, DiagnosticCode, DiagnosticMessage, ExecutionBudget, LocatedSpan,
    SourceLocation, SourceSpan, Span, SpanLocation, diagnostic_context,
};
use typol_i18n::diagnostic_message;

struct BlockStringSourcePosition<'source> {
    body_start: LocatedSpan,
    raw: &'source str,
    offset: usize,
    length: usize,
}

struct BlockStringLocation {
    line: usize,
    column: usize,
}

pub(crate) struct BlockStringContext<'borrow, 'input> {
    pub(crate) input: &'borrow mut Input<'input>,
    pub(crate) limits: InputLimits,
    pub(crate) stream: StreamLimits,
    pub(crate) budget: &'borrow ExecutionBudget,
}

pub(crate) struct BlockStringDecoder;

impl BlockStringDecoder {
    pub(crate) fn scan<'slot, 'borrow, 'input>(
        mut context: BlockStringContext<'borrow, 'input>,
        start: LocatedSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Token> {
        for _ in 0..3 {
            Self::step_token(&mut context, start, out.reborrow())?;
        }
        let body_start = match context.input.byte(0, out.reborrow())? {
            Some(b'\n') => {
                Self::step_token(&mut context, start, out.reborrow())?;
                context.input.mark()
            }
            Some(b'\r') if context.input.byte(1, out.reborrow())? == Some(b'\n') => {
                Self::step_token(&mut context, start, out.reborrow())?;
                Self::step_token(&mut context, start, out.reborrow())?;
                context.input.mark()
            }
            Some(b'\r') => {
                let mark = context.input.mark();
                Self::step_token(&mut context, start, out.reborrow())?;
                let diagnostic = Self::diagnostic(
                    &context,
                    mark,
                    diagnostic_message!(
                        "diagnostic.syntax.invalid_line_ending",
                        "a bare CR is not a valid line ending"
                    ),
                    out.reborrow(),
                )?;
                return Err(out.reborrow().fail(diagnostic));
            }
            _ => {
                let diagnostic = Self::diagnostic(
                    &context,
                    start,
                    diagnostic_message!(
                        "diagnostic.syntax.block_string_requires_newline",
                        "a block string opening delimiter must be followed by a line ending"
                    ),
                    out.reborrow(),
                )?;
                return Err(out.reborrow().fail(diagnostic));
            }
        };

        let mut raw = SyntaxDiagnostic::budgeted_with(
            out.reborrow(),
            |out| ChargedString::with_capacity(0, Some(context.budget), out),
            |error| context.input.budget_error(error),
        )?;
        let mut indentation = SyntaxDiagnostic::budgeted_with(
            out.reborrow(),
            |out| ChargedString::with_capacity(0, Some(context.budget), out),
            |error| context.input.budget_error(error),
        )?;
        let mut line_start = true;
        let mut issue = None;
        loop {
            let Some(character) = context.input.character(out.reborrow())? else {
                return match issue {
                    Some(error) => Err(out.reborrow().fail(error)),
                    None => {
                        let diagnostic = Self::diagnostic(
                            &context,
                            start,
                            diagnostic_message!(
                                "diagnostic.syntax.unterminated_string",
                                "string literal is unterminated"
                            ),
                            out.reborrow(),
                        )?;
                        Err(out.reborrow().fail(diagnostic))
                    }
                };
            };
            if line_start && matches!(character, ' ' | '\t') {
                let character = Self::step_token(&mut context, start, out.reborrow())?;
                Self::append(&context, &mut indentation, character, false, out.reborrow())?;
                continue;
            }
            if line_start
                && character == '"'
                && context.input.byte(1, out.reborrow())? == Some(b'"')
                && context.input.byte(2, out.reborrow())? == Some(b'"')
            {
                for _ in 0..3 {
                    Self::step_token(&mut context, start, out.reborrow())?;
                }
                let text = Self::decode(&context, body_start, &raw, issue, out.reborrow())?;
                return Ok(Token::new(
                    TokenKind::String,
                    context.input.span_from(start, out.reborrow())?,
                    Some(text),
                ));
            }

            for character in indentation.chars() {
                Self::append(&context, &mut raw, character, false, out.reborrow())?;
            }
            indentation.clear();
            let mark = context.input.mark();
            let character = Self::step_token(&mut context, start, out.reborrow())?;
            Self::append(&context, &mut raw, character, false, out.reborrow())?;
            if character == '\r'
                && context.input.byte(0, out.reborrow())? != Some(b'\n')
                && issue.is_none()
            {
                issue = Some(Self::diagnostic(
                    &context,
                    mark,
                    diagnostic_message!(
                        "diagnostic.syntax.invalid_line_ending",
                        "a bare CR is not a valid line ending"
                    ),
                    out.reborrow(),
                )?);
            }
            line_start = character == '\n';
        }
    }

    fn decode<'slot>(
        context: &BlockStringContext<'_, '_>,
        body_start: LocatedSpan,
        raw: &ChargedString,
        mut issue: Option<Diagnostic>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ChargedString> {
        let raw = raw.as_str();
        let raw = raw
            .strip_suffix("\r\n")
            .or_else(|| raw.strip_suffix('\n'))
            .unwrap_or(raw);
        let bytes = raw.as_bytes();
        let mut common_indent = usize::MAX;
        let mut line_start = 0;
        while line_start < bytes.len() {
            let mut line_end = line_start;
            while bytes.get(line_end).is_some_and(|byte| *byte != b'\n') {
                context.budget.consume(1, out.reborrow())?;
                line_end += 1;
            }
            let has_newline = line_end < bytes.len();
            let content_end = if has_newline
                && line_end > line_start
                && bytes.get(line_end - 1) == Some(&b'\r')
            {
                line_end - 1
            } else {
                line_end
            };
            let mut content_start = line_start;
            while matches!(bytes.get(content_start), Some(b' ' | b'\t')) {
                context.budget.consume(1, out.reborrow())?;
                content_start += 1;
            }
            if content_start < content_end {
                common_indent = common_indent.min(content_start - line_start);
            }
            line_start = if has_newline {
                line_end + 1
            } else {
                bytes.len()
            };
        }
        if common_indent == usize::MAX {
            common_indent = 0;
        }

        let mut text = SyntaxDiagnostic::budgeted_with(
            out.reborrow(),
            |out| ChargedString::with_capacity(0, Some(context.budget), out),
            |error| context.input.budget_error(error),
        )?;
        line_start = 0;
        while line_start < bytes.len() {
            let mut line_end = line_start;
            while bytes.get(line_end).is_some_and(|byte| *byte != b'\n') {
                context.budget.consume(1, out.reborrow())?;
                line_end += 1;
            }
            let has_newline = line_end < bytes.len();
            let content_end = if has_newline
                && line_end > line_start
                && bytes.get(line_end - 1) == Some(&b'\r')
            {
                line_end - 1
            } else {
                line_end
            };
            let mut content_start = line_start;
            while matches!(bytes.get(content_start), Some(b' ' | b'\t')) {
                context.budget.consume(1, out.reborrow())?;
                content_start += 1;
            }
            if content_start < content_end {
                let mut offset = line_start + common_indent;
                let mut leading = true;
                while offset < content_end {
                    context.budget.consume(1, out.reborrow())?;
                    let Some(character) = raw
                        .get(offset..content_end)
                        .and_then(|rest| rest.chars().next())
                    else {
                        return Err(out.reborrow().fail(context.input.internal_failure(
                            diagnostic_context!(
                                "typol-syntax",
                                "block-string",
                                "line-character-missing"
                            ),
                        )));
                    };
                    if character == '\\' {
                        leading = false;
                        let escaped_offset = offset + character.len_utf8();
                        let escaped = raw
                            .get(escaped_offset..content_end)
                            .and_then(|rest| rest.chars().next());
                        let decoded = match escaped {
                            Some('"') => Some('"'),
                            Some('\\') => Some('\\'),
                            Some('n') => Some('\n'),
                            Some('r') => Some('\r'),
                            Some('t') => Some('\t'),
                            Some(_) => {
                                if issue.is_none() {
                                    issue = Some(Self::diagnostic_for_raw(
                                        context,
                                        BlockStringSourcePosition {
                                            body_start,
                                            raw,
                                            offset,
                                            length: escaped
                                                .map_or(character.len_utf8(), |escaped| {
                                                    character.len_utf8() + escaped.len_utf8()
                                                }),
                                        },
                                        diagnostic_message!(
                                            "diagnostic.syntax.unsupported_escape_sequence",
                                            "string contains an unsupported escape sequence"
                                        ),
                                        out.reborrow(),
                                    )?);
                                }
                                None
                            }
                            None => {
                                if issue.is_none() {
                                    issue = Some(Self::diagnostic_for_raw(
                                        context,
                                        BlockStringSourcePosition {
                                            body_start,
                                            raw,
                                            offset,
                                            length: character.len_utf8(),
                                        },
                                        diagnostic_message!(
                                            "diagnostic.syntax.incomplete_escape_sequence",
                                            "string escape sequence is incomplete"
                                        ),
                                        out.reborrow(),
                                    )?);
                                }
                                None
                            }
                        };
                        if let Some(character) = decoded {
                            Self::append(context, &mut text, character, true, out.reborrow())?;
                        }
                        offset = escaped
                            .map_or(content_end, |escaped| escaped_offset + escaped.len_utf8());
                        continue;
                    }
                    if character == '\t' && leading {
                        Self::append(context, &mut text, character, true, out.reborrow())?;
                    } else if character.is_control() {
                        if issue.is_none() {
                            issue = Some(Self::diagnostic_for_raw(
                                context,
                                BlockStringSourcePosition {
                                    body_start,
                                    raw,
                                    offset,
                                    length: character.len_utf8(),
                                },
                                diagnostic_message!(
                                    "diagnostic.syntax.unescaped_control_character",
                                    "string contains an unescaped control character"
                                ),
                                out.reborrow(),
                            )?);
                        }
                    } else {
                        if !matches!(character, ' ' | '\t') {
                            leading = false;
                        }
                        Self::append(context, &mut text, character, true, out.reborrow())?;
                    }
                    offset += character.len_utf8();
                }
            }
            if has_newline {
                Self::append(context, &mut text, '\n', true, out.reborrow())?;
            }
            line_start = if has_newline {
                line_end + 1
            } else {
                bytes.len()
            };
        }
        if let Some(error) = issue {
            return Err(out.reborrow().fail(error));
        }
        Ok(text)
    }

    fn step_token<'slot>(
        context: &mut BlockStringContext<'_, '_>,
        start: LocatedSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, char> {
        let Some(character) = context.input.character(out.reborrow())? else {
            return Err(out
                .reborrow()
                .fail(context.input.internal_failure(diagnostic_context!(
                    "typol-syntax",
                    "token-cursor",
                    "token-step-reached-end-of-input"
                ))));
        };
        let length = context.input.mark().span().start() - start.span().start();
        if length
            .checked_add(character.len_utf8())
            .is_none_or(|next| next > context.stream.max_token_bytes())
        {
            return Err(out.reborrow().fail(
                context
                    .input
                    .failure(DiagnosticCode::RESOURCE_LIMIT_EXCEEDED),
            ));
        }
        context.input.advance(out.reborrow())
    }

    fn append<'slot>(
        context: &BlockStringContext<'_, '_>,
        text: &mut ChargedString,
        character: char,
        string_limit: bool,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ()> {
        if string_limit
            && text
                .len()
                .checked_add(character.len_utf8())
                .is_none_or(|next| next > context.limits.max_string_bytes())
        {
            return Err(out.reborrow().fail(
                context
                    .input
                    .failure(DiagnosticCode::RESOURCE_LIMIT_EXCEEDED),
            ));
        }
        SyntaxDiagnostic::budgeted_with(
            out,
            |out| text.try_push_char(character, out),
            |error| context.input.budget_error(error),
        )
    }

    fn diagnostic<'slot>(
        context: &BlockStringContext<'_, '_>,
        start: LocatedSpan,
        message: DiagnosticMessage<'static>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Diagnostic> {
        let span = context.input.span_from(start, out.reborrow())?;
        Self::diagnostic_at(context, span, message, out.reborrow())
    }

    fn diagnostic_for_raw<'slot>(
        context: &BlockStringContext<'_, '_>,
        position: BlockStringSourcePosition<'_>,
        message: DiagnosticMessage<'static>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Diagnostic> {
        let span = Self::source_span(context, position, out.reborrow())?;
        Self::diagnostic_at(context, span, message, out.reborrow())
    }

    fn source_span<'slot>(
        context: &BlockStringContext<'_, '_>,
        position: BlockStringSourcePosition<'_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, LocatedSpan> {
        let BlockStringSourcePosition {
            body_start,
            raw,
            offset,
            length,
        } = position;
        let Some(prefix) = raw.get(..offset) else {
            return Err(out
                .reborrow()
                .fail(context.input.internal_failure(diagnostic_context!(
                    "typol-syntax",
                    "block-string",
                    "diagnostic-offset-invalid"
                ))));
        };
        let Some(end) = offset.checked_add(length) else {
            return Err(out.reborrow().fail(
                context
                    .input
                    .failure(DiagnosticCode::RESOURCE_LIMIT_EXCEEDED),
            ));
        };
        let Some(issue) = raw.get(offset..end) else {
            return Err(out
                .reborrow()
                .fail(context.input.internal_failure(diagnostic_context!(
                    "typol-syntax",
                    "block-string",
                    "diagnostic-range-invalid"
                ))));
        };
        let Some(body_location) = body_start.location() else {
            return Err(out
                .reborrow()
                .fail(context.input.internal_failure(diagnostic_context!(
                    "typol-syntax",
                    "block-string",
                    "body-location-missing"
                ))));
        };
        let mut location = BlockStringLocation {
            line: body_location.start.line(),
            column: body_location.start.column(),
        };
        let mut characters = prefix.chars().peekable();
        while let Some(character) = characters.next() {
            context.budget.consume(1, out.reborrow())?;
            Self::advance_location(
                context,
                &mut location,
                character,
                characters.peek().copied(),
                out.reborrow(),
            )?;
        }
        let start_location =
            Self::location(context, location.line, location.column, out.reborrow())?;
        let mut issue_characters = issue.chars().peekable();
        while let Some(character) = issue_characters.next() {
            Self::advance_location(
                context,
                &mut location,
                character,
                issue_characters.peek().copied(),
                out.reborrow(),
            )?;
        }
        let end_location = Self::location(context, location.line, location.column, out.reborrow())?;
        let Some(start_offset) = body_start.span().start().checked_add(offset) else {
            return Err(out.reborrow().fail(
                context
                    .input
                    .failure(DiagnosticCode::RESOURCE_LIMIT_EXCEEDED),
            ));
        };
        let Some(end_offset) = start_offset.checked_add(length) else {
            return Err(out.reborrow().fail(
                context
                    .input
                    .failure(DiagnosticCode::RESOURCE_LIMIT_EXCEEDED),
            ));
        };
        let start = LocatedSpan::new(
            SourceSpan::new(body_start.source_id(), Span::point(start_offset)),
            Some(SpanLocation {
                start: start_location,
                end: start_location,
            }),
        );
        let end = LocatedSpan::new(
            SourceSpan::new(body_start.source_id(), Span::point(end_offset)),
            Some(SpanLocation {
                start: end_location,
                end: end_location,
            }),
        );
        start
            .through(end)
            .ok_or_else(|| {
                context.input.internal_failure(diagnostic_context!(
                    "typol-syntax",
                    "block-string",
                    "diagnostic-span-cannot-be-combined"
                ))
            })
            .map_err(|error| out.fail(error))
    }

    fn advance_location<'slot>(
        context: &BlockStringContext<'_, '_>,
        location: &mut BlockStringLocation,
        character: char,
        next: Option<char>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ()> {
        if character == '\n' {
            let Some(next_line) = location.line.checked_add(1) else {
                return Err(out.reborrow().fail(
                    context
                        .input
                        .failure(DiagnosticCode::RESOURCE_LIMIT_EXCEEDED),
                ));
            };
            location.line = next_line;
            location.column = 1;
        } else if character != '\r' || next != Some('\n') {
            let Some(next_column) = location.column.checked_add(1) else {
                return Err(out.reborrow().fail(
                    context
                        .input
                        .failure(DiagnosticCode::RESOURCE_LIMIT_EXCEEDED),
                ));
            };
            location.column = next_column;
        }
        Ok(())
    }

    fn location<'slot>(
        context: &BlockStringContext<'_, '_>,
        line: usize,
        column: usize,
        out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, SourceLocation> {
        SourceLocation::new(line, column)
            .ok_or_else(|| {
                context.input.internal_failure(diagnostic_context!(
                    "typol-syntax",
                    "block-string",
                    "diagnostic-location-invalid"
                ))
            })
            .map_err(|error| out.fail(error))
    }

    fn diagnostic_at<'slot>(
        context: &BlockStringContext<'_, '_>,
        span: LocatedSpan,
        message: DiagnosticMessage<'static>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Diagnostic> {
        let mut diagnostic = SyntaxDiagnostic::budgeted_with(
            out.reborrow(),
            |out| {
                Diagnostic::try_new(
                    DiagnosticCode::LEXICAL_ERROR,
                    message,
                    Some(context.budget),
                    out,
                )
            },
            |error| context.input.budget_error(error),
        )?;
        diagnostic.set_located_primary(Some(span));
        Ok(diagnostic)
    }
}
