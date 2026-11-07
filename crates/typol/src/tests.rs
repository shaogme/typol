mod allocation;
mod api;
mod profile;

use crate::{
    conversion::ConversionContext,
    descriptor::{
        DescribeContext, DescriptorNode, DescriptorProvider, MapContainer, TypeDescriptor,
        TypeShape, VariantShape,
    },
    diagnostics::{CodecErrors, CodecStage, ErrorBuffer, Failure},
    options::{BindOptions, BindingLimits, ConversionLimits, WriteOptions},
    session::{NodeBudget, SessionBudget},
    test_support::{TestOpeo, TestValues},
};
use opeo::{OResult, Out};
use std::{any::TypeId, cmp::Ordering, mem::size_of};
use typol_config::{ConfigLimits, ConfigValidationOptions};
use typol_core::{BudgetError, DiagnosticContext, ExecutionLimits, diagnostic_context};
use typol_model::BuiltinType;
use typol_policy::{ExcerptLimits, RegexOptions};
use typol_test_support::{
    TestExpect, TestExtract, TestResult, test_verify as assert, test_verify_eq as assert_eq,
};

pub(crate) use allocation::TestAssertions;

#[cfg(feature = "serde")]
pub(crate) use allocation::AllocationScope;

static TEST_INTERNAL_CONTEXT: DiagnosticContext =
    diagnostic_context!("typol-test", "binding", "test-invariant");

fn session() -> TestResult<SessionBudget> {
    TestOpeo::value(|out| SessionBudget::codec(WriteOptions::default(), out))
}

fn context<'session>(
    limits: BindingLimits,
    session: &'session SessionBudget,
) -> TestResult<DescribeContext<'session>> {
    TestOpeo::value(|out| DescribeContext::new(limits, session, out))
}

fn budget_value<T>(
    operation: impl for<'slot> FnOnce(Out<'slot, 'slot, BudgetError>) -> OResult<'slot, T>,
) -> TestResult<T> {
    TestOpeo::value(operation)
}

fn budget_error<T>(
    operation: impl for<'slot> FnOnce(Out<'slot, 'slot, BudgetError>) -> OResult<'slot, T>,
) -> TestResult<BudgetError> {
    TestOpeo::error(operation)
}

fn codec_value<T>(
    operation: impl for<'slot> FnOnce(Out<'slot, 'slot, CodecErrors>) -> OResult<'slot, T>,
) -> TestResult<T> {
    TestOpeo::value(operation)
}

fn codec_error<T>(
    operation: impl for<'slot> FnOnce(Out<'slot, 'slot, CodecErrors>) -> OResult<'slot, T>,
) -> TestResult<CodecErrors> {
    TestOpeo::error(operation)
}

fn session_with_limits(
    limits: ExecutionLimits,
    text_limit: usize,
    output_limit: usize,
) -> TestResult<SessionBudget> {
    budget_value(|out| SessionBudget::new(limits, text_limit, output_limit, out))
}

fn scalar<'slot, 'borrow>(
    context: &mut DescribeContext<'_>,
    depth: usize,
    out: Out<'slot, 'borrow, CodecErrors>,
) -> OResult<'slot, TypeDescriptor> {
    context.scalar::<u8>(BuiltinType::U8, depth, out)
}

fn provider() -> DescriptorProvider {
    DescriptorProvider::new::<u8>(scalar)
}

fn assert_code(errors: CodecErrors, code: u32) -> TestResult {
    assert_eq!(
        errors
            .diagnostics()
            .last()
            .test_expect("error diagnostic list should include the final diagnostic")?
            .code()
            .get(),
        code
    );
    Ok(())
}

#[test]
fn defaults_are_finite_and_zero_options_are_preserved() -> TestResult {
    let bind = BindOptions::default();
    assert_eq!(bind.limits().max_nodes(), 100_000);
    assert_eq!(bind.limits().max_depth(), 128);
    assert_eq!(bind.limits().max_name_bytes(), 262_144);
    assert_eq!(bind.limits().max_diagnostics(), 100);
    assert_eq!(
        bind.execution(),
        ExecutionLimits::new(16_777_216, 67_108_864)
    );
    let codec = WriteOptions::default();
    assert_eq!(
        codec.conversion(),
        ConversionLimits::new(100_000, 128).test_value()?
    );
    assert_eq!(codec.max_output_bytes(), 1_048_576);
    assert_eq!(codec.validation(), ConfigValidationOptions::default());
    assert_eq!(BindingLimits::new(0, 0, 0, 0).test_value()?.max_nodes(), 0);
    assert_eq!(
        TestOpeo::error(|out| SessionBudget::new(ExecutionLimits::new(0, 0), 0, 0, out,))?,
        BudgetError::Resource
    );
    Ok(())
}

