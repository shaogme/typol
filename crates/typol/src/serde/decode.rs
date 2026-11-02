use crate::{
    serde::{
        access::{EnumerationAccess, MappingAccess, SequenceAccess},
        model::{Cursor, Error, Fault},
    },
    value_walk::{ValueNode, ValueNodeKind},
};
use core::cell::Cell;
use opeo::{OResult, Out, ResultOutExt};
use serde::{
    Deserializer as DeTrait,
    de::{DeserializeSeed, Visitor},
};
use typol_core::ValueView;
use typol_model::BuiltinType;
use typol_semantic::TypeKind;

pub(crate) struct Decoder<'a, 'schema, 'call, 'de, 'used> {
    cursor: Cursor<'a, 'schema, 'call>,
    value: ValueView<'de>,
    used: &'used Cell<bool>,
}

impl<'a, 'schema, 'call, 'de, 'used> Decoder<'a, 'schema, 'call, 'de, 'used> {
    pub(crate) fn new(
        cursor: Cursor<'a, 'schema, 'call>,
        value: ValueView<'de>,
        used: &'used Cell<bool>,
    ) -> Self {
        Self {
            cursor,
            value,
            used,
        }
    }

    pub(crate) fn seed<'slot, S: DeserializeSeed<'de>>(
        cursor: Cursor<'a, 'schema, 'call>,
        value: ValueView<'de>,
        seed: S,
        mut out: Out<'slot, '_, Error>,
    ) -> OResult<'slot, S::Value> {
        let used = Cell::new(false);
        let result = seed
            .deserialize(Decoder::new(cursor, value, &used))
            .store_err_into(out.reborrow())?;
        if used.get() {
            Ok(result)
        } else {
            Err(out.fail(Error::shape_mismatch()))
        }
    }

    fn scalar<'slot>(
        &self,
        expected: BuiltinType,
        mut out: Out<'slot, '_, Error>,
    ) -> OResult<'slot, ValueNodeKind<'schema, 'de>> {
        let kind = self.cursor.session().schema_as(
            |schema_out| self.cursor.ty().kind(schema_out),
            out.reborrow(),
        )?;
        let node = ValueNode::resolve(kind, self.value)
            .ok_or_else(Error::shape_mismatch)
            .store_err_into(out.reborrow())?;
        if node.builtin() != Some(expected) {
            Err(out.fail(Error::shape_mismatch()))
        } else {
            self.used.set(true);
            Ok(node)
        }
    }

    fn signed<'slot, T>(
        &self,
        expected: BuiltinType,
        mut out: Out<'slot, '_, Error>,
    ) -> OResult<'slot, T>
    where
        T: TryFrom<i128>,
    {
        let ValueNodeKind::Signed { value, .. } = self.scalar(expected, out.reborrow())? else {
            return Err(out.fail(Error::shape_mismatch()));
        };
        match T::try_from(value) {
            Ok(value) => Ok(value),
            Err(_) => Err(out.fail(Error::shape_mismatch())),
        }
    }

    fn unsigned<'slot, T>(
        &self,
        expected: BuiltinType,
        mut out: Out<'slot, '_, Error>,
    ) -> OResult<'slot, T>
    where
        T: TryFrom<u128>,
    {
        let ValueNodeKind::Unsigned { value, .. } = self.scalar(expected, out.reborrow())? else {
            return Err(out.fail(Error::shape_mismatch()));
        };
        match T::try_from(value) {
            Ok(value) => Ok(value),
            Err(_) => Err(out.fail(Error::shape_mismatch())),
        }
    }

    fn visit_signed<'slot, T, R>(
        &self,
        expected: BuiltinType,
        visitor: impl FnOnce(T) -> Result<R, Error>,
        mut out: Out<'slot, '_, Error>,
    ) -> OResult<'slot, R>
    where
        T: TryFrom<i128>,
    {
        let value = self.signed(expected, out.reborrow())?;
        Fault::boundary(visitor(value), out)
    }

    fn visit_unsigned<'slot, T, R>(
        &self,
        expected: BuiltinType,
        visitor: impl FnOnce(T) -> Result<R, Error>,
        mut out: Out<'slot, '_, Error>,
    ) -> OResult<'slot, R>
    where
        T: TryFrom<u128>,
    {
        let value = self.unsigned(expected, out.reborrow())?;
        Fault::boundary(visitor(value), out)
    }
}

