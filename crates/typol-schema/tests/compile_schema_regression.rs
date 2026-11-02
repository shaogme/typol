#[path = "support/schema_compile.rs"]
mod schema_support;

use std::{
    format,
    str::from_utf8,
    string::{String, ToString},
    vec::Vec,
};
use typol_test_support::*;

use typol_core::{
    DiagnosticCode, ExecutionBudget, ExecutionLimits, SliceSource, SourceId, ValueKind,
};
use typol_schema::{
    LocationFailure, LocationStatus, ResolverError, SchemaCompileErrors, SchemaCompileOptions,
    SchemaLimits, SchemaResolveRequest, SchemaResolver, SchemaSource,
    opeo::{OResult, Out},
};
use typol_semantic::{
    CompiledSchema, ConstraintKind, ConstraintLayer, FieldView, SchemaAccessError, TypeKind,
    TypeView,
};
use typol_syntax::{InputConfig, StreamLimits, TokenCursor, TokenKind};

#[derive(Debug, PartialEq, Eq)]
struct DiagnosticPosition {
    code: u32,
    start: usize,
    end: usize,
}

struct NumericSumCase {
    ty: &'static str,
    values: String,
    total: String,
}

struct InvalidSumCase {
    ty: &'static str,
    values: String,
}

struct DefaultDiagnosticCase {
    source: &'static str,
    expected_message: &'static str,
    has_constraint_label: bool,
}

fn access<T>(
    operation: impl for<'slot> FnOnce(Out<'slot, 'slot, SchemaAccessError>) -> OResult<'slot, T>,
) -> TestResult<T> {
    schema_support::SchemaCompileTestSupport::schema_access(operation).test_value()
}

fn required<T>(value: Option<T>) -> TestResult<T> {
    value.ok_or_else(|| TestFailure::new("expected member is missing from the schema"))
}

fn field<'a>(ty: TypeView<'a>, name: &str) -> TestResult<FieldView<'a>> {
    required(access(|out| ty.field_named(name, out))?)
}

struct MemorySource {
    path: String,
    identity: String,
    text: &'static str,
}

struct Request {
    from: String,
    module: String,
    source_byte_limit: usize,
}

struct MemoryResolver {
    sources: Vec<MemorySource>,
    requests: Vec<Request>,
    selected: Vec<String>,
    error: Option<(String, ResolverError)>,
}

impl MemoryResolver {
    fn new() -> Self {
        Self {
            sources: Vec::new(),
            requests: Vec::new(),
            selected: Vec::new(),
            error: None,
        }
    }

    fn add(&mut self, path: &str, identity: &str, text: &'static str) {
        self.sources.push(MemorySource {
            path: path.to_string(),
            identity: identity.to_string(),
            text,
        });
    }

    fn fail_module(&mut self, module: &str, error: ResolverError) {
        self.error = Some((module.to_string(), error));
    }

    fn candidate_paths(from_identity: &str, module: &str) -> (String, String) {
        let (directory, file_name) = from_identity
            .rsplit_once('/')
            .map_or(("", from_identity), |(directory, file_name)| {
                (directory, file_name)
            });
        let primary = if directory.is_empty() {
            format!("{module}.tschem")
        } else {
            format!("{directory}/{module}.tschem")
        };
        let stem = file_name.strip_suffix(".tschem").unwrap_or(file_name);
        let fallback = if directory.is_empty() {
            format!("{stem}/{module}.tschem")
        } else {
            format!("{directory}/{stem}/{module}.tschem")
        };
        (primary, fallback)
    }

    fn compile(
        &mut self,
        root_identity: &str,
        root_text: &str,
        limits: &SchemaLimits,
    ) -> schema_support::TestOpeoOutcome<CompiledSchema, SchemaCompileErrors> {
        schema_support::SchemaCompileTestSupport::run(
            SchemaCompileOptions::new(*limits),
            SchemaSource::from_str(root_identity.as_bytes(), root_text),
            self,
            None,
        )
    }
}

impl SchemaResolver for MemoryResolver {
    fn resolve<'src, 'slot>(
        &'src mut self,
        request: SchemaResolveRequest<'_>,
        out: Out<'slot, '_, ResolverError>,
    ) -> OResult<'slot, SchemaSource<'src>> {
        let from_identity = request.from_identity;
        let module_name = request.module_name;
        let source_byte_limit = request.source_byte_limit;
        let Ok(from_identity) = from_utf8(from_identity) else {
            return Err(out.fail(ResolverError::HostFailure));
        };
        self.requests.push(Request {
            from: from_identity.to_string(),
            module: module_name.to_string(),
            source_byte_limit,
        });
        if let Some((failed_module, error)) = &self.error {
            if failed_module == module_name {
                return Err(out.fail(*error));
            }
        }

        let (primary, fallback) = Self::candidate_paths(from_identity, module_name);
        let Some(index) = self
            .sources
            .iter()
            .position(|source| source.path == primary)
            .or_else(|| {
                self.sources
                    .iter()
                    .position(|source| source.path == fallback)
            })
        else {
            return Err(out.fail(ResolverError::NotFound));
        };
        let Some(source) = self.sources.get(index) else {
            return Err(out.fail(ResolverError::NotFound));
        };
        if source.text.len() > source_byte_limit {
            return Err(out.fail(ResolverError::ResourceLimitExceeded));
        }
        self.selected.push(source.path.clone());
        Ok(SchemaSource::from_str(
            source.identity.as_bytes(),
            source.text,
        ))
    }
}

fn diagnostic_codes(errors: &SchemaCompileErrors) -> Vec<u32> {
    errors
        .diagnostics()
        .iter()
        .map(|diagnostic| diagnostic.code().get())
        .collect()
}

fn assert_compiles(
    resolver: &mut MemoryResolver,
    identity: &str,
    source: &str,
    limits: &SchemaLimits,
) -> TestResult<CompiledSchema> {
    resolver.compile(identity, source, limits).test_value()
}

#[test]
fn all_schema_examples_compile_through_public_entry() -> TestResult {
    let limits = SchemaLimits::default();
    let examples = [
        (
            "examples/config/all_types.tschem",
            include_str!("../../../examples/config/all_types.tschem"),
        ),
        (
            "examples/config/common.tschem",
            include_str!("../../../examples/config/common.tschem"),
        ),
        (
            "examples/config/database.tschem",
            include_str!("../../../examples/config/database.tschem"),
        ),
        (
            "examples/config/server.tschem",
            include_str!("../../../examples/config/server.tschem"),
        ),
    ];

    for (path, source) in examples {
        let mut resolver = MemoryResolver::new();
        for (common_path, common_source) in examples {
            resolver.add(common_path, common_path, common_source);
        }
        let schema = assert_compiles(&mut resolver, path, source, &limits).test_value()?;
        test_verify!(Some(schema.entry()).is_some(), "{path}");
    }

    Ok(())
}

