use crate::{
    serde::{
        decode::Decoder,
        model::{Cursor, Error},
        string::StringDeserializer,
    },
    value_walk::{EnumPayloadValue, EnumValue, NAMED_ENTRY_NAME_MISSING, ValueNode, ValueNodeKind},
};
use opeo::{OResult, Out, ResultOutExt};
use serde::de::{DeserializeSeed, EnumAccess, MapAccess, SeqAccess, VariantAccess, Visitor};
use typol_core::{
    ChargedVec, DiagnosticContext, ValueEntries, ValueEntryView, ValueView, diagnostic_context,
};
use typol_semantic::{FieldList, TypeView, VariantView};

static MAPPING_INDEX_INVARIANT: DiagnosticContext =
    diagnostic_context!("typol", "serde", "mapping-entry-index-out-of-bounds");

fn collect_entries<'slot, 'de>(
    cursor: &Cursor<'_, '_, '_>,
    values: ValueEntries<'de>,
    mut out: Out<'slot, '_, Error>,
) -> OResult<'slot, ChargedVec<ValueEntryView<'de>>> {
    let mut entries = cursor.session().budget_as(
        |budget_out| cursor.session().workspace(budget_out),
        out.reborrow(),
    )?;
    cursor.session().budget_as(
        |budget_out| entries.try_reserve_exact(values.len(), budget_out),
        out.reborrow(),
    )?;
    for entry in values {
        entries.try_push(entry, out.reborrow())?;
    }
    Ok(entries)
}

pub(crate) struct SequenceAccess<'a, 'schema, 'call, 'de> {
    cursor: Cursor<'a, 'schema, 'call>,
    element: TypeView<'schema>,
    values: ChargedVec<ValueEntryView<'de>>,
    next: usize,
}

impl<'a, 'schema, 'call, 'de> SequenceAccess<'a, 'schema, 'call, 'de> {
    pub(crate) fn new<'slot>(
        cursor: Cursor<'a, 'schema, 'call>,
        value: ValueView<'de>,
        length: Option<usize>,
        mut out: Out<'slot, '_, Error>,
    ) -> OResult<'slot, Self> {
        let kind = cursor
            .session()
            .schema_as(|schema_out| cursor.ty().kind(schema_out), out.reborrow())?;
        let Some(ValueNodeKind::Array {
            element,
            expected_length,
            values,
        }) = ValueNode::resolve(kind, value)
        else {
            return Err(out.fail(Error::shape_mismatch()));
        };
        let values = collect_entries(&cursor, values, out.reborrow())?;
        if expected_length.and_then(|value| usize::try_from(value).ok()) != length
            || length.is_some_and(|length| length != values.len())
        {
            return Err(out.fail(Error::shape_mismatch()));
        }
        Ok(Self {
            cursor,
            element,
            values,
            next: 0,
        })
    }

    pub(crate) fn finish<'slot, 'borrow>(
        &self,
        out: Out<'slot, 'borrow, Error>,
    ) -> OResult<'slot, ()> {
        if self.next == self.values.len() {
            Ok(())
        } else {
            Err(out.fail(Error::shape_mismatch()))
        }
    }
}

impl<'de> SeqAccess<'de> for &mut SequenceAccess<'_, '_, '_, 'de> {
    type Error = Error;

    fn next_element_seed<S: DeserializeSeed<'de>>(
        &mut self,
        seed: S,
    ) -> Result<Option<S::Value>, Error> {
        let fault = self.cursor.fault();
        fault.run(|mut out| {
            self.cursor.session().budget_as(
                |budget_out| self.cursor.session().visit(budget_out),
                out.reborrow(),
            )?;
            let Some(value) = self.values.get(self.next) else {
                return Ok(None);
            };
            let child = self.cursor.child(self.element, out.reborrow())?;
            let value = Decoder::seed(child, value.value(), seed, out.reborrow())?;
            self.next += 1;
            Ok(Some(value))
        })
    }

    fn size_hint(&self) -> Option<usize> {
        None
    }
}

