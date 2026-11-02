mod literals;

#[cfg(all(test, feature = "std"))]
mod tests;

use crate::{
    codec::literals::Literal,
    conversion::ConvertedValue,
    diagnostics::{CodecErrors, CodecStage, Failure},
    session::SessionBudget,
    value_walk::{
        EnumPayloadValue, FieldIndexError, NAMED_ENTRY_NAME_MISSING, ValueNode, ValueNodeKind,
    },
};
use alloc::string::String;
use core::fmt;
use opeo::{OResult, Out, ResultOutExt};
use typol_config::{
    ConfigParseRequest, ConfigParser, ConfigValidationOptions, ConfigValidationRequest,
    ConfigValueProcessor, Environment,
};
use typol_core::{
    ChargedVec, ConfigValue, DiagnosticContext, ValueEntries, ValueEntryView, ValueView,
    diagnostic_context,
};
use typol_semantic::{CompiledSchema, FieldList, TypeKind, TypeView};

#[cfg(feature = "serde")]
use crate::serde::SerdeValue;

#[cfg(all(test, feature = "std"))]
use typol_config::ConfigParseErrors;

static ENCODER_INVARIANT: DiagnosticContext =
    diagnostic_context!("typol", "encoding", "encoder-invariant");

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
    value: ConfigValue,
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
        let value = session
            .validation_codec(
                |validation_out| {
                    ConfigValueProcessor::new(schema).validate_schema_complete_with_budget(
                        value,
                        ConfigValidationRequest::new(options),
                        session.execution(),
                        validation_out,
                    )
                },
                out.reborrow(),
            )?
            .into_config_value();
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
    ) -> OResult<'slot, ConfigValue> {
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
        Ok(value.into_config_value())
    }

    #[cfg(all(test, feature = "std"))]
    pub(crate) fn value(&self) -> &ConfigValue {
        &self.value
    }

    pub(crate) fn encode<'slot>(
        &mut self,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, String> {
        let entry = self.schema.entry();
        let kind = self.session.schema_codec(
            CodecStage::Encoding,
            |schema_out| entry.kind(schema_out),
            out.reborrow(),
        )?;
        if !matches!(kind, TypeKind::Struct { .. }) {
            return Err(out.fail(encoding_error(Failure::internal(ENCODER_INVARIANT))));
        }
        Encoder::run(entry, &self.value, self.session, out.reborrow())
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
        let value = ConfigParser::new(schema)
            .parse(request, environment, out.reborrow())?
            .into_config_value();
        Ok(Self {
            schema,
            value,
            session,
        })
    }
}

enum Frame<'schema, 'value> {
    Value {
        ty: TypeView<'schema>,
        value: ValueView<'value>,
    },
    Text(&'static str),
    Fields {
        fields: FieldList<'schema>,
        values: ChargedVec<ValueEntryView<'value>>,
        slots: ChargedVec<usize>,
        next: usize,
    },
    Array {
        ty: TypeView<'schema>,
        values: ChargedVec<ValueEntryView<'value>>,
        next: usize,
    },
    Map {
        ty: TypeView<'schema>,
        values: ChargedVec<ValueEntryView<'value>>,
        slots: ChargedVec<usize>,
        next: usize,
    },
}

struct Encoder<'call> {
    session: &'call mut SessionBudget,
    output: ChargedVec<u8>,
}