#[test]
fn stage_four_handoff_uses_only_compiled_schema_token_cursor_and_core_values() -> TestResult {
    let execution = schema_support::SchemaCompileTestSupport::budget(|out| {
        ExecutionBudget::new(ExecutionLimits::new(67_108_864, 67_108_864), out)
    })
    .test_value()?;

    let source = "entry struct Root { count: u32 = 1 }";
    let mut resolver = MemoryResolver::new();
    let schema = assert_compiles(
        &mut resolver,
        "root.tschem",
        source,
        &SchemaLimits::default(),
    )
    .test_value()?;
    let field = field(schema.entry(), "count").test_value()?;
    let default = required(access(|out| field.default_value(out))?)?;
    test_verify!(matches!(default.as_ref(), ValueKind::Unsigned(1)));

    let limits = SchemaLimits::default();
    let input_limits = limits.syntax().input_limits();
    let mut byte_input_1 = SliceSource::new(("1").as_bytes());
    let mut cursor = schema_support::SchemaCompileTestSupport::diagnostic(|out| {
        TokenCursor::new(
            InputConfig {
                source_id: SourceId::new(0),
                source: &mut byte_input_1,
                resources: &input_limits,
                stream: StreamLimits::default(),
                budget: &execution,
            },
            out,
        )
    })
    .test_value()?;
    test_verify_eq!(
        schema_support::SchemaCompileTestSupport::diagnostic(|out| cursor.peek(out))
            .test_value()?
            .token()
            .test_value()?
            .kind(),
        TokenKind::Integer
    );
    schema_support::SchemaCompileTestSupport::diagnostic(|out| cursor.bump(out)).test_value()?;
    test_verify_eq!(
        schema_support::SchemaCompileTestSupport::diagnostic(|out| cursor.peek(out))
            .test_value()?
            .token()
            .test_value()?
            .kind(),
        TokenKind::Eof
    );

    Ok(())
}

#[test]
fn imported_entry_is_a_regular_type_and_span_locations_are_owned() -> TestResult {
    let root = "extern mod common;\nentry struct Root { item: common::Item }";
    let child = "entry struct Item { value: u32 }";
    let mut resolver = MemoryResolver::new();
    resolver.add("pkg/common.tschem", "pkg/common.tschem", child);

    let schema = assert_compiles(
        &mut resolver,
        "pkg/root.tschem",
        root,
        &SchemaLimits::default(),
    )
    .test_value()?;
    let root_info = schema.entry();
    let fields = access(|out| root_info.fields(out))?;
    test_verify_eq!(fields.len(), 1);
    let field = required(fields.get(0)).test_value()?;
    let item_type = access(|out| field.ty(out))?;
    test_verify_eq!(access(|out| item_type.name(out))?, Some("Item"));
    test_verify_eq!(
        access(|out| item_type.span(out))?.source_id(),
        SourceId::new(0)
    );
    test_verify_eq!(
        required(access(|out| item_type.declaration_span(out))?)?.source_id(),
        SourceId::new(1)
    );
    test_verify!(
        schema
            .span_location(access(|out| item_type.span(out))?)
            .is_some()
    );

    Ok(())
}

#[test]
fn recursive_imports_resolve_from_each_source_identity() -> TestResult {
    let root = "extern mod outer;\nentry struct Root { leaf: outer::inner::Leaf }";
    let outer = "extern mod inner;\nstruct Container {}";
    let inner = "entry struct Leaf { value: u32 }";
    let mut resolver = MemoryResolver::new();
    resolver.add("pkg/outer.tschem", "pkg/outer.tschem", outer);
    resolver.add("pkg/outer/inner.tschem", "pkg/outer/inner.tschem", inner);

    let schema = assert_compiles(
        &mut resolver,
        "pkg/root.tschem",
        root,
        &SchemaLimits::default(),
    )
    .test_value()?;
    let root_type = schema.entry();
    let leaf_field = required(access(|out| root_type.fields(out))?.get(0))?;
    let leaf_type = access(|out| leaf_field.ty(out))?;
    test_verify_eq!(access(|out| leaf_type.name(out))?, Some("Leaf"));
    test_verify_eq!(
        access(|out| leaf_type.span(out))?.source_id(),
        SourceId::new(0)
    );
    test_verify_eq!(
        required(access(|out| leaf_type.declaration_span(out))?)?.source_id(),
        SourceId::new(2)
    );
    test_verify_eq!(
        resolver.selected,
        ["pkg/outer.tschem", "pkg/outer/inner.tschem"]
    );

    Ok(())
}

#[test]
fn resolver_prefers_same_directory_then_uses_stem_directory_fallback() -> TestResult {
    let root = "extern mod common;\nentry struct Root { value: common::Selected }";
    let mut resolver = MemoryResolver::new();
    resolver.add(
        "pkg/common.tschem",
        "pkg/common.tschem",
        "struct Selected {}",
    );
    resolver.add(
        "pkg/root/common.tschem",
        "pkg/root/common.tschem",
        "struct Selected {}",
    );
    assert_compiles(
        &mut resolver,
        "pkg/root.tschem",
        root,
        &SchemaLimits::default(),
    )
    .test_value()?;
    test_verify_eq!(resolver.selected, ["pkg/common.tschem"]);

    let mut fallback_resolver = MemoryResolver::new();
    fallback_resolver.add(
        "pkg/root/common.tschem",
        "pkg/root/common.tschem",
        "struct Selected {}",
    );
    assert_compiles(
        &mut fallback_resolver,
        "pkg/root.tschem",
        root,
        &SchemaLimits::default(),
    )
    .test_value()?;
    test_verify_eq!(fallback_resolver.selected, ["pkg/root/common.tschem"]);

    Ok(())
}

#[test]
fn invalid_preferred_import_does_not_fall_back() -> TestResult {
    let root = "extern mod common;\nentry struct Root {}";
    let mut resolver = MemoryResolver::new();
    resolver.add(
        "pkg/common.tschem",
        "pkg/common.tschem",
        "entry struct Broken {",
    );
    resolver.add(
        "pkg/root/common.tschem",
        "pkg/root/common.tschem",
        "struct Backup {}",
    );

    let errors = resolver
        .compile("pkg/root.tschem", root, &SchemaLimits::default())
        .test_error()?;
    test_verify!(diagnostic_codes(&errors).contains(&DiagnosticCode::SCHEMA_SYNTAX_ERROR.get()));
    test_verify_eq!(resolver.selected, ["pkg/common.tschem"]);

    Ok(())
}

#[test]
fn resolver_errors_have_stable_diagnostic_codes_and_hide_host_errors() -> TestResult {
    let root = "extern mod missing;\nentry struct Root {}";
    for (failure, expected_code) in [
        (None, DiagnosticCode::SCHEMA_SEMANTIC_ERROR.get()),
        (
            Some(ResolverError::ResourceLimitExceeded),
            DiagnosticCode::RESOURCE_LIMIT_EXCEEDED.get(),
        ),
        (
            Some(ResolverError::AllocationFailed),
            DiagnosticCode::ALLOCATION_FAILED.get(),
        ),
        (
            Some(ResolverError::HostFailure),
            DiagnosticCode::SCHEMA_RESOLVER_ERROR.get(),
        ),
    ] {
        let mut resolver = MemoryResolver::new();
        if let Some(error) = failure {
            resolver.fail_module("missing", error);
        }
        let errors = resolver
            .compile("root.tschem", root, &SchemaLimits::default())
            .test_error()?;
        test_verify_eq!(diagnostic_codes(&errors), [expected_code]);
        test_verify!(!format!("{errors:?}").contains("HostFailure"));
    }

    Ok(())
}

