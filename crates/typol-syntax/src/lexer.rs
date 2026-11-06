use crate::{
    InputLimits, StreamLimits,
    error::SyntaxDiagnostic,
    input::{Input, InputConfig},
    parser::SourcePositionIndex,
    token::{CommentKind, Keyword, Symbol, Token, TokenKind},
};
use opeo::{OResult, Out};
use typol_core::{
    ChargedString, ChargedVec, Diagnostic, DiagnosticCode, DiagnosticContext, DiagnosticMessage,
    ExecutionBudget, LocatedSpan, SourceLocation, SourceSpan, Span, diagnostic_context,
};
use typol_i18n::diagnostic_message;

pub(crate) struct Lexer<'input> {
    input: Input<'input>,
    limits: InputLimits,
    stream: StreamLimits,
    budget: ExecutionBudget,
    positions: ChargedVec<(usize, u64)>,
    extended_positions: ChargedVec<(usize, SourceLocation)>,
    fatal: bool,
}

impl<'input> Lexer<'input> {
    pub(crate) fn new<'slot, 'config>(
        config: InputConfig<'input, 'config>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Self> {
        let limits = config.resources;
        let stream = config.stream;
        let budget = config.budget;
        let input = Input::new(config, out.reborrow())?;
        let budget = budget.try_clone_with(out.reborrow(), |error| input.budget_error(error))?;
        let mark = input.mark();
        let span = SourceSpan::new(mark.source_id(), Span::empty());
        let mut positions = SyntaxDiagnostic::new_vec(&budget, span, out.reborrow())?;
        let extended_positions = SyntaxDiagnostic::new_vec(&budget, span, out.reborrow())?;
        if let Some(location) = mark.location() {
            SyntaxDiagnostic::try_push(
                &mut positions,
                (0, pack_position(location.start)),
                span,
                out.reborrow(),
            )?;
        }
        Ok(Self {
            input,
            limits: *limits,
            stream,
            budget,
            positions,
            extended_positions,
            fatal: false,
        })
    }

    pub(crate) fn next_token<'slot>(
        &mut self,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Token> {
        let result = self.scan(out.reborrow());
        if result.is_err() {
            self.fatal = self.input.fatal();
            return result;
        }
        let token = result?;
        self.record_positions(token.located_span(), out.reborrow())?;
        Ok(token)
    }

    pub(crate) fn into_positions(self, source_len: usize) -> SourcePositionIndex {
        SourcePositionIndex::new(self.positions, self.extended_positions, source_len)
    }

    pub(crate) fn fatal(&self) -> bool {
        self.fatal
    }

    pub(crate) fn current_span(&self) -> LocatedSpan {
        self.input.mark()
    }

    pub(crate) fn internal_error(&self, context: DiagnosticContext) -> Diagnostic {
        self.input.internal_failure(context)
    }

    fn record_positions<'slot>(
        &mut self,
        span: LocatedSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ()> {
        let Some(location) = span.location() else {
            return Ok(());
        };
        for (offset, point) in [
            (span.span().start(), location.start),
            (span.span().end(), location.end),
        ] {
            if let Some((previous_offset, previous_location)) = self.positions.last() {
                if *previous_offset == offset {
                    if self.position(*previous_offset, *previous_location) != Some(point) {
                        return Err(out.reborrow().fail(self.input.internal_failure(
                            diagnostic_context!(
                                "typol-syntax",
                                "position-index",
                                "duplicate-offset-location-mismatch"
                            ),
                        )));
                    }
                    continue;
                }
                if *previous_offset > offset {
                    return Err(out.reborrow().fail(self.input.internal_failure(
                        diagnostic_context!(
                            "typol-syntax",
                            "position-index",
                            "offset-order-regressed"
                        ),
                    )));
                }
            }
            let packed = pack_position(point);
            if packed == 0 {
                SyntaxDiagnostic::try_push(
                    &mut self.extended_positions,
                    (offset, point),
                    span.source_span(),
                    out.reborrow(),
                )?;
            }
            SyntaxDiagnostic::try_push(
                &mut self.positions,
                (offset, packed),
                span.source_span(),
                out.reborrow(),
            )?;
        }
        Ok(())
    }

    fn position(&self, offset: usize, packed: u64) -> Option<SourceLocation> {
        if packed == 0 {
            return self
                .extended_positions
                .binary_search_by_key(&offset, |entry| entry.0)
                .ok()
                .and_then(|index| self.extended_positions.get(index))
                .map(|entry| entry.1);
        }
        let line = usize::try_from(packed >> 32).ok()?;
        let column = usize::try_from(packed as u32).ok()?;
        SourceLocation::new(line, column)
    }

    fn scan<'slot>(&mut self, mut out: Out<'slot, '_, Diagnostic>) -> OResult<'slot, Token> {
        self.budget.consume(1, out.reborrow())?;
        self.skip_trivia(out.reborrow())?;
        let start = self.input.mark();
        let Some(character) = self.input.character(out.reborrow())? else {
            return Ok(Token::new(TokenKind::Eof, start, None));
        };
        if character.is_ascii_alphabetic() || character == '_' {
            return self.identifier(start, out.reborrow());
        }
        if character.is_ascii_digit() {
            return self.number(start, out.reborrow());
        }
        if character == '.'
            && self
                .input
                .byte(1, out.reborrow())?
                .is_some_and(|byte| byte.is_ascii_digit())
        {
            self.step_token(start, out.reborrow())?;
            while self
                .input
                .character(out.reborrow())?
                .is_some_and(|character| Symbol::identifier_continue(character) || character == '.')
            {
                self.step_token(start, out.reborrow())?;
            }
            let diagnostic = self.diagnostic(
                start,
                diagnostic_message!(
                    "diagnostic.syntax.float_literal_requires_integer_part",
                    "floating-point literals must include an integer part"
                ),
                out.reborrow(),
            )?;
            return Err(out.reborrow().fail(diagnostic));
        }
        if character == '"' {
            return self.string(start, out.reborrow());
        }
        if Symbol::can_start(character) {
            if let Some(symbol) = self.symbol(start, out.reborrow())? {
                return Ok(Token::new(
                    TokenKind::Symbol(symbol),
                    self.input.span_from(start, out.reborrow())?,
                    None,
                ));
            }
        }
        self.input.advance(out.reborrow())?;
        let diagnostic = self.diagnostic(
            start,
            diagnostic_message!(
                "diagnostic.syntax.invalid_character",
                "source contains an invalid character"
            ),
            out.reborrow(),
        )?;
        Err(out.reborrow().fail(diagnostic))
    }

    fn diagnostic<'slot>(
        &self,
        start: LocatedSpan,
        message: DiagnosticMessage<'static>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Diagnostic> {
        let mut diagnostic = SyntaxDiagnostic::budgeted_with(
            out.reborrow(),
            |out| {
                Diagnostic::try_new(
                    DiagnosticCode::LEXICAL_ERROR,
                    message,
                    Some(&self.budget),
                    out,
                )
            },
            |error| self.input.budget_error(error),
        )?;
        let span = self.input.span_from(start, out.reborrow())?;
        diagnostic.set_located_primary(Some(span));
        Ok(diagnostic)
    }

