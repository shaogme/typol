#[path = "support/schema_compile.rs"]
mod support;

use typol_test_support::*;

use typol_core::DiagnosticCode;
use typol_policy::{RegexExecutionLimits, RegexLimits, RegexOptions};
use typol_schema::{SchemaCompileErrors, SchemaCompileOptions, SchemaLimits};
use typol_semantic::CompiledSchema;

fn compile_regex(
    source_id: u32,
    source: &str,
    limits: &RegexLimits,
) -> support::TestOpeoOutcome<CompiledSchema, SchemaCompileErrors> {
    let regex = RegexOptions::new(*limits, RegexExecutionLimits::default());
    support::SchemaCompileTestSupport::compile_with_options(
        source_id,
        source,
        SchemaCompileOptions::new(SchemaLimits::default()).with_regex(regex),
    )
}

#[test]
fn aggregate_limits_allow_exact_capacity_and_reject_one_below() -> TestResult {
    let defaults = RegexLimits::default();
    let source = r#"entry struct Settings {
        first: string @regex("a"),
        second: string @regex("b"),
    }"#;
    let baseline = compile_regex(21, source, &defaults).test_value()?;

    let mut at_limits = defaults;
    at_limits.set_max_regex_total_pattern_bytes(baseline.regex_counts().pattern_bytes);
    at_limits.set_max_regex_count(baseline.regex_counts().programs);
    at_limits.set_max_regex_instructions_total(baseline.regex_counts().instructions);
    at_limits.set_max_regex_ranges_total(baseline.regex_counts().ranges);
    let at_limit = compile_regex(21, source, &at_limits).test_value()?;
    test_verify_eq!(
        at_limit.regex_counts().pattern_bytes,
        baseline.regex_counts().pattern_bytes
    );
    test_verify_eq!(
        at_limit.regex_counts().programs,
        baseline.regex_counts().programs
    );
    test_verify_eq!(
        at_limit.regex_counts().instructions,
        baseline.regex_counts().instructions
    );
    test_verify_eq!(
        at_limit.regex_counts().ranges,
        baseline.regex_counts().ranges
    );

    let mut limits = at_limits;
    limits.set_max_regex_total_pattern_bytes(baseline.regex_counts().pattern_bytes - 1);
    test_verify_eq!(
        compile_regex(21, source, &limits)
            .test_error()?
            .diagnostics()
            .first()
            .test_value()?
            .code(),
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
    );

    let mut limits = at_limits;
    limits.set_max_regex_count(baseline.regex_counts().programs - 1);
    test_verify_eq!(
        compile_regex(21, source, &limits)
            .test_error()?
            .diagnostics()
            .first()
            .test_value()?
            .code(),
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
    );

    let mut limits = at_limits;
    limits.set_max_regex_instructions_total(baseline.regex_counts().instructions - 1);
    test_verify_eq!(
        compile_regex(21, source, &limits)
            .test_error()?
            .diagnostics()
            .first()
            .test_value()?
            .code(),
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
    );

    let mut limits = at_limits;
    limits.set_max_regex_ranges_total(baseline.regex_counts().ranges - 1);
    test_verify_eq!(
        compile_regex(21, source, &limits)
            .test_error()?
            .diagnostics()
            .first()
            .test_value()?
            .code(),
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
    );
    Ok(())
}

#[test]
fn enforces_per_pattern_limits_before_storing_a_constraint() -> TestResult {
    let source = r#"entry struct Settings { name: string @regex("ab") }"#;
    let defaults = RegexLimits::default();

    let mut limits = defaults;
    limits.set_max_regex_pattern_bytes(1);
    test_verify_eq!(
        compile_regex(22, source, &limits)
            .test_error()?
            .diagnostics()
            .first()
            .test_value()?
            .code(),
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
    );

    let mut limits = defaults;
    limits.set_max_regex_instructions_per_pattern(1);
    test_verify_eq!(
        compile_regex(22, source, &limits)
            .test_error()?
            .diagnostics()
            .first()
            .test_value()?
            .code(),
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
    );

    let mut limits = defaults;
    limits.set_max_regex_ranges_per_pattern(0);
    test_verify_eq!(
        compile_regex(22, source, &limits)
            .test_error()?
            .diagnostics()
            .first()
            .test_value()?
            .code(),
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
    );
    Ok(())
}

#[test]
fn attempted_bytes_include_syntax_failures_but_program_count_does_not() -> TestResult {
    let source = r#"entry struct Root {
        invalid: string @regex("["),
        valid: string @regex("a"),
    }"#;
    let mut limits = RegexLimits::default();
    limits.set_max_regex_total_pattern_bytes(1);
    let errors = compile_regex(23, source, &limits).test_error()?;
    test_verify_eq!(errors.diagnostics().len(), 2);
    test_verify_eq!(
        errors.diagnostics().first().test_value()?.code(),
        DiagnosticCode::REGEX_SYNTAX_ERROR
    );
    test_verify_eq!(
        errors.diagnostics().get(1).test_value()?.code(),
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
    );

    let mut limits = RegexLimits::default();
    limits.set_max_regex_count(1);
    let errors = compile_regex(24, source, &limits).test_error()?;
    test_verify_eq!(errors.diagnostics().len(), 1);
    test_verify_eq!(
        errors.diagnostics().first().test_value()?.code(),
        DiagnosticCode::REGEX_SYNTAX_ERROR
    );
    Ok(())
}

#[test]
fn exhausted_instruction_quota_precedes_later_syntax_errors() -> TestResult {
    let source = r#"entry struct Root {
        valid: string @regex("a"),
        invalid: string @regex("["),
    }"#;
    let mut limits = RegexLimits::default();
    limits.set_max_regex_instructions_total(2);
    let errors = compile_regex(25, source, &limits).test_error()?;
    test_verify_eq!(errors.diagnostics().len(), 1);
    test_verify_eq!(
        errors.diagnostics().first().test_value()?.code(),
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
    );

    Ok(())
}

#[test]
fn exhausted_range_quota_preserves_later_syntax_errors() -> TestResult {
    let source = r#"entry struct Root {
        valid: string @regex("a"),
        invalid: string @regex("["),
    }"#;
    let mut limits = RegexLimits::default();
    limits.set_max_regex_ranges_total(1);
    let errors = compile_regex(26, source, &limits).test_error()?;
    test_verify_eq!(errors.diagnostics().len(), 1);
    test_verify_eq!(
        errors.diagnostics().first().test_value()?.code(),
        DiagnosticCode::REGEX_SYNTAX_ERROR
    );
    Ok(())
}

#[test]
fn syntax_errors_are_preserved_when_range_quota_is_zero() -> TestResult {
    let source = r#"entry struct Root { invalid: string @regex("[") }"#;
    let mut limits = RegexLimits::default();
    limits.set_max_regex_ranges_total(0);
    let errors = compile_regex(27, source, &limits).test_error()?;

    test_verify_eq!(errors.diagnostics().len(), 1);
    test_verify_eq!(
        errors.diagnostics().first().test_value()?.code(),
        DiagnosticCode::REGEX_SYNTAX_ERROR
    );
    Ok(())
}
