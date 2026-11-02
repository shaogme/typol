mod access;
mod decode;
mod encode;
mod map_keys;
mod model;
mod serialize_containers;
mod string;

#[cfg(all(test, feature = "std"))]
mod tests;

use crate::{
    binding_impl::TypedSchema,
    descriptor::Typol,
    diagnostics::CodecErrors,
    options::ConversionLimits,
    serde::{
        decode::Decoder,
        encode::Encoder,
        model::{Cursor, Error, Fault, State},
    },
    session::SessionBudget,
};
use core::{cell::Cell, fmt};
use opeo::{OResult, Out, ResultOutExt};
use serde::{Serialize, de::DeserializeOwned};
use typol_core::{ConfigValue, ValueSource, ValueView};

#[cfg(all(test, feature = "std"))]
use typol_semantic::TypeView;

pub(crate) struct SerdeValue {
    value: ConfigValue,
}

pub(crate) struct Serde;

impl Serde {
    pub(crate) fn encode<'slot, 'borrow, T: Typol + Serialize>(
        binding: &TypedSchema<'_, T>,
        value: &T,
        session: &SessionBudget,
        limits: ConversionLimits,
        out: Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, SerdeValue> {
        let fault = Fault::new();
        match fault.run(|mut serde_out| {
            let mut state = State::new(session, limits, &fault, serde_out.reborrow())?;
            let cursor = state.root(binding.schema().entry(), serde_out.reborrow())?;
            let value =
                fault.finish(value.serialize(Encoder::new(cursor)), serde_out.reborrow())?;
            Ok(value)
        }) {
            Ok(value) => Ok(SerdeValue { value }),
            Err(error) => Err(out.fail(error.into_errors())),
        }
    }

    pub(crate) fn decode<'slot, 'borrow, T: Typol + DeserializeOwned>(
        binding: &TypedSchema<'_, T>,
        value: &ConfigValue,
        session: &SessionBudget,
        limits: ConversionLimits,
        out: Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, T> {
        let fault = Fault::new();
        match fault.run(|mut serde_out| {
            let mut state = State::new(session, limits, &fault, serde_out.reborrow())?;
            let cursor = state.root(binding.schema().entry(), serde_out.reborrow())?;
            let value = Self::read(cursor, value.root(), serde_out.reborrow())?;
            fault.complete(value, serde_out.reborrow())
        }) {
            Ok(value) => Ok(value),
            Err(error) => Err(out.fail(error.into_errors())),
        }
    }

    fn read<'slot, T: DeserializeOwned>(
        cursor: Cursor<'_, '_, '_>,
        value: ValueView<'_>,
        mut out: Out<'slot, '_, Error>,
    ) -> OResult<'slot, T> {
        let used = Cell::new(false);
        let result =
            T::deserialize(Decoder::new(cursor, value, &used)).store_err_into(out.reborrow())?;
        if used.get() {
            Ok(result)
        } else {
            Err(out.fail(Error::shape_mismatch()))
        }
    }
}

impl SerdeValue {
    pub(crate) fn into_value(self) -> ConfigValue {
        self.value
    }

    #[cfg(all(test, feature = "std"))]
    pub(crate) fn value(&self) -> &ConfigValue {
        &self.value
    }

    pub(crate) fn origin(&self) -> ValueSource {
        self.value.source()
    }
}

impl fmt::Debug for SerdeValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SerdeValue")
            .field("origin", &self.origin())
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for Serde {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Serde").finish_non_exhaustive()
    }
}

// Internal model tests do not forge full validation credentials; production wiring enters only through a matched binding.
#[cfg(all(test, feature = "std"))]
pub(crate) struct Model;

#[cfg(all(test, feature = "std"))]
impl Model {
    pub(crate) fn encode<'slot, 'borrow, T: Serialize>(
        ty: TypeView<'_>,
        value: &T,
        session: &SessionBudget,
        limits: ConversionLimits,
        out: Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, ConfigValue> {
        let fault = Fault::new();
        match fault.run(|mut serde_out| {
            let mut state = State::new(session, limits, &fault, serde_out.reborrow())?;
            let cursor = state.root(ty, serde_out.reborrow())?;
            let value =
                fault.finish(value.serialize(Encoder::new(cursor)), serde_out.reborrow())?;
            Ok(value)
        }) {
            Ok(value) => Ok(value),
            Err(error) => Err(out.fail(error.into_errors())),
        }
    }

    pub(crate) fn decode<'slot, 'borrow, T: DeserializeOwned>(
        ty: TypeView<'_>,
        value: &ConfigValue,
        session: &SessionBudget,
        limits: ConversionLimits,
        out: Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, T> {
        let fault = Fault::new();
        match fault.run(|mut serde_out| {
            let mut state = State::new(session, limits, &fault, serde_out.reborrow())?;
            let cursor = state.root(ty, serde_out.reborrow())?;
            let value = Serde::read(cursor, value.root(), serde_out.reborrow())?;
            fault.complete(value, serde_out.reborrow())
        }) {
            Ok(value) => Ok(value),
            Err(error) => Err(out.fail(error.into_errors())),
        }
    }
}
