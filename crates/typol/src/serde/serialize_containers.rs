use crate::{
    serde::{
        encode::Encoder,
        map_keys::MapKeys,
        model::{Cursor, Error},
        string::{Key, StringSerializer},
    },
    value::{EnumPayload, FieldValue, MapEntry, ValueFactory},
};
use opeo::{OResult, Out};
use serde::{
    Serialize,
    ser::{SerializeMap, SerializeSeq, SerializeStruct, SerializeStructVariant, SerializeTuple},
};
use typol_core::{ChargedVec, ConfigValue, DiagnosticContext, diagnostic_context};
use typol_semantic::{FieldList, TypeKind, TypeView};

static FIELD_SLOT_INVARIANT: DiagnosticContext =
    diagnostic_context!("typol", "serde", "serialized-field-slot-invariant");

pub(crate) struct Sequence<'a, 'schema, 'call> {
    cursor: Cursor<'a, 'schema, 'call>,
    element: TypeView<'schema>,
    length: Option<usize>,
    values: ChargedVec<ConfigValue>,
}

impl<'a, 'schema, 'call> Sequence<'a, 'schema, 'call> {
    pub(crate) fn new<'slot>(
        cursor: Cursor<'a, 'schema, 'call>,
        length: Option<usize>,
        mut out: Out<'slot, '_, Error>,
    ) -> OResult<'slot, Self> {
        let kind = cursor
            .session()
            .schema_as(|schema_out| cursor.ty().kind(schema_out), out.reborrow())?;
        let TypeKind::Array {
            element,
            length: expected,
        } = kind
        else {
            return Err(out.fail(Error::shape_mismatch()));
        };
        if expected.map(|value| value as usize) != length {
            return Err(out.fail(Error::shape_mismatch()));
        }
        let values = cursor.session().budget_as(
            |budget_out| cursor.session().workspace(budget_out),
            out.reborrow(),
        )?;
        Ok(Self {
            cursor,
            element,
            length,
            values,
        })
    }

    fn element<'slot, T: Serialize + ?Sized>(
        &mut self,
        value: &T,
        mut out: Out<'slot, '_, Error>,
    ) -> OResult<'slot, ()> {
        if self
            .length
            .is_some_and(|length| self.values.len() >= length)
        {
            return Err(out.fail(Error::shape_mismatch()));
        }
        let child = self.cursor.child(self.element, out.reborrow())?;
        let value = match value.serialize(Encoder::new(child)) {
            Ok(value) => value,
            Err(error) => return Err(out.fail(error)),
        };
        self.values.try_push(value, out.reborrow())?;
        Ok(())
    }

    fn finish<'slot, 'borrow>(
        self,
        out: Out<'slot, 'borrow, Error>,
    ) -> OResult<'slot, ConfigValue> {
        if self
            .length
            .is_some_and(|length| self.values.len() != length)
        {
            return Err(out.fail(Error::shape_mismatch()));
        }
        self.cursor.session().value_as(
            |value_out| {
                ValueFactory::array(
                    self.values,
                    Some(self.cursor.session().execution()),
                    value_out,
                )
            },
            out,
        )
    }
}

impl SerializeSeq for Sequence<'_, '_, '_> {
    type Ok = ConfigValue;
    type Error = Error;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        let fault = self.cursor.fault();
        fault.run(|out| self.element(value, out))
    }

    fn end(self) -> Result<ConfigValue, Error> {
        let fault = self.cursor.fault();
        fault.run(|out| self.finish(out))
    }
}

impl SerializeTuple for Sequence<'_, '_, '_> {
    type Ok = ConfigValue;
    type Error = Error;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        let fault = self.cursor.fault();
        fault.run(|out| self.element(value, out))
    }

    fn end(self) -> Result<ConfigValue, Error> {
        let fault = self.cursor.fault();
        fault.run(|out| self.finish(out))
    }
}