#[test]
fn repeated_source_identity_and_import_cycles_are_rejected() -> TestResult {
    let root = "extern mod first;\nextern mod second;\nentry struct Root {}";
    let mut duplicate_resolver = MemoryResolver::new();
    duplicate_resolver.add("first.tschem", "shared-identity", "struct First {}");
    duplicate_resolver.add("second.tschem", "shared-identity", "struct Second {}");
    let duplicate_errors = duplicate_resolver
        .compile("root.tschem", root, &SchemaLimits::default())
        .test_error()?;
    test_verify_eq!(
        diagnostic_codes(&duplicate_errors),
        [DiagnosticCode::SCHEMA_SEMANTIC_ERROR.get()]
    );
    test_verify_eq!(duplicate_resolver.requests.len(), 2);
    test_verify_eq!(
        duplicate_resolver.requests.first().test_value()?.from,
        "root.tschem"
    );
    test_verify_eq!(
        duplicate_resolver.requests.first().test_value()?.module,
        "first"
    );

    let cycle_root = "extern mod child;\nentry struct Root {}";
    let child = "extern mod root;\nstruct Child {}";
    let mut cycle_resolver = MemoryResolver::new();
    cycle_resolver.add("child.tschem", "child.tschem", child);
    cycle_resolver.add("root.tschem", "root.tschem", cycle_root);
    let cycle_errors = cycle_resolver
        .compile("root.tschem", cycle_root, &SchemaLimits::default())
        .test_error()?;
    test_verify_eq!(
        diagnostic_codes(&cycle_errors),
        [DiagnosticCode::SCHEMA_SEMANTIC_ERROR.get()]
    );

    Ok(())
}

#[test]
fn source_file_and_total_byte_limits_are_checked_at_boundaries() -> TestResult {
    let root = "extern mod child;\nentry struct Root {}";
    let child = "struct Child {}";
    let total = root.len() + child.len();

    let mut exact = SchemaLimits::default();
    exact.set_max_source_bytes(root.len());
    exact.set_max_schema_total_bytes(total);
    exact.set_max_schema_files(2);
    let mut exact_resolver = MemoryResolver::new();
    exact_resolver.add("child.tschem", "child.tschem", child);
    assert_compiles(&mut exact_resolver, "root.tschem", root, &exact).test_value()?;
    test_verify_eq!(
        exact_resolver.requests.first().test_value()?.from,
        "root.tschem"
    );
    test_verify_eq!(
        exact_resolver.requests.first().test_value()?.module,
        "child"
    );
    test_verify_eq!(
        exact_resolver
            .requests
            .first()
            .test_value()?
            .source_byte_limit,
        child.len()
    );

    let mut too_small_root = SchemaLimits::default();
    too_small_root.set_max_source_bytes(root.len() - 1);
    let mut root_resolver = MemoryResolver::new();
    let errors = root_resolver
        .compile("root.tschem", root, &too_small_root)
        .test_error()?;
    test_verify_eq!(
        diagnostic_codes(&errors),
        [DiagnosticCode::RESOURCE_LIMIT_EXCEEDED.get()]
    );
    test_verify!(root_resolver.requests.is_empty());

    let mut too_small_total = exact;
    too_small_total.set_max_schema_total_bytes(total - 1);
    let mut total_resolver = MemoryResolver::new();
    total_resolver.add("child.tschem", "child.tschem", child);
    let errors = total_resolver
        .compile("root.tschem", root, &too_small_total)
        .test_error()?;
    test_verify_eq!(
        diagnostic_codes(&errors),
        [DiagnosticCode::RESOURCE_LIMIT_EXCEEDED.get()]
    );
    test_verify_eq!(
        total_resolver
            .requests
            .first()
            .test_value()?
            .source_byte_limit,
        child.len() - 1
    );

    let mut one_file_only = SchemaLimits::default();
    one_file_only.set_max_schema_files(1);
    let mut file_resolver = MemoryResolver::new();
    file_resolver.add("child.tschem", "child.tschem", child);
    let errors = file_resolver
        .compile("root.tschem", root, &one_file_only)
        .test_error()?;
    test_verify_eq!(
        diagnostic_codes(&errors),
        [DiagnosticCode::RESOURCE_LIMIT_EXCEEDED.get()]
    );

    Ok(())
}

#[test]
fn diagnostic_limit_reserves_the_last_slot_for_one_summary() -> TestResult {
    let source = "entry struct Root { first: UnknownA, second: UnknownB }";
    let mut limits = SchemaLimits::default();
    limits.set_max_diagnostics(1);
    let mut resolver = MemoryResolver::new();

    let errors = resolver
        .compile("root.tschem", source, &limits)
        .test_error()?;
    test_verify_eq!(
        diagnostic_codes(&errors),
        [DiagnosticCode::DIAGNOSTIC_LIMIT_REACHED.get()]
    );
    let diagnostic = &errors.diagnostics().first().test_value()?;
    let span = diagnostic.primary().test_value()?;
    test_verify!(errors.span_location(span).is_some());

    Ok(())
}

#[test]
fn regex_resources_are_aggregated_across_imported_sources() -> TestResult {
    let root = "extern mod child;\nentry struct Root { value: string @regex(\"a\") }";
    let child = "struct Child { value: string @regex(\"b\") }";
    let mut resolver = MemoryResolver::new();
    resolver.add("child.tschem", "child.tschem", child);
    assert_compiles(&mut resolver, "root.tschem", root, &SchemaLimits::default()).test_value()?;

    let mut limits = SchemaLimits::default();
    limits.set_max_regex_count(1);
    let mut limited_resolver = MemoryResolver::new();
    limited_resolver.add("child.tschem", "child.tschem", child);
    let errors = limited_resolver
        .compile("root.tschem", root, &limits)
        .test_error()?;
    test_verify_eq!(
        diagnostic_codes(&errors),
        [DiagnosticCode::RESOURCE_LIMIT_EXCEEDED.get()]
    );

    Ok(())
}

#[test]
fn checked_array_length_boundary_is_platform_independent() -> TestResult {
    let maximum = "entry struct Root { values: [u8; 4294967295] }";
    let overflow = "entry struct Root { values: [u8; 4294967296] }";
    let mut resolver = MemoryResolver::new();
    let schema = assert_compiles(
        &mut resolver,
        "root.tschem",
        maximum,
        &SchemaLimits::default(),
    )
    .test_value()?;
    let root = schema.entry();
    let field = required(access(|out| root.fields(out))?.get(0))?;
    let array = access(|out| field.ty(out))?;
    let array_kind = access(|out| array.kind(out))?;
    test_verify!(matches!(
        array_kind,
        TypeKind::Array {
            length: Some(u32::MAX),
            ..
        }
    ));

    let mut overflow_resolver = MemoryResolver::new();
    let errors = overflow_resolver
        .compile("root.tschem", overflow, &SchemaLimits::default())
        .test_error()?;
    test_verify_eq!(
        diagnostic_codes(&errors),
        [DiagnosticCode::SCHEMA_SEMANTIC_ERROR.get()]
    );

    Ok(())
}

