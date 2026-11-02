use super::{Choice, ConversionTestSupport, ENUM_SCHEMA, Pair};
use crate::{
    BindingTestRoot as Root, BindingTestSupport,
    binding::ConversionErrorKind,
    binding_impl::TypedSchema,
    conversion::{ConversionContext, ConversionValue},
    descriptor::{DescribeContext, TypeDescriptor, Typol},
    diagnostics::{CodecErrors, CodecStage},
    options::{BindOptions, CodecOptions, ConversionLimits},
    session::SessionBudget,
    test_support::{TestOpeo, TestValues},
    value::EnumPayload,
};
use opeo::{OResult, Out};
use std::collections::HashMap;
use typol_config::{ConfigParseOptions, ConfigParseRequest, EmptyEnvironment};
use typol_core::{ConfigValue, DiagnosticCode, InstancePath, PathSegment, SliceSource, ValueId};
use typol_model::BuiltinType;
use typol_test_support::{
    TestExpect, TestResult, test_verify as assert, test_verify_eq as assert_eq,
};

struct RejectingScalar;

#[derive(crate::Typol)]
#[typol(crate = "crate", trusted)]
struct SecretChild {
    reading: RejectingScalar,
}

impl Typol for RejectingScalar {
    fn describe<'slot, 'borrow>(
        context: &mut DescribeContext<'_>,
        out: Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, TypeDescriptor> {
        context.scalar::<Self>(BuiltinType::F32, context.depth(), out)
    }

    fn to_config_value<'slot>(
        &self,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        context.start::<Self>(out.reborrow())?;
        context.reject_with_path(out)
    }

    fn from_config_value<'slot>(
        _value: ConversionValue<'_>,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, Self> {
        context.start::<Self>(out.reborrow())?;
        context.reject_with_path(out)
    }
}

fn dynamic(value: ConfigValue) -> TestResult<ConfigValue> {
    ConversionTestSupport::dynamic(value)
}

fn decode_error<T: Typol>(
    binding: &TypedSchema<'_, T>,
    value: &ConfigValue,
    session: &SessionBudget,
    limits: ConversionLimits,
) -> TestResult<CodecErrors> {
    ConversionTestSupport::decode_error(binding, value, session, limits)
}

fn path_of(errors: &CodecErrors) -> TestResult<&InstancePath> {
    errors
        .diagnostics()
        .first()
        .and_then(|diagnostic| diagnostic.path())
        .test_expect("conversion errors should include the instance path")
}

fn assert_secret_path_is_redacted(
    errors: &CodecErrors,
    boundary: &str,
    hidden_segment: &str,
) -> TestResult {
    let diagnostic = errors
        .diagnostics()
        .first()
        .test_expect("conversion errors should have redacted diagnostics")?;
    assert_eq!(errors.stage(), CodecStage::Conversion);
    assert_eq!(diagnostic.message(), "value is hidden");
    assert_eq!(path_of(errors)?.len(), 1);
    assert_eq!(
        path_of(errors)?
            .segments()
            .first()
            .and_then(PathSegment::as_field),
        Some(boundary)
    );
    let surfaces = format!("{errors} {errors:?} {diagnostic:?}");
    assert!(!surfaces.contains(hidden_segment));
    Ok(())
}

#[test]
fn nested_shape_mismatch_reports_field_and_array_index() -> TestResult {
    let schema = BindingTestSupport::compile("entry struct Root { value: [u8] }")?;
    let binding = BindingTestSupport::bind::<Root<Vec<u8>>>(&schema, BindOptions::default())?;
    let session = BindingTestSupport::codec_session()?;
    let input = dynamic(TestValues::array_unmetered(vec![
        TestValues::unsigned(1),
        TestValues::signed(2),
    ]))?;
    let errors = decode_error(&binding, &input, &session, ConversionLimits::default())?;
    let diagnostic = errors
        .diagnostics()
        .first()
        .test_expect("value shape errors should have diagnostics")?;

    assert_eq!(errors.stage(), CodecStage::Conversion);
    assert_eq!(
        errors.conversion_error_kind(),
        Some(ConversionErrorKind::ValueShapeMismatch)
    );
    assert_eq!(diagnostic.code(), DiagnosticCode::RUST_CONVERSION_ERROR);
    assert!(diagnostic.message().contains("expected: u8"));
    assert!(diagnostic.message().contains("actual: signed integer"));
    assert_eq!(path_of(&errors)?.len(), 2);
    assert_eq!(
        path_of(&errors)?
            .segments()
            .first()
            .and_then(PathSegment::as_field),
        Some("value")
    );
    assert_eq!(
        path_of(&errors)?
            .segments()
            .get(1)
            .and_then(PathSegment::as_index),
        Some(1)
    );
    Ok(())
}

