#[path = "support/schema_compile.rs"]
mod schema_support;

use typol_core::{DiagnosticCode, ExecutionBudget, ExecutionLimits, SliceSource, SourceId};
use typol_policy::{RegexExecutionLimits, RegexOptions};
use typol_schema::{
    LocationFailure, LocationStatus, ResolverError, SchemaCompileErrors, SchemaCompileOptions,
    SchemaLimits, SchemaResolveRequest, SchemaResolver, SchemaSource, SourceLoadedRequest,
    SourceObserver, SourceObserverError,
    opeo::{OResult, Out},
};
use typol_test_support::*;

use typol_syntax::{SchemaParseOptions, SchemaParser, StreamLimits};

struct Sources {
    requests: Vec<String>,
    broken_source: SliceSource<'static>,
    child_source: SliceSource<'static>,
}

impl SchemaResolver for Sources {
    fn resolve<'src, 'slot>(
        &'src mut self,
        request: SchemaResolveRequest<'_>,
        out: Out<'slot, '_, ResolverError>,
    ) -> OResult<'slot, SchemaSource<'src>> {
        let module = request.module_name;
        self.requests.push(module.to_owned());
        match module {
            "broken" => {
                self.broken_source = SliceSource::new(b"struct");
                Ok(SchemaSource::from_source(
                    b"broken",
                    &mut self.broken_source,
                ))
            }
            "child" => {
                self.child_source = SliceSource::new(b"?");
                Ok(SchemaSource::from_source(b"child", &mut self.child_source))
            }
            _ => Err(out.fail(ResolverError::NotFound)),
        }
    }
}

struct Observer {
    stop_at: SourceId,
    error: SourceObserverError,
    notified: Vec<SourceId>,
}

impl SourceObserver for Observer {
    fn source_loaded<'slot>(
        &mut self,
        request: SourceLoadedRequest<'_>,
        out: Out<'slot, '_, SourceObserverError>,
    ) -> OResult<'slot, ()> {
        let id = request.source_id;
        self.notified.push(id);
        if id == self.stop_at {
            Err(out.fail(self.error))
        } else {
            Ok(())
        }
    }
}

#[test]
fn observer_failure_is_terminal_before_current_parse_and_later_imports() -> TestResult {
    for limit in [0, 1, 4] {
        for (error, code) in [
            (
                SourceObserverError::ResourceLimitExceeded,
                DiagnosticCode::RESOURCE_LIMIT_EXCEEDED,
            ),
            (
                SourceObserverError::AllocationFailed,
                DiagnosticCode::ALLOCATION_FAILED,
            ),
            (
                SourceObserverError::HostFailure,
                DiagnosticCode::SCHEMA_RESOLVER_ERROR,
            ),
            (
                SourceObserverError::InternalInvariantFailure,
                DiagnosticCode::INTERNAL_ERROR,
            ),
        ] {
            for stop_at in [0, 1] {
                let mut observer = Observer {
                    stop_at: SourceId::new(stop_at),
                    error,
                    notified: Vec::new(),
                };
                let mut sources = Sources {
                    requests: Vec::new(),
                    broken_source: SliceSource::new(b""),
                    child_source: SliceSource::new(b""),
                };
                let mut limits = SchemaLimits::default();
                limits.set_max_diagnostics(limit);
                let errors = schema_support::SchemaCompileTestSupport::run(
                    SchemaCompileOptions::new(limits),
                    SchemaSource::from_str(
                        b"root",
                        "extern mod child; extern mod later; entry struct Root {}",
                    ),
                    &mut sources,
                    Some(&mut observer),
                )
                .test_error()?;
                test_verify_eq!(errors.diagnostics().len(), 1);
                let diagnostic = &errors.diagnostics().first().test_value()?;
                test_verify_eq!(diagnostic.code(), code);
                let span = diagnostic.primary().test_value()?;
                test_verify_eq!(span.source_id(), SourceId::new(stop_at));
                match error {
                    SourceObserverError::HostFailure => {
                        test_verify_eq!(
                            errors.location_status(),
                            LocationStatus::Unavailable(LocationFailure::SourceUnavailable)
                        );
                        test_verify!(errors.span_location(span).is_none());
                    }
                    SourceObserverError::ResourceLimitExceeded => {
                        test_verify_eq!(
                            errors.location_status(),
                            LocationStatus::Unavailable(LocationFailure::ResourceLimitExceeded)
                        );
                        test_verify!(errors.span_location(span).is_none());
                    }
                    SourceObserverError::AllocationFailed => {
                        test_verify_eq!(
                            errors.location_status(),
                            LocationStatus::Unavailable(LocationFailure::AllocationFailure)
                        );
                        test_verify!(errors.span_location(span).is_none());
                    }
                    SourceObserverError::CapacityExceeded => {
                        test_verify_eq!(
                            errors.location_status(),
                            LocationStatus::Unavailable(LocationFailure::CapacityExceeded)
                        );
                        test_verify!(errors.span_location(span).is_none());
                    }
                    SourceObserverError::InternalInvariantFailure => {
                        test_verify_eq!(
                            errors.location_status(),
                            LocationStatus::Unavailable(LocationFailure::InternalError)
                        );
                        test_verify!(errors.span_location(span).is_none());
                        let context = diagnostic.context().test_value()?;
                        test_verify_eq!(context.subsystem(), "typol-schema");
                        test_verify_eq!(context.stage(), "source-observer");
                        test_verify!(
                            context
                                .failure_point()
                                .starts_with("source-loaded-invariant")
                        );
                        test_verify_eq!(context.object_id(), Some(stop_at as u64));
                    }
                }
                test_verify_eq!(observer.notified.len(), stop_at as usize + 1);
                test_verify_eq!(sources.requests.len(), stop_at as usize);
            }
        }
    }

    Ok(())
}

