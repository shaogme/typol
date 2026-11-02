use crate::{
    BindingTestSupport, TestAssertions,
    binding_impl::TypedSchema,
    conversion::{Conversion, ConversionContext, ConversionValue},
    descriptor::{
        DescribeContext, DescriptorNode, DescriptorProvider, TypeDescriptor, TypeShape, Typol,
    },
    diagnostics::{CodecErrors, CodecStage, Failure},
    options::{BindOptions, CodecOptions, ConversionLimits},
    session::SessionBudget,
    test_support::{TestOpeo, TestOpeoOutcome},
};
use opeo::{OResult, Out};
use std::{any::TypeId, array, str::from_utf8};
use typol_config::{
    ConfigParseOptions, ConfigParseRequest, Environment, EnvironmentError, EnvironmentReadContext,
};
use typol_core::{
    BudgetError, DiagnosticCode, DiagnosticContext, ExecutionLimits, SliceSource, ValueId,
    diagnostic_context,
};
use typol_test_support::{
    TestExtract, TestFailure, TestResult, test_verify as assert, test_verify_eq as assert_eq,
};

static PROFILE_INTERNAL_CONTEXT: DiagnosticContext =
    diagnostic_context!("typol-test", "profile", "workload-invariant");

fn codec_value<T>(
    operation: impl for<'slot> FnOnce(Out<'slot, 'slot, CodecErrors>) -> OResult<'slot, T>,
) -> TestResult<T> {
    TestOpeo::value(operation)
}

fn budget_value<T>(
    operation: impl for<'slot> FnOnce(Out<'slot, 'slot, BudgetError>) -> OResult<'slot, T>,
) -> TestResult<T> {
    TestOpeo::value(operation)
}

fn codec_succeeds<T>(
    operation: impl for<'slot> FnOnce(Out<'slot, 'slot, CodecErrors>) -> OResult<'slot, T>,
) -> bool {
    matches!(TestOpeo::outcome(operation), TestOpeoOutcome::Value(_))
}