#[test]
fn enum_payload_mismatch_reports_the_schema_variant() -> TestResult {
    let schema = BindingTestSupport::compile(ENUM_SCHEMA)?;
    let binding = BindingTestSupport::bind::<Root<Choice>>(&schema, BindOptions::default())?;
    let session = BindingTestSupport::codec_session()?;
    let input = dynamic(TestValues::enumeration_unmetered(
        "Number".into(),
        EnumPayload::unit(),
    ))?;
    let errors = decode_error(&binding, &input, &session, ConversionLimits::default())?;
    let diagnostic = errors
        .diagnostics()
        .first()
        .test_expect("enum payload errors should have diagnostics")?;

    assert_eq!(
        errors.conversion_error_kind(),
        Some(ConversionErrorKind::EnumPayloadMismatch)
    );
    assert!(diagnostic.message().contains("single-value payload"));
    assert!(diagnostic.message().contains("unit payload"));
    assert_eq!(path_of(&errors)?.len(), 2);
    assert_eq!(
        path_of(&errors)?
            .segments()
            .get(1)
            .and_then(PathSegment::as_enum_payload),
        Some("Number")
    );
    Ok(())
}

#[test]
fn missing_field_and_map_value_errors_do_not_echo_input_names_or_keys() -> TestResult {
    let schema = BindingTestSupport::compile(
        "struct Renamed { b: string, a: u8 } entry struct Root { value: Renamed }",
    )?;
    let binding = BindingTestSupport::bind::<Root<Pair>>(&schema, BindOptions::default())?;
    let session = BindingTestSupport::codec_session()?;
    let fields = TestOpeo::value(|out| {
        TestValues::try_structure(
            vec![
                TestValues::field("b".into(), TestValues::string_unmetered("ok".into())),
                TestValues::field("sensitive-input-name".into(), TestValues::unsigned(1)),
            ],
            out,
        )
    })?;
    let input = dynamic(fields)?;
    let errors = decode_error(&binding, &input, &session, ConversionLimits::default())?;
    let diagnostic = errors
        .diagnostics()
        .first()
        .test_expect("missing fields should have diagnostics")?;
    assert_eq!(
        errors.conversion_error_kind(),
        Some(ConversionErrorKind::MissingField)
    );
    assert_eq!(
        path_of(&errors)?
            .segments()
            .last()
            .and_then(PathSegment::as_field),
        Some("a")
    );
    assert!(!diagnostic.message().contains("sensitive-input-name"));

    let map_schema = BindingTestSupport::compile("entry struct Root { value: map<string, u8> }")?;
    let map_binding =
        BindingTestSupport::bind::<Root<HashMap<String, u8>>>(&map_schema, BindOptions::default())?;
    let map_value = TestOpeo::value(|out| {
        TestValues::try_map(
            vec![TestValues::map_entry(
                "api-secret-key".into(),
                TestValues::string_unmetered("raw-secret-value".into()),
            )],
            out,
        )
    })?;
    let map_input = dynamic(map_value)?;
    let map_session = BindingTestSupport::codec_session()?;
    let map_errors = decode_error(
        &map_binding,
        &map_input,
        &map_session,
        ConversionLimits::default(),
    )?;
    let map_diagnostic = map_errors
        .diagnostics()
        .first()
        .test_expect("map value shape errors should have diagnostics")?;
    assert_eq!(
        map_errors.conversion_error_kind(),
        Some(ConversionErrorKind::ValueShapeMismatch)
    );
    assert_eq!(path_of(&map_errors)?.len(), 1);
    assert!(!map_diagnostic.message().contains("api-secret-key"));
    assert!(!map_diagnostic.message().contains("raw-secret-value"));
    assert!(!map_errors.to_string().contains("api-secret-key"));
    assert!(!map_errors.to_string().contains("raw-secret-value"));
    Ok(())
}

