use typol_core::{DiagnosticContext, DiagnosticContextDescriptor, SourceId, SourceSpan, Span};
use typol_semantic::{CopyError, EvalError, EvalFailure, RegexExecutionError};

static TEST_CONTEXT_DESCRIPTOR: DiagnosticContextDescriptor =
    DiagnosticContextDescriptor::new("typol-semantic", "context-test", "copy-failure");
static TEST_CONTEXT: DiagnosticContext = DiagnosticContext::new(&TEST_CONTEXT_DESCRIPTOR);

#[test]
fn copy_internal_error_exposes_its_static_failure_context() {
    let error = CopyError::internal(TEST_CONTEXT);

    assert!(error.is_internal());
    assert_eq!(
        error.context().map(|context| context.subsystem()),
        Some("typol-semantic")
    );
    assert_eq!(
        error.context().map(|context| context.stage()),
        Some("context-test")
    );
    assert_eq!(
        error.context().map(|context| context.failure_point()),
        Some("copy-failure")
    );
}

#[test]
fn regex_error_keeps_its_failure_context_and_constraint_span() {
    let span = SourceSpan::new(SourceId::new(3), Span::point(8));
    let error = RegexExecutionError::InternalAt {
        context: TEST_CONTEXT,
        source_span: None,
    }
    .with_source_span(span);

    assert!(error.is_internal());
    assert_eq!(error.context(), Some(TEST_CONTEXT));
    assert_eq!(error.source_span(), Some(span));
}

#[test]
fn unlocated_regex_error_does_not_invent_a_source_span() {
    let error = RegexExecutionError::InternalAt {
        context: TEST_CONTEXT,
        source_span: None,
    };

    assert_eq!(error.source_span(), None);
}

#[test]
fn located_evaluation_error_keeps_its_classification_and_source_span() {
    let span = SourceSpan::new(SourceId::new(2), Span::point(5));
    let error = EvalError::Resource.with_source_span(span);

    assert_eq!(error.kind(), EvalFailure::Resource);
    assert_eq!(error.source_span(), Some(span));
    assert_eq!(error.context(), None);
}
