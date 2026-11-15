mod state;

pub(crate) use state::{DelimiterFrame, ParserState, SyntaxBudget, TokenInfo};

use crate::{
    InputConfig, Keyword, LexEvent, Symbol, SyntaxLimits, Token, TokenCursor, TokenKind,
    error::{DiagnosticSink, LabelMessage, MessageBuild, SyntaxDiagnostic},
    storage::{
        Id, Name, NumberKind, NumberLiteral, NumberLiteralParams, Storage, StorageOptions,
        StringLiteral,
    },
};
use opeo::{OResult, Out, ResultOutExt};
use typol_core::{
    ChargedBox, ChargedVec, Diagnostic, DiagnosticCode, DiagnosticContext, RelatedLocationRole,
    SourceSpan, Span, diagnostic_context,
};
use typol_i18n::{Translator, t};
use u18n::RenderError;

macro_rules! parser_internal_error {
    ($parser:ident, $failure_point:literal, $span:expr $(,)?) => {{
        use typol_core::diagnostic_context;

        $crate::parser::context::ParserControl::internal_error(
            diagnostic_context!("typol-syntax", "parser", $failure_point),
            $span,
            $parser.translator,
        )
    }};
}

pub(crate) struct ParserControl;

impl ParserControl {
    pub(crate) fn create<'slot, 'src, 'config>(
        config: InputConfig<'src, 'config>,
        limits: &SyntaxLimits,
        translator: Translator,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ParserState<'src>> {
        let InputConfig {
            source_id,
            source,
            resources,
            stream,
            budget: execution,
        } = config;
        let source_limit = resources.max_source_bytes();
        let cursor = TokenCursor::new(
            InputConfig {
                source_id,
                source,
                resources,
                stream,
                budget: execution,
            },
            translator,
            out.reborrow(),
        )?;
        let diagnostics = DiagnosticSink::new(
            limits.max_diagnostics(),
            execution,
            SourceSpan::new(source_id, Span::empty()),
            translator,
            out.reborrow(),
        )?;