#[test]
fn descriptor_node_depth_name_and_length_boundaries_are_checked() -> TestResult {
    let first_session = session()?;
    let mut ctx = context(BindingLimits::new(1, 0, 0, 4).test_value()?, &first_session)?;
    let descriptor = TestOpeo::value(|out| scalar(&mut ctx, 0, out))?;
    assert!(matches!(
        TestOpeo::value(|out| ctx.get(descriptor, out))?,
        DescriptorNode::Type {
            shape: TypeShape::Scalar(BuiltinType::U8),
            ..
        }
    ));
    assert_eq!(TestOpeo::value(|out| ctx.name("four", out))?.text(), "four");
    assert_code(TestOpeo::error(|out| scalar(&mut ctx, 0, out))?, 1)?;
    let other_session = session()?;
    let ctx = context(BindingLimits::new(1, 0, 0, 4).test_value()?, &other_session)?;
    assert_code(TestOpeo::error(|out| ctx.name("fives", out))?, 1)?;
    assert!(ctx.session().execution().exhausted());
    let other_session = session()?;
    let mut ctx = context(BindingLimits::new(1, 0, 0, 4).test_value()?, &other_session)?;
    assert_code(
        TestOpeo::error(|out| provider().resolve(&mut ctx, 1, out))?,
        1,
    )?;
    let other_session = session()?;
    let mut ctx = context(BindingLimits::new(0, 0, 0, 0).test_value()?, &other_session)?;
    assert_eq!(TestOpeo::value(|out| ctx.name("", out))?.text(), "");
    assert_code(TestOpeo::error(|out| scalar(&mut ctx, 0, out))?, 1)?;
    assert_eq!(
        TestOpeo::value(|out| DescribeContext::array_length(u32::MAX as usize, out))?,
        u32::MAX
    );
    if let Some(too_large) = (u32::MAX as usize).checked_add(1) {
        assert_code(
            TestOpeo::error(|out| DescribeContext::array_length(too_large, out))?,
            14,
        )?;
    };
    Ok(())
}

#[test]
fn descriptor_members_preserve_slots_owner_and_workspace_limits() -> TestResult {
    let first_session = session()?;
    let mut first = context(BindingLimits::default(), &first_session)?;
    let second_session = session()?;
    let second = context(BindingLimits::default(), &second_session)?;
    let descriptor = TestOpeo::value(|out| scalar(&mut first, 0, out))?;
    let members = TestOpeo::value(|out| first.members(&[descriptor, descriptor], out))?;
    assert_eq!(members.as_slice(), &[descriptor, descriptor]);
    assert_code(
        TestOpeo::error(|out| second.members(&[descriptor], out))?,
        9,
    )?;
    let session =
        TestOpeo::value(|out| SessionBudget::new(ExecutionLimits::new(1000, 1024), 100, 0, out))?;
    let mut limited = context(BindingLimits::default(), &session)?;
    assert!(TestOpeo::value(|out| limited.members(&[], out))?.is_empty());
    let descriptor = TestOpeo::value(|out| scalar(&mut limited, 0, out))?;
    assert_code(
        TestOpeo::error(|out| limited.members(&[descriptor; 128], out))?,
        1,
    )?;
    Ok(())
}

#[test]
fn descriptor_providers_are_lazy_and_preserve_actual_type_identity() -> TestResult {
    let deferred = DescriptorProvider::new::<u8>(|_, _, out| ConversionContext::reject(out));
    assert_eq!(deferred.identity(), TypeId::of::<u8>());
    let session = session()?;
    let mut ctx = context(BindingLimits::default(), &session)?;
    for shape in [
        TypeShape::Option(deferred),
        TypeShape::Sequence(deferred),
        TypeShape::Array {
            element: deferred,
            length: 3,
        },
        TypeShape::Map {
            container: MapContainer::BTreeMap,
            value: deferred,
        },
        TypeShape::Map {
            container: MapContainer::HashMap,
            value: deferred,
        },
        TypeShape::Map {
            container: MapContainer::OrderedMap,
            value: deferred,
        },
    ] {
        TestOpeo::value(|out| {
            ctx.insert(
                DescriptorNode::Type {
                    identity: TypeId::of::<Vec<u8>>(),
                    name: None,
                    shape,
                },
                0,
                out,
            )
        })?;
    }
    let wrong = DescriptorProvider::new::<u16>(scalar);
    assert_code(TestOpeo::error(|out| wrong.resolve(&mut ctx, 0, out))?, 9)?;
    Ok(())
}