pub(crate) struct Fields<'a, 'schema, 'call> {
    cursor: Cursor<'a, 'schema, 'call>,
    fields: FieldList<'schema>,
    values: ChargedVec<Option<ConfigValue>>,
    seen: ChargedVec<bool>,
    variant: Option<&'schema str>,
}

impl<'a, 'schema, 'call> Fields<'a, 'schema, 'call> {
    pub(crate) fn new<'slot>(
        cursor: Cursor<'a, 'schema, 'call>,
        fields: FieldList<'schema>,
        variant: Option<&'schema str>,
        mut out: Out<'slot, '_, Error>,
    ) -> OResult<'slot, Self> {
        let mut values = cursor.session().budget_as(
            |budget_out| cursor.session().workspace(budget_out),
            out.reborrow(),
        )?;
        cursor.session().budget_as(
            |budget_out| values.try_reserve_exact(fields.len(), budget_out),
            out.reborrow(),
        )?;
        cursor.session().budget_as(
            |budget_out| values.resize_with(fields.len(), || None, budget_out),
            out.reborrow(),
        )?;
        let mut seen = cursor.session().budget_as(
            |budget_out| cursor.session().workspace(budget_out),
            out.reborrow(),
        )?;
        cursor.session().budget_as(
            |budget_out| seen.try_reserve_exact(fields.len(), budget_out),
            out.reborrow(),
        )?;
        cursor.session().budget_as(
            |budget_out| seen.resize(fields.len(), false, budget_out),
            out.reborrow(),
        )?;
        Ok(Self {
            cursor,
            fields,
            values,
            seen,
            variant,
        })
    }

    fn field<'slot, T: Serialize + ?Sized>(
        &mut self,
        name: &str,
        value: &T,
        mut out: Out<'slot, '_, Error>,
    ) -> OResult<'slot, ()> {
        let named = self.cursor.session().budget_as(
            |budget_out| {
                self.fields
                    .named_budgeted(name, self.cursor.session().execution(), budget_out)
            },
            out.reborrow(),
        )?;
        let Some((slot, field)) = named else {
            return Err(out.fail(Error::shape_mismatch()));
        };
        let Some(seen) = self.seen.get(slot) else {
            return Err(out.fail(Self::slot_invariant(slot)));
        };
        if *seen {
            return Err(out.fail(Error::shape_mismatch()));
        }
        let Some(seen) = self.seen.get_mut(slot) else {
            return Err(out.fail(Self::slot_invariant(slot)));
        };
        *seen = true;
        let ty = self
            .cursor
            .session()
            .schema_as(|schema_out| field.ty(schema_out), out.reborrow())?;
        let child = self.cursor.child(ty, out.reborrow())?;
        let value = match value.serialize(Encoder::new(child)) {
            Ok(value) => value,
            Err(error) => return Err(out.fail(error)),
        };
        let Some(stored) = self.values.get_mut(slot) else {
            return Err(out.fail(Self::slot_invariant(slot)));
        };
        *stored = Some(value);
        Ok(())
    }

    fn skip<'slot>(&mut self, name: &str, mut out: Out<'slot, '_, Error>) -> OResult<'slot, ()> {
        let named = self.cursor.session().budget_as(
            |budget_out| {
                self.fields
                    .named_budgeted(name, self.cursor.session().execution(), budget_out)
            },
            out.reborrow(),
        )?;
        let Some((slot, _)) = named else {
            return Err(out.fail(Error::shape_mismatch()));
        };
        let Some(seen) = self.seen.get(slot) else {
            return Err(out.fail(Self::slot_invariant(slot)));
        };
        if *seen {
            return Err(out.fail(Error::shape_mismatch()));
        }
        let Some(seen) = self.seen.get_mut(slot) else {
            return Err(out.fail(Self::slot_invariant(slot)));
        };
        *seen = true;
        Ok(())
    }

    fn finish<'slot>(mut self, mut out: Out<'slot, '_, Error>) -> OResult<'slot, ConfigValue> {
        let mut values = self.cursor.session().budget_as(
            |budget_out| self.cursor.session().workspace(budget_out),
            out.reborrow(),
        )?;
        self.cursor.session().budget_as(
            |budget_out| values.try_reserve_exact(self.fields.len(), budget_out),
            out.reborrow(),
        )?;
        for (slot, field) in self.fields.iter().enumerate() {
            self.cursor.session().budget_as(
                |budget_out| self.cursor.session().visit(budget_out),
                out.reborrow(),
            )?;
            let Some(stored) = self.values.get_mut(slot) else {
                return Err(out.fail(Self::slot_invariant(slot)));
            };
            let value = match stored.take() {
                Some(value) => value,
                None => {
                    let default = self
                        .cursor
                        .session()
                        .schema_as(|schema_out| field.default_value(schema_out), out.reborrow())?;
                    match default {
                        Some(value) => self.cursor.default_value(value, out.reborrow())?,
                        None => {
                            let ty = self
                                .cursor
                                .session()
                                .schema_as(|schema_out| field.ty(schema_out), out.reborrow())?;
                            let kind = self
                                .cursor
                                .session()
                                .schema_as(|schema_out| ty.kind(schema_out), out.reborrow())?;
                            if matches!(kind, TypeKind::Option { .. }) {
                                let _ = self.cursor.child(ty, out.reborrow())?;
                                self.cursor.session().value_as(
                                    |value_out| {
                                        ValueFactory::none(
                                            Some(self.cursor.session().execution()),
                                            value_out,
                                        )
                                    },
                                    out.reborrow(),
                                )?
                            } else {
                                return Err(out.fail(Error::shape_mismatch()));
                            }
                        }
                    }
                }
            };
            let name = self
                .cursor
                .session()
                .schema_as(|schema_out| field.name(schema_out), out.reborrow())?;
            self.cursor.text(name, out.reborrow())?;
            let value = self.cursor.session().value_as(
                |value_out| {
                    FieldValue::new(
                        name,
                        value,
                        Some(self.cursor.session().execution()),
                        value_out,
                    )
                },
                out.reborrow(),
            )?;
            values.try_push(value, out.reborrow())?;
        }
        match self.variant {
            Some(variant) => {
                self.cursor.text(variant, out.reborrow())?;
                let payload = EnumPayload::structure(values);
                self.cursor.session().value_as(
                    |value_out| {
                        ValueFactory::enumeration(
                            variant,
                            payload,
                            Some(self.cursor.session().execution()),
                            value_out,
                        )
                    },
                    out,
                )
            }
            None => self.cursor.session().value_as(
                |value_out| {
                    ValueFactory::structure(
                        values,
                        Some(self.cursor.session().execution()),
                        value_out,
                    )
                },
                out,
            ),
        }
    }

    fn slot_invariant(slot: usize) -> Error {
        Error::internal(FIELD_SLOT_INVARIANT.with_object_id(slot as u64))
    }
}

