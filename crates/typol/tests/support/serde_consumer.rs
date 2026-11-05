#[path = "read_input.rs"]
mod read_input;

use read_input::TestSource;
use renamed_typol::{
    ConfigParseOptions, Environment, EnvironmentError, EnvironmentReadContext, Typol,
    LoadedSchema, ProcessEnvironment, ProcessEnvironmentLimits, SchemaLoader, TypolError,
    TypolOptions, TypolStage, ValidatedConfigValue, WriteOptions,
    binding::*, config::*, core::*, host::*, schema::*, semantic::*,
    opeo::{OResult, Out},
};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{DeserializeOwned, Error as DeError},
    ser::{Error as SerError, SerializeStruct},
};
use std::{
    collections::HashMap,
    fs::write,
    io::{self, Cursor, Read},
    path::Path,
    sync::atomic::{AtomicUsize, Ordering},
};
use typol_test_support::TestOpeo;

fn invoke<E, T>(
    operation: impl for<'slot> FnOnce(Out<'slot, 'slot, E>) -> OResult<'slot, T>,
) -> T
where
    E: std::fmt::Debug,
{
    match TestOpeo::call(operation) {
        Ok(value) => value,
        Err(error) => panic!("OPEO call failed: {error:?}"),
    }
}

fn invoke_error<E, T>(
    operation: impl for<'slot> FnOnce(Out<'slot, 'slot, E>) -> OResult<'slot, T>,
) -> E
where
    E: std::fmt::Debug,
{
    match TestOpeo::error(operation) {
        Ok(error) => error,
        Err(failure) => panic!("{failure}"),
    }
}

fn read_serde<'slot, T: BudgetedTypol + DeserializeOwned>(
    loaded: &LoadedSchema,
    input: TestSource<'_>,
    mut out: Out<'slot, '_, TypolError>,
) -> OResult<'slot, T> {
    let bound = loaded.bind::<T>(out.reborrow())?;
    let mut environment = ProcessEnvironment::new(ProcessEnvironmentLimits::default());
    input.read_serde(&bound, &mut environment, out)
}

fn read_value_with_process_environment<'slot>(
    loaded: &LoadedSchema,
    input: TestSource<'_>,
    out: Out<'slot, '_, TypolError>,
) -> OResult<'slot, ValidatedConfigValue> {
    let mut environment = ProcessEnvironment::new(ProcessEnvironmentLimits::default());
    input.read_value(loaded, &mut environment, out)
}

fn read_serde_with_environment<'slot, T: BudgetedTypol + DeserializeOwned>(
    loaded: &LoadedSchema,
    input: TestSource<'_>,
    environment: &mut dyn Environment,
    mut out: Out<'slot, '_, TypolError>,
) -> OResult<'slot, T> {
    input.read_serde(&loaded.bind::<T>(out.reborrow())?, environment, out)
}

struct Resolver;

impl SchemaResolver for Resolver {
    fn resolve<'slot, 'borrow, 'src>(
        &'src mut self,
        _request: SchemaResolveRequest<'_>,
        out: Out<'slot, 'borrow, ResolverError>,
    ) -> OResult<'slot, SchemaSource<'src>> {
        Err(out.fail(ResolverError::NotFound))
    }
}

struct Provider;

impl Environment for Provider {
    fn read<'slot, 'borrow, 'env>(
        &'env mut self,
        _: &str,
        _: EnvironmentReadContext<'_>,
        _: Out<'slot, 'borrow, EnvironmentError>,
    ) -> OResult<'slot, Option<&'env [u8]>> {
        Ok(Some("actual secret".as_bytes()))
    }
}

fn compile(text: &str) -> CompiledSchema {
    let compiler = invoke(|out| {
        SchemaCompiler::new(SchemaCompileOptions::default(), out)
    });
    invoke(|out| {
        compiler.compile(SchemaSource::from_str(b"serde", text), &mut Resolver, None, out)
    })
}

fn request<'a>(input: &'a mut SliceSource<'_>) -> ConfigParseRequest<'a> {
    ConfigParseRequest::new(input, "config", ConfigParseOptions::default())
}

fn assert_serde_failure_point(errors: &CodecErrors, expected: &str) {
    assert_eq!(
        errors.diagnostic_context(0).unwrap().failure_point(),
        expected
    );
}

#[derive(Debug, PartialEq, Typol, Serialize, Deserialize)]
#[typol(crate = "renamed_typol")]
enum Choice {
    Unit,
    Single(u8),
    #[typol(rename = "Object")]
    #[serde(rename = "Object")]
    Fields {
        #[typol(rename = "n")]
        #[serde(rename = "n")]
        count: u8,
        label: String,
    },
    Empty {},
}

