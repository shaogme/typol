use super::resources::BoundedBuffer;
use core::{error::Error as CoreError, fmt};
use serde::{
    Serialize, Serializer,
    ser::{self, Impossible, SerializeSeq, SerializeStruct},
};

#[derive(Clone, Copy, Debug)]
pub(super) struct JsonError;

pub(super) struct JsonSerializer<'buffer> {
    output: &'buffer mut BoundedBuffer,
    pretty: bool,
    depth: usize,
}

pub(super) struct Compound<'serializer, 'buffer> {
    serializer: &'serializer mut JsonSerializer<'buffer>,
    elements: usize,
}

impl<'buffer> JsonSerializer<'buffer> {
    pub(super) fn new(output: &'buffer mut BoundedBuffer, pretty: bool) -> Self {
        Self {
            output,
            pretty,
            depth: 0,
        }
    }

    fn write(&mut self, bytes: &[u8]) -> Result<(), JsonError> {
        self.output.write_bytes(bytes).map_err(|_| JsonError)
    }

    fn write_string(&mut self, value: &str) -> Result<(), JsonError> {
        self.write(b"\"")?;
        let bytes = value.as_bytes();
        let mut start = 0;
        for (index, byte) in bytes.iter().copied().enumerate() {
            let escape = match byte {
                b'"' => Some(&b"\\\""[..]),
                b'\\' => Some(&b"\\\\"[..]),
                b'\x08' => Some(&b"\\b"[..]),
                b'\t' => Some(&b"\\t"[..]),
                b'\n' => Some(&b"\\n"[..]),
                b'\x0c' => Some(&b"\\f"[..]),
                b'\r' => Some(&b"\\r"[..]),
                0..=0x1f => None,
                _ => continue,
            };
            self.write(bytes.get(start..index).ok_or(JsonError)?)?;
            if let Some(escape) = escape {
                self.write(escape)?;
            } else {
                const HEX: &[u8; 16] = b"0123456789abcdef";
                let escaped = [
                    b'\\',
                    b'u',
                    b'0',
                    b'0',
                    HEX.get(usize::from(byte >> 4)).copied().ok_or(JsonError)?,
                    HEX.get(usize::from(byte & 0x0f))
                        .copied()
                        .ok_or(JsonError)?,
                ];
                self.write(&escaped)?;
            }
            start = index + 1;
        }
        self.write(bytes.get(start..).ok_or(JsonError)?)?;
        self.write(b"\"")
    }

    fn write_unsigned(&mut self, mut value: u128) -> Result<(), JsonError> {
        let mut digits = [0; 39];
        let mut start = digits.len();
        if value == 0 {
            start = start.checked_sub(1).ok_or(JsonError)?;
            *digits.get_mut(start).ok_or(JsonError)? = b'0';
        } else {
            while value != 0 {
                start = start.checked_sub(1).ok_or(JsonError)?;
                *digits.get_mut(start).ok_or(JsonError)? = b'0' + (value % 10) as u8;
                value /= 10;
            }
        }
        self.write(digits.get(start..).ok_or(JsonError)?)
    }

    fn write_signed(&mut self, value: i128) -> Result<(), JsonError> {
        if value < 0 {
            self.write(b"-")?;
        }
        self.write_unsigned(value.unsigned_abs())
    }

    fn open(&mut self, delimiter: u8) -> Result<(), JsonError> {
        self.write(&[delimiter])?;
        self.depth = self.depth.checked_add(1).ok_or(JsonError)?;
        Ok(())
    }

    fn start_item(&mut self, elements: usize) -> Result<(), JsonError> {
        if elements != 0 {
            self.write(b",")?;
        }
        if self.pretty {
            self.write(b"\n")?;
            self.indent(self.depth)?;
        }
        Ok(())
    }

    fn indent(&mut self, depth: usize) -> Result<(), JsonError> {
        for _ in 0..depth {
            self.write(b"  ")?;
        }
        Ok(())
    }

    fn close(&mut self, delimiter: u8, elements: usize) -> Result<(), JsonError> {
        if elements != 0 && self.pretty {
            self.write(b"\n")?;
            self.indent(self.depth.saturating_sub(1))?;
        }
        self.depth = self.depth.saturating_sub(1);
        self.write(&[delimiter])
    }

    fn unsupported<T>() -> Result<T, JsonError> {
        Err(JsonError)
    }
}

impl fmt::Display for JsonError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Config IR JSON serialization failed")
    }
}

impl CoreError for JsonError {}

impl ser::Error for JsonError {
    fn custom<T: fmt::Display>(_message: T) -> Self {
        Self
    }
}

impl<'serializer, 'buffer> Serializer for &'serializer mut JsonSerializer<'buffer> {
    type Ok = ();
    type Error = JsonError;
    type SerializeSeq = Compound<'serializer, 'buffer>;
    type SerializeTuple = Impossible<(), JsonError>;
    type SerializeTupleStruct = Impossible<(), JsonError>;
    type SerializeTupleVariant = Impossible<(), JsonError>;
    type SerializeMap = Impossible<(), JsonError>;
    type SerializeStruct = Compound<'serializer, 'buffer>;
    type SerializeStructVariant = Impossible<(), JsonError>;

