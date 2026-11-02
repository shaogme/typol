use super::loader::LoadSupport;
use crate::TypolStage;
use typol_core::{
    DiagnosticContext, SourceId, SourceReadError, SourceReadErrorKind, diagnostic_context,
};
use typol_std::{HostError, HostErrorKind, HostOperation};
use typol_test_support::{
    TestExpect, TestExtract, TestResult, test_verify as assert, test_verify_eq as assert_eq,
};

static READER_FAILURE_CONTEXT: DiagnosticContext =
    diagnostic_context!("typol-std", "read-source", "host-reader-call",);

fn host_error(kind: SourceReadErrorKind) -> TestResult<HostError> {
    let source_error = SourceReadError::new(kind).with_input(SourceId::new(7), 19);
    let error = LoadSupport::read_source_error(source_error);
    assert_eq!(error.stage(), TypolStage::ConfigRead);
    Ok(*error.host_error().test_value()?)
}

#[test]
fn source_read_errors_keep_their_host_categories_and_location() -> TestResult {
    let cases = [
        (
            SourceReadErrorKind::ResourceLimitExceeded,
            HostErrorKind::ResourceLimitExceeded,
        ),
        (
            SourceReadErrorKind::AllocationFailed,
            HostErrorKind::AllocationFailed,
        ),
        (
            SourceReadErrorKind::CapacityExceeded,
            HostErrorKind::CapacityExceeded,
        ),
        (SourceReadErrorKind::HostFailure, HostErrorKind::ReadFailure),
        (
            SourceReadErrorKind::InternalInvariantFailure,
            HostErrorKind::InternalError,
        ),
    ];
    for (source_kind, host_kind) in cases {
        let error = host_error(source_kind)?;
        assert_eq!(error.operation(), HostOperation::ConfigRead);
        assert_eq!(error.kind(), host_kind);
        assert_eq!(error.source_id(), Some(SourceId::new(7)));
        assert_eq!(error.offset(), Some(19));
    }
    Ok(())
}

#[test]
fn source_read_invariant_uses_source_or_generic_context() -> TestResult {
    let source_error = SourceReadError::new(SourceReadErrorKind::InternalInvariantFailure)
        .with_context(READER_FAILURE_CONTEXT);
    let mapped = LoadSupport::read_source_error(source_error);
    let host = mapped.host_error().test_value()?;
    assert_eq!(
        host.source_context()
            .test_expect("underlying read context should be preserved")?
            .failure_point(),
        "host-reader-call"
    );

    let fallback = host_error(SourceReadErrorKind::InternalInvariantFailure)?;
    assert_eq!(
        fallback
            .source_context()
            .test_expect("a generic invariant context should be provided when context is missing")?
            .failure_point(),
        "source-read-invariant"
    );
    Ok(())
}

#[test]
fn host_read_failure_display_keeps_location_without_native_details() -> TestResult {
    let error = host_error(SourceReadErrorKind::HostFailure)?;
    let display = error.to_string();
    assert!(display.contains("source ID 7"));
    assert!(display.contains("offset 19"));
    assert!(display.contains("read failed"));
    assert!(!display.contains("ReadFailure"));
    assert!(!display.contains("reader-secret"));
    Ok(())
}
