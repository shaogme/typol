use crate::{
    LocationFailure, LocationStatus, ResolverError, SchemaCompileOptions, SchemaCompileUsage,
    SchemaLimits, SchemaResolveRequest, SchemaResolver, SchemaSource,
    allocation_support::{CompileRunErrors, TestOperation},
    annotations::{AnnotationCompilation, AnnotationCompileInput, AnnotationCompiler},
    context::CompileContext,
    defaults::{DefaultCompilation, DefaultInputs, DefaultPass},
    diagnostics::{BuildFailure, DiagnosticCollector},
    expressions::{ExpressionCompilation, ExpressionCompileInput, ExpressionCompiler},
    loader::SourceLoader,
    module_tree::ModuleTree,
    opeo::{OResult, Out},
    session::CompileBudget,
    session::SourceCatalog,
    symbols::SymbolTable,
    test_support::SchemaCompileTestSupport,
    types::{ResolvedTypeGraph, TypeGraphCompiler},
};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    env, format,
    process::Command,
    ptr::null_mut,
    string::String,
    thread_local,
};
use typol_core::{
    BudgetError, DiagnosticCode, ExcerptLimits, ExcerptStatus, ExecutionBudget, ExecutionLimits,
    SourceId, SourceSpan, Span,
};
use typol_regex::{Regex, RegexCompiler};
use typol_test_support::*;

#[derive(Clone, Copy)]
struct AllocationState {
    active: bool,
    fail_at: Option<usize>,
    persistent: bool,
    attempts: usize,
    live_bytes: isize,
    peak_bytes: isize,
}

thread_local! {
    static STATE: Cell<AllocationState> = const { Cell::new(AllocationState {
        active: false, fail_at: None, persistent: false, attempts: 0, live_bytes: 0, peak_bytes: 0,
    }) };
}

struct CheckedAllocator;

fn should_fail() -> bool {
    STATE
        .try_with(|state| {
            let mut current = state.get();
            if !current.active {
                return false;
            }
            let fail = current.fail_at.is_some_and(|index| {
                index == current.attempts || (current.persistent && index < current.attempts)
            });
            current.attempts += 1;
            state.set(current);
            fail
        })
        .unwrap_or(false)
}

fn account(bytes: isize) {
    let _ = STATE.try_with(|state| {
        let mut current = state.get();
        if current.active {
            current.live_bytes += bytes;
            current.peak_bytes = current.peak_bytes.max(current.live_bytes);
            state.set(current);
        }
    });
}

unsafe impl GlobalAlloc for CheckedAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if should_fail() {
            return null_mut();
        }
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            account(layout.size() as isize);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        account(-(layout.size() as isize));
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if should_fail() {
            return null_mut();
        }
        let result = unsafe { System.realloc(pointer, layout, new_size) };
        if !result.is_null() {
            account(new_size as isize - layout.size() as isize);
        }
        result
    }
}

#[global_allocator]
static ALLOCATOR: CheckedAllocator = CheckedAllocator;

struct AllocationScope;

impl AllocationScope {
    fn start(fail_at: Option<usize>) -> Self {
        STATE.with(|state| {
            state.set(AllocationState {
                active: true,
                fail_at,
                persistent: false,
                attempts: 0,
                live_bytes: 0,
                peak_bytes: 0,
            })
        });
        Self
    }

    fn finish(self) -> AllocationState {
        STATE.with(Cell::get)
    }
}

impl Drop for AllocationScope {
    fn drop(&mut self) {
        STATE.with(|state| {
            let mut current = state.get();
            current.active = false;
            state.set(current);
        });
    }
}

struct EmptyResolver;

impl SchemaResolver for EmptyResolver {
    fn resolve<'src, 'slot>(
        &'src mut self,
        _request: SchemaResolveRequest<'_>,
        out: Out<'slot, '_, ResolverError>,
    ) -> OResult<'slot, SchemaSource<'src>> {
        Err(out.fail(ResolverError::NotFound))
    }
}

fn source_span(operations: &mut TestOperation, start: usize, end: usize) -> TestResult<SourceSpan> {
    let span = operations
        .span(|out| Span::new(start, end, out))
        .into_test_value()?;
    Ok(SourceSpan::new(SourceId::new(0), span))
}

fn make_execution(
    operations: &mut TestOperation,
    limits: ExecutionLimits,
) -> TestResult<ExecutionBudget> {
    operations
        .budget(|out| ExecutionBudget::new(limits, out))
        .into_test_value()
}

fn make_context(
    operations: &mut TestOperation,
    limits: SchemaLimits,
    execution: &ExecutionBudget,
) -> TestResult<CompileContext> {
    operations
        .budget(|out| CompileContext::new(limits, execution, out))
        .into_test_value()
}

fn make_diagnostics(
    operations: &mut TestOperation,
    limit: usize,
    span: SourceSpan,
    execution: &ExecutionBudget,
) -> TestResult<DiagnosticCollector> {
    operations
        .diagnostic(|out| DiagnosticCollector::new(limit, span, execution, out))
        .into_test_value()
}

