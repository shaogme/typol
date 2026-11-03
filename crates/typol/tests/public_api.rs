#![recursion_limit = "512"]

#[path = "support/values.rs"]
mod value_support;

use value_support::ValueSupport;

use std::{collections::HashMap, fmt::Debug, io::Cursor, marker::PhantomData, ptr::eq};
use typol::{
    ConfigInput, ConfigParseOptions, ConfigParser, ConfigValue, ConfigValueProcessor,
    EmptyEnvironment, Environment, EnvironmentError, EnvironmentReadContext, SchemaLoader, Typol,
    TypolOptions, ValidatedConfigValue, WriteOptions,
    binding::{
        BindOptions, BindingLimitError, BindingLimits, BindingMismatchKind, BindingShape,
        BudgetedTypol, CodecOptions, CodecStage, ConversionLimits, MAX_CONVERSION_DEPTH,
        TypedSchema,
    },
    config::{ConfigLimits, ConfigParseRequest, ConfigValidationOptions, ConfigValidationRequest},
    core::{BudgetError, DiagnosticCode, ExecutionBudget, ExecutionLimits, ValueKind, ValueSource},
    opeo::{OResult, Out},
    schema::{SchemaCompileOptions, SchemaCompiler, SchemaSource},
    semantic::CompiledSchema,
};
use typol_core::SliceSource;
use typol_test_support::{
    TestExpect, TestExtract, TestFailure, TestOpeo, TestOpeoOutcome, TestResult,
    test_verify as assert, test_verify_eq as assert_eq,
};

struct NoEnvironment;

impl Environment for NoEnvironment {
    fn read<'slot, 'borrow, 'env>(
        &'env mut self,
        _: &str,
        _: EnvironmentReadContext<'_>,
        out: Out<'slot, 'borrow, EnvironmentError>,
    ) -> OResult<'slot, Option<&'env [u8]>> {
        Err(out.fail(EnvironmentError::HostFailure))
    }
}

fn compile(text: &str) -> TestResult<CompiledSchema> {
    let compiler =
        TestOpeo::value(|out| SchemaCompiler::new(SchemaCompileOptions::default(), out))?;
    TestOpeo::value(|out| {
        compiler.compile_without_imports(SchemaSource::from_str(b"public", text), out)
    })
}

fn request<'a>(input: &'a mut SliceSource<'_>) -> ConfigParseRequest<'a> {
    ConfigParseRequest::new(input, "config", ConfigParseOptions::default())
}

#[derive(Debug, PartialEq, Typol)]
enum Choice {
    Unit,
    Single(u8),
    Object {
        #[typol(rename = "n")]
        count: u8,
        label: String,
    },
    Empty {},
}

#[derive(Debug, PartialEq, Typol)]
#[typol(rename = "Root")]
struct Application<T, const N: usize>
where
    T: PartialEq,
{
    values: [T; N],
    choice: Choice,
    #[typol(rename = "name")]
    r#type: String,
    optional: Option<u8>,
    dictionary: HashMap<String, u8>,
}

#[test]
fn derive_generics_payloads_rename_and_reordered_text_roundtrip() -> TestResult {
    let schema = compile(
        "enum Choice { Unit, Single(u8), Object { label: string, n: u8 }, Empty {} }
         entry struct Root { name: string @secret, choice: Choice, optional: option<u8>, dictionary: map<string, u8>, values: [u8; 2] }",
    )?;
    let binding = TestOpeo::value(|out| {
        TypedSchema::<Application<u8, 2>>::bind(&schema, BindOptions::default(), out)
    })?;
    assert!(eq(binding.schema(), &schema));
    for choice in [
        Choice::Unit,
        Choice::Single(7),
        Choice::Object {
            count: 8,
            label: "label".into(),
        },
        Choice::Empty {},
    ] {
        let value = Application {
            values: [0, 255],
            choice,
            r#type: "secret".into(),
            optional: None,
            dictionary: HashMap::from([("z".into(), 2), ("a".into(), 1)]),
        };
        let text = TestOpeo::value(|out| binding.serialize(&value, CodecOptions::default(), out))?;
        assert!(text.starts_with("{name: \"secret\", choice:"));
        assert!(text.contains("optional: none"));
        assert!(text.contains("dictionary: {\"a\": 1, \"z\": 2}"));
        assert_eq!(
            TestOpeo::value(|out| binding.deserialize(
                request(&mut SliceSource::new(text.as_bytes())),
                &mut NoEnvironment,
                ConversionLimits::default(),
                out,
            ))?,
            value
        );
    }
    let value = TestOpeo::value(|out| {
        binding.deserialize(
            request(&mut SliceSource::new(("{values: [1, 2], dictionary: {}, choice: .Object {n: 3, label: \"alpha\"}, name: \"beta\"}").as_bytes())),
            &mut NoEnvironment,
            ConversionLimits::default(),
            out,
        )
    })?;
    assert_eq!(
        value.choice,
        Choice::Object {
            count: 3,
            label: "alpha".into()
        }
    );
    assert_eq!(value.optional, None);
    Ok(())
}

