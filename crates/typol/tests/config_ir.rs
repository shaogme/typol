#![cfg(feature = "config-ir")]

#[path = "support/values.rs"]
mod value_support;

use value_support::ValueSupport;

use std::{
    error::Error,
    io,
    path::{Path, PathBuf},
    process::abort,
};

use serde_json::{Value, from_slice};
use typol::{
    ConfigInput, ConfigIr, ConfigIrError, ConfigIrErrorKind, ConfigIrOptions, ConfigIrValueMode,
    ConfigValidationOptions, ConfigValue, ConfigValueErrors, EmptyEnvironment, Environment,
    EnvironmentError, EnvironmentReadContext, LoadedSchema, SchemaLoader, TypolOptions,
    config::ConfigLimits,
    core::{ChargedVec, ConfigValueBuilder, ValueError, ValueSource},
    opeo::{OResult, Out},
};
use typol_test_support::TestOpeo;

struct FixtureEnvironment;

struct ScalarExpectation {
    section: &'static str,
    field: &'static str,
    kind: &'static str,
}

fn example(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(path)
}

fn capture<T, E>(
    operation: impl for<'slot> FnOnce(Out<'slot, 'slot, E>) -> OResult<'slot, T>,
) -> Result<T, E> {
    TestOpeo::call(operation)
}

fn build_value<T>(
    operation: impl for<'slot> FnOnce(Out<'slot, 'slot, ValueError>) -> OResult<'slot, T>,
) -> T {
    match TestOpeo::call(operation) {
        Ok(value) => value,
        Err(_) => abort(),
    }
}

fn array_unmetered(values: Vec<ConfigValue>) -> ConfigValue {
    build_value(|mut out| {
        let mut builder = ConfigValueBuilder::new(None, out.reborrow())?;
        let mut ids = ChargedVec::new(None, out.reborrow())?;
        ids.try_reserve_exact(values.len(), out.reborrow())?;
        for value in values {
            ids.try_push(builder.append_value(value, out.reborrow())?, out.reborrow())?;
        }
        let root = builder.add_array(&ids, ValueSource::RustValue, out.reborrow())?;
        builder.finish(root, out)
    })
}

fn float32_array(values: &[f32]) -> ConfigValue {
    array_unmetered(
        values
            .iter()
            .map(|value| {
                ValueSupport::scalar(|builder, out| {
                    builder.add_float32(*value, ValueSource::RustValue, out)
                })
            })
            .collect(),
    )
}

fn float64_array(values: &[f64]) -> ConfigValue {
    array_unmetered(
        values
            .iter()
            .map(|value| {
                ValueSupport::scalar(|builder, out| {
                    builder.add_float64(*value, ValueSource::RustValue, out)
                })
            })
            .collect(),
    )
}

fn finite_float_value(
    f32_values: &[f32],
    f64_values: &[f64],
) -> Result<ConfigValue, Box<dyn Error>> {
    let fields = vec![
        ValueSupport::field("values32".into(), float32_array(f32_values)),
        ValueSupport::field("values64".into(), float64_array(f64_values)),
    ];
    Ok(capture(|out| ValueSupport::structure(fields, None, out))?)
}

fn ensure(condition: bool, message: &str) -> Result<(), Box<dyn Error>> {
    if condition {
        Ok(())
    } else {
        Err(io::Error::other(message).into())
    }
}

fn ensure_equal<T: PartialEq>(actual: T, expected: T, message: &str) -> Result<(), Box<dyn Error>> {
    if actual == expected {
        Ok(())
    } else {
        Err(io::Error::other(message).into())
    }
}

fn load(path: &Path) -> Result<LoadedSchema, Box<dyn Error>> {
    let loader = SchemaLoader::new(TypolOptions::default());
    Ok(capture(|out| loader.compile_file(path, out))?)
}

fn load_config(
    loaded: &LoadedSchema,
    path: &Path,
    environment: &mut dyn Environment,
) -> Result<ConfigValue, Box<dyn Error>> {
    Ok(
        capture(|out| loaded.read_value(ConfigInput::file(path), environment, out))?
            .into_config_value(),
    )
}

