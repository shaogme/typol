use crate::serde::model::{Cursor, Error, Fault};
use core::fmt;
use serde::{
    Deserializer, Serialize, Serializer, de::Visitor, forward_to_deserialize_any, ser::Impossible,
};
use typol_core::ChargedString;

pub(crate) struct Key {
    text: ChargedString,
}

impl Key {
    pub(crate) fn text(&self) -> &str {
        self.text.as_str()
    }

    pub(crate) fn into_name(self) -> ChargedString {
        self.text
    }
}

pub(crate) struct StringSerializer<'a, 'b, 'schema, 'call> {
    cursor: &'a Cursor<'b, 'schema, 'call>,
}

impl<'a, 'b, 'schema, 'call> StringSerializer<'a, 'b, 'schema, 'call> {
    pub(crate) fn new(cursor: &'a Cursor<'b, 'schema, 'call>) -> Self {
        Self { cursor }
    }
}

impl Serializer for StringSerializer<'_, '_, '_, '_> {
    type Ok = Key;
    type Error = Error;
    type SerializeSeq = Impossible<Key, Error>;
    type SerializeTuple = Impossible<Key, Error>;
    type SerializeTupleStruct = Impossible<Key, Error>;
    type SerializeTupleVariant = Impossible<Key, Error>;
    type SerializeMap = Impossible<Key, Error>;
    type SerializeStruct = Impossible<Key, Error>;
    type SerializeStructVariant = Impossible<Key, Error>;

    fn serialize_str(self, value: &str) -> Result<Self::Ok, Error> {
        let fault = self.cursor.fault();
        fault.run(|mut out| {
            let text = self.cursor.session().budget_as(
                |budget_out| self.cursor.session().copy_text(value, budget_out),
                out.reborrow(),
            )?;
            Ok(Key { text })
        })
    }

    fn serialize_i8(self, _: i8) -> Result<Self::Ok, Error> {
        self.cursor.fault().unsupported()
    }

    fn serialize_i16(self, _: i16) -> Result<Self::Ok, Error> {
        self.cursor.fault().unsupported()
    }

    fn serialize_i32(self, _: i32) -> Result<Self::Ok, Error> {
        self.cursor.fault().unsupported()
    }

    fn serialize_i64(self, _: i64) -> Result<Self::Ok, Error> {
        self.cursor.fault().unsupported()
    }

    fn serialize_i128(self, _: i128) -> Result<Self::Ok, Error> {
        self.cursor.fault().unsupported()
    }

    fn serialize_u8(self, _: u8) -> Result<Self::Ok, Error> {
        self.cursor.fault().unsupported()
    }

    fn serialize_u16(self, _: u16) -> Result<Self::Ok, Error> {
        self.cursor.fault().unsupported()
    }

    fn serialize_u32(self, _: u32) -> Result<Self::Ok, Error> {
        self.cursor.fault().unsupported()
    }

    fn serialize_u64(self, _: u64) -> Result<Self::Ok, Error> {
        self.cursor.fault().unsupported()
    }

    fn serialize_u128(self, _: u128) -> Result<Self::Ok, Error> {
        self.cursor.fault().unsupported()
    }

    fn serialize_char(self, _value: char) -> Result<Self::Ok, Error> {
        self.cursor.fault().unsupported()
    }

    fn serialize_bytes(self, _value: &[u8]) -> Result<Self::Ok, Error> {
        self.cursor.fault().unsupported()
    }