enum Entries<'schema, 'de> {
    Map {
        ty: TypeView<'schema>,
        values: ChargedVec<ValueEntryView<'de>>,
    },
    Fields {
        fields: FieldList<'schema>,
        values: ChargedVec<ValueEntryView<'de>>,
    },
}

pub(crate) struct MappingAccess<'a, 'schema, 'call, 'de> {
    cursor: Cursor<'a, 'schema, 'call>,
    entries: Entries<'schema, 'de>,
    next: usize,
    pending: bool,
}

impl<'a, 'schema, 'call, 'de> MappingAccess<'a, 'schema, 'call, 'de> {
    pub(crate) fn map<'slot>(
        cursor: Cursor<'a, 'schema, 'call>,
        value: ValueView<'de>,
        mut out: Out<'slot, '_, Error>,
    ) -> OResult<'slot, Self> {
        let kind = cursor
            .session()
            .schema_as(|schema_out| cursor.ty().kind(schema_out), out.reborrow())?;
        let Some(ValueNodeKind::Map {
            value_type: ty,
            values,
        }) = ValueNode::resolve(kind, value)
        else {
            return Err(out.fail(Error::shape_mismatch()));
        };
        let values = collect_entries(&cursor, values, out.reborrow())?;
        Ok(Self {
            cursor,
            entries: Entries::Map { ty, values },
            next: 0,
            pending: false,
        })
    }

    pub(crate) fn fields<'slot>(
        cursor: Cursor<'a, 'schema, 'call>,
        fields: FieldList<'schema>,
        values: ValueEntries<'de>,
        mut out: Out<'slot, '_, Error>,
    ) -> OResult<'slot, Self> {
        let values = collect_entries(&cursor, values, out.reborrow())?;
        if fields.len() != values.len() {
            return Err(out.fail(Error::shape_mismatch()));
        }
        for value in &values {
            let Some(value) = value.as_named() else {
                return Err(out.fail(Error::internal(NAMED_ENTRY_NAME_MISSING)));
            };
            let named = cursor.session().budget_as(
                |budget_out| {
                    fields.named_budgeted(value.name(), cursor.session().execution(), budget_out)
                },
                out.reborrow(),
            )?;
            if named.is_none() {
                return Err(out.fail(Error::shape_mismatch()));
            }
        }
        Ok(Self {
            cursor,
            entries: Entries::Fields { fields, values },
            next: 0,
            pending: false,
        })
    }

    fn len(&self) -> usize {
        match &self.entries {
            Entries::Map { values, .. } => values.len(),
            Entries::Fields { values, .. } => values.len(),
        }
    }

    pub(crate) fn finish<'slot, 'borrow>(
        &self,
        out: Out<'slot, 'borrow, Error>,
    ) -> OResult<'slot, ()> {
        if !self.pending && self.next == self.len() {
            Ok(())
        } else {
            Err(out.fail(Error::shape_mismatch()))
        }
    }
}

impl<'de> MapAccess<'de> for &mut MappingAccess<'_, '_, '_, 'de> {
    type Error = Error;

