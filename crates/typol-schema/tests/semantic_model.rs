#[path = "support/schema_compile.rs"]
mod schema_support;

use std::{format, string::String};
use typol_core::{ExecutionBudget, ExecutionLimits, SourceId, SourceSpan};
use typol_policy::{RegexLimits, ValueLimits};
use typol_schema::{
    ResolverError, SchemaCompileOptions, SchemaLimits, SchemaResolveRequest, SchemaResolver,
    SchemaSource, SourceLoadedRequest, SourceObserver, SourceObserverError,
    opeo::{OResult, Out},
};
use typol_semantic::{
    ComparisonOptions, CompiledSchema, Constraint, ConstraintData, ConstraintKind, EffectiveRules,
    EvalError, FieldView, NumberValue, SchemaAccessError, TypeKind, TypeView,
};
use typol_test_support::*;

#[derive(Debug, PartialEq, Eq)]
struct SourceNotification {
    source_id: SourceId,
    identity: Vec<u8>,
}

struct Observer {
    notifications: Vec<SourceNotification>,
}

impl SourceObserver for Observer {
    fn source_loaded<'slot>(
        &mut self,
        request: SourceLoadedRequest<'_>,
        _out: Out<'slot, '_, SourceObserverError>,
    ) -> OResult<'slot, ()> {
        self.notifications.push(SourceNotification {
            source_id: request.source_id,
            identity: request.identity.to_vec(),
        });
        Ok(())
    }
}

struct Sources;

impl SchemaResolver for Sources {
    fn resolve<'src, 'slot>(
        &'src mut self,
        request: SchemaResolveRequest<'_>,
        out: Out<'slot, '_, ResolverError>,
    ) -> OResult<'slot, SchemaSource<'src>> {
        match request.module_name {
            "broken" => Ok(SchemaSource::from_str(b"broken", "struct")),
            "good" | "duplicate" => Ok(SchemaSource::from_str(
                b"same",
                "struct Item { name: string }",
            )),
            _ => Err(out.fail(ResolverError::NotFound)),
        }
    }
}

fn compile(source: &str) -> TestResult<CompiledSchema> {
    schema_support::SchemaCompileTestSupport::run_without_imports(
        SchemaCompileOptions::new(SchemaLimits::default()),
        SchemaSource::from_str(b"owned-root", source),
    )
    .test_value()
}

fn access<T>(
    operation: impl for<'slot> FnOnce(Out<'slot, 'slot, SchemaAccessError>) -> OResult<'slot, T>,
) -> TestResult<T> {
    schema_support::SchemaCompileTestSupport::schema_access(operation).test_value()
}

fn present<T>(value: Option<T>) -> TestResult<T> {
    value.ok_or_else(|| TestFailure::new("expected member is missing from the schema"))
}

fn schema_field<'a>(ty: TypeView<'a>, name: &str) -> TestResult<FieldView<'a>> {
    present(access(|out| ty.field_named(name, out))?)
}

fn field_type<'a>(schema: &'a CompiledSchema, name: &str) -> TestResult<TypeView<'a>> {
    let field = schema_field(schema.entry(), name)?;
    access(|out| field.ty(out))
}