#[test]
fn arena_handles_and_member_storage_cannot_cross_contexts() -> TestResult {
    let first_session = session()?;
    let mut first = context(BindingLimits::default(), &first_session)?;
    let second_session = session()?;
    let mut second = context(BindingLimits::default(), &second_session)?;
    let descriptor = TestOpeo::value(|out| scalar(&mut first, 0, out))?;
    assert_code(TestOpeo::error(|out| second.get(descriptor, out))?, 9)?;
    let name = TestOpeo::value(|out| first.name("field", out))?;
    assert_code(
        TestOpeo::error(|out| {
            second.insert(
                DescriptorNode::Field {
                    name,
                    value: provider(),
                },
                0,
                out,
            )
        })?,
        9,
    )?;
    let name = TestOpeo::value(|out| second.name("field", out))?;
    let field = TestOpeo::value(|out| {
        second.insert(
            DescriptorNode::Field {
                name,
                value: provider(),
            },
            0,
            out,
        )
    })?;
    let mut fields = TestOpeo::value(|out| second.session().workspace(out))?;
    TestOpeo::value::<BudgetError, _>(|out| fields.try_reserve_exact(1, out))?;
    TestOpeo::value::<BudgetError, _>(|out| fields.try_push(field, out))?;
    let name = TestOpeo::value(|out| second.name("Root", out))?;
    let root = TestOpeo::value(|out| {
        second.insert(
            DescriptorNode::Type {
                identity: TypeId::of::<()>(),
                name: Some(name),
                shape: TypeShape::Structure(fields),
            },
            0,
            out,
        )
    })?;
    assert!(matches!(
        TestOpeo::value(|out| second.get(root, out))?,
        DescriptorNode::Type {
            shape: TypeShape::Structure(_),
            ..
        }
    ));
    let fields = TestOpeo::value(|out| first.session().workspace(out))?;
    let name = TestOpeo::value(|out| second.name("Variant", out))?;
    assert_code(
        TestOpeo::error(|out| {
            second.insert(
                DescriptorNode::Variant {
                    name,
                    shape: VariantShape::Structure(fields),
                },
                0,
                out,
            )
        })?,
        9,
    )?;
    for shape in [
        VariantShape::Unit,
        VariantShape::Single(provider()),
        VariantShape::Structure(TestOpeo::value(|out| second.session().workspace(out))?),
    ] {
        let name = TestOpeo::value(|out| second.name("Variant", out))?;
        let variant =
            TestOpeo::value(|out| second.insert(DescriptorNode::Variant { name, shape }, 0, out))?;
        let mut variants = TestOpeo::value(|out| second.session().workspace(out))?;
        TestOpeo::value::<BudgetError, _>(|out| variants.try_reserve_exact(1, out))?;
        TestOpeo::value::<BudgetError, _>(|out| variants.try_push(variant, out))?;
        let name = TestOpeo::value(|out| second.name("Enum", out))?;
        TestOpeo::value(|out| {
            second.insert(
                DescriptorNode::Type {
                    identity: TypeId::of::<()>(),
                    name: Some(name),
                    shape: TypeShape::Enumeration(variants),
                },
                0,
                out,
            )
        })?;
    }
    Ok(())
}