#[derive(Debug, PartialEq, Typol, Serialize, Deserialize)]
#[typol(crate = "renamed_typol")]
struct Child {
    high: u8,
    low: u8,
}

#[derive(Debug, PartialEq, Typol, Serialize, Deserialize)]
#[typol(crate = "renamed_typol", rename = "Root")]
#[serde(rename = "Root")]
struct Application {
    child: Child,
    choice: Choice,
    #[typol(rename = "name")]
    #[serde(rename = "name")]
    label: String,
    optional: Option<u8>,
    dictionary: HashMap<String, u8>,
    values: [u8; 2],
}

fn roundtrips_and_validation() {
    let schema = compile(
        "struct Child { low: u8, high: u8 } @assert(self.low < self.high, \"low value is less than high value\")\n enum Choice { Unit, Single(u8), Object { label: string, n: u8 @range(1, 9) } @assert(self.n < root.child.high, \"payload is less than root high value\"), Empty {} }\n entry struct Root { name: string @secret @env(\"SECRET\"), choice: Choice, optional: option<u8>, dictionary: map<string, u8>, values: [u8; 2] @unique, child: Child }",
    );
    let binding = invoke(|out| TypedSchema::<Application>::bind(&schema, BindOptions::default(), out));
    let input = "{child: {high: 8, low: 2}, values: [1, 2], dictionary: {\"😀\": 2, \"a\": 1}, choice: .Object {n: 3, label: \"label\"}, name: env(\"SECRET\")}";
    let mut value = invoke(|out| binding.deserialize_serde(
            request(&mut SliceSource::new((input).as_bytes())),
            &mut Provider,
            ConversionLimits::default(),
            out,
        ));
    assert_eq!(value.label, "actual secret");
    assert_eq!(value.optional, None);
    for choice in [
        Choice::Unit,
        Choice::Single(3),
        Choice::Fields {
            count: 3,
            label: "label".into(),
        },
        Choice::Empty {},
    ] {
        value.choice = choice;
        let text = invoke(|out| binding.serialize_serde_to_string(&value, WriteOptions::default(), out));
        assert!(text.contains("actual secret"));
        assert!(!text.contains("env("));
        assert_eq!(
            text,
            invoke(|out| binding.serialize_to_string(&value, WriteOptions::default(), out))
        );
        assert_eq!(
            invoke(|out| binding.deserialize_serde(
                    request(&mut SliceSource::new(text.as_bytes())),
                    &mut Provider,
                    ConversionLimits::default(),
                    out,
                )),
            value
        );
    }
    value.child.low = 8;
    assert_eq!(
        invoke_error(|out| binding.serialize_serde_to_string(&value, WriteOptions::default(), out)).stage(),
        CodecStage::Validation
    );
    value.child.low = 2;
    value.values = [1, 1];
    assert_eq!(
        invoke_error(|out| binding.serialize_serde_to_string(&value, WriteOptions::default(), out)).stage(),
        CodecStage::Validation
    );
    value.values = [1, 2];
    value.choice = Choice::Fields {
        count: 9,
        label: "secret payload".into(),
    };
    let errors = invoke_error(|out| binding.serialize_serde_to_string(&value, WriteOptions::default(), out));
    assert_eq!(errors.stage(), CodecStage::Validation);
    assert!(
        errors
            .diagnostics()
            .iter()
            .all(|error| error.primary().is_none())
    );
    for text in [
        input.replace("n: 3", "n: 0"),
        input.replace("n: 3", "n: 9"),
        input.replace("[1, 2]", "[1, 1]"),
    ] {
        assert_eq!(
            invoke_error(|out| binding.deserialize_serde(
                    request(&mut SliceSource::new(text.as_bytes())),
                    &mut Provider,
                    ConversionLimits::default(),
                    out,
                )).stage(),
            CodecStage::Parsing
        );
    }
}

fn serde_default() -> u8 {
    99
}

#[derive(Debug, PartialEq, Typol, Serialize, Deserialize)]
#[typol(crate = "renamed_typol", rename = "Root")]
#[serde(rename = "Root")]
struct Defaults {
    #[serde(skip_serializing, default = "serde_default")]
    value: u8,
    #[serde(skip_serializing)]
    optional: Option<u8>,
}

