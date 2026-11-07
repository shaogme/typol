use crate::{
    ResolverError, SchemaCompileOptions, SchemaCompileUsage, SchemaLimits, SchemaResolveRequest,
    SchemaResolver, SchemaSource,
    allocation_support::CompileRunErrors,
    allocation_tests::{AllocationScope, STATE},
};
use alloc::string::String;
use core::cell::Cell;
use opeo::{OResult, Out};
use typol_core::{
    BudgetError, DiagnosticCode, ExcerptLimits, ExecutionLimits, RegexExecutionBudget,
};
use typol_policy::{RegexExecutionLimits, RegexOptions};
use typol_test_support::*;

const ACCEPTANCE_KINDS: [&str; 10] = [
    "fields",
    "prefixes",
    "aliases",
    "shapes",
    "annotations",
    "expressions",
    "regex",
    "numbers",
    "labels",
    "endpoints",
];

struct EmptyResolver;

impl SchemaResolver for EmptyResolver {
    fn resolve<'src, 'slot>(
        &'src mut self,
        _request: SchemaResolveRequest<'_>,
        out: Out<'slot, '_, ResolverError>,
    ) -> OResult<'slot, SchemaSource<'src>> {
        Err(out.fail(ResolverError::ImportsNotSupported))
    }
}

struct AcceptanceMeasurement {
    usage: SchemaCompileUsage,
    succeeded: bool,
}

fn regex_budget_state_bytes() -> TestResult<usize> {
    let scope = AllocationScope::start(None);
    let budget = TestOpeo::call(|out| RegexExecutionBudget::new(1, 1, 1, out)).test_value()?;
    let bytes = STATE.with(Cell::get).live_bytes as usize;
    drop(budget);
    test_verify_eq!(scope.finish().live_bytes, 0);
    Ok(bytes)
}

fn acceptance_source(kind: &str, count: usize) -> String {
    let mut source = String::new();
    if kind == "aliases" {
        source.push_str("type Base = u8;");
        for index in 0..count {
            source.push_str(&format!("type Alias_{index:06} = Base;"));
        }
    }
    if kind == "regex" {
        source.push_str(&format!(
            "type Text = string @regex(\"{}\");",
            "(a|b){32}".repeat(count / 32)
        ));
    }
    if kind == "endpoints" {
        source.push_str(&" // Unicode comment🙂\r\n".repeat(count));
    }
    source.push_str("struct Item { values: [u8] = [1, 2, 3, 4] } entry struct Root {");
    for index in (0..count).rev() {
        let name = if kind == "labels" {
            String::from("duplicate")
        } else if kind == "prefixes" {
            format!("{}_field_{index:06}", "common_prefix_".repeat(16))
        } else {
            format!("field_{index:06}")
        };
        let definition = match kind {
            "aliases" => format!("Alias_{index:06}"),
            "shapes" => format!("[u8; {}]", index + 1),
            "annotations" => String::from("u8 @min(0) @max(255)"),
            "expressions" => String::from("[u8] @all(item > 0 && item < 255, \"range\") = [1]"),
            "regex" => String::from("Text"),
            "numbers" => String::from("f64 = 123456789.012345678901234567890123456789"),
            "fields" | "prefixes" | "endpoints" => String::from("Item = {}"),
            _ => String::from("u8"),
        };
        source.push_str(&format!("{name}: {definition},"));
    }
    source.push('}');
    source
}