fn field_value<'a>(fields: &'a [Value], name: &str) -> Option<&'a Value> {
    fields
        .iter()
        .find(|field| field.get("name").and_then(Value::as_str) == Some(name))?
        .get("value")
}

fn export_fixture(
    schema_name: &str,
    config_name: &str,
    example_directory: &str,
    environment: &mut dyn Environment,
    options: ConfigIrOptions,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let base = example(example_directory);
    let loaded = load(&base.join(schema_name))?;
    let config = load_config(&loaded, &base.join(config_name), environment)?;
    Ok(ConfigIr::serialize(loaded.schema(), config, options)?)
}

impl Environment for FixtureEnvironment {
    fn read<'slot, 'borrow, 'env>(
        &'env mut self,
        name: &str,
        _: EnvironmentReadContext<'_>,
        _out: Out<'slot, 'borrow, EnvironmentError>,
    ) -> OResult<'slot, Option<&'env [u8]>> {
        Ok((name == "TYPOL_NOTIFY_TOKEN").then_some(b"test-notify-token".as_slice()))
    }
}

#[test]
fn v1_fixture_and_pretty_output_are_deterministic() -> Result<(), Box<dyn Error>> {
    let mut environment = EmptyEnvironment;
    let compact = export_fixture(
        "cli_demo.tschem",
        "cli_demo.tconf",
        "cli",
        &mut environment,
        ConfigIrOptions::default(),
    )?;
    let expected: Value = from_slice(include_bytes!("../../../examples/cli/cli_demo-ir-v1.json"))?;
    let actual: Value = from_slice(&compact)?;
    ensure_equal(actual, expected.clone(), "v1 fixture content")?;
    ensure_equal(compact.last(), Some(&b'\n'), "compact trailing newline")?;

    let pretty = export_fixture(
        "cli_demo.tschem",
        "cli_demo.tconf",
        "cli",
        &mut environment,
        ConfigIrOptions::default().with_pretty(true),
    )?;
    ensure_equal(
        from_slice::<Value>(&pretty)?,
        expected,
        "pretty v1 fixture content",
    )?;
    ensure_equal(pretty.last(), Some(&b'\n'), "pretty trailing newline")?;
    ensure(
        compact != pretty,
        "pretty and compact encodings should differ",
    )?;

    let repeated = export_fixture(
        "cli_demo.tschem",
        "cli_demo.tconf",
        "cli",
        &mut environment,
        ConfigIrOptions::default(),
    )?;
    ensure_equal(
        compact,
        repeated,
        "repeated serialization should produce deterministic bytes",
    )?;
    Ok(())
}

#[test]
fn strings_escape_control_characters_and_preserve_utf8_in_both_modes() -> Result<(), Box<dyn Error>>
{
    let mut source = String::from("controls:");
    for control in 0..=0x1f {
        source.push(char::from(control));
    }
    source.push_str("; quote:\" slash:\\; UTF-8: é😀");
    let loader = SchemaLoader::new(TypolOptions::default());
    let loaded = capture(|out| {
        loader.compile_text(
            b"ir-string-escaping",
            "entry struct Root { text: string }",
            out,
        )
    })?;
    for pretty in [false, true] {
        let value = capture(|out| {
            ValueSupport::structure(
                vec![ValueSupport::field(
                    "text".into(),
                    ValueSupport::scalar(|builder, out| {
                        builder.add_string(&source, ValueSource::RustValue, out)
                    }),
                )],
                None,
                out,
            )
        })?;
        let bytes = ConfigIr::serialize(
            loaded.schema(),
            value,
            ConfigIrOptions::default()
                .with_value_mode(ConfigIrValueMode::AsProvided)
                .with_pretty(pretty),
        )?;
        let document: Value = from_slice(&bytes)?;
        let fields = document
            .get("value")
            .and_then(|value| value.get("fields"))
            .and_then(Value::as_array)
            .ok_or_else(|| io::Error::other("serialized fields are missing"))?;
        ensure_equal(
            field_value(fields, "text")
                .and_then(|value| value.get("value"))
                .and_then(Value::as_str),
            Some(source.as_str()),
            "string escaping should preserve control characters and UTF-8",
        )?;
    }
    Ok(())
}