fn defaults_and_output_limits() {
    let schema = compile("entry struct Root { value: u8 = 7, optional: option<u8> }");
    let binding = invoke(|out| TypedSchema::<Defaults>::bind(&schema, BindOptions::default(), out));
    let value = Defaults {
        value: 42,
        optional: Some(3),
    };
    let text = invoke(|out| binding.serialize_serde_to_string(&value, WriteOptions::default(), out));
    assert_eq!(text, "{value: 7, optional: none}");
    assert_eq!(
        invoke(|out| binding.deserialize_serde(
                request(&mut SliceSource::new(text.as_bytes())),
                &mut Provider,
                ConversionLimits::default(),
                out,
            )),
        Defaults {
            value: 7,
            optional: None
        }
    );
    for (limit, success) in [(text.len(), true), (text.len() - 1, false)] {
        let defaults = WriteOptions::default();
        if success {
            invoke(|out| binding.serialize_serde_to_string(&value, defaults.with_max_output_bytes(limit), out));
        } else {
            let errors = invoke_error(|out| binding.serialize_serde_to_string(&value, defaults.with_max_output_bytes(limit), out));
            assert_eq!(errors.stage(), CodecStage::Encoding);
        }
    }
    let required = compile("entry struct Root { value: u8, optional: option<u8> }");
    let binding = invoke(|out| TypedSchema::<Defaults>::bind(&required, BindOptions::default(), out));
    assert_eq!(
        invoke_error(|out| binding.serialize_serde_to_string(&value, WriteOptions::default(), out))
            .diagnostics().first().map(|diagnostic| diagnostic.code()),
        Some(DiagnosticCode::RUST_CONVERSION_ERROR)
    );
}

static CONSTRUCTIONS: AtomicUsize = AtomicUsize::new(0);

#[derive(Typol)]
#[typol(crate = "renamed_typol", rename = "Root")]
struct Custom<const MODE: u8> {
    value: u8,
}

impl<const MODE: u8> Serialize for Custom<MODE> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if MODE == 0 {
            return Err(S::Error::custom("secret custom error"));
        }
        let mut fields = serializer.serialize_struct("Root", 1)?;
        match MODE {
            1 => fields.serialize_field("value", &u16::from(self.value))?,
            2 => fields.serialize_field("secret unknown field", &self.value)?,
            3 => {
                fields.serialize_field("value", &self.value)?;
                fields.serialize_field("value", &self.value)?;
            }
            _ => fields.serialize_field("value", &self.value)?,
        }
        fields.end()
    }
}

impl<'de, const MODE: u8> Deserialize<'de> for Custom<MODE> {
    fn deserialize<D: Deserializer<'de>>(_: D) -> Result<Self, D::Error> {
        CONSTRUCTIONS.fetch_add(1, Ordering::SeqCst);
        if MODE == 0 {
            Err(D::Error::custom("secret custom error"))
        } else {
            Ok(Self { value: 1 })
        }
    }
}

fn reject_custom<const MODE: u8>() {
    let schema = compile("entry struct Root { value: u8 @range(2, 9) @secret }");
    let binding = invoke(|out| TypedSchema::<Custom<MODE>>::bind(&schema, BindOptions::default(), out));
    let errors = invoke_error(|out| {
        binding.serialize_serde_to_string(&Custom { value: 3 }, WriteOptions::default(), out)
    });
    assert_eq!(errors.stage(), CodecStage::Conversion);
    assert_eq!(
        errors.diagnostics().first().map(|diagnostic| diagnostic.code()),
        Some(DiagnosticCode::RUST_CONVERSION_ERROR)
    );
    let expected = if MODE == 0 {
        "serde-user-custom-rejection"
    } else {
        "serde-schema-shape-mismatch"
    };
    assert_serde_failure_point(&errors, expected);
    assert!(!format!("{errors:?}{errors}").contains("secret"));
    CONSTRUCTIONS.store(0, Ordering::SeqCst);
    let errors = invoke_error(|out| binding.deserialize_serde(
            request(&mut SliceSource::new(("{value: 1}").as_bytes())),
            &mut Provider,
            ConversionLimits::default(),
            out,
        ));
    assert_eq!(errors.stage(), CodecStage::Parsing);
    assert_eq!(CONSTRUCTIONS.load(Ordering::SeqCst), 0);
    let errors = invoke_error(|out| binding.deserialize_serde(
            request(&mut SliceSource::new(("{value: 3}").as_bytes())),
            &mut Provider,
            ConversionLimits::default(),
            out,
        ));
    assert_eq!(errors.stage(), CodecStage::Conversion);
    assert_eq!(
        errors.diagnostics().first().map(|diagnostic| diagnostic.code()),
        Some(DiagnosticCode::RUST_CONVERSION_ERROR)
    );
    assert_serde_failure_point(
        &errors,
        if MODE == 0 {
            "serde-user-custom-rejection"
        } else {
            "serde-schema-shape-mismatch"
        },
    );
    assert!(!format!("{errors:?}{errors}").contains("secret"));
    assert_eq!(CONSTRUCTIONS.load(Ordering::SeqCst), 1);
}

