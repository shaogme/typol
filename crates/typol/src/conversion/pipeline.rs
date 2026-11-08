use crate::{
    binding_impl::{BindingPlan, TypeBinding, TypedSchema, VariantBinding},
    conversion::{ConversionContext, value_builder::ValueBuilder},
    descriptor::{
        BudgetedTypol, DescribeContext, DescriptorNode, DescriptorProvider, TypeDescriptor,
        TypeShape, Typol,
    },
    diagnostics::{CodecErrors, CodecStage, Failure},
    options::ConversionLimits,
    session::{SessionBudget, TargetCharge},
};
use alloc::string::String;
use core::any::TypeId;
use opeo::{OResult, Out, ResultOutExt};
use typol_core::{
    BudgetError, ConfigValue, DiagnosticContext, ValueEntries, ValueId, ValuePayloadView,
    ValueScalar, ValueSource, ValueView, diagnostic_context,
};
use typol_semantic::TypeView;

static CONVERSION_BINDING_INVARIANT: DiagnosticContext =
    diagnostic_context!("typol", "conversion", "conversion-binding-invariant",);
static CONVERSION_STATE_MISSING: DiagnosticContext =
    diagnostic_context!("typol", "conversion", "conversion-target-state-missing",);

/// An input node handle managed by the conversion context.
#[derive(Clone, Copy)]
pub struct ConversionValue<'value> {
    value: ValueView<'value>,
}

impl<'value> ConversionValue<'value> {
    pub(crate) const fn new(value: ValueView<'value>) -> Self {
        Self { value }
    }

    pub(crate) const fn view(self) -> ValueView<'value> {
        self.value
    }

    pub const fn source(self) -> ValueSource {
        self.value.source()
    }
}

pub enum EnvironmentFallback {
    Signed(i128),
    Unsigned(u128),
    Float32(f32),
    Float64(f64),
    String(String),
    Bool(bool),
}

struct OwnedEnvironmentReference {
    name: String,
    fallback: Option<EnvironmentFallback>,
}

/// A decoded value paired with the origin of its configuration node.
///
/// The wrapper does not expose `Debug`, so it cannot accidentally reveal a wrapped secret.
pub struct Sourced<T> {
    value: T,
    source: ValueSource,
    environment: Option<OwnedEnvironmentReference>,
}

mod environment_scalar_sealed {
    pub trait Sealed {}
}

pub trait EnvironmentScalar: Typol + environment_scalar_sealed::Sealed {
    #[doc(hidden)]
    fn as_value_scalar(&self) -> ValueScalar<'_>;

    #[doc(hidden)]
    fn into_environment_fallback(self) -> EnvironmentFallback;
}

mod environment_value_sealed {
    pub trait Sealed {}
}

pub trait EnvironmentSourcedValue: Typol + environment_value_sealed::Sealed {
    type Scalar: EnvironmentScalar;
}

impl<T: EnvironmentSourcedValue> Sourced<T> {
    pub fn from_environment(name: String, value: T) -> Self {
        Self {
            value,
            source: ValueSource::EnvironmentVariable,
            environment: Some(OwnedEnvironmentReference {
                name,
                fallback: None,
            }),
        }
    }

    pub fn from_environment_with_fallback(name: String, value: T, fallback: T::Scalar) -> Self {
        Self {
            value,
            source: ValueSource::EnvironmentVariable,
            environment: Some(OwnedEnvironmentReference {
                name,
                fallback: Some(fallback.into_environment_fallback()),
            }),
        }
    }
}

impl<T: EnvironmentScalar> environment_value_sealed::Sealed for T {}

impl<T: EnvironmentScalar> EnvironmentSourcedValue for T {
    type Scalar = T;
}

impl<T: EnvironmentScalar> environment_value_sealed::Sealed for Option<T> {}

impl<T: EnvironmentScalar> EnvironmentSourcedValue for Option<T> {
    type Scalar = T;
}

