use crate::{
    ConfigLimits,
    parser::tests::{ParserTestSupport, foundation::Provider},
    value::ValueFactory as LocalValueFactory,
};
use alloc::vec::Vec;
use core::cmp::Ordering;
use typol_core::{ConfigValue, ValueError, ValueKind, ValuePayloadView, ValueSource, ValueView};
use typol_model::BuiltinType;
use typol_policy::{RegexLimits, ValueLimits};
use typol_semantic::{
    ComparisonOptions, CompiledSchema, EvalError, EvalFailure, EvalKind, ExpressionBindingInfo,
    ExpressionKind, ExpressionView, Numeric, SchemaAccessError, SchemaBindings as Bindings,
    SchemaComparison, SchemaEvaluator as Evaluator, SchemaValue as EvalValue,
};
use typol_test_support::*;

fn compile(text: &str) -> TestResult<CompiledSchema> {
    ParserTestSupport::compile(text)
}

fn parse(schema: &CompiledSchema, text: &str) -> TestResult<ConfigValue> {
    ParserTestSupport::parse_local_value(
        schema,
        text,
        &mut Provider::new(Ok(None)),
        &ConfigLimits::default(),
    )
    .map(|(value, _)| value)
}

fn value_field<'a>(root: &'a ConfigValue, name: &str) -> TestResult<ValueView<'a>> {
    let ValueKind::Structure(mut fields) = root.root().as_ref() else {
        return Err(TestFailure::new(
            "configuration root node should be a struct",
        ));
    };
    fields
        .find(|entry| entry.name() == Some(name))
        .test_value()
        .map(|entry| entry.value())
}

fn evaluate_bool<'schema>(
    errors: &mut TestSlot<EvalError>,
    evaluator: &Evaluator<'_>,
    expression: ExpressionView<'schema>,
    bindings: Bindings<'schema>,
) -> TestOpeoOutcome<bool, EvalError> {
    match errors.run(|out| evaluator.evaluate(expression, bindings, out)) {
        TestOpeoOutcome::Value(value) => errors.run(|out| value.boolean(out)),
        TestOpeoOutcome::Error(error) => TestOpeoOutcome::Error(error),
    }
}

fn compare(
    evaluator: &Evaluator<'_>,
    left: EvalValue<'_>,
    right: EvalValue<'_>,
    host: bool,
    depth: usize,
) -> TestOpeoOutcome<Ordering, EvalError> {
    let mut errors = TestSlot::default();
    errors.run(|out| {
        SchemaComparison::compare(
            evaluator,
            left,
            right,
            ComparisonOptions {
                equality_only: host,
                depth,
            },
            out,
        )
    })
}

fn evaluate_first(
    schema: &CompiledSchema,
    root: &ConfigValue,
) -> TestOpeoOutcome<bool, EvalFailure> {
    let limits = ValueLimits::default();
    let regex_limits = RegexLimits::default();
    let evaluator = Evaluator::new(schema, &limits, &regex_limits, None);
    let info = schema.entry();
    let mut schema_errors = TestSlot::<SchemaAccessError>::default();
    let assertion = match ParserTestSupport::access(&mut schema_errors, |out| info.assertions(out))
        .and_then(|assertions| {
            ParserTestSupport::access(&mut schema_errors, |out| assertions.get(0, out))
        }) {
        Ok(Some(assertion)) => assertion,
        Ok(None) | Err(_) => return TestOpeoOutcome::Error(EvalFailure::Internal),
    };
    let mut eval_errors = TestSlot::<EvalError>::default();
    match evaluate_bool(
        &mut eval_errors,
        &evaluator,
        assertion.expression(),
        Bindings {
            self_value: Some(EvalValue::config(root.root(), schema.entry(), false)),
            root: Some(root),
            ..Bindings::default()
        },
    ) {
        TestOpeoOutcome::Value(value) => TestOpeoOutcome::Value(value),
        TestOpeoOutcome::Error(error) => TestOpeoOutcome::Error(error.kind()),
    }
}