impl<'de> DeTrait<'de> for Decoder<'_, '_, '_, 'de, '_> {
    type Error = Error;

    fn deserialize_i8<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        let fault = self.cursor.fault();
        fault.run(|out| self.visit_signed(BuiltinType::I8, |value| visitor.visit_i8(value), out))
    }

    fn deserialize_i16<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        let fault = self.cursor.fault();
        fault.run(|out| self.visit_signed(BuiltinType::I16, |value| visitor.visit_i16(value), out))
    }

    fn deserialize_i32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        let fault = self.cursor.fault();
        fault.run(|out| self.visit_signed(BuiltinType::I32, |value| visitor.visit_i32(value), out))
    }

    fn deserialize_i64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        let fault = self.cursor.fault();
        fault.run(|out| self.visit_signed(BuiltinType::I64, |value| visitor.visit_i64(value), out))
    }

    fn deserialize_i128<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        let fault = self.cursor.fault();
        fault
            .run(|out| self.visit_signed(BuiltinType::I128, |value| visitor.visit_i128(value), out))
    }

    fn deserialize_u8<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        let fault = self.cursor.fault();
        fault.run(|out| self.visit_unsigned(BuiltinType::U8, |value| visitor.visit_u8(value), out))
    }

    fn deserialize_u16<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        let fault = self.cursor.fault();
        fault
            .run(|out| self.visit_unsigned(BuiltinType::U16, |value| visitor.visit_u16(value), out))
    }

    fn deserialize_u32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        let fault = self.cursor.fault();
        fault
            .run(|out| self.visit_unsigned(BuiltinType::U32, |value| visitor.visit_u32(value), out))
    }

    fn deserialize_u64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        let fault = self.cursor.fault();
        fault
            .run(|out| self.visit_unsigned(BuiltinType::U64, |value| visitor.visit_u64(value), out))
    }

    fn deserialize_u128<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        let fault = self.cursor.fault();
        fault.run(|out| {
            self.visit_unsigned(BuiltinType::U128, |value| visitor.visit_u128(value), out)
        })
    }

    fn deserialize_f32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        let fault = self.cursor.fault();
        fault.run(|mut out| {
            let ValueNodeKind::Float32(value) = self.scalar(BuiltinType::F32, out.reborrow())?
            else {
                return Err(out.fail(Error::shape_mismatch()));
            };
            if !value.is_finite() {
                return Err(out.fail(Error::shape_mismatch()));
            }
            Fault::boundary(visitor.visit_f32(value), out)
        })
    }

    fn deserialize_f64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        let fault = self.cursor.fault();
        fault.run(|mut out| {
            let ValueNodeKind::Float64(value) = self.scalar(BuiltinType::F64, out.reborrow())?
            else {
                return Err(out.fail(Error::shape_mismatch()));
            };
            if !value.is_finite() {
                return Err(out.fail(Error::shape_mismatch()));
            }
            Fault::boundary(visitor.visit_f64(value), out)
        })
    }

    fn deserialize_bool<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        let fault = self.cursor.fault();
        fault.run(|mut out| {
            let ValueNodeKind::Bool(value) = self.scalar(BuiltinType::Bool, out.reborrow())? else {
                return Err(out.fail(Error::shape_mismatch()));
            };
            Fault::boundary(visitor.visit_bool(value), out)
        })
    }

    fn deserialize_str<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        let fault = self.cursor.fault();
        fault.run(|mut out| {
            let ValueNodeKind::String(value) = self.scalar(BuiltinType::String, out.reborrow())?
            else {
                return Err(out.fail(Error::shape_mismatch()));
            };
            self.cursor.session().budget_as(
                |budget_out| {
                    self.cursor
                        .session()
                        .execution()
                        .consume(value.len(), budget_out)
                },
                out.reborrow(),
            )?;
            Fault::boundary(visitor.visit_str(value), out)
        })
    }

    fn deserialize_string<V: Visitor<'de>>(mut self, visitor: V) -> Result<V::Value, Error> {
        let fault = self.cursor.fault();
        fault.run(|mut out| {
            let ValueNodeKind::String(value) = self.scalar(BuiltinType::String, out.reborrow())?
            else {
                return Err(out.fail(Error::shape_mismatch()));
            };
            let value = self.cursor.target_text(value, out.reborrow())?;
            Fault::boundary(visitor.visit_string(value), out)
        })
    }

    fn deserialize_option<V: Visitor<'de>>(mut self, visitor: V) -> Result<V::Value, Error> {
        let fault = self.cursor.fault();
        fault.run(|mut out| {
            let kind = self.cursor.session().schema_as(
                |schema_out| self.cursor.ty().kind(schema_out),
                out.reborrow(),
            )?;
            let Some(ValueNodeKind::Optional { inner, value }) =
                ValueNode::resolve(kind, self.value)
            else {
                return Err(out.fail(Error::shape_mismatch()));
            };
            let inner_kind = self
                .cursor
                .session()
                .schema_as(|schema_out| inner.kind(schema_out), out.reborrow())?;
            if matches!(inner_kind, TypeKind::Option { .. }) {
                return Err(out.fail(Error::shape_mismatch()));
            }
            self.used.set(true);
            match value {
                None => Fault::boundary(visitor.visit_none(), out),
                Some(value) => {
                    let child = self.cursor.child(inner, out.reborrow())?;
                    let used = Cell::new(false);
                    let result = match visitor.visit_some(Decoder::new(child, value, &used)) {
                        Ok(value) => value,
                        Err(error) => return Err(out.fail(error)),
                    };
                    if used.get() {
                        Ok(result)
                    } else {
                        Err(out.fail(Error::shape_mismatch()))
                    }
                }
            }
        })
    }

    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        let fault = self.cursor.fault();
        fault.run(|mut out| {
            let mut access = SequenceAccess::new(self.cursor, self.value, None, out.reborrow())?;
            self.used.set(true);
            let result = match visitor.visit_seq(&mut access) {
                Ok(value) => value,
                Err(error) => return Err(out.fail(error)),
            };
            access.finish(out.reborrow())?;
            Ok(result)
        })
    }

    fn deserialize_tuple<V: Visitor<'de>>(self, len: usize, visitor: V) -> Result<V::Value, Error> {
        let fault = self.cursor.fault();
        fault.run(|mut out| {
            let mut access =
                SequenceAccess::new(self.cursor, self.value, Some(len), out.reborrow())?;
            self.used.set(true);
            let result = match visitor.visit_seq(&mut access) {
                Ok(value) => value,
                Err(error) => return Err(out.fail(error)),
            };
            access.finish(out.reborrow())?;
            Ok(result)
        })
    }

    fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        let fault = self.cursor.fault();
        fault.run(|mut out| {
            let mut access = MappingAccess::map(self.cursor, self.value, out.reborrow())?;
            self.used.set(true);
            let result = match visitor.visit_map(&mut access) {
                Ok(value) => value,
                Err(error) => return Err(out.fail(error)),
            };
            access.finish(out.reborrow())?;
            Ok(result)
        })
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        _: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Error> {
        let fault = self.cursor.fault();
        fault.run(|mut out| {
            self.cursor.name(name, out.reborrow())?;
            let kind = self.cursor.session().schema_as(
                |schema_out| self.cursor.ty().kind(schema_out),
                out.reborrow(),
            )?;
            let Some(ValueNodeKind::Structure { fields, values }) =
                ValueNode::resolve(kind, self.value)
            else {
                return Err(out.fail(Error::shape_mismatch()));
            };
            let mut access = MappingAccess::fields(self.cursor, fields, values, out.reborrow())?;
            self.used.set(true);
            let result = match visitor.visit_map(&mut access) {
                Ok(value) => value,
                Err(error) => return Err(out.fail(error)),
            };
            access.finish(out.reborrow())?;
            Ok(result)
        })
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        _: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Error> {
        let fault = self.cursor.fault();
        fault.run(|mut out| {
            self.cursor.name(name, out.reborrow())?;
            let mut access = EnumerationAccess::new(self.cursor, self.value, out.reborrow())?;
            self.used.set(true);
            let result = match visitor.visit_enum(&mut access) {
                Ok(value) => value,
                Err(error) => return Err(out.fail(error)),
            };
            access.finish(out.reborrow())?;
            Ok(result)
        })
    }

    fn deserialize_any<V: Visitor<'de>>(self, _visitor: V) -> Result<V::Value, Error> {
        self.cursor.fault().unsupported()
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, _visitor: V) -> Result<V::Value, Error> {
        self.cursor.fault().unsupported()
    }

    fn deserialize_identifier<V: Visitor<'de>>(self, _visitor: V) -> Result<V::Value, Error> {
        self.cursor.fault().unsupported()
    }

    fn deserialize_char<V: Visitor<'de>>(self, _visitor: V) -> Result<V::Value, Error> {
        self.cursor.fault().unsupported()
    }

    fn deserialize_bytes<V: Visitor<'de>>(self, _visitor: V) -> Result<V::Value, Error> {
        self.cursor.fault().unsupported()
    }

    fn deserialize_byte_buf<V: Visitor<'de>>(self, _visitor: V) -> Result<V::Value, Error> {
        self.cursor.fault().unsupported()
    }

    fn deserialize_unit<V: Visitor<'de>>(self, _visitor: V) -> Result<V::Value, Error> {
        self.cursor.fault().unsupported()
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _visitor: V,
    ) -> Result<V::Value, Error> {
        self.cursor.fault().unsupported()
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _visitor: V,
    ) -> Result<V::Value, Error> {
        self.cursor.fault().unsupported()
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _len: usize,
        _visitor: V,
    ) -> Result<V::Value, Error> {
        self.cursor.fault().unsupported()
    }

    fn is_human_readable(&self) -> bool {
        true
    }
}
