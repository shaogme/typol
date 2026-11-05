use super::support::{FieldsEvent, Model, SerdeTestSupport as Support};
use crate::{
    BindingTestSupport, TestAssertions,
    binding_impl::TypedSchema,
    conversion::{ConversionContext, ConversionValue},
    descriptor::{DescribeContext, DescriptorProvider, TypeDescriptor, Typol},
    diagnostics::{CodecErrors, CodecStage},
    options::{BindOptions, ConversionLimits, WriteOptions},
    serde::Serde,
};
use opeo::{OResult, Out};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{Error as DeError, MapAccess, Visitor},
    ser::{SerializeMap, SerializeStruct},
};
use std::{fmt, marker::PhantomData};
use typol_config::{
    ConfigParseOptions, ConfigParseRequest, ConfigValidationOptions, Environment, EnvironmentError,
    EnvironmentReadContext,
};
use typol_core::{DiagnosticCode, ExecutionLimits, SliceSource, ValueId, ValueKind, ValueSource};
use typol_test_support::{
    TestExpect, TestResult, test_verify as assert, test_verify_eq as assert_eq,
};

struct Named<T> {
    value: T,
}

impl<T: Typol> Typol for Named<T> {
    fn describe<'slot, 'borrow>(
        context: &mut DescribeContext<'_>,
        out: Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, TypeDescriptor> {
        BindingTestSupport::structure::<Self>(
            context,
            "Root",
            &[("value", DescriptorProvider::of::<T>())],
            out,
        )
    }

    fn to_config_value<'slot, 'borrow>(
        &self,
        context: &mut ConversionContext<'_>,
        out: Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        context.reject_with_path(out)
    }

    fn from_config_value<'slot, 'borrow>(
        _value: ConversionValue<'_>,
        context: &mut ConversionContext<'_>,
        out: Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, Self> {
        context.reject_with_path(out)
    }
}

impl<T: Serialize> Serialize for Named<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut fields = serializer.serialize_struct("Root", 1)?;
        fields.serialize_field("value", &self.value)?;
        fields.end()
    }
}

struct RootField;

impl<'de> Deserialize<'de> for RootField {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct FieldVisitor;

        impl<'de> Visitor<'de> for FieldVisitor {
            type Value = RootField;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("value field")
            }

            fn visit_str<E: DeError>(self, value: &str) -> Result<RootField, E> {
                if value == "value" {
                    Ok(RootField)
                } else {
                    Err(E::custom("unknown field"))
                }
            }
        }
        deserializer.deserialize_identifier(FieldVisitor)
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Named<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct NamedVisitor<T>(PhantomData<T>);

        impl<'de, T: Deserialize<'de>> Visitor<'de> for NamedVisitor<T> {
            type Value = Named<T>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("Root object")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
                let _: RootField = access
                    .next_key()?
                    .ok_or_else(|| DeError::custom("secret missing field"))?;
                let value = access.next_value()?;
                if access.next_key::<RootField>()?.is_some() {
                    return Err(DeError::custom("secret extra field"));
                }
                Ok(Named { value })
            }
        }
        deserializer.deserialize_struct("Root", &["value"], NamedVisitor(PhantomData))
    }
}

struct NoEnvironment;

impl Environment for NoEnvironment {
    fn read<'slot, 'borrow, 'env>(
        &'env mut self,
        _: &str,
        _: EnvironmentReadContext<'_>,
        _: Out<'slot, 'borrow, EnvironmentError>,
    ) -> OResult<'slot, Option<&'env [u8]>> {
        Ok(None)
    }
}

#[test]
fn public_serde_pipeline_allocations_fail_atomically() -> TestResult {
    let schema = Support::compile("entry struct Root { value: string @secret @length(1, 10) }")?;
    let binding = Support::codec_value(|out| {
        TypedSchema::<Named<String>>::bind_trusted(&schema, BindOptions::default(), out)
    })?;
    let input = Named {
        value: "secret".into(),
    };
    TestAssertions::assert_atomic_allocations(|| {
        drop(Support::allocation_codec_value(|out| {
            binding.serialize_serde_to_string(&input, WriteOptions::default(), out)
        })?);
        Ok(())
    })?;
    TestAssertions::assert_atomic_allocations(|| {
        drop(Support::allocation_codec_value(|out| {
            binding.deserialize_serde(
                ConfigParseRequest::new(
                    &mut SliceSource::new(("{value: \"secret\"}").as_bytes()),
                    "pipeline",
                    ConfigParseOptions::default(),
                ),
                &mut NoEnvironment,
                ConversionLimits::default(),
                out,
            )
        })?);
        Ok(())
    })?;
    let invalid = Named {
        value: String::new(),
    };
    let errors = Support::codec_error(|out| {
        binding.serialize_serde_to_string(&invalid, WriteOptions::default(), out)
    })?;
    assert_eq!(
        errors
            .diagnostics()
            .first()
            .test_expect("validation failures should include diagnostics")?
            .code()
            .get(),
        12
    );
    TestAssertions::assert_atomic_allocations(|| {
        let errors = Support::codec_error(|out| {
            binding.serialize_serde_to_string(&invalid, WriteOptions::default(), out)
        })?;
        if errors.diagnostics().last().is_some_and(|diagnostic| {
            let code = diagnostic.code().get();
            code == 12 || code == DiagnosticCode::ALLOCATION_FAILED.get()
        }) {
            Ok(())
        } else {
            Err(format!("encoding failed during atomic allocation checking: {errors:?}").into())
        }
    })?;
    let errors = Support::codec_error(|out| {
        binding.deserialize_serde(
            ConfigParseRequest::new(
                &mut SliceSource::new(("{value: \"\"}").as_bytes()),
                "pipeline",
                ConfigParseOptions::default(),
            ),
            &mut NoEnvironment,
            ConversionLimits::default(),
            out,
        )
    })?;
    assert_eq!(
        errors
            .diagnostics()
            .first()
            .test_expect("deserialization validation failures should include diagnostics")?
            .code()
            .get(),
        12
    );
    TestAssertions::assert_atomic_allocations(|| {
        let errors = Support::codec_error(|out| {
            binding.deserialize_serde(
                ConfigParseRequest::new(
                    &mut SliceSource::new(("{value: \"\"}").as_bytes()),
                    "pipeline",
                    ConfigParseOptions::default(),
                ),
                &mut NoEnvironment,
                ConversionLimits::default(),
                out,
            )
        })?;
        if errors.diagnostics().last().is_some_and(|diagnostic| {
            let code = diagnostic.code().get();
            code == 12 || code == DiagnosticCode::ALLOCATION_FAILED.get()
        }) {
            Ok(())
        } else {
            Err(format!("decoding failed during atomic allocation checking: {errors:?}").into())
        }
    })?;
    Ok(())
}

