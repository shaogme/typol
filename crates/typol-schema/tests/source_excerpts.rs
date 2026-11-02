#[path = "support/schema_compile.rs"]
mod schema_support;

use typol_core::{DiagnosticCode, ExcerptLimits, ExcerptStatus, SourceId};
use typol_schema::{SchemaCompileOptions, SchemaLimits, SchemaSource};
use typol_test_support::*;

#[test]
fn failed_compilation_masks_defaults_before_source_drop() -> TestResult {
    let errors = {
        let source = String::from(
            r#"type Secret = string @secret; entry struct Root { password: Secret = "PRIVATE-SENTINEL", value: u8 @min(2) = 1 }"#,
        );
        schema_support::SchemaCompileTestSupport::run_without_imports(
            SchemaCompileOptions::default(),
            SchemaSource::from_str(b"private-path", &source),
        )
        .test_error()?
    };
    let text = errors
        .excerpts()
        .records()
        .iter()
        .map(|record| record.text())
        .collect::<Vec<_>>()
        .join("\n");
    test_assert!(text.contains("@min(2)"))?;
    test_assert!(text.contains("<redacted>"))?;
    test_assert!(!text.contains("PRIVATE-SENTINEL"))?;
    test_assert!(!format!("{errors:?} {:?}", errors.excerpts()).contains("PRIVATE"))?;
    let primary = errors
        .diagnostics()
        .first()
        .test_value()?
        .primary()
        .test_value()?;
    test_ne!(
        errors.excerpts().span_status(primary),
        ExcerptStatus::Unavailable
    )?;
    Ok(())
}

#[test]
fn successful_rule_index_is_bounded_and_does_not_retain_whole_source() -> TestResult {
    let mut source = String::from(
        "entry struct Root {\n value: u8 @min(2),\n password: string @secret = \"PRIVATE-SENTINEL\"\n}\n",
    );
    source.push_str(&"// UNREFERENCED-SENTINEL\n".repeat(500));
    let mut limits = SchemaLimits::default();
    let excerpts = ExcerptLimits::new(4096, 128, 16, 0);
    limits.set_excerpts(excerpts);
    limits.set_max_schema_files(1);
    let schema = schema_support::SchemaCompileTestSupport::run_without_imports(
        SchemaCompileOptions::new(limits),
        SchemaSource::from_str(b"source", &source),
    )
    .test_value()?;
    drop(source);
    test_assert!(schema.excerpts().storage_bytes() <= excerpts.max_bytes())?;
    test_assert!(schema.excerpts().records().len() <= excerpts.max_records())?;
    let text = schema
        .excerpts()
        .records()
        .iter()
        .map(|record| record.text())
        .collect::<Vec<_>>()
        .join("\n");
    test_assert!(text.contains("@min(2)"))?;
    test_assert!(!text.contains("PRIVATE-SENTINEL"))?;
    test_assert!(!text.contains("UNREFERENCED-SENTINEL"))?;
    test_assert!(
        schema
            .excerpts()
            .records()
            .iter()
            .all(|record| record.source_id() == SourceId::new(0))
    )?;
    Ok(())
}

#[test]
fn invalid_ast_and_zero_snapshot_budget_preserve_schema_diagnostics() -> TestResult {
    for source in [
        r#"entry struct Root { password: string @secret = "PRIVATE-SENTINEL" "#,
        "entry struct Root { value: Missing }",
    ] {
        let mut limits = SchemaLimits::default();
        limits.set_excerpts(ExcerptLimits::new(0, 0, 0, 0));
        let errors = schema_support::SchemaCompileTestSupport::run_without_imports(
            SchemaCompileOptions::new(limits),
            SchemaSource::from_str(b"source", source),
        )
        .test_error()?;
        test_eq!(errors.excerpts().status(), ExcerptStatus::Unavailable)?;
        test_assert!(!errors.diagnostics().is_empty())?;
        test_assert!(
            errors
                .diagnostics()
                .iter()
                .all(|diagnostic| diagnostic.code() != DiagnosticCode::ALLOCATION_FAILED)
        )?;
    }
    Ok(())
}
