use super::{Bad, BindingTestSupport, DuplicateVariants, MultipleFieldMismatches, Root};
use crate::{
    binding_impl::{BindingMismatch, BindingMismatchKind},
    conversion::{ConversionContext, ConversionValue},
    descriptor::{DescribeContext, DescriptorProvider, TypeDescriptor, Typol},
    diagnostics::CodecErrors,
    options::{BindOptions, BindingLimits},
};
use opeo::{OResult, Out};
use typol_core::{ExecutionLimits, ValueId};
use typol_test_support::{
    TestExpect, TestExtract, TestResult, test_verify as assert, test_verify_eq as assert_eq,
};

macro_rules! rejected_typol_impl {
    ($ty:ty, |$context:ident, $out:ident| $describe:block) => {
        impl Typol for $ty {
            fn describe<'slot, 'borrow>(
                $context: &mut DescribeContext<'_>,
                mut $out: Out<'slot, 'borrow, CodecErrors>,
            ) -> OResult<'slot, TypeDescriptor> $describe

            fn to_config_value<'slot, 'borrow>(
                &self,
                _: &mut ConversionContext<'_>,
                out: Out<'slot, 'borrow, CodecErrors>,
            ) -> OResult<'slot, ValueId> {
                ConversionContext::reject(out)
            }

            fn from_config_value<'slot, 'borrow>(
                _: ConversionValue<'_>,
                _: &mut ConversionContext<'_>,
                out: Out<'slot, 'borrow, CodecErrors>,
            ) -> OResult<'slot, Self> {
                ConversionContext::reject(out)
            }
        }
    };
}

struct NestedChildMismatches;

rejected_typol_impl!(NestedChildMismatches, |context, out| {
    BindingTestSupport::structure::<Self>(
        context,
        "Child",
        &[("wrong", DescriptorProvider::of::<i16>())],
        out.reborrow(),
    )
});

struct NestedRootMismatches;

rejected_typol_impl!(NestedRootMismatches, |context, out| {
    BindingTestSupport::structure::<Self>(
        context,
        "Root",
        &[
            ("child", DescriptorProvider::of::<NestedChildMismatches>()),
            ("other", DescriptorProvider::of::<i16>()),
        ],
        out.reborrow(),
    )
});

#[test]
fn variant_rename_collisions_and_diagnostic_limits_preserve_stable_codes() -> TestResult {
    let schema = BindingTestSupport::compile(
        "enum Choice { One, Two } entry struct Root { value: Choice }",
    )?;
    let errors =
        BindingTestSupport::bind_error::<Root<DuplicateVariants>>(&schema, BindOptions::default())?;
    assert_eq!(BindingTestSupport::code(errors)?, 14);

    let schema = BindingTestSupport::compile("entry struct Root { value: u8 }")?;
    for limit in [0, 1, 2] {
        let options = BindOptions::new(
            BindingLimits::new(100, 128, limit, 262_144).test_value()?,
            ExecutionLimits::new(16_777_216, 67_108_864),
        );
        let errors = BindingTestSupport::bind_error::<Bad<0>>(&schema, options)?;
        assert_eq!(errors.diagnostics().len(), 1);
        assert_eq!(
            errors
                .diagnostics()
                .last()
                .map(|diagnostic| diagnostic.code().get()),
            Some(14)
        );
        let binding = errors
            .binding_errors()
            .test_expect("structured binding errors should be preserved under a limit")?;
        assert_eq!(binding.len(), 1);
        assert_eq!(binding.is_truncated(), limit < 2);
    }
    Ok(())
}

#[test]
fn binding_error_limits_preserve_mismatches_and_mark_truncation() -> TestResult {
    let schema =
        BindingTestSupport::compile("entry struct Root { alpha: u8, beta: string, gamma: bool }")?;
    let errors =
        BindingTestSupport::bind_error::<MultipleFieldMismatches>(&schema, BindOptions::default())?;
    let binding = errors
        .binding_errors()
        .test_expect("independent binding errors in a struct should be collected")?;
    assert_eq!(binding.len(), 4);
    assert!(!binding.is_truncated());
    assert_eq!(errors.diagnostics().len(), binding.len());
    assert_eq!(
        binding.get(0).map(BindingMismatch::kind),
        Some(BindingMismatchKind::MissingField)
    );
    assert_eq!(
        binding.get(0).map(BindingMismatch::schema_path),
        Some("$.Root.beta")
    );
    assert_eq!(
        binding.get(1).map(BindingMismatch::schema_path),
        Some("$.Root.gamma")
    );
    assert_eq!(
        binding.get(2).map(BindingMismatch::kind),
        Some(BindingMismatchKind::ExtraField)
    );
    assert_eq!(
        binding.get(3).map(BindingMismatch::kind),
        Some(BindingMismatchKind::FieldShape)
    );

    for limit in [0, 1, 2] {
        let options = BindOptions::new(
            BindingLimits::new(100, 128, limit, 262_144).test_value()?,
            ExecutionLimits::new(16_777_216, 67_108_864),
        );
        let errors = BindingTestSupport::bind_error::<MultipleFieldMismatches>(&schema, options)?;
        let binding = errors
            .binding_errors()
            .test_expect("the specific mismatch should be preserved within the limit")?;
        assert_eq!(binding.len(), 1);
        assert!(binding.is_truncated());
        assert_eq!(errors.diagnostics().len(), if limit > 1 { 2 } else { 1 });
        assert_eq!(
            errors
                .diagnostics()
                .last()
                .map(|diagnostic| diagnostic.code().get()),
            Some(if limit > 1 { 5 } else { 14 })
        );
        assert_eq!(
            binding.first().map(BindingMismatch::kind),
            Some(BindingMismatchKind::MissingField)
        );
    }
    Ok(())
}

#[test]
fn nested_mismatches_continue_across_sibling_fields() -> TestResult {
    let schema = BindingTestSupport::compile(
        "struct Child { value: u8 } entry struct Root { child: Child, other: string }",
    )?;
    let errors =
        BindingTestSupport::bind_error::<NestedRootMismatches>(&schema, BindOptions::default())?;
    let mismatches = errors
        .binding_errors()
        .test_expect("nested errors should not block sibling field checks")?;
    assert_eq!(mismatches.len(), 3);
    assert_eq!(
        mismatches.get(0).map(BindingMismatch::schema_path),
        Some("$.Root.child.Child.value")
    );
    assert_eq!(
        mismatches.get(1).map(BindingMismatch::rust_path),
        Some("$.Root.child.Child.wrong")
    );
    assert_eq!(
        mismatches.get(2).map(BindingMismatch::schema_path),
        Some("$.Root.other")
    );
    assert!(
        mismatches
            .get(2)
            .is_some_and(|mismatch| mismatch.kind() == BindingMismatchKind::FieldShape)
    );
    Ok(())
}