#[test]
fn numeric_fixture_preserves_extremes_unicode_and_qualified_names() -> Result<(), Box<dyn Error>> {
    let mut environment = EmptyEnvironment;
    let actual = export_fixture(
        "cli_numeric.tschem",
        "cli_numeric.tconf",
        "cli",
        &mut environment,
        ConfigIrOptions::default(),
    )?;
    let expected: Value = from_slice(include_bytes!(
        "../../../examples/cli/cli_numeric-ir-v1.json"
    ))?;
    ensure_equal(
        from_slice::<Value>(&actual)?,
        expected,
        "numeric fixture content",
    )?;
    ensure(
        String::from_utf8(actual)?.contains("\"value\":\"-0\""),
        "negative floating-point zero should use string encoding",
    )?;
    Ok(())
}

#[test]
fn finite_float_display_extremes_fit_ir_and_respect_the_output_limit() -> Result<(), Box<dyn Error>>
{
    let f32_values = [
        f32::MAX,
        f32::MIN,
        f32::from_bits(1),
        -f32::from_bits(1),
        -0.0f32,
        1.25,
    ];
    let f64_values = [
        f64::MAX,
        f64::MIN,
        f64::from_bits(1),
        -f64::from_bits(1),
        -0.0f64,
        1.25,
    ];
    let loader = SchemaLoader::new(TypolOptions::default());
    let loaded = capture(|out| {
        loader.compile_text(
            b"float-display-boundary-schema",
            "entry struct FloatRoot { values32: [f32], values64: [f64], }",
            out,
        )
    })?;
    let bytes = ConfigIr::serialize(
        loaded.schema(),
        finite_float_value(&f32_values, &f64_values)?,
        ConfigIrOptions::default(),
    )?;
    let document: Value = from_slice(&bytes)?;
    let fields = document
        .get("value")
        .and_then(|value| value.get("fields"))
        .and_then(Value::as_array)
        .ok_or("floating-point root field is missing from the IR")?;
    let values32 = field_value(fields, "values32")
        .and_then(|value| value.get("items"))
        .and_then(Value::as_array)
        .ok_or("f32 array is missing from the IR")?;
    let values64 = field_value(fields, "values64")
        .and_then(|value| value.get("items"))
        .and_then(Value::as_array)
        .ok_or("f64 array is missing from the IR")?;
    ensure_equal(values32.len(), f32_values.len(), "f32 array length")?;
    ensure_equal(values64.len(), f64_values.len(), "f64 array length")?;
    for (actual, expected) in values32.iter().zip(&f32_values) {
        let expected = if *expected == 0.0 && expected.is_sign_negative() {
            "-0".to_owned()
        } else {
            expected.to_string()
        };
        ensure_equal(
            actual.get("value").and_then(Value::as_str),
            Some(expected.as_str()),
            "f32 IR string should match its Display form",
        )?;
    }
    for (actual, expected) in values64.iter().zip(&f64_values) {
        let expected = if *expected == 0.0 && expected.is_sign_negative() {
            "-0".to_owned()
        } else {
            expected.to_string()
        };
        ensure_equal(
            actual.get("value").and_then(Value::as_str),
            Some(expected.as_str()),
            "f64 IR string should match its Display form",
        )?;
    }

    let at_limit = ConfigIr::serialize(
        loaded.schema(),
        finite_float_value(&f32_values, &f64_values)?,
        ConfigIrOptions::default().with_max_output_bytes(bytes.len()),
    )?;
    ensure_equal(
        at_limit.as_slice(),
        bytes.as_slice(),
        "serialization should succeed when the output limit equals the actual JSON length",
    )?;
    ensure(
        matches!(
            ConfigIr::serialize(
            loaded.schema(),
            finite_float_value(&f32_values, &f64_values)?,
            ConfigIrOptions::default().with_max_output_bytes(bytes.len() - 1),
            ),
            Err(error) if error.kind() == ConfigIrErrorKind::OutputLimitExceeded
        ),
        "an insufficient output limit should return a limit error",
    )?;
    Ok(())
}

