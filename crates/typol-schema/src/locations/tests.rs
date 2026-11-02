use crate::{
    LocationFailure, LocationStatus,
    compiled::SchemaInternalCause,
    diagnostics::{BuildFailure, DiagnosticCollector, EmergencyDiagnostics},
    locations::SourceLocations,
    test_support::TestOperation,
};
use alloc::string::ToString;
use typol_core::{
    DiagnosticCode, ExcerptLimits, ExecutionBudget, ExecutionLimits, LineIndex, SliceSource,
    SourceId, SourceSpan, Span,
};
use typol_syntax::{
    AstAccessError, SchemaParseOptions, SchemaParser, SourcePositionIndex, SyntaxLimits,
};
use typol_test_support::*;

fn execution(operations: &mut TestOperation) -> TestResult<ExecutionBudget> {
    operations
        .budget(|out| ExecutionBudget::new(ExecutionLimits::new(67_108_864, 67_108_864), out))
        .into_test_value()
}

fn span(operations: &mut TestOperation, start: usize, end: usize) -> TestResult<SourceSpan> {
    let span = operations
        .span(|out| Span::new(start, end, out))
        .into_test_value()?;
    Ok(SourceSpan::new(SourceId::new(0), span))
}

fn parse_positions(source: &str, execution: &ExecutionBudget) -> TestResult<SourcePositionIndex> {
    let mut input = SliceSource::new(source.as_bytes());
    let parsed = TestOpeo::call(|out| {
        SchemaParser::parse_with_positions(
            SourceId::new(0),
            &mut input,
            &SchemaParseOptions::with_syntax(SyntaxLimits::default()),
            execution,
            out,
        )
    })
    .map_err(|_| TestFailure::new("schema position index parsing failed"))?;
    Ok(parsed.into_parts().1)
}

#[test]
fn scan_matches_unicode_crlf_lf_and_eof() -> TestResult {
    let mut operations = TestOperation::new();
    let source = "// Unicode comment🙂\r\nentry struct Root {\n\tvalue: string\n}";
    let execution = execution(&mut operations)?;
    let positions = parse_positions(source, &execution)?;
    let reference = operations
        .allocation(|out| LineIndex::new(source, out))
        .into_test_value()?;
    test_verify_eq!(positions.source_len(), source.len());
    for offset in [
        0,
        source.find("entry").into_test_value()?,
        source.find("Root").into_test_value()?,
        source.find("value").into_test_value()?,
        source.find('}').into_test_value()? + 1,
        source.len(),
    ] {
        test_verify_eq!(
            positions.location(offset),
            Some(
                operations
                    .core_location(|out| reference.location(offset, out))
                    .into_test_value()?
            )
        );
    }

    Ok(())
}

#[test]
fn location_failures_preserve_ordinary_errors_and_append_internal_tail() -> TestResult {
    let mut operations = TestOperation::new();
    for (invalid, expected_failure) in [
        (span(&mut operations, 1, 2)?, LocationFailure::InvalidOffset),
        (span(&mut operations, 0, 4)?, LocationFailure::InvalidOffset),
        (
            SourceSpan::new(
                SourceId::new(1),
                operations
                    .span(|out| Span::new(0, 0, out))
                    .into_test_value()?,
            ),
            LocationFailure::SourceUnavailable,
        ),
    ] {
        let execution = execution(&mut operations)?;
        let fallback = span(&mut operations, 0, 3)?;
        let mut collector = operations
            .diagnostic(|out| DiagnosticCollector::new(4, fallback, &execution, out))
            .into_test_value()?;
        collector.push(collector.make_semantic("type is invalid", invalid));
        let errors = SourceLocations::errors_for_sources(
            collector.into_diagnostics(),
            &[],
            &execution,
            ExcerptLimits::default(),
        );
        test_verify_eq!(errors.diagnostics().len(), 1);
        test_verify_eq!(
            errors.diagnostics().first().into_test_value()?.code(),
            DiagnosticCode::SCHEMA_SEMANTIC_ERROR
        );
        test_verify_eq!(
            errors.location_status(),
            LocationStatus::Unavailable(if expected_failure == LocationFailure::InvalidOffset {
                LocationFailure::SourceUnavailable
            } else {
                expected_failure
            })
        );
        test_verify!(errors.span_location(invalid).is_none());
    }

    Ok(())
}

#[test]
fn exhausted_cleanup_never_scans_or_changes_first_reason() -> TestResult {
    let mut operations = TestOperation::new();
    let execution = execution(&mut operations)?;
    let source = " ".repeat(100_000);
    let fallback = span(&mut operations, 0, source.len())?;
    let mut collector = operations
        .diagnostic(|out| DiagnosticCollector::new(4, fallback, &execution, out))
        .into_test_value()?;
    let invalid = span(&mut operations, 0, 1)?;
    collector.push(collector.make_semantic("type is invalid", invalid));
    collector.abort(EmergencyDiagnostics::allocation(span(
        &mut operations,
        0,
        source.len(),
    )?));
    let before = execution.steps();
    let errors = SourceLocations::errors_for_sources(
        collector.into_diagnostics(),
        &[],
        &execution,
        ExcerptLimits::default(),
    );
    test_verify_eq!(execution.steps(), before);
    test_verify_eq!(errors.diagnostics().len(), 2);
    test_verify_eq!(
        errors.diagnostics().get(1).into_test_value()?.code(),
        DiagnosticCode::ALLOCATION_FAILED
    );
    test_verify_eq!(
        errors.location_status(),
        LocationStatus::Unavailable(LocationFailure::AllocationFailure)
    );
    drop(errors);
    test_verify!(execution.workspace_bytes() < 100);

    Ok(())
}

#[test]
fn public_compile_errors_keep_structured_ast_access_cause() -> TestResult {
    let mut operations = TestOperation::new();
    let execution = execution(&mut operations)?;
    let fallback = span(&mut operations, 0, 4)?;
    let mut collector = operations
        .diagnostic(|out| DiagnosticCollector::new(4, fallback, &execution, out))
        .into_test_value()?;
    let cause = AstAccessError::InvalidListRange {
        start: usize::MAX - 1,
        end: usize::MAX,
    };
    collector.report_failure(BuildFailure::from(cause));

    let errors = SourceLocations::errors_for_sources(
        collector.into_diagnostics(),
        &[],
        &execution,
        ExcerptLimits::default(),
    );
    test_verify_eq!(
        errors.internal_cause(),
        Some(SchemaInternalCause::AstAccess(cause))
    );
    test_verify!(errors.to_string().contains(&usize::MAX.to_string()));

    Ok(())
}