impl<'call> Encoder<'call> {
    fn run<'slot>(
        ty: TypeView<'_>,
        value: &ConfigValue,
        session: &'call mut SessionBudget,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, String> {
        let mut frames = session.budget_codec(
            CodecStage::Encoding,
            |budget_out| session.workspace(budget_out),
            out.reborrow(),
        )?;
        let output = session.budget_codec(
            CodecStage::Encoding,
            |budget_out| session.workspace(budget_out),
            out.reborrow(),
        )?;
        let mut encoder = Self { session, output };
        push(
            encoder.session,
            &mut frames,
            Frame::Value {
                ty,
                value: value.root(),
            },
            out.reborrow(),
        )?;
        while let Some(frame) = frames.pop() {
            encoder.session.budget_codec(
                CodecStage::Encoding,
                |budget_out| encoder.session.visit(budget_out),
                out.reborrow(),
            )?;
            encoder.frame(frame, &mut frames, out.reborrow())?;
        }
        drop(frames);
        // The complete UTF-8 fragment is written before handing off the underlying Vec, with no further allocation or output copy.
        encoder.session.budget_codec(
            CodecStage::Encoding,
            |budget_out| {
                encoder
                    .session
                    .execution()
                    .consume(encoder.output.len(), budget_out)
            },
            out.reborrow(),
        )?;
        let output = String::from_utf8(encoder.output.into_unmetered_vec())
            .map_err(|_| encoding_error(Failure::internal(ENCODER_INVARIANT)))
            .store_err_into(out.reborrow())?;
        Ok(output)
    }

    fn write<'slot>(
        &mut self,
        text: &str,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ()> {
        self.session.output_codec(text.len(), out.reborrow())?;
        self.session.budget_codec(
            CodecStage::Encoding,
            |budget_out| self.output.try_reserve(text.len(), budget_out),
            out.reborrow(),
        )?;
        for byte in text.bytes() {
            self.session.budget_codec(
                CodecStage::Encoding,
                |budget_out| self.output.try_push(byte, budget_out),
                out.reborrow(),
            )?;
        }
        Ok(())
    }

    fn string<'slot>(
        &mut self,
        text: &str,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ()> {
        self.write("\"", out.reborrow())?;
        for character in text.chars() {
            self.session.budget_codec(
                CodecStage::Encoding,
                |budget_out| {
                    self.session
                        .execution()
                        .consume(character.len_utf8(), budget_out)
                },
                out.reborrow(),
            )?;
            match Literal::escape(character, out.reborrow())? {
                Some(escaped) => self.write(escaped, out.reborrow())?,
                None => self.write(character.encode_utf8(&mut [0; 4]), out.reborrow())?,
            }
        }
        self.write("\"", out.reborrow())
    }

    fn number<'slot>(
        &mut self,
        literal: Literal,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ()> {
        let text = literal.text(out.reborrow())?;
        self.session.budget_codec(
            CodecStage::Encoding,
            |budget_out| self.session.execution().consume(text.len(), budget_out),
            out.reborrow(),
        )?;
        self.write(text, out.reborrow())
    }

    fn fields<'slot, 'schema, 'value>(
        &mut self,
        fields: FieldList<'schema>,
        values: ValueEntries<'value>,
        frames: &mut ChargedVec<Frame<'schema, 'value>>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ()> {
        let mut slots = self.session.budget_codec(
            CodecStage::Encoding,
            |budget_out| self.session.workspace(budget_out),
            out.reborrow(),
        )?;
        self.session.budget_codec(
            CodecStage::Encoding,
            |budget_out| slots.try_reserve_exact(fields.len(), budget_out),
            out.reborrow(),
        )?;
        self.session.budget_codec(
            CodecStage::Encoding,
            |budget_out| slots.resize(fields.len(), usize::MAX, budget_out),
            out.reborrow(),
        )?;
        ValueNode::index_fields(
            values,
            slots.as_mut_slice(),
            |name, mut find_out| {
                self.session.budget_codec(
                    CodecStage::Encoding,
                    |budget_out| self.session.visit(budget_out),
                    find_out.reborrow(),
                )?;
                let named = self.session.budget_codec(
                    CodecStage::Encoding,
                    |budget_out| fields.named_budgeted(name, self.session.execution(), budget_out),
                    find_out.reborrow(),
                )?;
                Ok(named.map(|(slot, _)| slot))
            },
            |error, invalid_out| {
                let context = match error {
                    FieldIndexError::MissingName => NAMED_ENTRY_NAME_MISSING,
                    FieldIndexError::FieldCountMismatch
                    | FieldIndexError::UnknownField
                    | FieldIndexError::DuplicateField
                    | FieldIndexError::InvalidSlot
                    | FieldIndexError::MissingField => ENCODER_INVARIANT,
                };
                Err(invalid_out.fail(encoding_error(Failure::internal(context))))
            },
            out.reborrow(),
        )?;
        let entries = self.collect_entries(values, out.reborrow())?;
        self.write("{", out.reborrow())?;
        push(
            self.session,
            frames,
            Frame::Fields {
                fields,
                values: entries,
                slots,
                next: 0,
            },
            out.reborrow(),
        )
    }

    fn frame<'slot, 'schema, 'value>(
        &mut self,
        frame: Frame<'schema, 'value>,
        frames: &mut ChargedVec<Frame<'schema, 'value>>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ()> {
        match frame {
            Frame::Text(text) => self.write(text, out.reborrow()),
            Frame::Value { ty, value } => self.value(ty, value, frames, out.reborrow()),
            Frame::Fields {
                fields,
                values,
                slots,
                next,
            } => {
                let Some(field) = fields.get(next) else {
                    return self.write("}", out.reborrow());
                };
                if next > 0 {
                    self.write(", ", out.reborrow())?;
                }
                let name = self.session.schema_codec(
                    CodecStage::Encoding,
                    |schema_out| field.name(schema_out),
                    out.reborrow(),
                )?;
                self.write(name, out.reborrow())?;
                self.write(": ", out.reborrow())?;
                let slot = slots
                    .get(next)
                    .copied()
                    .ok_or_else(|| encoding_error(Failure::internal(ENCODER_INVARIANT)))
                    .store_err_into(out.reborrow())?;
                let value = values
                    .get(slot)
                    .ok_or_else(|| encoding_error(Failure::internal(ENCODER_INVARIANT)))
                    .store_err_into(out.reborrow())?
                    .value();
                let ty = self.session.schema_codec(
                    CodecStage::Encoding,
                    |schema_out| field.ty(schema_out),
                    out.reborrow(),
                )?;
                push(
                    self.session,
                    frames,
                    Frame::Fields {
                        fields,
                        values,
                        slots,
                        next: next + 1,
                    },
                    out.reborrow(),
                )?;
                push(
                    self.session,
                    frames,
                    Frame::Value { ty, value },
                    out.reborrow(),
                )
            }
            Frame::Array { ty, values, next } => {
                let Some(value) = values.get(next).map(ValueEntryView::value) else {
                    return self.write("]", out.reborrow());
                };
                if next > 0 {
                    self.write(", ", out.reborrow())?;
                }
                push(
                    self.session,
                    frames,
                    Frame::Array {
                        ty,
                        values,
                        next: next + 1,
                    },
                    out.reborrow(),
                )?;
                push(
                    self.session,
                    frames,
                    Frame::Value { ty, value },
                    out.reborrow(),
                )
            }
            Frame::Map {
                ty,
                values,
                slots,
                next,
            } => {
                let Some(slot) = slots.get(next) else {
                    return self.write("}", out.reborrow());
                };
                let entry = values
                    .get(*slot)
                    .ok_or_else(|| encoding_error(Failure::internal(ENCODER_INVARIANT)))
                    .store_err_into(out.reborrow())?;
                let Some(entry) = entry.as_named() else {
                    return Err(
                        out.fail(encoding_error(Failure::internal(NAMED_ENTRY_NAME_MISSING)))
                    );
                };
                let name = entry.name();
                let value = entry.value();
                if next > 0 {
                    self.write(", ", out.reborrow())?;
                }
                self.string(name, out.reborrow())?;
                self.write(": ", out.reborrow())?;
                push(
                    self.session,
                    frames,
                    Frame::Map {
                        ty,
                        values,
                        slots,
                        next: next + 1,
                    },
                    out.reborrow(),
                )?;
                push(
                    self.session,
                    frames,
                    Frame::Value { ty, value },
                    out.reborrow(),
                )
            }
        }
    }

    fn value<'slot, 'schema, 'value>(
        &mut self,
        ty: TypeView<'schema>,
        value: ValueView<'value>,
        frames: &mut ChargedVec<Frame<'schema, 'value>>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ()> {
        let kind = self.session.schema_codec(
            CodecStage::Encoding,
            |schema_out| ty.kind(schema_out),
            out.reborrow(),
        )?;
        let node = ValueNode::resolve(kind, value)
            .ok_or_else(|| encoding_error(Failure::internal(ENCODER_INVARIANT)))
            .store_err_into(out.reborrow())?;
        match node {
            ValueNodeKind::Signed { value, .. } => {
                let literal = Literal::number(value, out.reborrow())?;
                self.number(literal, out.reborrow())
            }
            ValueNodeKind::Unsigned { value, .. } => {
                let literal = Literal::number(value, out.reborrow())?;
                self.number(literal, out.reborrow())
            }
            ValueNodeKind::Float32(value) if value.is_finite() => {
                let literal = Literal::float(
                    value,
                    |length, mut check_out| {
                        self.session.budget_codec(
                            CodecStage::Encoding,
                            |budget_out| self.session.check_output(length, budget_out),
                            check_out.reborrow(),
                        )?;
                        self.session.budget_codec(
                            CodecStage::Encoding,
                            |budget_out| self.session.execution().consume(length, budget_out),
                            check_out.reborrow(),
                        )
                    },
                    out.reborrow(),
                )?;
                self.number(literal, out.reborrow())
            }
            ValueNodeKind::Float64(value) if value.is_finite() => {
                let literal = Literal::float(
                    value,
                    |length, mut check_out| {
                        self.session.budget_codec(
                            CodecStage::Encoding,
                            |budget_out| self.session.check_output(length, budget_out),
                            check_out.reborrow(),
                        )?;
                        self.session.budget_codec(
                            CodecStage::Encoding,
                            |budget_out| self.session.execution().consume(length, budget_out),
                            check_out.reborrow(),
                        )
                    },
                    out.reborrow(),
                )?;
                self.number(literal, out.reborrow())
            }
            ValueNodeKind::Bool(value) => {
                self.write(if value { "true" } else { "false" }, out.reborrow())
            }
            ValueNodeKind::String(value) => self.string(value, out.reborrow()),
            ValueNodeKind::Optional { value: None, .. } => self.write("none", out.reborrow()),
            ValueNodeKind::Optional {
                inner,
                value: Some(value),
            } => push(
                self.session,
                frames,
                Frame::Value { ty: inner, value },
                out.reborrow(),
            ),
            ValueNodeKind::Structure { fields, values } => {
                self.fields(fields, values, frames, out.reborrow())
            }
            ValueNodeKind::Array {
                element, values, ..
            } => {
                self.write("[", out.reborrow())?;
                push(
                    self.session,
                    frames,
                    Frame::Array {
                        ty: element,
                        values: self.collect_entries(values, out.reborrow())?,
                        next: 0,
                    },
                    out.reborrow(),
                )
            }
            ValueNodeKind::Map {
                value_type: ty,
                values,
            } => {
                let values = self.collect_entries(values, out.reborrow())?;
                let mut slots = self.session.budget_codec(
                    CodecStage::Encoding,
                    |budget_out| self.session.workspace(budget_out),
                    out.reborrow(),
                )?;
                self.session.budget_codec(
                    CodecStage::Encoding,
                    |budget_out| slots.try_reserve_exact(values.len(), budget_out),
                    out.reborrow(),
                )?;
                for slot in 0..values.len() {
                    let Some(entry) = values.get(slot) else {
                        return Err(out.fail(encoding_error(Failure::internal(ENCODER_INVARIANT))));
                    };
                    if entry.as_named().is_none() {
                        return Err(
                            out.fail(encoding_error(Failure::internal(NAMED_ENTRY_NAME_MISSING)))
                        );
                    }
                    self.session.budget_codec(
                        CodecStage::Encoding,
                        |budget_out| self.session.visit(budget_out),
                        out.reborrow(),
                    )?;
                    self.session.budget_codec(
                        CodecStage::Encoding,
                        |budget_out| slots.try_push(slot, budget_out),
                        out.reborrow(),
                    )?;
                }
                slots.sort_by(
                    |left, right, mut sort_out| {
                        let Some(left_entry) = values.get(*left) else {
                            return Err(
                                sort_out.fail(encoding_error(Failure::internal(ENCODER_INVARIANT)))
                            );
                        };
                        let Some(right_entry) = values.get(*right) else {
                            return Err(
                                sort_out.fail(encoding_error(Failure::internal(ENCODER_INVARIANT)))
                            );
                        };
                        let Some(left_entry) = left_entry.as_named() else {
                            return Err(sort_out.fail(encoding_error(Failure::internal(
                                NAMED_ENTRY_NAME_MISSING,
                            ))));
                        };
                        let Some(right_entry) = right_entry.as_named() else {
                            return Err(sort_out.fail(encoding_error(Failure::internal(
                                NAMED_ENTRY_NAME_MISSING,
                            ))));
                        };
                        let order = self.session.budget_codec(
                            CodecStage::Encoding,
                            |budget_out| {
                                self.session.compare_text(
                                    left_entry.name(),
                                    right_entry.name(),
                                    budget_out,
                                )
                            },
                            sort_out.reborrow(),
                        )?;
                        Ok(order)
                    },
                    out.reborrow(),
                )?;
                self.write("{", out.reborrow())?;
                push(
                    self.session,
                    frames,
                    Frame::Map {
                        ty,
                        values,
                        slots,
                        next: 0,
                    },
                    out.reborrow(),
                )
            }
            ValueNodeKind::Enumeration(value) => {
                let variant = self.session.budget_codec(
                    CodecStage::Encoding,
                    |budget_out| {
                        ty.variant_named_budgeted(
                            value.variant(),
                            self.session.execution(),
                            budget_out,
                        )
                    },
                    out.reborrow(),
                )?;
                let Some(variant) = variant else {
                    return Err(out.fail(encoding_error(Failure::internal(ENCODER_INVARIANT))));
                };
                self.write(".", out.reborrow())?;
                let name = self.session.schema_codec(
                    CodecStage::Encoding,
                    |schema_out| variant.name(schema_out),
                    out.reborrow(),
                )?;
                self.write(name, out.reborrow())?;
                let variant_payload = self.session.schema_codec(
                    CodecStage::Encoding,
                    |schema_out| variant.payload(schema_out),
                    out.reborrow(),
                )?;
                let payload = value
                    .resolve_payload(variant_payload)
                    .ok_or_else(|| encoding_error(Failure::internal(ENCODER_INVARIANT)))
                    .store_err_into(out.reborrow())?;
                match payload {
                    EnumPayloadValue::Unit => Ok(()),
                    EnumPayloadValue::Single { ty, value } => {
                        self.write("(", out.reborrow())?;
                        push(self.session, frames, Frame::Text(")"), out.reborrow())?;
                        push(
                            self.session,
                            frames,
                            Frame::Value { ty, value },
                            out.reborrow(),
                        )
                    }
                    EnumPayloadValue::Structure { fields, values } => {
                        self.write(" ", out.reborrow())?;
                        self.fields(fields, values, frames, out.reborrow())
                    }
                }
            }
            _ => Err(out.fail(encoding_error(Failure::internal(ENCODER_INVARIANT)))),
        }
    }

    fn collect_entries<'slot, 'value>(
        &self,
        values: ValueEntries<'value>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ChargedVec<ValueEntryView<'value>>> {
        let mut entries = self.session.budget_codec(
            CodecStage::Encoding,
            |budget_out| self.session.workspace(budget_out),
            out.reborrow(),
        )?;
        self.session.budget_codec(
            CodecStage::Encoding,
            |budget_out| entries.try_reserve_exact(values.len(), budget_out),
            out.reborrow(),
        )?;
        for entry in values {
            self.session.budget_codec(
                CodecStage::Encoding,
                |budget_out| entries.try_push(entry, budget_out),
                out.reborrow(),
            )?;
        }
        Ok(entries)
    }
}

fn push<'slot, 'schema, 'value>(
    session: &SessionBudget,
    frames: &mut ChargedVec<Frame<'schema, 'value>>,
    frame: Frame<'schema, 'value>,
    out: Out<'slot, '_, CodecErrors>,
) -> OResult<'slot, ()> {
    session.budget_codec(
        CodecStage::Encoding,
        |budget_out| frames.try_push(frame, budget_out),
        out,
    )
}

fn encoding_error(failure: Failure) -> CodecErrors {
    CodecErrors::emergency(CodecStage::Encoding, failure)
}