#[derive(Debug, Typol, Serialize, Deserialize)]
#[typol(crate = "renamed_typol", rename = "Root")]
#[serde(rename = "Root")]
struct Scalar {
    value: u8,
}

fn shared_budget_and_snapshots() {
    let schema = compile("entry struct Root { value: u8 @range(2, 9) @secret }");
    let binding = invoke(|out| TypedSchema::<Scalar>::bind(&schema, BindOptions::default(), out));
    let defaults = ConfigParseOptions::default();
    let budget = invoke(|out| ExecutionBudget::new(*defaults.execution(), out));
    invoke(|out| ConfigParser::new(&schema).parse_with_budget(
        request(&mut SliceSource::new(("{value: 3}").as_bytes())),
        &mut Provider,
        &budget,
        out,
    ));
    let options = ConfigParseOptions::new(
        *defaults.resources(),
        *defaults.config(),
        ExecutionLimits::new(budget.steps(), defaults.execution().max_workspace_bytes()),
    );
    let errors = invoke_error(|out| binding.deserialize_serde(
            ConfigParseRequest::new(
                &mut SliceSource::new(("{value: 3}").as_bytes()),
                "config",
                options,
            ),
            &mut Provider,
            ConversionLimits::default(),
            out,
        ));
    assert_eq!(errors.stage(), CodecStage::Conversion);
    assert_eq!(
        errors.diagnostics().first().map(|diagnostic| diagnostic.code()),
        Some(DiagnosticCode::RESOURCE_LIMIT_EXCEEDED)
    );
    let errors: CodecErrors = invoke_error(|out| {
        binding.serialize_serde_to_string(&Scalar { value: 1 }, WriteOptions::default(), out)
    });
    let span = errors.diagnostics().first().unwrap().labels()[0].span();
    let location = errors.span_location(span).unwrap();
    drop(binding);
    drop(schema);
    assert_eq!(errors.span_location(span).unwrap(), location);
}

#[derive(Debug, Typol, Serialize, Deserialize)]
#[typol(crate = "renamed_typol", rename = "Root")]
#[serde(rename = "Different")]
struct Mismatched {
    value: u8,
}

fn mismatched_names_reject_after_binding() {
    let schema = compile("entry struct Root { value: u8 }");
    let binding = invoke(|out| TypedSchema::<Mismatched>::bind(&schema, BindOptions::default(), out));
    let errors = invoke_error(|out| binding.serialize_serde_to_string(&Mismatched { value: 3 }, WriteOptions::default(), out));
    assert_eq!(errors.stage(), CodecStage::Conversion);
    assert_serde_failure_point(&errors, "serde-schema-shape-mismatch");
    let errors = invoke_error(|out| binding.deserialize_serde(
                request(&mut SliceSource::new(("{value: 3}").as_bytes())),
                &mut Provider,
                ConversionLimits::default(),
                out,
            ));
    assert_eq!(errors.stage(), CodecStage::Conversion);
    assert_serde_failure_point(&errors, "serde-schema-shape-mismatch");
}

#[derive(Typol)]
#[typol(crate = "renamed_typol", rename = "Root")]
struct UnsupportedOperation {
    value: u8,
}

impl Serialize for UnsupportedOperation {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let _ = self.value;
        serializer.serialize_unit()
    }
}

fn unsupported_serde_operations_have_a_public_failure_point() {
    let schema = compile("entry struct Root { value: u8 }");
    let binding = invoke(|out| {
        TypedSchema::<UnsupportedOperation>::bind(&schema, BindOptions::default(), out)
    });
    let errors = invoke_error(|out| {
        binding.serialize_serde_to_string(
            &UnsupportedOperation { value: 3 },
            WriteOptions::default(),
            out,
        )
    });
    assert_eq!(errors.stage(), CodecStage::Conversion);
    assert_serde_failure_point(&errors, "serde-data-model-operation-unsupported");
}

struct FailingReader {
    input: Cursor<&'static [u8; 10]>,
}

impl Read for FailingReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if self.input.position() == 10 {
            return Err(io::Error::other("secret error message"));
        }
        self.input.read(output)
    }
}

