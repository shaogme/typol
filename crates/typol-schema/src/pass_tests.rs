use alloc::{format, string::String, vec::Vec};
use typol_core::{
    BudgetError, ChargedVec, DiagnosticCode, ExecutionBudget, ExecutionLimits,
    RegexExecutionBudget, SourceId, SourceSpan, Span,
};
use typol_policy::{RegexExecutionLimits, RegexOptions};
use typol_semantic::{EvalError, ExecutionModel};

use crate::{
    ResolverError, SchemaCompileOptions, SchemaLimits, SchemaResolveRequest, SchemaResolver,
    SchemaSource,
    annotations::{AnnotationCompilation, AnnotationCompileInput, AnnotationCompiler},
    context::CompileContext,
    diagnostics::DiagnosticCollector,
    execution::CompileModel,
    expressions::{ExpressionCompilation, ExpressionCompileInput, ExpressionCompiler},
    loader::SourceLoader,
    module_tree::ModuleTree,
    session::{CompileBudget, SourceCatalog},
    symbols::{LookupResult, SymbolTable},
    test_support::{SchemaCompileTestSupport, TestOperation},
    types::{ResolvedTypeGraph, TypeGraphCompiler},
};
use opeo::{OResult, Out};
use typol_test_support::*;

const LIMIT: usize = 67_108_864;

struct EvalErrorSlot {
    errors: TestSlot<EvalError>,
}

impl EvalErrorSlot {
    fn new() -> Self {
        Self {
            errors: TestSlot::default(),
        }
    }

    fn call<T>(
        &mut self,
        operation: impl for<'slot> FnOnce(Out<'slot, 'slot, EvalError>) -> OResult<'slot, T>,
    ) -> Result<T, EvalError> {
        match self.errors.access(operation) {
            Ok(value) => Ok(value),
            Err(error) => Err(error),
        }
    }
}

fn source_span(operations: &mut TestOperation, start: usize, end: usize) -> TestResult<SourceSpan> {
    let span = operations
        .span(|out| Span::new(start, end, out))
        .into_test_value()?;
    Ok(SourceSpan::new(SourceId::new(0), span))
}

fn execution(
    operations: &mut TestOperation,
    limits: ExecutionLimits,
) -> TestResult<ExecutionBudget> {
    operations
        .budget(|out| ExecutionBudget::new(limits, out))
        .into_test_value()
}

fn context(
    operations: &mut TestOperation,
    limits: SchemaLimits,
    execution: &ExecutionBudget,
) -> TestResult<CompileContext> {
    let regex_options = RegexOptions::default();
    let regex_limits = RegexExecutionLimits::default();
    let regex_execution = operations
        .budget(|out| {
            RegexExecutionBudget::new(
                regex_limits.max_steps(),
                regex_limits.max_search_steps(),
                regex_limits.max_workspace_bytes(),
                out,
            )
        })
        .into_test_value()?;
    operations
        .budget(|out| {
            CompileContext::new(
                limits,
                regex_options.limits(),
                execution,
                &regex_execution,
                out,
            )
        })
        .into_test_value()
}

fn diagnostics(
    operations: &mut TestOperation,
    limit: usize,
    span: SourceSpan,
    execution: &ExecutionBudget,
) -> TestResult<DiagnosticCollector> {
    operations
        .diagnostic(|out| DiagnosticCollector::new(limit, span, execution, out))
        .into_test_value()
}

fn compile_budget(
    operations: &mut TestOperation,
    limits: SchemaLimits,
    execution: &ExecutionBudget,
) -> TestResult<CompileBudget> {
    let regex_options = RegexOptions::default();
    let regex_limits = RegexExecutionLimits::default();
    let regex_execution = operations
        .budget(|out| {
            RegexExecutionBudget::new(
                regex_limits.max_steps(),
                regex_limits.max_search_steps(),
                regex_limits.max_workspace_bytes(),
                out,
            )
        })
        .into_test_value()?;
    operations
        .budget(|out| CompileBudget::new(limits, &regex_options, execution, &regex_execution, out))
        .into_test_value()
}

