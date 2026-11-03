use alloc::{format, string::String, vec::Vec};

use typol_core::{
    Diagnostic, DiagnosticCode, ExecutionBudget, ExecutionLimits, SourceId, SourceSpan, Span,
};

use crate::{
    ResolverError, SchemaLimits, SchemaResolveRequest, SchemaResolver, SchemaSource,
    context::CompileContext,
    diagnostics::DiagnosticCollector,
    expressions::ExpressionType,
    loader::SourceLoader,
    module_tree::ModuleTree,
    session::CompileBudget,
    symbols::SymbolTable,
    test_support::TestOperation,
    types::{ResolvedTypeGraph, ResolvedTypeKind, TypeGraphCompiler, model::AliasLinkId},
};
use opeo::{OResult, Out};
use typol_test_support::*;

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

#[derive(Debug)]
enum TypeGraphFailure {
    Diagnostics(Vec<Diagnostic>),
    Test(TestFailure),
}

impl From<String> for TypeGraphFailure {
    fn from(message: String) -> Self {
        Self::Test(TestFailure::from(message))
    }
}

impl From<TestFailure> for TypeGraphFailure {
    fn from(failure: TestFailure) -> Self {
        Self::Test(failure)
    }
}

fn type_graph(source: &str) -> Result<ResolvedTypeGraph, TypeGraphFailure> {
    type_graph_with_step_budget(source, 67_108_864)
}

fn type_graph_with_step_budget(
    source: &str,
    max_steps: usize,
) -> Result<ResolvedTypeGraph, TypeGraphFailure> {
    let mut operations = TestOperation::new();
    let limits = SchemaLimits::default();
    let root = SchemaSource::from_str(b"root", source);
    let execution = operations
        .budget(|out| ExecutionBudget::new(ExecutionLimits::new(max_steps, 67_108_864), out))
        .into_test_value()?;
    let mut collector = diagnostics(&mut operations, &limits, source, &execution)?;
    let mut budget = operations
        .budget(|out| CompileBudget::new(limits, &execution, out))
        .into_test_value()?;
    let loaded = SourceLoader::load(root, &mut EmptyResolver, &mut budget, &mut collector, None)
        .into_test_value()?;
    let context = operations
        .budget(|out| CompileContext::new(limits, &execution, out))
        .into_test_value()?;
    let tree = operations
        .build(|out| ModuleTree::build(&loaded, &context, &mut collector, out))
        .into_test_value()?;
    let symbols = operations
        .build(|out| SymbolTable::build(&tree, &context, &mut collector, out))
        .into_test_value()?;
    let graph = operations
        .build(|out| TypeGraphCompiler::build(&tree, &symbols, &context, &mut collector, out))
        .into_test_value()?;
    let errors = collector.into_diagnostics();
    if errors.is_empty() {
        Ok(graph)
    } else {
        Err(TypeGraphFailure::Diagnostics(errors.into_vec_for_test()))
    }
}

fn diagnostics(
    operations: &mut TestOperation,
    limits: &SchemaLimits,
    source: &str,
    execution: &ExecutionBudget,
) -> TestResult<DiagnosticCollector> {
    let span = SourceSpan::new(
        SourceId::new(0),
        operations
            .span(|out| Span::new(0, source.len(), out))
            .into_test_value()?,
    );
    operations
        .diagnostic(|out| DiagnosticCollector::new(limits.max_diagnostics(), span, execution, out))
        .into_test_value()
}

fn single_diagnostic(source: &str) -> TestResult<Diagnostic> {
    match type_graph(source) {
        Err(TypeGraphFailure::Diagnostics(diagnostics)) => {
            diagnostics.into_iter().next().into_test_value()
        }
        Err(TypeGraphFailure::Test(message)) => Err(message),
        Ok(_) => Err(TestFailure::new("expected type resolution to fail")),
    }
}

fn assert_primary_text(source: &str, diagnostic: &Diagnostic, expected: &str) -> TestResult {
    let span = diagnostic.primary().into_test_value()?.span();
    test_verify_eq!(source.get(span.start()..span.end()), Some(expected));
    Ok(())
}