fn loaded_serde() {
    let schema = "entry struct Root { value: u8 @range(2, 9) @secret }";
    write("loading.tschem", schema).unwrap();
    write("loading.tconf", "{value: 3}").unwrap();
    let loaded = invoke(|out| SchemaLoader::new(TypolOptions::default()).compile_file(Path::new("loading.tschem"), out));
    let bound = invoke(|out| loaded.bind::<Scalar>(out));
    let value = Scalar { value: 3 };
    let write_options = WriteOptions::default().with_pretty(true);
    let expected = "{\n  value: 3,\n}";
    assert_eq!(
        invoke(|out| bound.write_to_string(&value, write_options, out)),
        expected
    );
    assert_eq!(
        invoke(|out| bound.write_serde_to_string(&value, write_options, out)),
        expected
    );
    write("loading-secret-path.tconf", "{value: 1}").unwrap();
    let named_errors = invoke_error(|out| {
        read_serde::<Scalar>(
            &loaded,
            TestSource::file(Path::new("loading-secret-path.tconf")).named("config\u{1b}\n"),
            out,
        )
    });
    let config_source_id = named_errors
        .codec_errors()
        .unwrap()
        .config_source_id()
        .unwrap();
    assert_eq!(named_errors.source_name(config_source_id), Some("config\u{1b}\n"));
    let named_display = named_errors.to_string();
    assert!(named_display.contains("config\\u{1b}\\n"));
    assert!(!named_display.contains('\u{1b}'));
    assert!(!named_display.contains("loading-secret-path.tconf"));
    assert_eq!(
        invoke(|out| read_serde::<Scalar>(&loaded, TestSource::file(Path::new("loading.tconf")), out)).value,
        3
    );
    assert_eq!(
        invoke(|out| read_serde_with_environment::<Scalar>(&loaded, TestSource::file(Path::new("loading.tconf")), &mut Provider, out)).value,
        3
    );
    let mut reader = Cursor::new(b"{value: 3}".as_slice());
    assert_eq!(
        invoke(|out| read_serde::<Scalar>(&loaded, TestSource::reader(&mut reader).named("reader"), out)).value,
        3
    );
    let mut reader = Cursor::new(b"{value: 3}".as_slice());
    assert_eq!(
        invoke(|out| read_serde_with_environment::<Scalar>(
                &loaded,
                TestSource::reader(&mut reader).named("reader"),
                &mut Provider,
                out,
            )).value,
        3
    );
    let mut reader = FailingReader {
        input: Cursor::new(b"{value: 3}"),
    };
    let errors = invoke_error(|out| read_serde::<Scalar>(&loaded, TestSource::reader(&mut reader).named("reader"), out));
    assert_eq!(errors.stage(), TypolStage::ConfigRead);
    assert_eq!(
        errors.host_error().unwrap().kind(),
        HostErrorKind::ReadFailure
    );
    assert_eq!(errors.host_error().unwrap().offset(), Some(10));
    let defaults = ConfigParseOptions::default();
    let budget = invoke(|out| ExecutionBudget::new(*defaults.execution(), out));
    let mut input = invoke(|out| FileConfigInput::open(
        Path::new("loading.tconf"),
        loaded.options().file_input_limits(),
        defaults.config().max_source_bytes(),
        &budget,
        out,
    ));
    invoke(|out| ConfigParser::new(loaded.schema()).parse_with_budget(
        ConfigParseRequest::new(&mut input, "config", defaults),
        &mut Provider,
        &budget,
        out,
    ));
    let limited = invoke(|out| SchemaLoader::new(TypolOptions::default().with_config(ConfigParseOptions::new(
        *defaults.resources(),
        *defaults.config(),
        ExecutionLimits::new(budget.steps(), defaults.execution().max_workspace_bytes()),
    )))
    .compile_file(Path::new("loading.tschem"), out));
    invoke(|out| read_value_with_process_environment(&limited, TestSource::file(Path::new("loading.tconf")), out));
    let errors = invoke_error(|out| read_serde::<Scalar>(&limited, TestSource::file(Path::new("loading.tconf")), out));
    assert_eq!(errors.stage(), TypolStage::Decoding);
    assert_eq!(
        errors.codec_errors().unwrap().stage(),
        CodecStage::Conversion
    );
    assert_eq!(
        errors.diagnostics().first().map(|diagnostic| diagnostic.code()),
        Some(DiagnosticCode::RESOURCE_LIMIT_EXCEEDED)
    );
}

fn main() {
    roundtrips_and_validation();
    defaults_and_output_limits();
    reject_custom::<0>();
    reject_custom::<1>();
    reject_custom::<2>();
    reject_custom::<3>();
    shared_budget_and_snapshots();
    mismatched_names_reject_after_binding();
    unsupported_serde_operations_have_a_public_failure_point();
    loaded_serde();
}