fn acceptance_measure(
    source: &str,
    kind: &str,
    count: usize,
    execution: ExecutionLimits,
    regex_execution: RegexExecutionLimits,
) -> TestResult<AcceptanceMeasurement> {
    let regex_state_bytes = regex_budget_state_bytes()?;
    let mut limits = SchemaLimits::default();
    limits.set_excerpts(ExcerptLimits::new(0, 0, 0, 0));
    if kind == "labels" {
        limits.set_max_diagnostics(count * 2 + 2);
    }
    let options = SchemaCompileOptions::new(limits)
        .with_execution(execution)
        .with_regex(RegexOptions::default().with_execution(regex_execution));
    let scope = AllocationScope::start(None);
    let mut compile_errors = CompileRunErrors::new();
    let run = CompileRunErrors::run_with_context(
        options,
        SchemaSource::from_str(b"root", source),
        &mut EmptyResolver,
        None,
        &mut compile_errors,
    )
    .into_test_value()?;
    let context = run.context;
    let result = run.result;
    let usage = context.usage();
    let state = STATE.with(Cell::get);
    test_verify_eq!(
        state.live_bytes as usize,
        usage.workspace_bytes + usage.regex_workspace_bytes + regex_state_bytes,
        "{kind}/{count}"
    );
    test_verify!(
        state.peak_bytes as usize
            <= usage.peak_workspace_bytes + usage.regex_peak_workspace_bytes + regex_state_bytes,
        "{kind}/{count}"
    );
    let succeeded = result.is_ok();
    if let Err(errors) = &result {
        test_verify!(!errors.diagnostics().is_empty());
        if usage.failure.is_some() || usage.regex_failure.is_some() {
            test_verify_eq!(
                errors.diagnostics().last().into_test_value()?.code(),
                DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
            );
        }
    }
    drop(result);
    drop(context);
    test_verify_eq!(scope.finish().live_bytes, 0);
    Ok(AcceptanceMeasurement { usage, succeeded })
}

#[test]
fn complete_public_compilation_scales_and_stops_at_exact_shared_limits() -> TestResult {
    for kind in ACCEPTANCE_KINDS {
        let mut previous = 0;
        for count in [128, 256, 512] {
            let source = acceptance_source(kind, count);
            let sample = acceptance_measure(
                &source,
                kind,
                count,
                ExecutionLimits::new(67_108_864, 67_108_864),
                RegexExecutionLimits::default(),
            )
            .into_test_value()?;
            test_verify_eq!(sample.succeeded, kind != "labels", "{kind}/{count}");
            test_verify!(sample.usage.steps > previous);
            previous = sample.usage.steps;
            let exact = acceptance_measure(
                &source,
                kind,
                count,
                ExecutionLimits::new(sample.usage.steps, sample.usage.peak_workspace_bytes),
                RegexExecutionLimits::new(
                    sample.usage.regex_steps,
                    67_108_864,
                    sample.usage.regex_peak_workspace_bytes,
                ),
            )
            .into_test_value()?;
            test_verify_eq!(exact.succeeded, sample.succeeded);
            test_verify_eq!(exact.usage, sample.usage);
            for execution in [
                ExecutionLimits::new(sample.usage.steps - 1, 67_108_864),
                ExecutionLimits::new(67_108_864, sample.usage.peak_workspace_bytes - 1),
            ] {
                let failure = acceptance_measure(
                    &source,
                    kind,
                    count,
                    execution,
                    RegexExecutionLimits::default(),
                )
                .into_test_value()?;
                test_verify!(!failure.succeeded);
                test_verify_eq!(failure.usage.failure, Some(BudgetError::Resource));
            }
            if kind == "regex" {
                for regex_execution in [
                    RegexExecutionLimits::new(sample.usage.regex_steps - 1, 67_108_864, 67_108_864),
                    RegexExecutionLimits::new(
                        67_108_864,
                        67_108_864,
                        sample.usage.regex_peak_workspace_bytes - 1,
                    ),
                ] {
                    let failure = acceptance_measure(
                        &source,
                        kind,
                        count,
                        ExecutionLimits::new(67_108_864, 67_108_864),
                        regex_execution,
                    )
                    .into_test_value()?;
                    test_verify!(!failure.succeeded);
                    test_verify!(failure.usage.regex_failure.is_some());
                }
            }
        }
    }

    Ok(())
}