        let cursor_span = SourceSpan::new(source_id, Span::empty());
        let ast = Storage::new(
            StorageOptions {
                source_id,
                source_limit,
                limits,
                execution,
                translator,
            },
            out.reborrow(),
        )?;
        let ast = ChargedBox::new(ast, Some(execution), out.reborrow())?;
        let delimiters =
            SyntaxDiagnostic::new_vec(execution, cursor_span, translator, out.reborrow())?;
        let execution = execution.try_clone_with(out.reborrow(), |error| {
            SyntaxDiagnostic::budget(error, cursor_span, translator)
        })?;
        Ok(ParserState {
            cursor,
            ast,
            source_id,
            source_limit,
            actual_source_len: None,
            diagnostics,
            delimiters,
            trusted: true,
            prefix_frames: 0,
            budget: SyntaxBudget::new(limits.max_nesting_depth(), translator),
            last_end: 0,
            stopped: false,
            execution,
            translator,
        })
    }

    pub(crate) fn peek<'slot>(
        parser: &mut ParserState<'_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, TokenInfo> {
        parser.execution.consume_with(1, out.reborrow(), |error| {
            SyntaxDiagnostic::budget(error, ParserControl::eof_span(parser), parser.translator)
        })?;
        match parser.cursor.peek(out.reborrow())? {
            LexEvent::Token(token) => Ok(TokenInfo {
                kind: token.kind(),
                span: token.span(),
            }),
            LexEvent::Eof(token) => {
                let info = TokenInfo {
                    kind: token.kind(),
                    span: token.span(),
                };
                ParserControl::confirm_eof(parser, info.span, out.reborrow())?;
                Ok(info)
            }
            LexEvent::Error(_) | LexEvent::Fatal(_) => {
                ParserControl::consume_error_event(parser, out.reborrow())
            }
            LexEvent::Halted(span) => {
                let span = *span;
                let diagnostic = ParserControl::halted_error(parser, span);
                Err(out.reborrow().fail(diagnostic))
            }
        }
    }

    fn consume_error_event<'slot>(
        parser: &mut ParserState<'_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, TokenInfo> {
        match parser.cursor.bump(out.reborrow())? {
            LexEvent::Error(diagnostic) | LexEvent::Fatal(diagnostic) => {
                Err(out.reborrow().fail(diagnostic))
            }
            _ => Err(out.reborrow().fail(parser_internal_error!(
                parser,
                "error-event-disappeared-during-bump",
                ParserControl::eof_span(parser),
            ))),
        }
    }

    pub(crate) fn peek_next<'slot>(
        parser: &mut ParserState<'_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, TokenInfo> {
        ParserControl::peek(parser, out.reborrow())?;
        match parser.cursor.peek_next(out.reborrow())? {
            LexEvent::Token(token) | LexEvent::Eof(token) => Ok(TokenInfo {
                kind: token.kind(),
                span: token.span(),
            }),
            LexEvent::Error(_) | LexEvent::Fatal(_) | LexEvent::Halted(_) => {
                ParserControl::bump(parser, out.reborrow())?;
                ParserControl::peek(parser, out.reborrow())
            }
        }
    }

    pub(crate) fn next_token<'slot, 'a, 'src>(
        parser: &'a mut ParserState<'src>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, &'a Token> {
        ParserControl::peek_next(parser, out.reborrow())?;
        let fallback = ParserControl::eof_span(parser);
        match parser.cursor.peek_next(out.reborrow())? {
            LexEvent::Token(token) | LexEvent::Eof(token) => Ok(token),
            LexEvent::Error(_) | LexEvent::Fatal(_) | LexEvent::Halted(_) => {
                Err(out.reborrow().fail(parser_internal_error!(
                    parser,
                    "next-token-event-not-token",
                    fallback
                )))
            }
        }
    }

    pub(crate) fn bump<'slot>(
        parser: &mut ParserState<'_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, TokenInfo> {
        let info = ParserControl::peek(parser, out.reborrow())?;
        if let TokenKind::Symbol(symbol) = info.kind {
            match symbol {
                Symbol::LeftParen | Symbol::LeftBrace | Symbol::LeftBracket => {
                    parser.budget.enter(info.span, out.reborrow())?;
                    let Some(close) = closing_symbol(symbol) else {
                        return Err(out.reborrow().fail(parser_internal_error!(
                            parser,
                            "opening-symbol-has-no-closing-symbol",
                            info.span,
                        )));
                    };
                    SyntaxDiagnostic::try_push(
                        &mut parser.delimiters,
                        DelimiterFrame {
                            close,
                            open_span: info.span,
                        },
                        info.span,
                        parser.translator,
                        out.reborrow(),
                    )?;
                }
                Symbol::RightParen | Symbol::RightBrace | Symbol::RightBracket
                    if parser
                        .delimiters
                        .last()
                        .is_some_and(|frame| frame.close == symbol) =>
                {
                    if parser.delimiters.pop().is_none() {
                        return Err(out.reborrow().fail(parser_internal_error!(
                            parser,
                            "closing-delimiter-stack-underflow",
                            info.span,
                        )));
                    }
                    parser.budget.leave(1, info.span, out.reborrow())?;
                }
                Symbol::RightParen | Symbol::RightBrace | Symbol::RightBracket => {
                    parser.trusted = false;
                    match parser.cursor.bump(out.reborrow())? {
                        LexEvent::Token(_) => {}
                        _ => {
                            return Err(out.reborrow().fail(parser_internal_error!(
                                parser,
                                "unexpected-event-while-consuming-close",
                                info.span,
                            )));
                        }
                    }
                    parser.last_end = info.span.span().end();
                    let mut diagnostic = ParserControl::syntax_error(
                        parser,
                        info.span,
                        t! {
                            name: "diagnostic.syntax.mismatched_closing_delimiter",
                            locale: parser.translator,
                        },
                        out.reborrow(),
                    )?;
                    if diagnostic.code() != DiagnosticCode::ALLOCATION_FAILED {
                        if let Some(frame) = parser.delimiters.last() {
                            SyntaxDiagnostic::try_add_label_message(
                                &mut diagnostic,
                                LabelMessage {
                                    span: frame.open_span,
                                    text: t! {
                                        name: "diagnostic.label.associated_input",
                                        locale: parser.translator,
                                    },
                                    role: RelatedLocationRole::AssociatedInput,
                                },
                                &parser.execution,
                                parser.translator,
                                out.reborrow(),
                            )?;
                        }
                    }
                    return Err(out.reborrow().fail(diagnostic));
                }
                _ => {}
            }
        }

        match parser.cursor.bump(out.reborrow())? {
            LexEvent::Token(_) | LexEvent::Eof(_) => {}
            _ => {
                return Err(out.reborrow().fail(parser_internal_error!(
                    parser,
                    "unexpected-event-while-consuming-token",
                    info.span,
                )));
            }
        }
        parser.last_end = info.span.span().end();
        Ok(info)
    }

    pub(crate) fn eat_symbol<'slot>(
        parser: &mut ParserState<'_>,
        expected: Symbol,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Option<TokenInfo>> {
        let info = ParserControl::peek(parser, out.reborrow())?;
        if info.kind == TokenKind::Symbol(expected) {
            Ok(Some(ParserControl::bump(parser, out.reborrow())?))
        } else {
            Ok(None)
        }
    }

    pub(crate) fn expect_symbol<'slot>(
        parser: &mut ParserState<'_>,
        expected: Symbol,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, TokenInfo> {
        match ParserControl::eat_symbol(parser, expected, out.reborrow())? {
            Some(info) => Ok(info),
            None => {
                let actual = ParserControl::peek(parser, out.reborrow())?;
                ParserControl::syntax_failure(
                    parser,
                    actual.span,
                    t! {
                        name: "diagnostic.syntax.expected_punctuation",
                        locale: parser.translator,
                    },
                    out.reborrow(),
                )
            }
        }
    }

    pub(crate) fn eat_keyword<'slot>(
        parser: &mut ParserState<'_>,
        expected: Keyword,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Option<TokenInfo>> {
        let info = ParserControl::peek(parser, out.reborrow())?;
        if info.kind == TokenKind::Keyword(expected) {
            Ok(Some(ParserControl::bump(parser, out.reborrow())?))
        } else {
            Ok(None)
        }
    }

    pub(crate) fn expect_keyword<'slot>(
        parser: &mut ParserState<'_>,
        expected: Keyword,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, TokenInfo> {
        match ParserControl::eat_keyword(parser, expected, out.reborrow())? {
            Some(info) => Ok(info),
            None => {
                let actual = ParserControl::peek(parser, out.reborrow())?;
                ParserControl::syntax_failure(
                    parser,
                    actual.span,
                    t! {
                        name: "diagnostic.syntax.expected_reserved_word",
                        locale: parser.translator,
                    },
                    out.reborrow(),
                )
            }
        }
    }

    pub(crate) fn take_name<'slot>(
        parser: &mut ParserState<'_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<Name>> {
        let info = ParserControl::peek(parser, out.reborrow())?;
        if info.kind != TokenKind::Identifier {
            return ParserControl::syntax_failure(
                parser,
                info.span,
                t! {
                    name: "diagnostic.syntax.expected_identifier",
                    locale: parser.translator,
                },
                out.reborrow(),
            );
        }
        let text = match parser.cursor.peek(out.reborrow())? {
            LexEvent::Token(token) => token
                .text()
                .ok_or_else(|| {
                    parser_internal_error!(parser, "identifier-token-text-missing", info.span)
                })
                .store_err_into(out.reborrow())?,
            _ => {
                return Err(out.reborrow().fail(parser_internal_error!(
                    parser,
                    "identifier-event-not-token",
                    info.span,
                )));
            }
        };
        let name = parser.ast.name_try_new(text, info.span, out.reborrow())?;
        ParserControl::bump(parser, out.reborrow())?;
        Ok(name)
    }

    pub(crate) fn take_string<'slot>(
        parser: &mut ParserState<'_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<StringLiteral>> {
        let info = ParserControl::peek(parser, out.reborrow())?;
        if info.kind != TokenKind::String {
            return ParserControl::syntax_failure(
                parser,
                info.span,
                t! {
                    name: "diagnostic.syntax.expected_string_literal",
                    locale: parser.translator,
                },
                out.reborrow(),
            );
        }
        parser.last_end = info.span.span().end();
        let token = match parser.cursor.bump(out.reborrow())? {
            LexEvent::Token(token) => token,
            _ => {
                return Err(out.reborrow().fail(parser_internal_error!(
                    parser,
                    "string-event-not-token",
                    info.span,
                )));
            }
        };
        let text = token
            .into_decoded_string()
            .ok_or_else(|| parser_internal_error!(parser, "decoded-string-missing", info.span))
            .store_err_into(out.reborrow())?;
        parser
            .ast
            .string_literal_from_owned(text, info.span, out.reborrow())
    }

    pub(crate) fn take_number<'slot>(
        parser: &mut ParserState<'_>,
        allow_negative: bool,
        decimal_only: bool,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<NumberLiteral>> {
        let minus = if allow_negative {
            ParserControl::eat_symbol(parser, Symbol::Minus, out.reborrow())?
        } else {
            None
        };
        let parsed = {
            let info = ParserControl::peek(parser, out.reborrow())?;
            let number_kind = match info.kind {
                TokenKind::Integer => Ok(NumberKind::Integer),
                TokenKind::Float if !decimal_only => Ok(NumberKind::Float),
                _ => Err(info.span),
            };
            match number_kind {
                Ok(number_kind) => {
                    let span = match minus {
                        Some(minus) => ParserControl::combine_spans(
                            parser,
                            minus.span,
                            info.span,
                            out.reborrow(),
                        )?,
                        None => info.span,
                    };
                    let token = match parser.cursor.peek(out.reborrow())? {
                        LexEvent::Token(token) | LexEvent::Eof(token) => token,
                        _ => {
                            return Err(out.reborrow().fail(parser_internal_error!(
                                parser,
                                "number-event-not-token",
                                info.span,
                            )));
                        }
                    };
                    let Some(raw) = token.text() else {
                        return Err(out.reborrow().fail(parser_internal_error!(
                            parser,
                            "number-token-text-missing",
                            info.span,
                        )));
                    };
                    if decimal_only {
                        parser
                            .execution
                            .consume_with(raw.len(), out.reborrow(), |error| {
                                SyntaxDiagnostic::budget(error, info.span, parser.translator)
                            })?;
                    }
                    if decimal_only && !raw.bytes().all(|byte| byte.is_ascii_digit()) {
                        Err(info.span)
                    } else {
                        Ok(parser.ast.number_literal_try_new(
                            NumberLiteralParams {
                                raw,
                                kind: number_kind,
                                negative: minus.is_some(),
                                span,
                            },
                            out.reborrow(),
                        )?)
                    }
                }
                Err(span) => Err(span),
            }
        };
        let literal = match parsed {
            Ok(literal) => literal,
            Err(span) => {
                return ParserControl::syntax_failure(
                    parser,
                    span,
                    if minus.is_some() {
                        t! {
                            name: "diagnostic.syntax.expected_decimal_after_minus",
                            locale: parser.translator,
                        }
                    } else {
                        t! {
                            name: "diagnostic.syntax.expected_number",
                            locale: parser.translator,
                        }
                    },
                    out.reborrow(),
                );
            }
        };
        ParserControl::bump(parser, out.reborrow())?;
        Ok(literal)
    }

    pub(crate) fn span_between<'slot>(
        parser: &ParserState<'_>,
        start: SourceSpan,
        end: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, SourceSpan> {
        ParserControl::combine_spans(parser, start, end, out.reborrow())
    }

    pub(crate) fn span_from_start<'slot>(
        parser: &ParserState<'_>,
        start: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, SourceSpan> {
        let end = SourceSpan::new(parser.source_id, Span::point(parser.last_end));
        ParserControl::combine_spans(parser, start, end, out.reborrow())
    }

    pub(crate) fn combine_spans<'slot>(
        parser: &ParserState<'_>,
        start: SourceSpan,
        end: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, SourceSpan> {
        if start.source_id() != parser.source_id
            || end.source_id() != parser.source_id
            || start.span().start() > end.span().end()
        {
            return Err(out.reborrow().fail(parser_internal_error!(
                parser,
                "combined-source-span-order-invalid",
                start,
            )));
        }
        let range = Span::new_with(
            start.span().start(),
            end.span().end(),
            out.reborrow(),
            |_| parser_internal_error!(parser, "combined-source-span-reversed", start),
        )?;
        Ok(SourceSpan::new(parser.source_id, range))
    }

    pub(crate) fn internal_error(
        context: DiagnosticContext,
        span: SourceSpan,
        translator: Translator,
    ) -> Diagnostic {
        SyntaxDiagnostic::internal(context, span, translator)
    }

    pub(crate) fn syntax_error<'slot>(
        parser: &ParserState<'_>,
        span: SourceSpan,
        message: Result<&'static str, RenderError>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Diagnostic> {
        SyntaxDiagnostic::make(
            MessageBuild {
                code: DiagnosticCode::SCHEMA_SYNTAX_ERROR,
                span,
                budget: &parser.execution,
                translator: parser.translator,
            },
            message,
            out.reborrow(),
        )
    }

    pub(crate) fn syntax_failure<'slot, T>(
        parser: &ParserState<'_>,
        span: SourceSpan,
        message: Result<&'static str, RenderError>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, T> {
        let diagnostic = SyntaxDiagnostic::make(
            MessageBuild {
                code: DiagnosticCode::SCHEMA_SYNTAX_ERROR,
                span,
                budget: &parser.execution,
                translator: parser.translator,
            },
            message,
            out.reborrow(),
        )?;
        Err(out.reborrow().fail(diagnostic))
    }

    pub(crate) fn halted_error(parser: &ParserState<'_>, span: SourceSpan) -> Diagnostic {
        SyntaxDiagnostic::internal(
            diagnostic_context!("typol-syntax", "parser", "cursor-already-halted"),
            span,
            parser.translator,
        )
    }

    pub(crate) fn push_diagnostic<'slot>(
        parser: &mut ParserState<'_>,
        diagnostic: Diagnostic,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ()> {
        if parser.stopped {
            return Ok(());
        }
        parser.diagnostics.push(diagnostic, out.reborrow())?;
        parser.stopped = parser.diagnostics.stopped() || !parser.trusted;
        Ok(())
    }

    pub(crate) fn stopped_error(parser: &ParserState<'_>) -> Diagnostic {
        SyntaxDiagnostic::emergency(
            DiagnosticCode::SCHEMA_SYNTAX_ERROR,
            t! {
                name: "diagnostic.emergency.syntax_error",
                locale: parser.translator,
            },
            ParserControl::eof_span(parser),
        )
    }

    pub(crate) fn eof_span(parser: &ParserState<'_>) -> SourceSpan {
        let offset = parser.actual_source_len.unwrap_or(parser.last_end);
        SourceSpan::new(parser.source_id, Span::point(offset))
    }

    fn confirm_eof<'slot>(
        parser: &mut ParserState<'_>,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ()> {
        let offset = span.span().start();
        if span.source_id() != parser.source_id
            || !span.span().is_empty()
            || offset > parser.source_limit
        {
            return Err(out.reborrow().fail(parser_internal_error!(
                parser,
                "eof-position-outside-source-limit",
                span,
            )));
        }
        if parser
            .actual_source_len
            .is_some_and(|actual| actual != offset)
        {
            return Err(out.reborrow().fail(parser_internal_error!(
                parser,
                "eof-position-changed",
                span,
            )));
        }
        parser.ast.confirm_eof(offset);
        parser.actual_source_len = Some(offset);
        Ok(())
    }

    pub(crate) fn current_depth(parser: &ParserState<'_>) -> usize {
        parser.delimiters.len()
    }

    pub(crate) fn enter_prefix_frame<'slot>(
        parser: &mut ParserState<'_>,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ()> {
        let Some(next) = parser.prefix_frames.checked_add(1) else {
            return Err(out.reborrow().fail(parser_internal_error!(
                parser,
                "prefix-frame-depth-overflow",
                span
            )));
        };
        parser.budget.enter(span, out.reborrow())?;
        parser.prefix_frames = next;
        Ok(())
    }

    pub(crate) fn leave_prefix_frame<'slot>(
        parser: &mut ParserState<'_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ()> {
        let span = ParserControl::eof_span(parser);
        let Some(frames) = parser.prefix_frames.checked_sub(1) else {
            return Err(out.reborrow().fail(parser_internal_error!(
                parser,
                "prefix-frame-depth-underflow",
                span
            )));
        };
        parser.prefix_frames = frames;
        parser.budget.leave(1, span, out.reborrow())
    }

    pub(crate) fn restore_prefix_frames<'slot>(
        parser: &mut ParserState<'_>,
        saved: usize,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ()> {
        let span = ParserControl::eof_span(parser);
        let Some(released) = parser.prefix_frames.checked_sub(saved) else {
            return Err(out.reborrow().fail(parser_internal_error!(
                parser,
                "prefix-frame-restore-underflow",
                span
            )));
        };
        parser.budget.leave(released, span, out.reborrow())?;
        parser.prefix_frames = saved;
        Ok(())
    }
}