#[test]
fn defaults_constraints_and_expressions_are_published_without_secret_debug_text() -> TestResult {
    let source = r#"
type Label = string @length(1, 20);
type SecretText = string @secret;
struct Child { id: u32, label: Label = "ready" }
enum Choice { Empty, Text(string), Named { label: Label } }
entry struct Root {
    child: Child = { id: 3 },
    choice: Choice = .Text("ready"),
    named_choice: Choice = .Named { label: "named" },
    optional: option<string> = "lifted",
    mapping: map<string, u32> @all(key != "" && value > 0, "map entries must be valid") = {"one": 1},
    values: [u32] @size(min = 1) @all(item > 0, "members must be positive") @sum(max = 10) = [1, 2],
    token: string @secret @env("ROOT_TOKEN") = "hidden-secret",
    alias_token: SecretText,
    name: string @regex("^safe$") = "safe",
}
@assert(self.child.id > 0, "private assertion text")
@assert(self.values.all(|element| element > 0), "values must be positive")
"#;
    let mut resolver = MemoryResolver::new();
    let schema = assert_compiles(
        &mut resolver,
        "root.tschem",
        source,
        &SchemaLimits::default(),
    )
    .test_value()?;
    let root = schema.entry();
    let child = field(schema.entry(), "child").test_value()?;
    test_verify!(access(|out| child.default_value(out))?.is_some());
    test_verify_eq!(access(|out| root.assertions(out))?.len(), 2);
    let child_type = access(|out| child.ty(out))?;
    let label = field(child_type, "label").test_value()?;
    let label_constraints = access(|out| label.constraints(out))?;
    let label_layers =
        schema_support::SchemaCompileTestSupport::collect_constraints(label_constraints)?
            .into_iter()
            .map(|value| (value.kind(), value.layer()))
            .collect::<Vec<_>>();
    test_verify!(label_layers.iter().any(|(kind, layer)| {
        *kind == ConstraintKind::Length && *layer == ConstraintLayer::Alias
    }));
    test_verify_eq!(access(|out| child_type.fields(out))?.len(), 2);
    for name in ["choice", "named_choice", "optional", "mapping"] {
        let field = field(schema.entry(), name).test_value()?;
        test_verify!(
            access(|out| field.default_value(out))?.is_some(),
            "field {name} should hold a compiled default value"
        );
    }
    let token = field(schema.entry(), "token").test_value()?;
    test_verify!(access(|out| token.is_secret(out))?);
    test_verify_eq!(
        access(|out| token.environment(out))?
            .ok_or_else(|| TestFailure::new("environment access is not authorized"))?
            .name(),
        "ROOT_TOKEN"
    );
    let alias_token = field(schema.entry(), "alias_token").test_value()?;
    test_verify!(access(|out| alias_token.is_secret(out))?);
    let debug = format!("{schema:?}");
    test_verify!(!debug.contains("hidden-secret"));
    test_verify!(!debug.contains("ROOT_TOKEN"));
    test_verify!(!debug.contains("private assertion text"));
    test_verify!(!debug.contains("^safe$"));

    Ok(())
}

#[test]
fn invalid_default_and_type_graph_inputs_return_semantic_diagnostics() -> TestResult {
    for source in [
        "entry struct Root { value: Missing }",
        "type First = Second; type Second = First; entry struct Root {}",
        "struct Node { next: option<Node> } entry struct Root { node: Node }",
        "entry struct Root { value: u8 = 256 }",
        "entry struct Root { value: option<option<u8>> }",
    ] {
        let mut resolver = MemoryResolver::new();
        let errors = resolver
            .compile("root.tschem", source, &SchemaLimits::default())
            .test_error()?;
        test_verify!(
            diagnostic_codes(&errors).contains(&DiagnosticCode::SCHEMA_SEMANTIC_ERROR.get()),
            "input produced no static semantic diagnostics"
        );
        for diagnostic in errors.diagnostics() {
            if let Some(span) = diagnostic.primary() {
                test_verify!(errors.span_location(span).is_some());
            }
        }
    }

    Ok(())
}

#[test]
fn namespaces_annotations_defaults_and_assertions_reject_independent_errors() -> TestResult {
    let cases = [
        "struct Item {}",
        "entry struct First {} entry struct Second {}",
        "mod shared {} extern mod shared; entry struct Root {}",
        "type Name = string; struct Name {} entry struct Root {}",
        "entry struct Root { value: u32, value: string }",
        "enum Mode { Value, Value } entry struct Root {}",
        "entry struct Root { value: string @length(1, 2) @length(2, 3) }",
        "type Small = u8 @max(10); entry struct Root { value: Small @min(20) }",
        "entry struct Root { value: string @min(1) }",
        "entry struct Root { values: [u8; 2] @size(min = 3) }",
        "entry struct Root { values: map<string, u8> = {\"same\": 1, \"same\": 2} }",
        "struct Child { required: u8 } entry struct Root { child: Child = {} }",
        "entry struct Root {} @assert(1, \"assertion must be bool\")",
        "entry struct Root { value: u32 } @assert(self.missing > 0, \"unknown field\")",
        "entry struct Root { values: [u8] @all(self.value > 0, \"invalid binding\") }",
    ];
    for source in cases {
        let mut resolver = MemoryResolver::new();
        let schema_support::TestOpeoOutcome::Error(errors) =
            resolver.compile("root.tschem", source, &SchemaLimits::default())
        else {
            return Err(format!("static negative case unexpectedly passed: {source}").into());
        };
        test_verify!(
            diagnostic_codes(&errors).contains(&DiagnosticCode::SCHEMA_SEMANTIC_ERROR.get()),
            "negative case produced no static semantic diagnostics"
        );
    }

    Ok(())
}

#[test]
fn semantic_diagnostics_keep_source_order_across_repeated_compiles() -> TestResult {
    let source = "entry struct Root { first: MissingFirst, second: MissingSecond }";
    let mut runs = Vec::new();
    for _ in 0..2 {
        let mut resolver = MemoryResolver::new();
        let errors = resolver
            .compile("root.tschem", source, &SchemaLimits::default())
            .test_error()?;
        let mut spans = Vec::new();
        for diagnostic in errors.diagnostics() {
            let span = diagnostic.primary().test_value()?.span();
            spans.push(DiagnosticPosition {
                code: diagnostic.code().get(),
                start: span.start(),
                end: span.end(),
            });
            test_verify!(
                errors
                    .span_location(diagnostic.primary().test_value()?)
                    .is_some()
            );
        }
        runs.push(spans);
    }
    test_verify_eq!(runs.first().test_value()?, runs.get(1).test_value()?);
    let first_run = runs.first().test_value()?;
    test_verify_eq!(first_run.len(), 2);
    let first = first_run.first().test_value()?;
    let second = first_run.get(1).test_value()?;
    test_verify!(first.start < second.start);
    test_verify_eq!(
        first_run.first().test_value()?.code,
        DiagnosticCode::SCHEMA_SEMANTIC_ERROR.get()
    );
    test_verify_eq!(
        first_run.get(1).test_value()?.code,
        DiagnosticCode::SCHEMA_SEMANTIC_ERROR.get()
    );

    Ok(())
}

#[test]
fn nested_schema_depth_is_limited() -> TestResult {
    let source = "mod outer { mod inner { struct Item {} } } entry struct Root {}";
    let mut limits = SchemaLimits::default();
    limits.set_max_nesting_depth(1);
    let mut resolver = MemoryResolver::new();
    let errors = resolver
        .compile("root.tschem", source, &limits)
        .test_error()?;
    test_verify_eq!(
        diagnostic_codes(&errors),
        [DiagnosticCode::RESOURCE_LIMIT_EXCEEDED.get()]
    );

    Ok(())
}