#[derive(Debug, PartialEq, Typol)]
struct Root {
    value: u8,
}

#[test]
fn pretty_codec_options_are_opt_in_for_typed_serialization() -> TestResult {
    assert!(!CodecOptions::default().pretty());
    assert!(!WriteOptions::default().pretty());
    let schema = compile("entry struct Root { value: u8 }")?;
    let binding =
        TestOpeo::value(|out| TypedSchema::<Root>::bind(&schema, BindOptions::default(), out))?;
    assert_eq!(
        TestOpeo::value(|out| binding.serialize(&Root { value: 7 }, CodecOptions::default(), out))?,
        "{value: 7}"
    );
    assert_eq!(
        TestOpeo::value(|out| {
            binding.serialize(
                &Root { value: 7 },
                CodecOptions::default().with_pretty(true),
                out,
            )
        })?,
        "{\n  value: 7,\n}"
    );
    Ok(())
}

#[test]
fn public_binding_errors_expose_reason_path_and_shapes() -> TestResult {
    let schema = compile("entry struct Root { value: i8 }")?;
    let errors =
        TestOpeo::error(|out| TypedSchema::<Root>::bind(&schema, BindOptions::default(), out))?;
    let mismatch = errors
        .binding_errors()
        .and_then(|errors| errors.first())
        .test_value()?;
    assert_eq!(mismatch.kind(), BindingMismatchKind::FieldShape);
    assert_eq!(mismatch.schema_path(), "$.Root.value");
    assert_eq!(mismatch.rust_path(), "$.Root.value");
    assert!(matches!(mismatch.expected(), BindingShape::Builtin(_)));
    assert!(matches!(mismatch.actual(), BindingShape::Builtin(_)));
    let diagnostic = errors
        .diagnostics()
        .first()
        .test_expect("public binding errors should include readable diagnostics")?;
    assert!(diagnostic.message().contains("field shape mismatch"));
    Ok(())
}

#[test]
fn public_conversion_limits_reject_depths_above_the_hard_cap() -> TestResult {
    assert_eq!(
        ConversionLimits::new(10, MAX_CONVERSION_DEPTH)
            .test_value()?
            .max_depth(),
        MAX_CONVERSION_DEPTH
    );
    let error = ConversionLimits::new(10, MAX_CONVERSION_DEPTH + 1)
        .err()
        .test_value()?;
    assert_eq!(error.requested_depth(), MAX_CONVERSION_DEPTH + 1);
    assert_eq!(error.maximum_depth(), MAX_CONVERSION_DEPTH);
    assert_eq!(
        ConversionLimits::default()
            .with_max_depth(MAX_CONVERSION_DEPTH + 1)
            .err()
            .test_value()?
            .maximum_depth(),
        MAX_CONVERSION_DEPTH
    );
    Ok(())
}