impl EnvironmentScalar for String {
    fn as_value_scalar(&self) -> ValueScalar<'_> {
        ValueScalar::String(self)
    }

    fn into_environment_fallback(self) -> EnvironmentFallback {
        EnvironmentFallback::String(self)
    }
}

impl environment_scalar_sealed::Sealed for String {}

impl EnvironmentScalar for bool {
    fn as_value_scalar(&self) -> ValueScalar<'_> {
        ValueScalar::Bool(*self)
    }

    fn into_environment_fallback(self) -> EnvironmentFallback {
        EnvironmentFallback::Bool(self)
    }
}

impl environment_scalar_sealed::Sealed for bool {}

macro_rules! signed_environment_scalar {
    ($($type:ty),+ $(,)?) => {
        $(
            impl environment_scalar_sealed::Sealed for $type {}

            impl EnvironmentScalar for $type {
                fn as_value_scalar(&self) -> ValueScalar<'_> {
                    ValueScalar::Signed(i128::from(*self))
                }

                fn into_environment_fallback(self) -> EnvironmentFallback {
                    EnvironmentFallback::Signed(i128::from(self))
                }

            }
        )+
    };
}

macro_rules! unsigned_environment_scalar {
    ($($type:ty),+ $(,)?) => {
        $(
            impl environment_scalar_sealed::Sealed for $type {}

            impl EnvironmentScalar for $type {
                fn as_value_scalar(&self) -> ValueScalar<'_> {
                    ValueScalar::Unsigned(u128::from(*self))
                }

                fn into_environment_fallback(self) -> EnvironmentFallback {
                    EnvironmentFallback::Unsigned(u128::from(self))
                }

            }
        )+
    };
}

signed_environment_scalar!(i8, i16, i32, i64, i128);
unsigned_environment_scalar!(u8, u16, u32, u64, u128);

impl environment_scalar_sealed::Sealed for f32 {}

impl EnvironmentScalar for f32 {
    fn as_value_scalar(&self) -> ValueScalar<'_> {
        ValueScalar::Float32(*self)
    }

    fn into_environment_fallback(self) -> EnvironmentFallback {
        EnvironmentFallback::Float32(self)
    }
}

impl environment_scalar_sealed::Sealed for f64 {}

impl EnvironmentScalar for f64 {
    fn as_value_scalar(&self) -> ValueScalar<'_> {
        ValueScalar::Float64(*self)
    }

    fn into_environment_fallback(self) -> EnvironmentFallback {
        EnvironmentFallback::Float64(self)
    }
}

impl<T> Sourced<T> {
    pub const fn value(&self) -> &T {
        &self.value
    }

    pub const fn source(&self) -> ValueSource {
        self.source
    }

    pub fn environment_name(&self) -> Option<&str> {
        self.environment
            .as_ref()
            .map(|reference| reference.name.as_str())
    }

    pub fn environment_fallback(&self) -> Option<&EnvironmentFallback> {
        self.environment
            .as_ref()
            .and_then(|reference| reference.fallback.as_ref())
    }

    pub fn into_value(self) -> T {
        self.value
    }
}

impl<T: Typol> Typol for Sourced<T> {
    fn describe<'slot, 'borrow>(
        context: &mut DescribeContext<'_>,
        out: Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, TypeDescriptor> {
        context.insert(
            DescriptorNode::Type {
                identity: TypeId::of::<Self>(),
                name: None,
                shape: TypeShape::Transparent(DescriptorProvider::of::<T>()),
            },
            context.depth(),
            out,
        )
    }

    fn to_config_value<'slot>(
        &self,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        context.start::<Self>(out.reborrow())?;
        let ty = context.current_type(out.reborrow())?;
        let value = context.encode_child::<T>(ty, &self.value, out.reborrow())?;
        if let Some(reference) = &self.environment {
            let fallback = reference
                .fallback
                .as_ref()
                .map(EnvironmentFallback::as_value_scalar);
            context.add_environment_reference(
                &[value],
                &reference.name,
                fallback,
                out.reborrow(),
            )?;
        }
        Ok(value)
    }

