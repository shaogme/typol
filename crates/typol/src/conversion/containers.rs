use crate::{
    conversion::{ConversionContext, ConversionValue},
    descriptor::{
        BudgetedTypol, DescribeContext, DescriptorNode, DescriptorProvider, TypeDescriptor,
        TypeShape, Typol,
    },
    diagnostics::{CodecErrors, CodecStage, ConversionDiagnosticInput, ConversionErrorKind},
    value_walk::ValueNodeKind,
};
use alloc::vec::Vec;
use core::{any::TypeId, mem::size_of};
use opeo::{OResult, Out};
use typol_core::{ChargedVec, ValueId, ValueView};

#[cfg(feature = "std")]
use crate::{descriptor::MapContainer, diagnostics::Failure, value_walk::NAMED_ENTRY_NAME_MISSING};
#[cfg(feature = "std")]
use alloc::string::String;
#[cfg(feature = "std")]
use std::collections::HashMap;
#[cfg(feature = "std")]
use typol_core::NamedValue;

impl<T: Typol> Typol for Option<T> {
    fn describe<'slot, 'borrow>(
        context: &mut DescribeContext<'_>,
        out: Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, TypeDescriptor> {
        describe_container::<Self>(
            context,
            TypeShape::Option(DescriptorProvider::of::<T>()),
            out,
        )
    }

    fn to_config_value<'slot>(
        &self,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        context.start::<Self>(out.reborrow())?;
        let ty = context.element_type(out.reborrow())?;
        match self {
            Some(value) => {
                let value = context.encode_child::<T>(ty, value, out.reborrow())?;
                context.optional(Some(value), out.reborrow())
            }
            None => context.none(out.reborrow()),
        }
    }

    fn from_config_value<'slot>(
        value: ConversionValue<'_>,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, Self> {
        context.start::<Self>(out.reborrow())?;
        match context.read_value_node(value.view(), out.reborrow())? {
            ValueNodeKind::Optional {
                inner,
                value: Some(value),
            } => context
                .decode_child::<T>(inner, ConversionValue::new(value), out.reborrow())
                .map(Some),
            ValueNodeKind::Optional { value: None, .. } => Ok(None),
            _ => context.reject_with_path(out.reborrow()),
        }
    }
}

impl<T: BudgetedTypol> BudgetedTypol for Option<T> {}

impl<T: Typol> Typol for Vec<T> {
    fn describe<'slot, 'borrow>(
        context: &mut DescribeContext<'_>,
        out: Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, TypeDescriptor> {
        describe_container::<Self>(
            context,
            TypeShape::Sequence(DescriptorProvider::of::<T>()),
            out,
        )
    }

    fn to_config_value<'slot>(
        &self,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        context.start::<Self>(out.reborrow())?;
        write_sequence(self, context, out.reborrow())
    }

    fn from_config_value<'slot>(
        value: ConversionValue<'_>,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, Self> {
        context.start::<Self>(out.reborrow())?;
        let values =
            read_sequence::<T>(value.view(), context, out.reborrow())?.into_unmetered_vec();
        let Some(bytes) = values.capacity().checked_mul(size_of::<T>()) else {
            return Err(out.fail(ConversionContext::resource_error()));
        };
        context.target_bytes(bytes, out.reborrow())?;
        Ok(values)
    }
}

impl<T: BudgetedTypol> BudgetedTypol for Vec<T> {}

impl<T: Typol, const N: usize> Typol for [T; N] {
    fn describe<'slot>(
        context: &mut DescribeContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, TypeDescriptor> {
        let length = DescribeContext::array_length(N, out.reborrow())?;
        describe_container::<Self>(
            context,
            TypeShape::Array {
                element: DescriptorProvider::of::<T>(),
                length,
            },
            out.reborrow(),
        )
    }

