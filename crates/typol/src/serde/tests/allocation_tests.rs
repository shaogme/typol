use super::support::{Choice, Deep, FieldsEvent, Model, SerdeTestSupport as Support};
use crate::{
    TestAssertions, options::ConversionLimits, serde::Model as OpeoModel, test_support::TestValues,
};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{DeserializeSeed, Error as DeError, SeqAccess, Visitor},
    ser::{Error as SerError, SerializeTuple},
};
use std::{
    collections::BTreeMap,
    fmt,
    sync::atomic::{AtomicUsize, Ordering},
    thread,
};
use typol_core::{ExecutionLimits, ValueKind};
use typol_test_support::{
    TestExpect, TestExtract, TestResult, test_verify as assert, test_verify_eq as assert_eq,
};

#[test]
fn default_subtrees_and_structure_payloads_cleanup_on_allocation_failure() -> TestResult {
    let schema = Support::compile(
        "entry struct Root { value: [string] = [\"secret default\", \"other\"], optional: option<u8>, }",
    )?;
    let empty = FieldsEvent::new("Root", &[]);
    TestAssertions::assert_atomic_allocations(|| {
        let Some(call) = Support::allocation_session()? else {
            return Ok(());
        };
        drop(Support::allocation_codec_value(|out| {
            OpeoModel::encode(
                schema.entry(),
                &empty,
                &call,
                ConversionLimits::default(),
                out,
            )
        })?);
        Ok(())
    })?;
    let schema = Support::compile(
        "enum Choice { Unit, Single(string), Structure { first: u8, } } entry struct Root { value: Choice, }",
    )?;
    let value_ty = Support::ty(&schema)?;
    for value in [
        Choice::Unit,
        Choice::Single("secret payload".to_owned()),
        Choice::Structure { first: 7 },
    ] {
        TestAssertions::assert_atomic_allocations(|| {
            let Some(call) = Support::allocation_session()? else {
                return Ok(());
            };
            drop(Support::allocation_codec_value(|out| {
                OpeoModel::encode(value_ty, &value, &call, ConversionLimits::default(), out)
            })?);
            Ok(())
        })?;
    }
    Ok(())
}

#[test]
fn maps_scale_with_a_budgeted_duplicate_index() -> TestResult {
    let schema = Support::compile("entry struct Root { value: map<string, u8>, }")?;
    let mut steps = Vec::new();
    for length in [128, 256] {
        let values: BTreeMap<_, _> = (0..length)
            .map(|number| (format!("key{number:04}"), 7u8))
            .collect();
        let call = Support::session()?;
        let tree = Model::encode(
            Support::ty(&schema)?,
            &values,
            &call,
            ConversionLimits::default(),
        )
        .test_value()?;
        steps.push(call.execution().steps());
        let ValueKind::Map(entries) = tree.as_ref() else {
            return Err("encoded result should be a map".into());
        };
        assert_eq!(entries.len(), length);
    }
    assert!(
        *steps
            .get(1)
            .test_expect("should record the second budget group")?
            < *steps
                .first()
                .test_expect("should record the first budget group")?
                * 3
    );

    Ok(())
}

#[test]
fn live_capacity_text_and_default_node_limits_are_exact() -> TestResult {
    let schema = Support::compile("entry struct Root { value: string, }")?;
    let call = Support::new_session(ExecutionLimits::new(1000, 4096), 4, 0)?;
    let tree = Model::encode(
        Support::ty(&schema)?,
        &"four",
        &call,
        ConversionLimits::new(1, 0).test_value()?,
    )
    .test_value()?;
    assert_eq!(call.execution().constructed_text_bytes(), 4);
    assert_eq!(
        Support::code(
            Model::encode(
                Support::ty(&schema)?,
                &"x",
                &call,
                ConversionLimits::new(1, 0).test_value()?
            )
            .err()
            .test_value()?
        )?,
        1
    );
    drop(tree);
    let call = Support::session()?;
    let tree = Model::encode(
        Support::ty(&schema)?,
        &"four",
        &call,
        ConversionLimits::new(1, 0).test_value()?,
    )
    .test_value()?;
    let capacity = call.execution().peak_workspace_bytes();
    drop(tree);
    let baseline = call.execution().workspace_bytes();
    drop(call);
    let call = Support::new_session(ExecutionLimits::new(1000, capacity), 4, 0)?;
    drop(
        Model::encode(
            Support::ty(&schema)?,
            &"four",
            &call,
            ConversionLimits::new(1, 0).test_value()?,
        )
        .test_value()?,
    );
    assert_eq!(call.execution().workspace_bytes(), baseline);
    let call = Support::new_session(ExecutionLimits::new(1000, capacity - 1), 4, 0)?;
    assert_eq!(
        Support::code(
            Model::encode(
                Support::ty(&schema)?,
                &"four",
                &call,
                ConversionLimits::new(1, 0).test_value()?
            )
            .err()
            .test_value()?
        )?,
        1
    );
    let schema = Support::compile("entry struct Root { value: [u8] = [1, 2], }")?;
    let empty = FieldsEvent::new("Root", &[]);
    assert!(
        Model::encode(
            schema.entry(),
            &empty,
            &Support::session()?,
            ConversionLimits::new(4, 2).test_value()?
        )
        .is_ok()
    );
    assert_eq!(
        Support::code(
            Model::encode(
                schema.entry(),
                &empty,
                &Support::session()?,
                ConversionLimits::new(3, 2).test_value()?
            )
            .err()
            .test_value()?
        )?,
        1
    );
    assert_eq!(
        Support::code(
            Model::encode(
                schema.entry(),
                &empty,
                &Support::session()?,
                ConversionLimits::new(4, 1).test_value()?
            )
            .err()
            .test_value()?
        )?,
        1
    );

    Ok(())
}