fn module_tree<'a>(
    operations: &mut TestOperation,
    sources: &'a SourceCatalog,
    context: &CompileContext,
    diagnostics: &mut DiagnosticCollector,
) -> TestResult<ModuleTree<'a>> {
    operations
        .build(|out| ModuleTree::build(sources, context, diagnostics, out))
        .into_test_value()
}

fn symbol_table<'a>(
    operations: &mut TestOperation,
    tree: &ModuleTree<'a>,
    context: &CompileContext,
    diagnostics: &mut DiagnosticCollector,
) -> TestResult<SymbolTable<'a>> {
    operations
        .build(|out| SymbolTable::build(tree, context, diagnostics, out))
        .into_test_value()
}

fn type_graph<'a>(
    operations: &mut TestOperation,
    tree: &'a ModuleTree<'a>,
    symbols: &'a SymbolTable<'a>,
    context: &'a CompileContext,
    diagnostics: &mut DiagnosticCollector,
) -> TestResult<ResolvedTypeGraph> {
    operations
        .build(|out| TypeGraphCompiler::build(tree, symbols, context, diagnostics, out))
        .into_test_value()
}

fn annotation_compilation<'tree, 'symbol, 'types, 'context, 'diagnostics>(
    operations: &mut TestOperation,
    input: AnnotationCompileInput<'tree, 'symbol, 'types, 'context, 'diagnostics>,
) -> TestResult<AnnotationCompilation> {
    operations
        .build(|out| AnnotationCompiler::compile(input, out))
        .into_test_value()
}