#[test]
fn default_value_depth_limit_is_inclusive_and_shared_with_freeze() -> TestResult {
    let mut limits = SchemaLimits::default();
    limits.set_max_value_nesting_depth(2);

    let at_limit = r#"entry struct Root { value: [[u8]] = [[1]], }"#;
    MemoryResolver::new()
        .compile("root.tschem", at_limit, &limits)
        .test_value()?;

    let over_limit = r#"entry struct Root { value: [[[u8]]] = [[[1]]], }"#;
    let errors = MemoryResolver::new()
        .compile("root.tschem", over_limit, &limits)
        .test_error()?;
    test_verify_eq!(
        diagnostic_codes(&errors),
        [DiagnosticCode::RESOURCE_LIMIT_EXCEEDED.get()]
    );

    Ok(())
}

#[test]
fn host_defaults_reject_invalid_numeric_ipv4_without_dns_fallback() -> TestResult {
    for text in ["01.2.3.4", "256.2.3.4", "01.2.3.4.", "256.2.3.4."] {
        let source = format!("entry struct Root {{ value: string @host = \"{text}\", }}");
        let errors = MemoryResolver::new()
            .compile("root.tschem", &source, &SchemaLimits::default())
            .test_error()?;
        test_verify!(
            errors
                .diagnostics()
                .iter()
                .any(|error| error.code() == DiagnosticCode::SCHEMA_SEMANTIC_ERROR)
        );
    }
    for text in [
        "192.0.2.9",
        "192.0.2.9.",
        "1a.2.3.4",
        "Example.COM.",
        "2001:db8::1",
    ] {
        let source = format!("entry struct Root {{ value: string @host = \"{text}\", }}");
        MemoryResolver::new()
            .compile("root.tschem", &source, &SchemaLimits::default())
            .test_value()?;
    }

    Ok(())
}

#[test]
fn default_expression_sum_uses_wide_intermediates_and_final_target_range() -> TestResult {
    let limits = SchemaLimits::default();
    for case in [
        NumericSumCase {
            ty: "i8",
            values: "127, 1, -1".to_string(),
            total: "127".to_string(),
        },
        NumericSumCase {
            ty: "i128",
            values: format!("{0}, {0}, -{0}", i128::MAX),
            total: i128::MAX.to_string(),
        },
        NumericSumCase {
            ty: "i128",
            values: format!("{0}, {0}, {1}, 1", i128::MIN, i128::MAX),
            total: i128::MIN.to_string(),
        },
        NumericSumCase {
            ty: "u128",
            values: String::new(),
            total: "0".to_string(),
        },
        NumericSumCase {
            ty: "f32",
            values: "16777216.0, 1.0, -16777216.0".to_string(),
            total: "0.0".to_string(),
        },
    ] {
        let NumericSumCase { ty, values, total } = case;
        let text = format!(
            "entry struct Root {{ values: [[{ty}]] @all(item.sum() == {total}, \"rule\") = [[{values}]], }}"
        );
        MemoryResolver::new()
            .compile("root", &text, &limits)
            .test_value()?;
    }
    for case in [
        InvalidSumCase {
            ty: "i8",
            values: "127, 1".to_string(),
        },
        InvalidSumCase {
            ty: "u128",
            values: format!("{}, 1", u128::MAX),
        },
        InvalidSumCase {
            ty: "f32",
            values: "340282346638528860000000000000000000000.0, 340282346638528860000000000000000000000.0, -340282346638528860000000000000000000000.0".to_string(),
        },
    ] {
        let InvalidSumCase { ty, values } = case;
        let text = format!("entry struct Root {{ values: [[{ty}]] @all(item.sum() == 0, \"rule\") = [[{values}]], }}");
        let errors = MemoryResolver::new().compile("root", &text, &limits).test_error()?;
        test_verify!(errors.diagnostics().iter().any(|error| error.code() == DiagnosticCode::SCHEMA_SEMANTIC_ERROR));
    }

    Ok(())
}

#[test]
fn float32_defaults_and_contextual_numbers_round_directly() -> TestResult {
    let boundary = "1.000000059604644775390626";
    let next_float = "1.0000001192092896";
    let maximum = "340282346638528859811704183484516925440.0";
    let source = format!(
        "entry struct Root {{ value: f32 @min({boundary}) = {boundary}, values: [f32] @all(item == {boundary}, \"floating-point boundary equality\") = [{next_float}], contextual_integer: [f32] @all(item == 16777217, \"integer boundary rounded as f32\") = [16777216.0], integer_bound: f32 @min(16777217) = 16777216.0, negative_zero: f32 = -0.0, maximum: f32 = {maximum}, }}"
    );
    let mut resolver = MemoryResolver::new();
    let schema = resolver
        .compile("root", &source, &SchemaLimits::default())
        .test_value()?;
    for (name, expected) in [
        ("value", f32::from_bits(0x3f80_0001)),
        ("negative_zero", -0.0_f32),
        ("maximum", f32::MAX),
    ] {
        let field = field(schema.entry(), name).test_value()?;
        let default = required(access(|out| field.default_value(out))?)?;
        test_verify!(
            matches!(default.as_ref(), ValueKind::Float32(value) if value.to_bits() == expected.to_bits())
        );
    }

    for builtin in ["f32", "f64"] {
        let invalid = format!("entry struct Root {{ value: {builtin} = 1, }}");
        let errors = resolver
            .compile("root", &invalid, &SchemaLimits::default())
            .test_error()?;
        test_verify!(
            errors
                .diagnostics()
                .iter()
                .any(|error| { error.code() == DiagnosticCode::SCHEMA_SEMANTIC_ERROR })
        );
    }

    let overflow = "entry struct Root { value: f32 = 340282400000000000000000000000000000000.0, }";
    let errors = resolver
        .compile("root", overflow, &SchemaLimits::default())
        .test_error()?;
    test_verify!(
        errors
            .diagnostics()
            .iter()
            .any(|error| { error.code() == DiagnosticCode::SCHEMA_SEMANTIC_ERROR })
    );

    Ok(())
}

#[test]
fn default_expressions_match_runtime_short_circuit_numeric_and_host_cases() -> TestResult {
    let limits = SchemaLimits::default();
    for text in [
        r#"entry struct Root { v: [u8] @all(false && item / 0 > 0 || true, "rule") = [1] }"#,
        r#"entry struct Root { v: [[u8]] @all(item.any(|v| v == 0 || 1 / v > 0), "rule") = [[0, 0]] }"#,
        r#"entry struct Root { v: [[u8]] @all(!item.all(|v| v > 0 && 1 / v > 0), "rule") = [[0, 0]] }"#,
        r#"entry struct Root { v: [[u8]] @all(!item.any(|v| 1 / v > 0) && item.all(|v| 1 / v > 0), "rule") = [[]] }"#,
        r#"entry struct Root { v: [[[u8]]] @all(item.all(|v| v.any(|v| v > 0) && v.len() == 2), "rule") = [[[0, 1], [2, 0]]] }"#,
        r#"entry struct Root { v: [i128] @all(item == -170141183460469231731687303715884105728, "rule") = [-170141183460469231731687303715884105728] }"#,
        r#"entry struct Root { v: [i8] @all(item / 2 == -1, "rule") = [-3] }"#,
        r#"entry struct Root { v: [f32] @all((item + 1.0) - item == 0.0, "rule") = [16777216.0] }"#,
        r#"struct Pair { a: string @host, b: string @host } entry struct Root { v: [Pair] @all(item.a != item.b, "rule") = [{a: "Example.COM.", b: "example.com"}] }"#,
        r#"struct Pair { a: string @host, b: string } entry struct Root { v: [Pair] @all(item.a != item.b, "rule") = [{a: "Example.COM.", b: "example.com"}] }"#,
        r#"struct Pair { a: string @host, b: string @host } entry struct Root { v: [Pair] @all(item.a != item.b, "rule") = [{a: "::ffff:192.0.2.1", b: "0:0:0:0:0:ffff:c000:201"}] }"#,
    ] {
        MemoryResolver::new()
            .compile("root", text, &limits)
            .test_value()?;
    }

    Ok(())
}

