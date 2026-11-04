use super::{DescribeContext, DescriptorProvider, MapContainer, TypeDescriptor, TypeShape};
use crate::{
    conversion::{ContainerConversions, ConversionContext, ConversionValue},
    diagnostics::{CodecErrors, ConversionDiagnosticInput, ConversionErrorKind},
    value_walk::ValueNodeKind,
};
use alloc::{string::String, vec::Vec};
use core::mem::size_of;
use opeo::{OResult, Out};
use typol_core::ValueId;
use typol_model::BuiltinType;

#[cfg(feature = "btree-map")]
use alloc::collections::BTreeMap;

#[cfg(feature = "std")]
use std::collections::HashMap as StdHashMap;

#[cfg(feature = "hashbrown")]
use hashbrown::HashMap as HashbrownMap;

/// Conversion contract between Rust values and configuration trees. Each implementation must call `start` once, and decoded children must be dispatched through the context.
/// Derived and built-in implementations route nested conversions through library-budgeted paths; the library cannot limit ordinary allocations, blocking, or arbitrary computation performed directly by handwritten implementations.
/// `from_config_value` receives input validated against the bound Schema, but the library does not validate the returned `Self` again.
/// Handwritten structs can reuse the built-in string conversion through `context.read_field::<String>(...)`; it calls `target_text` to charge target text storage.
/// When a handwritten implementation creates owned text itself, use `target_text` instead of copying the string outside the context.
/// Handwritten implementations must follow this contract themselves; the library cannot prove or enforce compliance. `target_text` and `target_bytes` are recommended accounting APIs, not interceptors for all target storage.
pub trait Typol: Sized + 'static {
    fn describe<'slot, 'borrow>(
        context: &mut DescribeContext<'_>,
        out: Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, TypeDescriptor>;

    fn to_config_value<'slot, 'borrow>(
        &self,
        context: &mut ConversionContext<'_>,
        out: Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, ValueId>;

    fn from_config_value<'slot, 'borrow>(
        value: ConversionValue<'_>,
        context: &mut ConversionContext<'_>,
        out: Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, Self>;
}

/// This hidden capability marker selects the library-managed binding API path. It is not a
/// sandbox or a security boundary; handwritten Rust code can still be implemented and trusted
/// explicitly through `bind_trusted`.
#[doc(hidden)]
pub trait BudgetedTypol: Typol {}

macro_rules! integer {
    ($rust:ty, $builtin:ident, $view:ident, $constructor:ident, $wide:ty) => {
        impl Typol for $rust {
            fn describe<'slot, 'borrow>(
                context: &mut DescribeContext<'_>,
                out: Out<'slot, 'borrow, CodecErrors>,
            ) -> OResult<'slot, TypeDescriptor> {
                context.scalar::<Self>(BuiltinType::$builtin, context.depth(), out)
            }

            fn to_config_value<'slot>(
                &self,
                context: &mut ConversionContext<'_>,
                mut out: Out<'slot, '_, CodecErrors>,
            ) -> OResult<'slot, ValueId> {
                context.start::<Self>(out.reborrow())?;
                context.$constructor(*self as $wide, out.reborrow())
            }

            fn from_config_value<'slot>(
                value: ConversionValue<'_>,
                context: &mut ConversionContext<'_>,
                mut out: Out<'slot, '_, CodecErrors>,
            ) -> OResult<'slot, Self> {
                context.start::<Self>(out.reborrow())?;
                match context.read_value_node(value.view(), out.reborrow())? {
                    ValueNodeKind::$view { builtin, value } if builtin == BuiltinType::$builtin => {
                        match Self::try_from(value) {
                            Ok(value) => Ok(value),
                            Err(_) => context.fail_as(
                                ConversionDiagnosticInput::new(
                                    ConversionErrorKind::ValueOutOfRange,
                                    stringify!($rust),
                                    "integer outside the target range",
                                    context.current_path(),
                                ),
                                out.reborrow(),
                            ),
                        }
                    }
                    _ => context.reject_with_path(out.reborrow()),
                }
            }
        }

        impl BudgetedTypol for $rust {}
    };
}