#[test]
fn all_builtin_types_use_the_v1_scalar_encoding() -> Result<(), Box<dyn Error>> {
    let mut environment = FixtureEnvironment;
    let bytes = export_fixture(
        "all_types.tschem",
        "all_types.tconf",
        "config",
        &mut environment,
        ConfigIrOptions::default(),
    )?;
    let document: Value = from_slice(&bytes)?;
    let fields = document
        .get("value")
        .and_then(|value| value.get("fields"))
        .and_then(Value::as_array)
        .ok_or("IR root fields are missing")?;
    for expected in [
        ScalarExpectation {
            section: "integers",
            field: "val_i8",
            kind: "i8",
        },
        ScalarExpectation {
            section: "integers",
            field: "val_i16",
            kind: "i16",
        },
        ScalarExpectation {
            section: "integers",
            field: "val_i32",
            kind: "i32",
        },
        ScalarExpectation {
            section: "integers",
            field: "val_i64",
            kind: "i64",
        },
        ScalarExpectation {
            section: "integers",
            field: "val_i128",
            kind: "i128",
        },
        ScalarExpectation {
            section: "integers",
            field: "val_u8",
            kind: "u8",
        },
        ScalarExpectation {
            section: "integers",
            field: "val_u16",
            kind: "u16",
        },
        ScalarExpectation {
            section: "integers",
            field: "val_u32",
            kind: "u32",
        },
        ScalarExpectation {
            section: "integers",
            field: "val_u64",
            kind: "u64",
        },
        ScalarExpectation {
            section: "integers",
            field: "val_u128",
            kind: "u128",
        },
        ScalarExpectation {
            section: "floats",
            field: "val_f32",
            kind: "f32",
        },
        ScalarExpectation {
            section: "floats",
            field: "val_f64",
            kind: "f64",
        },
    ] {
        let section = field_value(fields, expected.section).ok_or("IR section is missing")?;
        let section_fields = section
            .get("fields")
            .and_then(Value::as_array)
            .ok_or("IR nested fields are missing")?;
        let value = field_value(section_fields, expected.field).ok_or("IR scalar is missing")?;
        ensure_equal(
            value
                .get("type")
                .and_then(|ty| ty.get("kind"))
                .and_then(Value::as_str),
            Some(expected.kind),
            "scalar type label",
        )?;
        if expected.kind.starts_with('i') || expected.kind.starts_with('u') {
            ensure(
                value.get("value").and_then(Value::as_str).is_some(),
                "integer payload must be a decimal string",
            )?;
        }
    }
    Ok(())
}

#[test]
fn empty_map_keys_survive_default_normalization_and_ir_serialization() -> Result<(), Box<dyn Error>>
{
    let loader = SchemaLoader::new(TypolOptions::default());
    let loaded = capture(|out| {
        loader.compile_text(
            b"empty-map-key-schema",
            "entry struct Root { values: map<string, u8> = {\"\": 7} }",
            out,
        )
    })?;
    let value = capture::<_, ValueError>(|out| ValueSupport::structure(Vec::new(), None, out))?;
    let bytes = ConfigIr::serialize(
        loaded.schema(),
        value,
        ConfigIrOptions::default().with_value_mode(ConfigIrValueMode::WithDefaults),
    )?;
    ensure(
        String::from_utf8(bytes)?.contains("\"key\":\"\""),
        "empty-string map keys should remain in normalized Config IR",
    )?;
    Ok(())
}

