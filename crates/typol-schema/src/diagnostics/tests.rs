use crate::{
    diagnostics::{BuildFailure, DiagnosticCollector, EmergencyDiagnostics},
    test_support::TestOperation,
};
use typol_core::{DiagnosticCode, ExecutionBudget, ExecutionLimits, SourceId, SourceSpan, Span};
use typol_syntax::AstAccessError;
use typol_test_support::*;

fn span(operations: &mut TestOperation, start: usize, end: usize) -> TestResult<SourceSpan> {
    let span = operations
        .span(|out| Span::new(start, end, out))
        .into_test_value()?;
    Ok(SourceSpan::new(SourceId::new(0), span))
}

#[test]
fn stable_order_uses_both_endpoints_and_terminal_stays_last() -> TestResult {
    let mut operations = TestOperation::new();
    let execution = operations
        .budget(|out| ExecutionBudget::new(ExecutionLimits::new(10_000, 10_000), out))
        .into_test_value()?;
    let fallback = span(&mut operations, 0, 0)?;
    let mut collector = operations
        .diagnostic(|out| DiagnosticCollector::new(10, fallback, &execution, out))
        .into_test_value()?;
    for (message, location) in [
        ("first at the same position", span(&mut operations, 3, 5)?),
        ("later endpoint", span(&mut operations, 3, 7)?),
        ("earlier endpoint", span(&mut operations, 3, 4)?),
        ("second at the same position", span(&mut operations, 3, 5)?),
        ("earliest", span(&mut operations, 1, 2)?),
    ] {
        collector.push(collector.make_semantic(message, location));
    }
    collector.abort(EmergencyDiagnostics::resource(span(&mut operations, 0, 0)?));
    let before = execution.steps();
    collector.push(collector.make_semantic("subsequent", span(&mut operations, 0, 0)?));
    let errors = collector.into_diagnostics();
    test_verify_eq!(execution.steps(), before);
    test_verify_eq!(
        errors
            .iter()
            .take(5)
            .map(|diagnostic| diagnostic.message())
            .collect::<alloc::vec::Vec<_>>(),
        [
            "earliest",
            "earlier endpoint",
            "first at the same position",
            "second at the same position",
            "later endpoint"
        ]
    );
    test_verify_eq!(
        errors.last().into_test_value()?.code(),
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
    );
    test_verify!(errors.last().into_test_value()?.message().is_empty());

    Ok(())
}

#[test]
fn build_failure_keeps_the_full_ast_access_cause() -> TestResult {
    let mut operations = TestOperation::new();
    let execution = operations
        .budget(|out| ExecutionBudget::new(ExecutionLimits::new(10_000, 10_000), out))
        .into_test_value()?;
    let fallback = span(&mut operations, 0, 1)?;
    let mut collector = operations
        .diagnostic(|out| DiagnosticCollector::new(4, fallback, &execution, out))
        .into_test_value()?;
    let cause = AstAccessError::InvalidTextRange {
        start: usize::MAX - 1,
        end: usize::MAX,
    };

    collector.report_failure(BuildFailure::from(cause));
    let diagnostics = collector.into_diagnostics();
    let context = diagnostics
        .last()
        .into_test_value()?
        .context()
        .into_test_value()?;
    test_verify_eq!(context.object_range(), Some((usize::MAX - 1, usize::MAX)));

    Ok(())
}
