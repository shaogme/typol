use typol_core::{
    BudgetError, Diagnostic, DiagnosticCode, ExecutionBudget, ExecutionLimits, Read, SourceId,
    SourceReadError, Span, SpanError,
};
use typol_policy::RegexLimits;
use typol_schema::{
    LocationFailure, LocationStatus, ResolverError, SchemaCompileContext,
    SchemaCompileContextFailure, SchemaCompileErrors, SchemaCompileOptions, SchemaCompiler,
    SchemaLimits, SchemaResolveRequest, SchemaResolver, SchemaSource, SourceObserver,
    opeo::{OResult, Out},
};
use typol_semantic::{
    CompiledSchema, Constraint, ConstraintList, EvalError, SchemaAccessError, SchemaEvaluator,
};
use typol_syntax::{SchemaParseOptions, SchemaParser, SyntaxErrors};
pub(super) use typol_test_support::TestOpeoOutcome;
use typol_test_support::*;

pub(super) struct SchemaCompileTestSupport;

impl SchemaCompileTestSupport {
    pub(super) fn schema_access<T>(
        operation: impl for<'slot> FnOnce(Out<'slot, 'slot, SchemaAccessError>) -> OResult<'slot, T>,
    ) -> TestOpeoOutcome<T, SchemaAccessError> {
        TestOpeo::outcome(operation)
    }

    pub(super) fn budget<T>(
        operation: impl for<'slot> FnOnce(Out<'slot, 'slot, BudgetError>) -> OResult<'slot, T>,
    ) -> TestOpeoOutcome<T, BudgetError> {
        TestOpeo::outcome(operation)
    }

    pub(super) fn parse<T>(
        operation: impl for<'slot> FnOnce(Out<'slot, 'slot, SyntaxErrors>) -> OResult<'slot, T>,
    ) -> TestOpeoOutcome<T, SyntaxErrors> {
        TestOpeo::outcome(operation)
    }

    pub(super) fn diagnostic<T>(
        operation: impl for<'slot> FnOnce(Out<'slot, 'slot, Diagnostic>) -> OResult<'slot, T>,
    ) -> TestOpeoOutcome<T, Diagnostic> {
        TestOpeo::outcome(operation)
    }

    pub(super) fn span<T>(
        operation: impl for<'slot> FnOnce(Out<'slot, 'slot, SpanError>) -> OResult<'slot, T>,
    ) -> TestOpeoOutcome<T, SpanError> {
        TestOpeo::outcome(operation)
    }

    pub(super) fn evaluation<T>(
        operation: impl for<'slot> FnOnce(Out<'slot, 'slot, EvalError>) -> OResult<'slot, T>,
    ) -> TestOpeoOutcome<T, EvalError> {
        TestOpeo::outcome(operation)
    }

    pub(super) fn collect_constraints<'a>(
        constraints: ConstraintList<'a>,
    ) -> TestResult<Vec<Constraint<'a>>> {
        let mut values = Vec::new();
        match TestOpeo::outcome(|mut out| {
            let iter = constraints.iter(out.reborrow()).collect::<Vec<_>>();
            for constraint in iter {
                values.push(constraint?);
            }
            Ok(())
        }) {
            TestOpeoOutcome::Value(()) => Ok(values),
            TestOpeoOutcome::Error(error) => {
                Err(TestFailure::new(format!("unexpected error: {error:?}")))
            }
        }
    }
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

struct HostFailureResolver;

impl SchemaResolver for HostFailureResolver {
    fn resolve<'src, 'slot>(
        &'src mut self,
        _request: SchemaResolveRequest<'_>,
        out: Out<'slot, '_, ResolverError>,
    ) -> OResult<'slot, SchemaSource<'src>> {
        Err(out.fail(ResolverError::HostFailure))
    }
}

pub(super) struct CompileRun {
    pub(super) result: TestOpeoOutcome<CompiledSchema, SchemaCompileErrors>,
    pub(super) context: SchemaCompileContext,
}

impl SchemaCompileTestSupport {
    pub(super) fn compile(
        _source_id: u32,
        source: &str,
        limits: &SchemaLimits,
    ) -> TestOpeoOutcome<CompiledSchema, SchemaCompileErrors> {
        Self::compile_with_options(_source_id, source, SchemaCompileOptions::new(*limits))
    }

    pub(super) fn compile_with_options(
        _source_id: u32,
        source: &str,
        options: SchemaCompileOptions,
    ) -> TestOpeoOutcome<CompiledSchema, SchemaCompileErrors> {
        Self::run(
            options,
            SchemaSource::from_str(b"test", source),
            &mut NoImports,
            None,
        )
    }