    fn to_config_value<'slot>(
        &self,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        context.start::<Self>(out.reborrow())?;
        write_sequence(self, context, out.reborrow())
    }

    fn from_config_value<'slot>(
        value: ConversionValue<'_>,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, Self> {
        context.start::<Self>(out.reborrow())?;
        let values =
            read_sequence::<T>(value.view(), context, out.reborrow())?.into_unmetered_vec();
        match values.try_into() {
            Ok(values) => Ok(values),
            Err(_) => context.fail_as(
                ConversionDiagnosticInput::new(
                    ConversionErrorKind::ArrayLengthMismatch,
                    "value with the Rust fixed-array length",
                    "array length mismatch",
                    context.path,
                ),
                out.reborrow(),
            ),
        }
    }
}

impl<T: BudgetedTypol, const N: usize> BudgetedTypol for [T; N] {}

#[cfg(feature = "std")]
impl<T: Typol> Typol for HashMap<String, T> {
    fn describe<'slot, 'borrow>(
        context: &mut DescribeContext<'_>,
        out: Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, TypeDescriptor> {
        describe_container::<Self>(
            context,
            TypeShape::Map {
                container: MapContainer::HashMap,
                value: DescriptorProvider::of::<T>(),
            },
            out,
        )
    }

    fn to_config_value<'slot>(
        &self,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        context.start::<Self>(out.reborrow())?;
        write_map(self.iter(), context, out.reborrow())
    }

    fn from_config_value<'slot>(
        value: ConversionValue<'_>,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, Self> {
        context.start::<Self>(out.reborrow())?;
        let ValueNodeKind::Map {
            value_type: ty,
            values: entries,
        } = context.read_value_node(value.view(), out.reborrow())?
        else {
            return context.reject_with_path(out.reborrow());
        };
        let Some(bytes) = entries.len().checked_mul(size_of::<(String, T)>()) else {
            return Err(out.fail(ConversionContext::resource_error()));
        };
        context.target_bytes(bytes, out.reborrow())?;
        let mut result = HashMap::new();
        if result.try_reserve(entries.len()).is_err() {
            return Err(out.fail(ConversionContext::allocation_error()));
        }
        let Some(actual) = result.capacity().checked_mul(size_of::<(String, T)>()) else {
            return Err(out.fail(ConversionContext::resource_error()));
        };
        context.target_bytes(actual - bytes, out.reborrow())?;
        for entry in entries {
            context.child(|child, child_out| child.node(child_out), out.reborrow())?;
            let Some(entry) = entry.as_named() else {
                return Err(out.fail(CodecErrors::emergency(
                    CodecStage::Conversion,
                    Failure::internal(NAMED_ENTRY_NAME_MISSING),
                )));
            };
            let key = context.target_text(entry.name(), out.reborrow())?;
            context.session().budget_codec(
                CodecStage::Conversion,
                |budget_out| context.session().execution().consume(key.len(), budget_out),
                out.reborrow(),
            )?;
            let value = context.decode_child::<T>(
                ty,
                ConversionValue::new(entry.value()),
                out.reborrow(),
            )?;
            if result.insert(key, value).is_some() {
                return context.reject_with_path(out.reborrow());
            }
        }
        Ok(result)
    }
}

#[cfg(feature = "std")]
impl<T: BudgetedTypol> BudgetedTypol for HashMap<String, T> {}

fn describe_container<'slot, 'borrow, T: 'static>(
    context: &mut DescribeContext<'_>,
    shape: TypeShape,
    out: Out<'slot, 'borrow, CodecErrors>,
) -> OResult<'slot, TypeDescriptor> {
    context.insert(
        DescriptorNode::Type {
            identity: TypeId::of::<T>(),
            name: None,
            shape,
        },
        context.depth(),
        out,
    )
}

