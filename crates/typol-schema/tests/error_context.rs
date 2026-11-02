use typol_core::{BudgetError, DiagnosticCode, DiagnosticContext, diagnostic_context};
use typol_schema::{LocationFailure, LocationStatus, SchemaCompileErrors};
use typol_test_support::*;

static BUDGET_FAILURE_CONTEXT: DiagnosticContext =
    diagnostic_context!("test", "schema-init", "context-preserved");

#[test]
fn inline_budget_failures_keep_context_without_inventing_a_span() -> TestResult {
    let errors =
        SchemaCompileErrors::budget_failure(BudgetError::invariant(&BUDGET_FAILURE_CONTEXT));
    let diagnostic = errors.diagnostics().first().test_value()?;
    let context = diagnostic.context().test_value()?;

    test_verify_eq!(diagnostic.code(), DiagnosticCode::INTERNAL_ERROR);
    test_verify!(diagnostic.primary().is_none());
    test_verify_eq!(context.subsystem(), "test");
    test_verify_eq!(context.stage(), "schema-init");
    test_verify_eq!(context.failure_point(), "context-preserved");
    test_verify_eq!(errors.internal_context(), Some(context));
    test_verify_eq!(
        errors.location_status(),
        LocationStatus::Unavailable(LocationFailure::InternalError)
    );
    Ok(())
}
