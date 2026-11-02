#[path = "support/values.rs"]
mod value_support;

use value_support::ValueSupport;

use typol::{
    ConfigParseOptions, Environment, EnvironmentError, EnvironmentReadContext, Typol,
    binding::{BindOptions, CodecOptions, ConversionLimits, TypedSchema},
    config::{ConfigParseRequest, ConfigParser},
    core::{SliceSource, ValueKind, ValueSource},
    host::{DisplayOptions, SafeValueFormatter},
    opeo::{OResult, Out},
    schema::{
        ResolverError, SchemaCompileOptions, SchemaCompiler, SchemaResolveRequest, SchemaResolver,
        SchemaSource,
    },
};
use typol_test_support::{
    TestOpeo, TestResult, test_verify as assert, test_verify_eq as assert_eq,
};

struct NoImports;

impl SchemaResolver for NoImports {
    fn resolve<'slot, 'borrow, 'src>(
        &'src mut self,
        _: SchemaResolveRequest<'_>,
        out: Out<'slot, 'borrow, ResolverError>,
    ) -> OResult<'slot, SchemaSource<'src>> {
        Err(out.fail(ResolverError::NotFound))
    }
}

struct Env;

impl Environment for Env {
    fn read<'slot, 'borrow, 'env>(
        &'env mut self,
        name: &str,
        _: EnvironmentReadContext<'_>,
        _: Out<'slot, 'borrow, EnvironmentError>,
    ) -> OResult<'slot, Option<&'env [u8]>> {
        Ok((name == "X").then_some(b"PRIVATE-ENV".as_slice()))
    }
}

struct ForbiddenEnv;

impl Environment for ForbiddenEnv {
    fn read<'slot, 'borrow, 'env>(
        &'env mut self,
        _: &str,
        _: EnvironmentReadContext<'_>,
        out: Out<'slot, 'borrow, EnvironmentError>,
    ) -> OResult<'slot, Option<&'env [u8]>> {
        Err(out.fail(EnvironmentError::HostFailure))
    }
}

#[derive(Debug, PartialEq, Typol)]
struct Root {
    literal: String,
    environment: String,
    fallback: String,
    defaulted: String,
}

fn request<'a>(input: &'a mut SliceSource<'_>) -> ConfigParseRequest<'a> {
    ConfigParseRequest::new(input, "config", ConfigParseOptions::default())
}

#[test]
fn real_codec_preserves_secrets_while_schema_display_hides_the_same_inputs() -> TestResult {
    let compiler =
        TestOpeo::value(|out| SchemaCompiler::new(SchemaCompileOptions::default(), out))?;
    let schema = TestOpeo::value(|out| {
        compiler.compile(
            SchemaSource::from_str(b"root", r#"entry struct Root { literal: string @secret, environment: string @secret @env("X"), fallback: string @secret @env("Y"), defaulted: string @secret = "PRIVATE-DEFAULT" }"#),
            &mut NoImports,
            None,
            out,
        )
    })?;
    let text = r#"{literal: "PRIVATE-LITERAL", environment: env("X"), fallback: env("Y", "PRIVATE-FALLBACK")}"#;
    let binding =
        TestOpeo::value(|out| TypedSchema::<Root>::bind(&schema, BindOptions::default(), out))?;
    let model = TestOpeo::value(|out| {
        binding.deserialize(
            request(&mut SliceSource::new((text).as_bytes())),
            &mut Env,
            ConversionLimits::default(),
            out,
        )
    })?;
    let encoded = TestOpeo::value(|out| binding.serialize(&model, CodecOptions::default(), out))?;
    for sentinel in [
        "PRIVATE-LITERAL",
        "PRIVATE-ENV",
        "PRIVATE-FALLBACK",
        "PRIVATE-DEFAULT",
    ] {
        assert!(encoded.contains(sentinel));
    }
    assert!(!encoded.contains("env("));
    let environment_failure = TestOpeo::error(|out| {
        binding.deserialize(
            request(&mut SliceSource::new(text.as_bytes())),
            &mut ForbiddenEnv,
            ConversionLimits::default(),
            out,
        )
    })?;
    assert!(!format!("{environment_failure:?}").contains("PRIVATE-ENV"));
    assert_eq!(
        TestOpeo::value(|out| binding.deserialize(
            request(&mut SliceSource::new(encoded.as_bytes())),
            &mut ForbiddenEnv,
            ConversionLimits::default(),
            out,
        ))?,
        model
    );
    let parsed = TestOpeo::value(|out| {
        ConfigParser::new(&schema).parse(
            request(&mut SliceSource::new((text).as_bytes())),
            &mut Env,
            out,
        )
    })?;
    let parsed = parsed.into_config_value();
    let rust_value = TestOpeo::value(|out| {
        ValueSupport::structure(
            vec![
                ValueSupport::field(
                    "literal".into(),
                    ValueSupport::scalar(|builder, out| {
                        builder.add_string(&model.literal, ValueSource::RustValue, out)
                    }),
                ),
                ValueSupport::field(
                    "environment".into(),
                    ValueSupport::scalar(|builder, out| {
                        builder.add_string(&model.environment, ValueSource::RustValue, out)
                    }),
                ),
                ValueSupport::field(
                    "fallback".into(),
                    ValueSupport::scalar(|builder, out| {
                        builder.add_string(&model.fallback, ValueSource::RustValue, out)
                    }),
                ),
                ValueSupport::field(
                    "defaulted".into(),
                    ValueSupport::scalar(|builder, out| {
                        builder.add_string(&model.defaulted, ValueSource::RustValue, out)
                    }),
                ),
            ],
            None,
            out,
        )
    })?;
    let expected =
        "{literal: ********, environment: ********, fallback: ********, defaulted: ********}";
    for value in [&parsed, &rust_value] {
        assert_eq!(
            TestOpeo::value(|out| SafeValueFormatter::format(
                &schema,
                value,
                DisplayOptions::default(),
                out
            ))?,
            expected
        );
        let ValueKind::Structure(fields) = value.as_ref() else {
            return Err("formatted input should be a struct".into());
        };
        assert!(fields.into_iter().all(|field| matches!(field.value().as_ref(), ValueKind::String(s) if s.starts_with("PRIVATE"))));
    }
    Ok(())
}
