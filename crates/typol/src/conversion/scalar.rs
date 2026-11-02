use crate::{
    conversion::{ConversionContext, ConversionValue},
    descriptor::{BudgetedTypol, DescribeContext, TypeDescriptor, Typol},
    diagnostics::{CodecErrors, ConversionDiagnosticInput, ConversionErrorKind},
    value_walk::ValueNodeKind,
};
use alloc::string::String;
use opeo::{OResult, Out};
use typol_core::ValueId;
use typol_model::BuiltinType;

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
                                    context.path,
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
                            context.path,
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
                            context.path,
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
