use crate::{
    BindingTestRoot as Root, BindingTestSupport, TestAssertions,
    binding_impl::TypedSchema,
    conversion::{Conversion, ConvertedValue, OrderedMap, Sourced},
    descriptor::Typol,
    diagnostics::CodecErrors,
    options::{BindOptions, CodecOptions, ConversionLimits, MAX_CONVERSION_DEPTH},
    session::SessionBudget,
    test_support::{TestOpeo, TestOpeoOutcome, TestValues},
};
use std::{collections::HashMap, string::String};
use typol_core::{BudgetError, ConfigValue, DiagnosticCode, ExecutionLimits, ValueSource};
use typol_test_support::{
    TestExtract, TestFailure, TestResult, test_verify as assert, test_verify_eq as assert_eq,
};

fn roundtrip<T: Typol>(dsl: &str, value: T) -> TestResult<T> {
    let schema = BindingTestSupport::compile(dsl)?;
    let binding = BindingTestSupport::bind::<T>(&schema, BindOptions::default())?;
    let session = BindingTestSupport::codec_session()?;
    let encoded = encode(&binding, &value, &session, ConversionLimits::default())?;
    decode(
        &binding,
        encoded.value(),
        &session,
        ConversionLimits::default(),
    )
}

fn encode<T: Typol>(
    binding: &TypedSchema<'_, T>,
    value: &T,
    session: &SessionBudget,
    limits: ConversionLimits,
) -> TestResult<ConvertedValue> {
    BindingTestSupport::codec_value(|out| Conversion::encode(binding, value, session, limits, out))
}

fn decode<T: Typol>(
    binding: &TypedSchema<'_, T>,
    value: &ConfigValue,
    session: &SessionBudget,
    limits: ConversionLimits,
) -> TestResult<T> {
    BindingTestSupport::codec_value(|out| Conversion::decode(binding, value, session, limits, out))
}

fn encode_error<T: Typol>(
    binding: &TypedSchema<'_, T>,
    value: &T,
    session: &SessionBudget,
    limits: ConversionLimits,
) -> TestResult<CodecErrors> {
    BindingTestSupport::codec_error(|out| Conversion::encode(binding, value, session, limits, out))
}

fn decode_error<T: Typol>(
    binding: &TypedSchema<'_, T>,
    value: &ConfigValue,
    session: &SessionBudget,
    limits: ConversionLimits,
) -> TestResult<CodecErrors> {
    BindingTestSupport::codec_error(|out| Conversion::decode(binding, value, session, limits, out))
}

fn session_with_text_limit(max_text_bytes: usize) -> TestResult<SessionBudget> {
    TestOpeo::value(|out| {
        SessionBudget::new(
            ExecutionLimits::new(16_777_216, 67_108_864),
            max_text_bytes,
            0,
            out,
        )
    })
}

fn allocation_codec<T>(outcome: TestOpeoOutcome<T, CodecErrors>) -> TestResult<Option<T>> {
    match outcome {
        TestOpeoOutcome::Value(value) => Ok(Some(value)),
        TestOpeoOutcome::Error(errors)
            if errors.diagnostics().last().is_some_and(|diagnostic| {
                diagnostic.code() == DiagnosticCode::ALLOCATION_FAILED
            }) =>
        {
            Ok(None)
        }
        TestOpeoOutcome::Error(errors) => Err(TestFailure::new(format!(
            "operation returned a non-allocation error: {errors:?}"
        ))),
    }
}

