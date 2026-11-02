use super::support::{CustomError, Deep, Model, SerdeTestSupport as Support};
use crate::{AllocationScope, options::ConversionLimits, test_support::TestValues};
use serde::{
    Serialize, Serializer,
    ser::{SerializeSeq, SerializeStruct},
};
use std::thread;
use typol_core::ExecutionLimits;
use typol_test_support::{TestExtract, TestResult, test_verify_eq as assert_eq};

#[test]
fn nested_calls_check_depth_on_a_small_stack() -> TestResult {
    let mut source = String::from("type A0 = u8;\n");
    for depth in 1..=128 {
        source.push_str(&format!("type A{depth} = [A{}];\n", depth - 1));
    }
    source.push_str("entry struct Root { value: A128, }");
    let schema = Support::compile(&source)?;
    thread::Builder::new()
        .stack_size(512 * 1024)
        .spawn(move || {
            let call = Support::session()?;
            let value = Model::encode(
                Support::ty(&schema)?,
                &Deep::new(128),
                &call,
                ConversionLimits::new(129, 128).test_value()?,
            )
            .test_value()?;
            assert_eq!(
                Support::code(
                    Model::encode(
                        Support::ty(&schema)?,
                        &Deep::new(128),
                        &call,
                        ConversionLimits::new(129, 127).test_value()?
                    )
                    .err()
                    .test_value()?
                )?,
                1
            );
            drop(value);
            Ok(())
        })
        .test_value()?
        .join()
        .test_value()?
        .test_value()?;

    Ok(())
}

#[test]
fn scalar_serde_callbacks_need_no_unmetered_error_slot_pool() -> TestResult {
    let schema = Support::compile("entry struct Root { value: u8, }")?;
    let session = Support::session()?;
    let baseline = session.execution().workspace_bytes();
    drop(session);

    let call = Support::new_session(ExecutionLimits::new(100, baseline), 0, 0)?;
    let value = TestValues::unsigned(7);
    let ty = Support::ty(&schema)?;
    let limits = ConversionLimits::new(1, 0).test_value()?;
    let scope = AllocationScope::start(None, false);
    let decoded = Model::decode::<u8>(ty, &value, &call, limits).test_value()?;
    let allocations = scope.finish();
    assert_eq!(decoded, 7);
    assert_eq!(allocations.attempts(), 0);
    assert_eq!(call.execution().peak_workspace_bytes(), baseline);
    assert_eq!(call.execution().workspace_bytes(), baseline);
    Ok(())
}

struct Swallow;

impl Serialize for Swallow {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut fields = serializer.serialize_struct("Root", 1)?;
        let _ = fields.serialize_field("value", &CustomError);
        fields.end()
    }
}

struct SwallowSequence;

impl Serialize for SwallowSequence {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut sequence = serializer.serialize_seq(None)?;
        let _ = sequence.serialize_element(&1u8);
        sequence.end()
    }
}

#[test]
fn swallowed_errors_cannot_turn_failure_into_success() -> TestResult {
    let schema = Support::compile("entry struct Root { value: string = \"secret default\", }")?;
    assert_eq!(
        Support::code(
            Model::encode(
                schema.entry(),
                &Swallow,
                &Support::session()?,
                ConversionLimits::default()
            )
            .err()
            .test_value()?
        )?,
        15
    );
    let schema = Support::compile("entry struct Root { value: [u8], }")?;
    let call = Support::session()?;
    assert_eq!(
        Support::code(
            Model::encode(
                Support::ty(&schema)?,
                &SwallowSequence,
                &call,
                ConversionLimits::new(1, 128).test_value()?
            )
            .err()
            .test_value()?
        )?,
        1
    );

    Ok(())
}