impl SerializeStruct for Fields<'_, '_, '_> {
    type Ok = ConfigValue;
    type Error = Error;

    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        name: &'static str,
        value: &T,
    ) -> Result<(), Error> {
        let fault = self.cursor.fault();
        fault.run(|out| self.field(name, value, out))
    }

    fn skip_field(&mut self, name: &'static str) -> Result<(), Error> {
        let fault = self.cursor.fault();
        fault.run(|out| self.skip(name, out))
    }

    fn end(self) -> Result<ConfigValue, Error> {
        let fault = self.cursor.fault();
        fault.run(|out| self.finish(out))
    }
}

impl SerializeStructVariant for Fields<'_, '_, '_> {
    type Ok = ConfigValue;
    type Error = Error;

    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        name: &'static str,
        value: &T,
    ) -> Result<(), Error> {
        let fault = self.cursor.fault();
        fault.run(|out| self.field(name, value, out))
    }

    fn skip_field(&mut self, name: &'static str) -> Result<(), Error> {
        let fault = self.cursor.fault();
        fault.run(|out| self.skip(name, out))
    }

    fn end(self) -> Result<ConfigValue, Error> {
        let fault = self.cursor.fault();
        fault.run(|out| self.finish(out))
    }
}

pub(crate) struct Mapping<'a, 'schema, 'call> {
    cursor: Cursor<'a, 'schema, 'call>,
    value: TypeView<'schema>,
    entries: ChargedVec<MapEntry>,
    pending: Option<Key>,
    keys: MapKeys,
}

