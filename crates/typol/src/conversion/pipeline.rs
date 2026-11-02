use crate::{
    binding_impl::{BindingPlan, TypeBinding, TypedSchema, VariantBinding},
    conversion::{ConversionContext, value_builder::ValueBuilder},
    descriptor::Typol,
    diagnostics::{CodecErrors, CodecStage, Failure},
    options::ConversionLimits,
    session::{SessionBudget, TargetCharge},
};
use alloc::string::String;
use core::any::TypeId;
use opeo::{OResult, Out, ResultOutExt};
use typol_core::{
    BudgetError, ConfigValue, DiagnosticContext, ValueEntries, ValuePayloadView, ValueView,
    diagnostic_context,
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
}

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