fn write_sequence<'slot, T: Typol>(
    values: &[T],
    context: &mut ConversionContext<'_>,
    mut out: Out<'slot, '_, CodecErrors>,
) -> OResult<'slot, ValueId> {
    let ty = context.element_type(out.reborrow())?;
    let mut result: ChargedVec<ValueId> = context.session().budget_codec(
        CodecStage::Conversion,
        |budget_out| context.session().workspace(budget_out),
        out.reborrow(),
    )?;
    for (index, value) in values.iter().enumerate() {
        let converted = context.encode_indexed_child::<T>(index, ty, value, out.reborrow())?;
        context.session().budget_codec(
            CodecStage::Conversion,
            |budget_out| result.try_push(converted, budget_out),
            out.reborrow(),
        )?;
    }
    context.array(result.as_slice(), out)
}

fn read_sequence<'slot, T: Typol>(
    value: ValueView<'_>,
    context: &mut ConversionContext<'_>,
    mut out: Out<'slot, '_, CodecErrors>,
) -> OResult<'slot, ChargedVec<T>> {
    let ValueNodeKind::Array {
        element: ty,
        values,
        ..
    } = context.read_value_node(value, out.reborrow())?
    else {
        return context.reject_with_path(out.reborrow());
    };
    let mut result = context.session().budget_codec(
        CodecStage::Conversion,
        |budget_out| context.session().workspace(budget_out),
        out.reborrow(),
    )?;
    context.session().budget_codec(
        CodecStage::Conversion,
        |budget_out| result.try_reserve_exact(values.len(), budget_out),
        out.reborrow(),
    )?;
    for (index, value) in values.enumerate() {
        let converted = context.decode_indexed_child::<T>(
            index,
            ty,
            ConversionValue::new(value.value()),
            out.reborrow(),
        )?;
        context.session().budget_codec(
            CodecStage::Conversion,
            |budget_out| result.try_push(converted, budget_out),
            out.reborrow(),
        )?;
    }
    Ok(result)
}

#[cfg(feature = "std")]
fn write_map<'slot, 'value, T: Typol>(
    entries: impl ExactSizeIterator<Item = (&'value String, &'value T)>,
    context: &mut ConversionContext<'_>,
    mut out: Out<'slot, '_, CodecErrors>,
) -> OResult<'slot, ValueId> {
    let ty = context.element_type(out.reborrow())?;
    let mut sorted = context.session().budget_codec(
        CodecStage::Conversion,
        |budget_out| context.session().workspace(budget_out),
        out.reborrow(),
    )?;
    context.session().budget_codec(
        CodecStage::Conversion,
        |budget_out| sorted.try_reserve_exact(entries.len(), budget_out),
        out.reborrow(),
    )?;
    for entry in entries {
        context.session().budget_codec(
            CodecStage::Conversion,
            |budget_out| context.session().visit(budget_out),
            out.reborrow(),
        )?;
        context.session().budget_codec(
            CodecStage::Conversion,
            |budget_out| sorted.try_push(entry, budget_out),
            out.reborrow(),
        )?;
    }
    sorted.sort_by(
        |left, right, sort_out| {
            context.session().budget_codec(
                CodecStage::Conversion,
                |budget_out| context.session().compare_text(left.0, right.0, budget_out),
                sort_out,
            )
        },
        out.reborrow(),
    )?;
    let mut result: ChargedVec<NamedValue<'value>> = context.session().budget_codec(
        CodecStage::Conversion,
        |budget_out| context.session().workspace(budget_out),
        out.reborrow(),
    )?;
    context.session().budget_codec(
        CodecStage::Conversion,
        |budget_out| result.try_reserve_exact(sorted.len(), budget_out),
        out.reborrow(),
    )?;
    for (key, value) in sorted {
        context.child(|child, child_out| child.node(child_out), out.reborrow())?;
        let value = context.encode_child::<T>(ty, value, out.reborrow())?;
        let entry = NamedValue::new(key, value);
        context.session().budget_codec(
            CodecStage::Conversion,
            |budget_out| result.try_push(entry, budget_out),
            out.reborrow(),
        )?;
    }
    context.map(result.as_slice(), out.reborrow())
}
