mod encoder;
mod literals;

#[cfg(all(test, feature = "std"))]
mod tests;

use crate::{
    conversion::ConvertedValue,
    diagnostics::{CodecErrors, CodecStage},
    session::SessionBudget,
};
use alloc::string::String;
use core::fmt;
use encoder::Encoder;
use opeo::{OResult, Out};
use typol_config::{
    ConfigParseRequest, ConfigParser, ConfigValidationOptions, ConfigValidationRequest,
    ConfigValueProcessor, Environment, ValidatedConfigValue,
};
use typol_core::ConfigValue;
use typol_semantic::{CompiledSchema, TypeKind};

#[cfg(feature = "serde")]
use crate::serde::SerdeValue;

#[cfg(all(test, feature = "std"))]
use typol_config::ConfigParseErrors;

enum EncodedValue {
    Rust(ConvertedValue),
    #[cfg(feature = "serde")]
    Serde(SerdeValue),
}

pub(crate) struct SchemaCompleteValue(EncodedValue);

// The encoder wrapper preserves its trusted origin until validation selects the copy-free path.
impl SchemaCompleteValue {
    pub(crate) fn rust(value: ConvertedValue) -> Self {
        Self(EncodedValue::Rust(value))
    }

    #[cfg(feature = "serde")]
    pub(crate) fn serde(value: SerdeValue) -> Self {
        Self(EncodedValue::Serde(value))
    }

    fn into_config_value(self) -> ConfigValue {
        match self.0 {
            EncodedValue::Rust(value) => value.into_value(),
            #[cfg(feature = "serde")]
            EncodedValue::Serde(value) => value.into_value(),
        }
    }
}

// The credential owns the value and is tied to this session; callers cannot replace its Schema or budget.
pub(crate) struct ValidatedValue<'schema, 'call> {
    schema: &'schema CompiledSchema,
    value: ValidatedConfigValue,
    session: &'call mut SessionBudget,
}

impl ValidatedValue<'_, '_> {
    pub(crate) fn validate<'slot, 'schema, 'call>(
        schema: &'schema CompiledSchema,
        value: SchemaCompleteValue,
        options: ConfigValidationOptions,
        session: &'call mut SessionBudget,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValidatedValue<'schema, 'call>> {
        let value = value.into_config_value();
        let value = session.validation_codec(
            |validation_out| {
                ConfigValueProcessor::new(schema).validate_schema_complete_with_budget(
                    value,
                    ConfigValidationRequest::new(options),
                    session.execution(),
                    validation_out,
                )
            },
            out.reborrow(),
        )?;
        Ok(ValidatedValue {
            schema,
            value,
            session,
        })
    }

    pub(crate) fn parse_value<'slot>(
        schema: &CompiledSchema,
        request: ConfigParseRequest<'_>,
        environment: &mut dyn Environment,
        session: &SessionBudget,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValidatedConfigValue> {
        let value = session.parse_codec(
            |parse_out| {
                ConfigParser::new(schema).parse_with_budget(
                    request,
                    environment,
                    session.execution(),
                    parse_out,
                )
            },
            out.reborrow(),
        )?;
        Ok(value)
    }

    #[cfg(all(test, feature = "std"))]
    pub(crate) fn value(&self) -> &ConfigValue {
        self.value.as_config_value()
    }

    pub(crate) fn encode<'slot>(
        &mut self,
        pretty: bool,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, String> {
        let entry = self.schema.entry();
        let kind = self.session.schema_codec(
            CodecStage::Encoding,
            |schema_out| entry.kind(schema_out),
            out.reborrow(),
        )?;
        if !matches!(kind, TypeKind::Struct { .. }) {
            return Err(out.fail(Encoder::invariant_error()));
        }
        Encoder::run(
            entry,
            self.value.as_config_value(),
            self.session,
            pretty,
            out.reborrow(),
        )
    }
}

impl fmt::Debug for ValidatedValue<'_, '_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ValidatedValue")
            .finish_non_exhaustive()
    }
}

#[cfg(all(test, feature = "std"))]
impl<'schema, 'call> ValidatedValue<'schema, 'call> {
    pub(crate) fn parse<'slot>(
        schema: &'schema CompiledSchema,
        request: ConfigParseRequest<'_>,
        environment: &mut dyn Environment,
        session: &'call mut SessionBudget,
        mut out: Out<'slot, '_, ConfigParseErrors>,
    ) -> OResult<'slot, Self> {
        let value = ConfigParser::new(schema).parse(request, environment, out.reborrow())?;
        Ok(Self {
            schema,
            value,
            session,
        })
    }
}