#[test]
fn byte_and_workspace_limits_fail_without_returning_partial_json() -> Result<(), Box<dyn Error>> {
    ensure_equal(
        ConfigIrOptions::default()
            .try_with_max_nesting_depth(128)?
            .max_nesting_depth(),
        128,
        "a nesting limit equal to the fixed safety limit should be accepted",
    )?;
    ensure(
        matches!(
            ConfigIrOptions::default().try_with_max_nesting_depth(129),
            Err(ConfigIrError::InvalidNestingDepthLimit {
                requested: 129,
                maximum: 128,
            })
        ),
        "an over-limit request should be preserved and returned as a configuration error",
    )?;
    ensure_equal(
        ConfigIrError::InvalidNestingDepthLimit {
            requested: 129,
            maximum: 128,
        }
        .to_string(),
        "requested IR nesting depth limit 129 exceeds the fixed safety limit 128".to_owned(),
        "depth configuration errors should report the requested and fixed limits",
    )?;
    let mut environment = EmptyEnvironment;
    let baseline = export_fixture(
        "cli_demo.tschem",
        "cli_demo.tconf",
        "cli",
        &mut environment,
        ConfigIrOptions::default(),
    )?;
    let loaded = load(&example("cli/cli_demo.tschem"))?;
    let config = load_config(&loaded, &example("cli/cli_demo.tconf"), &mut environment)?;
    let at_limit = ConfigIr::serialize(
        loaded.schema(),
        config,
        ConfigIrOptions::default().with_max_output_bytes(baseline.len()),
    )?;
    ensure_equal(
        at_limit.as_slice(),
        baseline.as_slice(),
        "serialization should succeed when the limit equals the output length",
    )?;
    ensure(
        matches!(
            ConfigIr::serialize(
            loaded.schema(),
            load_config(&loaded, &example("cli/cli_demo.tconf"), &mut environment)?,
            ConfigIrOptions::default().with_max_output_bytes(baseline.len() - 1),
            ),
            Err(error) if error.kind() == ConfigIrErrorKind::OutputLimitExceeded
        ),
        "output overflow should return a limit error",
    )?;
    ensure(
        matches!(
            ConfigIr::serialize(
            loaded.schema(),
            load_config(&loaded, &example("cli/cli_demo.tconf"), &mut environment)?,
            ConfigIrOptions::default().with_max_workspace_bytes(0),
            ),
            Err(error) if error.kind() == ConfigIrErrorKind::WorkspaceLimitExceeded
        ),
        "workspace overflow should return a limit error",
    )?;
    ensure(
        matches!(
            ConfigIr::serialize(
            loaded.schema(),
            load_config(&loaded, &example("cli/cli_demo.tconf"), &mut environment)?,
            ConfigIrOptions::default().try_with_max_nesting_depth(1)?,
            ),
            Err(error) if error.kind() == ConfigIrErrorKind::NestingLimitExceeded
        ),
        "nesting depth overflow should return a limit error",
    )?;
    Ok(())
}

