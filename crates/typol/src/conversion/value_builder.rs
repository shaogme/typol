use crate::{
    diagnostics::{CodecErrors, CodecStage},
    session::SessionBudget,
};
use opeo::{OResult, Out};
use typol_core::{
    ConfigValue, ConfigValueBuilder, NamedValue, ValueError, ValueId, ValuePayload, ValueScalar,
    ValueSource,
};

pub(super) struct EnvironmentReferenceInput<'value> {
    values: &'value [ValueId],
    name: &'value str,
    fallback: Option<ValueScalar<'value>>,
}

impl<'value> EnvironmentReferenceInput<'value> {
    pub(super) fn new(
        values: &'value [ValueId],
        name: &'value str,
        fallback: Option<ValueScalar<'value>>,
    ) -> Self {
        Self {
            values,
            name,
            fallback,
        }
    }
}

/// The value arena shared by one binding encode operation.
pub(crate) struct ValueBuilder {
    builder: ConfigValueBuilder,
}

impl ValueBuilder {
    pub(super) fn new<'slot>(
        session: &SessionBudget,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, Self> {
        let builder = session.value_codec(
            CodecStage::Conversion,
            |value_out| ConfigValueBuilder::new(Some(session.execution()), value_out),
            out.reborrow(),
        )?;
        Ok(Self { builder })
    }

    pub(super) fn finish<'slot>(
        self,
        session: &SessionBudget,
        root: ValueId,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ConfigValue> {
        session.value_codec(
            CodecStage::Conversion,
            |value_out| self.builder.finish(root, value_out),
            out.reborrow(),
        )
    }

    pub(super) fn text<'slot>(
        &mut self,
        session: &SessionBudget,
        value: &str,
        out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        self.add(
            session,
            |builder, value_out| builder.add_string(value, ValueSource::RustValue, value_out),
            out,
        )
    }

    pub(super) fn signed<'slot>(
        &mut self,
        session: &SessionBudget,
        value: i128,
        out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        self.add(
            session,
            |builder, value_out| builder.add_signed(value, ValueSource::RustValue, value_out),
            out,
        )
    }

    pub(super) fn unsigned<'slot>(
        &mut self,
        session: &SessionBudget,
        value: u128,
        out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        self.add(
            session,
            |builder, value_out| builder.add_unsigned(value, ValueSource::RustValue, value_out),
            out,
        )
    }

    pub(super) fn float32<'slot>(
        &mut self,
        session: &SessionBudget,
        value: f32,
        out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        self.add(
            session,
            |builder, value_out| builder.add_float32(value, ValueSource::RustValue, value_out),
            out,
        )
    }

    pub(super) fn float64<'slot>(
        &mut self,
        session: &SessionBudget,
        value: f64,
        out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        self.add(
            session,
            |builder, value_out| builder.add_float64(value, ValueSource::RustValue, value_out),
            out,
        )
    }

    pub(super) fn boolean<'slot>(
        &mut self,
        session: &SessionBudget,
        value: bool,
        out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        self.add(
            session,
            |builder, value_out| builder.add_bool(value, ValueSource::RustValue, value_out),
            out,
        )
    }

    pub(super) fn none<'slot>(
        &mut self,
        session: &SessionBudget,
        out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        self.optional(session, None, out)
    }

    pub(super) fn optional<'slot>(
        &mut self,
        session: &SessionBudget,
        value: Option<ValueId>,
        out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        self.add(
            session,
            |builder, value_out| builder.add_optional(value, ValueSource::RustValue, value_out),
            out,
        )
    }

    pub(super) fn array<'slot>(
        &mut self,
        session: &SessionBudget,
        values: &[ValueId],
        out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        self.add(
            session,
            |builder, value_out| builder.add_array(values, ValueSource::RustValue, value_out),
            out,
        )
    }

    pub(super) fn map<'slot>(
        &mut self,
        session: &SessionBudget,
        values: &[NamedValue<'_>],
        out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        self.add(
            session,
            |builder, value_out| builder.add_map(values, ValueSource::RustValue, value_out),
            out,
        )
    }

    pub(super) fn structure<'slot>(
        &mut self,
        session: &SessionBudget,
        fields: &[NamedValue<'_>],
        out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        self.add(
            session,
            |builder, value_out| builder.add_structure(fields, ValueSource::RustValue, value_out),
            out,
        )
    }

    pub(super) fn enumeration<'slot>(
        &mut self,
        session: &SessionBudget,
        variant: &str,
        payload: ValuePayload<'_>,
        out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        self.add(
            session,
            |builder, value_out| {
                builder.add_enumeration(variant, payload, ValueSource::RustValue, value_out)
            },
            out,
        )
    }

    pub(super) fn environment_reference<'slot>(
        &mut self,
        session: &SessionBudget,
        reference: EnvironmentReferenceInput<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ()> {
        session.charge_environment_reference_text(
            reference.name,
            reference.fallback,
            out.reborrow(),
        )?;
        session.value_codec(
            CodecStage::Conversion,
            |value_out| {
                self.builder.add_environment_reference(
                    reference.values,
                    reference.name,
                    reference.fallback,
                    value_out,
                )
            },
            out.reborrow(),
        )
    }

    fn add<'slot>(
        &mut self,
        session: &SessionBudget,
        add: impl for<'value_slot> FnOnce(
            &mut ConfigValueBuilder,
            Out<'value_slot, 'value_slot, ValueError>,
        ) -> OResult<'value_slot, ValueId>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        session.value_codec(
            CodecStage::Conversion,
            |value_out| add(&mut self.builder, value_out),
            out.reborrow(),
        )
    }
}