fn alias_chain_len(graph: &ResolvedTypeGraph, mut link: Option<AliasLinkId>) -> TestResult<usize> {
    let mut count = 0;
    while let Some(id) = link {
        let node = graph.alias_link(id).into_test_value()?;
        let _ = node.symbol().index();
        count += 1;
        link = node.next();
    }
    Ok(count)
}

#[test]
fn resolves_forward_paths_containers_and_alias_chains() -> TestResult {
    let source = "type LeafName = Leaf; type LeafAlias = LeafName; type MaybeLeaf = option<LeafName>; entry struct Root { items: [MaybeLeaf; 2], lookup: map<string, Leaf>, leaf: LeafAlias, } struct Leaf { value: i32, }";
    let graph = type_graph(source).into_test_value()?;
    test_verify_eq!(graph.aliases.len(), 5);
    test_verify_eq!(graph.fields.len(), 4);

    let fields = graph
        .fields
        .iter()
        .map(|field| field.target().into_test_value())
        .collect::<TestResult<Vec<_>>>()?;
    let array = graph
        .type_use(*fields.first().into_test_value()?)
        .into_test_value()?;
    let ResolvedTypeKind::Array {
        element,
        length: Some(2),
    } = array.kind()
    else {
        return Err(TestFailure::new(
            "fixed-size array shape should be preserved",
        ));
    };
    let element = graph.type_use(element).into_test_value()?;
    test_verify!(matches!(element.kind(), ResolvedTypeKind::Option(_)));
    test_verify_eq!(
        alias_chain_len(&graph, element.alias_chain()).into_test_value()?,
        1
    );
    let ResolvedTypeKind::Option(value) = element.kind() else {
        return Err(TestFailure::new("alias should expand to an option type"));
    };
    let value = graph.type_use(value).into_test_value()?;
    test_verify_eq!(
        alias_chain_len(&graph, value.alias_chain()).into_test_value()?,
        1
    );

    let map = graph
        .type_use(*fields.get(1).into_test_value()?)
        .into_test_value()?;
    test_verify!(matches!(map.kind(), ResolvedTypeKind::Map(_)));
    let aliased_leaf = graph
        .type_use(*fields.get(2).into_test_value()?)
        .into_test_value()?;
    test_verify_eq!(
        alias_chain_len(&graph, aliased_leaf.alias_chain()).into_test_value()?,
        2
    );
    test_verify!(!aliased_leaf.span().span().is_empty());

    Ok(())
}

#[test]
fn interns_many_reverse_ordered_shapes_with_bounded_work() -> TestResult {
    let mut source = String::from("entry struct Root {");
    for length in (1..=12_000).rev() {
        source.push_str(&format!(" field_{length}: [u8; {length}],"));
    }
    source.push_str(" duplicate: [u8; 12000], }");
    let graph = type_graph(&source).into_test_value()?;
    let first = graph
        .fields
        .first()
        .copied()
        .and_then(|field| field.target())
        .into_test_value()?;
    let duplicate = graph
        .fields
        .last()
        .copied()
        .and_then(|field| field.target())
        .into_test_value()?;
    test_verify_eq!(
        graph.shape_id(ExpressionType::Resolved(first)),
        graph.shape_id(ExpressionType::Resolved(duplicate))
    );

    Ok(())
}

#[test]
fn detects_alias_cycles_with_related_declarations() -> TestResult {
    let source = "type First = Second; type Second = First; entry struct Root { value: First, }";
    let diagnostic = single_diagnostic(source).into_test_value()?;
    test_verify_eq!(diagnostic.code(), DiagnosticCode::SCHEMA_SEMANTIC_ERROR);
    test_verify_eq!(diagnostic.labels().len(), 2);
    assert_primary_text(source, &diagnostic, "First").into_test_value()?;

    Ok(())
}

#[test]
fn accepts_finite_recursive_types_through_containers_and_aliases() -> TestResult {
    let source = "type Parent = option<map<string, Node>>; entry struct Node { next: [Other], } struct Other { parent: Parent, }";
    test_verify!(type_graph(source).is_ok());
    test_verify!(type_graph("entry struct Node { children: map<string, Node> }").is_ok());
    test_verify!(type_graph("entry struct Node { next: option<Node> }").is_ok());

    Ok(())
}

