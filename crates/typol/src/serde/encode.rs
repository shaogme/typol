use crate::{
    serde::{
        model::{Cursor, Error},
        serialize_containers::{Fields, Mapping, Sequence},
    },
    value::{EnumPayload, ValueFactory},
};
use core::fmt;
use opeo::{OResult, Out};
use serde::{Serialize, Serializer as SerTrait, ser::Impossible};
use typol_core::ConfigValue;
use typol_model::BuiltinType;
use typol_semantic::{TypeKind, VariantPayloadInfo};

pub(crate) struct Encoder<'a, 'schema, 'call> {
    cursor: Cursor<'a, 'schema, 'call>,
}

impl<'a, 'schema, 'call> Encoder<'a, 'schema, 'call> {
    pub(crate) fn new(cursor: Cursor<'a, 'schema, 'call>) -> Self {
        Self { cursor }
    }

    fn scalar<'slot>(
        &self,
        ty: BuiltinType,
        value: ConfigValue,
        mut out: Out<'slot, '_, Error>,
    ) -> OResult<'slot, ConfigValue> {
        let kind = self.cursor.session().schema_as(
            |schema_out| self.cursor.ty().kind(schema_out),
            out.reborrow(),
        )?;
        if matches!(kind, TypeKind::Builtin(expected) if expected == ty) {
            Ok(value)
        } else {
            Err(out.fail(Error::shape_mismatch()))
        }
    }

    fn text<'slot>(
        &self,
        text: &str,
        mut out: Out<'slot, '_, Error>,
    ) -> OResult<'slot, ConfigValue> {
        let kind = self.cursor.session().schema_as(
            |schema_out| self.cursor.ty().kind(schema_out),
            out.reborrow(),
        )?;
        if !matches!(kind, TypeKind::Builtin(BuiltinType::String)) {
            return Err(out.fail(Error::shape_mismatch()));
        }
        self.cursor.text(text, out.reborrow())?;
        self.cursor.session().value_as(
            |value_out| {
                ValueFactory::string(text, Some(self.cursor.session().execution()), value_out)
            },
            out,
        )
    }

    fn signed_scalar<'slot>(
        &self,
        ty: BuiltinType,
        value: i128,
        mut out: Out<'slot, '_, Error>,
    ) -> OResult<'slot, ConfigValue> {
        let value = self.cursor.session().value_as(
            |value_out| {
                ValueFactory::signed(value, Some(self.cursor.session().execution()), value_out)
            },
            out.reborrow(),
        )?;
        self.scalar(ty, value, out)
    }

    fn unsigned_scalar<'slot>(
        &self,
        ty: BuiltinType,
        value: u128,
        mut out: Out<'slot, '_, Error>,
    ) -> OResult<'slot, ConfigValue> {
        let value = self.cursor.session().value_as(
            |value_out| {
                ValueFactory::unsigned(value, Some(self.cursor.session().execution()), value_out)
            },
            out.reborrow(),
        )?;
        self.scalar(ty, value, out)
    }

    fn boolean_scalar<'slot>(
        &self,
        value: bool,
        mut out: Out<'slot, '_, Error>,
    ) -> OResult<'slot, ConfigValue> {
        let value = self.cursor.session().value_as(
            |value_out| {
                ValueFactory::boolean(value, Some(self.cursor.session().execution()), value_out)
            },
            out.reborrow(),
        )?;
        self.scalar(BuiltinType::Bool, value, out)
    }

    fn float32_scalar<'slot>(
        &self,
        value: f32,
        mut out: Out<'slot, '_, Error>,
    ) -> OResult<'slot, ConfigValue> {
        let value = self.cursor.session().value_as(
            |value_out| {
                ValueFactory::float32(value, Some(self.cursor.session().execution()), value_out)
            },
            out.reborrow(),
        )?;
        self.scalar(BuiltinType::F32, value, out)
    }

    fn float64_scalar<'slot>(
        &self,
        value: f64,
        mut out: Out<'slot, '_, Error>,
    ) -> OResult<'slot, ConfigValue> {
        let value = self.cursor.session().value_as(
            |value_out| {
                ValueFactory::float64(value, Some(self.cursor.session().execution()), value_out)
            },
            out.reborrow(),
        )?;
        self.scalar(BuiltinType::F64, value, out)
    }
}

impl<'a, 'schema, 'call> SerTrait for Encoder<'a, 'schema, 'call> {
    type Ok = ConfigValue;
    type Error = Error;
    type SerializeSeq = Sequence<'a, 'schema, 'call>;
    type SerializeTuple = Sequence<'a, 'schema, 'call>;
    type SerializeTupleStruct = Impossible<ConfigValue, Error>;
    type SerializeTupleVariant = Impossible<ConfigValue, Error>;
    type SerializeMap = Mapping<'a, 'schema, 'call>;
    type SerializeStruct = Fields<'a, 'schema, 'call>;
    type SerializeStructVariant = Fields<'a, 'schema, 'call>;