#[test]
fn shared_session_counts_text_reads_writes_and_output_without_resetting() -> TestResult {
    let mut call = session_with_limits(ExecutionLimits::new(9, 4096), 6, 2)?;
    let shared = budget_value(|out| call.execution().try_clone(out))?;
    drop(budget_value(|out| call.copy_text("ab", out))?);
    assert_eq!(shared.steps(), 4);
    assert_eq!(
        budget_value(|out| call.compare_text("a", "b", out))?,
        Ordering::Less
    );
    assert_eq!(shared.steps(), 7);
    codec_value(|out| call.output_codec(2, out))?;
    assert_eq!(shared.steps(), 9);
    assert_eq!(shared.constructed_text_bytes(), 4);
    assert_eq!(
        budget_error(|out| shared.consume(1, out))?,
        BudgetError::Resource
    );
    let mut call = session_with_limits(ExecutionLimits::new(20, 4096), 2, 2)?;
    codec_value(|out| call.output_codec(2, out))?;
    assert_eq!(
        codec_error(|out| call.output_codec(1, out))?.stage(),
        CodecStage::Encoding
    );
    let call = session_with_limits(ExecutionLimits::new(20, 4096), 2, 2)?;
    drop(budget_value(|out| call.copy_text("ab", out))?);
    assert_eq!(
        budget_error(|out| call.copy_text("c", out))?,
        BudgetError::Resource
    );
    let mut call = session_with_limits(ExecutionLimits::new(0, 4096), 0, 0)?;
    codec_value(|out| call.output_codec(0, out))?;
    assert_eq!(
        codec_error(|out| call.output_codec(1, out))?.stage(),
        CodecStage::Encoding
    );
    let mut call = session_with_limits(
        ExecutionLimits::new(usize::MAX, 4096),
        usize::MAX,
        usize::MAX,
    )?;
    codec_value(|out| call.output_codec(1, out))?;
    assert_eq!(
        codec_error(|out| call.output_codec(usize::MAX, out))?.stage(),
        CodecStage::Encoding
    );
    Ok(())
}

#[test]
fn workspace_capacity_releases_only_when_storage_is_dropped() -> TestResult {
    let initial = session()?.execution().workspace_bytes();
    let call = session_with_limits(
        ExecutionLimits::new(100, initial + 3 * size_of::<u64>()),
        100,
        100,
    )?;
    let mut values = budget_value(|out| call.workspace::<u64>(out))?;
    TestOpeo::value::<BudgetError, _>(|out| values.try_reserve_exact(1, out))?;
    budget_value(|out| values.try_push(1, out))?;
    TestOpeo::value::<BudgetError, _>(|out| values.try_reserve_exact(1, out))?;
    assert_eq!(
        call.execution().peak_workspace_bytes(),
        initial + 3 * size_of::<u64>()
    );
    values.clear();
    assert_eq!(
        call.execution().workspace_bytes(),
        initial + 2 * size_of::<u64>()
    );
    drop(values);
    assert_eq!(call.execution().workspace_bytes(), initial);
    let call = session_with_limits(
        ExecutionLimits::new(100, initial + 3 * size_of::<u64>() - 1),
        100,
        100,
    )?;
    let mut values = budget_value(|out| call.workspace::<u64>(out))?;
    TestOpeo::value::<BudgetError, _>(|out| values.try_reserve_exact(1, out))?;
    budget_value(|out| values.try_push(7, out))?;
    assert_eq!(
        budget_error(|out| values.try_reserve_exact(1, out))?,
        BudgetError::Resource
    );
    assert_eq!(values.as_slice(), [7]);
    Ok(())
}

#[test]
fn conversion_children_share_node_counts_and_restore_parent_depth_on_failure() -> TestResult {
    let call = session()?;
    let mut nodes = ConversionContext::counters(ConversionLimits::new(0, 0).test_value()?);
    let mut ctx = ConversionContext::new(&call, &mut nodes);
    assert_code(codec_error(|out| ctx.node(out))?, 1)?;
    let call = session()?;
    let mut nodes = ConversionContext::counters(ConversionLimits::new(1, 0).test_value()?);
    let mut ctx = ConversionContext::new(&call, &mut nodes);
    codec_value(|out| ctx.node(out))?;
    assert_code(
        codec_error(|out| {
            ctx.child_with_path::<()>(
                None,
                |_, child_out| ConversionContext::reject(child_out),
                out,
            )
        })?,
        1,
    )?;
    let call = session()?;
    let mut nodes = ConversionContext::counters(ConversionLimits::new(3, 1).test_value()?);
    let mut ctx = ConversionContext::new(&call, &mut nodes);
    codec_value(|out| ctx.node(out))?;
    codec_value(|out| ctx.child_with_path(None, |child, child_out| child.node(child_out), out))?;
    assert_code(
        codec_error(|out| {
            ctx.child_with_path(
                None,
                |child, child_out| child.child_with_path(None, |_, _nested_out| Ok(()), child_out),
                out,
            )
        })?,
        1,
    )?;
    let call = session()?;
    let mut nodes = NodeBudget::new(3, 1);
    let mut ctx = ConversionContext::new(&call, &mut nodes);
    codec_value(|out| ctx.node(out))?;
    assert_code(
        codec_error(|out| {
            ctx.child_with_path(
                None,
                |child, mut child_out| {
                    child.node(child_out.reborrow())?;
                    Err::<(), _>(child_out.fail(CodecErrors::emergency(
                        CodecStage::Conversion,
                        Failure::Conversion,
                    )))
                },
                out,
            )
        })?,
        15,
    )?;
    codec_value(|out| ctx.node(out))?;
    assert_code(codec_error(|out| ctx.node(out))?, 1)?;
    assert_eq!(nodes.nodes(), 3);
    Ok(())
}

