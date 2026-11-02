use crate::{
    Keyword, LexEvent, Symbol, TokenKind,
    error::SyntaxDiagnostic,
    parser::context::{ParserControl, ParserState},
};
use opeo::{OResult, Out};
use typol_core::{Diagnostic, DiagnosticCode};

enum RecoveryContext {
    Separated { depth: usize },
    Module { depth: usize, failed_at: usize },
    TopLevel { failed_at: usize },
}

pub(crate) enum RecoveryBoundary {
    Separated { depth: usize },
    Module { depth: usize, failed_at: usize },
}

impl RecoveryContext {
    fn depth(&self) -> usize {
        match *self {
            Self::Separated { depth } | Self::Module { depth, .. } => depth,
            Self::TopLevel { .. } => 0,
        }
    }
}

pub(crate) struct RecoveryParser;

impl RecoveryParser {
    pub(crate) fn recover_member<'slot, T>(
        parser: &mut ParserState<'_>,
        boundary: RecoveryBoundary,
        attempt: Result<T, Diagnostic>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Option<T>> {
        match attempt {
            Ok(member) => Ok(Some(member)),
            Err(diagnostic) => {
                if parser.stopped
                    || !parser.trusted
                    || !matches!(
                        diagnostic.code(),
                        DiagnosticCode::SCHEMA_SYNTAX_ERROR | DiagnosticCode::LEXICAL_ERROR
                    )
                {
                    return Err(out.reborrow().fail(diagnostic));
                }
                ParserControl::push_diagnostic(parser, diagnostic, out.reborrow())?;
                if parser.stopped {
                    return Err(out.reborrow().fail(ParserControl::stopped_error(parser)));
                }
                let context = match boundary {
                    RecoveryBoundary::Separated { depth } => RecoveryContext::Separated { depth },
                    RecoveryBoundary::Module { depth, failed_at } => {
                        RecoveryContext::Module { depth, failed_at }
                    }
                };
                RecoveryParser::recover(parser, context, out.reborrow())?;
                Ok(None)
            }
        }
    }

    pub(crate) fn recover_fields<'slot>(
        parser: &mut ParserState<'_>,
        base_depth: usize,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ()> {
        RecoveryParser::recover(
            parser,
            RecoveryContext::Separated { depth: base_depth },
            out.reborrow(),
        )
    }

    pub(crate) fn recover_variants<'slot>(
        parser: &mut ParserState<'_>,
        base_depth: usize,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ()> {
        RecoveryParser::recover(
            parser,
            RecoveryContext::Separated { depth: base_depth },
            out.reborrow(),
        )
    }

    pub(crate) fn recover_module_member<'slot>(
        parser: &mut ParserState<'_>,
        base_depth: usize,
        failed_at: usize,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ()> {
        RecoveryParser::recover(
            parser,
            RecoveryContext::Module {
                depth: base_depth,
                failed_at,
            },
            out.reborrow(),
        )
    }

    pub(crate) fn recover_top_level<'slot>(
        parser: &mut ParserState<'_>,
        failed_at: usize,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ()> {
        RecoveryParser::recover(
            parser,
            RecoveryContext::TopLevel { failed_at },
            out.reborrow(),
        )
    }

    fn recover<'slot>(
        parser: &mut ParserState<'_>,
        context: RecoveryContext,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ()> {
        let base_depth = context.depth();
        while !parser.stopped && parser.trusted {
            let info = ParserControl::peek(parser, out.reborrow())?;
            if info.kind == TokenKind::Eof {
                return Ok(());
            }
            let top_level_brace = match &context {
                RecoveryContext::TopLevel { .. }
                    if info.kind == TokenKind::Symbol(Symbol::RightBrace) =>
                {
                    parser
                        .delimiters
                        .iter()
                        .rposition(|frame| frame.close == Symbol::RightBrace)
                }
                _ => None,
            };
            if let Some(index) = top_level_brace {
                let released = parser.delimiters.len() - index;
                parser.budget.leave(released, info.span, out.reborrow())?;
                parser.delimiters.truncate(index);
                match parser.cursor.bump(out.reborrow())? {
                    LexEvent::Token(_) => {}
                    _ => {
                        return Err(out.reborrow().fail(parser_internal_error!(
                            "unexpected-event-while-recovering-top-level-brace",
                            info.span,
                        )));
                    }
                }
                parser.last_end = info.span.span().end();
                continue;
            }
            // Only this context’s brace boundary may discard an unmatched parenthesis or square bracket inside it.
            let closes_context =
                if info.kind == TokenKind::Symbol(Symbol::RightBrace) && base_depth > 0 {
                    let mut matched = None;
                    for (index, frame) in parser.delimiters.iter().enumerate().rev() {
                        parser.execution.consume_with(1, out.reborrow(), |error| {
                            SyntaxDiagnostic::budget(error, info.span)
                        })?;
                        if frame.close == Symbol::RightBrace {
                            matched = Some(index);
                            break;
                        }
                    }
                    matched == Some(base_depth - 1)
                } else {
                    false
                };
            if closes_context {
                let Some(released) = parser.delimiters.len().checked_sub(base_depth) else {
                    return Err(out.reborrow().fail(parser_internal_error!(
                        "module-recovery-delimiter-depth-underflow",
                        info.span,
                    )));
                };
                parser.budget.leave(released, info.span, out.reborrow())?;
                parser.delimiters.truncate(base_depth);
                return Ok(());
            }
            if ParserControl::current_depth(parser) == base_depth {
                match context {
                    RecoveryContext::Separated { .. }
                        if info.kind == TokenKind::Symbol(Symbol::Comma) =>
                    {
                        ParserControl::bump(parser, out.reborrow())?;
                        return Ok(());
                    }
                    RecoveryContext::Module { failed_at, .. }
                        if is_declaration_start(info.kind)
                            && info.span.span().start() != failed_at =>
                    {
                        return Ok(());
                    }
                    RecoveryContext::TopLevel { failed_at }
                        if is_top_level_start(info.kind)
                            && info.span.span().start() != failed_at =>
                    {
                        return Ok(());
                    }
                    _ => {}
                }
            }
            ParserControl::bump(parser, out.reborrow())?;
        }
        Ok(())
    }
}