    fn serialize_unit(self) -> Result<Self::Ok, Error> {
        self.cursor.fault().unsupported()
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<Self::Ok, Error> {
        self.cursor.fault().unsupported()
    }

    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        _value: &T,
    ) -> Result<Self::Ok, Error> {
        self.cursor.fault().unsupported()
    }

    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleStruct, Error> {
        self.cursor.fault().unsupported()
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleVariant, Error> {
        self.cursor.fault().unsupported()
    }

    fn collect_str<T: fmt::Display + ?Sized>(self, _value: &T) -> Result<Self::Ok, Error> {
        self.cursor.fault().unsupported()
    }

    fn serialize_f32(self, _value: f32) -> Result<Self::Ok, Error> {
        self.cursor.fault().unsupported()
    }

    fn serialize_f64(self, _value: f64) -> Result<Self::Ok, Error> {
        self.cursor.fault().unsupported()
    }

    fn serialize_bool(self, _value: bool) -> Result<Self::Ok, Error> {
        self.cursor.fault().unsupported()
    }

    fn serialize_none(self) -> Result<Self::Ok, Error> {
        self.cursor.fault().unsupported()
    }

    fn serialize_some<T: Serialize + ?Sized>(self, _value: &T) -> Result<Self::Ok, Error> {
        self.cursor.fault().unsupported()
    }

    fn serialize_seq(self, _len: Option<usize>) -> Result<Self::SerializeSeq, Error> {
        self.cursor.fault().unsupported()
    }

    fn serialize_tuple(self, _len: usize) -> Result<Self::SerializeTuple, Error> {
        self.cursor.fault().unsupported()
    }

    fn serialize_map(self, _len: Option<usize>) -> Result<Self::SerializeMap, Error> {
        self.cursor.fault().unsupported()
    }

    fn serialize_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStruct, Error> {
        self.cursor.fault().unsupported()
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
    ) -> Result<Self::Ok, Error> {
        self.cursor.fault().unsupported()
    }

    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _value: &T,
    ) -> Result<Self::Ok, Error> {
        self.cursor.fault().unsupported()
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStructVariant, Error> {
        self.cursor.fault().unsupported()
    }
}

pub(crate) struct StringDeserializer<'a, 'b, 'schema, 'call, 'de> {
    text: &'de str,
    cursor: &'a mut Cursor<'b, 'schema, 'call>,
    identifier: bool,
    used: &'a mut bool,
}

impl<'a, 'b, 'schema, 'call, 'de> StringDeserializer<'a, 'b, 'schema, 'call, 'de> {
    pub(crate) fn new(
        text: &'de str,
        cursor: &'a mut Cursor<'b, 'schema, 'call>,
        identifier: bool,
        used: &'a mut bool,
    ) -> Self {
        Self {
            text,
            cursor,
            identifier,
            used,
        }
    }
}

impl<'de> Deserializer<'de> for StringDeserializer<'_, '_, '_, '_, '_> {
    type Error = Error;

    fn deserialize_str<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        let fault = self.cursor.fault();
        fault.run(|mut out| {
            if self.identifier {
                return Err(out.fail(Error::shape_mismatch()));
            }
            *self.used = true;
            self.cursor.session().budget_as(
                |budget_out| {
                    self.cursor
                        .session()
                        .execution()
                        .consume(self.text.len(), budget_out)
                },
                out.reborrow(),
            )?;
            Fault::boundary(visitor.visit_str(self.text), out)
        })
    }

    fn deserialize_string<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        let fault = self.cursor.fault();
        fault.run(|mut out| {
            if self.identifier {
                return Err(out.fail(Error::shape_mismatch()));
            }
            *self.used = true;
            let text = self.cursor.target_text(self.text, out.reborrow())?;
            Fault::boundary(visitor.visit_string(text), out)
        })
    }

    fn deserialize_identifier<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        let fault = self.cursor.fault();
        fault.run(|mut out| {
            if !self.identifier {
                return Err(out.fail(Error::shape_mismatch()));
            }
            *self.used = true;
            self.cursor.session().budget_as(
                |budget_out| {
                    self.cursor
                        .session()
                        .execution()
                        .consume(self.text.len(), budget_out)
                },
                out.reborrow(),
            )?;
            Fault::boundary(visitor.visit_str(self.text), out)
        })
    }

    fn deserialize_any<V: Visitor<'de>>(self, _: V) -> Result<V::Value, Error> {
        self.cursor.fault().unsupported()
    }

    forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char bytes byte_buf option unit unit_struct newtype_struct seq tuple tuple_struct map struct enum ignored_any
    }
}