fn expression_compilation<'tree, 'src, 'annotations, 'diagnostics>(
    operations: &mut TestOperation,
    input: ExpressionCompileInput<'tree, 'src, 'annotations, 'diagnostics>,
) -> TestResult<ExpressionCompilation> {
    operations
        .build(|out| ExpressionCompiler::compile(input, out))
        .into_test_value()
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

fn load(
    operations: &mut TestOperation,
    source: &str,
    execution: &ExecutionBudget,
) -> TestResult<(SourceCatalog, DiagnosticCollector)> {
    let limits = SchemaLimits::default();
    let span = source_span(operations, 0, source.len())?;
    let mut diagnostics = diagnostics(operations, limits.max_diagnostics(), span, execution)?;
    let mut budget = compile_budget(operations, limits, execution)?;
    let sources = SourceLoader::load(
        SchemaSource::from_str(b"root", source),
        &mut NoImports,
        &mut budget,
        &mut diagnostics,
        None,
    )
    .into_test_value()?;
    test_verify!(!diagnostics.has_diagnostics());
    Ok((sources, diagnostics))
}

fn leave_one_step(operations: &mut TestOperation, execution: &ExecutionBudget) -> TestResult {
    operations
        .budget(|out| execution.consume(LIMIT - execution.steps() - 1, out))
        .into_test_value()?;
    Ok(())
}

#[test]
fn module_and_symbol_passes_stop_with_shared_budget_and_release_payloads() -> TestResult {
    let mut operations = TestOperation::new();
    let source = "mod a { struct A { value: u8 } } entry struct Root { value: a::A }";
    for stop_in_symbols in [false, true] {
        let execution = execution(&mut operations, ExecutionLimits::new(LIMIT, LIMIT))?;
        let retained = execution.workspace_bytes();
        let context = context(&mut operations, SchemaLimits::default(), &execution)?;
        let (sources, mut diagnostics) = load(&mut operations, source, &execution)?;
        if stop_in_symbols {
            let tree = module_tree(&mut operations, &sources, &context, &mut diagnostics)?;
            leave_one_step(&mut operations, &execution)?;
            test_verify!(symbol_table(&mut operations, &tree, &context, &mut diagnostics).is_err());
        } else {
            leave_one_step(&mut operations, &execution)?;
            test_verify!(
                module_tree(&mut operations, &sources, &context, &mut diagnostics).is_err()
            );
        }
        test_verify!(execution.exhausted());
        let errors = diagnostics.into_diagnostics();
        test_verify_eq!(
            errors.last().into_test_value()?.code(),
            DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
        );
        drop(errors);
        drop(sources);
        test_verify_eq!(execution.workspace_bytes(), retained);
    }

    Ok(())
}

#[test]
fn lookup_exhaustion_is_distinct_from_missing_or_ambiguous_names() -> TestResult {
    let mut operations = TestOperation::new();
    let execution = execution(&mut operations, ExecutionLimits::new(LIMIT, LIMIT))?;
    let context = context(&mut operations, SchemaLimits::default(), &execution)?;
    let (sources, mut diagnostics) = load(
        &mut operations,
        "entry struct Root { field: u8 }",
        &execution,
    )?;
    let tree = module_tree(&mut operations, &sources, &context, &mut diagnostics)?;
    let symbols = symbol_table(&mut operations, &tree, &context, &mut diagnostics)?;
    let owner = symbols.entry_type().into_test_value()?;
    test_verify!(matches!(
        operations.budget(|out| symbols.lookup_field(owner, "field", out)),
        Ok(LookupResult::Found(_))
    ));
    test_verify!(matches!(
        operations.budget(|out| symbols.lookup_field(owner, "missing", out)),
        Ok(LookupResult::Missing)
    ));
    leave_one_step(&mut operations, &execution)?;
    test_verify_eq!(
        operations.budget(|out| symbols.lookup_field(owner, "field", out)),
        Err(BudgetError::Resource)
    );
    test_verify_eq!(
        operations.budget(|out| symbols.lookup_field(owner, "missing", out)),
        Err(BudgetError::Resource)
    );

    Ok(())
}

struct Usage {
    steps: usize,
    bytes: usize,
    lookup_steps: usize,
}

fn profile(count: usize) -> TestResult<Usage> {
    let mut operations = TestOperation::new();
    let prefix = "shared_prefix_".repeat(8);
    let fields: String = (0..count)
        .rev()
        .map(|index| format!("{prefix}{index:06}: u8,"))
        .collect();
    let source = format!("entry struct Root {{ {fields} }}");
    let execution = execution(&mut operations, ExecutionLimits::new(LIMIT, LIMIT))?;
    let context = context(&mut operations, SchemaLimits::default(), &execution)?;
    let (sources, mut diagnostics) = load(&mut operations, &source, &execution)?;
    let initial_steps = execution.steps();
    let initial_bytes = execution.workspace_bytes();
    let tree = module_tree(&mut operations, &sources, &context, &mut diagnostics)?;
    let symbols = symbol_table(&mut operations, &tree, &context, &mut diagnostics)?;
    let steps = execution.steps() - initial_steps;
    let bytes = execution.workspace_bytes() - initial_bytes;
    let initial_steps = execution.steps();
    let owner = symbols.entry_type().into_test_value()?;
    for index in 0..count {
        test_verify!(matches!(
            operations
                .budget(|out| { symbols.lookup_field(owner, &format!("{prefix}{index:06}"), out) }),
            Ok(LookupResult::Found(_))
        ));
    }
    let lookup_steps = execution.steps() - initial_steps;
    drop(symbols);
    drop(tree);
    test_verify_eq!(execution.workspace_bytes(), initial_bytes);
    Ok(Usage {
        steps,
        bytes,
        lookup_steps,
    })
}

#[test]
fn long_prefix_wide_names_use_linear_storage_and_subquadratic_work() -> TestResult {
    let small = profile(128).into_test_value()?;
    let medium = profile(256).into_test_value()?;
    let large = profile(512).into_test_value()?;
    for (a, b) in [(&small, &medium), (&medium, &large)] {
        test_verify!(b.steps > a.steps && b.steps < a.steps * 3);
        test_verify!(b.lookup_steps > a.lookup_steps && b.lookup_steps < a.lookup_steps * 3);
        test_verify!(b.bytes > a.bytes && b.bytes < a.bytes * 3);
    }

    Ok(())
}

#[test]
fn public_entry_stops_in_passes_and_propagates_type_lookup_exhaustion() -> TestResult {
    let mut operations = TestOperation::new();
    let source = "mod a { struct A { value: u8 } } entry struct Root { value: a::A }";
    let execution = execution(&mut operations, ExecutionLimits::new(LIMIT, LIMIT))?;
    let context = context(&mut operations, SchemaLimits::default(), &execution)?;
    let (sources, mut diagnostics) = load(&mut operations, source, &execution)?;
    let after_loading = execution.steps();
    let tree = module_tree(&mut operations, &sources, &context, &mut diagnostics)?;
    let after_modules = execution.steps();
    let symbols = symbol_table(&mut operations, &tree, &context, &mut diagnostics)?;
    let after_symbols = execution.steps();
    for (cap, expected) in [
        (after_loading + 1, source),
        (after_modules + 1, "A"),
        (after_symbols + 1, source),
    ] {
        let errors = SchemaCompileTestSupport::run(
            SchemaCompileOptions::default().with_execution(ExecutionLimits::new(cap, LIMIT)),
            SchemaSource::from_str(b"root", source),
            &mut NoImports,
            None,
        )
        .into_test_error()?;
        test_verify_eq!(errors.diagnostics().len(), 1);
        let terminal = &errors.diagnostics().first().into_test_value()?;
        test_verify_eq!(terminal.code(), DiagnosticCode::RESOURCE_LIMIT_EXCEEDED);
        let span = terminal.primary().into_test_value()?.span();
        test_verify_eq!(
            source.get(span.start()..span.end()).into_test_value()?,
            expected
        );
    }
    drop(symbols);
    drop(tree);

    Ok(())
}

#[test]
fn module_and_symbol_allocations_are_rejected_before_capacity_limit() -> TestResult {
    let mut operations = TestOperation::new();
    let source = "entry struct Root { value: u8 }";
    for stop_in_symbols in [false, true] {
        let execution = execution(&mut operations, ExecutionLimits::new(LIMIT, LIMIT))?;
        let retained = execution.workspace_bytes();
        let context = context(&mut operations, SchemaLimits::default(), &execution)?;
        let (sources, mut diagnostics) = load(&mut operations, source, &execution)?;
        let tree = if stop_in_symbols {
            Some(module_tree(
                &mut operations,
                &sources,
                &context,
                &mut diagnostics,
            )?)
        } else {
            None
        };
        let reserved = LIMIT - execution.workspace_bytes() - 1;
        let mut reservation = operations
            .budget(|out| ChargedVec::<u8>::new(Some(&execution), out))
            .into_test_value()?;
        operations
            .budget(|out| reservation.charge_storage(reserved, out))
            .into_test_value()?;
        if let Some(tree) = &tree {
            test_verify!(symbol_table(&mut operations, tree, &context, &mut diagnostics).is_err());
        } else {
            test_verify!(
                module_tree(&mut operations, &sources, &context, &mut diagnostics).is_err()
            );
        }
        test_verify!(execution.exhausted());
        test_verify!(execution.workspace_bytes() <= LIMIT);
        test_verify_eq!(
            diagnostics
                .into_diagnostics()
                .last()
                .into_test_value()?
                .code(),
            DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
        );
        drop(reservation);
        drop(tree);
        drop(sources);
        test_verify_eq!(execution.workspace_bytes(), retained);
    }

    Ok(())
}

#[test]
fn public_entry_can_stop_in_types_annotations_expressions_and_defaults() -> TestResult {
    let mut operations = TestOperation::new();
    let source = r#"type Text = string @regex("[a-z]+") @length(1, 32);
        entry struct Root { name: Text = "abc", values: [u8] @all(item > 0, "positive") = [1, 2] }
        @assert(self.values.len() > 0, "nonempty")"#;
    let execution = execution(&mut operations, ExecutionLimits::new(LIMIT, LIMIT))?;
    let context = context(&mut operations, SchemaLimits::default(), &execution)?;
    let (sources, mut diagnostics) = load(&mut operations, source, &execution)?;
    let tree = module_tree(&mut operations, &sources, &context, &mut diagnostics)?;
    let symbols = symbol_table(&mut operations, &tree, &context, &mut diagnostics)?;
    let mut boundaries = Vec::new();
    boundaries.push(execution.steps());
    let types = type_graph(&mut operations, &tree, &symbols, &context, &mut diagnostics)?;
    boundaries.push(execution.steps());
    let mut annotations = annotation_compilation(
        &mut operations,
        AnnotationCompileInput {
            tree: &tree,
            symbols: &symbols,
            types: &types,
            context: &context,
            diagnostics: &mut diagnostics,
        },
    )?;
    boundaries.push(execution.steps());
    let expressions = expression_compilation(
        &mut operations,
        ExpressionCompileInput {
            tree: &tree,
            symbols: &symbols,
            types: &types,
            annotations: &annotations,
            diagnostics: &mut diagnostics,
        },
    )?;
    operations
        .build(|out| annotations.bind_programs(&expressions, &context, &mut diagnostics, out))
        .into_test_value()?;
    boundaries.push(execution.steps());
    for cap in boundaries {
        let errors = SchemaCompileTestSupport::run(
            SchemaCompileOptions::default().with_execution(ExecutionLimits::new(cap + 1, LIMIT)),
            SchemaSource::from_str(b"root", source),
            &mut NoImports,
            None,
        )
        .into_test_error()?;
        test_verify!(
            errors
                .diagnostics()
                .iter()
                .any(|error| error.code() == DiagnosticCode::RESOURCE_LIMIT_EXCEEDED)
        );
        test_verify!(
            errors
                .diagnostics()
                .iter()
                .all(|error| error.code() != DiagnosticCode::SCHEMA_SEMANTIC_ERROR)
        );
    }
    Ok(())
}

#[test]
fn controlled_navigation_never_turns_exhaustion_into_false_or_missing() -> TestResult {
    let mut operations = TestOperation::new();
    let source = "type Base = string @host; type Alias = Base; entry struct Root { value: Alias }";
    let execution = execution(&mut operations, ExecutionLimits::new(LIMIT, LIMIT))?;
    let context = context(&mut operations, SchemaLimits::default(), &execution)?;
    let (sources, mut diagnostics) = load(&mut operations, source, &execution)?;
    let tree = module_tree(&mut operations, &sources, &context, &mut diagnostics)?;
    let symbols = symbol_table(&mut operations, &tree, &context, &mut diagnostics)?;
    let types = type_graph(&mut operations, &tree, &symbols, &context, &mut diagnostics)?;
    let annotations = annotation_compilation(
        &mut operations,
        AnnotationCompileInput {
            tree: &tree,
            symbols: &symbols,
            types: &types,
            context: &context,
            diagnostics: &mut diagnostics,
        },
    )?;
    let model = CompileModel::new(&types, &symbols, &annotations);
    let field = symbols.fields().first().into_test_value()?.id();
    let mut eval_errors = EvalErrorSlot::new();
    let type_use = types.field_type(field).into_test_value()?;
    let type_id = eval_errors
        .call(|out| model.use_id(type_use, out))
        .into_test_value()?;
    let ty = type_id.into_test_value()?;
    test_verify_eq!(eval_errors.call(|out| model.host(ty, out)), Ok(Some(true)));
    operations
        .budget(|out| execution.consume(LIMIT - execution.steps(), out))
        .into_test_value()?;
    test_verify_eq!(
        eval_errors.call(|out| model.same_type(ty, ty, out)),
        Err(EvalError::Resource)
    );
    test_verify_eq!(
        eval_errors.call(|out| model.host(ty, out)),
        Err(EvalError::Resource)
    );
    test_verify!(matches!(
        eval_errors.call(|out| model.field_info(field, out)),
        Err(EvalError::Resource)
    ));

    Ok(())
}
