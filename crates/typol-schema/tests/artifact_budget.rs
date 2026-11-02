#[path = "support/schema_compile.rs"]
mod schema_support;

use typol_core::{DiagnosticCode, ExcerptLimits, ExecutionBudget, ExecutionLimits, SourceLocation};
use typol_schema::{
    LocationFailure, LocationStatus, ResolverError, SchemaCompileOptions, SchemaCompileStage,
    SchemaLimits, SchemaResolveRequest, SchemaResolver, SchemaSource,
    opeo::{OResult, Out},
};
use typol_semantic::SchemaAccessError;
use typol_test_support::*;

const LIMIT: usize = 67_108_864;

fn access<T>(
    operation: impl for<'slot> FnOnce(Out<'slot, 'slot, SchemaAccessError>) -> OResult<'slot, T>,
) -> TestResult<T> {
    schema_support::SchemaCompileTestSupport::schema_access(operation).test_value()
}

struct NoImports;

impl SchemaResolver for NoImports {
    fn resolve<'src, 'slot>(
        &'src mut self,
        _request: SchemaResolveRequest<'_>,
        out: Out<'slot, '_, ResolverError>,
    ) -> OResult<'slot, SchemaSource<'src>> {
        Err(out.fail(ResolverError::NotFound))
    }
}

fn compile(source: &str, steps: usize) -> TestResult<schema_support::CompileRun> {
    // Phase boundaries measure business operations only; optional snapshot work is verified separately.
    let mut resources = SchemaLimits::default();
    resources.set_excerpts(ExcerptLimits::default().with_max_bytes(0));
    schema_support::SchemaCompileTestSupport::run_with_context(
        SchemaCompileOptions::new(resources).with_execution(ExecutionLimits::new(steps, LIMIT)),
        SchemaSource::from_str(b"root", source),
        &mut NoImports,
    )
    .test_value()
}

fn wide(count: usize) -> String {
    let mut source = String::from("entry struct Root {");
    for index in (0..count).rev() {
        source.push_str(&format!("common_prefix_{index:06}: [u8; {}],", index + 1));
    }
    source.push('}');
    source
}