#[test]
fn value_shape_and_non_finite_float_errors_are_classified_safely() -> Result<(), Box<dyn Error>> {
    let schema_text = "entry struct FloatRoot { value: f64, }";
    let loader = SchemaLoader::new(TypolOptions::default());
    let loaded = capture(|out| loader.compile_text(b"float-schema", schema_text, out))?;
    ensure(
        matches!(
            ConfigIr::serialize(
            loaded.schema(),
            ValueSupport::scalar(|builder, out| {
                builder.add_string("must-not-appear", ValueSource::RustValue, out)
            }),
            ConfigIrOptions::default().with_value_mode(ConfigIrValueMode::AsProvided),
            ),
            Err(error) if error.kind() == ConfigIrErrorKind::ValueShapeMismatch
        ),
        "a value-kind mismatch should return a shape error",
    )?;

    let fields = vec![ValueSupport::field(
        "value".into(),
        ValueSupport::scalar(|builder, out| {
            builder.add_float64(f64::INFINITY, ValueSource::RustValue, out)
        }),
    )];
    let value = capture::<_, ValueError>(|out| ValueSupport::structure(fields, None, out))?;
    let error = match ConfigIr::serialize(
        loaded.schema(),
        value,
        ConfigIrOptions::default().with_value_mode(ConfigIrValueMode::AsProvided),
    ) {
        Ok(_) => return Err(io::Error::other("a non-finite float was not rejected").into()),
        Err(error) => error,
    };
    ensure(
        error.kind() == ConfigIrErrorKind::UnsupportedFloat,
        "non-finite float error category",
    )?;
    ensure(
        !format!("{error:?} {error}").contains("must-not-appear"),
        "error formatting must not include configuration values",
    )?;

    let shape_schema_text = "entry struct ShapeRoot { scalar: i8, values: [u8; 1], }";
    let shape_loader = SchemaLoader::new(TypolOptions::default());
    let shape_schema =
        capture(|out| shape_loader.compile_text(b"shape-schema", shape_schema_text, out))?;
    let out_of_range = capture::<_, ValueError>(|out| {
        ValueSupport::structure(
            vec![
                ValueSupport::field(
                    "scalar".into(),
                    ValueSupport::scalar(|builder, out| {
                        builder.add_signed(128, ValueSource::RustValue, out)
                    }),
                ),
                ValueSupport::field(
                    "values".into(),
                    array_unmetered(vec![ValueSupport::scalar(|builder, out| {
                        builder.add_unsigned(1, ValueSource::RustValue, out)
                    })]),
                ),
            ],
            None,
            out,
        )
    })?;
    ensure(
        matches!(
            ConfigIr::serialize(
            shape_schema.schema(),
            out_of_range,
            ConfigIrOptions::default().with_value_mode(ConfigIrValueMode::AsProvided),
            ),
            Err(error) if error.kind() == ConfigIrErrorKind::ValueShapeMismatch
        ),
        "an integer outside its bit width should return a shape error",
    )?;

    let wrong_array_length = capture::<_, ValueError>(|out| {
        ValueSupport::structure(
            vec![
                ValueSupport::field(
                    "scalar".into(),
                    ValueSupport::scalar(|builder, out| {
                        builder.add_signed(1, ValueSource::RustValue, out)
                    }),
                ),
                ValueSupport::field(
                    "values".into(),
                    array_unmetered(vec![
                        ValueSupport::scalar(|builder, out| {
                            builder.add_unsigned(1, ValueSource::RustValue, out)
                        }),
                        ValueSupport::scalar(|builder, out| {
                            builder.add_unsigned(2, ValueSource::RustValue, out)
                        }),
                    ]),
                ),
            ],
            None,
            out,
        )
    })?;
    ensure(
        matches!(
            ConfigIr::serialize(
            shape_schema.schema(),
            wrong_array_length,
            ConfigIrOptions::default().with_value_mode(ConfigIrValueMode::AsProvided),
            ),
            Err(error) if error.kind() == ConfigIrErrorKind::ValueShapeMismatch
        ),
        "a fixed-array length mismatch should return a shape error",
    )?;
    Ok(())
}

fn processing_schema() -> Result<LoadedSchema, Box<dyn Error>> {
    let schema_text = "type Small = u8 @range(1, 4);\n\
        struct Nested { depth: u8 = 7 }\n\
        enum Choice { Fields { count: u8 = 9 }, Unit }\n\
        entry struct Root {\n\
            amount: Small = 2,\n\
            title: string @secret @length(2, 6) = \"base\",\n\
            tags: [u8] @unique = [1, 2],\n\
            nested: Nested = {},\n\
            choice: Choice,\n\
            note: option<string> = \"fallback\",\n\
        } @assert(self.amount < 4, \"below max\")";
    let loader = SchemaLoader::new(TypolOptions::default());
    Ok(capture(|out| {
        loader.compile_text(b"ir-processing", schema_text, out)
    })?)
}