#[test]
fn conversions_share_budget_and_check_exact_node_depth_and_live_capacity_boundaries() -> TestResult
{
    let schema = BindingTestSupport::compile("entry struct Root { value: [string] }")?;
    let binding = BindingTestSupport::bind::<Root<Vec<String>>>(&schema, BindOptions::default())?;
    let value = Root::new(vec!["a".into(), "b".into()]);
    let session = BindingTestSupport::codec_session()?;
    let initial = session.execution().workspace_bytes();
    let encoded = encode(
        &binding,
        &value,
        &session,
        ConversionLimits::new(4, 2).test_value()?,
    )?;
    let decoded = decode(
        &binding,
        encoded.value(),
        &session,
        ConversionLimits::new(4, 2).test_value()?,
    )?;
    assert_eq!(decoded, value);
    let steps = session.execution().steps();
    let peak = session.execution().peak_workspace_bytes();
    drop(encoded);
    assert_eq!(session.execution().workspace_bytes(), initial);
    for limits in [
        ConversionLimits::new(3, 2).test_value()?,
        ConversionLimits::new(4, 1).test_value()?,
    ] {
        let session = BindingTestSupport::codec_session()?;
        assert_eq!(
            BindingTestSupport::code(encode_error(&binding, &value, &session, limits)?)?,
            1
        );
        assert_eq!(session.execution().workspace_bytes(), initial);
    }
    for (capacity, succeeds) in [(peak, true), (peak - 1, false)] {
        let session = TestOpeo::value(|out| {
            SessionBudget::new(
                ExecutionLimits::new(16_777_216, capacity),
                16_777_216,
                0,
                out,
            )
        })?;
        let limits = ConversionLimits::new(4, 2).test_value()?;
        let result = TestOpeo::outcome(|mut out| {
            let encoded = Conversion::encode(&binding, &value, &session, limits, out.reborrow())?;
            Conversion::decode(&binding, encoded.value(), &session, limits, out.reborrow())
        });
        match (succeeds, result) {
            (true, TestOpeoOutcome::Value(decoded)) => assert_eq!(decoded, value),
            (false, TestOpeoOutcome::Error(errors)) => {
                assert_eq!(BindingTestSupport::code(errors)?, 1)
            }
            (true, TestOpeoOutcome::Error(errors)) => {
                return Err(TestFailure::new(format!(
                    "success boundary failed unexpectedly: {errors:?}"
                )));
            }
            (false, TestOpeoOutcome::Value(_)) => {
                return Err(TestFailure::new("resource boundary succeeded unexpectedly"));
            }
        }
        assert_eq!(session.execution().workspace_bytes(), initial);
    }
    let session = TestOpeo::value(|out| {
        SessionBudget::new(ExecutionLimits::new(steps, 67_108_864), 16_777_216, 0, out)
    })?;
    let encoded = encode(
        &binding,
        &value,
        &session,
        ConversionLimits::new(4, 2).test_value()?,
    )?;
    decode(
        &binding,
        encoded.value(),
        &session,
        ConversionLimits::new(4, 2).test_value()?,
    )?;
    assert_eq!(session.execution().steps(), steps);
    assert_eq!(
        BindingTestSupport::code(decode_error(
            &binding,
            encoded.value(),
            &session,
            ConversionLimits::new(4, 2).test_value()?,
        )?)?,
        1
    );
    Ok(())
}

#[test]
fn decoded_strings_preserve_unicode_and_exact_text_budget() -> TestResult {
    let schema = BindingTestSupport::compile("entry struct Root { value: string }")?;
    let binding = BindingTestSupport::bind::<Root<String>>(&schema, BindOptions::default())?;
    let encoding_session = BindingTestSupport::codec_session()?;
    let expected = String::from("\0é");
    let encoded = encode(
        &binding,
        &Root::new(expected.clone()),
        &encoding_session,
        ConversionLimits::default(),
    )?;
    let exact_session = session_with_text_limit(expected.len())?;
    let decoded = decode(
        &binding,
        encoded.value(),
        &exact_session,
        ConversionLimits::default(),
    )?;
    assert_eq!(decoded.into_value(), expected);
    assert_eq!(exact_session.execution().constructed_text_bytes(), 3);

    let empty = encode(
        &binding,
        &Root::new(String::new()),
        &encoding_session,
        ConversionLimits::default(),
    )?;
    let empty_session = session_with_text_limit(0)?;
    assert_eq!(
        decode(
            &binding,
            empty.value(),
            &empty_session,
            ConversionLimits::default(),
        )?
        .into_value(),
        ""
    );
    assert_eq!(empty_session.execution().constructed_text_bytes(), 0);
    assert_eq!(
        exact_session.execution().steps() - empty_session.execution().steps(),
        6
    );

    let short_session = session_with_text_limit(2)?;
    let errors = decode_error(
        &binding,
        encoded.value(),
        &short_session,
        ConversionLimits::default(),
    )?;
    assert_eq!(
        errors
            .diagnostics()
            .last()
            .map(|diagnostic| diagnostic.code()),
        Some(DiagnosticCode::RESOURCE_LIMIT_EXCEEDED)
    );
    assert_eq!(short_session.execution().constructed_text_bytes(), 0);
    Ok(())
}