fn first_phase_budget(source: &str, total: usize, target: SchemaCompileStage) -> TestResult<usize> {
    let mut low = 0;
    let mut high = total;
    while low < high {
        let middle = low + (high - low) / 2;
        let run = compile(source, middle)?;
        let stage = run.context.usage().last_stage;
        let reached = match target {
            SchemaCompileStage::Locations => matches!(
                stage,
                SchemaCompileStage::Locations
                    | SchemaCompileStage::Freezing
                    | SchemaCompileStage::Finished
            ),
            SchemaCompileStage::Freezing => matches!(
                stage,
                SchemaCompileStage::Freezing | SchemaCompileStage::Finished
            ),
            _ => return Err(String::from("checks only the final phase").into()),
        };
        if reached {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    Ok(low)
}

#[test]
fn records_locations_and_freezing_can_each_stop_the_public_entry() -> TestResult {
    let source = wide(64);
    let baseline = compile(&source, LIMIT).test_value()?;
    let total = baseline.context.usage().steps;
    test_verify!(baseline.result.is_ok());
    let location_start =
        first_phase_budget(&source, total, SchemaCompileStage::Locations).test_value()?;
    let freeze_start =
        first_phase_budget(&source, total, SchemaCompileStage::Freezing).test_value()?;
    for (steps, stage) in [
        (location_start - 1, SchemaCompileStage::Records),
        (location_start, SchemaCompileStage::Locations),
        (freeze_start, SchemaCompileStage::Freezing),
        (total - 1, SchemaCompileStage::Freezing),
    ] {
        let run = compile(&source, steps).test_value()?;
        test_verify_eq!(run.context.usage().last_stage, stage);
        let errors = run.result.test_error()?;
        test_verify_eq!(
            errors.diagnostics().last().test_value()?.code(),
            DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
        );
        test_verify_eq!(
            errors.location_status(),
            LocationStatus::Unavailable(LocationFailure::ResourceLimitExceeded)
        );
        test_verify!(run.context.usage().steps <= steps);
    }
    test_verify!(compile(&source, total).test_value()?.result.is_ok());

    Ok(())
}

#[test]
fn models_and_errors_keep_payloads_until_their_owners_drop() -> TestResult {
    let initial = schema_support::SchemaCompileTestSupport::budget(|out| {
        ExecutionBudget::new(ExecutionLimits::new(LIMIT, LIMIT), out)
    })
    .test_value()?
    .workspace_bytes();
    for source in [
        wide(32),
        String::from("entry struct Root { invalid: Missing }"),
    ] {
        let run = schema_support::SchemaCompileTestSupport::run_with_context(
            SchemaCompileOptions::default(),
            SchemaSource::from_str(b"root", &source),
            &mut NoImports,
        )
        .test_value()?;
        let context = run.context;
        let result = run.result;
        drop(source);
        test_verify!(context.usage().workspace_bytes > initial);
        match &result {
            schema_support::TestOpeoOutcome::Value(model) => {
                test_verify_eq!(access(|out| model.entry().fields(out))?.len(), 32);
                test_verify!(
                    model
                        .span_location(access(|out| model.entry().span(out))?)
                        .is_some()
                );
            }
            schema_support::TestOpeoOutcome::Error(errors) => {
                test_verify_eq!(errors.location_status(), LocationStatus::Complete);
                test_verify!(
                    errors
                        .span_location(
                            errors
                                .diagnostics()
                                .first()
                                .test_value()?
                                .primary()
                                .test_value()?
                        )
                        .is_some()
                );
            }
        }
        drop(result);
        test_verify_eq!(context.usage().workspace_bytes, initial);
    }

    Ok(())
}

#[test]
fn semantic_error_survives_later_location_exhaustion() -> TestResult {
    let source = format!(
        "{}entry struct Root {{ invalid: Missing }}",
        " ".repeat(16_384)
    );
    let baseline = compile(&source, LIMIT).test_value()?;
    let total = baseline.context.usage().steps;
    test_verify_eq!(
        baseline
            .result
            .error()
            .test_expect("baseline compilation should fail")?
            .location_status(),
        LocationStatus::Complete
    );
    let run = compile(&source, total - 1).test_value()?;
    test_verify_eq!(
        run.context.usage().last_stage,
        SchemaCompileStage::Locations
    );
    let errors = run.result.test_error()?;
    test_verify_eq!(errors.diagnostics().len(), 2);
    test_verify_eq!(
        errors.diagnostics().first().test_value()?.code(),
        DiagnosticCode::SCHEMA_SEMANTIC_ERROR
    );
    test_verify_eq!(
        errors.diagnostics().get(1).test_value()?.code(),
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
    );
    test_verify_eq!(
        errors.location_status(),
        LocationStatus::Unavailable(LocationFailure::ResourceLimitExceeded)
    );
    test_verify!(
        errors
            .diagnostics()
            .get(1)
            .test_value()?
            .message()
            .is_empty()
    );
    test_verify!(errors.diagnostics().iter().all(|error| {
        error
            .primary()
            .is_some_and(|span| errors.span_location(span).is_none())
    }));

    Ok(())
}

#[test]
fn enormous_diagnostic_limit_does_not_reserve_enormous_storage() -> TestResult {
    let mut options = SchemaCompileOptions::default();
    let mut resources = *options.limits();
    resources.set_max_diagnostics(usize::MAX);
    options =
        SchemaCompileOptions::new(resources).with_execution(ExecutionLimits::new(LIMIT, 65_536));
    let model = schema_support::SchemaCompileTestSupport::run_without_imports(
        options,
        SchemaSource::from_str(b"root", "entry struct Root {}"),
    )
    .test_value()?;
    test_verify_eq!(access(|out| model.entry().fields(out))?.len(), 0);

    Ok(())
}

#[test]
fn public_error_snapshots_count_unicode_columns_and_crlf() -> TestResult {
    let source = String::from("// Unicode comment🙂\r\nentry struct Root { invalid: Missing }");
    let run = compile(&source, LIMIT).test_value()?;
    let errors = run.result.test_error()?;
    let span = errors
        .diagnostics()
        .first()
        .test_value()?
        .primary()
        .test_value()?;
    let start = source
        .get(..span.span().start())
        .test_value()?
        .rsplit('\n')
        .next()
        .test_value()?
        .chars()
        .count()
        + 1;
    let end = source
        .get(..span.span().end())
        .test_value()?
        .rsplit('\n')
        .next()
        .test_value()?
        .chars()
        .count()
        + 1;
    drop(source);
    let location = errors.span_location(span).test_value()?;
    test_verify_eq!(location.start, SourceLocation::new(2, start).test_value()?);
    test_verify_eq!(location.end, SourceLocation::new(2, end).test_value()?);

    Ok(())
}