fn make_budget(
    operations: &mut TestOperation,
    limits: SchemaLimits,
    execution: &ExecutionBudget,
) -> TestResult<CompileBudget> {
    operations
        .budget(|out| CompileBudget::new(limits, execution, out))
        .into_test_value()
}

fn module_tree<'a>(
    operations: &mut TestOperation,
    loaded: &'a SourceCatalog,
    context: &CompileContext,
    diagnostics: &mut DiagnosticCollector,
) -> Result<ModuleTree<'a>, BuildFailure> {
    operations.build(|out| ModuleTree::build(loaded, context, diagnostics, out))
}

fn symbol_table<'a>(
    operations: &mut TestOperation,
    tree: &ModuleTree<'a>,
    context: &CompileContext,
    diagnostics: &mut DiagnosticCollector,
) -> Result<SymbolTable<'a>, BuildFailure> {
    operations.build(|out| SymbolTable::build(tree, context, diagnostics, out))
}

fn type_graph<'a>(
    operations: &mut TestOperation,
    tree: &'a ModuleTree<'a>,
    symbols: &'a SymbolTable<'a>,
    context: &'a CompileContext,
    diagnostics: &mut DiagnosticCollector,
) -> Result<ResolvedTypeGraph, BuildFailure> {
    operations.build(|out| TypeGraphCompiler::build(tree, symbols, context, diagnostics, out))
}

fn compile_annotations(
    operations: &mut TestOperation,
    input: AnnotationCompileInput<'_, '_, '_, '_, '_>,
) -> Result<AnnotationCompilation, BuildFailure> {
    operations.build(|out| AnnotationCompiler::compile(input, out))
}

fn compile_expressions(
    operations: &mut TestOperation,
    input: ExpressionCompileInput<'_, '_, '_, '_>,
) -> Result<ExpressionCompilation, BuildFailure> {
    operations.build(|out| ExpressionCompiler::compile(input, out))
}

fn compile_defaults(
    operations: &mut TestOperation,
    inputs: DefaultInputs<'_>,
    diagnostics: &mut DiagnosticCollector,
) -> Result<DefaultCompilation, BuildFailure> {
    operations.build(|out| DefaultPass::compile(inputs, diagnostics, out))
}

#[cfg(test)]
#[test]
fn iterative_schema_passes_fail_atomically_and_release_all_tracked_storage() -> TestResult {
    let mut operations = TestOperation::new();
    for source in [
        r#"struct Holder { payload: [[[u8]]] = [[[1]]] } enum E { V([[u8]]) }
        entry struct Root { holder: Holder = {}, values: [[[u8]]] @all(item.all(|x| x.all(|y| y > 0)), "rule") = [[[1]]],
        unique: [E] @unique = [.V([[1]]), .V([[2]])] } @assert(1 + 2 * 3 > 0 && true, "rule")"#,
        r#"enum E { V(map<string, [string]>) } entry struct Root { values: [E] @unique = [.V({"a":["x"]}), .V({"a":["x"]})] }"#,
        r#"type Text = string @regex("(ab|[a-z]){2,8}") @length(1, 32); type Alias = Text; struct Row { id: u8 } entry struct Root { value: Alias @env("NAME") = "abab", items: [Row] @unique(by = "id") = [{id: 1}, {id: 2}] } @assert(self.items.len() > 0, "nonempty")"#,
    ] {
        let limits = required_limits();
        let span = source_span(&mut operations, 0, source.len())?;
        let execution = make_execution(
            &mut operations,
            ExecutionLimits::new(67_108_864, 67_108_864),
        )?;
        let mut setup =
            make_diagnostics(&mut operations, limits.max_diagnostics(), span, &execution)?;
        let mut budget = make_budget(&mut operations, limits, &execution)?;
        let loaded = SourceLoader::load(
            SchemaSource::from_str(b"root", source),
            &mut EmptyResolver,
            &mut budget,
            &mut setup,
            None,
        )
        .into_test_value()?;
        let context = make_context(&mut operations, limits, &execution)?;
        let tree = module_tree(&mut operations, &loaded, &context, &mut setup).into_test_value()?;
        let symbols =
            symbol_table(&mut operations, &tree, &context, &mut setup).into_test_value()?;
        let run = |operations: &mut TestOperation, collector: &mut DiagnosticCollector| {
            match (|| {
                let types = type_graph(operations, &tree, &symbols, &context, collector)?;
                let annotations = compile_annotations(
                    operations,
                    AnnotationCompileInput {
                        tree: &tree,
                        symbols: &symbols,
                        types: &types,
                        context: &context,
                        diagnostics: collector,
                    },
                )?;
                let expressions = compile_expressions(
                    operations,
                    ExpressionCompileInput {
                        tree: &tree,
                        symbols: &symbols,
                        types: &types,
                        annotations: &annotations,
                        diagnostics: collector,
                    },
                )?;
                let defaults = compile_defaults(
                    operations,
                    DefaultInputs {
                        symbols: &symbols,
                        types: &types,
                        annotations: &annotations,
                        expressions: &expressions,
                        context: &context,
                        fallback: tree.root_span(),
                    },
                    collector,
                )?;
                drop(defaults);
                Ok::<(), BuildFailure>(())
            })() {
                Ok(()) => Ok(()),
                Err(failure) => {
                    collector.report_failure(failure);
                    Err(BuildFailure::empty())
                }
            }
        };
        let scope = AllocationScope::start(None);
        let mut collector =
            make_diagnostics(&mut operations, limits.max_diagnostics(), span, &execution)?;
        test_verify!(run(&mut operations, &mut collector).is_ok());
        drop(collector);
        let attempts = scope.finish().attempts;
        for fail_at in 0..attempts {
            for persistent in [false, true] {
                let scope = AllocationScope::start(Some(fail_at));
                STATE.with(|state| {
                    let mut current = state.get();
                    current.persistent = persistent;
                    state.set(current);
                });
                let (failed, has_allocation) = match operations.diagnostic(|out| {
                    DiagnosticCollector::new(limits.max_diagnostics(), span, &execution, out)
                }) {
                    Ok(mut collector) => {
                        let result = run(&mut operations, &mut collector);
                        let diagnostics = collector.into_diagnostics();
                        (
                            result.is_err(),
                            diagnostics.iter().any(|diagnostic| {
                                diagnostic.code() == DiagnosticCode::ALLOCATION_FAILED
                            }),
                        )
                    }
                    Err(diagnostic) => {
                        (true, diagnostic.code() == DiagnosticCode::ALLOCATION_FAILED)
                    }
                };
                let state = scope.finish();
                test_verify!(
                    failed || has_allocation,
                    "allocation failure must stop: {fail_at}, persistent={persistent}"
                );
                test_verify!(
                    has_allocation,
                    "allocation failure must preserve exit code 2: {fail_at}, persistent={persistent}"
                );
                test_verify_eq!(
                    state.live_bytes,
                    0,
                    "failed destruction must not leak: {fail_at}, persistent={persistent}"
                );
            }
        }
    }

    Ok(())
}