#[test]
fn schema_limits_configure_token_window_and_maximum_token_size() -> TestResult {
    let mut small_window = SchemaLimits::default();
    small_window.set_stream_limits(StreamLimits::try_new(1, 64).test_value()?);
    schema_support::SchemaCompileTestSupport::run_without_imports(
        SchemaCompileOptions::new(small_window),
        SchemaSource::from_str(b"root", "entry struct Root {}"),
    )
    .test_value()?;

    let mut short_tokens = SchemaLimits::default();
    short_tokens.set_stream_limits(StreamLimits::try_new(8, 4).test_value()?);
    let errors = schema_support::SchemaCompileTestSupport::run_without_imports(
        SchemaCompileOptions::new(short_tokens),
        SchemaSource::from_str(b"root", "entry struct Root {}"),
    )
    .test_error()?;
    test_verify_eq!(
        errors.diagnostics().first().test_value()?.code(),
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
    );

    Ok(())
}

#[test]
fn observer_failure_preserves_diagnostics_without_unparsed_source_locations() -> TestResult {
    let mut observer = Observer {
        stop_at: SourceId::new(2),
        error: SourceObserverError::HostFailure,
        notified: Vec::new(),
    };
    let mut sources = Sources {
        requests: Vec::new(),
        broken_source: SliceSource::new(b""),
        child_source: SliceSource::new(b""),
    };
    let errors = schema_support::SchemaCompileTestSupport::run(
        SchemaCompileOptions::default(),
        SchemaSource::from_str(
            b"root",
            "extern mod broken; extern mod child; extern mod later; entry struct Root {}",
        ),
        &mut sources,
        Some(&mut observer),
    )
    .test_error()?;
    test_verify_eq!(sources.requests, ["broken", "child"]);
    drop(sources);
    drop(observer);
    test_verify_eq!(errors.diagnostics().len(), 2);
    test_verify_eq!(
        errors.diagnostics().first().test_value()?.code(),
        DiagnosticCode::SCHEMA_SYNTAX_ERROR
    );
    test_verify_eq!(
        errors.diagnostics().get(1).test_value()?.code(),
        DiagnosticCode::SCHEMA_RESOLVER_ERROR
    );
    test_verify_eq!(
        errors.location_status(),
        LocationStatus::Unavailable(LocationFailure::SourceUnavailable)
    );
    for diagnostic in errors.diagnostics() {
        test_verify!(
            errors
                .span_location(diagnostic.primary().test_value()?)
                .is_none()
        );
    }

    Ok(())
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

struct InternalResolver;

impl SchemaResolver for InternalResolver {
    fn resolve<'src, 'slot>(
        &'src mut self,
        _request: SchemaResolveRequest<'_>,
        out: Out<'slot, '_, ResolverError>,
    ) -> OResult<'slot, SchemaSource<'src>> {
        Err(out.fail(ResolverError::InternalInvariantFailure))
    }
}

#[test]
fn resolver_internal_failure_keeps_callback_and_import_context() -> TestResult {
    let errors = schema_support::SchemaCompileTestSupport::run(
        SchemaCompileOptions::default(),
        SchemaSource::from_str(b"root", "extern mod child; entry struct Root {}"),
        &mut InternalResolver,
        None,
    )
    .test_error()?;
    let diagnostic = errors.diagnostics().first().test_value()?;
    let span = diagnostic.primary().test_value()?;
    let context = diagnostic.context().test_value()?;

    test_verify_eq!(diagnostic.code(), DiagnosticCode::INTERNAL_ERROR);
    test_verify_eq!(span.source_id(), SourceId::new(0));
    test_verify_eq!(context.subsystem(), "typol-schema");
    test_verify_eq!(context.stage(), "resolver-callback");
    test_verify!(context.failure_point().starts_with("resolve-invariant"));
    test_verify_eq!(context.object_id(), Some(0));
    Ok(())
}

fn compile_with_execution(
    source: &str,
    execution: ExecutionLimits,
) -> schema_support::TestOpeoOutcome<(), SchemaCompileErrors> {
    match schema_support::SchemaCompileTestSupport::run(
        SchemaCompileOptions::default().with_execution(execution),
        SchemaSource::from_str(b"root", source),
        &mut NoImports,
        None,
    ) {
        schema_support::TestOpeoOutcome::Value(_) => schema_support::TestOpeoOutcome::Value(()),
        schema_support::TestOpeoOutcome::Error(error) => {
            schema_support::TestOpeoOutcome::Error(error)
        }
    }
}

fn compile_with_regex_execution(
    source: &str,
    execution: RegexExecutionLimits,
) -> schema_support::TestOpeoOutcome<(), SchemaCompileErrors> {
    match schema_support::SchemaCompileTestSupport::run(
        SchemaCompileOptions::default()
            .with_regex(RegexOptions::default().with_execution(execution)),
        SchemaSource::from_str(b"root", source),
        &mut NoImports,
        None,
    ) {
        schema_support::TestOpeoOutcome::Value(_) => schema_support::TestOpeoOutcome::Value(()),
        schema_support::TestOpeoOutcome::Error(error) => {
            schema_support::TestOpeoOutcome::Error(error)
        }
    }
}

#[test]
fn compiler_execution_is_independent_of_source_byte_limits() -> TestResult {
    let source = "entry struct Root {}";
    for execution in [
        ExecutionLimits::new(0, 67_108_864),
        ExecutionLimits::new(67_108_864, 0),
        ExecutionLimits::new(67_108_864, 256),
    ] {
        let errors = compile_with_execution(source, execution).test_error()?;
        test_verify_eq!(
            errors.diagnostics().last().test_value()?.code(),
            DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
        );
    }
    compile_with_execution(source, ExecutionLimits::new(67_108_864, 67_108_864)).test_value()?;

    Ok(())
}

#[test]
fn identity_storage_is_charged_before_observer_notification() -> TestResult {
    let identity = vec![0; 4096];
    let mut observer = Observer {
        stop_at: SourceId::new(1),
        error: SourceObserverError::HostFailure,
        notified: Vec::new(),
    };
    let errors = schema_support::SchemaCompileTestSupport::run(
        SchemaCompileOptions::default().with_execution(ExecutionLimits::new(67_108_864, 2048)),
        SchemaSource::from_str(&identity, "entry struct Root {}"),
        &mut NoImports,
        Some(&mut observer),
    )
    .test_error()?;
    test_verify_eq!(
        errors.diagnostics().first().test_value()?.code(),
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
    );
    test_verify!(observer.notified.is_empty());

    Ok(())
}

#[test]
fn nested_default_quantifiers_use_compiler_execution_budget() -> TestResult {
    let row = format!("[{}]", vec!["1"; 32].join(","));
    let value = format!("[{}]", vec![row.as_str(); 32].join(","));
    let source = format!(
        "entry struct Root {{ values: [[u8]] @all(item.all(|x| item.all(|y| x == y || x != y)), \"secret sentinel\") = {value} }}"
    );
    compile_with_execution(&source, ExecutionLimits::new(67_108_864, 67_108_864)).test_value()?;
    let errors = compile_with_execution(&source, ExecutionLimits::new(1_000_000, 67_108_864))
        .test_error()?;
    let diagnostic = errors.diagnostics().last().test_value()?;
    test_verify_eq!(diagnostic.code(), DiagnosticCode::RESOURCE_LIMIT_EXCEEDED);
    test_verify_eq!(
        diagnostic.primary().test_value()?.span().start(),
        source.find("item.all(").test_value()?
    );
    test_verify!(!format!("{errors:?}").contains("secret sentinel"));

    Ok(())
}

#[test]
fn default_regex_searches_share_one_cumulative_budget() -> TestResult {
    let text = "a".repeat(4096);
    let single =
        format!("entry struct Root {{ first: string @regex(\"^(a|aa)*$\") = \"{text}\" }}");
    let multiple = format!(
        "entry struct Root {{ first: string @regex(\"^(a|aa)*$\") = \"{text}\", second: string @regex(\"^(a|aa)*$\") = \"{text}\" }}"
    );
    let single_run = schema_support::SchemaCompileTestSupport::run_with_context(
        SchemaCompileOptions::default(),
        SchemaSource::from_str(b"root", &single),
        &mut NoImports,
    )
    .test_value()?;
    let single_steps = single_run.context.usage().regex_steps;
    test_verify!(single_run.result.is_ok());
    drop(single_run);
    let second_default = multiple.rfind("= \"").test_value()? + 2;
    // Increase the quota until the first default passes and the second hits the shared budget.
    let mut allowance = 10_000_usize;
    let (execution, errors) = loop {
        let execution = RegexExecutionLimits::new(single_steps + allowance, 67_108_864, 67_108_864);
        match compile_with_regex_execution(&multiple, execution) {
            schema_support::TestOpeoOutcome::Value(()) => {
                return Err(TestFailure::new(
                    "two regex defaults did not trigger the cumulative execution budget",
                ));
            }
            schema_support::TestOpeoOutcome::Error(errors) => {
                let diagnostic = errors.diagnostics().last().test_value()?;
                test_verify_eq!(diagnostic.code(), DiagnosticCode::RESOURCE_LIMIT_EXCEEDED);
                let start = diagnostic.primary().test_value()?.span().start();
                if start == second_default {
                    break (execution, errors);
                }
            }
        }
        allowance = allowance
            .checked_mul(2)
            .ok_or_else(|| TestFailure::new("cumulative-budget search exceeded its step limit"))?;
    };
    compile_with_regex_execution(&single, execution).test_value()?;
    let diagnostic = errors.diagnostics().last().test_value()?;
    test_verify_eq!(diagnostic.code(), DiagnosticCode::RESOURCE_LIMIT_EXCEEDED);
    test_verify_eq!(
        diagnostic.primary().test_value()?.span().start(),
        second_default
    );
    compile_with_regex_execution(&multiple, RegexExecutionLimits::default()).test_value()?;

    Ok(())
}

struct WideSource {
    text: String,
    requests: usize,
    source: OwnedReadSource,
}

impl SchemaResolver for WideSource {
    fn resolve<'src, 'slot>(
        &'src mut self,
        request: SchemaResolveRequest<'_>,
        out: Out<'slot, '_, ResolverError>,
    ) -> OResult<'slot, SchemaSource<'src>> {
        let module = request.module_name;
        if module != "child" {
            return Err(out.fail(ResolverError::NotFound));
        }
        self.requests += 1;
        self.source.replace(&self.text);
        Ok(SchemaSource::from_source(b"child", &mut self.source))
    }
}

#[test]
fn compiler_budget_is_cumulative_across_imports_without_defaults() -> TestResult {
    let fields = (0..128)
        .map(|index| format!("field_{index}: u8,"))
        .collect::<String>();
    let single = format!("entry struct Root {{ {fields} }}");
    let child = format!("struct Child {{ {fields} }}");
    let limits = SchemaLimits::default();
    let probe = schema_support::SchemaCompileTestSupport::budget(|out| {
        ExecutionBudget::new(ExecutionLimits::new(67_108_864, 67_108_864), out)
    })
    .test_value()?;
    drop(
        schema_support::SchemaCompileTestSupport::parse(|out| {
            SchemaParser::parse_str(
                SourceId::new(0),
                &single,
                &SchemaParseOptions::new(*limits.syntax(), limits.stream_limits()),
                &probe,
                out,
            )
        })
        .test_value()?,
    );
    let mut low = probe.steps();
    let mut high = 67_108_864;
    while low < high {
        let middle = low + (high - low) / 2;
        if compile_with_execution(&single, ExecutionLimits::new(middle, 67_108_864)).is_ok() {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    let execution = ExecutionLimits::new(low, 67_108_864);
    compile_with_execution(&single, execution).test_value()?;
    let mut resolver = WideSource {
        text: child,
        requests: 0,
        source: OwnedReadSource::new(""),
    };
    let errors = schema_support::SchemaCompileTestSupport::run(
        SchemaCompileOptions::default().with_execution(execution),
        SchemaSource::from_str(b"root", &format!("extern mod child; {single}")),
        &mut resolver,
        None,
    )
    .test_error()?;
    let terminal = errors.diagnostics().last().test_value()?;
    test_verify_eq!(terminal.code(), DiagnosticCode::RESOURCE_LIMIT_EXCEEDED);
    test_verify_eq!(resolver.requests, 1);

    Ok(())
}