#[test]
fn public_binding_limits_validate_depth_at_construction_and_update() -> TestResult {
    let zero_depth = BindingLimits::new(10, 0, 5, 100).test_value()?;
    assert_eq!(zero_depth.max_depth(), 0);
    let maximum_depth = BindingLimits::new(10, MAX_CONVERSION_DEPTH, 5, 100).test_value()?;
    assert_eq!(maximum_depth.max_depth(), MAX_CONVERSION_DEPTH);

    let construction_error: BindingLimitError =
        BindingLimits::new(10, MAX_CONVERSION_DEPTH + 1, 5, 100)
            .err()
            .test_value()?;
    assert_eq!(
        construction_error.requested_depth(),
        MAX_CONVERSION_DEPTH + 1
    );
    assert_eq!(construction_error.maximum_depth(), MAX_CONVERSION_DEPTH);

    let update_error: BindingLimitError = maximum_depth
        .with_max_depth(MAX_CONVERSION_DEPTH + 1)
        .err()
        .test_value()?;
    assert_eq!(update_error.requested_depth(), MAX_CONVERSION_DEPTH + 1);
    assert_eq!(update_error.maximum_depth(), MAX_CONVERSION_DEPTH);
    assert_eq!(maximum_depth.with_max_depth(0).test_value()?.max_depth(), 0);
    assert_eq!(
        BindOptions::default()
            .with_max_depth(0)
            .test_value()?
            .limits()
            .max_depth(),
        0
    );
    assert_eq!(
        TypolOptions::default()
            .with_max_typed_depth(0)
            .test_value()?
            .bind()
            .limits()
            .max_depth(),
        0
    );
    Ok(())
}

#[test]
fn full_validation_errors_own_schema_snapshots_and_safe_diagnostics() -> TestResult {
    let errors = {
        let schema = compile("entry struct Root { value: u8 @range(1, 3) @secret }")?;
        let binding =
            TestOpeo::value(|out| TypedSchema::<Root>::bind(&schema, BindOptions::default(), out))?;
        TestOpeo::error(|out| binding.serialize(&Root { value: 99 }, CodecOptions::default(), out))?
    };
    assert_eq!(errors.stage(), CodecStage::Validation);
    assert!(errors.config_parse_errors().is_none());
    let diagnostic = errors
        .diagnostics()
        .first()
        .test_expect("validation errors should include diagnostics")?;
    assert_eq!(diagnostic.code(), DiagnosticCode::CONFIG_VALIDATION_ERROR);
    assert_eq!(diagnostic.primary(), None);
    assert_eq!(diagnostic.value_source(), Some(ValueSource::RustValue));
    assert_eq!(diagnostic.message(), "value is hidden");
    assert!(!format!("{errors:?} {errors}").contains("99"));
    for label in diagnostic.labels() {
        assert!(errors.span_location(label.span()).is_some());
    }
    let schema =
        compile("entry struct Root { value: u8 } @assert(self.value > 0, \"positive number\")")?;
    let binding =
        TestOpeo::value(|out| TypedSchema::<Root>::bind(&schema, BindOptions::default(), out))?;
    let errors =
        TestOpeo::error(|out| binding.serialize(&Root { value: 0 }, CodecOptions::default(), out))?;
    assert_eq!(errors.stage(), CodecStage::Validation);
    assert_eq!(
        errors
            .diagnostics()
            .first()
            .test_expect("validation errors should include diagnostics")?
            .value_source(),
        Some(ValueSource::RustValue)
    );
    let errors = TestOpeo::error(|out| {
        binding.deserialize(
            request(&mut SliceSource::new(("{value: 256}").as_bytes())),
            &mut NoEnvironment,
            ConversionLimits::default(),
            out,
        )
    })?;
    assert_eq!(errors.stage(), CodecStage::Parsing);
    let parser = errors.config_parse_errors().test_value()?;
    assert_eq!(parser.config_source_name(), Some("config"));
    assert!(
        errors
            .span_location(
                errors
                    .diagnostics()
                    .first()
                    .test_expect("parse errors should include diagnostics")?
                    .primary()
                    .test_value()?
            )
            .is_some()
    );
    Ok(())
}

fn object(value: ConfigValue) -> TestResult<ConfigValue> {
    let field = ValueSupport::field("value".to_owned(), value);
    TestOpeo::value(|out| ValueSupport::structure(vec![field], None, out))
}

