mod allocation_tests;
mod budget_tests;
mod enum_tests;
mod model_tests;
mod pipeline;
mod rejection_tests;
mod support;

use crate::test_support::TestValues;

use alloc::collections::BTreeMap;
use typol_core::ValueKind;
use typol_test_support::{TestExpect, TestResult, test_verify_eq as assert_eq};

use self::support::SerdeTestSupport as Support;

#[test]
fn precise_scalars_round_trip_and_reject_other_widths() -> TestResult {
    macro_rules! round_trip {
        ($rust:ty => $schema:literal => $value:expr) => {{
            let schema = Support::compile(concat!("entry struct Root { value: ", $schema, ", }"))?;
            let value: $rust = $value;
            let tree = Support::encode(&schema, &value)?.test_value()?;
            assert_eq!(
                Support::decode::<$rust>(&schema, &tree)?.test_value()?,
                value
            );
        }};
    }
    round_trip!(i8 => "i8" => i8::MIN);
    round_trip!(i16 => "i16" => i16::MIN);
    round_trip!(i32 => "i32" => i32::MIN);
    round_trip!(i64 => "i64" => i64::MIN);
    round_trip!(i128 => "i128" => i128::MIN);
    round_trip!(u8 => "u8" => u8::MAX);
    round_trip!(u16 => "u16" => u16::MAX);
    round_trip!(u32 => "u32" => u32::MAX);
    round_trip!(u64 => "u64" => u64::MAX);
    round_trip!(u128 => "u128" => u128::MAX);
    round_trip!(bool => "bool" => true);
    round_trip!(String => "string" => "secret😀\n".to_owned());
    let schema = Support::compile("entry struct Root { value: u8, }")?;
    let errors = Support::encode(&schema, &1u16)?.test_error()?;
    let context = errors
        .diagnostic_context(0)
        .test_expect("schema shape mismatches should preserve context")?;
    assert_eq!(context.failure_point(), "serde-schema-shape-mismatch");
    assert_eq!(Support::code(errors)?, 15);
    let errors = Support::decode::<u16>(&schema, &TestValues::unsigned(1))?.test_error()?;
    let context = errors
        .diagnostic_context(0)
        .test_expect("schema shape mismatches should preserve context")?;
    assert_eq!(context.failure_point(), "serde-schema-shape-mismatch");
    assert_eq!(Support::code(errors)?, 15);
    assert_eq!(
        Support::code(Support::decode::<u8>(&schema, &TestValues::unsigned(256))?.test_error()?)?,
        15
    );
    assert_eq!(
        Support::code(Support::decode::<u8>(&schema, &TestValues::signed(1))?.test_error()?)?,
        15
    );

    Ok(())
}

#[test]
fn floating_bits_and_nonfinite_values_are_checked() -> TestResult {
    let schema = Support::compile("entry struct Root { value: f32, }")?;
    for value in [-0.0f32, f32::from_bits(1), f32::MAX] {
        assert_eq!(
            Support::decode::<f32>(&schema, &Support::encode(&schema, &value)?.test_value()?)?
                .test_value()?
                .to_bits(),
            value.to_bits()
        );
    }
    for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert_eq!(
            Support::code(Support::encode(&schema, &value)?.test_error()?)?,
            15
        );
        assert_eq!(
            Support::code(
                Support::decode::<f32>(&schema, &TestValues::float32(value))?.test_error()?
            )?,
            15
        );
    }
    assert_eq!(
        Support::code(Support::encode(&schema, &1.0f64)?.test_error()?)?,
        15
    );
    let schema = Support::compile("entry struct Root { value: f64, }")?;
    for value in [-0.0f64, f64::from_bits(1), f64::MAX] {
        assert_eq!(
            Support::decode::<f64>(&schema, &Support::encode(&schema, &value)?.test_value()?)?
                .test_value()?
                .to_bits(),
            value.to_bits()
        );
    }

    Ok(())
}

#[test]
fn options_sequences_fixed_arrays_and_maps_round_trip() -> TestResult {
    let schema = Support::compile("entry struct Root { value: option<[u8]>, }")?;
    for value in [None, Some(Vec::new()), Some(vec![0u8, 255])] {
        assert_eq!(
            Support::decode::<Option<Vec<u8>>>(
                &schema,
                &Support::encode(&schema, &value)?.test_value()?
            )?
            .test_value()?,
            value
        );
    }
    assert_eq!(
        Support::code(Support::encode(&schema, &vec![1u8])?.test_error()?)?,
        15
    );
    let schema = Support::compile("entry struct Root { value: [u8; 2], }")?;
    assert_eq!(
        Support::decode::<[u8; 2]>(
            &schema,
            &Support::encode(&schema, &[0u8, 255])?.test_value()?
        )?
        .test_value()?,
        [0, 255]
    );
    assert_eq!(
        Support::code(Support::encode(&schema, &vec![0u8, 255])?.test_error()?)?,
        15
    );
    assert_eq!(
        Support::code(Support::encode(&schema, &[0u8])?.test_error()?)?,
        15
    );
    assert_eq!(
        Support::code(
            Support::decode::<Vec<u8>>(&schema, &TestValues::array_unmetered(vec![]),)?
                .test_error()?,
        )?,
        15
    );
    let schema = Support::compile("entry struct Root { value: [u8; 0], }")?;
    assert_eq!(
        Support::decode::<[u8; 0]>(&schema, &Support::encode(&schema, &[0u8; 0])?.test_value()?)?
            .test_value()?,
        [] as [u8; 0]
    );
    let schema = Support::compile("entry struct Root { value: map<string, option<[u8; 2]>>, }")?;
    let values = BTreeMap::from([("😀".to_owned(), None), ("a".to_owned(), Some([2u8, 3]))]);
    let tree = Support::encode(&schema, &values)?.test_value()?;
    assert_eq!(
        Support::decode::<BTreeMap<String, Option<[u8; 2]>>>(&schema, &tree)?.test_value()?,
        values
    );
    let ValueKind::Map(mut entries) = tree.as_ref() else {
        return Err("encoded result should be a map".into());
    };
    assert_eq!(
        entries
            .next()
            .test_expect("map should include the first element")?
            .name()
            .test_expect("map key should be present")?,
        "a"
    );
    assert_eq!(
        Support::code(
            Support::encode(&schema, &BTreeMap::from([(1u8, None::<[u8; 2]>)]))?.test_error()?
        )?,
        15
    );

    Ok(())
}