    fn serialize_i8(self, value: i8) -> Result<Self::Ok, Error> {
        let fault = self.cursor.fault();
        fault.run(|out| self.signed_scalar(BuiltinType::I8, value as i128, out))
    }

    fn serialize_i16(self, value: i16) -> Result<Self::Ok, Error> {
        let fault = self.cursor.fault();
        fault.run(|out| self.signed_scalar(BuiltinType::I16, value as i128, out))
    }

    fn serialize_i32(self, value: i32) -> Result<Self::Ok, Error> {
        let fault = self.cursor.fault();
        fault.run(|out| self.signed_scalar(BuiltinType::I32, value as i128, out))
    }

    fn serialize_i64(self, value: i64) -> Result<Self::Ok, Error> {
        let fault = self.cursor.fault();
        fault.run(|out| self.signed_scalar(BuiltinType::I64, value as i128, out))
    }

    fn serialize_i128(self, value: i128) -> Result<Self::Ok, Error> {
        let fault = self.cursor.fault();
        fault.run(|out| self.signed_scalar(BuiltinType::I128, value, out))
    }

    fn serialize_u8(self, value: u8) -> Result<Self::Ok, Error> {
        let fault = self.cursor.fault();
        fault.run(|out| self.unsigned_scalar(BuiltinType::U8, value as u128, out))
    }

    fn serialize_u16(self, value: u16) -> Result<Self::Ok, Error> {
        let fault = self.cursor.fault();
        fault.run(|out| self.unsigned_scalar(BuiltinType::U16, value as u128, out))
    }

    fn serialize_u32(self, value: u32) -> Result<Self::Ok, Error> {
        let fault = self.cursor.fault();
        fault.run(|out| self.unsigned_scalar(BuiltinType::U32, value as u128, out))
    }

    fn serialize_u64(self, value: u64) -> Result<Self::Ok, Error> {
        let fault = self.cursor.fault();
        fault.run(|out| self.unsigned_scalar(BuiltinType::U64, value as u128, out))
    }

    fn serialize_u128(self, value: u128) -> Result<Self::Ok, Error> {
        let fault = self.cursor.fault();
        fault.run(|out| self.unsigned_scalar(BuiltinType::U128, value, out))
    }

    fn serialize_f32(self, value: f32) -> Result<Self::Ok, Error> {
        let fault = self.cursor.fault();
        fault.run(|out| {
            if value.is_finite() {
                self.float32_scalar(value, out)
            } else {
                Err(out.fail(Error::shape_mismatch()))
            }
        })
    }

    fn serialize_f64(self, value: f64) -> Result<Self::Ok, Error> {
        let fault = self.cursor.fault();
        fault.run(|out| {
            if value.is_finite() {
                self.float64_scalar(value, out)
            } else {
                Err(out.fail(Error::shape_mismatch()))
            }
        })
    }

    fn serialize_bool(self, value: bool) -> Result<Self::Ok, Error> {
        let fault = self.cursor.fault();
        fault.run(|out| self.boolean_scalar(value, out))
    }

    fn serialize_str(self, value: &str) -> Result<Self::Ok, Error> {
        let fault = self.cursor.fault();
        fault.run(|out| self.text(value, out))
    }

    fn serialize_none(self) -> Result<Self::Ok, Error> {
        let fault = self.cursor.fault();
        fault.run(|mut out| {
            let kind = self.cursor.session().schema_as(
                |schema_out| self.cursor.ty().kind(schema_out),
                out.reborrow(),
            )?;
            if matches!(kind, TypeKind::Option { .. }) {
                self.cursor.session().value_as(
                    |value_out| {
                        ValueFactory::none(Some(self.cursor.session().execution()), value_out)
                    },
                    out,
                )
            } else {
                Err(out.fail(Error::shape_mismatch()))
            }
        })
    }

    fn serialize_some<T: Serialize + ?Sized>(mut self, value: &T) -> Result<Self::Ok, Error> {
        let fault = self.cursor.fault();
        fault.run(|mut out| {
            let kind = self.cursor.session().schema_as(
                |schema_out| self.cursor.ty().kind(schema_out),
                out.reborrow(),
            )?;
            let TypeKind::Option { inner } = kind else {
                return Err(out.fail(Error::shape_mismatch()));
            };
            let inner_kind = self
                .cursor
                .session()
                .schema_as(|schema_out| inner.kind(schema_out), out.reborrow())?;
            if matches!(inner_kind, TypeKind::Option { .. }) {
                return Err(out.fail(Error::shape_mismatch()));
            }
            let child = self.cursor.child(inner, out.reborrow())?;
            let value = match value.serialize(Encoder::new(child)) {
                Ok(value) => value,
                Err(error) => return Err(out.fail(error)),
            };
            self.cursor.session().value_as(
                |value_out| {
                    ValueFactory::optional(
                        Some(value),
                        Some(self.cursor.session().execution()),
                        value_out,
                    )
                },
                out,
            )
        })
    }