fn processing_value(
    amount: Option<u128>,
    title: &str,
    tags: Vec<u8>,
) -> Result<ConfigValue, Box<dyn Error>> {
    let mut fields = Vec::new();
    if let Some(amount) = amount {
        fields.push(ValueSupport::field(
            "amount".into(),
            ValueSupport::scalar(|builder, out| {
                builder.add_unsigned(amount, ValueSource::RustValue, out)
            }),
        ));
    }
    fields.push(ValueSupport::field(
        "title".into(),
        ValueSupport::scalar(|builder, out| builder.add_string(title, ValueSource::RustValue, out)),
    ));
    fields.push(ValueSupport::field(
        "tags".into(),
        array_unmetered(
            tags.into_iter()
                .map(|value| {
                    ValueSupport::scalar(|builder, out| {
                        builder.add_unsigned(u128::from(value), ValueSource::RustValue, out)
                    })
                })
                .collect(),
        ),
    ));
    let payload = Vec::new();
    fields.push(ValueSupport::field(
        "choice".into(),
        capture(|out| ValueSupport::structure(payload, Some("Fields"), out))?,
    ));
    fields.push(ValueSupport::field(
        "note".into(),
        ValueSupport::scalar(|builder, out| {
            builder.add_optional(None, ValueSource::RustValue, out)
        }),
    ));
    Ok(capture(|out| ValueSupport::structure(fields, None, out))?)
}