#[test]
fn arbitrary_input_provenance_defaults_and_validation_are_public() -> TestResult {
    let schema = compile(
        "entry struct Root { value: u8 @range(1, 3), fallback: u8 = 2, absent: option<u8> }",
    )?;
    let input = object(ValueSupport::scalar(|builder, out| {
        builder.add_unsigned(2, ValueSource::SchemaDefault, out)
    }))?;
    let validated = TestOpeo::value(|out| {
        ConfigValueProcessor::new(&schema).validate(
            input,
            ConfigValidationRequest::new(ConfigValidationOptions::default()),
            out,
        )
    })?;
    assert_eq!(validated.as_config_value().source(), ValueSource::RustValue);
    let ValueKind::Structure(fields) = validated.as_config_value().root().as_ref() else {
        return Err("validated root value should be a struct".into());
    };
    assert_eq!(
        fields
            .into_iter()
            .next()
            .test_expect("root value should contain the value field")?
            .value()
            .source(),
        ValueSource::RustValue
    );
    assert_eq!(
        fields
            .into_iter()
            .nth(1)
            .test_expect("root value should contain the fallback field")?
            .value()
            .source(),
        ValueSource::SchemaDefault
    );
    assert_eq!(
        fields
            .into_iter()
            .nth(2)
            .test_expect("root value should contain the absent field")?
            .value()
            .source(),
        ValueSource::RustValue
    );
    let invalid = object(ValueSupport::scalar(|builder, out| {
        builder.add_unsigned(9, ValueSource::SchemaDefault, out)
    }))?;
    let errors = TestOpeo::error(|out| {
        ConfigValueProcessor::new(&schema).validate(
            invalid,
            ConfigValidationRequest::new(ConfigValidationOptions::default()),
            out,
        )
    })?;
    assert_eq!(
        errors
            .diagnostics()
            .first()
            .test_expect("validation errors should include diagnostics")?
            .code(),
        DiagnosticCode::CONFIG_VALIDATION_ERROR
    );
    assert_eq!(
        errors
            .diagnostics()
            .first()
            .test_expect("validation errors should include diagnostics")?
            .value_source(),
        Some(ValueSource::RustValue)
    );
    Ok(())
}

#[test]
fn shared_budget_mismatch_and_exhausted_balance_are_rejected() -> TestResult {
    let schema = compile("entry struct Root { value: u8 }")?;
    let options = ConfigValidationOptions::default();
    let different =
        TestOpeo::value(|out| ExecutionBudget::new(ExecutionLimits::new(123, 1_048_576), out))?;
    let errors = TestOpeo::error(|out| {
        ConfigParser::new(&schema).parse_with_budget(
            request(&mut SliceSource::new(("{value: 1}").as_bytes())),
            &mut NoEnvironment,
            &different,
            out,
        )
    })?;
    assert_eq!(
        errors
            .diagnostics()
            .first()
            .test_expect("budget mismatches should include diagnostics")?
            .code(),
        DiagnosticCode::INTERNAL_ERROR
    );
    let input = object(ValueSupport::scalar(|builder, out| {
        builder.add_unsigned(1, ValueSource::RustValue, out)
    }))?;
    let errors = TestOpeo::error(|out| {
        ConfigValueProcessor::new(&schema).validate_with_budget(
            input,
            ConfigValidationRequest::new(options),
            &different,
            out,
        )
    })?;
    assert_eq!(
        errors
            .diagnostics()
            .first()
            .test_expect("budget mismatches should include diagnostics")?
            .code(),
        DiagnosticCode::INTERNAL_ERROR
    );
    let budget = TestOpeo::value(|out| ExecutionBudget::new(*options.execution(), out))?;
    TestOpeo::value::<BudgetError, _>(|out| budget.consume(options.execution().max_steps(), out))?;
    let input = object(ValueSupport::scalar(|builder, out| {
        builder.add_unsigned(1, ValueSource::RustValue, out)
    }))?;
    let errors = TestOpeo::error(|out| {
        ConfigValueProcessor::new(&schema).validate_with_budget(
            input,
            ConfigValidationRequest::new(options),
            &budget,
            out,
        )
    })?;
    assert_eq!(
        errors
            .diagnostics()
            .first()
            .test_expect("resource exhaustion should include diagnostics")?
            .code(),
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
    );
    Ok(())
}

