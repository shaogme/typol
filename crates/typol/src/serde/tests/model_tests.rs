use super::support::{Identifier, Model, NoConsume, SerdeTestSupport as Support};
use crate::{
    TestAssertions, options::ConversionLimits, serde::Model as OpeoModel, test_support::TestOpeo,
    test_support::TestValues,
};
use alloc::collections::BTreeMap;
use core::fmt;
use serde::{
    Deserialize, Deserializer,
    de::{SeqAccess, Visitor},
};
use typol_core::{AllocationError, ExecutionLimits};
use typol_test_support::{
    TestExtract, TestResult, test_verify as assert, test_verify_eq as assert_eq,
};

struct Partial;

impl<'de> Deserialize<'de> for Partial {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct PartialVisitor;

        impl<'de> Visitor<'de> for PartialVisitor {
            type Value = Partial;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("sequence")
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut access: A) -> Result<Partial, A::Error> {
                let _: Option<u8> = access.next_element()?;
                Ok(Partial)
            }
        }
        deserializer.deserialize_seq(PartialVisitor)
    }
}

#[test]
fn user_deserializers_must_consume_all_required_events() -> TestResult {
    let schema = Support::compile("entry struct Root { value: [u8], }")?;
    let value = TestValues::array_unmetered(vec![TestValues::unsigned(1), TestValues::unsigned(2)]);
    assert_eq!(
        Support::code(
            Support::decode::<Partial>(&schema, &value)?
                .err()
                .test_value()?
        )?,
        15
    );
    assert_eq!(
        Support::code(
            Support::decode::<NoConsume>(&schema, &value)?
                .err()
                .test_value()?
        )?,
        15
    );
    assert_eq!(
        Support::code(
            Support::decode::<Vec<NoConsume>>(&schema, &value)?
                .err()
                .test_value()?
        )?,
        15
    );
    let schema = Support::compile("entry struct Root { value: option<u8>, }")?;
    assert_eq!(
        Support::code(
            Support::decode::<Option<NoConsume>>(
                &schema,
                &TestOpeo::value::<AllocationError, _>(|_out| {
                    Ok(TestValues::some_unmetered(TestValues::unsigned(1)))
                })?
            )?
            .err()
            .test_value()?
        )?,
        15
    );

    Ok(())
}

#[test]
fn unsupported_model_access_does_not_guess_or_ignore() -> TestResult {
    let schema = Support::compile("entry struct Root { value: string, }")?;
    assert_eq!(
        Support::code(Support::encode(&schema, &'x')?.err().test_value()?)?,
        15
    );
    assert_eq!(
        Support::code(Support::encode(&schema, &())?.err().test_value()?)?,
        15
    );
    assert_eq!(
        Support::code(Support::encode(&schema, &(1u8, 2u8))?.err().test_value()?)?,
        15
    );
    let value = TestValues::string_unmetered("x".to_owned());
    assert_eq!(
        Support::code(
            Support::decode::<char>(&schema, &value)?
                .err()
                .test_value()?
        )?,
        15
    );
    assert_eq!(
        Support::code(Support::decode::<()>(&schema, &value)?.err().test_value()?)?,
        15
    );
    let schema = Support::compile("entry struct Root { value: u8, }")?;
    assert_eq!(
        Support::code(
            Support::decode::<Identifier>(&schema, &TestValues::unsigned(1))?
                .err()
                .test_value()?
        )?,
        15
    );

    Ok(())
}

#[test]
fn node_depth_and_shared_step_limits_are_exact() -> TestResult {
    let schema = Support::compile("entry struct Root { value: [u8], }")?;
    let call = Support::session()?;
    let value = [1u8, 2].to_vec();
    assert!(
        Model::encode(
            Support::ty(&schema)?,
            &value,
            &call,
            ConversionLimits::new(3, 1).test_value()?
        )
        .is_ok()
    );
    assert_eq!(
        Support::code(
            Model::encode(
                Support::ty(&schema)?,
                &value,
                &call,
                ConversionLimits::new(2, 1).test_value()?
            )
            .err()
            .test_value()?
        )?,
        1
    );
    assert_eq!(
        Support::code(
            Model::encode(
                Support::ty(&schema)?,
                &value,
                &call,
                ConversionLimits::new(3, 0).test_value()?
            )
            .err()
            .test_value()?
        )?,
        1
    );
    let call = Support::new_session(ExecutionLimits::new(18, 4096), 4096, 4096)?;
    assert!(
        Model::encode(
            Support::ty(&schema)?,
            &1u8,
            &call,
            ConversionLimits::default()
        )
        .is_err()
    );
    let schema = Support::compile("entry struct Root { value: u8, }")?;
    for index in 0..5 {
        if let Err(error) = Model::encode(
            Support::ty(&schema)?,
            &1u8,
            &call,
            ConversionLimits::default(),
        )
        .test_value()
        {
            return Err(format!(
                "execution-step budget exhausted on successful call {index}: {error:?}"
            )
            .into());
        }
    }
    assert_eq!(
        Support::code(
            Model::encode(
                Support::ty(&schema)?,
                &1u8,
                &call,
                ConversionLimits::default()
            )
            .err()
            .test_value()?
        )?,
        1
    );

    Ok(())
}

#[test]
fn every_library_allocation_can_fail_atomically() -> TestResult {
    let schema = Support::compile("entry struct Root { value: map<string, [option<string>]>, }")?;
    let values = BTreeMap::from([
        ("a".to_owned(), vec![Some("secret".to_owned()), None]),
        ("😀".to_owned(), vec![Some("x".to_owned())]),
    ]);
    let value_ty = Support::ty(&schema)?;
    TestAssertions::assert_atomic_allocations(|| {
        let Some(call) = Support::allocation_session()? else {
            return Ok(());
        };
        let value = Support::allocation_codec_value(|out| {
            OpeoModel::encode(value_ty, &values, &call, ConversionLimits::default(), out)
        })?;
        drop(value);
        Ok(())
    })?;
    let schema = Support::compile("entry struct Root { value: string, }")?;
    let value = TestValues::string_unmetered("secret".to_owned());
    let value_ty = Support::ty(&schema)?;
    TestAssertions::assert_atomic_allocations(|| {
        let Some(call) = Support::allocation_session()? else {
            return Ok(());
        };
        let result = Support::allocation_codec_value(|out| {
            OpeoModel::decode::<String>(value_ty, &value, &call, ConversionLimits::default(), out)
        })?;
        drop(result);
        Ok(())
    })?;
    Ok(())
}