    pub(super) fn run<R: SchemaResolver + ?Sized, S: Read<SourceReadError>>(
        options: SchemaCompileOptions,
        source: SchemaSource<'_, S>,
        resolver: &mut R,
        observer: Option<&mut dyn SourceObserver>,
    ) -> TestOpeoOutcome<CompiledSchema, SchemaCompileErrors> {
        TestOpeo::outcome(|mut out| {
            let compiler = SchemaCompiler::new(options, out.reborrow())?;
            let schema = compiler.compile(source, resolver, observer, out.reborrow())?;
            Ok(schema)
        })
    }

    pub(super) fn run_without_imports<S: Read<SourceReadError>>(
        options: SchemaCompileOptions,
        source: SchemaSource<'_, S>,
    ) -> TestOpeoOutcome<CompiledSchema, SchemaCompileErrors> {
        Self::run(options, source, &mut NoImports, None)
    }

    pub(super) fn run_with_context<R: SchemaResolver + ?Sized, S: Read<SourceReadError>>(
        options: SchemaCompileOptions,
        source: SchemaSource<'_, S>,
        resolver: &mut R,
    ) -> TestOpeoOutcome<CompileRun, SchemaCompileErrors> {
        let compiler = match Self::run_compiler(options) {
            TestOpeoOutcome::Value(compiler) => compiler,
            TestOpeoOutcome::Error(error) => return TestOpeoOutcome::Error(error),
        };
        let context = match TestOpeo::outcome(|out| compiler.context(out)) {
            TestOpeoOutcome::Value(context) => TestOpeoOutcome::Value(context),
            TestOpeoOutcome::Error(error) => TestOpeoOutcome::Error(error.into()),
        };
        let context = match context {
            TestOpeoOutcome::Value(context) => context,
            TestOpeoOutcome::Error(error) => return TestOpeoOutcome::Error(error),
        };
        let result = TestOpeo::outcome(|out| compiler.compile(source, resolver, None, out));
        TestOpeoOutcome::Value(CompileRun { result, context })
    }

    pub(super) fn run_with_context_failure(
        options: SchemaCompileOptions,
        failure: SchemaCompileContextFailure,
    ) -> TestOpeoOutcome<CompiledSchema, SchemaCompileErrors> {
        let compiler = match Self::run_compiler(options) {
            TestOpeoOutcome::Value(compiler) => compiler,
            TestOpeoOutcome::Error(error) => return TestOpeoOutcome::Error(error),
        };
        let context = match Self::compiler_context(&compiler) {
            TestOpeoOutcome::Value(context) => context,
            TestOpeoOutcome::Error(error) => return TestOpeoOutcome::Error(error),
        };
        let root_context = match failure {
            SchemaCompileContextFailure::RootContextMismatch => {
                let other = match Self::run_compiler(options) {
                    TestOpeoOutcome::Value(compiler) => compiler,
                    TestOpeoOutcome::Error(error) => return TestOpeoOutcome::Error(error),
                };
                match Self::compiler_context(&other) {
                    TestOpeoOutcome::Value(context) => context,
                    TestOpeoOutcome::Error(error) => return TestOpeoOutcome::Error(error),
                }
            }
            SchemaCompileContextFailure::SessionFinished => {
                context.execution().finish();
                context
            }
        };
        let root = SchemaSource::in_context(
            b"context-failure-root",
            "entry struct Root {}",
            &root_context,
        );
        let mut resolver = NoImports;
        TestOpeo::outcome(|out| compiler.compile(root, &mut resolver, None, out))
    }

    fn compiler_context(
        compiler: &SchemaCompiler,
    ) -> TestOpeoOutcome<SchemaCompileContext, SchemaCompileErrors> {
        match TestOpeo::outcome(|out| compiler.context(out)) {
            TestOpeoOutcome::Value(context) => TestOpeoOutcome::Value(context),
            TestOpeoOutcome::Error(error) => TestOpeoOutcome::Error(error.into()),
        }
    }

    fn run_compiler(
        options: SchemaCompileOptions,
    ) -> TestOpeoOutcome<SchemaCompiler, SchemaCompileErrors> {
        TestOpeo::outcome(|out| SchemaCompiler::new(options, out))
    }
}

#[test]
fn schema_compile_test_support_uses_opeo_helpers() {
    let result = check_schema_compile_test_support_helpers();
    assert!(result.is_ok(), "{result:?}");
}

struct ContextFailureTestCase {
    failure: SchemaCompileContextFailure,
    code: DiagnosticCode,
    failure_point: &'static str,
    message: &'static str,
}