    fn step_token<'slot>(
        &mut self,
        start: LocatedSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, char> {
        let Some(character) = self.input.character(out.reborrow())? else {
            return Err(out
                .reborrow()
                .fail(self.input.internal_failure(diagnostic_context!(
                    "typol-syntax",
                    "token-cursor",
                    "token-step-reached-end-of-input"
                ))));
        };
        let length = self.input.mark().span().start() - start.span().start();
        if length
            .checked_add(character.len_utf8())
            .is_none_or(|next| next > self.stream.max_token_bytes())
        {
            return Err(out
                .reborrow()
                .fail(self.input.failure(DiagnosticCode::RESOURCE_LIMIT_EXCEEDED)));
        }
        self.input.advance(out.reborrow())
    }

    fn append<'slot>(
        &self,
        text: &mut ChargedString,
        character: char,
        string_limit: bool,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ()> {
        if string_limit
            && text
                .len()
                .checked_add(character.len_utf8())
                .is_none_or(|next| next > self.limits.max_string_bytes())
        {
            return Err(out
                .reborrow()
                .fail(self.input.failure(DiagnosticCode::RESOURCE_LIMIT_EXCEEDED)));
        }
        SyntaxDiagnostic::budgeted_with(
            out,
            |out| text.try_push_char(character, out),
            |error| self.input.budget_error(error),
        )
    }

