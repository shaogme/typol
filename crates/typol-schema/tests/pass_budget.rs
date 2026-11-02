#[path = "support/schema_compile.rs"]
mod schema_support;

use typol_core::{DiagnosticCode, ExecutionBudget, ExecutionLimits, SourceId};
use typol_schema::{SchemaCompileOptions, SchemaLimits, SchemaSource};
use typol_syntax::{SchemaParseOptions, SchemaParser};
use typol_test_support::*;

struct DuplicateNameCase {
    source: &'static str,
    name: &'static str,
    message: &'static str,
}

#[test]
fn wide_names_without_defaults_need_budget_beyond_parsing() -> TestResult {
    let prefix = "shared_prefix_".repeat(8);
    let fields: String = (0..256)
        .rev()
        .map(|i| format!("{prefix}{i:06}: u8,"))
        .collect();
    let source = format!("entry struct Root {{ {fields} }}");
    let probe = schema_support::SchemaCompileTestSupport::budget(|out| {
        ExecutionBudget::new(ExecutionLimits::new(67_108_864, 67_108_864), out)
    })
    .test_value()?;
    let syntax_limits = SchemaLimits::default();
    drop(
        schema_support::SchemaCompileTestSupport::parse(|out| {
            SchemaParser::parse_str(
                SourceId::new(0),
                &source,
                &SchemaParseOptions::new(*syntax_limits.syntax(), syntax_limits.stream_limits()),
                &probe,
                out,
            )
        })
        .test_value()?,
    );
    let execution = ExecutionLimits::new(probe.steps() + source.len() * 2 + 5000, 67_108_864);
    let parser_budget = schema_support::SchemaCompileTestSupport::budget(|out| {
        ExecutionBudget::new(execution, out)
    })
    .test_value()?;
    drop(
        schema_support::SchemaCompileTestSupport::parse(|out| {
            SchemaParser::parse_str(
                SourceId::new(0),
                &source,
                &SchemaParseOptions::new(*syntax_limits.syntax(), syntax_limits.stream_limits()),
                &parser_budget,
                out,
            )
        })
        .test_value()?,
    );
    let errors = schema_support::SchemaCompileTestSupport::run_without_imports(
        SchemaCompileOptions::default().with_execution(execution),
        SchemaSource::from_str(b"root", &source),
    )
    .test_error()?;
    test_verify_eq!(
        errors.diagnostics().last().test_value()?.code(),
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
    );
    schema_support::SchemaCompileTestSupport::run_without_imports(
        SchemaCompileOptions::default(),
        SchemaSource::from_str(b"root", &source),
    )
    .test_value()?;

    Ok(())
}

#[test]
fn duplicate_names_keep_first_labels_and_declaration_order() -> TestResult {
    for case in [
        DuplicateNameCase {
            source: "entry struct Root { z: u8, a: u8, z: u8, a: u8, z: u8 }",
            name: "z",
            message: "duplicate field name in the same struct",
        },
        DuplicateNameCase {
            source: "enum E { Z, A, Z, A, Z } entry struct Root { value: E }",
            name: "Z",
            message: "duplicate variant name in the same enum",
        },
        DuplicateNameCase {
            source: "struct Z {} struct A {} struct Z {} struct A {} struct Z {} entry struct Root {}",
            name: "Z",
            message: "duplicate declaration name in the same module",
        },
    ] {
        let DuplicateNameCase {
            source,
            name,
            message,
        } = case;
        let errors = schema_support::SchemaCompileTestSupport::run_without_imports(
            SchemaCompileOptions::default(),
            SchemaSource::from_str(b"root", source),
        )
        .test_error()?;
        test_verify_eq!(errors.diagnostics().len(), 3);
        let mut last_start = 0;
        for diagnostic in errors.diagnostics() {
            test_verify_eq!(diagnostic.code(), DiagnosticCode::SCHEMA_SEMANTIC_ERROR);
            test_verify_eq!(diagnostic.message(), message);
            let primary = diagnostic.primary().test_value()?.span();
            test_verify!(primary.start() > last_start);
            last_start = primary.start();
            let earlier = diagnostic.labels().first().test_value()?.span().span();
            let duplicate_name = source.get(primary.start()..primary.end()).test_value()?;
            test_verify_eq!(
                source.get(earlier.start()..earlier.end()).test_value()?,
                duplicate_name
            );
            test_verify_eq!(earlier.start(), source.find(duplicate_name).test_value()?);
        }
        test_verify_eq!(
            errors
                .diagnostics()
                .first()
                .test_value()?
                .primary()
                .test_value()?
                .span()
                .len(),
            name.len()
        );
    }

    Ok(())
}

#[test]
fn variant_and_payload_duplicates_keep_diagnostic_limit_trigger_order() -> TestResult {
    let source = "enum E { V { a: u8, a: u8 }, V { b: u8, b: u8 } } entry struct Root { value: E }";
    let mut limits = SchemaLimits::default();
    limits.set_max_diagnostics(2);
    let errors = schema_support::SchemaCompileTestSupport::run_without_imports(
        SchemaCompileOptions::new(limits),
        SchemaSource::from_str(b"root", source),
    )
    .test_error()?;
    test_verify_eq!(errors.diagnostics().len(), 2);
    test_verify_eq!(
        errors.diagnostics().first().test_value()?.message(),
        "duplicate field name in the same struct"
    );
    let terminal = errors.diagnostics().last().test_value()?;
    test_verify_eq!(terminal.code(), DiagnosticCode::DIAGNOSTIC_LIMIT_REACHED);
    test_verify_eq!(
        terminal.primary().test_value()?.span().start(),
        source.rfind("V {").test_value()?
    );

    Ok(())
}
