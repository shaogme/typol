mod pretty;

use crate::{
    BindingTestSupport, TestAssertions,
    codec::ValidatedValue,
    diagnostics::CodecErrors,
    session::SessionBudget,
    test_support::{TestOpeo, TestOpeoOutcome},
};
use opeo::{OResult, Out};
use std::{cell::RefCell, thread};
use typol_config::{
    ConfigParseErrors, ConfigParseOptions, ConfigParseRequest, ConfigParser, Environment,
    EnvironmentError, EnvironmentReadContext,
};
use typol_core::{
    BudgetError, ConfigValue, DiagnosticCode, ExecutionBudget, ExecutionLimits, SliceSource,
    ValueKind,
};
use typol_semantic::CompiledSchema;
use typol_test_support::{
    TestExpect, TestExtract, TestFailure, TestResult, test_verify as assert,
    test_verify_eq as assert_eq,
};

struct IntegerRange {
    ty: &'static str,
    minimum: i128,
    maximum: i128,
}

struct EncodingLimitsCase {
    steps: usize,
    capacity: usize,
    text: usize,
    succeeds: bool,
}

struct FloatEncodingCase {
    ty: &'static str,
    input: &'static str,
    expected: &'static str,
}

struct Provider(Vec<u8>);

impl Environment for Provider {
    fn read<'slot, 'borrow, 'env>(
        &'env mut self,
        _: &str,
        _: EnvironmentReadContext<'_>,
        _: Out<'slot, 'borrow, EnvironmentError>,
    ) -> OResult<'slot, Option<&'env [u8]>> {
        Ok(Some(&self.0))
    }
}

fn request<'a>(input: &'a mut SliceSource<'_>) -> ConfigParseRequest<'a> {
    ConfigParseRequest::new(input, "encoding test", ConfigParseOptions::default())
}

fn session(output: usize) -> TestResult<SessionBudget> {
    TestOpeo::value(|out| {
        SessionBudget::new(
            ExecutionLimits::new(100_000_000, 67_108_864),
            100_000_000,
            output,
            out,
        )
    })
}

fn session_with_limits(
    limits: ExecutionLimits,
    text_limit: usize,
    output_limit: usize,
) -> TestResult<SessionBudget> {
    TestOpeo::value(|out| SessionBudget::new(limits, text_limit, output_limit, out))
}

fn cloned_budget(budget: &ExecutionBudget) -> TestResult<ExecutionBudget> {
    budget_value(|out| budget.try_clone(out))
}

fn consume_budget(budget: &ExecutionBudget, steps: usize) -> TestResult {
    budget_value(|out| budget.consume(steps, out))
}

fn parsed<'schema, 'call>(
    schema: &'schema CompiledSchema,
    request: ConfigParseRequest<'_>,
    environment: &mut dyn Environment,
    session: &'call mut SessionBudget,
) -> TestResult<ValidatedValue<'schema, 'call>> {
    TestOpeo::value(|out| ValidatedValue::parse(schema, request, environment, session, out))
}

fn parse_error(
    schema: &CompiledSchema,
    request: ConfigParseRequest<'_>,
    environment: &mut dyn Environment,
    session: &mut SessionBudget,
) -> TestResult<ConfigParseErrors> {
    TestOpeo::error(|out| ValidatedValue::parse(schema, request, environment, session, out))
}

fn parsed_config(
    schema: &CompiledSchema,
    request: ConfigParseRequest<'_>,
    environment: &mut dyn Environment,
) -> TestResult<ConfigValue> {
    TestOpeo::value(|out| ConfigParser::new(schema).parse(request, environment, out))
        .map(|value| value.into_config_value())
}

fn encoded(validated: &mut ValidatedValue<'_, '_>) -> TestResult<String> {
    let mut output = String::new();
    TestOpeo::value(|out| validated.encode(&mut output, false, out))?;
    Ok(output)
}

fn encode_error(validated: &mut ValidatedValue<'_, '_>) -> TestResult<CodecErrors> {
    let mut output = String::new();
    TestOpeo::error(|out| validated.encode(&mut output, false, out))
}

fn encode_outcome(validated: &mut ValidatedValue<'_, '_>) -> TestOpeoOutcome<String, CodecErrors> {
    encode_outcome_with_pretty(validated, false)
}

