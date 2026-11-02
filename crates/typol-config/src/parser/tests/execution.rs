mod advanced;

use super::{ConfigLimits, ParserTestSupport, foundation::Provider};

use crate::diagnostics::ExecutionDiagnostic;
use alloc::{format, vec, vec::Vec};
use core::cmp::Ordering;
use opeo::{OResult, Out};
use typol_core::{ConfigValue, DiagnosticCode, ValueKind, ValueView};
use typol_policy::{RegexLimits, ValueLimits};
use typol_schema::{
    ResolverError, SchemaCompileErrors, SchemaCompileOptions, SchemaCompiler, SchemaResolveRequest,
    SchemaResolver, SchemaSource,
};
use typol_semantic::{
    ComparisonOptions, CompiledSchema, ConstraintData, EvalError, EvalFailure, ExpressionView,
    SchemaAccessError, SchemaBindings as Bindings, SchemaComparison, SchemaEvaluator as Evaluator,
    SchemaValue as EvalValue, TypeKind, ValueComparison,
};
use typol_test_support::*;

struct IntegerWidthCase {
    ty: &'static str,
    min: i128,
    max: i128,
}

struct EqualityCase {
    fields: &'static str,
    config: &'static str,
    expected: Ordering,
}

struct VariantCase {
    left: &'static str,
    right: &'static str,
    expected: bool,
}

struct NoImports;

impl SchemaResolver for NoImports {
    fn resolve<'a, 'slot, 'borrow>(
        &'a mut self,
        _request: SchemaResolveRequest<'_>,
        out: Out<'slot, 'borrow, ResolverError>,
    ) -> OResult<'slot, SchemaSource<'a>> {
        Err(out.fail(ResolverError::NotFound))
    }
}