fn rule_data(rule: Constraint<'_>) -> TestResult<ConstraintData<'_>> {
    access(|out| rule.data(out))
}

fn first_constraint(field: FieldView<'_>) -> TestResult<Constraint<'_>> {
    schema_support::SchemaCompileTestSupport::collect_constraints(access(|out| {
        field.constraints(out)
    })?)?
    .into_iter()
    .next()
    .test_value()
}

fn is_schema_evaluation_internal(error: &EvalError, source_span: Option<SourceSpan>) -> bool {
    error.is_internal()
        && error.context().is_some_and(|context| {
            context.subsystem() == "typol-semantic"
                && context.stage() == "schema-evaluation"
                && context.failure_point() == "cross-schema-expression-or-view"
        })
        && error.source_span() == source_span
}

#[test]
fn shapes_ignore_use_spans_and_refinements_but_preserve_nominal_identity_and_length() -> TestResult
{
    let schema = compile(
        "type Byte = u8 @min(1); struct A { x: u8 } struct B { x: u8 } enum E { V } enum F { V } entry struct Root { raw: u8, refined: Byte, a: [u8], b: [Byte], fixed: [u8;2], other: [Byte;3], opt: option<u8>, refined_opt: option<Byte>, dictionary: map<string,u8>, refined_map: map<string,Byte>, first: A, repeated: A, second: B, en: E, other_en: F }",
    ).test_value()? ;
    for (left, right) in [
        ("raw", "refined"),
        ("a", "b"),
        ("opt", "refined_opt"),
        ("dictionary", "refined_map"),
        ("first", "repeated"),
    ] {
        let left = field_type(&schema, left).test_value()?;
        let right = field_type(&schema, right).test_value()?;
        test_verify_ne!(left, right);
        test_verify!(access(|out| left.same_type(right, out))?);
        test_verify_ne!(
            access(|out| left.span(out))?,
            access(|out| right.span(out))?
        );
    }
    for (left, right) in [
        ("a", "fixed"),
        ("fixed", "other"),
        ("first", "second"),
        ("en", "other_en"),
    ] {
        let left = field_type(&schema, left).test_value()?;
        let right = field_type(&schema, right).test_value()?;
        test_verify!(!access(|out| left.same_type(right, out))?);
    }
    let raw = field_type(&schema, "raw").test_value()?;
    let refined = field_type(&schema, "refined").test_value()?;
    test_verify!(
        schema_support::SchemaCompileTestSupport::collect_constraints(access(
            |out| raw.constraints(out)
        )?)?
        .is_empty()
    );
    test_verify_eq!(
        schema_support::SchemaCompileTestSupport::collect_constraints(access(
            |out| refined.constraints(out)
        )?)?
        .len(),
        1
    );

    Ok(())
}

#[test]
fn alias_fanout_shares_program_message_and_rules_without_ancestor_copies() -> TestResult {
    let mut baseline = None;
    for count in [1, 256] {
        let mut source = String::from(
            "type Base = [u8] @all(item > 0, \"shared-message\"); type Mid = Base @size(min=1); type Top = Mid @size(max=8); entry struct Root {",
        );
        for index in 0..count {
            source.push_str(&format!("field{index}: Top,"));
        }
        source.push('}');
        let schema = compile(&source).test_value()?;
        drop(source);
        let counts = schema.model_counts();
        test_verify_eq!(counts.constraints, 3);
        test_verify_eq!(counts.programs, 1);
        test_verify_eq!(counts.refinements, 3);
        if let Some((shapes, nodes)) = baseline {
            test_verify_eq!(counts.shapes, shapes);
            test_verify_eq!(counts.expression_nodes, nodes);
        } else {
            baseline = Some((counts.shapes, counts.expression_nodes));
        }
        let mut shared = None;
        for field in access(|out| schema.entry().fields(out))? {
            let constraints =
                schema_support::SchemaCompileTestSupport::collect_constraints(access(|out| {
                    field.constraints(out)
                })?)?;
            test_verify_eq!(
                constraints
                    .iter()
                    .map(|rule| rule.kind())
                    .collect::<Vec<_>>(),
                [
                    ConstraintKind::Size,
                    ConstraintKind::Size,
                    ConstraintKind::All
                ]
            );
            for rule in constraints {
                if let ConstraintData::All {
                    expression,
                    message,
                } = rule_data(rule).test_value()?
                {
                    test_verify_eq!(message, "shared-message");
                    let identity = (access(|out| expression.root(out))?, message.as_ptr());
                    if let Some(prior) = shared {
                        test_verify_eq!(identity, prior);
                    } else {
                        shared = Some(identity);
                    }
                }
            }
        }
        test_verify!(shared.is_some());
    }

    Ok(())
}

#[test]
fn effective_rules_deduplicate_stable_ids_and_keep_alias_first_order() -> TestResult {
    let schema = compile(
        "type Base = u8 @min(1) @max(9); type Alias = Base @min(2); entry struct Root { value: Alias @max(8) }",
    )?;
    let field = schema_field(schema.entry(), "value")?;
    let alias = access(|out| field.ty(out))?;
    let alias_rules = access(|out| alias.constraints(out))?;
    let local_rules = access(|out| field.constraints(out))?;
    let alias_items = schema_support::SchemaCompileTestSupport::collect_constraints(alias_rules)?;
    let local_items = schema_support::SchemaCompileTestSupport::collect_constraints(local_rules)?;
    let inherited = [alias_rules, alias_rules];
    let effective =
        TestOpeo::call(|out| EffectiveRules::new(&schema, &inherited, local_rules, out))
            .map_err(|error| TestFailure::new(format!("rule ownership error: {error:?}")))?;
    let budget = schema_support::SchemaCompileTestSupport::budget(|out| {
        ExecutionBudget::new(ExecutionLimits::new(67_108_864, 67_108_864), out)
    })
    .test_value()?;
    let rules = schema_support::SchemaCompileTestSupport::budget(|out| {
        effective.collect_budgeted(&budget, out)
    })
    .test_value()?;
    let identities = rules.iter().map(|rule| rule.identity()).collect::<Vec<_>>();
    let mut expected = Vec::new();
    for identity in alias_items
        .iter()
        .chain(local_items.iter())
        .map(|rule| rule.identity())
    {
        if !expected.contains(&identity) {
            expected.push(identity);
        }
    }
    test_verify_eq!(identities, expected);
    test_verify_eq!(rules.len(), expected.len());
    Ok(())
}

#[test]
fn variant_name_lookup_uses_budgeted_sorted_index_for_wide_enums() -> TestResult {
    let mut source = String::from("enum Large {");
    for index in (0..512).rev() {
        source.push_str(&format!(" Variant{index:03},"));
    }
    source.push_str("} entry struct Root { value: Large }");
    let schema = compile(&source)?;
    let large = field_type(&schema, "value")?;
    let budget = schema_support::SchemaCompileTestSupport::budget(|out| {
        ExecutionBudget::new(ExecutionLimits::new(256, 1_048_576), out)
    })
    .test_value()?;
    let found = schema_support::SchemaCompileTestSupport::budget(|out| {
        large.variant_named_budgeted("Variant000", &budget, out)
    })
    .test_value()?;
    test_verify_eq!(
        present(found)?,
        present(access(|out| large.variant_named("Variant000", out))?)?
    );
    Ok(())
}

#[test]
fn freeze_checks_nested_defaults_after_building_member_indexes() -> TestResult {
    let schema = compile(
        r#"struct Pair { low: u8, high: u8 } entry struct Root { values: [Pair] @all(item.high > item.low, "ordered") = [{low: 1, high: 2}] }"#,
    )?;
    test_verify_eq!(schema.model_counts().constraints, 1);
    Ok(())
}

#[test]
fn refinement_summary_keeps_source_order_secret_and_host_metadata_separate() -> TestResult {
    let source = "type Base = u8 @min(1); type Mid = Base @max(5); type Host = string @secret @host; type Hidden = string @secret; entry struct Root { value: Mid @range(2,4), host: Host @env(\"SERVICE_HOST\"), hidden: Hidden }";
    let schema = compile(source).test_value()?;
    let field = schema_field(schema.entry(), "value").test_value()?;
    let constraint_list = access(|out| field.constraints(out))?;
    let rules = schema_support::SchemaCompileTestSupport::collect_constraints(constraint_list)?;
    test_verify_eq!(
        rules.iter().map(|rule| rule.kind()).collect::<Vec<_>>(),
        [
            ConstraintKind::Max,
            ConstraintKind::Min,
            ConstraintKind::Range
        ]
    );
    test_verify_eq!(
        rules
            .iter()
            .map(|rule| rule.span().span().start())
            .collect::<Vec<_>>(),
        [
            source.find("@max").test_value()?,
            source.find("@min").test_value()?,
            source.find("@range").test_value()?
        ]
    );
    let bounds = access(|out| constraint_list.summary(out))?.number();
    test_verify_eq!(bounds.min(), Some(NumberValue::Unsigned(2)));
    test_verify_eq!(bounds.max(), Some(NumberValue::Unsigned(4)));
    let host = schema_field(schema.entry(), "host").test_value()?;
    test_verify!(access(|out| host.is_secret(out))?);
    let host_constraints = access(|out| host.constraints(out))?;
    test_verify!(access(|out| host_constraints.has_host(out))?);
    test_verify_eq!(
        schema_support::SchemaCompileTestSupport::collect_constraints(host_constraints)?.len(),
        1
    );
    test_verify_eq!(
        present(access(|out| host.environment(out))?)?.name(),
        "SERVICE_HOST"
    );
    let hidden = schema_field(schema.entry(), "hidden").test_value()?;
    test_verify!(access(|out| hidden.is_secret(out))?);
    let hidden_constraints = access(|out| hidden.constraints(out))?;
    test_verify!(!access(|out| hidden_constraints.has_host(out))?);
    test_verify!(
        schema_support::SchemaCompileTestSupport::collect_constraints(hidden_constraints)?
            .is_empty()
    );

    Ok(())
}

struct ReusedSource {
    text: String,
    identity: String,
}

impl SchemaResolver for ReusedSource {
    fn resolve<'src, 'slot>(
        &'src mut self,
        request: SchemaResolveRequest<'_>,
        out: Out<'slot, '_, ResolverError>,
    ) -> OResult<'slot, SchemaSource<'src>> {
        let module = request.module_name;
        self.identity.clear();
        self.identity.push_str(module);
        self.text.clear();
        self.text.push_str(match module {
            "first" => "type Values = [u8] @all(item > 0, \"retained\");",
            "second" => "struct Item { name: string }",
            _ => return Err(out.fail(ResolverError::NotFound)),
        });
        Ok(SchemaSource::from_str(self.identity.as_bytes(), &self.text))
    }
}

#[test]
fn frozen_model_owns_data_after_resolver_buffers_and_root_are_destroyed() -> TestResult {
    let schema = {
        let source = String::from(
            "extern mod first; extern mod second; entry struct Root { values: first::Values = [1], item: second::Item = {name: \"owned\"} }",
        );
        let mut resolver = ReusedSource {
            text: String::new(),
            identity: String::new(),
        };
        schema_support::SchemaCompileTestSupport::run(
            SchemaCompileOptions::new(SchemaLimits::default()),
            SchemaSource::from_str(b"root", &source),
            &mut resolver,
            None,
        )
        .test_value()?
    };
    let values = schema_field(schema.entry(), "values").test_value()?;
    test_verify!(access(|out| values.default_value(out))?.is_some());
    let rule = schema_support::SchemaCompileTestSupport::collect_constraints(access(|out| {
        values.constraints(out)
    })?)?
    .into_iter()
    .next()
    .test_value()?;
    let ConstraintData::All {
        expression,
        message,
    } = rule_data(rule).test_value()?
    else {
        return Err(String::from("collection program should be preserved").into());
    };
    test_verify_eq!(message, "retained");
    test_verify!(schema.span_location(expression.span()).is_some());
    test_verify_eq!(rule.span().source_id(), SourceId::new(1));
    let item = field_type(&schema, "item").test_value()?;
    test_verify_eq!(access(|out| item.name(out))?, Some("Item"));
    test_verify_eq!(access(|out| item.span(out))?.source_id(), SourceId::new(0));
    test_verify_eq!(
        present(access(|out| item.declaration_span(out))?)?.source_id(),
        SourceId::new(2)
    );
    let TypeKind::Struct { fields } = access(|out| item.kind(out))? else {
        return Err(String::from("nominal struct should be preserved").into());
    };
    let first_field = present(fields.get(0)).test_value()?;
    test_verify_eq!(access(|out| first_field.name(out))?, "name");

    Ok(())
}

#[test]
fn views_bind_identity_to_schema_and_program_and_navigate_without_indices() -> TestResult {
    use typol_semantic::{
        ExpressionArgument, ExpressionBindingInfo, ExpressionKind, SchemaBindings as Bindings,
        SchemaEvaluator as Evaluator, SchemaValue as EvalValue,
    };
    let source = r#"entry struct Root { a: [[u8]] @all(item.all(|x| x > 0), "hidden") = [[1]] }"#;
    let a = compile(source).test_value()?;
    let b = compile(source).test_value()?;
    test_verify_ne!(a.entry(), b.entry());
    test_verify!(!access(|out| a.entry().same_type(b.entry(), out))?);
    let field = schema_field(a.entry(), "a").test_value()?;
    test_verify_eq!(
        field,
        present(access(|out| a.entry().fields(out))?.get(0)).test_value()?
    );
    test_verify_ne!(field, schema_field(b.entry(), "a").test_value()?);
    let ConstraintData::All { expression, .. } = rule_data(first_constraint(field)?)? else {
        return Err(String::from("program should be preserved").into());
    };
    let ConstraintData::All {
        expression: foreign,
        ..
    } = rule_data(first_constraint(schema_field(b.entry(), "a")?)?)?
    else {
        return Err(String::from("program should be preserved").into());
    };
    test_verify_ne!(expression, foreign);
    test_verify_ne!(
        access(|out| expression.root(out))?,
        access(|out| foreign.root(out))?
    );
    let expression_kind = {
        let root = access(|out| expression.root(out))?;
        access(|out| root.kind(out))?
    };
    let ExpressionKind::Method {
        receiver,
        argument: Some(ExpressionArgument::Closure { parameter, body }),
        ..
    } = expression_kind
    else {
        return Err(String::from("quantifier should be preserved").into());
    };
    let receiver_kind = access(|out| receiver.kind(out))?;
    test_verify!(matches!(
        receiver_kind,
        ExpressionKind::Binding(ExpressionBindingInfo::Item)
    ));
    let ExpressionKind::Binary { left, right, .. } = access(|out| body.kind(out))? else {
        return Err(String::from("comparison should be preserved").into());
    };
    let ExpressionKind::Binding(ExpressionBindingInfo::Closure(binding)) =
        access(|out| left.kind(out))?
    else {
        return Err(String::from("closure binding should be preserved").into());
    };
    test_verify!(binding == parameter);
    let right_kind = access(|out| right.kind(out))?;
    test_verify!(matches!(right_kind, ExpressionKind::Literal(_)));
    let limits = ValueLimits::default();
    let regex_limits = RegexLimits::default();
    let evaluator = Evaluator::new(&b, &limits, &regex_limits, None);
    let field_type = access(|out| field.ty(out))?;
    test_verify!(is_schema_evaluation_internal(
        schema_support::SchemaCompileTestSupport::evaluation(|out| evaluator.host(field_type, out))
            .test_error()
            .as_ref()
            .test_value()?,
        None
    ));
    test_verify!(is_schema_evaluation_internal(
        schema_support::SchemaCompileTestSupport::evaluation(|out| {
            evaluator.evaluate(expression, Bindings::default(), out)
        })
        .test_error()
        .as_ref()
        .test_value()?,
        Some(expression.span())
    ));
    let value = present(access(|out| field.default_value(out))?)?;
    let left_type = access(|out| left.ty(out))?;
    test_verify!(is_schema_evaluation_internal(
        schema_support::SchemaCompileTestSupport::evaluation(|out| {
            evaluator.checked(
                EvalValue::config(value.root(), left_type, false),
                false,
                out,
            )
        })
        .test_error()
        .as_ref()
        .test_value()?,
        None
    ));
    let redacted = format!("{expression:?} {field:?}");
    test_verify!(!redacted.contains("hidden"));

    Ok(())
}

#[test]
fn observer_receives_actual_ids_before_parsing_and_skips_duplicate_identities() -> TestResult {
    let mut observer = Observer {
        notifications: Vec::new(),
    };
    let options = SchemaCompileOptions::new(SchemaLimits::default());
    let errors = schema_support::SchemaCompileTestSupport::run(
        options,
        SchemaSource::from_str(
            b"root",
            "extern mod broken; extern mod good; extern mod duplicate; entry struct Root { item: good::Item }",
        ),
        &mut Sources,
        Some(&mut observer),
    )
    .test_error()?;
    test_verify_eq!(
        observer.notifications,
        [
            SourceNotification {
                source_id: SourceId::new(0),
                identity: b"root".to_vec(),
            },
            SourceNotification {
                source_id: SourceId::new(1),
                identity: b"broken".to_vec(),
            },
            SourceNotification {
                source_id: SourceId::new(2),
                identity: b"same".to_vec(),
            }
        ]
    );
    test_verify!(errors.diagnostics().iter().any(|d| {
        d.primary()
            .is_some_and(|span| span.source_id() == SourceId::new(1))
    }));
    observer.notifications.clear();
    let errors = schema_support::SchemaCompileTestSupport::run(
        SchemaCompileOptions::new(SchemaLimits::default()),
        SchemaSource::from_str(b"root", "struct"),
        &mut Sources,
        Some(&mut observer),
    )
    .test_error()?;
    test_verify_eq!(observer.notifications.len(), 1);
    test_verify!(!errors.diagnostics().is_empty());
    observer.notifications.clear();
    let schema = schema_support::SchemaCompileTestSupport::run(
        SchemaCompileOptions::new(SchemaLimits::default()),
        SchemaSource::from_str(
            b"root",
            "extern mod good; entry struct Root { item: good::Item }",
        ),
        &mut Sources,
        Some(&mut observer),
    )
    .test_value()?;
    test_verify_eq!(observer.notifications.len(), 2);
    test_verify_eq!(schema.next_source_id(), Some(SourceId::new(2)));

    Ok(())
}

#[test]
fn program_and_member_views_distinguish_equal_positions_in_different_owners() -> TestResult {
    use typol_semantic::VariantPayloadInfo;
    let source = r#"enum Event { Empty, Single(u8), Object { value: string @regex("x") } } entry struct Root { event: Event, first: string @regex("x"), second: string @regex("x") } @assert(true, "first") @assert(true, "second")"#;
    let schema = compile(source).test_value()?;
    let other = compile(source).test_value()?;
    let root = schema.entry();
    let assertions = access(|out| root.assertions(out))?;
    let first_assertion = present(access(|out| assertions.get(0, out))?)?;
    let second_assertion = present(access(|out| assertions.get(1, out))?)?;
    test_verify_ne!(first_assertion.expression(), second_assertion.expression());
    test_verify_ne!(
        access(|out| first_assertion.expression().root(out))?,
        access(|out| second_assertion.expression().root(out))?
    );
    let event = field_type(&schema, "event")?;
    let other_event = field_type(&other, "event")?;
    let object = present(access(|out| event.variant_named("Object", out))?)?;
    test_verify_ne!(
        object,
        present(access(|out| other_event.variant_named("Object", out))?)?
    );
    let VariantPayloadInfo::Structure { fields } = access(|out| object.payload(out))? else {
        return Err(String::from("expected a struct payload").into());
    };
    test_verify_eq!(
        present(fields.get(0)).test_value()?,
        schema_field(present(access(|out| object.structure_type(out))?)?, "value")?
    );
    let single = present(access(|out| event.variant_named("Single", out))?)?;
    let VariantPayloadInfo::Single { ty } = access(|out| single.payload(out))? else {
        return Err(String::from("expected a single-value payload").into());
    };
    let type_kind = access(|out| ty.kind(out))?;
    test_verify!(matches!(type_kind, TypeKind::Builtin(_)));
    let empty_variant = present(access(|out| event.variant_named("Empty", out))?)?;
    let empty_payload = access(|out| empty_variant.payload(out))?;
    test_verify!(matches!(empty_payload, VariantPayloadInfo::Unit));
    let first = schema_field(root, "first").test_value()?;
    let second = schema_field(root, "second").test_value()?;
    let ConstraintData::Regex { program: a } = rule_data(first_constraint(first)?)? else {
        return Err(String::from("expected a regex").into());
    };
    let ConstraintData::Regex { program: b } = rule_data(first_constraint(second)?)? else {
        return Err(String::from("expected a regex").into());
    };
    test_verify!(a != b);
    let ConstraintData::Regex { program: foreign } =
        rule_data(first_constraint(schema_field(other.entry(), "first")?)?)?
    else {
        return Err(String::from("expected a regex").into());
    };
    test_verify!(a != foreign);

    Ok(())
}

#[test]
fn shared_execution_rejects_foreign_values_and_unique_paths() -> TestResult {
    use typol_semantic::{
        SchemaComparison, SchemaEvaluator as Evaluator, SchemaRules, SchemaValue as EvalValue,
    };
    let source = r#"struct Item { key: u8 } entry struct Root { items: [Item] @unique(by = "key") = [{key: 1}] }"#;
    let schema = compile(source).test_value()?;
    let other = compile(source).test_value()?;
    let limits = ValueLimits::default();
    let regex_limits = RegexLimits::default();
    let evaluator = Evaluator::new(&schema, &limits, &regex_limits, None);
    let items = schema_field(schema.entry(), "items")?;
    let value = present(access(|out| items.default_value(out))?)?;
    let local = field_type(&schema, "items")?;
    let foreign = field_type(&other, "items")?;
    let local_fingerprint = schema_support::SchemaCompileTestSupport::evaluation(|out| {
        SchemaRules::fingerprint(
            &evaluator,
            EvalValue::config(value.root(), foreign, false),
            out,
        )
    });
    test_verify!(
        matches!(local_fingerprint, schema_support::TestOpeoOutcome::Error(ref error) if is_schema_evaluation_internal(error, None))
    );
    let comparison = schema_support::SchemaCompileTestSupport::evaluation(|out| {
        SchemaComparison::compare(
            &evaluator,
            EvalValue::config(value.root(), local, false),
            EvalValue::config(value.root(), foreign, false),
            ComparisonOptions {
                equality_only: true,
                depth: 0,
            },
            out,
        )
    });
    test_verify!(
        matches!(comparison, schema_support::TestOpeoOutcome::Error(ref error) if is_schema_evaluation_internal(error, None))
    );
    let TypeKind::Array { element, .. } = access(|out| local.kind(out))? else {
        return Err(String::from("expected an array").into());
    };
    let foreign_items = schema_field(other.entry(), "items")?;
    let ConstraintData::Unique { by } = rule_data(first_constraint(foreign_items)?)? else {
        return Err(String::from("expected a uniqueness rule").into());
    };
    let unique = schema_support::SchemaCompileTestSupport::evaluation(|out| {
        SchemaRules::unique(&evaluator, value.root(), element, by, out)
    });
    test_verify!(
        matches!(unique, schema_support::TestOpeoOutcome::Error(ref error) if is_schema_evaluation_internal(error, None))
    );

    Ok(())
}