#[test]
fn binding_tree_and_hashmap_target_allocations_are_recoverable_and_leak_free() -> TestResult {
    let schema =
        BindingTestSupport::compile("entry struct Root { value: map<string, [string; 2]> }")?;
    let value = Root::new(HashMap::from([
        ("a".into(), ["alpha".into(), "beta".into()]),
        ("b".into(), ["gamma".into(), "delta".into()]),
    ]));
    TestAssertions::assert_atomic_allocations(|| {
        let binding = match allocation_codec(TestOpeo::outcome(|out| {
            TypedSchema::<Root<HashMap<String, [String; 2]>>>::bind(
                &schema,
                BindOptions::default(),
                out,
            )
        }))? {
            Some(binding) => binding,
            None => return Ok(()),
        };
        let session =
            match TestOpeo::outcome(|out| SessionBudget::codec(CodecOptions::default(), out)) {
                TestOpeoOutcome::Value(session) => session,
                TestOpeoOutcome::Error(BudgetError::Allocation) => return Ok(()),
                TestOpeoOutcome::Error(error) => {
                    return Err(TestFailure::new(format!(
                        "failed to initialize allocation-test budget: {error:?}"
                    )));
                }
            };
        let converted = match allocation_codec(TestOpeo::outcome(|out| {
            Conversion::encode(&binding, &value, &session, ConversionLimits::default(), out)
        }))? {
            Some(converted) => converted,
            None => return Ok(()),
        };
        let decoded = match allocation_codec(TestOpeo::outcome(|out| {
            Conversion::decode(
                &binding,
                converted.value(),
                &session,
                ConversionLimits::default(),
                out,
            )
        }))? {
            Some(decoded) => decoded,
            None => return Ok(()),
        };
        if decoded != value {
            return Err(TestFailure::new(
                "round-trip conversion result differs from the input value",
            ));
        }
        Ok(())
    })?;

    let ordered_schema =
        BindingTestSupport::compile("entry struct Root { value: map<string, u8> }")?;
    let mut ordered = OrderedMap::new();
    assert!(ordered.try_insert("a".into(), 1).is_ok());
    assert!(ordered.try_insert("b".into(), 2).is_ok());
    let ordered_value = Root::new(ordered);
    TestAssertions::assert_atomic_allocations(|| {
        let binding = match allocation_codec(TestOpeo::outcome(|out| {
            TypedSchema::<Root<OrderedMap<u8>>>::bind(&ordered_schema, BindOptions::default(), out)
        }))? {
            Some(binding) => binding,
            None => return Ok(()),
        };
        let session =
            match TestOpeo::outcome(|out| SessionBudget::codec(CodecOptions::default(), out)) {
                TestOpeoOutcome::Value(session) => session,
                TestOpeoOutcome::Error(BudgetError::Allocation) => return Ok(()),
                TestOpeoOutcome::Error(error) => {
                    return Err(TestFailure::new(format!(
                        "failed to initialize allocation-test budget: {error:?}"
                    )));
                }
            };
        let converted = match allocation_codec(TestOpeo::outcome(|out| {
            Conversion::encode(
                &binding,
                &ordered_value,
                &session,
                ConversionLimits::default(),
                out,
            )
        }))? {
            Some(converted) => converted,
            None => return Ok(()),
        };
        let decoded = match allocation_codec(TestOpeo::outcome(|out| {
            Conversion::decode(
                &binding,
                converted.value(),
                &session,
                ConversionLimits::default(),
                out,
            )
        }))? {
            Some(decoded) => decoded,
            None => return Ok(()),
        };
        if decoded != ordered_value {
            return Err(TestFailure::new(
                "ordered-map conversion result differs from the input value",
            ));
        }
        Ok(())
    })?;
    Ok(())
}