    fn skip_trivia<'slot>(&mut self, mut out: Out<'slot, '_, Diagnostic>) -> OResult<'slot, ()> {
        loop {
            match self.input.character(out.reborrow())? {
                Some(' ' | '\t' | '\n') => {
                    self.input.advance(out.reborrow())?;
                }
                Some('\r') => {
                    let start = self.input.mark();
                    let paired = self.input.byte(1, out.reborrow())? == Some(b'\n');
                    self.input.advance(out.reborrow())?;
                    if !paired {
                        let diagnostic = self.diagnostic(
                            start,
                            diagnostic_message!(
                                "diagnostic.syntax.invalid_line_ending",
                                "a bare CR is not a valid line ending"
                            ),
                            out.reborrow(),
                        )?;
                        return Err(out.reborrow().fail(diagnostic));
                    }
                    self.input.advance(out.reborrow())?;
                }
                Some('/') => {
                    match Symbol::comment_kind(b'/', self.input.byte(1, out.reborrow())?) {
                        Some(CommentKind::Line) => self.comment(false, out.reborrow())?,
                        Some(CommentKind::Block) => self.comment(true, out.reborrow())?,
                        None => break,
                    }
                }
                _ => break,
            }
        }
        Ok(())
    }

    fn comment<'slot>(
        &mut self,
        block: bool,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ()> {
        let start = self.input.mark();
        self.input.advance(out.reborrow())?;
        self.input.advance(out.reborrow())?;
        let mut issue = None;
        loop {
            let Some(character) = self.input.character(out.reborrow())? else {
                return match issue {
                    Some(error) => Err(out.reborrow().fail(error)),
                    None if block => {
                        let diagnostic = self.diagnostic(
                            start,
                            diagnostic_message!(
                                "diagnostic.syntax.unterminated_block_comment",
                                "block comment is unterminated"
                            ),
                            out.reborrow(),
                        )?;
                        Err(out.reborrow().fail(diagnostic))
                    }
                    None => Ok(()),
                };
            };
            if block && character == '*' && self.input.byte(1, out.reborrow())? == Some(b'/') {
                self.input.advance(out.reborrow())?;
                self.input.advance(out.reborrow())?;
                break;
            }
            if character == '\r' && self.input.byte(1, out.reborrow())? != Some(b'\n') {
                let mark = self.input.mark();
                self.input.advance(out.reborrow())?;
                if issue.is_none() {
                    issue = Some(self.diagnostic(
                        mark,
                        diagnostic_message!(
                            "diagnostic.syntax.invalid_line_ending",
                            "a bare CR is not a valid line ending"
                        ),
                        out.reborrow(),
                    )?);
                }
            } else {
                self.input.advance(out.reborrow())?;
                if !block && character == '\n' {
                    break;
                }
            }
        }
        match issue {
            Some(error) => Err(out.reborrow().fail(error)),
            None => Ok(()),
        }
    }

    fn identifier<'slot>(
        &mut self,
        start: LocatedSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Token> {
        let mut text = SyntaxDiagnostic::budgeted_with(
            out.reborrow(),
            |out| ChargedString::with_capacity(0, Some(&self.budget), out),
            |error| self.input.budget_error(error),
        )?;
        while self
            .input
            .character(out.reborrow())?
            .is_some_and(Symbol::identifier_continue)
        {
            let character = self.step_token(start, out.reborrow())?;
            self.append(&mut text, character, true, out.reborrow())?;
        }
        let keyword = Keyword::from_identifier(&text, &self.budget, out.reborrow())?;
        let kind = keyword
            .map(TokenKind::Keyword)
            .unwrap_or(TokenKind::Identifier);
        Ok(Token::new(
            kind,
            self.input.span_from(start, out.reborrow())?,
            keyword.is_none().then_some(text),
        ))
    }

