use crate::{
    BindingTestSupport, TestAssertions,
    binding_impl::TypedSchema,
    conversion::{ConversionContext, ConversionValue},
    descriptor::{DescribeContext, DescriptorProvider, TypeDescriptor, Typol},
    diagnostics::CodecErrors,
    options::{BindOptions, CodecOptions, ConversionLimits},
    test_support::{TestOpeo, TestOpeoOutcome},
};
use opeo::{OResult, Out};
use typol_config::{
    ConfigParseOptions, ConfigParseRequest, Environment, EnvironmentError, EnvironmentReadContext,
};
use typol_core::{DiagnosticCode, SliceSource, ValueId};
use typol_test_support::{TestFailure, TestResult};

#[derive(Debug)]
struct Application {
    values: Vec<String>,
}

impl Typol for Application {
    fn describe<'slot, 'borrow>(
        context: &mut DescribeContext<'_>,
        out: Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, TypeDescriptor> {
        BindingTestSupport::structure::<Self>(
            context,
            "Root",
            &[("values", DescriptorProvider::of::<Vec<String>>())],
            out,
        )
    }

    fn to_config_value<'slot>(
        &self,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        context.start::<Self>(out.reborrow())?;
        context.structure(
            |slot, child, child_out| match slot {
                0 => child.encode_here(&self.values, child_out),
                _ => child.reject_with_path(child_out),
            },
            out.reborrow(),
        )
    }

    fn from_config_value<'slot>(
        value: ConversionValue<'_>,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, Self> {
        context.start::<Self>(out.reborrow())?;
        let fields = context.read_structure(value, out.reborrow())?;
        let values = context.read_field(fields, 0, out.reborrow())?;
        Ok(Self { values })
    }
}

struct NoEnvironment;

impl Environment for NoEnvironment {
    fn read<'slot, 'borrow, 'env>(
        &'env mut self,
        _: &str,
        _: EnvironmentReadContext<'_>,
        _: Out<'slot, 'borrow, EnvironmentError>,
    ) -> OResult<'slot, Option<&'env [u8]>> {
        Ok(None)
    }
}

#[test]
fn public_pipeline_allocations_fail_atomically_without_error_wrapper_allocations() -> TestResult {
    let schema = BindingTestSupport::compile("entry struct Root { values: [string] @size(1, 2) }")?;
    let value = Application {
        values: vec!["secret alpha".into(), "secret beta".into()],
    };
    let binding = TestOpeo::value(|out| {
        TypedSchema::<Application>::bind_trusted(&schema, BindOptions::default(), out)
    })?;
    TestAssertions::assert_atomic_allocations(|| {
        allocation_result(TestOpeo::outcome(|out| {
            binding.serialize(&value, CodecOptions::default(), out)
        }))
    })?;
    TestAssertions::assert_atomic_allocations(|| {
        let mut request_input =
            SliceSource::new(("{values: [\"secret alpha\", \"secret beta\"]}").as_bytes());
        let request = ConfigParseRequest::new(
            &mut request_input,
            "pipeline",
            ConfigParseOptions::default(),
        );
        allocation_result(TestOpeo::outcome(|out| {
            binding.deserialize(
                request,
                &mut NoEnvironment,
                ConversionLimits::default(),
                out,
            )
        }))
    })?;
    let invalid = Application { values: vec![] };
    TestAssertions::assert_atomic_allocations(|| {
        validation_result(TestOpeo::outcome(|out| {
            binding.serialize(&invalid, CodecOptions::default(), out)
        }))
    })?;
    TestAssertions::assert_atomic_allocations(|| {
        let mut request_input = SliceSource::new(("{values: []}").as_bytes());
        let request = ConfigParseRequest::new(
            &mut request_input,
            "pipeline",
            ConfigParseOptions::default(),
        );
        validation_result(TestOpeo::outcome(|out| {
            binding.deserialize(
                request,
                &mut NoEnvironment,
                ConversionLimits::default(),
                out,
            )
        }))
    })?;
    Ok(())
}

fn allocation_result<T>(outcome: TestOpeoOutcome<T, CodecErrors>) -> TestResult {
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
        TestOpeoOutcome::Error(errors) => {
            Err(TestFailure::new(format!("unexpected error: {errors:?}")))
        }
    }
}

fn validation_result<T>(outcome: TestOpeoOutcome<T, CodecErrors>) -> TestResult {
    match outcome {
        TestOpeoOutcome::Value(_) => Err(TestFailure::new(
            "expected validation to fail, but the operation succeeded",
        )),
        TestOpeoOutcome::Error(errors)
            if errors.diagnostics().first().is_some_and(|diagnostic| {
                diagnostic.code() == DiagnosticCode::CONFIG_VALIDATION_ERROR
            }) || errors.diagnostics().last().is_some_and(|diagnostic| {
                diagnostic.code() == DiagnosticCode::ALLOCATION_FAILED
            }) =>
        {
            Ok(())
        }
        TestOpeoOutcome::Error(errors) => Err(TestFailure::new(format!(
            "expected a validation error, got {errors:?}"
        ))),
    }
}