    fn next_key_seed<S: DeserializeSeed<'de>>(
        &mut self,
        seed: S,
    ) -> Result<Option<S::Value>, Error> {
        let fault = self.cursor.fault();
        fault.run(|mut out| {
            self.cursor.session().budget_as(
                |budget_out| self.cursor.session().visit(budget_out),
                out.reborrow(),
            )?;
            if self.pending {
                return Err(out.fail(Error::shape_mismatch()));
            }
            if self.next == self.len() {
                return Ok(None);
            }
            if matches!(&self.entries, Entries::Map { .. }) {
                self.cursor.key(out.reborrow())?;
            }
            let (key, identifier) = match &self.entries {
                Entries::Map { values, .. } => {
                    let Some(entry) = values.get(self.next) else {
                        return Err(out.fail(Error::internal(
                            MAPPING_INDEX_INVARIANT.with_object_id(self.next as u64),
                        )));
                    };
                    let Some(entry) = entry.as_named() else {
                        return Err(out.fail(Error::internal(NAMED_ENTRY_NAME_MISSING)));
                    };
                    (entry.name(), false)
                }
                Entries::Fields { values, .. } => {
                    let Some(field) = values.get(self.next) else {
                        return Err(out.fail(Error::internal(
                            MAPPING_INDEX_INVARIANT.with_object_id(self.next as u64),
                        )));
                    };
                    let Some(field) = field.as_named() else {
                        return Err(out.fail(Error::internal(NAMED_ENTRY_NAME_MISSING)));
                    };
                    (field.name(), true)
                }
            };
            let mut used = false;
            let key = match seed.deserialize(StringDeserializer::new(
                key,
                &mut self.cursor,
                identifier,
                &mut used,
            )) {
                Ok(key) => key,
                Err(error) => return Err(out.fail(error)),
            };
            if !used {
                return Err(out.fail(Error::shape_mismatch()));
            }
            self.pending = true;
            Ok(Some(key))
        })
    }

    fn next_value_seed<S: DeserializeSeed<'de>>(&mut self, seed: S) -> Result<S::Value, Error> {
        let fault = self.cursor.fault();
        fault.run(|mut out| {
            if !self.pending {
                return Err(out.fail(Error::shape_mismatch()));
            }
            let (ty, value) = match &self.entries {
                Entries::Map { ty, values } => {
                    let Some(value) = values.get(self.next) else {
                        return Err(out.fail(Error::internal(
                            MAPPING_INDEX_INVARIANT.with_object_id(self.next as u64),
                        )));
                    };
                    (*ty, value.value())
                }
                Entries::Fields { fields, values } => {
                    let Some(value) = values.get(self.next) else {
                        return Err(out.fail(Error::internal(
                            MAPPING_INDEX_INVARIANT.with_object_id(self.next as u64),
                        )));
                    };
                    let Some(value) = value.as_named() else {
                        return Err(out.fail(Error::internal(NAMED_ENTRY_NAME_MISSING)));
                    };
                    let named = self.cursor.session().budget_as(
                        |budget_out| {
                            fields.named_budgeted(
                                value.name(),
                                self.cursor.session().execution(),
                                budget_out,
                            )
                        },
                        out.reborrow(),
                    )?;
                    let Some((_, field)) = named else {
                        return Err(out.fail(Error::shape_mismatch()));
                    };
                    let ty = self
                        .cursor
                        .session()
                        .schema_as(|schema_out| field.ty(schema_out), out.reborrow())?;
                    (ty, value.value())
                }
            };
            let child = self.cursor.child(ty, out.reborrow())?;
            let result = Decoder::seed(child, value, seed, out.reborrow())?;
            self.pending = false;
            self.next += 1;
            Ok(result)
        })
    }

    fn size_hint(&self) -> Option<usize> {
        None
    }
}

pub(crate) struct EnumerationAccess<'a, 'schema, 'call, 'de> {
    cursor: Cursor<'a, 'schema, 'call>,
    variant: VariantView<'schema>,
    value: EnumValue<'de>,
    selected: bool,
    consumed: bool,
}

impl<'a, 'schema, 'call, 'de> EnumerationAccess<'a, 'schema, 'call, 'de> {
    pub(crate) fn new<'slot>(
        cursor: Cursor<'a, 'schema, 'call>,
        value: ValueView<'de>,
        mut out: Out<'slot, '_, Error>,
    ) -> OResult<'slot, Self> {
        let kind = cursor
            .session()
            .schema_as(|schema_out| cursor.ty().kind(schema_out), out.reborrow())?;
        let Some(ValueNodeKind::Enumeration(value)) = ValueNode::resolve(kind, value) else {
            return Err(out.fail(Error::shape_mismatch()));
        };
        let variant = cursor.session().budget_as(
            |budget_out| {
                cursor.ty().variant_named_budgeted(
                    value.variant(),
                    cursor.session().execution(),
                    budget_out,
                )
            },
            out.reborrow(),
        )?;
        let variant = variant
            .ok_or_else(Error::shape_mismatch)
            .store_err_into(out.reborrow())?;
        Ok(Self {
            cursor,
            variant,
            value,
            selected: false,
            consumed: false,
        })
    }

    pub(crate) fn finish<'slot, 'borrow>(
        &self,
        out: Out<'slot, 'borrow, Error>,
    ) -> OResult<'slot, ()> {
        if self.selected && self.consumed {
            Ok(())
        } else {
            Err(out.fail(Error::shape_mismatch()))
        }
    }
}