integer!(i8, I8, Signed, signed, i128);
integer!(i16, I16, Signed, signed, i128);
integer!(i32, I32, Signed, signed, i128);
integer!(i64, I64, Signed, signed, i128);
integer!(i128, I128, Signed, signed, i128);
integer!(u8, U8, Unsigned, unsigned, u128);
integer!(u16, U16, Unsigned, unsigned, u128);
integer!(u32, U32, Unsigned, unsigned, u128);
integer!(u64, U64, Unsigned, unsigned, u128);
integer!(u128, U128, Unsigned, unsigned, u128);

macro_rules! floating {
    ($rust:ty, $builtin:ident, $view:ident, $constructor:ident) => {
        impl Typol for $rust {
            fn describe<'slot, 'borrow>(
                context: &mut DescribeContext<'_>,
                out: Out<'slot, 'borrow, CodecErrors>,
            ) -> OResult<'slot, TypeDescriptor> {
                context.scalar::<Self>(BuiltinType::$builtin, context.depth(), out)
            }

            fn to_config_value<'slot>(
                &self,
                context: &mut ConversionContext<'_>,
                mut out: Out<'slot, '_, CodecErrors>,
            ) -> OResult<'slot, ValueId> {
                context.start::<Self>(out.reborrow())?;
                if !self.is_finite() {
                    return context.fail_as(
                        ConversionDiagnosticInput::new(
                            ConversionErrorKind::InvalidScalar,
                            concat!("finite ", stringify!($rust)),
                            "non-finite floating-point value",
                            context.current_path(),
                        ),
                        out.reborrow(),
                    );
                }
                context.$constructor(*self, out.reborrow())
            }

            fn from_config_value<'slot>(
                value: ConversionValue<'_>,
                context: &mut ConversionContext<'_>,
                mut out: Out<'slot, '_, CodecErrors>,
            ) -> OResult<'slot, Self> {
                context.start::<Self>(out.reborrow())?;
                match context.read_value_node(value.view(), out.reborrow())? {
                    ValueNodeKind::$view(value) if value.is_finite() => Ok(value),
                    ValueNodeKind::$view(_) => context.fail_as(
                        ConversionDiagnosticInput::new(
                            ConversionErrorKind::InvalidScalar,
                            concat!("finite ", stringify!($rust)),
                            "non-finite floating-point value",
                            context.current_path(),
                        ),
                        out.reborrow(),
                    ),
                    _ => context.reject_with_path(out.reborrow()),
                }
            }
        }

        impl BudgetedTypol for $rust {}
    };
}

floating!(f32, F32, Float32, float32);
floating!(f64, F64, Float64, float64);

impl Typol for bool {
    fn describe<'slot, 'borrow>(
        context: &mut DescribeContext<'_>,
        out: Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, TypeDescriptor> {
        context.scalar::<Self>(BuiltinType::Bool, context.depth(), out)
    }

    fn to_config_value<'slot>(
        &self,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        context.start::<Self>(out.reborrow())?;
        context.boolean(*self, out.reborrow())
    }

    fn from_config_value<'slot>(
        value: ConversionValue<'_>,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, Self> {
        context.start::<Self>(out.reborrow())?;
        match context.read_value_node(value.view(), out.reborrow())? {
            ValueNodeKind::Bool(value) => Ok(value),
            _ => context.reject_with_path(out.reborrow()),
        }
    }
}

impl BudgetedTypol for bool {}

impl Typol for String {
    fn describe<'slot, 'borrow>(
        context: &mut DescribeContext<'_>,
        out: Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, TypeDescriptor> {
        context.scalar::<Self>(BuiltinType::String, context.depth(), out)
    }

    fn to_config_value<'slot>(
        &self,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        context.start::<Self>(out.reborrow())?;
        context.text(self, out.reborrow())
    }

    fn from_config_value<'slot>(
        value: ConversionValue<'_>,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, Self> {
        context.start::<Self>(out.reborrow())?;
        match context.read_value_node(value.view(), out.reborrow())? {
            ValueNodeKind::String(value) => context.target_text(value, out.reborrow()),
            _ => context.reject_with_path(out.reborrow()),
        }
    }
}