struct DeepSeed(usize);

impl<'de> DeserializeSeed<'de> for DeepSeed {
    type Value = usize;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<usize, D::Error> {
        if self.0 == 0 {
            let value = u8::deserialize(deserializer)?;
            return Ok(value as usize);
        }
        struct DeepVisitor(usize);

        impl<'de> Visitor<'de> for DeepVisitor {
            type Value = usize;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("nested sequence")
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut access: A) -> Result<usize, A::Error> {
                let value = access
                    .next_element_seed(DeepSeed(self.0 - 1))?
                    .ok_or_else(|| DeError::custom("missing element"))?;
                Ok(value + 1)
            }
        }
        deserializer.deserialize_seq(DeepVisitor(self.0))
    }
}

struct DeepTarget(usize);

impl<'de> Deserialize<'de> for DeepTarget {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        DeepSeed(128).deserialize(deserializer).map(Self)
    }
}

#[test]
fn deep_deserialization_and_failure_cleanup_fit_a_small_stack() -> TestResult {
    let mut source = String::from("type A0 = u8;\n");
    for depth in 1..=128 {
        source.push_str(&format!("type A{depth} = [A{}];\n", depth - 1));
    }
    source.push_str("entry struct Root { value: A128, }");
    let schema = Support::compile(&source)?;
    let tree = Support::encode(&schema, &Deep::new(128))?.test_value()?;
    thread::Builder::new()
        .stack_size(512 * 1024)
        .spawn(move || {
            assert_eq!(
                Support::decode::<DeepTarget>(&schema, &tree)?
                    .test_value()?
                    .0,
                129
            );
            assert_eq!(
                Support::code(
                    Model::decode::<DeepTarget>(
                        Support::ty(&schema)?,
                        &tree,
                        &Support::session()?,
                        ConversionLimits::new(129, 127).test_value()?
                    )
                    .err()
                    .test_value()?
                )?,
                1
            );
            Ok(())
        })
        .test_value()?
        .join()
        .test_value()?
        .test_value()?;

    Ok(())
}

struct ShortTuple;

impl Serialize for ShortTuple {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut tuple = serializer.serialize_tuple(2)?;
        tuple.serialize_element(&1u8)?;
        tuple.end()
    }
}

#[test]
fn fixed_arrays_check_actual_event_count() -> TestResult {
    let schema = Support::compile("entry struct Root { value: [u8; 2], }")?;
    assert_eq!(
        Support::code(Support::encode(&schema, &ShortTuple)?.err().test_value()?)?,
        15
    );
    let short = TestValues::array_unmetered(vec![TestValues::unsigned(1)]);
    assert_eq!(
        Support::code(
            Support::decode::<[u8; 2]>(&schema, &short)?
                .err()
                .test_value()?
        )?,
        15
    );

    Ok(())
}

static FORMAT_CALLS: AtomicUsize = AtomicUsize::new(0);

struct PanicDisplay;

impl fmt::Display for PanicDisplay {
    fn fmt(&self, _: &mut fmt::Formatter<'_>) -> fmt::Result {
        FORMAT_CALLS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

struct PanicError;

impl Serialize for PanicError {
    fn serialize<S: Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
        Err(S::Error::custom(PanicDisplay))
    }
}

impl<'de> Deserialize<'de> for PanicError {
    fn deserialize<D: Deserializer<'de>>(_: D) -> Result<Self, D::Error> {
        Err(DeError::custom(PanicDisplay))
    }
}

#[test]
fn custom_error_formatters_are_never_called() -> TestResult {
    FORMAT_CALLS.store(0, Ordering::SeqCst);
    let schema = Support::compile("entry struct Root { value: u8, }")?;
    assert_eq!(
        Support::code(Support::encode(&schema, &PanicError)?.err().test_value()?)?,
        15
    );
    assert_eq!(
        Support::code(
            Support::decode::<PanicError>(&schema, &TestValues::unsigned(1))?
                .err()
                .test_value()?
        )?,
        15
    );

    assert_eq!(FORMAT_CALLS.load(Ordering::SeqCst), 0);

    Ok(())
}