struct ImportResolver;

struct ExcerptAllocationOutcome {
    state: AllocationState,
    succeeded: bool,
    snapshot_complete: bool,
    diagnostics_nonempty: bool,
}

struct CompileObservation {
    state: AllocationState,
    succeeded: bool,
    nonempty: bool,
    allocation_terminal: bool,
    unavailable: bool,
    complete: bool,
}

impl SchemaResolver for ImportResolver {
    fn resolve<'src, 'slot>(
        &'src mut self,
        request: SchemaResolveRequest<'_>,
        out: Out<'slot, '_, ResolverError>,
    ) -> OResult<'slot, SchemaSource<'src>> {
        let module = request.module_name;
        match module {
            "child" => Ok(SchemaSource::from_str(
                b"child",
                "struct Child { text: string = \"value\" }",
            )),
            "broken" => Ok(SchemaSource::from_str(b"broken", "struct Broken {")),
            _ => Err(out.fail(ResolverError::NotFound)),
        }
    }
}

fn probe_excerpt_allocation(
    source: &str,
    limits: SchemaLimits,
    fail_at: Option<usize>,
    persistent: bool,
) -> TestResult<ExcerptAllocationOutcome> {
    let scope = AllocationScope::start(fail_at);
    STATE.with(|state| {
        let mut current = state.get();
        current.persistent = persistent;
        state.set(current);
    });
    let result = SchemaCompileTestSupport::run(
        SchemaCompileOptions::new(limits),
        SchemaSource::from_str(b"root", source),
        &mut ImportResolver,
        None,
    );
    let succeeded = result.is_ok();
    let snapshot_complete = match result.as_ref() {
        Ok(schema) => schema.excerpts().status() == ExcerptStatus::Complete,
        Err(errors) => errors.excerpts().status() == ExcerptStatus::Complete,
    };
    let diagnostics_nonempty = match result.as_ref() {
        Ok(_) => true,
        Err(errors) => !errors.diagnostics().is_empty(),
    };
    drop(result);
    Ok(ExcerptAllocationOutcome {
        state: scope.finish(),
        succeeded,
        snapshot_complete,
        diagnostics_nonempty,
    })
}

#[cfg(test)]
#[test]
fn source_excerpt_allocation_failures_degrade_without_aborting() -> TestResult {
    if env::var_os("TYPOL_SCHEMA_EXCERPT_OOM_CHILD").is_none() {
        let status = Command::new(env::current_exe().into_test_value()?)
            .args([
                "--exact",
                "allocation_tests::source_excerpt_allocation_failures_degrade_without_aborting",
                "--nocapture",
            ])
            .env("TYPOL_SCHEMA_EXCERPT_OOM_CHILD", "1")
            .status()
            .into_test_value()?;
        test_verify!(
            status.success(),
            "source snapshot OOM child process must succeed"
        );
        return Ok(());
    }

    let fields: String = (0..16).map(|index| format!("field_{index}: u8,")).collect();
    let source = format!("entry struct Root {{ {fields} }}");
    let mut limits = SchemaLimits::default();
    limits.set_excerpts(ExcerptLimits::new(65_536, 1_024, 256, 1));
    let baseline = probe_excerpt_allocation(&source, limits, None, false)?;
    test_verify!(
        baseline.succeeded,
        "baseline schema must compile successfully"
    );
    test_verify!(
        baseline.snapshot_complete,
        "baseline source snapshot must be complete"
    );
    test_verify_eq!(baseline.state.live_bytes, 0);

    let mut observed_truncated_success = false;
    for fail_at in 0..baseline.state.attempts {
        for persistent in [false, true] {
            let outcome = probe_excerpt_allocation(&source, limits, Some(fail_at), persistent)?;
            test_verify!(outcome.succeeded || outcome.diagnostics_nonempty);
            test_verify_eq!(
                outcome.state.live_bytes,
                0,
                "source snapshot error-path destruction must not leak: {fail_at}, persistent={persistent}"
            );
            observed_truncated_success |= outcome.succeeded && !outcome.snapshot_complete;
        }
    }
    test_verify!(
        observed_truncated_success,
        "snapshot resource allocation failure should preserve the schema and mark the snapshot incomplete"
    );
    Ok(())
}