#[test]
fn ir_type_mismatch_is_an_internal_error() -> TestResult<()> {
    let schema = compile(
        "entry struct Root { values: [u8] } @assert(self.values.all(|v| v > 0), \"rule\")",
    )?;
    let limits = ValueLimits::default();
    let regex_limits = RegexLimits::default();
    let evaluator = Evaluator::new(&schema, &limits, &regex_limits, None);
    let info = schema.entry();
    let mut schema_errors = TestSlot::<SchemaAccessError>::default();
    let assertions = ParserTestSupport::access(&mut schema_errors, |out| info.assertions(out))?;
    let assertion = ParserTestSupport::access(&mut schema_errors, |out| assertions.get(0, out))?
        .test_expect("schema assertion list is empty")?;
    let expression = assertion.expression();
    let value = ValueFactory::unsigned(1)?;
    let mut eval_errors = TestSlot::<EvalError>::default();
    let error = eval_errors
        .run(|out| {
            evaluator.evaluate(
                expression,
                Bindings {
                    self_value: Some(EvalValue::config(value.root(), schema.entry(), false)),
                    ..Bindings::default()
                },
                out,
            )
        })
        .test_error()?;
    test_assert!(error.is_internal())?;
    test_assert!(error.context().is_some())?;
    test_assert!(error.source_span().is_some())?;

    Ok(())
}

#[test]
fn recursive_struct_comparison_uses_field_identity_and_exact_host_text() -> TestResult<()> {
    let schema = compile(
        "struct Child { n: u8, host: string @host } entry struct Root { a: Child, b: Child }",
    )?;
    let root = parse(
        &schema,
        "{a: {n: 1, host: \"Example.COM.\"}, b: {host: \"example.com\", n: 1}}",
    )?;
    let limits = ValueLimits::default();
    let regex_limits = RegexLimits::default();
    let evaluator = Evaluator::new(&schema, &limits, &regex_limits, None);
    let mut schema_errors = TestSlot::<SchemaAccessError>::default();
    let field = ParserTestSupport::required(&mut schema_errors, |out| {
        schema.entry().field_named("a", out)
    })?;
    let ty = ParserTestSupport::access(&mut schema_errors, |out| field.ty(out))?;
    test_eq!(
        compare(
            &evaluator,
            EvalValue::config(value_field(&root, "a")?, ty, false),
            EvalValue::config(value_field(&root, "b")?, ty, false),
            true,
            0
        ),
        TestOpeoOutcome::Value(Ordering::Less)
    )?;

    Ok(())
}

#[test]
fn short_circuit_skips_a_branch_that_would_exceed_depth() -> TestResult<()> {
    let schema = compile(
        "entry struct Root { n: u8 } @assert(true || (((self.n + 1) + 1) + 1) > 0, \"rule\")",
    )?;
    let root = parse(&schema, "{n: 1}")?;
    let info = schema.entry();
    let mut schema_errors = TestSlot::<SchemaAccessError>::default();
    let mut limits = ValueLimits::default();
    let regex_limits = RegexLimits::default();
    limits.set_max_nesting_depth(2);
    let evaluator = Evaluator::new(&schema, &limits, &regex_limits, None);
    let assertions = ParserTestSupport::access(&mut schema_errors, |out| info.assertions(out))?;
    let assertion = ParserTestSupport::access(&mut schema_errors, |out| assertions.get(0, out))?
        .test_expect("schema assertion list is empty")?;
    let mut eval_errors = TestSlot::<EvalError>::default();
    test_assert!(matches!(
        evaluate_bool(
            &mut eval_errors,
            &evaluator,
            assertion.expression(),
            Bindings::default(),
        ),
        TestOpeoOutcome::Value(true)
    ))?;
    let schema = compile("entry struct Root { n: u8 } @assert(self.n > 0, \"rule\")")?;
    let bindings = Bindings {
        self_value: Some(EvalValue::config(root.root(), schema.entry(), false)),
        ..Bindings::default()
    };
    let evaluator = Evaluator::new(&schema, &limits, &regex_limits, None);
    let info = schema.entry();
    let assertions = ParserTestSupport::access(&mut schema_errors, |out| info.assertions(out))?;
    let assertion = ParserTestSupport::access(&mut schema_errors, |out| assertions.get(0, out))?
        .test_expect("schema assertion list is empty")?;
    let error = eval_errors
        .run(|out| evaluator.evaluate(assertion.expression(), bindings, out))
        .test_error()?;
    test_eq!(error.kind(), EvalFailure::Resource)?;
    test_assert!(error.source_span().is_some())?;

    Ok(())
}