fn closing_symbol(open: Symbol) -> Option<Symbol> {
    match open {
        Symbol::LeftParen => Some(Symbol::RightParen),
        Symbol::LeftBrace => Some(Symbol::RightBrace),
        Symbol::LeftBracket => Some(Symbol::RightBracket),
        _ => None,
    }
}

impl ParserControl {
    pub(crate) fn name_is<'slot>(
        parser: &ParserState<'_>,
        name: Id<Name>,
        expected: &str,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, bool> {
        let text = parser.ast.name_text_for_diagnostic(name, out.reborrow())?;
        Ok(parser
            .execution
            .compare_text(text, expected, out.reborrow())?
            .is_eq())
    }

    pub(crate) fn try_push<'slot, T>(
        parser: &ParserState<'_>,
        values: &mut ChargedVec<T>,
        value: T,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ()> {
        SyntaxDiagnostic::try_push(values, value, span, parser.translator, out.reborrow())
    }

    pub(crate) fn new_vec<'slot, T>(
        parser: &ParserState<'_>,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ChargedVec<T>> {
        SyntaxDiagnostic::new_vec(&parser.execution, span, parser.translator, out.reborrow())
    }
}

#[cfg(test)]
mod tests {
    use opeo::{ErrSlot, OResult, Out};
    use typol_core::{Diagnostic, DiagnosticCode, SourceId, SourceSpan, Span};
    use typol_i18n::{Language, Translator};
    use typol_test_support::{
        TestFailure as OpeoTestFailure, TestOpeoOutcome, test_assert_eq_early as test_assert_eq,
    };