#[test]
fn default_all_and_sum_report_arithmetic_errors_separately_from_constraint_failures() -> TestResult
{
    let cases = [
        DefaultDiagnosticCase {
            source: r#"entry struct Root { values: [u8] @all(item / 0 > 0, "rule") = [1, 2] }"#,
            expected_message: "schema default numeric evaluation failed",
            has_constraint_label: false,
        },
        DefaultDiagnosticCase {
            source: r#"entry struct Root { values: [u8] @sum(min = 0) = [255, 1] }"#,
            expected_message: "schema default numeric evaluation failed",
            has_constraint_label: false,
        },
        DefaultDiagnosticCase {
            source: r#"entry struct Root { values: [u8] @all(item > 0, "rule") = [0] }"#,
            expected_message: "schema default does not satisfy the @all constraint",
            has_constraint_label: true,
        },
        DefaultDiagnosticCase {
            source: r#"entry struct Root { values: [u8] @sum(min = 2) = [1] }"#,
            expected_message: "schema default does not satisfy the @sum constraint",
            has_constraint_label: true,
        },
    ];

    for case in cases {
        let errors = MemoryResolver::new()
            .compile("root.tschem", case.source, &SchemaLimits::default())
            .test_error()?;
        test_verify_eq!(errors.diagnostics().len(), 1, "{}", case.source);
        let diagnostic = errors.diagnostics().first().test_value()?;
        test_verify_eq!(diagnostic.code(), DiagnosticCode::SCHEMA_SEMANTIC_ERROR);
        test_verify_eq!(
            diagnostic.message(),
            case.expected_message,
            "{}",
            case.source
        );
        test_verify!(diagnostic.primary().is_some(), "{}", case.source);
        test_verify_eq!(
            diagnostic.labels().len(),
            usize::from(case.has_constraint_label)
        );
    }

    Ok(())
}

#[test]
fn large_default_array_and_map_all_constraints_fit_default_execution_budget() -> TestResult {
    let count = 8500;
    let values = (0..count).map(|_| "1").collect::<Vec<_>>().join(",");
    let entries = (0..count)
        .map(|index| format!("\"item{index:05}\": 1"))
        .collect::<Vec<_>>()
        .join(",");
    let source = format!(
        "entry struct Root {{ values: [u32] @all(item > 0, \"positive\") = [{values}], mapping: map<string, u32> @all(key.starts_with(\"item\") && value > 0, \"positive\") = {{{entries}}} }}"
    );
    let limits = SchemaLimits::default();
    let mut resolver = MemoryResolver::new();
    assert_compiles(&mut resolver, "root.tschem", &source, &limits).test_value()?;

    Ok(())
}

#[test]
fn unique_default_duplicates_report_in_original_element_order() -> TestResult {
    let text = "entry struct Root { a: [i8] @unique = [2, 1, 2, 1, 2], }";
    let errors = MemoryResolver::new()
        .compile("root", text, &SchemaLimits::default())
        .test_error()?;
    let starts: Vec<_> = errors
        .diagnostics()
        .iter()
        .map(|error| error.primary().map(|span| span.span().start()))
        .collect::<Option<Vec<_>>>()
        .test_value()?;
    let array_start = text.find("[2, 1, 2, 1, 2]").test_value()?;
    test_verify_eq!(
        starts,
        [array_start + 7, array_start + 10, array_start + 13]
    );

    Ok(())
}

#[test]
fn duplicate_annotation_bounds_report_both_parameter_locations() -> TestResult {
    for (annotation, parameter) in [
        ("@size(min = 1, min = 2)", "min"),
        ("@size(max = 1, max = 2)", "max"),
        ("@sum(min = 1, min = 2)", "min"),
        ("@sum(max = 1, max = 2)", "max"),
    ] {
        let source = format!("entry struct Root {{ values: [u8] {annotation} }}");
        let errors = MemoryResolver::new()
            .compile("root.tschem", &source, &SchemaLimits::default())
            .test_error()?;
        let diagnostic = &errors.diagnostics().first().test_value()?;
        test_verify_eq!(diagnostic.code(), DiagnosticCode::SCHEMA_SEMANTIC_ERROR);
        let earlier = source.find(parameter).test_value()?;
        let later = source.rfind(parameter).test_value()?;
        test_verify_eq!(diagnostic.primary().test_value()?.span().start(), later);
        test_verify_eq!(diagnostic.labels().len(), 1);
        test_verify_eq!(
            diagnostic
                .labels()
                .first()
                .test_value()?
                .span()
                .span()
                .start(),
            earlier
        );
        test_verify!(
            errors
                .span_location(diagnostic.labels().first().test_value()?.span())
                .is_some()
        );
    }

    Ok(())
}

#[test]
fn schema_node_budget_is_per_file_and_independent_of_configuration_nodes() -> TestResult {
    let mut limits = SchemaLimits::default();
    limits.set_max_schema_ast_nodes(11);
    limits.set_max_value_nodes(0);
    let source = "extern mod child; entry struct Root { child: child::Leaf }";
    let mut resolver = MemoryResolver::new();
    resolver.add(
        "child.tschem",
        "child.tschem",
        "struct Leaf { x: u8, y: u8, z: u8 }",
    );
    test_verify!(resolver.compile("root.tschem", source, &limits).is_ok());
    limits.set_max_schema_ast_nodes(10);
    let mut resolver = MemoryResolver::new();
    resolver.add(
        "child.tschem",
        "child.tschem",
        "struct Leaf { x: u8, y: u8, z: u8 }",
    );
    let errors = resolver
        .compile("root.tschem", source, &limits)
        .test_error()?;
    test_verify_eq!(
        errors.diagnostics().last().test_value()?.code(),
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
    );

    Ok(())
}