    fn number<'slot>(
        &mut self,
        start: LocatedSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Token> {
        let mut text = SyntaxDiagnostic::budgeted_with(
            out.reborrow(),
            |out| ChargedString::with_capacity(0, Some(&self.budget), out),
            |error| self.input.budget_error(error),
        )?;
        while self
            .input
            .character(out.reborrow())?
            .is_some_and(|character| Symbol::identifier_continue(character) || character == '.')
        {
            let character = self.step_token(start, out.reborrow())?;
            self.append(&mut text, character, false, out.reborrow())?;
        }
        self.budget.consume(text.len(), out.reborrow())?;
        let kind = if let Some(hex) = text.strip_prefix("0x") {
            if hex.is_empty() || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                let diagnostic = self.diagnostic(
                    start,
                    diagnostic_message!(
                        "diagnostic.syntax.malformed_hex_integer",
                        "hexadecimal integer literal is malformed"
                    ),
                    out.reborrow(),
                )?;
                return Err(out.reborrow().fail(diagnostic));
            }
            TokenKind::Integer
        } else if let Some((whole, fraction)) = text.split_once('.') {
            if whole.is_empty()
                || fraction.is_empty()
                || !whole
                    .bytes()
                    .chain(fraction.bytes())
                    .all(|byte| byte.is_ascii_digit())
            {
                let diagnostic = self.diagnostic(
                    start,
                    diagnostic_message!(
                        "diagnostic.syntax.malformed_float",
                        "floating-point literal is malformed"
                    ),
                    out.reborrow(),
                )?;
                return Err(out.reborrow().fail(diagnostic));
            }
            TokenKind::Float
        } else {
            if !text.bytes().all(|byte| byte.is_ascii_digit()) {
                let diagnostic = self.diagnostic(
                    start,
                    diagnostic_message!(
                        "diagnostic.syntax.number_followed_by_identifier",
                        "an identifier character cannot immediately follow a number"
                    ),
                    out.reborrow(),
                )?;
                return Err(out.reborrow().fail(diagnostic));
            }
            TokenKind::Integer
        };
        Ok(Token::new(
            kind,
            self.input.span_from(start, out.reborrow())?,
            Some(text),
        ))
    }

    fn string<'slot>(
        &mut self,
        start: LocatedSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Token> {
        self.step_token(start, out.reborrow())?;
        let mut text = SyntaxDiagnostic::budgeted_with(
            out.reborrow(),
            |out| ChargedString::with_capacity(0, Some(&self.budget), out),
            |error| self.input.budget_error(error),
        )?;
        let mut issue = None;
        loop {
            let Some(character) = self.input.character(out.reborrow())? else {
                return match issue {
                    Some(error) => Err(out.reborrow().fail(error)),
                    None => {
                        let diagnostic = self.diagnostic(
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
            let mark = self.input.mark();
            self.step_token(start, out.reborrow())?;
            if character == '"' {
                break;
            }
            let decoded = if character == '\\' {
                let Some(escaped) = self.input.character(out.reborrow())? else {
                    return match issue {
                        Some(error) => Err(out.reborrow().fail(error)),
                        None => {
                            let diagnostic = self.diagnostic(
                                mark,
                                diagnostic_message!(
                                    "diagnostic.syntax.incomplete_escape_sequence",
                                    "string escape sequence is incomplete"
                                ),
                                out.reborrow(),
                            )?;
                            Err(out.reborrow().fail(diagnostic))
                        }
                    };
                };
                self.step_token(start, out.reborrow())?;
                match escaped {
                    '"' => Some('"'),
                    '\\' => Some('\\'),
                    'n' => Some('\n'),
                    'r' => Some('\r'),
                    't' => Some('\t'),
                    _ => {
                        if issue.is_none() {
                            issue = Some(self.diagnostic(
                                mark,
                                diagnostic_message!(
                                    "diagnostic.syntax.unsupported_escape_sequence",
                                    "string contains an unsupported escape sequence"
                                ),
                                out.reborrow(),
                            )?);
                        }
                        None
                    }
                }
            } else if character.is_control() {
                if issue.is_none() {
                    issue = Some(self.diagnostic(
                        mark,
                        diagnostic_message!(
                            "diagnostic.syntax.unescaped_control_character",
                            "string contains an unescaped control character"
                        ),
                        out.reborrow(),
                    )?);
                }
                None
            } else {
                Some(character)
            };
            if let Some(character) = decoded {
                self.append(&mut text, character, true, out.reborrow())?;
            }
        }
        if let Some(error) = issue {
            return Err(out.reborrow().fail(error));
        }
        Ok(Token::new(
            TokenKind::String,
            self.input.span_from(start, out.reborrow())?,
            Some(text),
        ))
    }

    fn symbol<'slot>(
        &mut self,
        start: LocatedSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Option<Symbol>> {
        let Some(byte) = self.input.byte(0, out.reborrow())? else {
            return Err(out
                .reborrow()
                .fail(self.input.internal_failure(diagnostic_context!(
                    "typol-syntax",
                    "stream-input",
                    "symbol-start-byte-missing"
                ))));
        };
        let next = if Symbol::has_pair_prefix(byte) {
            self.input.byte(1, out.reborrow())?
        } else {
            None
        };
        let Some((symbol, width)) = Symbol::from_ascii(byte, next) else {
            return Ok(None);
        };
        for _ in 0..width {
            self.step_token(start, out.reborrow())?;
        }
        Ok(Some(symbol))
    }
}

fn pack_position(location: SourceLocation) -> u64 {
    let Some(line) = u32::try_from(location.line()).ok() else {
        return 0;
    };
    let Some(column) = u32::try_from(location.column()).ok() else {
        return 0;
    };
    (u64::from(line) << 32) | u64::from(column)
}