#[test]
fn root_context_and_finished_session_errors_have_distinct_identities() -> TestResult {
    let cases = [
        ContextFailureTestCase {
            failure: SchemaCompileContextFailure::RootContextMismatch,
            code: DiagnosticCode::SCHEMA_CONTEXT_MISMATCH,
            failure_point: "root-context-mismatch",
            message: "root schema source is bound to a different compilation context",
        },
        ContextFailureTestCase {
            failure: SchemaCompileContextFailure::SessionFinished,
            code: DiagnosticCode::SCHEMA_SESSION_FINISHED,
            failure_point: "execution-context-finished",
            message: "schema compilation session has ended and cannot continue",
        },
    ];
    for case in cases {
        let errors = SchemaCompileTestSupport::run_with_context_failure(
            SchemaCompileOptions::default(),
            case.failure,
        )
        .test_error()?;
        let diagnostic = errors.diagnostics().first().test_value()?;
        let span = diagnostic.primary().test_value()?;
        let context = diagnostic.context().test_value()?;

        test_verify_eq!(errors.diagnostics().len(), 1);
        test_verify_eq!(errors.context_failure(), Some(case.failure));
        test_verify_eq!(
            errors.location_status(),
            LocationStatus::Unavailable(LocationFailure::SourceUnavailable)
        );
        test_verify_eq!(diagnostic.code(), case.code);
        test_verify_eq!(span.source_id(), SourceId::new(0));
        test_verify!(errors.span_location(span).is_none());
        test_verify_eq!(context.subsystem(), "typol-schema");
        test_verify_eq!(context.stage(), "source-loading");
        test_verify!(context.failure_point().starts_with(case.failure_point));
        test_verify!(format!("{errors}").contains(case.message));
    }

    Ok(())
}

#[test]
fn resolver_host_failure_keeps_its_own_error_identity() -> TestResult {
    let errors = SchemaCompileTestSupport::run(
        SchemaCompileOptions::default(),
        SchemaSource::from_str(b"root", "extern mod child; entry struct Root {}"),
        &mut HostFailureResolver,
        None,
    )
    .test_error()?;
    let diagnostic = errors.diagnostics().first().test_value()?;
    let context = diagnostic.context().test_value()?;

    test_verify_eq!(errors.context_failure(), None);
    test_verify_eq!(diagnostic.code(), DiagnosticCode::SCHEMA_RESOLVER_ERROR);
    test_verify_eq!(context.stage(), "resolver-callback");
    test_verify!(
        context
            .failure_point()
            .starts_with("resolve-callback-failed")
    );

    Ok(())
}

fn check_schema_compile_test_support_helpers() -> TestResult {
    let limits = SchemaLimits::default();
    let source = "type Host = string @host; entry struct Root { value: Host }";
    let execution = SchemaCompileTestSupport::budget(|out| {
        ExecutionBudget::new(ExecutionLimits::new(67_108_864, 67_108_864), out)
    })
    .test_value()?;
    let span =
        SchemaCompileTestSupport::span(|out| Span::new(0, source.len(), out)).test_value()?;
    test_verify_eq!(span.start(), 0);
    SchemaCompileTestSupport::diagnostic(|_| Ok(())).test_value()?;
    let _ = SchemaCompileTestSupport::parse(|out| {
        SchemaParser::parse_str(
            SourceId::new(0),
            source,
            &SchemaParseOptions::new(*limits.syntax(), limits.stream_limits()),
            &execution,
            out,
        )
    })
    .test_value()?;

    let schema = SchemaCompileTestSupport::compile(0, source, &limits).test_value()?;
    let field =
        SchemaCompileTestSupport::schema_access(|out| schema.entry().field_named("value", out))
            .test_value()?;
    let field = field.ok_or_else(|| TestFailure::new("test schema is missing a field"))?;
    let ty = SchemaCompileTestSupport::schema_access(|out| field.ty(out)).test_value()?;
    let constraints =
        SchemaCompileTestSupport::schema_access(|out| field.constraints(out)).test_value()?;
    test_verify_eq!(
        SchemaCompileTestSupport::collect_constraints(constraints)?.len(),
        1
    );

    let regex_limits = RegexLimits::default();
    let evaluator = SchemaEvaluator::new(&schema, limits.values(), &regex_limits, None);
    let host = SchemaCompileTestSupport::evaluation(|out| evaluator.host(ty, out));
    test_verify!(*host.value().test_expect("host evaluation should succeed")?);

    let outcome = SchemaCompileTestSupport::run_without_imports(
        SchemaCompileOptions::new(limits),
        SchemaSource::from_str(b"support", source),
    );
    test_verify!(outcome.is_ok());
    let _ = outcome.test_value()?;

    let mut resolver = NoImports;
    let run = SchemaCompileTestSupport::run(
        SchemaCompileOptions::default(),
        SchemaSource::from_str(b"support", source),
        &mut resolver,
        None,
    );
    let _ = run.test_value()?;

    let mut resolver = NoImports;
    let contextual = SchemaCompileTestSupport::run_with_context(
        SchemaCompileOptions::default(),
        SchemaSource::from_str(b"support", source),
        &mut resolver,
    )
    .test_value()?;
    test_verify!(contextual.result.is_ok());
    let _ = contextual.context.usage();
    let invalid = SchemaCompileTestSupport::run_without_imports(
        SchemaCompileOptions::default(),
        SchemaSource::from_str(b"support", "entry struct Root {"),
    );
    let _ = invalid.test_error()?;

    Ok(())
}
