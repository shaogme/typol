use crate::{
    Keyword, Symbol, TokenKind,
    error::SyntaxDiagnostic,
    parser::context::{ParserControl, ParserState},
    storage::{
        BraceEntry, BraceKey, DefaultValue, DefaultValueKind, Id, Name, NumberLiteral,
        ScalarLiteral,
    },
};
use opeo::{OResult, Out};
use typol_core::{ChargedVec, Diagnostic, DiagnosticMessage, SourceSpan};
use typol_i18n::{diagnostic_key, diagnostic_message};

struct ArrayFrame {
    start: SourceSpan,
    elements: ChargedVec<Id<DefaultValue>>,
}

struct BracedStart {
    start: SourceSpan,
    open: SourceSpan,
    variant: Option<Id<Name>>,
}

struct SingleFrame {
    start: SourceSpan,
    open: SourceSpan,
    variant: Id<Name>,
}

struct BracedFrame {
    start: SourceSpan,
    open: SourceSpan,
    variant: Option<Id<Name>>,
    entries: ChargedVec<Id<BraceEntry>>,
    key: BraceKey,
}

enum DefaultFrame {
    Array(ArrayFrame),
    Braced(BracedFrame),
    Single(SingleFrame),
}

pub(crate) struct DefaultParser;

impl DefaultParser {
    pub(crate) fn parse_default_value<'slot>(
        parser: &mut ParserState<'_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<DefaultValue>> {
        let mut frames =
            ParserControl::new_vec(parser, ParserControl::eof_span(parser), out.reborrow())?;
        let mut completed = None;

