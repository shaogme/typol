#[path = "support/schema_compile.rs"]
mod schema_support;

use typol_core::DiagnosticCode;
use typol_schema::{
    ResolverError, SchemaCompileOptions, SchemaLimits, SchemaResolveRequest, SchemaResolver,
    SchemaSource,
    opeo::{OResult, Out},
};
use typol_test_support::*;

#[derive(Default)]
struct Imports {
    source: OwnedReadSource,
}

impl SchemaResolver for Imports {
    fn resolve<'src, 'slot>(
        &'src mut self,
        request: SchemaResolveRequest<'_>,
        out: Out<'slot, '_, ResolverError>,
    ) -> OResult<'slot, SchemaSource<'src>> {
        let module = request.module_name;
        match module {
            "events" => {
                self.source.replace(
                    r#"enum Event { Named(string @regex("^name$")), Detail { code: string @regex("[0-9]+"), }, }"#,
                );
                Ok(SchemaSource::from_source(b"events", &mut self.source))
            }
            _ => Err(out.fail(ResolverError::NotFound)),
        }
    }
}

#[test]
fn collects_constraints_across_files_aliases_modules_and_enum_payloads() -> TestResult {
    let source = r#"extern mod events; mod labels { type Prefix = string @regex("^svc-"); struct Service { name: string @regex("[a-z]+"), } } entry struct Settings { name: labels::Prefix, event: events::Event }"#;
    let schema = schema_support::SchemaCompileTestSupport::run(
        SchemaCompileOptions::new(SchemaLimits::default()),
        SchemaSource::from_str(b"root", source),
        &mut Imports::default(),
        None,
    )
    .test_value()?;
    let counts = schema.regex_counts();
    test_verify_eq!(counts.programs, 4);
    test_verify_eq!(
        counts.pattern_bytes,
        "^svc-".len() + "[a-z]+".len() + "^name$".len() + "[0-9]+".len()
    );
    test_verify_eq!(
        counts.instructions,
        schema
            .regex_programs()
            .map(|program| program.regex().instruction_count())
            .sum()
    );
    test_verify_eq!(
        counts.ranges,
        schema
            .regex_programs()
            .map(|program| program.regex().range_count())
            .sum()
    );
    for program in schema.regex_programs() {
        test_verify!(schema.span_location(program.span()).is_some());
    }
    Ok(())
}

#[test]
fn aggregate_regex_limits_include_imported_schema_programs() -> TestResult {
    let source = r#"extern mod events;
        entry struct Settings {
            event: events::Event,
            name: string @regex("root"),
        }"#;
    let defaults = SchemaLimits::default();
    let baseline = schema_support::SchemaCompileTestSupport::run(
        SchemaCompileOptions::new(defaults),
        SchemaSource::from_str(b"root", source),
        &mut Imports::default(),
        None,
    )
    .test_value()?;
    test_verify_eq!(baseline.regex_counts().programs, 3);

    let mut limits = defaults;
    limits.set_max_regex_count(baseline.regex_counts().programs - 1);
    let errors = schema_support::SchemaCompileTestSupport::run(
        SchemaCompileOptions::new(limits),
        SchemaSource::from_str(b"root", source),
        &mut Imports::default(),
        None,
    )
    .test_error()?;
    test_verify!(
        errors
            .diagnostics()
            .iter()
            .any(|diagnostic| { diagnostic.code() == DiagnosticCode::RESOURCE_LIMIT_EXCEEDED })
    );
    Ok(())
}

#[test]
fn schema_without_regex_has_no_programs() -> TestResult {
    let schema = schema_support::SchemaCompileTestSupport::run(
        SchemaCompileOptions::new(SchemaLimits::default()),
        SchemaSource::from_str(b"root", "entry struct Root {}"),
        &mut Imports::default(),
        None,
    )
    .test_value()?;
    test_verify_eq!(schema.regex_counts().programs, 0);
    test_verify_eq!(schema.regex_programs().count(), 0);
    Ok(())
}

#[test]
fn regex_on_resolved_non_string_alias_is_rejected() -> TestResult {
    let errors = schema_support::SchemaCompileTestSupport::run(
        SchemaCompileOptions::new(SchemaLimits::default()),
        SchemaSource::from_str(
            b"root",
            "type Flag = bool; entry struct Root { value: Flag @regex(\"x\") }",
        ),
        &mut Imports::default(),
        None,
    )
    .test_error()?;
    test_verify!(
        errors
            .diagnostics()
            .iter()
            .any(|d| d.code() == DiagnosticCode::SCHEMA_SEMANTIC_ERROR)
    );
    Ok(())
}