#[test]
fn value_modes_fill_defaults_preserve_explicit_none_and_validate_constraints()
-> Result<(), Box<dyn Error>> {
    ensure_equal(
        ConfigIrOptions::default().value_mode(),
        ConfigIrValueMode::Validated,
        "default mode should perform full schema validation",
    )?;
    let loaded = processing_schema()?;
    let schema = loaded.schema();
    let empty = capture::<_, ValueError>(|out| ValueSupport::structure(Vec::new(), None, out))?;
    let normalization_error = match ConfigIr::serialize(
        schema,
        empty,
        ConfigIrOptions::default().with_value_mode(ConfigIrValueMode::WithDefaults),
    ) {
        Ok(_) => {
            return Err(io::Error::other(
                "configuration missing a required field did not trigger a normalization error",
            )
            .into());
        }
        Err(error) => error,
    };
    ensure(
        matches!(&normalization_error, ConfigIrError::NormalizationFailed(_)),
        "a missing required field should return a normalization error",
    )?;
    ensure(
        normalization_error.kind() == ConfigIrErrorKind::NormalizationFailed,
        "normalization errors should report the corresponding category",
    )?;
    let source = Error::source(&normalization_error)
        .and_then(|source| source.downcast_ref::<ConfigValueErrors>())
        .ok_or("normalization errors did not expose ConfigValueErrors")?;
    ensure(
        !source.diagnostics().is_empty(),
        "normalization errors should include diagnostics",
    )?;
    ensure(
        source
            .diagnostics()
            .iter()
            .flat_map(|diagnostic| diagnostic.labels())
            .any(|label| source.span_location(label.span()).is_some()),
        "normalization errors should preserve schema locations",
    )?;

    let mut limited_config = ConfigLimits::default();
    limited_config = limited_config.with_max_value_nodes(0);
    let limited_options = ConfigValidationOptions::default().with_config(limited_config);
    let budget_error = ConfigIr::serialize(
        schema,
        processing_value(None, "safe", vec![1, 2])?,
        ConfigIrOptions::default()
            .with_value_mode(ConfigIrValueMode::WithDefaults)
            .with_validation_options(limited_options),
    );
    ensure(
        matches!(budget_error, Err(ConfigIrError::NormalizationFailed(_))),
        "configuration processing should respect the value-node limit",
    )?;

    let invalid = processing_value(Some(0), "PRIVATE-IR-SECRET", vec![1, 1])?;
    ensure(
        matches!(
            ConfigIr::serialize(
                schema,
                invalid,
                ConfigIrOptions::default().with_value_mode(ConfigIrValueMode::AsProvided),
            ),
            Err(error) if error.kind() == ConfigIrErrorKind::ValueShapeMismatch
        ),
        "AsProvided should preserve full structure requirements",
    )?;

    let normalized = ConfigIr::serialize(
        schema,
        processing_value(Some(0), "PRIVATE-IR-SECRET", vec![1, 1])?,
        ConfigIrOptions::default().with_value_mode(ConfigIrValueMode::WithDefaults),
    )?;
    let document: Value = from_slice(&normalized)?;
    let fields = document
        .get("value")
        .and_then(|value| value.get("fields"))
        .and_then(Value::as_array)
        .ok_or("normalized root field is missing")?;
    ensure_equal(
        field_value(fields, "amount").and_then(|value| value.get("value")),
        Some(&Value::String("0".into())),
        "WithDefaults should preserve explicit values without checking constraints",
    )?;
    ensure_equal(
        field_value(fields, "note").and_then(|value| value.get("value")),
        Some(&Value::Null),
        "explicit none should override an optional field default",
    )?;
    let nested = field_value(fields, "nested")
        .and_then(|value| value.get("fields"))
        .and_then(Value::as_array)
        .ok_or("nested struct default is missing")?;
    ensure_equal(
        field_value(nested, "depth").and_then(|value| value.get("value")),
        Some(&Value::String("7".into())),
        "nested struct field default",
    )?;
    let choice = field_value(fields, "choice")
        .and_then(|value| value.get("payload"))
        .and_then(|value| value.get("fields"))
        .and_then(Value::as_array)
        .ok_or("active enum struct payload default is missing")?;
    ensure_equal(
        field_value(choice, "count").and_then(|value| value.get("value")),
        Some(&Value::String("9".into())),
        "active enum anonymous struct payload default",
    )?;
    ensure_equal(
        field_value(fields, "title")
            .and_then(|value| value.get("secret"))
            .and_then(Value::as_bool),
        Some(true),
        "sensitive fields should preserve the IR marker",
    )?;
    ensure_equal(
        field_value(fields, "title")
            .and_then(|value| value.get("value"))
            .and_then(Value::as_str),
        Some("PRIVATE-IR-SECRET"),
        "the IR sensitive marker does not hide the payload",
    )?;

    let validation_error = match ConfigIr::serialize(
        schema,
        processing_value(Some(0), "PRIVATE-IR-SECRET", vec![1, 1])?,
        ConfigIrOptions::default().with_validation_options(ConfigValidationOptions::default()),
    ) {
        Ok(_) => {
            return Err(io::Error::other(
                "default mode allowed configuration that violates schema constraints",
            )
            .into());
        }
        Err(error) => error,
    };
    ensure(
        matches!(&validation_error, ConfigIrError::ValidationFailed(_)),
        "invalid configuration should return a validation error",
    )?;
    ensure(
        validation_error.kind() == ConfigIrErrorKind::ValidationFailed,
        "validation errors should report the corresponding category",
    )?;
    let source = Error::source(&validation_error)
        .and_then(|source| source.downcast_ref::<ConfigValueErrors>())
        .ok_or("validation errors did not expose ConfigValueErrors")?;
    ensure(
        !source.diagnostics().is_empty(),
        "validation errors should include diagnostics",
    )?;
    ensure(
        !format!("{validation_error:?} {validation_error}").contains("PRIVATE-IR-SECRET"),
        "schema error text must not leak sensitive values",
    )?;

    let valid = ConfigIr::serialize(
        schema,
        processing_value(None, "safe", vec![1, 2])?,
        ConfigIrOptions::default().with_value_mode(ConfigIrValueMode::Validated),
    )?;
    let document: Value = from_slice(&valid)?;
    let fields = document
        .get("value")
        .and_then(|value| value.get("fields"))
        .and_then(Value::as_array)
        .ok_or("root field is missing after full validation")?;
    ensure_equal(
        field_value(fields, "amount").and_then(|value| value.get("value")),
        Some(&Value::String("2".into())),
        "Validated should fill in root field defaults first",
    )?;
    Ok(())
}