impl BudgetedTypol for String {}

impl<T: Typol> Typol for Option<T> {
    fn describe<'slot, 'borrow>(
        context: &mut DescribeContext<'_>,
        out: Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, TypeDescriptor> {
        ContainerConversions::describe_container::<Self>(
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
        ContainerConversions::describe_container::<Self>(
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
        ContainerConversions::write_sequence(self, context, out.reborrow())
    }

    fn from_config_value<'slot>(
        value: ConversionValue<'_>,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, Self> {
        context.start::<Self>(out.reborrow())?;
        let values =
            ContainerConversions::read_sequence::<T>(value.view(), context, out.reborrow())?
                .into_unmetered_vec();
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
        ContainerConversions::describe_container::<Self>(
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
        ContainerConversions::write_sequence(self, context, out.reborrow())
    }

    fn from_config_value<'slot>(
        value: ConversionValue<'_>,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, Self> {
        context.start::<Self>(out.reborrow())?;
        let values =
            ContainerConversions::read_sequence::<T>(value.view(), context, out.reborrow())?
                .into_unmetered_vec();
        match values.try_into() {
            Ok(values) => Ok(values),
            Err(_) => context.fail_as(
                ConversionDiagnosticInput::new(
                    ConversionErrorKind::ArrayLengthMismatch,
                    "value with the Rust fixed-array length",
                    "array length mismatch",
                    context.current_path(),
                ),
                out.reborrow(),
            ),
        }
    }
}

impl<T: BudgetedTypol, const N: usize> BudgetedTypol for [T; N] {}

#[cfg(feature = "btree-map")]
impl<T: Typol> Typol for BTreeMap<String, T> {
    fn describe<'slot, 'borrow>(
        context: &mut DescribeContext<'_>,
        out: Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, TypeDescriptor> {
        ContainerConversions::describe_container::<Self>(
            context,
            TypeShape::Map {
                container: MapContainer::BTreeMap,
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
        ContainerConversions::write_map(
            self.iter().map(|(key, value)| (key.as_str(), value)),
            context,
            out.reborrow(),
        )
    }

    fn from_config_value<'slot>(
        value: ConversionValue<'_>,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, Self> {
        context.start::<Self>(out.reborrow())?;
        ContainerConversions::read_btree_map(value, context, out.reborrow())
    }
}

#[cfg(feature = "btree-map")]
impl<T: BudgetedTypol> BudgetedTypol for BTreeMap<String, T> {}

#[cfg(feature = "std")]
impl<T: Typol> Typol for StdHashMap<String, T> {
    fn describe<'slot, 'borrow>(
        context: &mut DescribeContext<'_>,
        out: Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, TypeDescriptor> {
        ContainerConversions::describe_container::<Self>(
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
        ContainerConversions::write_map(
            self.iter().map(|(key, value)| (key.as_str(), value)),
            context,
            out.reborrow(),
        )
    }

    fn from_config_value<'slot>(
        value: ConversionValue<'_>,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, Self> {
        context.start::<Self>(out.reborrow())?;
        ContainerConversions::read_std_hash_map::<T>(value, context, out.reborrow())
    }
}

#[cfg(feature = "std")]
impl<T: BudgetedTypol> BudgetedTypol for StdHashMap<String, T> {}

#[cfg(feature = "hashbrown")]
impl<T: Typol> Typol for HashbrownMap<String, T> {
    fn describe<'slot, 'borrow>(
        context: &mut DescribeContext<'_>,
        out: Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, TypeDescriptor> {
        ContainerConversions::describe_container::<Self>(
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
        ContainerConversions::write_map(
            self.iter().map(|(key, value)| (key.as_str(), value)),
            context,
            out.reborrow(),
        )
    }

    fn from_config_value<'slot>(
        value: ConversionValue<'_>,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, Self> {
        context.start::<Self>(out.reborrow())?;
        ContainerConversions::read_hashbrown_map::<T>(value, context, out.reborrow())
    }
}

#[cfg(feature = "hashbrown")]
impl<T: BudgetedTypol> BudgetedTypol for HashbrownMap<String, T> {}