impl<'a, 'schema, 'call> Mapping<'a, 'schema, 'call> {
    pub(crate) fn new<'slot>(
        cursor: Cursor<'a, 'schema, 'call>,
        mut out: Out<'slot, '_, Error>,
    ) -> OResult<'slot, Self> {
        let kind = cursor
            .session()
            .schema_as(|schema_out| cursor.ty().kind(schema_out), out.reborrow())?;
        let TypeKind::Map { value } = kind else {
            return Err(out.fail(Error::shape_mismatch()));
        };
        let entries = cursor.session().budget_as(
            |budget_out| cursor.session().workspace(budget_out),
            out.reborrow(),
        )?;
        let keys = MapKeys::new(cursor.session(), out.reborrow())?;
        Ok(Self {
            cursor,
            value,
            entries,
            pending: None,
            keys,
        })
    }
}

impl SerializeMap for Mapping<'_, '_, '_> {
    type Ok = ConfigValue;
    type Error = Error;

    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), Error> {
        let fault = self.cursor.fault();
        fault.run(|mut out| {
            if self.pending.is_some() {
                return Err(out.fail(Error::shape_mismatch()));
            }
            self.cursor.key(out.reborrow())?;
            let key = match key.serialize(StringSerializer::new(&self.cursor)) {
                Ok(key) => key,
                Err(error) => return Err(out.fail(error)),
            };
            self.keys.insert(
                key.text(),
                &self.entries,
                self.cursor.session(),
                out.reborrow(),
            )?;
            self.pending = Some(key);
            Ok(())
        })
    }

    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        let fault = self.cursor.fault();
        fault.run(|mut out| {
            let Some(key) = self.pending.take() else {
                return Err(out.fail(Error::shape_mismatch()));
            };
            self.cursor.session().budget_as(
                |budget_out| self.entries.try_reserve(1, budget_out),
                out.reborrow(),
            )?;
            let child = self.cursor.child(self.value, out.reborrow())?;
            let value = match value.serialize(Encoder::new(child)) {
                Ok(value) => value,
                Err(error) => return Err(out.fail(error)),
            };
            let entry = self.cursor.session().value_as(
                |value_out| {
                    MapEntry::from_name(
                        key.into_name(),
                        value,
                        Some(self.cursor.session().execution()),
                        value_out,
                    )
                },
                out.reborrow(),
            )?;
            self.entries.try_push(entry, out.reborrow())?;
            Ok(())
        })
    }

    fn end(mut self) -> Result<ConfigValue, Error> {
        let fault = self.cursor.fault();
        fault.run(|mut out| {
            if self.pending.is_some() {
                return Err(out.fail(Error::shape_mismatch()));
            }
            self.cursor.session().budget_as(
                |budget_out| {
                    self.entries.sort_by(
                        |left, right, compare_out| {
                            self.cursor.session().compare_text(
                                left.name(),
                                right.name(),
                                compare_out,
                            )
                        },
                        budget_out,
                    )
                },
                out.reborrow(),
            )?;
            self.cursor.session().value_as(
                |value_out| {
                    ValueFactory::map(
                        self.entries,
                        Some(self.cursor.session().execution()),
                        value_out,
                    )
                },
                out,
            )
        })
    }
}