#[test]
fn all_default_failures_redact_messages_values_and_debug_in_every_secret_context() -> TestResult {
    for source in [
        r#"entry struct Root { values: [u8] @secret @all(item > 0, "PRIVATE-MESSAGE") = [0] }"#,
        r#"entry struct Root { values: map<string, u8> @secret @all(value > 0, "PRIVATE-MESSAGE") = {"PRIVATE-VALUE": 0} }"#,
        r#"type Values = [u8] @secret @all(item > 0, "PRIVATE-MESSAGE"); entry struct Root { values: Values = [0] }"#,
        r#"type Values = map<string, u8> @secret @all(value > 0, "PRIVATE-MESSAGE"); entry struct Root { values: Values = {"PRIVATE-VALUE": 0} }"#,
        r#"struct Inner { values: [u8] @all(item > 0, "PRIVATE-MESSAGE") = [0] } entry struct Root { inner: Inner @secret = {} }"#,
        r#"struct Inner { values: map<string, u8> @all(value > 0, "PRIVATE-MESSAGE") = {"PRIVATE-VALUE": 0} } entry struct Root { inner: Inner @secret = {} }"#,
        r#"struct Inner { token: string @secret } entry struct Root { values: [Inner] @all(item.token != "PRIVATE-VALUE", "PRIVATE-MESSAGE") = [{token: "PRIVATE-VALUE"}] }"#,
        r#"struct Inner { token: string @secret } entry struct Root { values: map<string, Inner> @all(value.token != "PRIVATE-VALUE", "PRIVATE-MESSAGE") = {"key": {token: "PRIVATE-VALUE"}} }"#,
        r#"entry struct Root { values: [string] @all(item != "PRIVATE-VALUE", "PRIVATE-MESSAGE") = ["PRIVATE-VALUE"] }"#,
        r#"entry struct Root { values: map<string, string> @all(value != "PRIVATE-VALUE", "PRIVATE-MESSAGE") = {"key": "PRIVATE-VALUE"} }"#,
    ] {
        let errors = MemoryResolver::new()
            .compile("root.tschem", source, &SchemaLimits::default())
            .test_error()?;
        test_verify_eq!(diagnostic_codes(&errors), [7], "{source}");
        test_verify_eq!(errors.location_status(), LocationStatus::Complete);
        for diagnostic in errors.diagnostics() {
            test_verify_eq!(
                diagnostic.message(),
                "schema default does not satisfy the @all constraint"
            );
            let primary = diagnostic.primary().test_value()?;
            test_verify!(errors.span_location(primary).is_some());
            test_verify_eq!(diagnostic.labels().len(), 1);
            let label = &diagnostic.labels().first().test_value()?;
            test_verify_eq!(label.message(), "constraint is defined here");
            let span = label.span().span();
            test_verify!(source[span.start()..span.end()].starts_with("@all("));
            test_verify!(errors.span_location(label.span()).is_some());
            let debug = format!("{diagnostic:?} {label:?} {errors:?}");
            for secret in ["PRIVATE-MESSAGE", "PRIVATE-VALUE"] {
                test_verify!(!diagnostic.message().contains(secret));
                test_verify!(!label.message().contains(secret));
                test_verify!(!debug.contains(secret));
            }
        }
    }

    Ok(())
}

#[test]
fn diagnostic_terminal_stays_last_with_its_actual_cross_pass_trigger_span() -> TestResult {
    let source = "entry struct Root { early: string @min(1), later: Missing }";
    for budget in [0, 1, 2, 3] {
        let mut limits = SchemaLimits::default();
        limits.set_max_diagnostics(budget);
        let errors = MemoryResolver::new()
            .compile("root.tschem", source, &limits)
            .test_error()?;
        let expected: &[u32] = match budget {
            0 | 1 => &[5],
            2 => &[7, 5],
            _ => &[7, 7],
        };
        test_verify_eq!(diagnostic_codes(&errors), expected);
        test_verify_eq!(errors.location_status(), LocationStatus::Complete);
        if budget <= 2 {
            let terminal = errors.diagnostics().last().test_value()?;
            let span = terminal.primary().test_value()?.span();
            let trigger = if budget < 2 { "Missing" } else { "@min(1)" };
            test_verify_eq!(source.get(span.start()..span.end()).test_value()?, trigger);
        } else {
            let starts: Vec<_> = errors
                .diagnostics()
                .iter()
                .map(|diagnostic| diagnostic.primary().map(|span| span.span().start()))
                .collect::<Option<Vec<_>>>()
                .test_value()?;
            test_verify!(starts.first().test_value()? < starts.get(1).test_value()?);
        }
    }

    Ok(())
}

#[test]
fn resource_terminal_stays_after_later_semantic_error_and_stops_import_requests() -> TestResult {
    let mut limits = SchemaLimits::default();
    limits.set_max_regex_count(0);
    let source = r#"entry struct Root { early: string @regex("x"), later: Missing }"#;
    let errors = MemoryResolver::new()
        .compile("root", source, &limits)
        .test_error()?;
    test_verify_eq!(diagnostic_codes(&errors), [7, 1]);
    let ordinary = errors
        .diagnostics()
        .first()
        .test_value()?
        .primary()
        .test_value()?
        .span();
    let terminal = errors
        .diagnostics()
        .get(1)
        .test_value()?
        .primary()
        .test_value()?
        .span();
    test_verify!(terminal.start() < ordinary.start());
    test_verify_eq!(
        source.get(terminal.start()..terminal.end()).test_value()?,
        "@regex(\"x\")"
    );

    let mut resolver = MemoryResolver::new();
    resolver.fail_module("first", ResolverError::ResourceLimitExceeded);
    resolver
        .compile(
            "root",
            "extern mod first; extern mod second; entry struct Root { bad: Missing }",
            &limits,
        )
        .test_error()?;
    test_verify_eq!(resolver.requests.len(), 1);
    test_verify_eq!(resolver.requests.first().test_value()?.module, "first");

    Ok(())
}

#[test]
fn recoverable_import_errors_preserve_independent_semantic_checks() -> TestResult {
    let root = "extern mod missing; entry struct Root { bad: Missing }";
    let errors = MemoryResolver::new()
        .compile("root", root, &SchemaLimits::default())
        .test_error()?;
    test_verify_eq!(diagnostic_codes(&errors), [7, 7]);
    test_verify_eq!(
        errors.diagnostics().first().test_value()?.message(),
        "imported schema file does not exist"
    );
    let span = errors
        .diagnostics()
        .first()
        .test_value()?
        .primary()
        .test_value()?
        .span();
    test_verify_eq!(
        root.get(span.start()..span.end()).test_value()?,
        "extern mod missing;"
    );

    let mut resolver = MemoryResolver::new();
    resolver.add("broken.tschem", "broken", "struct Broken {");
    let errors = resolver
        .compile(
            "root",
            "extern mod broken; entry struct Root { bad: Missing }",
            &SchemaLimits::default(),
        )
        .test_error()?;
    test_verify!(diagnostic_codes(&errors).contains(&4));
    test_verify!(errors.diagnostics().iter().any(|diagnostic| {
        diagnostic.code() == DiagnosticCode::SCHEMA_SEMANTIC_ERROR
            && diagnostic
                .primary()
                .is_some_and(|span| span.source_id().get() == 0)
    }));
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

#[test]
fn recursive_user_types_skip_derived_default_errors() -> TestResult {
    let source = "struct Loop { value: Loop = {} } entry struct Root { value: Loop }";
    let errors = MemoryResolver::new()
        .compile("root", source, &SchemaLimits::default())
        .test_error()?;
    test_verify_eq!(diagnostic_codes(&errors), [7]);
    test_verify!(
        errors
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.message() == "user-defined types cannot be recursive")
    );
    test_verify!(
        !errors
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.message() == "default value has recursive expansion")
    );

    Ok(())
}

