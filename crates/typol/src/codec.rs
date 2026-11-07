mod encoder;
mod literals;

#[cfg(all(test, feature = "std"))]
mod tests;

use crate::{
    conversion::ConvertedValue,
    diagnostics::{CodecErrors, CodecStage, Failure},
    session::SessionBudget,
};
use core::fmt;
use encoder::{Encoder, EncoderOutput};
use opeo::{OResult, Out};
use typol_config::{
    ConfigParseRequest, ConfigParser, ConfigValidationOptions, ConfigValidationRequest,
    ConfigValueProcessor, Environment, ValidatedConfigValue,
};
use typol_core::{
    ChargedVec, ConfigValue, ConfigValueBuilder, DiagnosticContext, Read, ValueEntries, ValueKind,
    ValuePayloadView, ValueSource, ValueView, Write, diagnostic_context,
};
use typol_semantic::{CompiledSchema, FieldList, TypeKind, TypeView, VariantPayloadInfo};

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

struct SecretValueTask<'schema, 'value> {
    ty: TypeView<'schema>,
    value: ValueView<'value>,
}

struct SecretValueChecker;

static SECRET_VALUE_SHAPE: DiagnosticContext =
    diagnostic_context!("typol", "encoding", "secret-check-value-shape-mismatch");

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

    pub(crate) fn default_config<'slot, 'schema, 'call>(
        schema: &'schema CompiledSchema,
        options: ConfigValidationOptions,
        session: &'call mut SessionBudget,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValidatedValue<'schema, 'call>> {
        let execution = session.execution();
        let value = session.value_codec(
            CodecStage::Validation,
            |mut value_out| {
                let mut builder = ConfigValueBuilder::new(Some(execution), value_out.reborrow())?;
                let root =
                    builder.add_structure(&[], ValueSource::RustValue, value_out.reborrow())?;
                builder.finish(root, value_out)
            },
            out.reborrow(),
        )?;
        let value = session.validation_codec(
            |validation_out| {
                ConfigValueProcessor::new(schema).validate_with_budget(
                    value,
                    ConfigValidationRequest::new(options),
                    execution,
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

    pub(crate) fn contains_secret_values<'slot>(
        &self,
        out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, bool> {
        SecretValueChecker::contains(self.schema, self.value.as_config_value(), self.session, out)
    }

    pub(crate) fn parse_value<'slot, R: Read + ?Sized>(
        schema: &CompiledSchema,
        request: ConfigParseRequest<'_, R>,
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
        output: &mut dyn Write,
        pretty: bool,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ()> {
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
            EncoderOutput {
                sink: output,
                pretty,
            },
            out.reborrow(),
        )
    }
}

impl SecretValueChecker {
    fn contains<'slot>(
        schema: &CompiledSchema,
        value: &ConfigValue,
        session: &SessionBudget,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, bool> {
        let mut tasks = session.budget_codec(
            CodecStage::Encoding,
            |budget_out| session.workspace(budget_out),
            out.reborrow(),
        )?;
        let entry = schema.entry();
        let root = value.root();
        session.budget_codec(
            CodecStage::Encoding,
            |budget_out| {
                tasks.try_push(
                    SecretValueTask {
                        ty: entry,
                        value: root,
                    },
                    budget_out,
                )
            },
            out.reborrow(),
        )?;
        'tasks: while let Some(task) = tasks.pop() {
            session.budget_codec(
                CodecStage::Encoding,
                |budget_out| session.visit(budget_out),
                out.reborrow(),
            )?;
            let mut ty = task.ty;
            let mut value = Some(task.value);
            let kind = loop {
                if session.schema_codec(
                    CodecStage::Encoding,
                    |schema_out| ty.is_secret(schema_out),
                    out.reborrow(),
                )? {
                    return Ok(true);
                }
                match session.schema_codec(
                    CodecStage::Encoding,
                    |schema_out| ty.kind(schema_out),
                    out.reborrow(),
                )? {
                    TypeKind::Option { inner } => {
                        value = match value {
                            Some(current) => match current.as_ref() {
                                ValueKind::Optional(Some(child)) => Some(child),
                                ValueKind::Optional(None) => continue 'tasks,
                                _ => {
                                    return Err(out.fail(CodecErrors::emergency(
                                        CodecStage::Encoding,
                                        Failure::internal(SECRET_VALUE_SHAPE),
                                    )));
                                }
                            },
                            None => None,
                        };
                        ty = inner;
                    }
                    kind => break kind,
                }
            };
            let Some(value) = value else {
                continue;
            };
            match (kind, value.as_ref()) {
                (TypeKind::Array { element, .. }, ValueKind::Array(values)) => {
                    for entry in values {
                        Self::push(&mut tasks, element, entry.value(), session, out.reborrow())?;
                    }
                }
                (TypeKind::Map { value: value_type }, ValueKind::Map(values)) => {
                    for entry in values {
                        let Some(entry) = entry.as_named() else {
                            return Err(out.fail(CodecErrors::emergency(
                                CodecStage::Encoding,
                                Failure::internal(SECRET_VALUE_SHAPE),
                            )));
                        };
                        Self::push(
                            &mut tasks,
                            value_type,
                            entry.value(),
                            session,
                            out.reborrow(),
                        )?;
                    }
                }
                (TypeKind::Struct { fields }, ValueKind::Structure(values)) => {
                    if Self::fields_have_secrets(
                        fields,
                        values,
                        &mut tasks,
                        session,
                        out.reborrow(),
                    )? {
                        return Ok(true);
                    }
                }
                (TypeKind::Enum { .. }, ValueKind::Enumeration { variant, payload }) => {
                    let variant = session.budget_codec(
                        CodecStage::Encoding,
                        |budget_out| {
                            ty.variant_named_budgeted(variant, session.execution(), budget_out)
                        },
                        out.reborrow(),
                    )?;
                    let Some(variant) = variant else {
                        return Err(out.fail(CodecErrors::emergency(
                            CodecStage::Encoding,
                            Failure::internal(SECRET_VALUE_SHAPE),
                        )));
                    };
                    if session.schema_codec(
                        CodecStage::Encoding,
                        |schema_out| variant.is_secret(schema_out),
                        out.reborrow(),
                    )? {
                        return Ok(true);
                    }
                    let payload_kind = session.schema_codec(
                        CodecStage::Encoding,
                        |schema_out| variant.payload(schema_out),
                        out.reborrow(),
                    )?;
                    match (payload_kind, payload) {
                        (VariantPayloadInfo::Unit, ValuePayloadView::Unit) => {}
                        (VariantPayloadInfo::Single { ty }, ValuePayloadView::Single(value)) => {
                            Self::push(&mut tasks, ty, value, session, out.reborrow())?
                        }
                        (
                            VariantPayloadInfo::Structure { fields },
                            ValuePayloadView::Structure(values),
                        ) => {
                            if Self::fields_have_secrets(
                                fields,
                                values,
                                &mut tasks,
                                session,
                                out.reborrow(),
                            )? {
                                return Ok(true);
                            }
                        }
                        _ => {
                            return Err(out.fail(CodecErrors::emergency(
                                CodecStage::Encoding,
                                Failure::internal(SECRET_VALUE_SHAPE),
                            )));
                        }
                    }
                }
                (TypeKind::Builtin(_), ValueKind::Signed(_))
                | (TypeKind::Builtin(_), ValueKind::Unsigned(_))
                | (TypeKind::Builtin(_), ValueKind::Float32(_))
                | (TypeKind::Builtin(_), ValueKind::Float64(_))
                | (TypeKind::Builtin(_), ValueKind::Bool(_))
                | (TypeKind::Builtin(_), ValueKind::String(_)) => {}
                _ => {
                    return Err(out.fail(CodecErrors::emergency(
                        CodecStage::Encoding,
                        Failure::internal(SECRET_VALUE_SHAPE),
                    )));
                }
            }
        }
        Ok(false)
    }

    fn fields_have_secrets<'slot, 'schema, 'value>(
        fields: FieldList<'schema>,
        values: ValueEntries<'value>,
        tasks: &mut ChargedVec<SecretValueTask<'schema, 'value>>,
        session: &SessionBudget,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, bool> {
        for value in values {
            let Some(value) = value.as_named() else {
                return Err(out.fail(CodecErrors::emergency(
                    CodecStage::Encoding,
                    Failure::internal(SECRET_VALUE_SHAPE),
                )));
            };
            let field = session.budget_codec(
                CodecStage::Encoding,
                |budget_out| fields.named_budgeted(value.name(), session.execution(), budget_out),
                out.reborrow(),
            )?;
            let Some((_, field)) = field else {
                return Err(out.fail(CodecErrors::emergency(
                    CodecStage::Encoding,
                    Failure::internal(SECRET_VALUE_SHAPE),
                )));
            };
            if session.schema_codec(
                CodecStage::Encoding,
                |schema_out| field.is_secret(schema_out),
                out.reborrow(),
            )? {
                return Ok(true);
            }
            let ty = session.schema_codec(
                CodecStage::Encoding,
                |schema_out| field.ty(schema_out),
                out.reborrow(),
            )?;
            Self::push(tasks, ty, value.value(), session, out.reborrow())?;
        }
        Ok(false)
    }

    fn push<'slot, 'schema, 'value>(
        tasks: &mut ChargedVec<SecretValueTask<'schema, 'value>>,
        ty: TypeView<'schema>,
        value: ValueView<'value>,
        session: &SessionBudget,
        out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ()> {
        session.budget_codec(
            CodecStage::Encoding,
            |budget_out| tasks.try_push(SecretValueTask { ty, value }, budget_out),
            out,
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
    pub(crate) fn parse<'slot, R: Read + ?Sized>(
        schema: &'schema CompiledSchema,
        request: ConfigParseRequest<'_, R>,
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