fn encode_outcome_with_pretty(
    validated: &mut ValidatedValue<'_, '_>,
    pretty: bool,
) -> TestOpeoOutcome<String, CodecErrors> {
    let mut output = String::new();
    match TestOpeo::outcome(|out| validated.encode(&mut output, pretty, out)) {
        TestOpeoOutcome::Value(()) => TestOpeoOutcome::Value(output),
        TestOpeoOutcome::Error(error) => TestOpeoOutcome::Error(error),
    }
}

fn allocation_encode(outcome: TestOpeoOutcome<String, CodecErrors>) -> TestResult {
    match outcome {
        TestOpeoOutcome::Value(value) => {
            drop(value);
            Ok(())
        }
        TestOpeoOutcome::Error(errors)
            if errors.diagnostics().last().is_some_and(|diagnostic| {
                diagnostic.code() == DiagnosticCode::ALLOCATION_FAILED
            }) =>
        {
            Ok(())
        }
        TestOpeoOutcome::Error(errors) => Err(TestFailure::new(format!(
            "allocation test encountered an unexpected error: {errors:?}"
        ))),
    }
}

fn budget_value<T>(
    operation: impl for<'slot> FnOnce(Out<'slot, 'slot, BudgetError>) -> OResult<'slot, T>,
) -> TestResult<T> {
    TestOpeo::value(operation)
}

fn field(value: &ConfigValue) -> TestResult<ValueKind<'_>> {
    let ValueKind::Structure(mut fields) = value.as_ref() else {
        return Err("root should be an object".into());
    };
    Ok(fields
        .next()
        .test_expect("root object should contain one field")?
        .value()
        .as_ref())
}

#[test]
fn ordering_defaults_none_escapes_and_variants_round_trip() -> TestResult {
    let schema = BindingTestSupport::compile(
        r#"
        enum Choice { Unit, Single(string), Structure { first: u8 = 7, last: option<string>, } }
        entry struct Root {
            first: string @secret @env("X"),
            mapping: map<string, string>,
            array: [Choice],
            optional: option<u8>,
            defaulted: u8 = 9,
            empty: [u8; 0],
        }
    "#,
    )?;
    let input = r#"{empty: [], array: [.Unit, .Single("inner"), .Structure {}], mapping: {"😀": "astral", "ö": "han", "a": "ascii", "é": "accent"}, first: env("X")}"#;
    let mut provider = Provider("secret\"\\\n\r\t".as_bytes().to_vec());
    let mut call = session(4096)?;
    let mut validated = parsed(
        &schema,
        request(&mut SliceSource::new((input).as_bytes())),
        &mut provider,
        &mut call,
    )?;
    let before = format!("{:?}", validated.value());
    assert!(!format!("{validated:?}").contains("secret"));
    let output = encoded(&mut validated)?;
    assert_eq!(
        output,
        r#"{first: "secret\"\\\n\r\t", mapping: {"a": "ascii", "é": "accent", "ö": "han", "😀": "astral"}, array: [.Unit, .Single("inner"), .Structure {first: 7, last: none}], optional: none, defaulted: 9, empty: []}"#
    );
    assert_eq!(before, format!("{:?}", validated.value()));
    assert_eq!(output, encoded(&mut validated)?);
    let reparsed = parsed_config(
        &schema,
        request(&mut SliceSource::new(output.as_bytes())),
        &mut Provider(Vec::new()),
    )?;
    let mut second = session(4096)?;
    let mut verified = parsed(
        &schema,
        request(&mut SliceSource::new(output.as_bytes())),
        &mut provider,
        &mut second,
    )?;
    assert_eq!(output, encoded(&mut verified)?);
    assert!(matches!(reparsed.as_ref(), ValueKind::Structure(_)));
    Ok(())
}