fn allocation_codec<T>(outcome: TestOpeoOutcome<T, CodecErrors>) -> TestResult {
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

#[derive(Debug, PartialEq)]
struct Wide<const N: usize> {
    values: [u64; N],
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

impl<const N: usize> Typol for Wide<N> {
    fn describe<'slot>(
        context: &mut DescribeContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, TypeDescriptor> {
        let mut members = context.session().budget_codec(
            CodecStage::Binding,
            |budget_out| context.session().workspace(budget_out),
            out.reborrow(),
        )?;
        context.session().budget_codec(
            CodecStage::Binding,
            |budget_out| members.try_reserve_exact(N, budget_out),
            out.reborrow(),
        )?;
        for index in 0..N {
            let mut name = *b"field_0000";
            let mut digits = index;
            for position in (6..10).rev() {
                let digit = (digits % 10) as u8;
                let Some(byte) = name.get_mut(position) else {
                    return Err(out.fail(CodecErrors::emergency(
                        CodecStage::Binding,
                        Failure::internal(PROFILE_INTERNAL_CONTEXT),
                    )));
                };
                *byte = b'0' + digit;
                digits /= 10;
            }
            if digits != 0 {
                return Err(out.fail(CodecErrors::emergency(
                    CodecStage::Binding,
                    Failure::internal(PROFILE_INTERNAL_CONTEXT),
                )));
            }
            let field_name = match from_utf8(&name) {
                Ok(name) => name,
                Err(_) => {
                    return Err(out.fail(CodecErrors::emergency(
                        CodecStage::Binding,
                        Failure::internal(PROFILE_INTERNAL_CONTEXT),
                    )));
                }
            };
            let field_name = context.name(field_name, out.reborrow())?;
            let field = context.insert(
                DescriptorNode::Field {
                    name: field_name,
                    value: DescriptorProvider::of::<u64>(),
                },
                context.depth(),
                out.reborrow(),
            )?;
            context.session().budget_codec(
                CodecStage::Binding,
                |budget_out| members.try_push(field, budget_out),
                out.reborrow(),
            )?;
        }
        let type_name = context.name("Wide", out.reborrow())?;
        context.insert(
            DescriptorNode::Type {
                identity: TypeId::of::<Self>(),
                name: Some(type_name),
                shape: TypeShape::Structure(members),
            },
            context.depth(),
            out.reborrow(),
        )
    }

    fn to_config_value<'slot>(
        &self,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        context.start::<Self>(out.reborrow())?;
        context.structure(
            |slot, child, child_out| match self.values.get(slot) {
                Some(value) => child.encode_here(value, child_out),
                None => Err(child_out.fail(CodecErrors::emergency(
                    CodecStage::Conversion,
                    Failure::internal(PROFILE_INTERNAL_CONTEXT),
                ))),
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
        let mut values = [0; N];
        for (slot, value) in values.iter_mut().enumerate() {
            *value = context.read_field(fields, slot, out.reborrow())?;
        }
        Ok(Self { values })
    }
}

fn options(limits: ExecutionLimits) -> ConfigParseOptions {
    let defaults = ConfigParseOptions::default();
    ConfigParseOptions::new(*defaults.resources(), *defaults.config(), limits)
}

fn minimum(mut operation: impl FnMut(usize) -> bool, upper: usize) -> TestResult<usize> {
    assert!(operation(upper));
    let (mut low, mut high) = (0, upper);
    while low < high {
        let middle = low + (high - low) / 2;
        if operation(middle) {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    assert!(operation(low));
    assert!(low > 0 && !operation(low - 1));
    Ok(low)
}

#[test]
fn wide_public_pipeline_limits_and_allocation_failures_are_atomic() -> TestResult {
    let fields = (0..32)
        .map(|index| format!("field_{index:04}: u64"))
        .collect::<Vec<_>>()
        .join(", ");
    let schema = BindingTestSupport::compile(&format!("entry struct Wide {{ {fields} }}"))?;
    let value = Wide { values: [42; 32] };
    TestAssertions::assert_atomic_allocations(|| {
        allocation_codec(TestOpeo::outcome(|out| {
            TypedSchema::<Wide<32>>::bind_trusted(&schema, BindOptions::default(), out)
        }))
    })?;
    let binding = codec_value(|out| {
        TypedSchema::<Wide<32>>::bind_trusted(&schema, BindOptions::default(), out)
    })?;
    TestAssertions::assert_atomic_allocations(|| {
        allocation_codec(TestOpeo::outcome(|out| {
            binding.serialize(&value, CodecOptions::default(), out)
        }))
    })?;
    let text = codec_value(|out| binding.serialize(&value, CodecOptions::default(), out))?;
    TestAssertions::assert_atomic_allocations(|| {
        let mut request_input = SliceSource::new(text.as_bytes());
        let request = ConfigParseRequest::new(
            &mut request_input,
            "wide struct",
            ConfigParseOptions::default(),
        );
        allocation_codec(TestOpeo::outcome(|out| {
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

#[test]
fn wide_name_lookup_succeeds_with_defaults_and_preserves_shared_step_boundary() -> TestResult {
    let fields = (0..2048)
        .map(|index| format!("field_{index:04}: u64"))
        .collect::<Vec<_>>()
        .join(", ");
    let schema = BindingTestSupport::compile(&format!("entry struct Wide {{ {fields} }}"))?;
    let binding = codec_value(|out| {
        TypedSchema::<Wide<2048>>::bind_trusted(&schema, BindOptions::default(), out)
    })?;
    let text = codec_value(|out| {
        binding.serialize(&Wide { values: [42; 2048] }, CodecOptions::default(), out)
    })?;
    let run = |steps| {
        codec_succeeds(|out| {
            binding.deserialize(
                ConfigParseRequest::new(
                    &mut SliceSource::new(text.as_bytes()),
                    "wide struct",
                    options(ExecutionLimits::new(steps, 67_108_864)),
                ),
                &mut NoEnvironment,
                ConversionLimits::default(),
                out,
            )
        })
    };
    let steps = minimum(run, 16_777_216)?;
    let value = codec_value(|out| {
        binding.deserialize(
            ConfigParseRequest::new(
                &mut SliceSource::new(text.as_bytes()),
                "wide struct",
                options(ExecutionLimits::new(steps, 67_108_864)),
            ),
            &mut NoEnvironment,
            ConversionLimits::default(),
            out,
        )
    })?;
    assert_eq!(value.values, [42; 2048]);
    let errors = TestOpeo::error(|out| {
        binding.deserialize(
            ConfigParseRequest::new(
                &mut SliceSource::new(text.as_bytes()),
                "wide struct",
                options(ExecutionLimits::new(steps - 1, 67_108_864)),
            ),
            &mut NoEnvironment,
            ConversionLimits::default(),
            out,
        )
    })?;
    assert_eq!(
        errors.diagnostics().last().test_value()?.code(),
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
    );
    Ok(())
}

fn wide_decoding_steps<const N: usize>() -> TestResult<usize> {
    let fields = (0..N)
        .rev()
        .map(|index| format!("field_{index:04}: u64"))
        .collect::<Vec<_>>()
        .join(", ");
    let schema = BindingTestSupport::compile(&format!("entry struct Wide {{ {fields} }}"))?;
    let binding = codec_value(|out| {
        TypedSchema::<Wide<N>>::bind_trusted(&schema, BindOptions::default(), out)
    })?;
    let expected = Wide {
        values: array::from_fn(|index| index as u64),
    };
    let session = budget_value(|out| SessionBudget::codec(CodecOptions::default(), out))?;
    let tree = codec_value(|out| {
        Conversion::encode(
            &binding,
            &expected,
            &session,
            ConversionLimits::default(),
            out,
        )
    })?
    .into_value();
    let session = budget_value(|out| SessionBudget::codec(CodecOptions::default(), out))?;
    let initial = session.execution().workspace_bytes();
    let actual = codec_value(|out| {
        Conversion::decode(&binding, &tree, &session, ConversionLimits::default(), out)
    })?;
    assert_eq!(actual, expected);
    assert_eq!(session.execution().workspace_bytes(), initial);
    let text = (0..N)
        .map(|index| format!("field_{index:04}: {index}"))
        .collect::<Vec<_>>()
        .join(", ");
    assert_eq!(
        codec_value(|out| binding.deserialize(
            ConfigParseRequest::new(
                &mut SliceSource::new(format!("{{ {text} }}").as_bytes()),
                "out-of-order wide field",
                ConfigParseOptions::default(),
            ),
            &mut NoEnvironment,
            ConversionLimits::default(),
            out,
        ))?,
        expected
    );
    Ok(session.execution().steps())
}

#[test]
fn wide_target_lookup_steps_grow_subquadratically_with_reordered_schema_and_input() -> TestResult {
    let small = wide_decoding_steps::<512>()?;
    let large = wide_decoding_steps::<1024>()?;
    assert!(large <= small * 5 / 2, "512: {small}, 1024: {large}");
    Ok(())
}