#[test]
fn failed_import_slots_do_not_shift_successful_targets_or_repeat_path_errors() -> TestResult {
    let mut resolver = MemoryResolver::new();
    resolver.add("broken.tschem", "broken", "struct Broken {");
    resolver.add(
        "good.tschem",
        "good",
        "struct Good { bad: Missing, valid: u8 = 300 }",
    );
    let root = "extern mod missing; extern mod broken; extern mod good; type Alias = missing::Item; entry struct Root { a: Alias = {}, b: broken::Item = {}, c: good::Good = {} }";
    let errors = resolver
        .compile("root", root, &SchemaLimits::default())
        .test_error()?;
    let semantic: Vec<_> = errors
        .diagnostics()
        .iter()
        .filter(|diagnostic| diagnostic.code() == DiagnosticCode::SCHEMA_SEMANTIC_ERROR)
        .collect();
    test_verify_eq!(semantic.len(), 3);
    test_verify_eq!(
        semantic.first().test_value()?.message(),
        "imported schema file does not exist"
    );
    test_verify_eq!(semantic.get(1).test_value()?.message(), "unknown type path");
    test_verify_eq!(
        semantic
            .get(1)
            .test_value()?
            .primary()
            .test_value()?
            .source_id()
            .get(),
        2
    );
    test_verify_eq!(
        semantic
            .get(2)
            .test_value()?
            .primary()
            .test_value()?
            .source_id()
            .get(),
        2
    );
    test_verify_eq!(resolver.requests.len(), 3);
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
        for label in diagnostic.labels() {
            test_verify!(errors.span_location(label.span()).is_none());
        }
    }

    Ok(())
}

#[test]
fn recursive_and_incomplete_dependencies_skip_defaults_but_check_independent_fields() -> TestResult
{
    for source in [
        "struct A { next: B = {} } struct B { back: A = {}, other: C = {} } struct C { back: A = {} } type Alias = option<[C]>; entry struct Root { bad: Alias = [], independent: u8 = 300 }",
        "struct Broken { bad: Missing, independent: u8 = 300 } type Alias = Broken; entry struct Root { bad: Alias = {} }",
        "type A = B; type B = A; entry struct Root { bad: [A] = [], independent: u8 = 300 }",
        "enum A { Next(B) } enum B { Back { a: A = {} } } entry struct Root { bad: A = .Next(.Back({})), independent: u8 = 300 }",
    ] {
        let errors = MemoryResolver::new()
            .compile("root", source, &SchemaLimits::default())
            .test_error()?;
        test_verify!(
            errors
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.message().contains("range")),
            "independent defaults should be checked: {source}; {:?}",
            errors
                .diagnostics()
                .iter()
                .map(|diagnostic| (diagnostic.code(), diagnostic.message()))
                .collect::<Vec<_>>()
        );
        test_verify!(
            !errors
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.message() == "default value has recursive expansion")
        );
        test_verify!(
            errors
                .diagnostics()
                .iter()
                .all(|diagnostic| diagnostic.code() == DiagnosticCode::SCHEMA_SEMANTIC_ERROR)
        );
    }

    Ok(())
}

#[test]
fn loading_and_semantic_passes_share_diagnostic_budget_and_terminal_tail() -> TestResult {
    let source = "extern mod missing; entry struct Root { early: string @min(1), later: Missing }";
    for limit in 0..=4 {
        let mut limits = SchemaLimits::default();
        limits.set_max_diagnostics(limit);
        let errors = MemoryResolver::new()
            .compile("root", source, &limits)
            .test_error()?;
        let expected: &[u32] = match limit {
            0 | 1 => &[5],
            2 => &[7, 5],
            3 => &[7, 7, 5],
            _ => &[7, 7, 7],
        };
        test_verify_eq!(diagnostic_codes(&errors), expected);
        if limit == 3 {
            let span = errors
                .diagnostics()
                .last()
                .test_value()?
                .primary()
                .test_value()?
                .span();
            test_verify_eq!(
                source.get(span.start()..span.end()).test_value()?,
                "@min(1)"
            );
        }
    }

    Ok(())
}

#[test]
fn import_depth_and_file_budget_stop_before_resolver_requests() -> TestResult {
    let root = "extern mod child; entry struct Root {}";
    for (depth, expected_requests) in [(0, 0), (1, 1)] {
        let mut limits = SchemaLimits::default();
        limits.set_max_nesting_depth(depth);
        let mut resolver = MemoryResolver::new();
        resolver.add("child.tschem", "child", "extern mod next; struct Child {}");
        let errors = resolver.compile("root", root, &limits).test_error()?;
        test_verify_eq!(diagnostic_codes(&errors), [1]);
        test_verify_eq!(resolver.requests.len(), expected_requests);
    }
    let mut limits = SchemaLimits::default();
    limits.set_max_schema_files(1);
    let mut resolver = MemoryResolver::new();
    resolver.add("child.tschem", "root", root);
    let errors = resolver.compile("root", root, &limits).test_error()?;
    test_verify_eq!(diagnostic_codes(&errors), [1]);
    test_verify!(resolver.requests.is_empty());

    Ok(())
}

#[test]
fn failed_source_bytes_are_charged_and_fatal_errors_stop_all_later_work() -> TestResult {
    let root = "extern mod broken; extern mod good; entry struct Root { bad: Missing }";
    let broken = "struct Broken {";
    let mut limits = SchemaLimits::default();
    limits.set_max_schema_total_bytes(root.len() + broken.len());
    let mut resolver = MemoryResolver::new();
    resolver.add("broken.tschem", "broken", broken);
    resolver.add("good.tschem", "good", "struct Good {}");
    let errors = resolver.compile("root", root, &limits).test_error()?;
    test_verify_eq!(
        errors.diagnostics().last().test_value()?.code(),
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
    );
    test_verify!(
        errors
            .diagnostics()
            .iter()
            .all(|diagnostic| diagnostic.code() != DiagnosticCode::SCHEMA_SEMANTIC_ERROR)
    );
    test_verify_eq!(resolver.requests.get(1).test_value()?.source_byte_limit, 0);
    test_verify_eq!(
        errors.location_status(),
        LocationStatus::Unavailable(LocationFailure::ResourceLimitExceeded)
    );

    Ok(())
}

#[test]
fn assertions_depending_on_invalid_root_do_not_add_derived_errors() -> TestResult {
    let source = "struct Independent { valid: u8 = 300 } @assert(root.bad > 0, \"rule\") entry struct Root { bad: Missing }";
    let errors = MemoryResolver::new()
        .compile("root", source, &SchemaLimits::default())
        .test_error()?;
    test_verify_eq!(diagnostic_codes(&errors), [7, 7]);
    test_verify!(
        !errors
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.message() == "field type is unavailable")
    );

    Ok(())
}

#[test]
fn recoverable_host_failure_keeps_siblings_and_root_syntax_failure_stops_loading() -> TestResult {
    let mut resolver = MemoryResolver::new();
    resolver.fail_module("failed", ResolverError::HostFailure);
    resolver.add("good.tschem", "good", "struct Good { bad: Missing }");
    let errors = resolver
        .compile(
            "root",
            "extern mod failed; extern mod good; entry struct Root { bad: failed::Item }",
            &SchemaLimits::default(),
        )
        .test_error()?;
    test_verify_eq!(diagnostic_codes(&errors), [8, 7]);
    test_verify_eq!(resolver.requests.len(), 2);
    let mut resolver = MemoryResolver::new();
    let errors = resolver
        .compile(
            "root",
            "extern mod good; entry struct Root {",
            &SchemaLimits::default(),
        )
        .test_error()?;
    test_verify!(diagnostic_codes(&errors).contains(&4));
    test_verify!(resolver.requests.is_empty());

    Ok(())
}