#[test]
fn public_serde_validation_cannot_reset_conversion_balance() -> TestResult {
    let schema = Support::compile("entry struct Root { value: u8 }")?;
    let binding = Support::codec_value(|out| {
        TypedSchema::<Named<u8>>::bind_trusted(&schema, BindOptions::default(), out)
    })?;
    let input = Named { value: 3 };
    let call = Support::session()?;
    drop(Support::codec_value(|out| {
        Serde::encode(&binding, &input, &call, ConversionLimits::default(), out)
    })?);
    let defaults = WriteOptions::default();
    let validation = defaults.validation();
    let options = WriteOptions::new(defaults.max_output_bytes())
        .with_validation(ConfigValidationOptions::new(
            *validation.resources(),
            *validation.config(),
            ExecutionLimits::new(
                call.execution().steps(),
                validation.execution().max_workspace_bytes(),
            ),
        ))
        .with_conversion_limits(defaults.conversion_limits());
    let errors =
        Support::codec_error(|out| binding.serialize_serde_to_string(&input, options, out))?;
    assert_eq!(errors.stage(), CodecStage::Validation);
    assert_eq!(Support::code(errors)?, 1);

    Ok(())
}

#[test]
fn bound_service_preserves_secret_values_and_safe_origin() -> TestResult {
    let schema = Support::compile("entry struct Root { value: string @secret, }")?;
    let binding = Support::codec_value(|out| {
        TypedSchema::<Named<String>>::bind_trusted(&schema, BindOptions::default(), out)
    })?;
    let input = Named {
        value: "secret original value".to_owned(),
    };
    let call = Support::session()?;
    let value = Support::codec_value(|out| {
        Serde::encode(&binding, &input, &call, ConversionLimits::default(), out)
    })?;
    assert_eq!(value.origin(), ValueSource::RustValue);
    assert!(!format!("{value:?}").contains("secret"));
    let decoded = Support::codec_value(|out| {
        Serde::decode(
            &binding,
            value.value(),
            &call,
            ConversionLimits::default(),
            out,
        )
    })?;
    assert_eq!(decoded.value, input.value);

    Ok(())
}

#[test]
fn fields_defaults_none_and_renames_are_exact() -> TestResult {
    let schema = Support::compile("entry struct Root { value: u8 = 9, optional: option<u8>, }")?;
    let empty = FieldsEvent::new("Root", &[]);
    let tree = Model::encode(
        schema.entry(),
        &empty,
        &Support::session()?,
        ConversionLimits::default(),
    )
    .test_value()?;
    let ValueKind::Structure(fields) = tree.as_ref() else {
        return Err("encoded result should be a struct".into());
    };
    let value_field = fields
        .get(0)
        .test_expect("struct should contain the value field")?;
    let optional_field = fields
        .get(1)
        .test_expect("struct should contain the optional field")?;
    assert_eq!(
        value_field
            .name()
            .test_expect("field name should be present")?,
        "value"
    );
    assert!(matches!(
        value_field.value().as_ref(),
        ValueKind::Unsigned(9)
    ));
    assert!(matches!(
        optional_field.value().as_ref(),
        ValueKind::Optional(None)
    ));
    for event in [
        FieldsEvent::new("Alias", &[]),
        FieldsEvent::new("Root", &["value", "value"]),
        FieldsEvent::new("Root", &["secret unknown field"]),
    ] {
        let error = Model::encode(
            schema.entry(),
            &event,
            &Support::session()?,
            ConversionLimits::default(),
        )
        .err()
        .test_value()?;
        assert!(!format!("{error:?}{error}").contains("secret"));
        assert_eq!(Support::code(error)?, 15);
    }
    let schema = Support::compile("entry struct Root { value: u8, }")?;
    assert_eq!(
        Support::code(
            Model::encode(
                schema.entry(),
                &empty,
                &Support::session()?,
                ConversionLimits::default()
            )
            .err()
            .test_value()?
        )?,
        15
    );

    Ok(())
}

struct DuplicateMap;

impl Serialize for DuplicateMap {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(usize::MAX))?;
        map.serialize_entry("secret key", &1u8)?;
        map.serialize_entry("secret key", &2u8)?;
        map.end()
    }
}

#[test]
fn duplicate_map_events_reject_before_overwrite() -> TestResult {
    let schema = Support::compile("entry struct Root { value: map<string, u8> @secret, }")?;
    let error = Support::encode(&schema, &DuplicateMap)?
        .err()
        .test_value()?;
    assert!(!format!("{error:?}{error}").contains("secret"));
    assert_eq!(Support::code(error)?, 15);

    Ok(())
}