#[test]
fn constructibility_work_scales_with_dependency_edges() -> TestResult {
    let dependency_count = 1_000;
    let mut source = String::new();
    for index in 0..dependency_count {
        source.push_str(&format!("struct Leaf{index} {{ value: u8 }}\n"));
    }
    source.push_str("entry struct Root {\n");
    for index in 0..dependency_count {
        source.push_str(&format!("field{index}: Leaf{index},\n"));
    }
    source.push('}');

    test_verify!(type_graph_with_step_budget(&source, 2_000_000).is_ok());

    Ok(())
}

#[test]
fn rejects_recursive_types_without_finite_values() -> TestResult {
    let source = "entry struct Loop { next: Loop, }";
    let diagnostic = single_diagnostic(source).into_test_value()?;
    test_verify_eq!(diagnostic.code(), DiagnosticCode::SCHEMA_SEMANTIC_ERROR);
    test_verify_eq!(diagnostic.message(), "recursive type has no finite value");
    test_verify!(!diagnostic.labels().is_empty());

    let fixed = single_diagnostic("entry struct Loop { next: [Loop; 1], }").into_test_value()?;
    test_verify_eq!(fixed.code(), DiagnosticCode::SCHEMA_SEMANTIC_ERROR);
    test_verify!(type_graph("entry struct Loop { next: [Loop; 0], }").is_ok());

    Ok(())
}

#[test]
fn rejects_recursive_enums_without_terminal_variants() -> TestResult {
    let diagnostic = single_diagnostic(
        "enum First { next(Second) } enum Second { previous { value: First } } entry struct Root { value: First, }",
    ).into_test_value()?;
    test_verify_eq!(diagnostic.code(), DiagnosticCode::SCHEMA_SEMANTIC_ERROR);
    test_verify!(diagnostic.labels().len() >= 2);

    test_verify!(type_graph(
        "enum Expr { Number(i64), Add { left: Expr, right: Expr } } entry struct Root { value: Expr, }"
    )
    .is_ok());

    Ok(())
}

#[test]
fn rejects_double_option_after_alias_expansion() -> TestResult {
    let source = "type MaybeInt = option<i32>; entry struct Root { value: option<MaybeInt>, }";
    let diagnostic = single_diagnostic(source).into_test_value()?;
    test_verify_eq!(diagnostic.code(), DiagnosticCode::SCHEMA_SEMANTIC_ERROR);
    assert_primary_text(source, &diagnostic, "option<MaybeInt>").into_test_value()?;

    Ok(())
}

#[test]
fn rejects_array_length_outside_u32() -> TestResult {
    let source = "entry struct Root { values: [i32; 4294967296], }";
    let diagnostic = single_diagnostic(source).into_test_value()?;
    test_verify_eq!(diagnostic.code(), DiagnosticCode::SCHEMA_SEMANTIC_ERROR);
    assert_primary_text(source, &diagnostic, "4294967296").into_test_value()?;

    Ok(())
}

#[test]
fn resolves_paths_from_current_module_toward_ancestors() -> TestResult {
    let source = "type Global = i32; mod outer { type Local = string; mod inner { struct Item { local: Local, global: Global, } } } entry struct Root { item: outer::inner::Item, }";
    test_verify!(type_graph(source).is_ok());

    Ok(())
}

#[test]
fn rejects_unknown_paths_and_module_as_final_segment() -> TestResult {
    let unknown_source = "entry struct Root { value: Missing, }";
    let unknown = single_diagnostic(unknown_source).into_test_value()?;
    test_verify_eq!(unknown.code(), DiagnosticCode::SCHEMA_SEMANTIC_ERROR);
    assert_primary_text(unknown_source, &unknown, "Missing").into_test_value()?;
    let module_source = "mod nested { struct Value {} } entry struct Root { value: nested, }";
    let module = single_diagnostic(module_source).into_test_value()?;
    test_verify_eq!(module.code(), DiagnosticCode::SCHEMA_SEMANTIC_ERROR);
    assert_primary_text(module_source, &module, "nested").into_test_value()?;

    Ok(())
}
