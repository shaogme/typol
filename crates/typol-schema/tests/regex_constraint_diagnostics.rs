#[path = "support/schema_compile.rs"]
mod support;

use typol_test_support::*;

use typol_core::{DiagnosticCode, ExecutionBudget, ExecutionLimits, SourceId};
use typol_schema::SchemaLimits;
use typol_syntax::{SchemaParseOptions, SchemaParser};

#[test]
fn maps_invalid_pattern_to_redacted_schema_diagnostic() -> TestResult {
    let limits = SchemaLimits::default();
    let source = r#"entry struct Settings { token: string @regex("secret[") }"#;
    let diagnostic =
        support::SchemaCompileTestSupport::compile(31, source, &limits).test_error()?;

    let first = diagnostic.diagnostics().first().test_value()?;
    test_verify_eq!(first.code(), DiagnosticCode::REGEX_SYNTAX_ERROR);
    test_verify!(first.primary().is_some());
    test_verify!(!first.message().contains("secret"));
    test_verify!(!first.message().contains("["));
    let context = first.context().test_value()?;
    test_verify_eq!(context.subsystem(), "typol-schema");
    test_verify_eq!(context.stage(), "regex-pattern");
    test_verify!(context.failure_point().starts_with("pattern-start-offset"));
    test_verify_eq!(diagnostic.regex_pattern_start_offset(0), Some(7));
    Ok(())
}

#[test]
fn rejects_incompatible_types_and_invalid_argument_shapes() -> TestResult {
    let execution = support::SchemaCompileTestSupport::budget(|out| {
        ExecutionBudget::new(ExecutionLimits::new(67_108_864, 67_108_864), out)
    })
    .test_value()?;

    let limits = SchemaLimits::default();
    let invalid_target = r#"entry struct Settings { enabled: bool @regex("true") }"#;
    let diagnostic =
        support::SchemaCompileTestSupport::compile(32, invalid_target, &limits).test_error()?;
    test_verify_eq!(
        diagnostic.diagnostics().first().test_value()?.code(),
        DiagnosticCode::SCHEMA_SEMANTIC_ERROR
    );

    let invalid_shape = r#"entry struct Settings { name: string @regex(pattern = "name") }"#;
    let errors = support::SchemaCompileTestSupport::parse(|out| {
        SchemaParser::parse_str(
            SourceId::new(33),
            invalid_shape,
            &SchemaParseOptions::new(*limits.syntax(), limits.stream_limits()),
            &execution,
            out,
        )
    })
    .test_error()?;
    test_verify_eq!(
        errors.diagnostics().first().test_value()?.code(),
        DiagnosticCode::SCHEMA_SYNTAX_ERROR
    );
    Ok(())
}

#[test]
fn a_failure_returns_no_partial_collection() -> TestResult {
    let limits = SchemaLimits::default();
    let source = r#"entry struct Settings {
        first: string @regex("valid"),
        second: string @regex("invalid["),
    }"#;
    let diagnostic =
        support::SchemaCompileTestSupport::compile(34, source, &limits).test_error()?;

    test_verify_eq!(
        diagnostic.diagnostics().first().test_value()?.code(),
        DiagnosticCode::REGEX_SYNTAX_ERROR
    );
    Ok(())
}