impl<'a, 'b, 'schema, 'call, 'de> EnumAccess<'de>
    for &'a mut EnumerationAccess<'b, 'schema, 'call, 'de>
{
    type Error = Error;
    type Variant = Self;

    fn variant_seed<S: DeserializeSeed<'de>>(self, seed: S) -> Result<(S::Value, Self), Error> {
        let fault = self.cursor.fault();
        fault.run(|mut out| {
            if self.selected {
                return Err(out.fail(Error::shape_mismatch()));
            }
            let name = self
                .cursor
                .session()
                .schema_as(|schema_out| self.variant.name(schema_out), out.reborrow())?;
            let mut used = false;
            let value = match seed.deserialize(StringDeserializer::new(
                name,
                &mut self.cursor,
                true,
                &mut used,
            )) {
                Ok(value) => value,
                Err(error) => return Err(out.fail(error)),
            };
            if !used {
                return Err(out.fail(Error::shape_mismatch()));
            }
            self.selected = true;
            Ok((value, self))
        })
    }
}

impl<'de> VariantAccess<'de> for &mut EnumerationAccess<'_, '_, '_, 'de> {
    type Error = Error;

    fn unit_variant(self) -> Result<(), Error> {
        let fault = self.cursor.fault();
        fault.run(|mut out| {
            let payload = self.cursor.session().schema_as(
                |schema_out| self.variant.payload(schema_out),
                out.reborrow(),
            )?;
            if !matches!(
                self.value.resolve_payload(payload),
                Some(EnumPayloadValue::Unit)
            ) {
                return Err(out.fail(Error::shape_mismatch()));
            }
            self.consumed = true;
            Ok(())
        })
    }

    fn newtype_variant_seed<S: DeserializeSeed<'de>>(self, seed: S) -> Result<S::Value, Error> {
        let fault = self.cursor.fault();
        fault.run(|mut out| {
            let payload = self.cursor.session().schema_as(
                |schema_out| self.variant.payload(schema_out),
                out.reborrow(),
            )?;
            let Some(EnumPayloadValue::Single { ty, value }) = self.value.resolve_payload(payload)
            else {
                return Err(out.fail(Error::shape_mismatch()));
            };
            let child = self.cursor.child(ty, out.reborrow())?;
            let result = Decoder::seed(child, value, seed, out.reborrow())?;
            self.consumed = true;
            Ok(result)
        })
    }

    fn struct_variant<V: Visitor<'de>>(
        self,
        _: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Error> {
        let fault = self.cursor.fault();
        fault.run(|mut out| {
            let payload = self.cursor.session().schema_as(
                |schema_out| self.variant.payload(schema_out),
                out.reborrow(),
            )?;
            let Some(EnumPayloadValue::Structure { fields, values }) =
                self.value.resolve_payload(payload)
            else {
                return Err(out.fail(Error::shape_mismatch()));
            };
            let mut access =
                MappingAccess::fields(self.cursor.reborrow(), fields, values, out.reborrow())?;
            let result = match visitor.visit_map(&mut access) {
                Ok(result) => result,
                Err(error) => return Err(out.fail(error)),
            };
            access.finish(out.reborrow())?;
            self.consumed = true;
            Ok(result)
        })
    }

    fn tuple_variant<V: Visitor<'de>>(self, _: usize, _: V) -> Result<V::Value, Error> {
        self.cursor.fault().unsupported()
    }
}