#[cfg(test)]
#[test]
fn public_compile_allocation_failures_are_nonempty_and_release_returned_storage() -> TestResult {
    if env::var_os("TYPOL_SCHEMA_OOM_CHILD").is_none() {
        let status = Command::new(env::current_exe().into_test_value()?)
            .args(["--exact", "allocation_tests::public_compile_allocation_failures_are_nonempty_and_release_returned_storage", "--nocapture"])
            .env("TYPOL_SCHEMA_OOM_CHILD", "1")
            .status().into_test_value()?;
        test_verify!(
            status.success(),
            "public compile entry OOM child process must succeed"
        );
        return Ok(());
    }

    for (source, valid) in [
        (
            r#"type Text = string @regex("(ab|[a-z]){2,8}") @length(1, 32); type Alias = Text; struct Row { id: u8 } entry struct Root { value: Alias @env("NAME") = "abab", items: [Row] @unique(by = "id") = [{id: 1}, {id: 2}] } @assert(self.items.len() > 0, "nonempty")"#,
            true,
        ),
        ("entry struct Root {}", true),
        (
            r#"type Base = [[i128]] @all(item.any(|x| x == -170141183460469231731687303715884105728), "PRIVATE-MESSAGE"); type Mid = Base @size(min=1); type Top = Mid @size(max=4); entry struct Root { first: Top = [[-170141183460469231731687303715884105728]], second: Top @size(max=2), }"#,
            true,
        ),
        (
            r#"type Text = string @regex("x") @secret; enum E { V([Text]) @assert(true, "PRIVATE-MESSAGE"), S { text: Text = "x" } @assert(true, "PRIVATE-MESSAGE") } entry struct Root { text: Text @env("PRIVATE_ENV") = "x", values: [E] @unique = [.V(["x"])], mapping: map<string, u8> @all(value > 0, "PRIVATE-MESSAGE") = {"key": 1} } @assert(self.values.all(|element| true), "PRIVATE-MESSAGE")"#,
            true,
        ),
        (
            r#"extern mod child; entry struct Root { child: child::Child = {}, values: [u8] @all(item > 0, "PRIVATE-MESSAGE") = [1] }"#,
            true,
        ),
        (
            r#"entry struct Root { values: [u8] @secret @all(item > 0, "PRIVATE-MESSAGE") = [0] }"#,
            false,
        ),
        (
            "extern mod missing; extern mod child; entry struct Root { bad: Missing, valid: u8 = 300 }",
            false,
        ),
        (
            "extern mod broken; extern mod child; entry struct Root { bad: broken::Broken, valid: u8 = 300 }",
            false,
        ),
        ("entry struct Root {", false),
        (
            "struct Loop { next: Loop = {} } entry struct Root { bad: Loop, valid: u8 = 300 }",
            false,
        ),
    ] {
        let limits = required_limits();
        let run = |fail_at, persistent| {
            let scope = AllocationScope::start(fail_at);
            STATE.with(|state| {
                let mut current = state.get();
                current.persistent = persistent;
                state.set(current);
            });
            let result = SchemaCompileTestSupport::run(
                SchemaCompileOptions::new(limits),
                SchemaSource::from_str(b"root", source),
                &mut ImportResolver,
                None,
            );
            let succeeded = result.is_ok();
            let mut nonempty = false;
            let mut allocation_terminal = false;
            let mut unavailable = false;
            let mut complete = true;
            if let Err(errors) = result.as_ref() {
                nonempty = !errors.diagnostics().is_empty();
                allocation_terminal = errors.diagnostics().last().is_some_and(|diagnostic| {
                    diagnostic.code() == DiagnosticCode::ALLOCATION_FAILED
                });
                unavailable = errors.location_status()
                    == LocationStatus::Unavailable(LocationFailure::AllocationFailure);
                let location_status = errors.location_status();
                complete = errors.diagnostics().iter().all(|diagnostic| {
                    diagnostic
                        .primary()
                        .into_iter()
                        .chain(diagnostic.labels().iter().map(|label| label.span()))
                        .all(|span| {
                            let location = errors.span_location(span);
                            match location_status {
                                LocationStatus::Complete => location.is_some(),
                                LocationStatus::Unavailable(_) => {
                                    let attached =
                                        errors.diagnostics().iter().find_map(|candidate| {
                                            candidate
                                                .located_primary()
                                                .filter(|located| located.source_span() == span)
                                                .and_then(|located| located.location())
                                                .or_else(|| {
                                                    candidate
                                                        .labels()
                                                        .iter()
                                                        .find(|label| label.span() == span)
                                                        .and_then(|label| {
                                                            label.located_span().location()
                                                        })
                                                })
                                        });
                                    location == attached
                                }
                            }
                        })
                });
            }
            drop(result);
            let state = scope.finish();
            CompileObservation {
                state,
                succeeded,
                nonempty,
                allocation_terminal,
                unavailable,
                complete,
            }
        };
        let baseline = run(None, false);
        test_verify_eq!(
            baseline.succeeded,
            valid,
            "test source validity must match expectations"
        );
        test_verify!(baseline.succeeded || baseline.nonempty);
        test_verify!(baseline.complete);
        test_verify_eq!(baseline.state.live_bytes, 0);
        let mut saw_unavailable = false;
        for fail_at in 0..baseline.state.attempts {
            for persistent in [false, true] {
                let observation = run(Some(fail_at), persistent);
                test_verify!(
                    !observation.succeeded
                        && observation.nonempty
                        && observation.allocation_terminal,
                    "public entry point must return non-empty exit code 2: source={source}, fail_at={fail_at}, persistent={persistent}"
                );
                test_verify!(
                    observation.complete,
                    "location completeness must be explicit: {fail_at}, persistent={persistent}"
                );
                test_verify_eq!(
                    observation.state.live_bytes,
                    0,
                    "returned object destruction must not leak: {fail_at}, persistent={persistent}"
                );
                saw_unavailable |= observation.unavailable;
                if fail_at == 0 && persistent {
                    test_verify!(
                        observation.complete,
                        "session initialization has no source span yet, and location state is complete and empty"
                    );
                }
            }
        }
        test_verify!(saw_unavailable);
    }

    Ok(())
}