    fn serialize_seq(self, _: Option<usize>) -> Result<Self::SerializeSeq, Error> {
        let fault = self.cursor.fault();
        fault.run(|out| Sequence::new(self.cursor, None, out))
    }

    fn serialize_tuple(self, len: usize) -> Result<Self::SerializeTuple, Error> {
        let fault = self.cursor.fault();
        fault.run(|out| Sequence::new(self.cursor, Some(len), out))
    }

    fn serialize_map(self, _: Option<usize>) -> Result<Self::SerializeMap, Error> {
        let fault = self.cursor.fault();
        fault.run(|out| Mapping::new(self.cursor, out))
    }

    fn serialize_struct(
        self,
        name: &'static str,
        _: usize,
    ) -> Result<Self::SerializeStruct, Error> {
        let fault = self.cursor.fault();
        fault.run(|mut out| {
            self.cursor.name(name, out.reborrow())?;
            let kind = self.cursor.session().schema_as(
                |schema_out| self.cursor.ty().kind(schema_out),
                out.reborrow(),
            )?;
            let TypeKind::Struct { fields } = kind else {
                return Err(out.fail(Error::shape_mismatch()));
            };
            Fields::new(self.cursor, fields, None, out)
        })
    }

    fn serialize_unit_variant(
        self,
        name: &'static str,
        _: u32,
        variant_name: &'static str,
    ) -> Result<Self::Ok, Error> {
        let fault = self.cursor.fault();
        fault.run(|mut out| {
            let variant = self.cursor.variant(name, variant_name, out.reborrow())?;
            let payload = self
                .cursor
                .session()
                .schema_as(|schema_out| variant.payload(schema_out), out.reborrow())?;
            if !matches!(payload, VariantPayloadInfo::Unit) {
                return Err(out.fail(Error::shape_mismatch()));
            }
            let name = self
                .cursor
                .session()
                .schema_as(|schema_out| variant.name(schema_out), out.reborrow())?;
            self.cursor.text(name, out.reborrow())?;
            self.cursor.session().value_as(
                |value_out| {
                    ValueFactory::enumeration(
                        name,
                        EnumPayload::unit(),
                        Some(self.cursor.session().execution()),
                        value_out,
                    )
                },
                out,
            )
        })
    }

    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        mut self,
        name: &'static str,
        _: u32,
        variant_name: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Error> {
        let fault = self.cursor.fault();
        fault.run(|mut out| {
            let variant = self.cursor.variant(name, variant_name, out.reborrow())?;
            let payload = self
                .cursor
                .session()
                .schema_as(|schema_out| variant.payload(schema_out), out.reborrow())?;
            let VariantPayloadInfo::Single { ty } = payload else {
                return Err(out.fail(Error::shape_mismatch()));
            };
            let child = self.cursor.child(ty, out.reborrow())?;
            let value = match value.serialize(Encoder::new(child)) {
                Ok(value) => value,
                Err(error) => return Err(out.fail(error)),
            };
            let payload = EnumPayload::single(value);
            let name = self
                .cursor
                .session()
                .schema_as(|schema_out| variant.name(schema_out), out.reborrow())?;
            self.cursor.text(name, out.reborrow())?;
            self.cursor.session().value_as(
                |value_out| {
                    ValueFactory::enumeration(
                        name,
                        payload,
                        Some(self.cursor.session().execution()),
                        value_out,
                    )
                },
                out,
            )
        })
    }

    fn serialize_struct_variant(
        self,
        name: &'static str,
        _: u32,
        variant_name: &'static str,
        _: usize,
    ) -> Result<Self::SerializeStructVariant, Error> {
        let fault = self.cursor.fault();
        fault.run(|mut out| {
            let variant = self.cursor.variant(name, variant_name, out.reborrow())?;
            let payload = self
                .cursor
                .session()
                .schema_as(|schema_out| variant.payload(schema_out), out.reborrow())?;
            let VariantPayloadInfo::Structure { fields } = payload else {
                return Err(out.fail(Error::shape_mismatch()));
            };
            let variant_name = self
                .cursor
                .session()
                .schema_as(|schema_out| variant.name(schema_out), out.reborrow())?;
            Fields::new(self.cursor, fields, Some(variant_name), out)
        })
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

    fn is_human_readable(&self) -> bool {
        true
    }
}