fn compile(text: &str) -> TestResult<CompiledSchema> {
    let mut errors = TestSlot::<SchemaCompileErrors>::default();
    match errors.run(|mut out| {
        let compiler = SchemaCompiler::new(SchemaCompileOptions::default(), out.reborrow())?;
        compiler.compile(
            SchemaSource::from_str(b"test", text),
            &mut NoImports,
            None,
            out.reborrow(),
        )
    }) {
        TestOpeoOutcome::Value(schema) => Ok(schema),
        TestOpeoOutcome::Error(errors) => Err(TestFailure::new(format!(
            "test schema compilation failed for {text}: {:?}",
            errors
                .diagnostics()
                .iter()
                .map(|diagnostic| diagnostic.message())
                .collect::<Vec<_>>()
        ))),
    }
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

fn check(
    fields: &str,
    expression: &str,
    config: &str,
) -> TestResult<TestOpeoOutcome<bool, EvalFailure>> {
    let schema = compile(&format!(
        "entry struct Root {{ {fields} }} @assert({expression}, \"rule\")"
    ))?;
    let root = parse(&schema, config)?;
    let limits = ValueLimits::default();
    let regex_limits = RegexLimits::default();
    let evaluator = Evaluator::new(&schema, &limits, &regex_limits, None);
    let info = schema.entry();
    let bindings = Bindings {
        self_value: Some(EvalValue::config(root.root(), schema.entry(), false)),
        root: Some(&root),
        ..Bindings::default()
    };
    let mut schema_errors = TestSlot::<SchemaAccessError>::default();
    let assertions = ParserTestSupport::access(&mut schema_errors, |out| info.assertions(out))?;
    let assertion = ParserTestSupport::access(&mut schema_errors, |out| assertions.get(0, out))?
        .test_expect("test schema is missing an assertion")?;
    let mut eval_errors = TestSlot::<EvalError>::default();
    match eval_errors.run(|out| evaluator.evaluate(assertion.expression(), bindings, out)) {
        TestOpeoOutcome::Value(value) => match eval_errors.run(|out| value.boolean(out)) {
            TestOpeoOutcome::Value(value) => Ok(TestOpeoOutcome::Value(value)),
            TestOpeoOutcome::Error(error) => Ok(TestOpeoOutcome::Error(error.kind())),
        },
        TestOpeoOutcome::Error(error) => Ok(TestOpeoOutcome::Error(error.kind())),
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

#[test]
fn arithmetic_uses_each_integer_target_width() -> TestResult<()> {
    for ty in [
        "i8", "i16", "i32", "i64", "i128", "u8", "u16", "u32", "u64", "u128",
    ] {
        test_eq!(
            check(
                &format!("n: {ty}"),
                "(self.n + 2) * 3 - 1 == 14 && self.n / 2 == 1",
                "{ n: 3 }"
            )?,
            TestOpeoOutcome::Value(true)
        )?;
        test_eq!(
            check(&format!("n: {ty}"), "self.n / 0 == 1", "{ n: 3 }")?,
            TestOpeoOutcome::Error(EvalFailure::Arithmetic)
        )?;
    }
    for case in [
        IntegerWidthCase {
            ty: "i8",
            min: i8::MIN as i128,
            max: i8::MAX as i128,
        },
        IntegerWidthCase {
            ty: "i16",
            min: i16::MIN as i128,
            max: i16::MAX as i128,
        },
        IntegerWidthCase {
            ty: "i32",
            min: i32::MIN as i128,
            max: i32::MAX as i128,
        },
        IntegerWidthCase {
            ty: "i64",
            min: i64::MIN as i128,
            max: i64::MAX as i128,
        },
        IntegerWidthCase {
            ty: "i128",
            min: i128::MIN,
            max: i128::MAX,
        },
    ] {
        let IntegerWidthCase { ty, min, max } = case;
        let fields = format!("n: {ty}");
        test_eq!(
            check(
                &fields,
                &format!("self.n == {min}"),
                &format!("{{n: {min}}}")
            )?,
            TestOpeoOutcome::Value(true)
        )?;
        for expr in ["-self.n == 0", "self.n / -1 == 0", "self.n - 1 == 0"] {
            test_eq!(
                check(&fields, expr, &format!("{{n: {min}}}"))?,
                TestOpeoOutcome::Error(EvalFailure::Arithmetic)
            )?;
        }
        test_eq!(
            check(&fields, "self.n + 1 == 0", &format!("{{n: {max}}}"))?,
            TestOpeoOutcome::Error(EvalFailure::Arithmetic)
        )?;
        test_eq!(
            check(&fields, "self.n / 2 == -1", "{n: -3}")?,
            TestOpeoOutcome::Value(true)
        )?;
    }
    for (ty, max) in [
        ("u8", u8::MAX as u128),
        ("u16", u16::MAX as u128),
        ("u32", u32::MAX as u128),
        ("u64", u64::MAX as u128),
        ("u128", u128::MAX),
    ] {
        test_eq!(
            check(
                &format!("n: {ty}"),
                &format!("self.n == {max}"),
                &format!("{{n: {max}}}")
            )?,
            TestOpeoOutcome::Value(true)
        )?;
        test_eq!(
            check(
                &format!("n: {ty}"),
                "self.n + 1 == 0",
                &format!("{{n: {max}}}")
            )?,
            TestOpeoOutcome::Error(EvalFailure::Arithmetic)
        )?;
        test_eq!(
            check(&format!("n: {ty}"), "self.n - 1 == 0", "{n: 0}")?,
            TestOpeoOutcome::Error(EvalFailure::Arithmetic)
        )?;
    }

    Ok(())
}

#[test]
fn logic_and_array_methods_short_circuit() -> TestResult<()> {
    for (expr, expected) in [
        ("false && self.n / 0 > 1", false),
        ("true || self.n / 0 > 1", true),
    ] {
        test_eq!(
            check("n: u8", expr, "{ n: 1 }")?,
            TestOpeoOutcome::Value(expected)
        )?;
    }
    test_eq!(
        check(
            "values: [u8]",
            "self.values.any(|v| v == 0 || 1 / v > 0)",
            "{values: [0, 0]}"
        )?,
        TestOpeoOutcome::Value(true)
    )?;
    test_eq!(
        check(
            "values: [u8]",
            "self.values.all(|v| v > 0 && 1 / v > 0)",
            "{values: [0, 0]}"
        )?,
        TestOpeoOutcome::Value(false)
    )?;
    for expr in [
        "!self.values.any(|v| 1 / v > 0)",
        "self.values.all(|v| 1 / v > 0)",
    ] {
        test_eq!(
            check("values: [u8]", expr, "{values: []}")?,
            TestOpeoOutcome::Value(true)
        )?;
    }
    test_eq!(
        check(
            "values: [[u8]]",
            "self.values.all(|v| v.any(|v| v > 0) && v.len() == 2)",
            "{values: [[0, 1], [2, 0]]}"
        )?,
        TestOpeoOutcome::Value(true)
    )?;
    test_eq!(
        check(
            "values: [[u8]]",
            "self.values.all(|outer| outer.any(|inner| inner > 0 && outer.len() == 2))",
            "{values: [[0, 1], [2, 0]]}"
        )?,
        TestOpeoOutcome::Value(true)
    )?;

    Ok(())
}

#[test]
fn every_read_only_method_and_ordering() -> TestResult<()> {
    let fields = "a: [u8], m: map<string, u8>, o: option<u8>, s: string";
    test_eq!(
        check(
            fields,
            "self.a.len() == 2 && !self.a.is_empty() && self.m.len() == 1 && !self.m.is_empty() && self.o.is_some() && !self.o.is_none() && self.s.starts_with(\"é\")",
            "{a: [1, 2], m: {\"x\": 1}, o: 1, s: \"éø\"}"
        )?,
        TestOpeoOutcome::Value(true)
    )?;
    test_eq!(
        check(
            fields,
            "self.a.is_empty() && self.m.is_empty() && self.o.is_none() && !self.o.is_some() && self.o == none",
            "{a: [], m: {}, o: none, s: \"\"}"
        )?,
        TestOpeoOutcome::Value(true)
    )?;
    test_eq!(
        check(
            "n: i8, s: string",
            "self.n < 4 && self.n <= 3 && self.n > 2 && self.n >= 3 && self.n != 2 && !(self.n == 2) && self.s < \"ø\"",
            "{n: 3, s: \"é\"}"
        )?,
        TestOpeoOutcome::Value(true)
    )?;
    let schema = compile(
        "enum E { A, B(u8), C { n: u8 } } entry struct Root { e: E } @assert(self.e.is_variant(\"B\"), \"rule\")",
    )?;
    let root = parse(&schema, "{e: .B(3)}")?;
    test_assert!(matches!(
        evaluate_first(&schema, &root),
        TestOpeoOutcome::Value(true)
    ))?;

    Ok(())
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
        Ok(None) => return TestOpeoOutcome::Error(EvalFailure::Internal),
        Err(_) => return TestOpeoOutcome::Error(EvalFailure::Internal),
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
fn sum_checks_only_final_integer_range_and_float_each_step() -> TestResult<()> {
    test_eq!(
        check("v: [i8]", "self.v.sum() == 127", "{v: [127, 1, -1]}")?,
        TestOpeoOutcome::Value(true)
    )?;
    test_eq!(
        check(
            "v: [i128]",
            &format!("self.v.sum() == {}", i128::MAX),
            &format!("{{v: [{0}, {0}, -{0}]}}", i128::MAX)
        )?,
        TestOpeoOutcome::Value(true)
    )?;
    test_eq!(
        check(
            "v: [i128]",
            &format!("self.v.sum() == {}", i128::MIN),
            &format!("{{v: [{0}, {0}, {1}]}}", i128::MIN, i128::MAX)
        )?,
        TestOpeoOutcome::Error(EvalFailure::Arithmetic)
    )?;
    test_eq!(
        check(
            "v: [u128]",
            "self.v.sum() == 0",
            &format!("{{v: [{}, 1]}}", u128::MAX)
        )?,
        TestOpeoOutcome::Error(EvalFailure::Arithmetic)
    )?;
    test_eq!(
        check("v: [i8]", "self.v.sum() == 0", "{v: [127, 1]}")?,
        TestOpeoOutcome::Error(EvalFailure::Arithmetic)
    )?;
    for ty in ["i8", "u128", "f32", "f64"] {
        test_eq!(
            check(&format!("v: [{ty}]"), "self.v.sum() == 0", "{v: []}")?,
            TestOpeoOutcome::Value(true)
        )?;
    }
    test_eq!(
        check(
            "v: [f32]",
            "self.v.sum() == 0",
            "{v: [16777216.0, 1.0, -16777216.0]}"
        )?,
        TestOpeoOutcome::Value(true)
    )?;
    test_eq!(
        check(
            "v: [f32]",
            "self.v.sum() == 0",
            "{v: [340282346638528860000000000000000000000.0, 340282346638528860000000000000000000000.0, -340282346638528860000000000000000000000.0]}"
        )?,
        TestOpeoOutcome::Error(EvalFailure::Arithmetic)
    )?;
    test_eq!(
        check("n: f32", "(self.n + 1.0) - self.n == 0", "{n: 16777216.0}")?,
        TestOpeoOutcome::Value(true)
    )?;
    test_eq!(
        check(
            "n: f64",
            "(self.n + 1.0) - self.n == 1.0",
            "{n: 16777216.0}"
        )?,
        TestOpeoOutcome::Value(true)
    )?;
    for ty in ["f32", "f64"] {
        test_eq!(
            check(
                &format!("n: {ty}"),
                "self.n == -0.0 && -self.n == 0.0",
                "{n: 0.0}"
            )?,
            TestOpeoOutcome::Value(true)
        )?;
        test_eq!(
            check(&format!("n: {ty}"), "self.n / 0.0 == 0.0", "{n: 1.0}")?,
            TestOpeoOutcome::Error(EvalFailure::Arithmetic)
        )?;
    }

    Ok(())
}

#[test]
fn host_annotations_do_not_change_string_equality() -> TestResult<()> {
    for (fields, expected) in [
        ("a: string @host, b: string @host", false),
        ("a: string @host, b: string", false),
    ] {
        test_eq!(
            check(
                fields,
                "self.a == self.b",
                "{a: \"Example.COM.\", b: \"example.com\"}"
            )?,
            TestOpeoOutcome::Value(expected)
        )?;
    }
    test_eq!(
        check(
            "a: string @host, b: string @host",
            "self.a == self.b",
            "{a: \"::ffff:192.0.2.1\", b: \"0:0:0:0:0:ffff:c000:201\"}"
        )?,
        TestOpeoOutcome::Value(false)
    )?;
    test_eq!(
        check(
            "a: option<string> @host, b: option<string> @host",
            "self.a == self.b",
            "{a: \"Example.COM.\", b: \"example.com\"}"
        )?,
        TestOpeoOutcome::Value(false)
    )?;
    test_eq!(
        check(
            "a: option<string> @host, b: option<string>",
            "self.a == self.b",
            "{a: \"Example.COM.\", b: \"example.com\"}"
        )?,
        TestOpeoOutcome::Value(false)
    )?;
    test_eq!(
        check(
            "a: string @host, b: string @host",
            "self.a != self.b",
            "{a: \"Example.COM.\", b: \"example.com\"}"
        )?,
        TestOpeoOutcome::Value(true)
    )?;
    test_eq!(
        check(
            "a: string @host",
            "self.a == \"example.com\"",
            "{a: \"Example.COM\"}"
        )?,
        TestOpeoOutcome::Value(false)
    )?;
    let mut eval_errors = TestSlot::<EvalError>::default();
    for (left, right) in [
        ("Example.COM.", "example.com"),
        ("192.0.2.9.", "192.0.2.9"),
        ("2001:db8::1", "2001:0db8:0:0:0:0:0:1"),
    ] {
        test_assert!(matches!(
            eval_errors.run(|out| ValueComparison::host_equal(left, right, out)),
            TestOpeoOutcome::Value(true)
        ))?;
    }

    Ok(())
}

#[test]
fn recursive_equality_uses_map_key_sets_and_enum_payload_metadata() -> TestResult<()> {
    let mut schema_errors = TestSlot::<SchemaAccessError>::default();
    for case in [
        EqualityCase {
            fields: "a: [map<string, option<f32>>], b: [map<string, option<f32>>]",
            config: r#"{a: [{"x": 0.0, "y": none}], b: [{"y": none, "x": -0.0}]}"#,
            expected: Ordering::Equal,
        },
        EqualityCase {
            fields: "a: map<string, u8>, b: map<string, u8>",
            config: r#"{a: {"x": 1, "y": 2}, b: {"y": 1, "x": 1}}"#,
            expected: Ordering::Greater,
        },
    ] {
        let schema = compile(&format!("entry struct Root {{ {} }}", case.fields))?;
        let root = parse(&schema, case.config)?;
        let limits = ValueLimits::default();
        let regex_limits = RegexLimits::default();
        let evaluator = Evaluator::new(&schema, &limits, &regex_limits, None);
        let mut value = |name| -> TestResult<EvalValue<'_>> {
            let field = ParserTestSupport::required(&mut schema_errors, |out| {
                schema.entry().field_named(name, out)
            })?;
            let ty = ParserTestSupport::access(&mut schema_errors, |out| field.ty(out))?;
            Ok(EvalValue::config(value_field(&root, name)?, ty, false))
        };
        let left = value("a")?;
        let right = value("b")?;
        let mut check_errors = TestSlot::<EvalError>::default();
        test_assert!(
            check_errors
                .run(|out| evaluator.checked(left, false, out))
                .is_ok()
        )?;
        test_assert!(
            check_errors
                .run(|out| evaluator.checked(right, false, out))
                .is_ok()
        )?;
        test_eq!(
            compare(&evaluator, left, right, true, 0),
            TestOpeoOutcome::Value(case.expected)
        )?;
    }
    let schema = compile(
        "struct Child { host: string @host } enum E { A, B(string @host), C { host: string @host } } entry struct Root { a: E, b: E, x: Child, y: Child } @assert(self.a == self.b, \"rule\")",
    )?;
    for case in [
        VariantCase {
            left: ".A",
            right: ".A",
            expected: true,
        },
        VariantCase {
            left: ".A",
            right: ".B(\"a\")",
            expected: false,
        },
        VariantCase {
            left: ".B(\"Example.COM.\")",
            right: ".B(\"example.com\")",
            expected: false,
        },
        VariantCase {
            left: ".C {host: \"Example.COM.\"}",
            right: ".C {host: \"example.com\"}",
            expected: false,
        },
    ] {
        let root = parse(
            &schema,
            &format!(
                "{{a: {}, b: {}, x: {{host: \"Example.COM.\"}}, y: {{host: \"example.com\"}}}}",
                case.left, case.right
            ),
        )?;
        test_eq!(
            evaluate_first(&schema, &root),
            TestOpeoOutcome::Value(case.expected)
        )?;
    }

    Ok(())
}

#[test]
fn unknown_bindings_and_depth_have_distinct_codes() -> TestResult<()> {
    let schema = compile("entry struct Root { n: u8 } @assert(self.n > 0, \"rule\")")?;
    let limits = ValueLimits::default();
    let regex_limits = RegexLimits::default();
    let evaluator = Evaluator::new(&schema, &limits, &regex_limits, None);
    let info = schema.entry();
    let mut schema_errors = TestSlot::<SchemaAccessError>::default();
    let assertions = ParserTestSupport::access(&mut schema_errors, |out| info.assertions(out))?;
    let assertion = ParserTestSupport::access(&mut schema_errors, |out| assertions.get(0, out))?
        .test_expect("schema assertion list is empty")?;
    let expression = assertion.expression();
    let mut eval_errors = TestSlot::<EvalError>::default();
    let error = eval_errors
        .run(|out| evaluator.evaluate(expression, Bindings::default(), out))
        .test_error()?;
    test_assert!(error.is_internal())?;
    test_assert!(error.context().is_some())?;
    test_assert!(error.source_span().is_some())?;
    let root = parse(&schema, "{n: 1}")?;
    let bindings = Bindings {
        self_value: Some(EvalValue::config(root.root(), schema.entry(), false)),
        ..Bindings::default()
    };
    let mut limits = limits;
    limits.set_max_nesting_depth(1);
    let evaluator = Evaluator::new(&schema, &limits, &regex_limits, None);
    let error = eval_errors
        .run(|out| evaluator.evaluate(expression, bindings, out))
        .test_error()?;
    test_eq!(error.kind(), EvalFailure::Resource)?;
    test_assert!(error.source_span().is_some())?;
    test_eq!(
        ExecutionDiagnostic::code(EvalError::Internal),
        DiagnosticCode::INTERNAL_ERROR
    )?;
    test_eq!(
        ExecutionDiagnostic::code(EvalError::Resource),
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
    )?;
    test_eq!(
        ExecutionDiagnostic::code(EvalError::Arithmetic),
        DiagnosticCode::CONFIG_VALIDATION_ERROR
    )?;

    Ok(())
}

#[test]
fn comparison_depth_is_checked_independently_of_value_construction() -> TestResult<()> {
    let schema = compile("entry struct Root { v: [[[u8]]] }")?;
    let mut schema_errors = TestSlot::<SchemaAccessError>::default();
    let field = ParserTestSupport::required(&mut schema_errors, |out| {
        schema.entry().field_named("v", out)
    })?;
    let ty = ParserTestSupport::access(&mut schema_errors, |out| field.ty(out))?;
    let value = ValueFactory::try_array(vec![
        ValueFactory::try_array(vec![
            ValueFactory::try_array(vec![ValueFactory::unsigned(1)?])
                .map_err(TestFailure::from_debug)?,
        ])
        .map_err(TestFailure::from_debug)?,
    ])
    .map_err(TestFailure::from_debug)?;
    let mut limits = ValueLimits::default();
    let regex_limits = RegexLimits::default();
    limits.set_max_nesting_depth(2);
    let evaluator = Evaluator::new(&schema, &limits, &regex_limits, None);
    let value = EvalValue::config(value.root(), ty, false);
    test_eq!(
        compare(&evaluator, value, value, true, 0),
        TestOpeoOutcome::Error(EvalError::Resource)
    )?;
    limits.set_max_nesting_depth(4);
    test_eq!(
        compare(
            &Evaluator::new(&schema, &limits, &regex_limits, None),
            value,
            value,
            true,
            0
        ),
        TestOpeoOutcome::Value(Ordering::Equal)
    )?;

    Ok(())
}

#[test]
fn all_expressions_bind_item_key_and_value_without_implicit_conversion() -> TestResult<()> {
    let schema = compile(
        "type Host = string @host; entry struct Root { a: [Host] @all(item == item, \"rule\"), m: map<string, u8> @all(key.starts_with(\"ok\") && value > 0, \"rule\") }",
    )?;
    let limits = ValueLimits::default();
    let regex_limits = RegexLimits::default();
    let evaluator = Evaluator::new(&schema, &limits, &regex_limits, None);
    let mut schema_errors = TestSlot::<SchemaAccessError>::default();
    let mut iteration_errors = TestSlot::<SchemaAccessError>::default();
    let mut eval_errors = TestSlot::<EvalError>::default();
    for name in ["a", "m"] {
        let field = ParserTestSupport::required(&mut schema_errors, |out| {
            schema.entry().field_named(name, out)
        })?;
        let info = ParserTestSupport::access(&mut schema_errors, |out| field.ty(out))?;
        let constraint_list =
            ParserTestSupport::access(&mut schema_errors, |out| field.constraints(out))?;
        let constraints = iteration_errors
            .run(|mut out| {
                let mut constraints = Vec::new();
                for result in constraint_list.iter(out.reborrow()) {
                    let constraint = match result {
                        Ok(constraint) => constraint,
                        Err(failure) => return Err(failure),
                    };
                    constraints.push(constraint);
                }
                Ok(constraints)
            })
            .test_value()?;
        let mut found = None;
        for constraint in constraints {
            if let ConstraintData::All { expression, .. } =
                ParserTestSupport::access(&mut schema_errors, |out| constraint.data(out))?
            {
                found = Some(expression);
                break;
            }
        }
        let constraint = found.test_expect("field is missing the all constraint")?;
        let map_value = ValueFactory::unsigned(1)?;
        let bindings = match ParserTestSupport::access(&mut schema_errors, |out| info.kind(out))? {
            TypeKind::Array { element, .. } => {
                let value = ValueFactory::string("Example.COM.")?;
                let bindings = Bindings {
                    item: Some(EvalValue::config(
                        value.root(),
                        element,
                        eval_errors
                            .run(|out| evaluator.host(element, out))
                            .test_value()?,
                    )),
                    ..Bindings::default()
                };
                test_assert!(bindings.item.test_value()?.host())?;
                test_assert!(matches!(
                    evaluate_bool(&mut eval_errors, &evaluator, constraint, bindings),
                    TestOpeoOutcome::Value(true)
                ))?;
                continue;
            }
            TypeKind::Map { value } => Bindings {
                key: Some("ok-key"),
                value: Some(EvalValue::config(map_value.root(), value, false)),
                ..Bindings::default()
            },
            _ => {
                return Err(TestFailure::new(
                    "test field should be an array or map type",
                ));
            }
        };
        test_assert!(matches!(
            evaluate_bool(&mut eval_errors, &evaluator, constraint, bindings),
            TestOpeoOutcome::Value(true)
        ))?;
        let error = evaluate_bool(
            &mut eval_errors,
            &evaluator,
            constraint,
            Bindings::default(),
        )
        .test_error()?;
        test_assert!(error.is_internal())?;
        test_assert!(error.context().is_some())?;
    }

    Ok(())
}