struct PassMeasurement {
    steps: usize,
    bytes: usize,
    peak: usize,
}

fn measure_passes(count: usize) -> TestResult<PassMeasurement> {
    let mut operations = TestOperation::new();
    let fields: String = (0..count)
        .rev()
        .map(|index| format!("common_prefix_field_{index:06}: u8,"))
        .collect();
    let modules: String = (0..count)
        .rev()
        .map(|index| {
            format!("mod common_prefix_module_{index:06} {{ struct Item {{ value: u8 }} }}")
        })
        .collect();
    let source = format!(
        "{modules} enum E {{ V {{ {fields} }}, W(u8) }} entry struct Root {{ {fields} value: E }}"
    );
    let limits = required_limits();
    let execution = make_execution(
        &mut operations,
        ExecutionLimits::new(67_108_864, 67_108_864),
    )?;
    let context = make_context(&mut operations, limits, &execution)?;
    let span = source_span(&mut operations, 0, source.len())?;
    let mut diagnostics =
        make_diagnostics(&mut operations, limits.max_diagnostics(), span, &execution)?;
    let mut budget = make_budget(&mut operations, limits, &execution)?;
    let loaded = SourceLoader::load(
        SchemaSource::from_str(b"root", &source),
        &mut EmptyResolver,
        &mut budget,
        &mut diagnostics,
        None,
    )
    .into_test_value()?;
    let initial = execution.workspace_bytes();
    let old_peak = execution.peak_workspace_bytes();
    let initial_steps = execution.steps();
    let scope = AllocationScope::start(None);
    let tree =
        module_tree(&mut operations, &loaded, &context, &mut diagnostics).into_test_value()?;
    test_verify_eq!(
        STATE.with(Cell::get).live_bytes as usize,
        execution.workspace_bytes() - initial
    );
    let symbols =
        symbol_table(&mut operations, &tree, &context, &mut diagnostics).into_test_value()?;
    let steps = execution.steps() - initial_steps;
    let allocated = STATE.with(Cell::get);
    test_verify_eq!(
        allocated.live_bytes as usize,
        execution.workspace_bytes() - initial
    );
    test_verify_eq!(
        execution.peak_workspace_bytes(),
        old_peak.max(initial + allocated.peak_bytes as usize)
    );
    drop(tree);
    test_verify_eq!(
        STATE.with(Cell::get).live_bytes as usize,
        execution.workspace_bytes() - initial
    );
    drop(symbols);
    test_verify_eq!(execution.workspace_bytes(), initial);
    test_verify_eq!(scope.finish().live_bytes, 0);
    Ok(PassMeasurement {
        steps,
        bytes: allocated.live_bytes as usize,
        peak: allocated.peak_bytes as usize,
    })
}