#[test]
fn parsing_and_final_target_share_the_same_step_limit() -> TestResult {
    let schema = compile("entry struct Root { value: u8 }")?;
    let binding =
        TestOpeo::value(|out| TypedSchema::<Root>::bind(&schema, BindOptions::default(), out))?;
    let defaults = ConfigParseOptions::default();
    let budget = TestOpeo::value(|out| ExecutionBudget::new(*defaults.execution(), out))?;
    drop(TestOpeo::value(|out| {
        ConfigParser::new(&schema).parse_with_budget(
            request(&mut SliceSource::new(("{value: 1}").as_bytes())),
            &mut NoEnvironment,
            &budget,
            out,
        )
    })?);
    let options = ConfigParseOptions::new(
        *defaults.resources(),
        *defaults.config(),
        ExecutionLimits::new(budget.steps(), defaults.execution().max_workspace_bytes()),
    );
    let errors = TestOpeo::error(|out| {
        binding.deserialize(
            ConfigParseRequest::new(
                &mut SliceSource::new(("{value: 1}").as_bytes()),
                "config",
                options,
            ),
            &mut NoEnvironment,
            ConversionLimits::default(),
            out,
        )
    })?;
    assert_eq!(errors.stage(), CodecStage::Conversion);
    assert_eq!(
        errors
            .diagnostics()
            .first()
            .test_expect("decode errors should include diagnostics")?
            .code(),
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
    );
    let options = CodecOptions::new(
        ConfigValidationOptions::new(
            *defaults.resources(),
            ConfigLimits::default()
                .with_max_environment_entries(0)
                .with_max_environment_bytes(0)
                .with_max_constructed_text_bytes(16_777_216)
                .with_max_display_name_bytes(0),
            *defaults.execution(),
        ),
        ConversionLimits::default(),
        0,
    );
    let errors = TestOpeo::error(|out| binding.serialize(&Root { value: 1 }, options, out))?;
    assert_eq!(errors.stage(), CodecStage::Encoding);
    assert_eq!(
        errors
            .diagnostics()
            .first()
            .test_expect("encode errors should include diagnostics")?
            .code(),
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
    );
    Ok(())
}

#[derive(Typol)]
#[typol(rename = "Root")]
struct TextRoot {
    value: String,
}

#[test]
fn encoding_rejects_unrepresentable_values_after_schema_validation() -> TestResult {
    let schema = compile("entry struct Root { value: string @secret }")?;
    let binding =
        TestOpeo::value(|out| TypedSchema::<TextRoot>::bind(&schema, BindOptions::default(), out))?;
    let errors = TestOpeo::error(|out| {
        binding.serialize(
            &TextRoot {
                value: "secret\0".into(),
            },
            CodecOptions::default(),
            out,
        )
    })?;
    assert_eq!(errors.stage(), CodecStage::Encoding);
    assert_eq!(
        errors
            .diagnostics()
            .first()
            .test_expect("encode errors should include diagnostics")?
            .code(),
        DiagnosticCode::CONFIG_ENCODING_ERROR
    );
    assert!(!format!("{errors:?} {errors}").contains("secret"));
    Ok(())
}

#[derive(Debug, PartialEq, Typol)]
struct FacadeRoot {
    value: u8,
}

#[derive(Debug, PartialEq, Typol)]
struct EmptyEnvironmentRoot {
    value: String,
}

#[test]
fn facade_reads_text_and_reader_with_one_reusable_binding() -> TestResult {
    let loader = SchemaLoader::new(TypolOptions::default());
    let schema = TestOpeo::value(|out| {
        loader.compile_text(b"facade", "entry struct FacadeRoot { value: u8 }", out)
    })?;
    let binding = TestOpeo::value(|out| schema.bind::<FacadeRoot>(out))?;
    let mut environment = EmptyEnvironment;
    let first = TestOpeo::value(|out| {
        binding.read(
            ConfigInput::text("{value: 7}").named("memory config"),
            &mut environment,
            out,
        )
    })?;
    let mut reader = Cursor::new(b"{value: 9}");
    let second = TestOpeo::value(|out| {
        binding.read(
            ConfigInput::reader(&mut reader).named("reader"),
            &mut environment,
            out,
        )
    })?;
    let encoded = TestOpeo::value(|out| binding.write(&second, WriteOptions::default(), out))?;
    let dynamic: ValidatedConfigValue = TestOpeo::value(|out| {
        schema.read_value(ConfigInput::text("{value: 11}"), &mut environment, out)
    })?;

    assert_eq!(first, FacadeRoot { value: 7 });
    assert_eq!(second, FacadeRoot { value: 9 });
    assert!(encoded.contains("value"));
    assert!(matches!(
        dynamic.as_config_value().as_ref(),
        ValueKind::Structure(_)
    ));
    Ok(())
}