#[test]
fn anonymous_variant_self_borrows_payload_fields_without_a_second_value_tree() -> TestResult<()> {
    let schema = compile(
        "enum E { Object { n: u8 } @assert(self.n < root.limit, \"rule\"), Unit @assert(root.limit > 0, \"rule\") } entry struct Root { limit: u8, e: E }",
    )?;
    let root = parse(&schema, "{limit: 5, e: .Object {n: 3}}")?;
    let ValueKind::Enumeration {
        payload: ValuePayloadView::Structure(payload),
        ..
    } = value_field(&root, "e")?.as_ref()
    else {
        return Err(TestFailure::new(
            "enum payload should be an anonymous struct",
        ));
    };
    let mut schema_errors = TestSlot::<SchemaAccessError>::default();
    let enum_field = ParserTestSupport::required(&mut schema_errors, |out| {
        schema.entry().field_named("e", out)
    })?;
    let enum_ty = ParserTestSupport::access(&mut schema_errors, |out| enum_field.ty(out))?;
    let variant = ParserTestSupport::required(&mut schema_errors, |out| {
        enum_ty.variant_named("Object", out)
    })?;
    let assertions = ParserTestSupport::access(&mut schema_errors, |out| variant.assertions(out))?;
    let assertion = ParserTestSupport::access(&mut schema_errors, |out| assertions.get(0, out))?
        .test_expect("schema assertion list is empty")?;
    let expression = assertion.expression();
    let mut iteration_errors = TestSlot::<SchemaAccessError>::default();
    let nodes = iteration_errors
        .run(|mut out| {
            let mut nodes = Vec::new();
            for result in expression.nodes(out.reborrow()) {
                match result {
                    Ok(node) => nodes.push(node),
                    Err(failure) => return Err(failure),
                }
            }
            Ok(nodes)
        })
        .test_value()?;
    let mut self_type = None;
    for node in nodes {
        if matches!(
            ParserTestSupport::access(&mut schema_errors, |out| node.kind(out))?,
            ExpressionKind::Binding(ExpressionBindingInfo::SelfValue)
        ) {
            self_type = Some(ParserTestSupport::access(&mut schema_errors, |out| {
                node.ty(out)
            })?);
            break;
        }
    }
    let ty = self_type.test_expect("assertion is missing a self binding")?;
    let mut value_errors = TestSlot::<ValueError>::default();
    let payload_value = value_errors
        .run(|out| LocalValueFactory::structure_view(payload, ValueSource::RustValue, None, out))
        .test_value()?;
    let limits = ValueLimits::default();
    let regex_limits = RegexLimits::default();
    let evaluator = Evaluator::new(&schema, &limits, &regex_limits, None);
    let mut eval_errors = TestSlot::<EvalError>::default();
    let bindings = Bindings {
        self_value: Some(EvalValue::config(payload_value.root(), ty, false)),
        root: Some(&root),
        ..Bindings::default()
    };
    test_assert!(matches!(
        evaluate_bool(&mut eval_errors, &evaluator, expression, bindings),
        TestOpeoOutcome::Value(true)
    ))?;
    let variant =
        ParserTestSupport::required(&mut schema_errors, |out| enum_ty.variant_named("Unit", out))?;
    let assertions = ParserTestSupport::access(&mut schema_errors, |out| variant.assertions(out))?;
    let assertion = ParserTestSupport::access(&mut schema_errors, |out| assertions.get(0, out))?
        .test_expect("schema assertion list is empty")?;
    test_assert!(matches!(
        evaluate_bool(
            &mut eval_errors,
            &evaluator,
            assertion.expression(),
            Bindings {
                root: Some(&root),
                ..Bindings::default()
            },
        ),
        TestOpeoOutcome::Value(true)
    ))?;

    Ok(())
}

