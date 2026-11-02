use super::support::{Identifier, NoConsume, SerdeTestSupport as Support};
use crate::{test_support::TestValues, value::EnumPayload};
use alloc::collections::BTreeMap;
use core::fmt;
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{EnumAccess, MapAccess, VariantAccess, Visitor},
    ser::{SerializeMap, SerializeTupleStruct, SerializeTupleVariant},
};
use typol_test_support::{TestExpect, TestResult, test_verify_eq as assert_eq};

struct Request<const MODE: u8>;

impl<'de, const MODE: u8> Deserialize<'de> for Request<MODE> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct RequestVisitor<const MODE: u8>;

        impl<'de, const MODE: u8> Visitor<'de> for RequestVisitor<MODE> {
            type Value = Request<MODE>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("incompatible access")
            }
        }
        match MODE {
            0 => deserializer.deserialize_any(RequestVisitor),
            1 => deserializer.deserialize_ignored_any(RequestVisitor),
            2 => deserializer.deserialize_bytes(RequestVisitor),
            3 => deserializer.deserialize_byte_buf(RequestVisitor),
            4 => deserializer.deserialize_newtype_struct("Root", RequestVisitor),
            5 => deserializer.deserialize_tuple_struct("Root", 0, RequestVisitor),
            6 => deserializer.deserialize_unit_struct("Root", RequestVisitor),
            _ => deserializer.deserialize_identifier(RequestVisitor),
        }
    }
}

#[test]
fn all_unsupported_deserializer_methods_reject() -> TestResult {
    let schema = Support::compile("entry struct Root { value: string, }")?;
    let value = TestValues::string_unmetered("secret content".to_owned());
    macro_rules! reject {
        ($mode:literal) => {
            let errors = Support::decode::<Request<$mode>>(&schema, &value)?
                .err()
                .test_value()?;
            let context = errors
                .diagnostic_context(0)
                .test_expect("unsupported decoding operations should preserve context")?;
            assert_eq!(
                context.failure_point(),
                "serde-data-model-operation-unsupported"
            );
            assert_eq!(Support::code(errors)?, 15);
        };
    }
    reject!(0);
    reject!(1);
    reject!(2);
    reject!(3);
    reject!(4);
    reject!(5);
    reject!(6);
    reject!(7);

    Ok(())
}

struct TupleVariantRequest;

impl<'de> Deserialize<'de> for TupleVariantRequest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct TupleVisitor;

        impl<'de> Visitor<'de> for TupleVisitor {
            type Value = TupleVariantRequest;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("tuple payload")
            }
        }

        struct EnumVisitor;

        impl<'de> Visitor<'de> for EnumVisitor {
            type Value = TupleVariantRequest;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("tuple enum variant")
            }

            fn visit_enum<A: EnumAccess<'de>>(self, access: A) -> Result<Self::Value, A::Error> {
                let (_, variant) = access.variant::<Identifier>()?;
                variant.tuple_variant(1, TupleVisitor)
            }
        }

        deserializer.deserialize_enum("Choice", &["Tuple"], EnumVisitor)
    }
}

#[test]
fn tuple_enum_variant_rejection_is_classified_as_unsupported() -> TestResult {
    let schema =
        Support::compile("enum Choice { Tuple(u8) } entry struct Root { value: Choice, }")?;
    let payload = TestValues::enumeration_unmetered(
        "Tuple".to_owned(),
        EnumPayload::single(TestValues::unsigned(7)),
    );
    let errors = Support::decode::<TupleVariantRequest>(&schema, &payload)?
        .err()
        .test_value()?;
    let context = errors
        .diagnostic_context(0)
        .test_expect("tuple enum variants should preserve unsupported-operation context")?;
    assert_eq!(
        context.failure_point(),
        "serde-data-model-operation-unsupported"
    );
    assert_eq!(Support::code(errors)?, 15);
    Ok(())
}

struct UnsupportedEvent(u8);

impl Serialize for UnsupportedEvent {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            0 => serializer.serialize_bytes(b"secret"),
            1 => serializer.serialize_unit_struct("Root"),
            2 => serializer.serialize_newtype_struct("Root", &1u8),
            3 => {
                let value = serializer.serialize_tuple_struct("Root", 0)?;
                SerializeTupleStruct::end(value)
            }
            4 => {
                let value = serializer.serialize_tuple_variant("Choice", 0, "Unit", 0)?;
                SerializeTupleVariant::end(value)
            }
            _ => serializer.collect_str("secret content"),
        }
    }
}

#[test]
fn all_unsupported_serializer_methods_reject() -> TestResult {
    let schema = Support::compile("entry struct Root { value: string, }")?;
    for mode in 0..=5 {
        let errors = Support::encode(&schema, &UnsupportedEvent(mode))?
            .err()
            .test_value()?;
        let context = errors
            .diagnostic_context(0)
            .test_expect("unsupported encoding operations should preserve context")?;
        assert_eq!(
            context.failure_point(),
            "serde-data-model-operation-unsupported"
        );
        assert_eq!(Support::code(errors)?, 15);
    }

    Ok(())
}

struct BrokenMap(u8);

impl Serialize for BrokenMap {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        match self.0 {
            0 => map.serialize_value(&1u8)?,
            1 => {
                map.serialize_key("x")?;
                map.serialize_key("y")?;
            }
            _ => map.serialize_key("x")?,
        }
        map.end()
    }
}

#[test]
fn incomplete_map_events_never_return_a_partial_tree() -> TestResult {
    let schema = Support::compile("entry struct Root { value: map<string, u8>, }")?;
    for mode in 0..=2 {
        assert_eq!(
            Support::code(
                Support::encode(&schema, &BrokenMap(mode))?
                    .err()
                    .test_value()?
            )?,
            15
        );
    }

    Ok(())
}

struct PartialMap;

impl<'de> Deserialize<'de> for PartialMap {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct MapVisitor;

        impl<'de> Visitor<'de> for MapVisitor {
            type Value = PartialMap;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("map")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<PartialMap, A::Error> {
                let _: Option<String> = access.next_key()?;
                Ok(PartialMap)
            }
        }
        deserializer.deserialize_map(MapVisitor)
    }
}

#[test]
fn pending_map_values_and_nonstring_key_access_reject() -> TestResult {
    let schema = Support::compile("entry struct Root { value: map<string, u8>, }")?;
    let tree = Support::encode(&schema, &BTreeMap::from([("secret key".to_owned(), 7u8)]))?
        .test_value()?;
    assert_eq!(
        Support::code(
            Support::decode::<PartialMap>(&schema, &tree)?
                .err()
                .test_value()?
        )?,
        15
    );
    assert_eq!(
        Support::code(
            Support::decode::<BTreeMap<u8, u8>>(&schema, &tree)?
                .err()
                .test_value()?
        )?,
        15
    );
    assert_eq!(
        Support::code(
            Support::decode::<BTreeMap<String, NoConsume>>(&schema, &tree)?
                .err()
                .test_value()?
        )?,
        15
    );

    Ok(())
}