#[test]
fn standard_empty_environment_uses_missing_value_fallback() -> TestResult {
    let loader = SchemaLoader::new(TypolOptions::default());
    let schema = TestOpeo::value(|out| {
        loader.compile_text(
            b"empty-environment",
            r#"entry struct EmptyEnvironmentRoot { value: string @env("TYPOL_EMPTY_PROVIDER_TEST") = "fallback" }"#,
            out,
        )
    })?;
    let binding = TestOpeo::value(|out| schema.bind::<EmptyEnvironmentRoot>(out))?;
    let value = TestOpeo::value(|out| {
        binding.read(
            ConfigInput::text(r#"{value: env("TYPOL_EMPTY_PROVIDER_TEST")}"#),
            &mut EmptyEnvironment,
            out,
        )
    })?;

    assert_eq!(
        value,
        EmptyEnvironmentRoot {
            value: "fallback".into(),
        }
    );
    Ok(())
}

macro_rules! nested_vec_type {
    ($leaf:ty;) => {
        $leaf
    };
    ($leaf:ty; x $($layers:tt)*) => {
        Vec<nested_vec_type!($leaf; $($layers)*)>
    };
}

macro_rules! nested_vec_value {
    ($leaf:expr;) => {
        $leaf
    };
    ($leaf:expr; x $($layers:tt)*) => {
        vec![nested_vec_value!($leaf; $($layers)*)]
    };
}

type NestedDepth127 = nested_vec_type!(u8;
        x x x x x x x x x x x x x x x x x x
        x x x x x x x x x x x x x x x x x x
        x x x x x x x x x x x x x x x x x x
        x x x x x x x x x x x x x x x x x x
        x x x x x x x x x x x x x x x x x x
        x x x x x x x x x x x x x x x x x x
        x x x x x x x x x x x x x x x x x x
);
type NestedDepth128 = nested_vec_type!(u8;
        x x x x x x x x x x x x x x x x x x
        x x x x x x x x x x x x x x x x x x
        x x x x x x x x x x x x x x x x x x
        x x x x x x x x x x x x x x x x x x
        x x x x x x x x x x x x x x x x x x
        x x x x x x x x x x x x x x x x x x
        x x x x x x x x x x x x x x x x x x
        x
);
type NestedDepth129 = nested_vec_type!(u8;
        x x x x x x x x x x x x x x x x x x
        x x x x x x x x x x x x x x x x x x
        x x x x x x x x x x x x x x x x x x
        x x x x x x x x x x x x x x x x x x
        x x x x x x x x x x x x x x x x x x
        x x x x x x x x x x x x x x x x x x
        x x x x x x x x x x x x x x x x x x
        x x
);

#[derive(Debug, PartialEq, Typol)]
struct DeepRoot<T> {
    value: T,
}

fn assert_deep_roundtrip<T: BudgetedTypol + PartialEq + Debug>(
    depth: usize,
    value: DeepRoot<T>,
) -> TestResult {
    let options = TypolOptions::default()
        .with_max_typed_depth(depth)
        .test_value()?;
    let source_layers = depth - 1;
    let schema_text = format!(
        "entry struct DeepRoot {{ value: {}u8{} }}",
        "[".repeat(source_layers),
        "]".repeat(source_layers)
    );
    let compiler = TestOpeo::value(|out| SchemaCompiler::new(*options.schema(), out))?;
    let schema = match TestOpeo::outcome(|out| {
        compiler.compile_without_imports(SchemaSource::from_str(b"deep", &schema_text), out)
    }) {
        TestOpeoOutcome::Value(schema) => schema,
        TestOpeoOutcome::Error(errors) => {
            let diagnostic = errors.diagnostics().first();
            return Err(TestFailure::new(format!(
                "schema compilation failed at depth {depth}: diagnostic code {:?}, context {:?}",
                diagnostic.map(|diagnostic| diagnostic.code()),
                diagnostic.and_then(|diagnostic| diagnostic.context())
            )));
        }
    };
    let binding =
        TestOpeo::value(|out| TypedSchema::<DeepRoot<T>>::bind(&schema, options.bind(), out))?;
    let input = format!(
        "{{value: {}7{}}}",
        "[".repeat(source_layers),
        "]".repeat(source_layers)
    );
    let decoded = TestOpeo::value(|out| {
        binding.deserialize(
            ConfigParseRequest::new(
                &mut SliceSource::new(input.as_bytes()),
                "deep",
                options.config(),
            ),
            &mut NoEnvironment,
            options.conversion(),
            out,
        )
    })?;
    assert_eq!(decoded, value);
    let serialized = TestOpeo::value(|out| {
        binding.serialize(
            &decoded,
            CodecOptions::default()
                .with_validation(options.validation())
                .with_conversion(options.conversion()),
            out,
        )
    })?;
    let decoded_again = TestOpeo::value(|out| {
        binding.deserialize(
            ConfigParseRequest::new(
                &mut SliceSource::new(serialized.as_bytes()),
                "deep",
                options.config(),
            ),
            &mut NoEnvironment,
            options.conversion(),
            out,
        )
    })?;
    assert_eq!(decoded_again, value);
    Ok(())
}

#[test]
fn facade_depth_contract_roundtrips_at_127_and_128_and_rejects_129_early() -> TestResult {
    let _: (PhantomData<NestedDepth127>, PhantomData<NestedDepth128>) = (PhantomData, PhantomData);
    assert_deep_roundtrip(
        127,
        DeepRoot {
            value: nested_vec_value!(7u8;
            x x x x x x x x x x x x x x x x x x
            x x x x x x x x x x x x x x x x x x
            x x x x x x x x x x x x x x x x x x
            x x x x x x x x x x x x x x x x x x
            x x x x x x x x x x x x x x x x x x
            x x x x x x x x x x x x x x x x x x
            x x x x x x x x x x x x x x x x x x
                ),
        },
    )?;
    assert_deep_roundtrip(
        128,
        DeepRoot {
            value: nested_vec_value!(7u8;
            x x x x x x x x x x x x x x x x x x
            x x x x x x x x x x x x x x x x x x
            x x x x x x x x x x x x x x x x x x
            x x x x x x x x x x x x x x x x x x
            x x x x x x x x x x x x x x x x x x
            x x x x x x x x x x x x x x x x x x
            x x x x x x x x x x x x x x x x x x
            x
                ),
        },
    )?;

    let options = TypolOptions::default()
        .with_schema(SchemaCompileOptions::default().with_max_nesting_depth(131))
        .with_config_limits(ConfigLimits::default().with_max_nesting_depth(129));
    let schema_layers = 128;
    let schema_text = format!(
        "entry struct DeepRoot {{ value: {}u8{} }}",
        "[".repeat(schema_layers),
        "]".repeat(schema_layers)
    );
    let compiler = TestOpeo::value(|out| SchemaCompiler::new(*options.schema(), out))?;
    let schema = TestOpeo::value(|out| {
        compiler.compile_without_imports(SchemaSource::from_str(b"deep", &schema_text), out)
    })?;
    let errors = TestOpeo::error(|out| {
        TypedSchema::<DeepRoot<NestedDepth129>>::bind(&schema, options.bind(), out)
    })?;
    let diagnostic = errors
        .diagnostics()
        .first()
        .test_expect("binding at depth 129 should return a conversion-depth diagnostic")?;
    assert_eq!(diagnostic.code(), DiagnosticCode::RESOURCE_LIMIT_EXCEEDED);
    assert_eq!(
        diagnostic
            .context()
            .and_then(|context| context.object_range()),
        Some((129, MAX_CONVERSION_DEPTH))
    );
    assert_eq!(
        diagnostic.context().map(|context| context.failure_point()),
        Some("conversion-depth-limit")
    );

    let limit_error = TypolOptions::default()
        .with_max_typed_depth(129)
        .err()
        .test_value()?;
    assert_eq!(limit_error.requested_depth(), 129);
    assert_eq!(limit_error.maximum_depth(), MAX_CONVERSION_DEPTH);
    Ok(())
}