    fn from_config_value<'slot>(
        value: ConversionValue<'_>,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, Self> {
        context.start::<Self>(out.reborrow())?;
        let source = value.source();
        let environment = value
            .view()
            .environment_reference()
            .map(|reference| {
                let name = context.target_text(reference.name(), out.reborrow())?;
                let fallback = reference
                    .fallback()
                    .copied()
                    .map(|fallback| match fallback {
                        ValueScalar::Signed(value) => Ok(EnvironmentFallback::Signed(value)),
                        ValueScalar::Unsigned(value) => Ok(EnvironmentFallback::Unsigned(value)),
                        ValueScalar::Float32(value) => Ok(EnvironmentFallback::Float32(value)),
                        ValueScalar::Float64(value) => Ok(EnvironmentFallback::Float64(value)),
                        ValueScalar::String(value) => context
                            .target_text(value, out.reborrow())
                            .map(EnvironmentFallback::String),
                        ValueScalar::Bool(value) => Ok(EnvironmentFallback::Bool(value)),
                    })
                    .transpose()?;
                Ok(OwnedEnvironmentReference { name, fallback })
            })
            .transpose()?;
        let ty = context.current_type(out.reborrow())?;
        let value = context.decode_child::<T>(ty, value, out.reborrow())?;
        Ok(Self {
            value,
            source,
            environment,
        })
    }
}

impl EnvironmentFallback {
    fn as_value_scalar(&self) -> ValueScalar<'_> {
        match self {
            Self::Signed(value) => ValueScalar::Signed(*value),
            Self::Unsigned(value) => ValueScalar::Unsigned(*value),
            Self::Float32(value) => ValueScalar::Float32(*value),
            Self::Float64(value) => ValueScalar::Float64(*value),
            Self::String(value) => ValueScalar::String(value),
            Self::Bool(value) => ValueScalar::Bool(*value),
        }
    }
}

impl<T: BudgetedTypol> BudgetedTypol for Sourced<T> {}

/// A structure field set validated by the context.
#[derive(Clone, Copy)]
pub struct ConversionFields<'value> {
    values: ValueEntries<'value>,
}

impl<'value> ConversionFields<'value> {
    pub(crate) const fn new(values: ValueEntries<'value>) -> Self {
        Self { values }
    }

    pub(crate) const fn entries(self) -> ValueEntries<'value> {
        self.values
    }
}

/// A controlled input shape for an enum payload.
#[derive(Clone, Copy)]
pub enum ConversionPayload<'value> {
    Unit,
    Single(ConversionValue<'value>),
    Structure(ConversionFields<'value>),
}

impl<'value> ConversionPayload<'value> {
    pub(crate) fn from_view(payload: ValuePayloadView<'value>) -> Self {
        match payload {
            ValuePayloadView::Unit => Self::Unit,
            ValuePayloadView::Single(value) => Self::Single(ConversionValue::new(value)),
            ValuePayloadView::Structure(values) => Self::Structure(ConversionFields::new(values)),
        }
    }
}

pub struct EnumInput<'value> {
    pub rust_slot: usize,
    pub payload: ConversionPayload<'value>,
}

#[derive(Clone, Copy)]
pub(crate) struct BindingCursor<'call> {
    pub(super) plan: &'call BindingPlan<'call>,
    pub(super) ty: TypeView<'call>,
    pub(super) identity: TypeId,
}

#[derive(Clone, Copy)]
pub(crate) enum BindingState<'call> {
    Unbound,
    Type(BindingCursor<'call>),
    Variant {
        cursor: BindingCursor<'call>,
        slot: usize,
    },
}