#[cfg(test)]
#[test]
fn module_and_symbol_payloads_match_live_allocator_storage() -> TestResult {
    for count in [128, 256, 512] {
        let _ = measure_passes(count).into_test_value()?;
    }

    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum CompilePass {
    Types,
    Annotations,
    Expressions,
    Defaults,
    DefaultRecord,
    Regex,
}

fn measure_compile_pass(pass: CompilePass, count: usize) -> TestResult<PassMeasurement> {
    let mut operations = TestOperation::new();
    let fields: String = (0..count)
        .map(|index| format!("value_{index:06}: [u8] @all(item > 0, \"positive\") = [1, 2],"))
        .collect();
    let source = if matches!(pass, CompilePass::DefaultRecord) {
        let declaration: String = (0..count)
            .map(|index| format!("value_{index:06}: u8,"))
            .collect();
        let supplied: String = (0..count)
            .rev()
            .map(|index| format!("value_{index:06}: 1,"))
            .collect();
        format!(
            "struct Holder {{ {declaration} }} entry struct Root {{ holder: Holder = {{ {supplied} }} }}"
        )
    } else {
        format!(
            "type Text = string @regex(\"[a-z]+\") @length(1, 32); entry struct Root {{ text: Text = \"abc\", {fields} }}"
        )
    };
    let pattern = "(ab|[a-z])".repeat(count);
    let mut limits = required_limits();
    limits.set_max_regex_pattern_bytes(pattern.len());
    let execution = make_execution(
        &mut operations,
        ExecutionLimits::new(67_108_864, 67_108_864),
    )?;
    let context = make_context(&mut operations, limits, &execution)?;
    let span = source_span(&mut operations, 0, source.len())?;
    let mut diagnostics =
        make_diagnostics(&mut operations, limits.max_diagnostics(), span, &execution)?;
    let mut budget = make_budget(&mut operations, limits, &execution)?;
    let loaded = SourceLoader::load(
        SchemaSource::from_str(b"root", &source),
        &mut EmptyResolver,
        &mut budget,
        &mut diagnostics,
        None,
    )
    .into_test_value()?;
    let tree =
        module_tree(&mut operations, &loaded, &context, &mut diagnostics).into_test_value()?;
    let symbols =
        symbol_table(&mut operations, &tree, &context, &mut diagnostics).into_test_value()?;
    let types = if matches!(pass, CompilePass::Types) {
        None
    } else {
        Some(
            type_graph(&mut operations, &tree, &symbols, &context, &mut diagnostics)
                .into_test_value()?,
        )
    };
    let mut annotations = if matches!(
        pass,
        CompilePass::Expressions | CompilePass::Defaults | CompilePass::DefaultRecord
    ) {
        let types = types.as_ref().into_test_value()?;
        Some(
            compile_annotations(
                &mut operations,
                AnnotationCompileInput {
                    tree: &tree,
                    symbols: &symbols,
                    types,
                    context: &context,
                    diagnostics: &mut diagnostics,
                },
            )
            .into_test_value()?,
        )
    } else {
        None
    };
    let expressions = if matches!(pass, CompilePass::Defaults | CompilePass::DefaultRecord) {
        let expressions = compile_expressions(
            &mut operations,
            ExpressionCompileInput {
                tree: &tree,
                symbols: &symbols,
                types: types.as_ref().into_test_value()?,
                annotations: annotations.as_ref().into_test_value()?,
                diagnostics: &mut diagnostics,
            },
        )
        .into_test_value()?;
        let annotations = annotations.as_mut().into_test_value()?;
        operations
            .build(|out| annotations.bind_programs(&expressions, &context, &mut diagnostics, out))
            .into_test_value()?;
        Some(expressions)
    } else {
        None
    };
    let initial = execution.workspace_bytes();
    let old_peak = execution.peak_workspace_bytes();
    let initial_steps = execution.steps();
    let scope = AllocationScope::start(None);
    let output = match pass {
        CompilePass::Types => MeasuredOutput::types(
            type_graph(&mut operations, &tree, &symbols, &context, &mut diagnostics)
                .into_test_value()?,
        ),
        CompilePass::Annotations => MeasuredOutput::annotations(
            compile_annotations(
                &mut operations,
                AnnotationCompileInput {
                    tree: &tree,
                    symbols: &symbols,
                    types: types.as_ref().into_test_value()?,
                    context: &context,
                    diagnostics: &mut diagnostics,
                },
            )
            .into_test_value()?,
        ),
        CompilePass::Expressions => MeasuredOutput::expressions(
            compile_expressions(
                &mut operations,
                ExpressionCompileInput {
                    tree: &tree,
                    symbols: &symbols,
                    types: types.as_ref().into_test_value()?,
                    annotations: annotations.as_ref().into_test_value()?,
                    diagnostics: &mut diagnostics,
                },
            )
            .into_test_value()?,
        ),
        CompilePass::Defaults | CompilePass::DefaultRecord => MeasuredOutput::defaults(
            compile_defaults(
                &mut operations,
                DefaultInputs {
                    symbols: &symbols,
                    types: types.as_ref().into_test_value()?,
                    annotations: annotations.as_ref().into_test_value()?,
                    expressions: expressions.as_ref().into_test_value()?,
                    context: &context,
                    fallback: tree.root_span(),
                },
                &mut diagnostics,
            )
            .into_test_value()?,
        ),
        CompilePass::Regex => MeasuredOutput::regex(
            operations
                .regex(|out| RegexCompiler::compile(&pattern, &limits, &execution, out))
                .into_test_value()?,
        ),
    };
    let steps = execution.steps() - initial_steps;
    let allocated = STATE.with(Cell::get);
    test_verify_eq!(
        allocated.live_bytes as usize,
        execution.workspace_bytes() - initial
    );
    test_verify_eq!(
        execution.peak_workspace_bytes(),
        old_peak.max(initial + allocated.peak_bytes as usize)
    );
    output.release();
    test_verify_eq!(execution.workspace_bytes(), initial);
    test_verify_eq!(scope.finish().live_bytes, 0);
    test_verify!(!diagnostics.has_diagnostics());
    Ok(PassMeasurement {
        steps,
        bytes: allocated.live_bytes as usize,
        peak: allocated.peak_bytes as usize,
    })
}

struct MeasuredOutput {
    types: Option<ResolvedTypeGraph>,
    annotations: Option<AnnotationCompilation>,
    expressions: Option<ExpressionCompilation>,
    defaults: Option<DefaultCompilation>,
    regex: Option<Regex>,
}

impl MeasuredOutput {
    fn empty() -> Self {
        Self {
            types: None,
            annotations: None,
            expressions: None,
            defaults: None,
            regex: None,
        }
    }

    fn types(value: ResolvedTypeGraph) -> Self {
        Self {
            types: Some(value),
            ..Self::empty()
        }
    }

    fn annotations(value: AnnotationCompilation) -> Self {
        Self {
            annotations: Some(value),
            ..Self::empty()
        }
    }

    fn expressions(value: ExpressionCompilation) -> Self {
        Self {
            expressions: Some(value),
            ..Self::empty()
        }
    }

    fn defaults(value: DefaultCompilation) -> Self {
        Self {
            defaults: Some(value),
            ..Self::empty()
        }
    }

    fn regex(value: Regex) -> Self {
        Self {
            regex: Some(value),
            ..Self::empty()
        }
    }

    fn release(self) {
        drop(self.types);
        drop(self.annotations);
        drop(self.expressions);
        drop(self.defaults);
        drop(self.regex);
    }
}

#[cfg(test)]
#[test]
fn compilation_pass_payloads_and_work_scale_with_input() -> TestResult {
    for pass in [
        CompilePass::Types,
        CompilePass::Annotations,
        CompilePass::Expressions,
        CompilePass::Defaults,
        CompilePass::DefaultRecord,
        CompilePass::Regex,
    ] {
        let small = measure_compile_pass(pass, 128).into_test_value()?;
        let medium = measure_compile_pass(pass, 256).into_test_value()?;
        let large = measure_compile_pass(pass, 512).into_test_value()?;
        for (left, right) in [(&small, &medium), (&medium, &large)] {
            test_verify!(
                right.steps > left.steps && right.steps < left.steps * 3,
                "{pass:?} steps: {} -> {}",
                left.steps,
                right.steps
            );
            test_verify!(
                right.bytes > left.bytes && right.bytes < left.bytes * 3,
                "{pass:?} bytes: {} -> {}",
                left.bytes,
                right.bytes
            );
            test_verify!(
                right.peak > left.peak && right.peak < left.peak * 3,
                "{pass:?} peak: {} -> {}",
                left.peak,
                right.peak
            );
        }
    }

    Ok(())
}

fn measure_artifact(kind: &str, count: usize) -> TestResult<PassMeasurement> {
    let mut source = String::from("entry struct Root {");
    for index in (0..count).rev() {
        if kind == "labels" {
            source.push_str("duplicate: u8,");
        } else {
            source.push_str(&format!("common_prefix_{index:06}: [u8; {}],", index + 1));
        }
    }
    source.push('}');
    if kind == "long_line" {
        source = format!("{}{}", " ".repeat(count * 128), source);
    }
    let mut limits = required_limits();
    limits.set_max_diagnostics(count * 2 + 2);
    let scope = AllocationScope::start(None);
    let mut compile_errors = CompileRunErrors::new();
    let run = CompileRunErrors::run_with_context(
        SchemaCompileOptions::new(limits),
        SchemaSource::from_str(b"root", &source),
        &mut EmptyResolver,
        None,
        &mut compile_errors,
    )
    .into_test_value()?;
    let context = run.context;
    let result = run.result;
    test_verify_eq!(result.is_ok(), kind != "labels");
    let usage = context.usage();
    let allocated = STATE.with(Cell::get);
    test_verify_eq!(allocated.live_bytes as usize, usage.workspace_bytes);
    test_verify_eq!(allocated.peak_bytes as usize, usage.peak_workspace_bytes);
    if let Err(errors) = &result {
        test_verify_eq!(errors.location_status(), LocationStatus::Complete);
        test_verify!(
            errors
                .diagnostics()
                .iter()
                .any(|error| !error.labels().is_empty())
        );
    }
    drop(result);
    test_verify_eq!(
        STATE.with(Cell::get).live_bytes as usize,
        context.usage().workspace_bytes
    );
    drop(context);
    test_verify_eq!(scope.finish().live_bytes, 0);
    Ok(PassMeasurement {
        steps: usage.steps,
        bytes: allocated.live_bytes as usize,
        peak: allocated.peak_bytes as usize,
    })
}

#[cfg(test)]
#[test]
fn returned_artifacts_and_errors_match_real_live_and_peak_payloads() -> TestResult {
    for kind in ["shapes", "labels", "long_line"] {
        let mut previous: Option<PassMeasurement> = None;
        for count in [128, 256, 512] {
            let sample = measure_artifact(kind, count).into_test_value()?;
            if let Some(previous) = previous {
                test_verify!(sample.steps > previous.steps && sample.steps < previous.steps * 4);
                test_verify!(sample.bytes > previous.bytes && sample.bytes < previous.bytes * 4);
                test_verify!(sample.peak > previous.peak && sample.peak < previous.peak * 4);
            }
            previous = Some(sample);
        }
    }

    Ok(())
}

#[cfg(test)]
#[test]
fn location_oom_keeps_existing_messages_labels_and_first_terminal_reason() -> TestResult {
    let source = "entry struct Root { duplicate: u8, duplicate: u8 }";
    let run = |fail_at, persistent| {
        let scope = AllocationScope::start(fail_at);
        STATE.with(|state| {
            let mut current = state.get();
            current.persistent = persistent;
            state.set(current);
        });
        let mut compile_errors = CompileRunErrors::new();
        let run = CompileRunErrors::run_with_context(
            SchemaCompileOptions::new(required_limits()),
            SchemaSource::from_str(b"root", source),
            &mut EmptyResolver,
            None,
            &mut compile_errors,
        )
        .into_test_value()?;
        let context = run.context;
        let errors = run.result.into_test_error()?;
        let usage = context.usage();
        test_verify_eq!(
            STATE.with(Cell::get).live_bytes as usize,
            usage.workspace_bytes
        );
        test_verify_eq!(
            errors.diagnostics().first().into_test_value()?.code(),
            DiagnosticCode::SCHEMA_SEMANTIC_ERROR
        );
        test_verify!(
            !errors
                .diagnostics()
                .first()
                .into_test_value()?
                .message()
                .is_empty()
        );
        test_verify_eq!(
            errors
                .diagnostics()
                .first()
                .into_test_value()?
                .labels()
                .len(),
            1
        );
        if fail_at.is_some() {
            test_verify_eq!(errors.diagnostics().len(), 2);
            test_verify_eq!(
                errors.diagnostics().get(1).into_test_value()?.code(),
                DiagnosticCode::ALLOCATION_FAILED
            );
            test_verify!(
                errors
                    .diagnostics()
                    .get(1)
                    .into_test_value()?
                    .message()
                    .is_empty()
            );
            test_verify_eq!(
                errors.location_status(),
                LocationStatus::Unavailable(LocationFailure::AllocationFailure)
            );
            test_verify!(errors.diagnostics().iter().all(|error| {
                error
                    .primary()
                    .is_some_and(|span| errors.span_location(span).is_none())
            }));
        } else {
            test_verify_eq!(errors.location_status(), LocationStatus::Complete);
        }
        test_verify!(STATE.with(Cell::get).peak_bytes as usize <= usage.peak_workspace_bytes);
        drop(errors);
        drop(context);
        let state = scope.finish();
        test_verify_eq!(state.live_bytes, 0);
        Ok(state.attempts)
    };
    let attempts = run(None, false).into_test_value()?;
    for persistent in [false, true] {
        run(Some(attempts - 1), persistent).into_test_value()?;
    }

    Ok(())
}

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

struct AcceptanceMeasurement {
    usage: SchemaCompileUsage,
    succeeded: bool,
}

fn acceptance_measure(
    source: &str,
    kind: &str,
    count: usize,
    quota: ExecutionLimits,
) -> TestResult<AcceptanceMeasurement> {
    let mut limits = required_limits();
    if kind == "labels" {
        limits.set_max_diagnostics(count * 2 + 2);
    }
    let scope = AllocationScope::start(None);
    let mut compile_errors = CompileRunErrors::new();
    let run = CompileRunErrors::run_with_context(
        SchemaCompileOptions::new(limits).with_execution(quota),
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
        usage.workspace_bytes,
        "{kind}/{count}"
    );
    test_verify_eq!(
        state.peak_bytes as usize,
        usage.peak_workspace_bytes,
        "{kind}/{count}"
    );
    let succeeded = result.is_ok();
    if let Err(errors) = &result {
        test_verify!(!errors.diagnostics().is_empty());
        if usage.failure.is_some() {
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

#[cfg(test)]
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
            )
            .into_test_value()?;
            test_verify_eq!(exact.succeeded, sample.succeeded);
            test_verify_eq!(exact.usage, sample.usage);
            for quota in [
                ExecutionLimits::new(sample.usage.steps - 1, 67_108_864),
                ExecutionLimits::new(67_108_864, sample.usage.peak_workspace_bytes - 1),
            ] {
                let failure = acceptance_measure(&source, kind, count, quota).into_test_value()?;
                test_verify!(!failure.succeeded);
                test_verify_eq!(failure.usage.failure, Some(BudgetError::Resource));
            }
        }
    }

    Ok(())
}

fn required_limits() -> SchemaLimits {
    let mut limits = SchemaLimits::default();
    limits.set_excerpts(ExcerptLimits::new(0, 0, 0, 0));
    limits
}