#[test]
fn numeric_aliases_use_underlying_width_without_reapplying_constraints_to_results() -> TestResult<()>
{
    let schema = compile(
        "type A = i8 @min(0); type B = A @max(127); entry struct Root { a: B, b: i8, values: [B] } @assert(self.a + self.b == 3 && self.a - 2 == -1 && self.values.sum() == 127, \"rule\")",
    )?;
    let root = parse(&schema, "{a: 1, b: 2, values: [126, 1]}")?;
    test_assert!(matches!(
        evaluate_first(&schema, &root),
        TestOpeoOutcome::Value(true)
    ))?;
    let root = parse(&schema, "{a: 127, b: 1, values: []}")?;
    test_eq!(
        evaluate_first(&schema, &root),
        TestOpeoOutcome::Error(EvalFailure::Arithmetic)
    )?;

    Ok(())
}

#[test]
fn self_and_root_bindings_can_refer_to_different_instances() -> TestResult<()> {
    let schema = compile(
        "struct Child { n: u8 } @assert(self.n < root.limit, \"rule\") entry struct Root { limit: u8, child: Child }",
    )?;
    let root = parse(&schema, "{limit: 5, child: {n: 3}}")?;
    let mut schema_errors = TestSlot::<SchemaAccessError>::default();
    let field = ParserTestSupport::required(&mut schema_errors, |out| {
        schema.entry().field_named("child", out)
    })?;
    let child = value_field(&root, "child")?;
    let limits = ValueLimits::default();
    let regex_limits = RegexLimits::default();
    let evaluator = Evaluator::new(&schema, &limits, &regex_limits, None);
    let info = ParserTestSupport::access(&mut schema_errors, |out| field.ty(out))?;
    let assertions = ParserTestSupport::access(&mut schema_errors, |out| info.assertions(out))?;
    let assertion = ParserTestSupport::access(&mut schema_errors, |out| assertions.get(0, out))?
        .test_expect("schema assertion list is empty")?;
    let bindings = Bindings {
        self_value: Some(EvalValue::config(child, info, false)),
        root: Some(&root),
        ..Bindings::default()
    };
    let mut eval_errors = TestSlot::<EvalError>::default();
    test_assert!(matches!(
        evaluate_bool(
            &mut eval_errors,
            &evaluator,
            assertion.expression(),
            bindings
        ),
        TestOpeoOutcome::Value(true)
    ))?;

    Ok(())
}

#[test]
fn wide_sum_handles_large_cancellation_without_mutating_elements() -> TestResult<()> {
    let elements: Vec<_> = (0..1000)
        .map(|_| ValueFactory::signed(i128::MAX))
        .chain((0..999).map(|_| ValueFactory::signed(-i128::MAX)))
        .collect::<TestResult<_>>()?;
    let array = ValueFactory::try_array(elements).map_err(TestFailure::from_debug)?;
    let ValueKind::Array(values) = array.root().as_ref() else {
        return Err(TestFailure::new("expected an array"));
    };
    let mut errors = TestSlot::<EvalError>::default();
    test_assert!(matches!(
        errors.run(|out| Numeric::sum(values, BuiltinType::I128, None, out)),
        TestOpeoOutcome::Value(EvalKind::Signed(i128::MAX))
    ))?;
    let ValueKind::Array(mut values) = array.root().as_ref() else {
        return Err(TestFailure::new("expected an array"));
    };
    test_assert!(matches!(
        values.next().test_value()?.value().as_ref(),
        ValueKind::Signed(i128::MAX)
    ))?;

    Ok(())
}