pub(crate) struct ResolvedBinding<'context, 'schema> {
    pub(super) binding: &'context TypeBinding<'schema>,
    pub(super) variant: Option<&'context VariantBinding<'schema>>,
}

pub(crate) struct ConversionState {
    target: TargetCharge,
}

impl ConversionState {
    pub(super) fn new(target: TargetCharge) -> Self {
        Self { target }
    }

    pub(super) fn add<'slot, 'borrow>(
        &mut self,
        bytes: usize,
        out: Out<'slot, 'borrow, BudgetError>,
    ) -> OResult<'slot, ()> {
        self.target.add(bytes, out)
    }

    pub(super) fn target_bytes<'slot>(
        state: Option<&mut Self>,
        session: &SessionBudget,
        bytes: usize,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ()> {
        let state = state
            .ok_or_else(|| {
                CodecErrors::emergency(
                    CodecStage::Conversion,
                    Failure::internal(CONVERSION_STATE_MISSING),
                )
            })
            .store_err(out.reborrow())?;
        session.budget_codec(
            CodecStage::Conversion,
            |budget_out| state.add(bytes, budget_out),
            out.reborrow(),
        )
    }

    pub(super) fn target_text<'slot>(
        state: Option<&mut Self>,
        session: &SessionBudget,
        text: &str,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, String> {
        let state = state
            .ok_or_else(|| {
                CodecErrors::emergency(
                    CodecStage::Conversion,
                    Failure::internal(CONVERSION_STATE_MISSING),
                )
            })
            .store_err(out.reborrow())?;
        session.budget_codec(
            CodecStage::Conversion,
            |budget_out| session.copy_target_text(&mut state.target, text, budget_out),
            out.reborrow(),
        )
    }
}

pub(crate) struct ConvertedValue {
    value: ConfigValue,
}

impl ConvertedValue {
    pub(super) fn new(value: ConfigValue) -> Self {
        Self { value }
    }

    pub(crate) fn into_value(self) -> ConfigValue {
        self.value
    }

    #[cfg(all(test, feature = "std"))]
    pub(crate) fn value(&self) -> &ConfigValue {
        &self.value
    }
}

pub struct Conversion;

impl Conversion {
    pub(crate) fn internal_error() -> CodecErrors {
        CodecErrors::emergency(
            CodecStage::Conversion,
            Failure::internal(CONVERSION_BINDING_INVARIANT),
        )
    }

    pub(crate) fn encode<'slot, T: Typol>(
        binding: &TypedSchema<'_, T>,
        value: &T,
        session: &SessionBudget,
        limits: ConversionLimits,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ConvertedValue> {
        let mut nodes = ConversionContext::counters(limits);
        let mut state = ConversionState::new(session.budget_codec(
            CodecStage::Conversion,
            |budget_out| TargetCharge::new(session, budget_out),
            out.reborrow(),
        )?);
        let mut builder = ValueBuilder::new(session, out.reborrow())?;
        let root = {
            let mut context = ConversionContext::encoding(session, &mut builder, &mut nodes);
            context.bind(binding, &mut state, out.reborrow())?;
            context.encode_here(value, out.reborrow())?
        };
        let value = builder.finish(session, root, out.reborrow())?;
        Ok(ConvertedValue::new(value))
    }

    pub(crate) fn decode<'slot, T: Typol>(
        binding: &TypedSchema<'_, T>,
        value: &ConfigValue,
        session: &SessionBudget,
        limits: ConversionLimits,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, T> {
        let mut nodes = ConversionContext::counters(limits);
        let mut state = ConversionState::new(session.budget_codec(
            CodecStage::Conversion,
            |budget_out| TargetCharge::new(session, budget_out),
            out.reborrow(),
        )?);
        let mut context = ConversionContext::new(session, &mut nodes);
        context.bind(binding, &mut state, out.reborrow())?;
        context.decode_here(ConversionValue::new(value.root()), out.reborrow())
    }
}