        loop {
            parser.execution.consume_with(1, out.reborrow(), |error| {
                SyntaxDiagnostic::budget(error, ParserControl::eof_span(parser))
            })?;

            completed = match completed.take() {
                Some(value) => match frames.pop() {
                    Some(frame) => DefaultParser::complete_frame(
                        parser,
                        &mut frames,
                        frame,
                        value,
                        out.reborrow(),
                    )?,
                    None => return Ok(value),
                },
                None => DefaultParser::start_value(parser, &mut frames, out.reborrow())?,
            };
        }
    }

    fn start_value<'slot>(
        parser: &mut ParserState<'_>,
        frames: &mut ChargedVec<DefaultFrame>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Option<Id<DefaultValue>>> {
        let info = ParserControl::peek(parser, out.reborrow())?;
        match info.kind {
            TokenKind::Symbol(Symbol::LeftBracket) => {
                DefaultParser::start_array(parser, frames, info.span, out.reborrow())
            }
            TokenKind::Symbol(Symbol::LeftBrace) => DefaultParser::start_braced(
                parser,
                frames,
                BracedStart {
                    start: info.span,
                    open: info.span,
                    variant: None,
                },
                out.reborrow(),
            ),
            TokenKind::Symbol(Symbol::Dot) => {
                DefaultParser::start_enum(parser, frames, info.span, out.reborrow())
            }
            _ => {
                let kind = DefaultParser::parse_default_atom(parser, out.reborrow())?;
                let value = DefaultParser::make_default(parser, kind, info.span, out.reborrow())?;
                Ok(Some(value))
            }
        }
    }

    fn start_array<'slot>(
        parser: &mut ParserState<'_>,
        frames: &mut ChargedVec<DefaultFrame>,
        start: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Option<Id<DefaultValue>>> {
        ParserControl::bump(parser, out.reborrow())?;
        let elements =
            ParserControl::new_vec(parser, ParserControl::eof_span(parser), out.reborrow())?;
        if ParserControl::eat_symbol(parser, Symbol::RightBracket, out.reborrow())?.is_some() {
            let span = ParserControl::span_from_start(parser, start, out.reborrow())?;
            let array = parser
                .ast
                .array_default_new(elements, span, out.reborrow())?;
            let value = DefaultParser::make_default(
                parser,
                DefaultValueKind::Array(array),
                start,
                out.reborrow(),
            )?;
            return Ok(Some(value));
        }
        ParserControl::try_push(
            frames,
            DefaultFrame::Array(ArrayFrame { start, elements }),
            start,
            out.reborrow(),
        )?;
        Ok(None)
    }

    fn start_braced<'slot>(
        parser: &mut ParserState<'_>,
        frames: &mut ChargedVec<DefaultFrame>,
        braced: BracedStart,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Option<Id<DefaultValue>>> {
        let BracedStart {
            start,
            open,
            variant,
        } = braced;
        ParserControl::bump(parser, out.reborrow())?;
        if ParserControl::eat_symbol(parser, Symbol::RightBrace, out.reborrow())?.is_some() {
            let entries =
                ParserControl::new_vec(parser, ParserControl::eof_span(parser), out.reborrow())?;
            let span = ParserControl::span_from_start(parser, start, out.reborrow())?;
            let kind = if let Some(variant) = variant {
                let payload_span = ParserControl::span_from_start(parser, open, out.reborrow())?;
                let payload = parser.ast.enum_default_payload_structure(
                    entries,
                    payload_span,
                    out.reborrow(),
                )?;
                DefaultValueKind::Enum(parser.ast.enum_default_new(
                    variant,
                    payload,
                    span,
                    out.reborrow(),
                )?)
            } else {
                DefaultValueKind::Braced(parser.ast.braced_default_new(
                    entries,
                    span,
                    out.reborrow(),
                )?)
            };
            let value = DefaultParser::make_default(parser, kind, start, out.reborrow())?;
            return Ok(Some(value));
        }
        let key = DefaultParser::parse_brace_key(parser, variant.is_some(), out.reborrow())?;
        let entries =
            ParserControl::new_vec(parser, ParserControl::eof_span(parser), out.reborrow())?;
        ParserControl::try_push(
            frames,
            DefaultFrame::Braced(BracedFrame {
                start,
                open,
                variant,
                entries,
                key,
            }),
            start,
            out.reborrow(),
        )?;
        Ok(None)
    }

    fn start_enum<'slot>(
        parser: &mut ParserState<'_>,
        frames: &mut ChargedVec<DefaultFrame>,
        start: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Option<Id<DefaultValue>>> {
        ParserControl::bump(parser, out.reborrow())?;
        let variant = ParserControl::take_name(parser, out.reborrow())?;
        match ParserControl::peek(parser, out.reborrow())?.kind {
            TokenKind::Symbol(Symbol::LeftParen) => {
                let open = ParserControl::bump(parser, out.reborrow())?.span;
                ParserControl::try_push(
                    frames,
                    DefaultFrame::Single(SingleFrame {
                        start,
                        open,
                        variant,
                    }),
                    start,
                    out.reborrow(),
                )?;
                Ok(None)
            }
            TokenKind::Symbol(Symbol::LeftBrace) => {
                let open = ParserControl::peek(parser, out.reborrow())?.span;
                DefaultParser::start_braced(
                    parser,
                    frames,
                    BracedStart {
                        start,
                        open,
                        variant: Some(variant),
                    },
                    out.reborrow(),
                )
            }
            _ => {
                let variant_span = parser.ast.span(variant, out.reborrow())?;
                let payload = parser
                    .ast
                    .enum_default_payload_unit(variant_span, out.reborrow())?;
                let span = ParserControl::span_from_start(parser, start, out.reborrow())?;
                let kind = DefaultValueKind::Enum(parser.ast.enum_default_new(
                    variant,
                    payload,
                    span,
                    out.reborrow(),
                )?);
                let value = DefaultParser::make_default(parser, kind, start, out.reborrow())?;
                Ok(Some(value))
            }
        }
    }

    fn complete_frame<'slot>(
        parser: &mut ParserState<'_>,
        frames: &mut ChargedVec<DefaultFrame>,
        frame: DefaultFrame,
        value: Id<DefaultValue>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Option<Id<DefaultValue>>> {
        match frame {
            DefaultFrame::Array(frame) => {
                DefaultParser::complete_array(parser, frames, frame, value, out.reborrow())
            }
            DefaultFrame::Braced(frame) => {
                DefaultParser::complete_braced(parser, frames, frame, value, out.reborrow())
            }
            DefaultFrame::Single(frame) => {
                let value = DefaultParser::complete_single(parser, frame, value, out.reborrow())?;
                Ok(Some(value))
            }
        }
    }

    fn complete_array<'slot>(
        parser: &mut ParserState<'_>,
        frames: &mut ChargedVec<DefaultFrame>,
        frame: ArrayFrame,
        value: Id<DefaultValue>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Option<Id<DefaultValue>>> {
        let ArrayFrame {
            start,
            mut elements,
        } = frame;
        let value_span = parser.ast.span(value, out.reborrow())?;
        ParserControl::try_push(&mut elements, value, value_span, out.reborrow())?;
        if !DefaultParser::finish_default_list(parser, Symbol::RightBracket, out.reborrow())? {
            ParserControl::try_push(
                frames,
                DefaultFrame::Array(ArrayFrame { start, elements }),
                start,
                out.reborrow(),
            )?;
            return Ok(None);
        }
        let span = ParserControl::span_from_start(parser, start, out.reborrow())?;
        let array = parser
            .ast
            .array_default_new(elements, span, out.reborrow())?;
        let value = DefaultParser::make_default(
            parser,
            DefaultValueKind::Array(array),
            start,
            out.reborrow(),
        )?;
        Ok(Some(value))
    }

    fn complete_braced<'slot>(
        parser: &mut ParserState<'_>,
        frames: &mut ChargedVec<DefaultFrame>,
        frame: BracedFrame,
        value: Id<DefaultValue>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Option<Id<DefaultValue>>> {
        let BracedFrame {
            start,
            open,
            variant,
            mut entries,
            key,
        } = frame;
        let entry_start = parser.ast.brace_key_span(&key, out.reborrow())?;
        let value_span = parser.ast.span(value, out.reborrow())?;
        let span = ParserControl::span_between(parser, entry_start, value_span, out.reborrow())?;
        let entry = parser
            .ast
            .brace_entry_new(key, value, span, out.reborrow())?;
        ParserControl::try_push(&mut entries, entry, entry_start, out.reborrow())?;
        if !DefaultParser::finish_default_list(parser, Symbol::RightBrace, out.reborrow())? {
            let key = DefaultParser::parse_brace_key(parser, variant.is_some(), out.reborrow())?;
            ParserControl::try_push(
                frames,
                DefaultFrame::Braced(BracedFrame {
                    start,
                    open,
                    variant,
                    entries,
                    key,
                }),
                start,
                out.reborrow(),
            )?;
            return Ok(None);
        }
        let span = ParserControl::span_from_start(parser, start, out.reborrow())?;
        let kind = if let Some(variant) = variant {
            let payload_span = ParserControl::span_from_start(parser, open, out.reborrow())?;
            let payload =
                parser
                    .ast
                    .enum_default_payload_structure(entries, payload_span, out.reborrow())?;
            DefaultValueKind::Enum(parser.ast.enum_default_new(
                variant,
                payload,
                span,
                out.reborrow(),
            )?)
        } else {
            DefaultValueKind::Braced(parser.ast.braced_default_new(
                entries,
                span,
                out.reborrow(),
            )?)
        };
        let value = DefaultParser::make_default(parser, kind, start, out.reborrow())?;
        Ok(Some(value))
    }

    fn complete_single<'slot>(
        parser: &mut ParserState<'_>,
        frame: SingleFrame,
        value: Id<DefaultValue>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<DefaultValue>> {
        let SingleFrame {
            start,
            open,
            variant,
        } = frame;
        let close = ParserControl::expect_symbol(parser, Symbol::RightParen, out.reborrow())?.span;
        let payload_span = ParserControl::span_between(parser, open, close, out.reborrow())?;
        let payload =
            parser
                .ast
                .enum_default_payload_single(value, payload_span, out.reborrow())?;
        let span = ParserControl::span_from_start(parser, start, out.reborrow())?;
        let kind = DefaultValueKind::Enum(parser.ast.enum_default_new(
            variant,
            payload,
            span,
            out.reborrow(),
        )?);
        DefaultParser::make_default(parser, kind, start, out.reborrow())
    }

    fn make_default<'slot>(
        parser: &mut ParserState<'_>,
        kind: DefaultValueKind,
        start: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<DefaultValue>> {
        parser.ast.default_value_new(
            kind,
            ParserControl::span_from_start(parser, start, out.reborrow())?,
            out.reborrow(),
        )
    }

    fn finish_default_list<'slot>(
        parser: &mut ParserState<'_>,
        close: Symbol,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, bool> {
        if ParserControl::eat_symbol(parser, Symbol::Comma, out.reborrow())?.is_some()
            && ParserControl::peek(parser, out.reborrow())?.kind != TokenKind::Symbol(close)
        {
            return Ok(false);
        }
        ParserControl::expect_symbol(parser, close, out.reborrow())?;
        Ok(true)
    }

    fn parse_brace_key<'slot>(
        parser: &mut ParserState<'_>,
        identifier_only: bool,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, BraceKey> {
        let info = ParserControl::peek(parser, out.reborrow())?;
        let key = match info.kind {
            TokenKind::Identifier => {
                BraceKey::Identifier(ParserControl::take_name(parser, out.reborrow())?)
            }
            TokenKind::String if !identifier_only => {
                BraceKey::String(ParserControl::take_string(parser, out.reborrow())?)
            }
            _ => {
                let message = if identifier_only {
                    "object field names must be identifiers"
                } else {
                    "braced literal keys must be identifiers or strings"
                };
                let localizer = if identifier_only {
                    diagnostic_key!("diagnostic.syntax.object_field_names")
                } else {
                    diagnostic_key!("diagnostic.syntax.braced_literal_keys")
                };
                return ParserControl::syntax_failure(
                    parser,
                    info.span,
                    DiagnosticMessage::localized(message, localizer),
                    out.reborrow(),
                );
            }
        };
        ParserControl::expect_symbol(parser, Symbol::Colon, out.reborrow())?;
        Ok(key)
    }

    fn parse_default_atom<'slot>(
        parser: &mut ParserState<'_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, DefaultValueKind> {
        let info = ParserControl::peek(parser, out.reborrow())?;
        let kind = match info.kind {
            TokenKind::String => DefaultValueKind::Scalar(ScalarLiteral::String(
                ParserControl::take_string(parser, out.reborrow())?,
            )),
            TokenKind::Integer | TokenKind::Float | TokenKind::Symbol(Symbol::Minus) => {
                let number = ParserControl::take_number(parser, true, false, out.reborrow())?;
                DefaultValueKind::Scalar(ScalarLiteral::Number(number))
            }
            TokenKind::Keyword(Keyword::True | Keyword::False) => {
                let value = info.kind == TokenKind::Keyword(Keyword::True);
                ParserControl::bump(parser, out.reborrow())?;
                let literal = parser
                    .ast
                    .boolean_literal_new(value, info.span, out.reborrow())?;
                DefaultValueKind::Scalar(ScalarLiteral::Boolean(literal))
            }
            TokenKind::Keyword(Keyword::None) => {
                ParserControl::bump(parser, out.reborrow())?;
                DefaultValueKind::None
            }
            _ => {
                return ParserControl::syntax_failure(
                    parser,
                    info.span,
                    diagnostic_message!(
                        "diagnostic.syntax.expected_schema_default",
                        "expected a Schema default value here"
                    ),
                    out.reborrow(),
                );
            }
        };
        Ok(kind)
    }

    pub(crate) fn parse_number_annotation<'slot>(
        parser: &mut ParserState<'_>,
        decimal_only: bool,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<NumberLiteral>> {
        if decimal_only {
            return ParserControl::take_number(parser, false, true, out.reborrow());
        }
        let info = ParserControl::peek(parser, out.reborrow())?;
        if info.kind == TokenKind::Symbol(Symbol::Minus) {
            ParserControl::take_number(parser, true, false, out.reborrow())
        } else {
            ParserControl::take_number(parser, false, false, out.reborrow())
        }
    }
}
