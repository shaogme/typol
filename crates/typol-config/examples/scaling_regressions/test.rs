mod workloads;

use typol_config::{ConfigParseErrors, ConfigParseOptions, opeo::ErrSlot};
use typol_core::{ConfigValue, DiagnosticCode, ExecutionLimits, ValueKind};
use typol_test_support::*;

use workloads::{Provider, Workload, WorkloadBuildError, WorkloadCase};

enum RunOutcome {
    Success(ConfigValue),
    Failure(ConfigParseErrors),
}

impl RunOutcome {
    fn is_ok(&self) -> bool {
        matches!(self, Self::Success(_))
    }

    fn test_value(self) -> TestResult<ConfigValue> {
        match self {
            Self::Success(value) => Ok(value),
            Self::Failure(_) => Err(TestFailure::new("workload parsing failed")),
        }
    }

    fn test_error(self) -> TestResult<ConfigParseErrors> {
        match self {
            Self::Success(_) => Err(TestFailure::new(
                "workload parsing did not produce the expected error",
            )),
            Self::Failure(errors) => Ok(errors),
        }
    }
}

fn build_workload(case: WorkloadCase, size: usize) -> TestResult<Workload> {
    let mut errors = ErrSlot::<WorkloadBuildError>::new();
    errors
        .try_call(|out| Workload::new(case, size, out))
        .map_err(|caught| TestFailure::new(caught.take().to_string()))
}

fn run_workload(
    workload: &Workload,
    options: ConfigParseOptions,
    provider: &mut Provider,
) -> RunOutcome {
    let mut errors = ErrSlot::new();
    match errors.try_call(|out| workload.run(options, provider, out)) {
        Ok(value) => RunOutcome::Success(value),
        Err(caught) => RunOutcome::Failure(caught.take()),
    }
}

#[test]
fn wide_inputs_and_adversarial_unique_keep_semantics_at_both_sizes() -> TestResult<()> {
    for case in [
        WorkloadCase::WideFields,
        WorkloadCase::EnvironmentNames,
        WorkloadCase::DefaultMap,
        WorkloadCase::UniqueCaseVariants,
        WorkloadCase::AssertionChain,
    ] {
        for size in [128, 256] {
            let workload = build_workload(case, size)?;
            let mut provider = Provider::new();
            let value = run_workload(
                &workload,
                workload.options(ExecutionLimits::new(16_777_216, 67_108_864)),
                &mut provider,
            )
            .test_value()?;
            test_eq!(provider.calls(), workload.reads())?;
            test_eq!(workload.errors(), 0)?;
            test_assert!(workload.source_bytes() > 0)?;
            test_eq!(workload.name(), case.name())?;
            let fields = match value.as_ref() {
                ValueKind::Structure(fields) => fields,
                _ => {
                    return Err(TestFailure::new(
                        "profiling workload root should be a struct",
                    ));
                }
            };
            if matches!(
                case,
                WorkloadCase::WideFields | WorkloadCase::EnvironmentNames
            ) {
                test_eq!(fields.len(), size)?;
                for (index, field) in fields.into_iter().enumerate() {
                    test_eq!(field.name().unwrap_or_default(), format!("f{index:05}"))?;
                    test_assert!(matches!(field.value().as_ref(), ValueKind::String("value")))?;
                }
            } else if !matches!(case, WorkloadCase::AssertionChain) {
                let first = fields.into_iter().next().test_value()?;
                let count = match first.value().as_ref() {
                    ValueKind::Array(values) => values.len(),
                    ValueKind::Map(entries) => entries.len(),
                    _ => {
                        return Err(TestFailure::new(
                            "profiling workload first field should be a collection",
                        ));
                    }
                };
                test_eq!(count, size)?;
            }
        }
    }

    Ok(())
}