#[test]
fn requests_and_error_formatting_never_print_input_names_or_values() -> TestResult {
    let value = TestValues::string_unmetered("secret-map-key-and-value".into());
    assert!(!format!("{value:?}").contains("secret"));
    let call = session()?;
    let ctx = context(BindingLimits::default(), &call)?;
    let name = codec_value(|out| ctx.name("secret-name", out))?;
    assert!(!format!("{name:?} {ctx:?}").contains("secret"));
    for failure in [
        Failure::BindingMismatch,
        Failure::RecursiveType,
        Failure::ArrayLength,
        Failure::Conversion,
        Failure::Encoding,
    ] {
        let call = session()?;
        let mut buffer =
            budget_value(|out| ErrorBuffer::new(CodecStage::Conversion, 2, &call, out))?;
        assert!(buffer.push(failure));
        let errors = buffer.finish();
        assert_eq!(errors.stage(), CodecStage::Conversion);
        assert!(!format!("{errors:?} {errors}").contains("secret"));
        let diagnostic = errors
            .diagnostics()
            .first()
            .test_expect("error list should preserve the terminal diagnostic")?;
        assert!(diagnostic.primary().is_none());
        assert!(diagnostic.value_source().is_none());
    }
    Ok(())
}

#[test]
fn diagnostic_limits_preserve_nonempty_terminal_and_real_failure_codes() -> TestResult {
    for limit in [0, 1, 2, 3] {
        let call = session()?;
        let mut buffer =
            budget_value(|out| ErrorBuffer::new(CodecStage::Binding, limit, &call, out))?;
        for _ in 0..limit.max(1) - 1 {
            assert!(buffer.push(Failure::BindingMismatch));
        }
        assert!(!buffer.push(Failure::BindingMismatch));
        assert!(!buffer.push(Failure::Conversion));
        let errors = buffer.finish();
        assert_eq!(errors.diagnostics().len(), limit.max(1));
        assert_code(errors, 5)?;
    }
    for failure in [
        Failure::Resource,
        Failure::Allocation,
        Failure::internal(TEST_INTERNAL_CONTEXT),
    ] {
        let call = session()?;
        let mut buffer = budget_value(|out| ErrorBuffer::new(CodecStage::Binding, 2, &call, out))?;
        assert!(buffer.push(Failure::BindingMismatch));
        assert!(!buffer.push(failure));
        assert_code(buffer.finish(), failure.code().get())?;
    }
    let call = session()?;
    assert_code(
        budget_value(|out| ErrorBuffer::new(CodecStage::Binding, 0, &call, out))?.finish(),
        9,
    )?;
    let options = ConfigValidationOptions::new(
        ExcerptLimits::default(),
        RegexOptions::default(),
        ConfigLimits::default()
            .with_max_environment_entries(0)
            .with_max_environment_bytes(0)
            .with_max_constructed_text_bytes(0)
            .with_max_display_name_bytes(0),
        ExecutionLimits::new(100, 4096),
    );
    let call = budget_value(|out| {
        SessionBudget::codec(
            WriteOptions::new(0)
                .with_validation(options)
                .with_conversion_limits(ConversionLimits::default()),
            out,
        )
    })?;
    let mut buffer = budget_value(|out| ErrorBuffer::new(CodecStage::Encoding, 2, &call, out))?;
    assert!(!buffer.push(Failure::Encoding));
    assert_code(buffer.finish(), 1)?;
    Ok(())
}