    use super::SyntaxBudget;

    #[derive(Debug)]
    struct TestFailure;

    impl From<OpeoTestFailure> for TestFailure {
        fn from(_: OpeoTestFailure) -> Self {
            Self
        }
    }

    type TestResult<T = ()> = Result<T, TestFailure>;

    fn call<T>(
        slot: &mut ErrSlot<Diagnostic>,
        operation: impl for<'slot> FnOnce(Out<'slot, 'slot, Diagnostic>) -> OResult<'slot, T>,
    ) -> TestOpeoOutcome<T, Diagnostic> {
        match slot.try_call(operation) {
            Ok(value) => TestOpeoOutcome::Value(value),
            Err(caught) => TestOpeoOutcome::Error(caught.take()),
        }
    }

    #[test]
    fn syntax_frame_budget_fails_before_growth_and_detects_underflow() -> TestResult {
        let span = SourceSpan::new(SourceId::new(7), Span::empty());
        let mut errors = ErrSlot::<Diagnostic>::new();
        let mut budget = SyntaxBudget::new(1, Translator::new(Language::English));
        call(&mut errors, |out| budget.enter(span, out)).test_value()?;
        test_assert_eq!(
            call(&mut errors, |out| budget.enter(span, out))
                .test_error()?
                .code(),
            DiagnosticCode::SCHEMA_PARSE_DEPTH_LIMIT_EXCEEDED
        );
        call(&mut errors, |out| budget.leave(1, span, out)).test_value()?;
        let internal = call(&mut errors, |out| budget.leave(1, span, out)).test_error()?;
        test_assert_eq!(
            internal.code(),
            DiagnosticCode::SYNTAX_STORAGE_INVARIANT_FAILURE
        );
        let Some(context) = internal.context() else {
            return Err(TestFailure);
        };
        test_assert_eq!(context.subsystem(), "typol-syntax");
        test_assert_eq!(context.stage(), "parser-budget");
        test_assert_eq!(context.failure_point(), "frame-depth-underflow");
        call(&mut errors, |out| budget.enter(span, out)).test_value()?;

        Ok(())
    }
}