    fn serialize_bool(self, value: bool) -> Result<Self::Ok, Self::Error> {
        self.write(if value { b"true" } else { b"false" })
    }

    fn serialize_i8(self, value: i8) -> Result<Self::Ok, Self::Error> {
        self.write_signed(i128::from(value))
    }

    fn serialize_i16(self, value: i16) -> Result<Self::Ok, Self::Error> {
        self.write_signed(i128::from(value))
    }

    fn serialize_i32(self, value: i32) -> Result<Self::Ok, Self::Error> {
        self.write_signed(i128::from(value))
    }

    fn serialize_i64(self, value: i64) -> Result<Self::Ok, Self::Error> {
        self.write_signed(i128::from(value))
    }

    fn serialize_i128(self, value: i128) -> Result<Self::Ok, Self::Error> {
        self.write_signed(value)
    }

    fn serialize_u8(self, value: u8) -> Result<Self::Ok, Self::Error> {
        self.write_unsigned(u128::from(value))
    }

    fn serialize_u16(self, value: u16) -> Result<Self::Ok, Self::Error> {
        self.write_unsigned(u128::from(value))
    }

    fn serialize_u32(self, value: u32) -> Result<Self::Ok, Self::Error> {
        self.write_unsigned(u128::from(value))
    }

    fn serialize_u64(self, value: u64) -> Result<Self::Ok, Self::Error> {
        self.write_unsigned(u128::from(value))
    }

    fn serialize_u128(self, value: u128) -> Result<Self::Ok, Self::Error> {
        self.write_unsigned(value)
    }

    fn serialize_f32(self, _value: f32) -> Result<Self::Ok, Self::Error> {
        JsonSerializer::unsupported()
    }

    fn serialize_f64(self, _value: f64) -> Result<Self::Ok, Self::Error> {
        JsonSerializer::unsupported()
    }

    fn serialize_char(self, value: char) -> Result<Self::Ok, Self::Error> {
        let mut encoded = [0; 4];
        self.write_string(value.encode_utf8(&mut encoded))
    }

    fn serialize_str(self, value: &str) -> Result<Self::Ok, Self::Error> {
        self.write_string(value)
    }

    fn serialize_bytes(self, _value: &[u8]) -> Result<Self::Ok, Self::Error> {
        JsonSerializer::unsupported()
    }

    fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
        self.write(b"null")
    }

    fn serialize_some<T>(self, value: &T) -> Result<Self::Ok, Self::Error>
    where
        T: ?Sized + Serialize,
    {
        value.serialize(self)
    }

    fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
        self.write(b"null")
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<Self::Ok, Self::Error> {
        self.write(b"null")
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
    ) -> Result<Self::Ok, Self::Error> {
        JsonSerializer::unsupported()
    }

    fn serialize_newtype_struct<T>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error>
    where
        T: ?Sized + Serialize,
    {
        value.serialize(self)
    }

    fn serialize_newtype_variant<T>(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _value: &T,
    ) -> Result<Self::Ok, Self::Error>
    where
        T: ?Sized + Serialize,
    {
        JsonSerializer::unsupported()
    }

    fn serialize_seq(self, _length: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        self.open(b'[')?;
        Ok(Compound {
            serializer: self,
            elements: 0,
        })
    }

    fn serialize_tuple(self, _length: usize) -> Result<Self::SerializeTuple, Self::Error> {
        JsonSerializer::unsupported()
    }

    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        _length: usize,
    ) -> Result<Self::SerializeTupleStruct, Self::Error> {
        JsonSerializer::unsupported()
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _length: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        JsonSerializer::unsupported()
    }

    fn serialize_map(self, _length: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        JsonSerializer::unsupported()
    }

    fn serialize_struct(
        self,
        _name: &'static str,
        _length: usize,
    ) -> Result<Self::SerializeStruct, Self::Error> {
        self.open(b'{')?;
        Ok(Compound {
            serializer: self,
            elements: 0,
        })
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _length: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        JsonSerializer::unsupported()
    }
}

impl SerializeSeq for Compound<'_, '_> {
    type Ok = ();
    type Error = JsonError;

    fn serialize_element<T>(&mut self, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize,
    {
        self.serializer.start_item(self.elements)?;
        value.serialize(&mut *self.serializer)?;
        self.elements = self.elements.checked_add(1).ok_or(JsonError)?;
        Ok(())
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.serializer.close(b']', self.elements)
    }
}

impl SerializeStruct for Compound<'_, '_> {
    type Ok = ();
    type Error = JsonError;

    fn serialize_field<T>(&mut self, key: &'static str, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize,
    {
        self.serializer.start_item(self.elements)?;
        self.serializer.write_string(key)?;
        self.serializer
            .write(if self.serializer.pretty { b": " } else { b":" })?;
        value.serialize(&mut *self.serializer)?;
        self.elements = self.elements.checked_add(1).ok_or(JsonError)?;
        Ok(())
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.serializer.close(b'}', self.elements)
    }
}