#[test]
fn typed_schema_conversion_redacts_secret_paths_in_both_directions() -> TestResult {
    let schema = BindingTestSupport::compile(
        "struct SecretChild { reading: f32 } entry struct Root { value: SecretChild @secret }",
    )?;
    let binding = BindingTestSupport::bind::<Root<SecretChild>>(&schema, BindOptions::default())?;
    let value = Root::new(SecretChild {
        reading: RejectingScalar,
    });
    let errors = BindingTestSupport::codec_error(|out| {
        binding.serialize(&value, CodecOptions::default(), out)
    })?;
    assert_eq!(
        errors.conversion_error_kind(),
        Some(ConversionErrorKind::Rejected)
    );
    assert_secret_path_is_redacted(&errors, "value", "reading")?;

    let mut source = SliceSource::new(b"{value: {reading: 1.0}}");
    let request = ConfigParseRequest::new(&mut source, "secret", ConfigParseOptions::default());
    let errors = BindingTestSupport::codec_error(|out| {
        binding.deserialize(
            request,
            &mut EmptyEnvironment,
            ConversionLimits::default(),
            out,
        )
    })?;
    assert_eq!(
        errors.conversion_error_kind(),
        Some(ConversionErrorKind::Rejected)
    );
    assert_secret_path_is_redacted(&errors, "value", "reading")
}

#[test]
fn secret_array_elements_and_enum_payloads_hide_dynamic_suffixes() -> TestResult {
    let scalar_schema = BindingTestSupport::compile("entry struct Root { value: f32 @secret }")?;
    let scalar_binding =
        BindingTestSupport::bind::<Root<f32>>(&scalar_schema, BindOptions::default())?;
    let errors = BindingTestSupport::codec_error(|out| {
        scalar_binding.serialize(&Root::new(f32::NAN), CodecOptions::default(), out)
    })?;
    assert_eq!(
        errors.conversion_error_kind(),
        Some(ConversionErrorKind::InvalidScalar)
    );
    assert_secret_path_is_redacted(&errors, "value", "non-finite floating-point value")?;

    let array_schema = BindingTestSupport::compile(
        "type SecretFloat = f32 @secret; entry struct Root { value: [SecretFloat] }",
    )?;
    let array_binding =
        BindingTestSupport::bind::<Root<Vec<f32>>>(&array_schema, BindOptions::default())?;
    let errors = BindingTestSupport::codec_error(|out| {
        array_binding.serialize(
            &Root::new(vec![1.0, f32::NAN]),
            CodecOptions::default(),
            out,
        )
    })?;
    assert_eq!(
        errors.conversion_error_kind(),
        Some(ConversionErrorKind::InvalidScalar)
    );
    assert_secret_path_is_redacted(&errors, "value", "[1]")?;

    let enum_schema = BindingTestSupport::compile(
        "enum Choice { Unit, Number(i128 @secret), Object { count: u8, name: string } } entry struct Root { value: Choice }",
    )?;
    let enum_binding =
        BindingTestSupport::bind::<Root<Choice>>(&enum_schema, BindOptions::default())?;
    let input = dynamic(TestValues::enumeration_unmetered(
        "Number".into(),
        EnumPayload::unit(),
    ))?;
    let session = BindingTestSupport::codec_session()?;
    let errors = decode_error(&enum_binding, &input, &session, ConversionLimits::default())?;
    assert_eq!(
        errors.conversion_error_kind(),
        Some(ConversionErrorKind::EnumPayloadMismatch)
    );
    assert_secret_path_is_redacted(&errors, "value", "Number")
}