#[test]
fn map_source_order_does_not_change_text() -> TestResult {
    let schema = BindingTestSupport::compile("entry struct Root { value: map<string, u8>, }")?;
    let mut outputs = Vec::new();
    for input in [
        r#"{value: {"z": 1, "a": 2}}"#,
        r#"{value: {"a": 2, "z": 1}}"#,
    ] {
        let mut call = session(1024)?;
        let mut verified = parsed(
            &schema,
            request(&mut SliceSource::new((input).as_bytes())),
            &mut Provider(Vec::new()),
            &mut call,
        )?;
        let ValueKind::Map(mut entries) = field(verified.value())? else {
            return Err("expected a map".into());
        };
        let original = entries
            .next()
            .test_expect("Map should contain at least one entry")?
            .as_named()
            .test_expect("Map entries must be named")?
            .name()
            .to_owned();
        outputs.push(encoded(&mut verified)?);
        let ValueKind::Map(mut entries) = field(verified.value())? else {
            return Err("expected a map".into());
        };
        assert_eq!(
            entries
                .next()
                .test_expect("Map should contain at least one entry")?
                .as_named()
                .test_expect("Map entries must be named")?
                .name(),
            original
        );
    }
    assert_eq!(
        outputs
            .first()
            .test_expect("first encoding should be produced")?,
        outputs
            .get(1)
            .test_expect("second encoding should be produced")?
    );
    Ok(())
}

#[test]
fn every_integer_width_extreme_round_trips() -> TestResult {
    for case in [
        IntegerRange {
            ty: "i8",
            minimum: i8::MIN as i128,
            maximum: i8::MAX as i128,
        },
        IntegerRange {
            ty: "i16",
            minimum: i16::MIN as i128,
            maximum: i16::MAX as i128,
        },
        IntegerRange {
            ty: "i32",
            minimum: i32::MIN as i128,
            maximum: i32::MAX as i128,
        },
        IntegerRange {
            ty: "i64",
            minimum: i64::MIN as i128,
            maximum: i64::MAX as i128,
        },
        IntegerRange {
            ty: "i128",
            minimum: i128::MIN,
            maximum: i128::MAX,
        },
    ] {
        for value in [case.minimum, case.maximum, 0] {
            let schema = BindingTestSupport::compile(&format!(
                "entry struct Root {{ value: {}, }}",
                case.ty
            ))?;
            let input = format!("{{value: {value}}}");
            let mut call = session(1024)?;
            let mut verified = parsed(
                &schema,
                request(&mut SliceSource::new(input.as_bytes())),
                &mut Provider(Vec::new()),
                &mut call,
            )?;
            let output = encoded(&mut verified)?;
            let parsed = parsed_config(
                &schema,
                request(&mut SliceSource::new(output.as_bytes())),
                &mut Provider(Vec::new()),
            )?;
            assert!(matches!(field(&parsed)?, ValueKind::Signed(actual) if actual == value));
        }
    }
    for (ty, maximum) in [
        ("u8", u8::MAX as u128),
        ("u16", u16::MAX as u128),
        ("u32", u32::MAX as u128),
        ("u64", u64::MAX as u128),
        ("u128", u128::MAX),
    ] {
        for value in [0, maximum] {
            let schema =
                BindingTestSupport::compile(&format!("entry struct Root {{ value: {ty}, }}"))?;
            let input = format!("{{value: {value}}}");
            let mut call = session(1024)?;
            let mut verified = parsed(
                &schema,
                request(&mut SliceSource::new(input.as_bytes())),
                &mut Provider(Vec::new()),
                &mut call,
            )?;
            let output = encoded(&mut verified)?;
            let parsed = parsed_config(
                &schema,
                request(&mut SliceSource::new(output.as_bytes())),
                &mut Provider(Vec::new()),
            )?;
            assert!(matches!(field(&parsed)?, ValueKind::Unsigned(actual) if actual == value));
        }
    }
    Ok(())
}