#[test]
fn many_item_errors_preserve_source_order_and_owned_unicode_locations() -> TestResult<()> {
    for case in [
        WorkloadCase::CollectionErrors,
        WorkloadCase::LongLineLocations,
    ] {
        for size in [128, 256] {
            let workload = build_workload(case, size)?;
            let errors = run_workload(
                &workload,
                workload.options(ExecutionLimits::new(16_777_216, 67_108_864)),
                &mut Provider::new(),
            )
            .test_error()?;
            test_eq!(errors.diagnostics().len(), workload.errors())?;
            drop(workload);
            let mut previous_column = 0;
            for (index, error) in errors.diagnostics().iter().enumerate() {
                test_eq!(error.code(), DiagnosticCode::CONFIG_VALIDATION_ERROR)?;
                test_eq!(
                    error
                        .path()
                        .test_value()?
                        .segments()
                        .get(1)
                        .test_value()?
                        .as_index(),
                    Some(index)
                )?;
                let span = errors
                    .span_location(error.primary().test_value()?)
                    .test_value()?;
                test_assert!(span.start.column() > previous_column)?;
                previous_column = span.start.column();
                test_assert!(
                    errors
                        .span_location(error.labels().first().test_value()?.span())
                        .is_some()
                )?;
            }
        }
    }

    Ok(())
}

#[test]
fn calibrated_defaults_accept_large_inputs_with_exact_unique_semantics() -> TestResult<()> {
    for case in [
        WorkloadCase::WideFields,
        WorkloadCase::DefaultMap,
        WorkloadCase::EnvironmentNames,
    ] {
        let workload = build_workload(case, 1024)?;
        let mut provider = Provider::new();
        test_assert!(
            run_workload(&workload, ConfigParseOptions::default(), &mut provider).is_ok()
        )?;
        test_eq!(provider.calls(), workload.reads())?;
    }
    let workload = build_workload(WorkloadCase::UniqueCaseVariants, 4096)?;
    let value = run_workload(
        &workload,
        ConfigParseOptions::default(),
        &mut Provider::new(),
    )
    .test_value()?;
    let fields = match value.as_ref() {
        ValueKind::Structure(fields) => fields,
        _ => {
            return Err(TestFailure::new(
                "profiling workload root should be a struct",
            ));
        }
    };
    let first = fields.into_iter().next().test_value()?;
    let values = match first.value().as_ref() {
        ValueKind::Array(values) => values,
        _ => return Err(TestFailure::new("unique workload field should be an array")),
    };
    test_eq!(values.len(), 4096)?;

    Ok(())
}

#[test]
fn calibrated_options_remain_overridable_without_reusing_call_balance() -> TestResult<()> {
    let options = ConfigParseOptions::default();
    test_eq!(options.config().max_environment_entries(), 1024)?;
    test_eq!(options.config().max_environment_bytes(), 8_388_608)?;
    test_eq!(options.config().max_constructed_text_bytes(), 16_777_216)?;
    test_eq!(options.config().max_display_name_bytes(), 4096)?;
    test_eq!(options.execution().max_steps(), 16_777_216)?;
    test_eq!(options.execution().max_workspace_bytes(), 67_108_864)?;
    let workload = build_workload(WorkloadCase::WideFields, 256)?;
    for _ in 0..2 {
        test_assert!(run_workload(&workload, options, &mut Provider::new()).is_ok())?;
    }

    Ok(())
}

#[test]
fn large_environment_values_use_the_same_default_step_balance_and_stop_later_reads()
-> TestResult<()> {
    let workload = build_workload(WorkloadCase::LargeEnvironmentValues, 128)?;
    let mut provider = Provider::new();
    test_assert!(run_workload(&workload, ConfigParseOptions::default(), &mut provider).is_ok())?;
    test_eq!(provider.calls(), 128)?;
    let workload = build_workload(WorkloadCase::LargeEnvironmentValues, 256)?;
    let mut provider = Provider::new();
    let errors =
        run_workload(&workload, ConfigParseOptions::default(), &mut provider).test_error()?;
    test_eq!(
        errors.diagnostics().last().test_value()?.code(),
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
    )?;
    test_assert!(provider.calls() < 256)?;

    Ok(())
}