#[test]
fn output_and_owned_targets_remain_valid_after_session_and_schema_drop() -> TestResult {
    let value: Root<[String; 2]> = roundtrip(
        "entry struct Root { value: [string; 2] }",
        Root::new(["alpha".into(), "beta".into()]),
    )?;
    assert_eq!(value.into_value(), ["alpha", "beta"]);
    let converted = {
        let schema = BindingTestSupport::compile("entry struct Root { value: string @secret }")?;
        let binding = BindingTestSupport::bind::<Root<String>>(&schema, BindOptions::default())?;
        let session = BindingTestSupport::codec_session()?;
        encode(
            &binding,
            &Root::new("secret".into()),
            &session,
            ConversionLimits::default(),
        )?
    };
    assert!(!format!("{:?}", converted.value()).contains("secret"));
    assert_eq!(converted.value().source(), ValueSource::RustValue);
    Ok(())
}

#[derive(crate::Typol)]
#[typol(crate = "crate", rename = "SourcedRoot")]
struct SourcedRoot {
    value: Sourced<String>,
}

#[test]
fn sourced_values_obey_conversion_node_limits() -> TestResult {
    let schema = BindingTestSupport::compile("entry struct SourcedRoot { value: string }")?;
    let binding = TestOpeo::value(|out| {
        TypedSchema::<SourcedRoot>::bind(&schema, BindOptions::default(), out)
    })?;
    let value = TestOpeo::value(|out| {
        TestValues::try_structure(
            vec![TestValues::field(
                String::from("value"),
                TestValues::string_unmetered(String::from("sourced")),
            )],
            out,
        )
    })?;
    let session = BindingTestSupport::codec_session()?;
    let limits = ConversionLimits::new(2, MAX_CONVERSION_DEPTH).test_value()?;
    let errors = decode_error(&binding, &value, &session, limits)?;
    assert_eq!(
        errors
            .diagnostics()
            .last()
            .map(|diagnostic| diagnostic.code()),
        Some(DiagnosticCode::RESOURCE_LIMIT_EXCEEDED)
    );
    Ok(())
}

#[test]
fn sourced_value_allocations_remain_recoverable_and_leak_free() -> TestResult {
    let schema = BindingTestSupport::compile("entry struct SourcedRoot { value: string }")?;
    let binding = TestOpeo::value(|out| {
        TypedSchema::<SourcedRoot>::bind(&schema, BindOptions::default(), out)
    })?;
    let value = TestOpeo::value(|out| {
        TestValues::try_structure(
            vec![TestValues::field(
                String::from("value"),
                TestValues::string_unmetered(String::from("sourced")),
            )],
            out,
        )
    })?;
    let session = BindingTestSupport::codec_session()?;
    TestAssertions::assert_atomic_allocations(|| {
        let decoded = allocation_codec(TestOpeo::outcome(|out| {
            Conversion::decode(&binding, &value, &session, ConversionLimits::default(), out)
        }))?;
        if let Some(decoded) = decoded {
            assert_eq!(decoded.value.value(), "sourced");
            assert_eq!(decoded.value.source(), ValueSource::RustValue);
        }
        Ok(())
    })?;
    Ok(())
}