#[test]
fn floats_preserve_bits_at_extremes_and_rounding_boundaries() -> TestResult {
    for ty in ["f32", "f64"] {
        let schema = BindingTestSupport::compile(&format!(
            "entry struct Root {{ value: {ty} @env(\"X\"), }}"
        ))?;
        let mut patterns = vec![0, 1, 2];
        if ty == "f32" {
            patterns.extend([
                0x8000_0000,
                0x007f_ffff,
                0x0080_0000,
                0x3f80_0000,
                0x3f80_0001,
                0x7f7f_ffff,
                0xff7f_ffff,
            ]);
        } else {
            patterns.extend([
                0x8000_0000_0000_0000,
                0x000f_ffff_ffff_ffff,
                0x0010_0000_0000_0000,
                0x3ff0_0000_0000_0000,
                0x3ff0_0000_0000_0001,
                0x7fef_ffff_ffff_ffff,
                0xffef_ffff_ffff_ffff,
            ]);
        }
        let mut state = 123456789_u64;
        for _ in 0..512 {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            patterns.push(if ty == "f32" {
                state & 0xffff_ffff
            } else {
                state
            });
        }
        for bits in patterns {
            let mut text = if ty == "f32" {
                let value = f32::from_bits(bits as u32);
                if !value.is_finite() {
                    continue;
                }
                value.to_string()
            } else {
                let value = f64::from_bits(bits);
                if !value.is_finite() {
                    continue;
                }
                value.to_string()
            };
            if !text.contains('.') {
                text.push_str(".0");
            }
            let mut call = session(4096)?;
            let mut verified = parsed(
                &schema,
                request(&mut SliceSource::new((r#"{value: env("X")}"#).as_bytes())),
                &mut Provider(text.into_bytes()),
                &mut call,
            )?;
            let output = encoded(&mut verified)?;
            let end = output.len().checked_sub(1).test_value()?;
            let literal = output
                .get(8..end)
                .test_expect("encoding should contain the numeric literal for the root field")?;
            assert!(!literal.contains(['e', 'E']));
            assert!(literal.contains('.'));
            let parsed = parsed_config(
                &schema,
                request(&mut SliceSource::new(output.as_bytes())),
                &mut Provider(Vec::new()),
            )?;
            match field(&parsed)? {
                ValueKind::Float32(value) => assert_eq!(value.to_bits(), bits as u32),
                ValueKind::Float64(value) => assert_eq!(value.to_bits(), bits),
                _ => return Err("expected a float".into()),
            }
        }
    }
    Ok(())
}

#[test]
fn unrepresentable_controls_fail_safely_after_validation() -> TestResult {
    let schema =
        BindingTestSupport::compile(r#"entry struct Root { value: string @secret @env("X"), }"#)?;
    for character in (0..=0x1f)
        .chain(0x7f..=0x9f)
        .filter(|value| ![9, 10, 13].contains(value))
    {
        let secret = format!("secret{}", char::from_u32(character).test_value()?);
        let mut call = session(4096)?;
        let mut verified = parsed(
            &schema,
            request(&mut SliceSource::new((r#"{value: env("X")}"#).as_bytes())),
            &mut Provider(secret.into_bytes()),
            &mut call,
        )?;
        let error = encode_error(&mut verified)?;
        assert!(!format!("{error:?} {error}").contains("secret"));
        assert_eq!(BindingTestSupport::code(error)?, 16);
    }
    Ok(())
}

#[test]
fn output_steps_capacity_and_text_limits_are_atomic() -> TestResult {
    let schema = BindingTestSupport::compile("entry struct Root { value: u8, }")?;
    for (limit, success) in [(10, true), (9, false), (0, false)] {
        let mut call = session(limit)?;
        let mut verified = parsed(
            &schema,
            request(&mut SliceSource::new(("{value: 1}").as_bytes())),
            &mut Provider(Vec::new()),
            &mut call,
        )?;
        match encode_outcome(&mut verified) {
            TestOpeoOutcome::Value(_) => assert!(success),
            TestOpeoOutcome::Error(error) => {
                assert!(!success);
                assert_eq!(BindingTestSupport::code(error)?, 1);
            }
        }
    }
    let mut baseline = session(10)?;
    let shared = cloned_budget(baseline.execution())?;
    let mut verified = parsed(
        &schema,
        request(&mut SliceSource::new(("{value: 1}").as_bytes())),
        &mut Provider(Vec::new()),
        &mut baseline,
    )?;
    encoded(&mut verified)?;
    let steps = shared.steps();
    let peak = shared.peak_workspace_bytes();
    drop(verified);
    for case in [
        EncodingLimitsCase {
            steps,
            capacity: peak,
            text: 10,
            succeeds: true,
        },
        EncodingLimitsCase {
            steps: steps - 1,
            capacity: peak,
            text: 10,
            succeeds: false,
        },
        EncodingLimitsCase {
            steps,
            capacity: peak - 1,
            text: 10,
            succeeds: false,
        },
        EncodingLimitsCase {
            steps,
            capacity: peak,
            text: 9,
            succeeds: false,
        },
    ] {
        let mut call = session_with_limits(
            ExecutionLimits::new(case.steps, case.capacity),
            case.text,
            10,
        )?;
        let shared = cloned_budget(call.execution())?;
        let initial = shared.workspace_bytes();
        let mut verified = parsed(
            &schema,
            request(&mut SliceSource::new(("{value: 1}").as_bytes())),
            &mut Provider(Vec::new()),
            &mut call,
        )?;
        match encode_outcome(&mut verified) {
            TestOpeoOutcome::Value(_) => assert!(case.succeeds),
            TestOpeoOutcome::Error(error) => {
                assert!(!case.succeeds);
                assert_eq!(BindingTestSupport::code(error)?, 1);
            }
        }
        assert_eq!(shared.workspace_bytes(), initial);
    }
    Ok(())
}

#[test]
fn allocations_fail_atomically_during_sorting_and_output() -> TestResult {
    let schema =
        BindingTestSupport::compile("entry struct Root { value: map<string, [string]>, }")?;
    let mut call = session(1_000_000)?;
    let verified = RefCell::new(parsed(
        &schema,
        request(&mut SliceSource::new(
            (r#"{value: {"z": ["long Unicode secret", "second"], "a": ["third"]}}"#).as_bytes(),
        )),
        &mut Provider(Vec::new()),
        &mut call,
    )?);
    for pretty in [false, true] {
        TestAssertions::assert_atomic_allocations(|| {
            allocation_encode(encode_outcome_with_pretty(
                &mut verified.borrow_mut(),
                pretty,
            ))
        })?;
    }
    Ok(())
}

#[test]
fn invalid_semantic_values_never_receive_credentials() -> TestResult {
    let schema = BindingTestSupport::compile(
        "entry struct Root { value: u8 @min(2), } @assert(self.value > 3, \"too small\")",
    )?;
    for input in ["{value: 1}", "{value: 3}", "{}"] {
        let mut call = session(1024)?;
        assert!(
            parse_error(
                &schema,
                request(&mut SliceSource::new((input).as_bytes())),
                &mut Provider(Vec::new()),
                &mut call,
            )
            .is_ok()
        );
    }
    Ok(())
}

#[test]
fn deep_tree_encoding_uses_a_small_stack() -> TestResult {
    let mut source = String::from("struct N0 { value: u8, } ");
    let mut input = String::from("{value: 1}");
    for depth in 1..=90 {
        source.push_str(&format!("struct N{depth} {{ value: N{}, }} ", depth - 1));
        input = format!("{{value: {input}}}");
    }
    source.push_str("entry struct Root { value: N90, }");
    input = format!("{{value: {input}}}");
    let schema = BindingTestSupport::compile(&source)?;
    let mut call = session(100_000)?;
    let mut verified = parsed(
        &schema,
        request(&mut SliceSource::new(input.as_bytes())),
        &mut Provider(Vec::new()),
        &mut call,
    )?;
    let joined = thread::scope(|scope| {
        thread::Builder::new()
            .stack_size(64 * 1024)
            .spawn_scoped(scope, move || {
                let encoded = encoded(&mut verified)?;
                assert_eq!(encoded, input);
                Ok::<(), TestFailure>(())
            })
            .map(|handle| handle.join())
    });
    match joined {
        Ok(Ok(result)) => result?,
        Ok(Err(_)) => return Err("encoding thread failed unexpectedly".into()),
        Err(error) => return Err(format!("failed to create encoding thread: {error}").into()),
    }
    Ok(())
}

#[test]
fn prior_session_work_is_not_reset_by_encoding() -> TestResult {
    let schema = BindingTestSupport::compile("entry struct Root { value: u8, }")?;
    let mut baseline = session(10)?;
    let budget = cloned_budget(baseline.execution())?;
    let mut verified = parsed(
        &schema,
        request(&mut SliceSource::new(("{value: 1}").as_bytes())),
        &mut Provider(Vec::new()),
        &mut baseline,
    )?;
    encoded(&mut verified)?;
    let steps = budget.steps();
    let mut call = session_with_limits(ExecutionLimits::new(steps, 4096), 100, 10)?;
    consume_budget(call.execution(), 1)?;
    let mut verified = parsed(
        &schema,
        request(&mut SliceSource::new(("{value: 1}").as_bytes())),
        &mut Provider(Vec::new()),
        &mut call,
    )?;
    assert_eq!(BindingTestSupport::code(encode_error(&mut verified)?)?, 1);
    Ok(())
}

#[test]
fn sorting_and_unicode_scanning_can_stop_midway_without_leaks() -> TestResult {
    let schema = BindingTestSupport::compile("entry struct Root { value: map<string, string>, }")?;
    let input = r#"{value: {"😀": "secret", "é": "é", "a": "a", "ö": "han"}}"#;
    let mut baseline = session(4096)?;
    let shared = cloned_budget(baseline.execution())?;
    let mut verified = parsed(
        &schema,
        request(&mut SliceSource::new((input).as_bytes())),
        &mut Provider(Vec::new()),
        &mut baseline,
    )?;
    encoded(&mut verified)?;
    let steps = shared.steps();
    for limit in 0..steps {
        let mut call = session_with_limits(ExecutionLimits::new(limit, 100_000), 4096, 4096)?;
        let shared = cloned_budget(call.execution())?;
        let initial = shared.workspace_bytes();
        let mut verified = parsed(
            &schema,
            request(&mut SliceSource::new((input).as_bytes())),
            &mut Provider(Vec::new()),
            &mut call,
        )?;
        assert_eq!(BindingTestSupport::code(encode_error(&mut verified)?)?, 1);
        assert_eq!(shared.workspace_bytes(), initial);
    }
    Ok(())
}

#[test]
fn empty_containers_some_and_quoted_map_keys_round_trip() -> TestResult {
    let schema = BindingTestSupport::compile(
        "struct Empty {} entry struct Root { empty: Empty, mapping: map<string, bool>, optional: option<[u8]>, }",
    )?;
    for input in [
        r#"{optional: [], mapping: {}, empty: {}}"#,
        r#"{empty: {}, mapping: {"": true}, optional: [1, 2]}"#,
        r#"{empty: {}, mapping: {"quote\"\\\n\r\t": true}, optional: [1, 2]}"#,
    ] {
        let output = {
            let mut call = session(1024)?;
            let mut verified = parsed(
                &schema,
                request(&mut SliceSource::new((input).as_bytes())),
                &mut Provider(Vec::new()),
                &mut call,
            )?;
            encoded(&mut verified)?
        };
        let mut call = session(1024)?;
        let mut verified = parsed(
            &schema,
            request(&mut SliceSource::new(output.as_bytes())),
            &mut Provider(Vec::new()),
            &mut call,
        )?;
        assert_eq!(output, encoded(&mut verified)?);
        assert!(output.starts_with("{empty: {}, mapping: {"));
    }
    Ok(())
}

#[test]
fn float_spelling_uses_original_width_and_expands_exponents() -> TestResult {
    for case in [
        FloatEncodingCase {
            ty: "f32",
            input: "0.1",
            expected: "0.1",
        },
        FloatEncodingCase {
            ty: "f32",
            input: "0.0",
            expected: "0.0",
        },
        FloatEncodingCase {
            ty: "f32",
            input: "-0.0",
            expected: "-0.0",
        },
        FloatEncodingCase {
            ty: "f64",
            input: "0.0",
            expected: "0.0",
        },
        FloatEncodingCase {
            ty: "f64",
            input: "-0.0",
            expected: "-0.0",
        },
        FloatEncodingCase {
            ty: "f64",
            input: "100000000000000000000.0",
            expected: "100000000000000000000.0",
        },
        FloatEncodingCase {
            ty: "f64",
            input: "0.0000001",
            expected: "0.0000001",
        },
    ] {
        let schema =
            BindingTestSupport::compile(&format!("entry struct Root {{ value: {}, }}", case.ty))?;
        let input = format!("{{value: {}}}", case.input);
        let expected = format!("{{value: {}}}", case.expected);
        let mut call = session(expected.len())?;
        let mut verified = parsed(
            &schema,
            request(&mut SliceSource::new(input.as_bytes())),
            &mut Provider(Vec::new()),
            &mut call,
        )?;
        assert_eq!(encoded(&mut verified)?, expected);
        let mut call = session(expected.len() - 1)?;
        let mut verified = parsed(
            &schema,
            request(&mut SliceSource::new(input.as_bytes())),
            &mut Provider(Vec::new()),
            &mut call,
        )?;
        assert_eq!(BindingTestSupport::code(encode_error(&mut verified)?)?, 1);
    }
    Ok(())
}