fn is_declaration_start(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Keyword(Keyword::Mod | Keyword::Type | Keyword::Struct | Keyword::Enum)
    )
}

fn is_top_level_start(kind: TokenKind) -> bool {
    is_declaration_start(kind)
        || matches!(kind, TokenKind::Keyword(Keyword::Extern | Keyword::Entry))
}

#[cfg(test)]
mod tests {
    use crate::{
        InputConfig, InputLimits, StreamLimits, SyntaxLimits,
        parser::{context::ParserControl, recovery::RecoveryParser},
    };
    use opeo::{ErrSlot, OResult, Out};
    use typol_core::{
        BudgetError, Diagnostic, DiagnosticCode, ExecutionBudget, ExecutionLimits, SliceSource,
        SourceId,
    };

    #[derive(Debug)]
    struct TestFailure;

    type TestResult<T = ()> = Result<T, TestFailure>;

    fn call<T>(
        slot: &mut ErrSlot<Diagnostic>,
        operation: impl for<'slot> FnOnce(Out<'slot, 'slot, Diagnostic>) -> OResult<'slot, T>,
    ) -> Result<T, Diagnostic> {
        match slot.try_call(operation) {
            Ok(value) => Ok(value),
            Err(caught) => Err(caught.take()),
        }
    }

    trait TestValue<T> {
        fn test_value(self) -> TestResult<T>;
    }

    impl<T, E> TestValue<T> for Result<T, E> {
        fn test_value(self) -> TestResult<T> {
            self.map_err(|_| TestFailure)
        }
    }

    impl<T> TestValue<T> for Option<T> {
        fn test_value(self) -> TestResult<T> {
            self.ok_or(TestFailure)
        }
    }

    macro_rules! test_assert {
        ($condition:expr $(, $($message:tt)*)?) => {{
            if !$condition {
                return Err(TestFailure);
            }
        }};
    }

    macro_rules! test_assert_eq {
        ($left:expr, $right:expr $(, $($message:tt)*)?) => {{
            let left = &$left;
            let right = &$right;
            if left != right {
                return Err(TestFailure);
            }
        }};
    }

    #[test]
    fn exhausted_recovery_work_stops_before_consuming_another_event() -> TestResult {
        let step_limit = 67_108_864;
        let mut budget_errors = ErrSlot::<BudgetError>::new();
        let execution = match budget_errors
            .try_call(|out| ExecutionBudget::new(ExecutionLimits::new(step_limit, 67_108_864), out))
        {
            Ok(execution) => execution,
            Err(_) => return Err(TestFailure),
        };

        let mut parser_input_1 = SliceSource::new(("u8 u8").as_bytes());
        let mut errors = ErrSlot::<Diagnostic>::new();
        let mut parser = call(&mut errors, |out| {
            ParserControl::create(
                InputConfig {
                    source_id: SourceId::new(7),
                    source: &mut parser_input_1,
                    resources: &InputLimits::default(),
                    stream: StreamLimits::default(),
                    budget: &execution,
                },
                &SyntaxLimits::default(),
                out,
            )
        })
        .test_value()?;
        let remaining_steps = step_limit.saturating_sub(execution.steps());
        budget_errors
            .try_call(|out| execution.consume(remaining_steps, out))
            .test_value()?;
        let recovery_error = match call(&mut errors, |out| {
            RecoveryParser::recover_top_level(&mut parser, 0, out)
        }) {
            Ok(()) => return Err(TestFailure),
            Err(error) => error,
        };
        test_assert_eq!(
            recovery_error.code(),
            DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
        );
        call(&mut errors, |out| {
            ParserControl::push_diagnostic(&mut parser, recovery_error, out)
        })
        .test_value()?;
        test_assert!(parser.stopped);
        test_assert_eq!(parser.last_end, 0);
        let errors = parser.diagnostics.finish().test_value()?;
        test_assert_eq!(errors.diagnostics().len(), 1);
        test_assert_eq!(
            errors.diagnostics().first().test_value()?.code(),
            DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
        );

        Ok(())
    }
}
