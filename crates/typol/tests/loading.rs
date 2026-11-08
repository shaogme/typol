use std::{
    env::temp_dir,
    error::Error,
    fs::{create_dir, read_to_string, remove_dir_all, write},
    io::{self, ErrorKind, Read},
    path::{Path, PathBuf},
    process::id,
    sync::atomic::{AtomicUsize, Ordering},
};
use typol::{
    BoundSchema, ConfigParseOptions, ConfigPath, EmptyEnvironment, Environment, EnvironmentError,
    EnvironmentReadContext, LoadedSchema, ProcessEnvironment, ProcessEnvironmentLimits,
    SchemaLoader, SourceCommitError, SourceExpectation, Sourced, Typol, TypolError, TypolOptions,
    TypolStage, ValidatedConfigValue, WriteOptions,
    binding::{
        BindOptions, CodecStage, ConversionLimits, MAX_CONVERSION_DEPTH, Typol as TypolTrait,
    },
    config::{
        ConfigFailureStage, ConfigLimits, ConfigParseErrors, ConfigParseRequest, ConfigParser,
        ConfigValidationOptions,
    },
    core::{
        BudgetError, ConfigValueBuilder, DiagnosticCode, ExecutionBudget, ExecutionLimits,
        SliceSource, SourceId, ValueSource,
    },
    host::{
        FileConfigInput, FileConfigInputLimits, FileSchemaCompileErrors, HostError, HostErrorKind,
        HostOperation, Language, RenderOptions, SourceNameEditOptions, SourceNameLimits,
        Translator,
    },
    io::{Read as TypolRead, ReadRequest},
    opeo::{OResult, Out},
    schema::{
        ResolverError, SchemaCompileContextFailure, SchemaCompileErrors, SchemaCompileOptions,
        SchemaCompiler, SchemaResolveRequest, SchemaResolver, SchemaSource,
    },
};
use typol_test_support::{
    TestExpect, TestExtract, TestFailure, TestOpeo, TestOpeoOutcome, TestResult,
    test_verify as assert, test_verify_eq as assert_eq,
};

trait BoundProcessEnvironmentTestExt<T: TypolTrait> {
    fn read_with_process_environment<'slot, R: TypolRead>(
        &self,
        input: ReadRequest<'_, R>,
        out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, T>;

    fn read_file_with_process_environment<'slot>(
        &self,
        path: &Path,
        out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, T>;

    fn read_file_named_with_process_environment<'slot>(
        &self,
        path: &Path,
        display_name: &str,
        out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, T>;

    fn read_io_with_process_environment<'slot>(
        &self,
        reader: &mut dyn Read,
        display_name: &str,
        out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, T>;
}

impl<T: TypolTrait> BoundProcessEnvironmentTestExt<T> for BoundSchema<'_, T> {
    fn read_with_process_environment<'slot, R: TypolRead>(
        &self,
        input: ReadRequest<'_, R>,
        out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, T> {
        let mut environment = ProcessEnvironment::new(ProcessEnvironmentLimits::default());
        self.read(input, &mut environment, out)
    }

    fn read_file_with_process_environment<'slot>(
        &self,
        path: &Path,
        out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, T> {
        let mut environment = ProcessEnvironment::new(ProcessEnvironmentLimits::default());
        self.read_file(path, &mut environment, out)
    }

    fn read_file_named_with_process_environment<'slot>(
        &self,
        path: &Path,
        display_name: &str,
        out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, T> {
        let mut environment = ProcessEnvironment::new(ProcessEnvironmentLimits::default());
        self.read_file_named(path, display_name, &mut environment, out)
    }

    fn read_io_with_process_environment<'slot>(
        &self,
        reader: &mut dyn Read,
        display_name: &str,
        out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, T> {
        let mut environment = ProcessEnvironment::new(ProcessEnvironmentLimits::default());
        self.read_io(reader, display_name, &mut environment, out)
    }
}

trait LoadedProcessEnvironmentTestExt {
    fn read_value_with_process_environment<'slot, R: TypolRead>(
        &self,
        input: ReadRequest<'_, R>,
        out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, ValidatedConfigValue>;

    fn read_file_with_process_environment<'slot>(
        &self,
        path: &Path,
        out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, ValidatedConfigValue>;

    fn read_file_named_with_process_environment<'slot>(
        &self,
        path: &Path,
        display_name: &str,
        out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, ValidatedConfigValue>;

    fn read_io_with_process_environment<'slot>(
        &self,
        reader: &mut dyn Read,
        display_name: &str,
        out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, ValidatedConfigValue>;
}

impl LoadedProcessEnvironmentTestExt for LoadedSchema {
    fn read_value_with_process_environment<'slot, R: TypolRead>(
        &self,
        input: ReadRequest<'_, R>,
        out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, ValidatedConfigValue> {
        let mut environment = ProcessEnvironment::new(ProcessEnvironmentLimits::default());
        self.read_value(input, &mut environment, out)
    }

    fn read_file_with_process_environment<'slot>(
        &self,
        path: &Path,
        out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, ValidatedConfigValue> {
        let mut environment = ProcessEnvironment::new(ProcessEnvironmentLimits::default());
        self.read_file_value(path, &mut environment, out)
    }

    fn read_file_named_with_process_environment<'slot>(
        &self,
        path: &Path,
        display_name: &str,
        out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, ValidatedConfigValue> {
        let mut environment = ProcessEnvironment::new(ProcessEnvironmentLimits::default());
        self.read_file_value_named(path, display_name, &mut environment, out)
    }

    fn read_io_with_process_environment<'slot>(
        &self,
        reader: &mut dyn Read,
        display_name: &str,
        out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, ValidatedConfigValue> {
        let mut environment = ProcessEnvironment::new(ProcessEnvironmentLimits::default());
        self.read_io(reader, display_name, &mut environment, out)
    }
}

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Directory {
    path: PathBuf,
}

impl Directory {
    fn new() -> TestResult<Self> {
        let path = temp_dir().join(format!(
            "typol-loading-{}-{}",
            id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        create_dir(&path).test_value()?;
        Ok(Self { path })
    }

    fn put(&self, name: &str, text: &str) -> TestResult<PathBuf> {
        let path = self.path.join(name);
        write(&path, text).test_value()?;
        Ok(path)
    }

    fn cleanup(self) -> TestResult {
        remove_dir_all(&self.path).test_value()
    }
}

#[derive(Debug, PartialEq, Typol)]
struct Root {
    value: u8,
    optional: Option<String>,
}

#[derive(Debug, Typol)]
struct Different {
    value: bool,
}

#[derive(Typol)]
struct OriginRoot {
    token: Sourced<String>,
    optional: Option<Sourced<u16>>,
    flag: Sourced<Option<bool>>,
}

#[derive(Typol)]
struct ManualOrigin {
    value: Sourced<String>,
}

#[derive(Typol)]
struct ManualFloatOrigin {
    value: Sourced<f64>,
}

#[derive(Typol)]
struct ManualRangedOrigin {
    value: Sourced<u8>,
}

#[derive(Typol)]
struct ManualOptionalOrigin {
    value: Sourced<Option<u16>>,
}

#[derive(Typol)]
struct ManualOptionalLeafOrigin {
    value: Option<Sourced<u16>>,
}

struct OriginProvider {
    token: Option<String>,
    optional: Option<String>,
    flag: Option<String>,
}

impl Environment for OriginProvider {
    fn read<'slot, 'borrow, 'env>(
        &'env mut self,
        name: &str,
        _: EnvironmentReadContext<'_>,
        _: Out<'slot, 'borrow, EnvironmentError>,
    ) -> OResult<'slot, Option<&'env [u8]>> {
        let value = match name {
            "SERVICE_TOKEN" => self.token.as_deref(),
            "OPTIONAL_PORT" => self.optional.as_deref(),
            "FLAG" => self.flag.as_deref(),
            _ => None,
        };
        Ok(value.map(str::as_bytes))
    }
}

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

#[derive(Default)]
struct Provider {
    calls: usize,
    deny: bool,
}

impl Environment for Provider {
    fn read<'slot, 'borrow, 'env>(
        &'env mut self,
        name: &str,
        _: EnvironmentReadContext<'_>,
        out: Out<'slot, 'borrow, EnvironmentError>,
    ) -> OResult<'slot, Option<&'env [u8]>> {
        if self.deny || name != "VALUE" {
            return Err(out.fail(EnvironmentError::HostFailure));
        }
        self.calls += 1;
        Ok(Some(b"3"))
    }
}

struct ReentrantEnvironment<'loaded> {
    loaded: &'loaded LoadedSchema,
    nested_succeeded: bool,
}

impl Environment for ReentrantEnvironment<'_> {
    fn read<'slot, 'borrow, 'env>(
        &'env mut self,
        _: &str,
        _: EnvironmentReadContext<'_>,
        out: Out<'slot, 'borrow, EnvironmentError>,
    ) -> OResult<'slot, Option<&'env [u8]>> {
        self.nested_succeeded = capture_load(|nested_out| {
            self.loaded.read_value_with_process_environment(
                ReadRequest::from_text("{value: 7}"),
                nested_out,
            )
        });
        Err(out.fail(EnvironmentError::HostFailure))
    }
}

struct ReentrantResolver<'loader> {
    loader: &'loader SchemaLoader,
    nested_succeeded: bool,
    child_source: SliceSource<'static>,
}

impl SchemaResolver for ReentrantResolver<'_> {
    fn resolve<'slot, 'borrow, 'src>(
        &'src mut self,
        _: SchemaResolveRequest<'_>,
        _: Out<'slot, 'borrow, ResolverError>,
    ) -> OResult<'slot, SchemaSource<'src>> {
        self.nested_succeeded = capture_load(|nested_out| {
            self.loader
                .compile_text(b"nested", "entry struct Nested {}", nested_out)
        });
        Ok(SchemaSource::from_source(b"child", &mut self.child_source))
    }
}

struct ReentrantReader<'loaded> {
    loaded: &'loaded LoadedSchema,
    reentered: bool,
}

impl Read for ReentrantReader<'_> {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        if !self.reentered {
            self.reentered = true;
            let nested_succeeded = capture_load(|nested_out| {
                self.loaded.read_value_with_process_environment(
                    ReadRequest::from_text("{value: 7}"),
                    nested_out,
                )
            });
            if !nested_succeeded {
                return Err(io::Error::new(
                    ErrorKind::InvalidData,
                    "reentrant configuration read failed",
                ));
            }
        }
        Err(io::Error::other("outer reader failed"))
    }
}

fn default_options() -> TypolOptions {
    TypolOptions::default()
}

fn load(text: &str, options: TypolOptions) -> TestResult<LoadedSchema> {
    let loader = SchemaLoader::new(options);
    TestOpeo::value(|out| loader.compile_text(b"root", text, out))
}

fn load_value<T>(
    operation: impl for<'slot> FnOnce(Out<'slot, 'slot, TypolError>) -> OResult<'slot, T>,
) -> TestResult<T> {
    TestOpeo::value(operation)
}

fn load_error<T>(
    operation: impl for<'slot> FnOnce(Out<'slot, 'slot, TypolError>) -> OResult<'slot, T>,
) -> TestResult<TypolError> {
    TestOpeo::error(operation)
}

#[test]
fn bound_write_inherits_loaded_conversion_limits_unless_explicitly_overridden() -> TestResult {
    let loaded_limits = ConversionLimits::new(0, MAX_CONVERSION_DEPTH).test_value()?;
    let loaded = load(
        "entry struct Root { value: u8, optional: option<string> }",
        default_options().with_conversion(loaded_limits),
    )?;
    let binding = load_value(|out| loaded.bind::<Root>(out))?;
    let value = Root {
        value: 7,
        optional: None,
    };
    let errors = load_error(|out| binding.write_to_string(&value, WriteOptions::default(), out))?;
    assert_eq!(errors.stage(), TypolStage::Encoding);
    assert_eq!(
        errors.codec_errors().test_value()?.stage(),
        CodecStage::Conversion
    );
    let options = WriteOptions::default()
        .with_conversion_limits(ConversionLimits::default())
        .with_pretty(true);
    assert_eq!(
        load_value(|out| binding.write_to_string(&value, options, out))?,
        "{\n  value: 7,\n  optional: none,\n}"
    );

    let validation_defaults = default_options();
    let restrictive_validation = ConfigValidationOptions::new(
        *validation_defaults.config().excerpts(),
        *validation_defaults.config().regex(),
        ConfigLimits::default().with_max_nesting_depth(0),
        *validation_defaults.config().execution(),
    );
    let errors = load_error(|out| {
        binding.write_to_string(
            &value,
            WriteOptions::default()
                .with_validation(restrictive_validation)
                .with_conversion_limits(ConversionLimits::default()),
            out,
        )
    })?;
    assert_eq!(errors.stage(), TypolStage::Encoding);
    assert_eq!(
        errors.codec_errors().test_value()?.stage(),
        CodecStage::Validation
    );
    Ok(())
}

#[test]
fn explicit_translator_localizes_schema_error_reports() -> TestResult {
    let loader = SchemaLoader::new(default_options());
    let errors = load_error(|out| loader.compile_text(b"root", "struct Root {}", out))?;
    let rendered = TestOpeo::value(|out| {
        errors.render(
            Translator::new(Language::Chinese),
            RenderOptions::default(),
            out,
        )
    })?;

    assert!(rendered.contains("Schema 编译失败"));
    assert!(rendered.contains("根 Schema 必须声明 entry 结构体"));
    assert!(!rendered.contains("root schema must declare"));
    Ok(())
}

fn assert_file_display_name(
    errors: &TypolError,
    source_id: SourceId,
    expected_name: &str,
    path: &Path,
) -> TestResult {
    assert_eq!(errors.source_name(source_id), Some(expected_name));
    let rendered = TestOpeo::value(|out| {
        errors.render(
            Translator::new(Language::English),
            RenderOptions::default(),
            out,
        )
    })?;
    let output = format!("{errors}{rendered}");
    assert!(output.contains("config\\u{1b}\\n"));
    assert!(!output.contains('\x1b'));
    assert!(!output.contains(path.to_string_lossy().as_ref()));
    Ok(())
}

fn capture_load<T>(
    operation: impl for<'slot> FnOnce(Out<'slot, 'slot, TypolError>) -> OResult<'slot, T>,
) -> bool {
    TestOpeo::outcome(operation).is_ok()
}

fn schema_value<T>(
    operation: impl for<'slot> FnOnce(Out<'slot, 'slot, SchemaCompileErrors>) -> OResult<'slot, T>,
) -> TestResult<T> {
    TestOpeo::value(operation)
}

fn host_value<T>(
    operation: impl for<'slot> FnOnce(Out<'slot, 'slot, HostError>) -> OResult<'slot, T>,
) -> TestResult<T> {
    TestOpeo::value(operation)
}

fn budget_value<T>(
    operation: impl for<'slot> FnOnce(Out<'slot, 'slot, BudgetError>) -> OResult<'slot, T>,
) -> TestResult<T> {
    TestOpeo::value(operation)
}

fn parse_value<T>(
    operation: impl for<'slot> FnOnce(Out<'slot, 'slot, ConfigParseErrors>) -> OResult<'slot, T>,
) -> TestResult<T> {
    TestOpeo::value(operation)
}

#[test]
fn files_and_memory_return_owned_values_and_refresh_each_call() -> TestResult {
    let directory = Directory::new()?;
    let schema = directory.put(
        "root.tschem",
        "entry struct Root { value: u8 = 7, optional: option<string> }",
    )?;
    let config = directory.put("root.tconf", "{}")?;
    let loader = SchemaLoader::new(TypolOptions::default());
    let loaded = load_value(|out| loader.compile_file(&schema, out))?;
    assert_eq!(loaded.options().bind(), BindOptions::default());
    let typed = load_value(|out| loaded.bind::<Root>(out))?;
    let value = load_value(|out| typed.read_file_with_process_environment(&config, out))?;
    assert_eq!(
        value,
        Root {
            value: 7,
            optional: None
        }
    );
    load_value(|out| loaded.read_file_with_process_environment(&config, out))?;
    let loaded = load(
        "entry struct Root { value: u8 @env(\"VALUE\"), optional: option<string> }",
        default_options(),
    )?;
    write(&config, "{value: env(\"VALUE\")}").test_value()?;
    let mut provider = Provider::default();
    let typed = load_value(|out| loaded.bind::<Root>(out))?;
    for _ in 0..2 {
        assert_eq!(
            load_value(|out| typed.read_file(&config, &mut provider, out))?.value,
            3
        );
    }
    assert_eq!(provider.calls, 2);
    drop(typed);
    drop(loaded);
    directory.cleanup()?;
    assert_eq!(value.value, 7);
    Ok(())
}

#[test]
fn loaded_schema_writes_the_edited_document_with_its_original_snapshot() -> TestResult {
    let loaded = load("entry struct Root { value: u8 }", TypolOptions::default())?;
    let directory = Directory::new()?;
    let config = directory.put("root.tconf", "{ value: 1, // keep this comment\n}")?;
    let mut environment = EmptyEnvironment;
    let mut document = load_value(|out| loaded.read_file(&config, &mut environment, out))?;
    let budget =
        TestOpeo::value(|out| ExecutionBudget::new(ExecutionLimits::new(100_000, 100_000), out))?;
    let value = TestOpeo::value(|mut out| {
        let mut builder = ConfigValueBuilder::new(Some(&budget), out.reborrow())?;
        let root = builder.add_unsigned(2, ValueSource::RustValue, out.reborrow())?;
        builder.finish(root, out)
    })?;
    let path = ConfigPath::new().field("value");
    TestOpeo::value(|out| document.set_value(&path, &value, &mut environment, out))?;
    let revision = loaded
        .write_file_with_expectation(&mut document, &config, SourceExpectation::ReplaceAny)
        .test_value()?;
    assert!(revision.value() > 0);
    assert_eq!(
        read_to_string(&config).test_value()?,
        "{ value: 2, // keep this comment\n}"
    );
    directory.cleanup()?;
    Ok(())
}

#[test]
fn file_write_enforces_input_file_limit_and_exact_snapshot_fails_closed() -> TestResult {
    let source = "{value: 1}";
    let limits = FileConfigInputLimits::default().with_max_file_bytes(source.len() - 1);
    let loaded = load(
        "entry struct Root { value: u8 }",
        default_options().with_file_input_limits(limits),
    )?;
    let mut environment = EmptyEnvironment;
    let mut document = load_value(|out| {
        loaded.read_document(ReadRequest::from_text(source), &mut environment, out)
    })?;
    let directory = Directory::new()?;
    let limited_target = directory.path.join("limited.tconf");
    let limited = loaded.write_file_with_expectation(
        &mut document,
        &limited_target,
        SourceExpectation::MustBeAbsent,
    );
    assert!(matches!(
        limited,
        Err(SourceCommitError::ResourceLimitExceeded)
    ));
    assert!(!limited_target.exists());

    let original = directory.put("existing.tconf", source)?;
    let unrestricted = load("entry struct Root { value: u8 }", default_options())?;
    let mut unrestricted_environment = EmptyEnvironment;
    let mut exact_document =
        load_value(|out| unrestricted.read_file(&original, &mut unrestricted_environment, out))?;
    let exact = unrestricted.write_file(&mut exact_document, &original);
    assert!(matches!(
        exact,
        Err(SourceCommitError::Writer(error))
            if error.kind() == typol::host::SourceWriteErrorKind::AtomicReplaceUnsupported
    ));
    assert_eq!(read_to_string(&original).test_value()?, source);
    directory.cleanup()?;
    Ok(())
}

#[test]
fn explicit_schema_binding_supports_typed_and_environment_reads() -> TestResult {
    let directory = Directory::new()?;
    let schema = directory.put(
        "root.tschem",
        "entry struct Root { value: u8 = 7, optional: option<string> }",
    )?;
    let environment_schema = directory.put(
        "environment.tschem",
        "entry struct Root { value: u8 @env(\"VALUE\"), optional: option<string> }",
    )?;
    let config = directory.put("root.tconf", "{}")?;
    let loader = SchemaLoader::new(TypolOptions::default());

    let loaded = load_value(|out| loader.compile_file(&schema, out))?;
    let binding = load_value(|out| loaded.bind::<Root>(out))?;
    let value = load_value(|out| binding.read_file_with_process_environment(&config, out))?;
    assert_eq!(value.value, 7);
    assert_eq!(value.optional, None);

    let input = || ReadRequest::from_text("{value: env(\"VALUE\")}").named("environment config");
    let mut provider = Provider::default();
    let loaded = load_value(|out| loader.compile_file(&environment_schema, out))?;
    let binding = load_value(|out| loaded.bind::<Root>(out))?;
    let value = load_value(|out| binding.read(input(), &mut provider, out))?;
    assert_eq!(value.value, 3);
    assert_eq!(value.optional, None);
    assert_eq!(provider.calls, 1);

    directory.cleanup()?;
    Ok(())
}

#[test]
fn sourced_environment_values_roundtrip_the_reference_and_typed_fallback() -> TestResult {
    let loaded = load(
        "entry struct OriginRoot { token: string @env(\"SERVICE_TOKEN\"), optional: option<u16> @env(\"OPTIONAL_PORT\"), flag: option<bool> @env(\"FLAG\") }",
        default_options(),
    )?;
    let binding = load_value(|out| loaded.bind::<OriginRoot>(out))?;
    let source = "{token: env(\"SERVICE_TOKEN\", \"fallback-secret\"), optional: env(\"OPTIONAL_PORT\", 9), flag: env(\"FLAG\", true)}";
    let mut first_provider = OriginProvider {
        token: Some("provider-token-one".to_string()),
        optional: Some("8123".to_string()),
        flag: Some("false".to_string()),
    };
    let read = TestOpeo::outcome(|out| {
        binding.read(ReadRequest::from_text(source), &mut first_provider, out)
    });
    let value = match read {
        TestOpeoOutcome::Value(value) => value,
        TestOpeoOutcome::Error(error) => {
            let rendered = TestOpeo::value(|out| {
                error.render(
                    Translator::new(Language::English),
                    RenderOptions::default(),
                    out,
                )
            })?;
            return Err(TestFailure::new(format!("{error:?}{rendered}")));
        }
    };
    assert_eq!(value.token.value(), "provider-token-one");
    assert_eq!(value.token.source(), ValueSource::EnvironmentVariable);
    assert_eq!(value.token.environment_name(), Some("SERVICE_TOKEN"));
    assert!(matches!(
        value.token.environment_fallback(),
        Some(typol::EnvironmentFallback::String(text)) if text == "fallback-secret"
    ));
    let optional = value
        .optional
        .as_ref()
        .test_expect("environment-backed optional should be Some")?;
    assert_eq!(*optional.value(), 8123);
    assert_eq!(optional.environment_name(), Some("OPTIONAL_PORT"));
    assert_eq!(value.flag.value(), &Some(false));
    assert_eq!(value.flag.environment_name(), Some("FLAG"));

    let encoded = load_value(|out| binding.write_to_string(&value, WriteOptions::default(), out))?;
    assert!(encoded.contains("env(\"SERVICE_TOKEN\", \"fallback-secret\")"));
    assert!(encoded.contains("env(\"OPTIONAL_PORT\", 9)"));
    assert!(encoded.contains("env(\"FLAG\", true)"));
    assert!(!encoded.contains("provider-token-one"));

    let mut second_provider = OriginProvider {
        token: Some("provider-token-two".to_string()),
        optional: Some("9000".to_string()),
        flag: Some("true".to_string()),
    };
    let reparsed = load_value(|out| {
        binding.read(ReadRequest::from_text(&encoded), &mut second_provider, out)
    })?;
    assert_eq!(reparsed.token.value(), "provider-token-two");
    assert_eq!(reparsed.token.environment_name(), Some("SERVICE_TOKEN"));
    assert_eq!(
        *reparsed
            .optional
            .as_ref()
            .test_expect("environment-backed optional should remain Some")?
            .value(),
        9000
    );
    assert_eq!(reparsed.flag.value(), &Some(true));

    let mut unset_provider = OriginProvider {
        token: None,
        optional: None,
        flag: None,
    };
    let fallback_value =
        load_value(|out| binding.read(ReadRequest::from_text(source), &mut unset_provider, out))?;
    assert_eq!(fallback_value.token.value(), "fallback-secret");
    assert_eq!(fallback_value.token.source(), ValueSource::ExplicitFallback);
    assert_eq!(fallback_value.token.environment_name(), None);
    let fallback_encoded =
        load_value(|out| binding.write_to_string(&fallback_value, WriteOptions::default(), out))?;
    assert!(!fallback_encoded.contains("env("));
    assert!(fallback_encoded.contains("fallback-secret"));

    let manual_schema = load(
        "entry struct ManualOrigin { value: string }",
        default_options(),
    )?;
    let manual_binding = load_value(|out| manual_schema.bind::<ManualOrigin>(out))?;
    let manual_value = ManualOrigin {
        value: Sourced::from_environment("UNAUTHORIZED".to_string(), "secret".to_string()),
    };
    let mut output = Vec::new();
    let error = load_error(|out| {
        manual_binding.write(&manual_value, &mut output, WriteOptions::default(), out)
    })?;
    assert_eq!(error.stage(), TypolStage::Encoding);
    assert!(output.is_empty());

    let float_schema = load(
        "entry struct ManualFloatOrigin { value: f64 @env(\"VALUE\") }",
        default_options(),
    )?;
    let float_binding = load_value(|out| float_schema.bind::<ManualFloatOrigin>(out))?;
    let invalid_float = ManualFloatOrigin {
        value: Sourced::from_environment_with_fallback("VALUE".to_string(), 1.0, f64::NAN),
    };
    let mut output = Vec::new();
    let error = load_error(|out| {
        float_binding.write(&invalid_float, &mut output, WriteOptions::default(), out)
    })?;
    assert_eq!(error.stage(), TypolStage::Encoding);
    assert!(output.is_empty());

    let ranged_schema = load(
        "entry struct ManualRangedOrigin { value: u8 @range(10, 20) @env(\"VALUE\") }",
        default_options(),
    )?;
    let ranged_binding = load_value(|out| ranged_schema.bind::<ManualRangedOrigin>(out))?;
    let invalid_ranged = ManualRangedOrigin {
        value: Sourced::from_environment_with_fallback("VALUE".to_string(), 12, 0),
    };
    let mut output = Vec::new();
    let error = load_error(|out| {
        ranged_binding.write(&invalid_ranged, &mut output, WriteOptions::default(), out)
    })?;
    assert_eq!(error.stage(), TypolStage::Encoding);
    assert!(output.is_empty());

    let string_limits = ConfigLimits::default().with_max_string_bytes(3);
    let string_schema = load(
        "entry struct ManualOrigin { value: string @env(\"X\") }",
        default_options().with_config_limits(string_limits),
    )?;
    let string_binding = load_value(|out| string_schema.bind::<ManualOrigin>(out))?;
    let invalid_string = ManualOrigin {
        value: Sourced::from_environment_with_fallback(
            "X".to_string(),
            "ok".to_string(),
            "long".to_string(),
        ),
    };
    let mut output = Vec::new();
    let error = load_error(|out| {
        string_binding.write(&invalid_string, &mut output, WriteOptions::default(), out)
    })?;
    assert_eq!(error.stage(), TypolStage::Encoding);
    assert!(output.is_empty());

    let optional_leaf_schema = load(
        "entry struct ManualOptionalLeafOrigin { value: option<u16> @range(10, 20) @env(\"VALUE\") }",
        default_options(),
    )?;
    let optional_leaf_binding =
        load_value(|out| optional_leaf_schema.bind::<ManualOptionalLeafOrigin>(out))?;
    let invalid_optional_leaf = ManualOptionalLeafOrigin {
        value: Some(Sourced::from_environment_with_fallback(
            "VALUE".to_string(),
            12,
            0,
        )),
    };
    let mut output = Vec::new();
    let error = load_error(|out| {
        optional_leaf_binding.write(
            &invalid_optional_leaf,
            &mut output,
            WriteOptions::default(),
            out,
        )
    })?;
    assert_eq!(error.stage(), TypolStage::Encoding);
    assert!(output.is_empty());

    let optional_schema = load(
        "entry struct ManualOptionalOrigin { value: option<u16> @env(\"VALUE\") }",
        default_options(),
    )?;
    let optional_binding = load_value(|out| optional_schema.bind::<ManualOptionalOrigin>(out))?;
    let invalid_optional = ManualOptionalOrigin {
        value: Sourced::from_environment("VALUE".to_string(), None),
    };
    let mut output = Vec::new();
    let error = load_error(|out| {
        optional_binding.write(&invalid_optional, &mut output, WriteOptions::default(), out)
    })?;
    assert_eq!(error.stage(), TypolStage::Encoding);
    assert!(output.is_empty());
    Ok(())
}

#[test]
fn environment_callback_can_reenter_loaded_schema_without_losing_outer_error() -> TestResult {
    let loaded = load(
        "entry struct Root { value: u8 @env(\"VALUE\") }",
        default_options(),
    )?;
    let mut environment = ReentrantEnvironment {
        loaded: &loaded,
        nested_succeeded: false,
    };
    let errors = load_error(|out| {
        loaded.read_value(
            ReadRequest::from_text("{value: env(\"VALUE\")}"),
            &mut environment,
            out,
        )
    })?;
    assert!(environment.nested_succeeded);
    assert_eq!(errors.stage(), TypolStage::Decoding);
    assert_eq!(
        errors.config_errors().test_value()?.stage(),
        ConfigFailureStage::Parsing
    );
    Ok(())
}

#[test]
fn reader_callback_can_reenter_loaded_schema_without_losing_outer_error() -> TestResult {
    let loaded = load("entry struct Root { value: u8 }", default_options())?;
    let mut reader = ReentrantReader {
        loaded: &loaded,
        reentered: false,
    };
    let errors =
        load_error(|out| loaded.read_io(&mut reader, "config", &mut Provider::default(), out))?;
    assert!(reader.reentered);
    assert_eq!(errors.stage(), TypolStage::ConfigRead);
    assert_eq!(
        errors.host_error().test_value()?.kind(),
        HostErrorKind::ReadFailure
    );
    Ok(())
}

#[test]
fn schema_resolver_can_reenter_loader_without_losing_outer_schema_error() -> TestResult {
    let loader = SchemaLoader::new(default_options());
    let mut resolver = ReentrantResolver {
        loader: &loader,
        nested_succeeded: false,
        child_source: SliceSource::new(b"?"),
    };
    let errors = load_error(|out| {
        loader.compile_with_resolver(
            SchemaSource::from_str(b"root", "extern mod child; entry struct Root {}"),
            &mut resolver,
            out,
        )
    })?;
    assert!(resolver.nested_succeeded);
    assert_eq!(errors.stage(), TypolStage::SchemaCompile);
    assert!(errors.schema_errors().is_some());
    Ok(())
}

#[test]
fn binding_and_config_read_fail_before_environment_access() -> TestResult {
    let loaded = load(
        "entry struct Root { value: u8, optional: option<string> }",
        default_options(),
    )?;
    let mut provider = Provider {
        deny: true,
        ..Provider::default()
    };
    let missing = Path::new("/missing/secret-path.tconf");
    let errors = load_error(|mut out| {
        loaded
            .bind::<Different>(out.reborrow())?
            .read_file(missing, &mut provider, out)
    })?;
    assert_eq!(errors.stage(), TypolStage::Binding);
    assert_eq!(
        errors.codec_errors().test_value()?.stage(),
        CodecStage::Binding
    );
    assert_eq!(
        errors
            .diagnostics()
            .first()
            .test_expect("diagnostic list should not be empty")?
            .code(),
        DiagnosticCode::TYPE_BINDING_ERROR
    );
    assert!(errors.host_error().is_none());
    for errors in [
        load_error(|mut out| {
            loaded
                .bind::<Root>(out.reborrow())?
                .read_file(missing, &mut provider, out)
        })?,
        load_error(|out| loaded.read_file_value(missing, &mut provider, out))?,
    ] {
        assert_eq!(errors.stage(), TypolStage::ConfigRead);
        assert_eq!(
            errors.host_error().test_value()?.kind(),
            HostErrorKind::NotFound
        );
        assert!(errors.diagnostics().is_empty());
        assert_eq!(errors.source_name(SourceId::new(0)), Some("schema#0"));
        assert!(
            errors
                .source()
                .and_then(|source| source.downcast_ref::<HostError>())
                .is_some()
        );
        assert!(!format!("{errors:?}{errors}").contains("secret path"));
    }
    assert_eq!(provider.calls, 0);
    Ok(())
}

#[test]
fn failed_name_snapshot_does_not_block_success_and_preserves_original_errors() -> TestResult {
    let loaded = load(
        "entry struct Root { value: u8, optional: option<string> }",
        default_options().with_name_edits(SourceNameEditOptions::new(
            SourceNameLimits::new(0, 0),
            ExecutionLimits::new(16_777_216, 16_777_216),
        )),
    )?;
    let missing = Path::new("/missing.tconf");
    let mut provider = Provider {
        deny: true,
        ..Provider::default()
    };
    let errors = load_error(|mut out| {
        loaded
            .bind::<Different>(out.reborrow())?
            .read_file(missing, &mut provider, out)
    })?;
    assert_eq!(errors.stage(), TypolStage::Binding);
    assert_eq!(
        errors.names_failure(),
        Some(HostErrorKind::ResourceLimitExceeded)
    );
    assert_eq!(
        errors
            .diagnostics()
            .first()
            .test_expect("diagnostic list should not be empty")?
            .code(),
        DiagnosticCode::TYPE_BINDING_ERROR
    );
    assert!(errors.source().is_some());
    let encoded = load_value(|mut out| {
        let bound = loaded.bind::<Root>(out.reborrow())?;
        let value = bound.read_with_process_environment(
            ReadRequest::from_text("{value: 3, optional: none}"),
            out.reborrow(),
        )?;
        bound.write_to_string(&value, WriteOptions::default(), out)
    })?;
    assert!(!encoded.is_empty());
    let errors = load_error(|mut out| {
        loaded
            .bind::<Root>(out.reborrow())?
            .read_with_process_environment(
                ReadRequest::from_text("{value: 300, optional: none}"),
                out,
            )
    })?;
    assert_eq!(errors.stage(), TypolStage::Decoding);
    assert!(errors.codec_errors().is_some());
    assert_eq!(
        errors.names_failure(),
        Some(HostErrorKind::ResourceLimitExceeded)
    );
    let errors = load_error(|mut out| {
        loaded
            .bind::<Root>(out.reborrow())?
            .read_file(missing, &mut provider, out)
    })?;
    assert_eq!(errors.stage(), TypolStage::ConfigRead);
    assert_eq!(
        errors.host_error().test_value()?.kind(),
        HostErrorKind::NotFound
    );
    assert_eq!(
        errors.names_failure(),
        Some(HostErrorKind::ResourceLimitExceeded)
    );
    Ok(())
}

#[test]
fn schema_and_config_errors_keep_names_and_locations_after_inputs_are_dropped() -> TestResult {
    let directory = Directory::new()?;
    let schema = directory.put(
        "root.tschem",
        "extern mod rules; entry struct Root { value: rules::Small, optional: option<string> }",
    )?;
    directory.put("rules.tschem", "type Small = u8 @range(2, 9);")?;
    let config = directory.put("root.tconf", "{value: 1}")?;
    let loader = SchemaLoader::new(TypolOptions::default());
    let mut loaded = load_value(|out| loader.compile_file(&schema, out))?;
    load_value(|out| loaded.set_source_name(SourceId::new(1), "rule\x1b\n", out))?;
    let dynamic = load_error(|out| loaded.read_file_with_process_environment(&config, out))?;
    let typed = load_error(|mut out| {
        loaded
            .bind::<Root>(out.reborrow())?
            .read_file_with_process_environment(&config, out)
    })?;
    let config_id = dynamic
        .config_errors()
        .test_value()?
        .config_source_id()
        .test_value()?;
    assert_eq!(dynamic.source_name(config_id), Some("config"));
    assert_eq!(dynamic.source_name(SourceId::new(1)), Some("rule\x1b\n"));
    assert_eq!(
        typed.codec_errors().test_value()?.stage(),
        CodecStage::Parsing
    );
    assert_eq!(
        typed
            .diagnostics()
            .first()
            .test_expect("diagnostic list should not be empty")?
            .code(),
        dynamic
            .diagnostics()
            .first()
            .test_expect("diagnostic list should not be empty")?
            .code()
    );
    drop(loaded);
    directory.cleanup()?;
    for errors in [dynamic, typed] {
        for diagnostic in errors.diagnostics().iter() {
            assert!(
                errors
                    .span_location(diagnostic.primary().test_value()?)
                    .is_some()
            );
            for label in diagnostic.labels() {
                assert!(errors.span_location(label.span()).is_some());
            }
        }
        let text = TestOpeo::value(|out| {
            errors.render(
                Translator::new(Language::English),
                RenderOptions::default(),
                out,
            )
        })?;
        assert!(text.contains("rule\\u{1b}\\n"));
        assert!(!text.contains('\x1b'));
        assert_eq!(errors.stage(), TypolStage::Decoding);
    }
    Ok(())
}

#[test]
fn named_file_inputs_keep_display_names_for_dynamic_and_typed_reads() -> TestResult {
    let directory = Directory::new()?;
    let config = directory.put("actual-secret-path.tconf", "{value: 1, optional: none}")?;
    let loaded = load(
        "entry struct Root { value: u8 @range(2, 9), optional: option<string> }",
        default_options(),
    )?;
    let display_name = "config\x1b\n";
    let dynamic = load_error(|out| {
        loaded.read_file_named_with_process_environment(&config, display_name, out)
    })?;
    let config_source_id = dynamic
        .config_errors()
        .test_value()?
        .config_source_id()
        .test_value()?;
    assert_file_display_name(&dynamic, config_source_id, display_name, &config)?;

    let typed = load_error(|mut out| {
        loaded
            .bind::<Root>(out.reborrow())?
            .read_file_named_with_process_environment(&config, display_name, out)
    })?;
    assert_file_display_name(&typed, config_source_id, display_name, &config)?;

    let default_name = load_error(|out| loaded.read_file_with_process_environment(&config, out))?;
    let default_source_id = default_name
        .config_errors()
        .test_value()?
        .config_source_id()
        .test_value()?;
    assert_eq!(default_name.source_name(default_source_id), Some("config"));
    let default_rendered = TestOpeo::value(|out| {
        default_name.render(
            Translator::new(Language::English),
            RenderOptions::default(),
            out,
        )
    })?;
    assert!(
        !format!("{default_name}{default_rendered}").contains(config.to_string_lossy().as_ref())
    );

    directory.cleanup()?;
    Ok(())
}

#[test]
fn root_and_compile_failures_preserve_finite_classification() -> TestResult {
    let directory = Directory::new()?;
    let loader = SchemaLoader::new(TypolOptions::default());
    let missing = directory.path.join("missing.tschem");
    let errors = load_error(|out| loader.compile_file(&missing, out))?;
    assert_eq!(errors.stage(), TypolStage::SchemaRead);
    assert!(errors.diagnostics().is_empty());
    let file_errors = errors
        .source()
        .and_then(|source| source.downcast_ref::<FileSchemaCompileErrors>());
    assert!(file_errors.is_some());
    assert!(
        file_errors
            .and_then(|error| error.source())
            .and_then(|source| source.downcast_ref::<HostError>())
            .is_some()
    );
    let schema = directory.put("invalid.tschem", "entry struct Root { value: Missing }")?;
    let errors = load_error(|out| loader.compile_file(&schema, out))?;
    assert_eq!(errors.stage(), TypolStage::SchemaCompile);
    assert!(errors.schema_errors().is_some());
    assert_eq!(errors.source_name(SourceId::new(0)), Some("schema#0"));
    assert!(errors.to_string().contains("SCHEMA_SEMANTIC_ERROR"));
    let file_errors = errors
        .source()
        .and_then(|source| source.downcast_ref::<FileSchemaCompileErrors>());
    assert!(
        file_errors
            .and_then(|error| error.source())
            .and_then(|source| source.downcast_ref::<SchemaCompileErrors>())
            .is_some()
    );
    let memory_errors = load_error(|out| {
        loader.compile_text(b"memory", "entry struct Root { value: Missing }", out)
    })?;
    assert_eq!(memory_errors.stage(), TypolStage::SchemaCompile);
    assert_eq!(
        memory_errors.source_name(SourceId::new(0)),
        Some("schema#0")
    );
    directory.cleanup()?;
    for errors in [&errors, &memory_errors] {
        let rendered = TestOpeo::value(|out| {
            errors.render(
                Translator::new(Language::English),
                RenderOptions::default(),
                out,
            )
        })?;
        let displayed = errors.to_string();
        assert!(rendered.contains("Location: schema#0:"));
        assert!(displayed.contains("Location: schema#0:"));
    }
    Ok(())
}

#[test]
fn typed_loading_keeps_parser_and_conversion_on_one_budget() -> TestResult {
    let text = "entry struct Root { value: u8, optional: option<string> }";
    let loaded = load(text, default_options())?;
    let directory = Directory::new()?;
    let config = directory.put("root.tconf", "{value: 3}")?;
    let defaults = ConfigParseOptions::default();
    let budget = budget_value(|out| ExecutionBudget::new(*defaults.execution(), out))?;
    let mut input = host_value(|out| {
        FileConfigInput::open(
            &config,
            loaded.options().file_input_limits(),
            defaults.config().max_source_bytes(),
            &budget,
            out,
        )
    })?;
    parse_value(|out| {
        ConfigParser::new(loaded.schema()).parse_with_budget(
            ConfigParseRequest::new(&mut input, "config", defaults),
            &mut Provider::default(),
            &budget,
            out,
        )
    })?;
    let limited = load(
        text,
        default_options().with_config(ConfigParseOptions::new(
            *defaults.excerpts(),
            *defaults.regex(),
            *defaults.config(),
            ExecutionLimits::new(budget.steps(), defaults.execution().max_workspace_bytes()),
        )),
    )?;
    load_value(|out| limited.read_file_with_process_environment(&config, out))?;
    let errors = load_error(|mut out| {
        limited
            .bind::<Root>(out.reborrow())?
            .read_file_with_process_environment(&config, out)
    })?;
    assert_eq!(errors.stage(), TypolStage::Decoding);
    assert_eq!(
        errors.codec_errors().test_value()?.stage(),
        CodecStage::Conversion
    );
    assert_eq!(
        errors
            .diagnostics()
            .first()
            .test_expect("diagnostic list should not be empty")?
            .code(),
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
    );
    Ok(())
}

#[test]
fn observer_failure_retains_schema_error_and_name_failure() -> TestResult {
    let options = default_options().with_names(SourceNameLimits::new(0, 4096));
    let loader = SchemaLoader::new(options);
    let errors = load_error(|out| {
        loader.compile_with_resolver(
            SchemaSource::from_str(b"root", "entry struct Root {}"),
            &mut NoImports,
            out,
        )
    })?;
    assert_eq!(errors.stage(), TypolStage::SchemaCompile);
    assert_eq!(
        errors.names_failure(),
        Some(HostErrorKind::ResourceLimitExceeded)
    );
    assert_eq!(
        errors
            .diagnostics()
            .first()
            .test_expect("diagnostic list should not be empty")?
            .code(),
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
    );
    Ok(())
}

#[test]
fn file_and_memory_compile_failures_retain_name_limits_without_copying() -> TestResult {
    let directory = Directory::new()?;
    let path = directory.put(
        "root.tschem",
        "extern mod child; extern mod later; entry struct Root {}",
    )?;
    directory.put("child.tschem", "?")?;
    let options = default_options()
        .with_names(SourceNameLimits::new(1, 4096))
        .with_name_edits(SourceNameEditOptions::new(
            SourceNameLimits::new(0, 0),
            ExecutionLimits::new(0, 0),
        ));
    let loader = SchemaLoader::new(options);
    let file_errors = load_error(|out| loader.compile_file(&path, out))?;
    let memory_loader =
        SchemaLoader::new(default_options().with_names(SourceNameLimits::new(0, 4096)));
    let memory_errors =
        load_error(|out| memory_loader.compile_text(b"root", "entry struct Root {}", out))?;
    directory.cleanup()?;
    assert_eq!(file_errors.stage(), TypolStage::SchemaCompile);
    assert_eq!(file_errors.source_name(SourceId::new(0)), Some("schema#0"));
    assert_eq!(file_errors.source_name(SourceId::new(1)), None);
    assert_eq!(
        file_errors.names_failure(),
        Some(HostErrorKind::ResourceLimitExceeded)
    );
    assert_eq!(file_errors.diagnostics().len(), 1);
    assert_eq!(memory_errors.stage(), TypolStage::SchemaCompile);
    assert_eq!(
        memory_errors.names_failure(),
        Some(HostErrorKind::ResourceLimitExceeded)
    );
    for errors in [&file_errors, &memory_errors] {
        assert_eq!(
            errors
                .diagnostics()
                .first()
                .test_expect("diagnostic list should not be empty")?
                .code(),
            DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
        );
        TestOpeo::value(|out| {
            errors.render(
                Translator::new(Language::English),
                RenderOptions::default(),
                out,
            )
        })?;
    }
    Ok(())
}

#[test]
fn error_storage_reservation_is_independent_of_name_edit_limits() -> TestResult {
    let loaded = load(
        "entry struct Root { value: u8, optional: option<string> }",
        default_options().with_name_edits(SourceNameEditOptions::new(
            SourceNameLimits::default(),
            ExecutionLimits::new(0, 0),
        )),
    )?;
    let value = load_value(|mut out| {
        loaded
            .bind::<Root>(out.reborrow())?
            .read_with_process_environment(
                ReadRequest::from_text("{value: 3, optional: none}"),
                out,
            )
    })?;
    assert_eq!(value.value, 3);
    let errors = load_error(|mut out| {
        loaded
            .bind::<Root>(out.reborrow())?
            .read_file_with_process_environment(Path::new("/missing.tconf"), out)
    })?;
    assert_eq!(errors.stage(), TypolStage::ConfigRead);
    assert_eq!(
        errors.host_error().test_value()?.kind(),
        HostErrorKind::NotFound
    );
    assert_eq!(
        errors.names_failure(),
        Some(HostErrorKind::ResourceLimitExceeded)
    );
    let mut loaded = load(
        "entry struct Root { value: u8, optional: option<string> }",
        default_options(),
    )?;
    load_value(|out| loaded.set_source_name(SourceId::new(0), "root", out))?;
    assert!(load_error(|out| loaded.set_source_name(SourceId::new(1), "unknown", out)).is_ok());
    assert_eq!(loaded.source_names().name(SourceId::new(0)), Some("root"));
    assert_eq!(
        loaded.source_names().identity(SourceId::new(0)),
        Some(b"root".as_slice())
    );
    let mut text = "root".repeat(4097);
    assert!(load_error(|out| loaded.set_source_name(SourceId::new(0), &text, out)).is_ok());
    text.clear();
    assert_eq!(loaded.source_names().name(SourceId::new(0)), Some("root"));
    Ok(())
}

#[test]
fn memory_compile_rejects_foreign_sessions_and_preserves_error_on_render_failure() -> TestResult {
    let foreign = schema_value(|out| SchemaCompiler::new(SchemaCompileOptions::default(), out))?;
    let foreign_context = budget_value(|out| foreign.context(out))?;
    let loader = SchemaLoader::new(TypolOptions::default());
    let errors = load_error(|out| {
        loader.compile_with_resolver(
            SchemaSource::in_context(b"root", "entry struct Root {}", &foreign_context),
            &mut NoImports,
            out,
        )
    })?;
    assert_eq!(
        errors
            .diagnostics()
            .first()
            .test_expect("diagnostic list should not be empty")?
            .code(),
        DiagnosticCode::SCHEMA_CONTEXT_MISMATCH
    );
    assert_eq!(
        errors
            .schema_errors()
            .test_expect("schema compilation errors should be preserved")?
            .context_failure(),
        Some(SchemaCompileContextFailure::RootContextMismatch)
    );
    assert!(
        TestOpeo::call(|out| {
            errors.render(
                Translator::new(Language::English),
                RenderOptions::new(ExecutionLimits::new(0, 0), 0),
                out,
            )
        })
        .is_err()
    );
    let rendered = TestOpeo::value(|out| {
        errors.render(
            Translator::new(Language::English),
            RenderOptions::new(ExecutionLimits::new(4096, 4096), 4096),
            out,
        )
    })?;
    assert!(rendered.contains("root schema source is bound to a different compilation context"));
    assert_eq!(
        errors
            .diagnostics()
            .first()
            .test_expect("diagnostic list should not be empty")?
            .code(),
        DiagnosticCode::SCHEMA_CONTEXT_MISMATCH
    );
    let primary = errors
        .diagnostics()
        .first()
        .test_expect("diagnostic list should not be empty")?
        .primary()
        .test_expect(
            "the actual root source span should be preserved when source contexts mismatch",
        )?;
    assert_eq!(primary.source_id(), SourceId::new(0));
    assert_eq!(primary.span().start(), 0);
    assert_eq!(primary.span().end(), "entry struct Root {}".len());
    Ok(())
}

struct SecretProvider;

impl Environment for SecretProvider {
    fn read<'slot, 'borrow, 'env>(
        &'env mut self,
        _: &str,
        _: EnvironmentReadContext<'_>,
        _: Out<'slot, 'borrow, EnvironmentError>,
    ) -> OResult<'slot, Option<&'env [u8]>> {
        Ok(Some(b"loading-secret-sentinel"))
    }
}

#[test]
fn invalid_secret_environment_never_enters_safe_error_outputs() -> TestResult {
    let directory = Directory::new()?;
    let config = directory.put("secret.tconf", "{value: env(\"VALUE\")}")?;
    let loaded = load(
        "entry struct Root { value: u8 @secret @env(\"VALUE\"), optional: option<string> }",
        default_options(),
    )?;
    for errors in [
        load_error(|out| loaded.read_file_value(&config, &mut SecretProvider, out))?,
        load_error(|mut out| {
            loaded
                .bind::<Root>(out.reborrow())?
                .read_file(&config, &mut SecretProvider, out)
        })?,
    ] {
        let mut output = format!(
            "{errors:?}{errors}{}",
            TestOpeo::value(|out| {
                errors.render(
                    Translator::new(Language::English),
                    RenderOptions::default(),
                    out,
                )
            })?
        );
        if let Some(source) = errors.source() {
            output.push_str(&format!("{source:?}{source}"));
        }
        assert!(!output.contains("loading-secret-sentinel"));
        assert_eq!(errors.stage(), TypolStage::Decoding);
        assert_eq!(
            errors
                .diagnostics()
                .first()
                .test_expect("diagnostic list should not be empty")?
                .code(),
            DiagnosticCode::CONFIG_VALUE_ERROR
        );
    }
    Ok(())
}

struct Reader {
    bytes: &'static [u8],
    offset: usize,
    attempts: usize,
    failure: Option<ErrorKind>,
}

impl Reader {
    fn new(bytes: &'static [u8], failure: Option<ErrorKind>) -> Self {
        Self {
            bytes,
            offset: 0,
            attempts: 0,
            failure,
        }
    }
}

impl Read for Reader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        self.attempts += 1;
        if self.offset == self.bytes.len() {
            return self.failure.map_or(Ok(0), |kind| {
                Err(io::Error::new(kind, "reader-secret-sentinel"))
            });
        }
        if self.attempts <= 2 {
            return Err(io::Error::from(ErrorKind::Interrupted));
        }
        let count = output.len().min(1).min(self.bytes.len() - self.offset);
        let next_offset = self
            .offset
            .checked_add(count)
            .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "test read position overflow"))?;
        let source = self.bytes.get(self.offset..next_offset).ok_or_else(|| {
            io::Error::new(ErrorKind::UnexpectedEof, "test input range is invalid")
        })?;
        let destination = output.get_mut(..count).ok_or_else(|| {
            io::Error::new(ErrorKind::WriteZero, "test buffer capacity is insufficient")
        })?;
        destination.copy_from_slice(source);
        self.offset = next_offset;
        Ok(count)
    }
}

#[test]
fn reader_paths_preserve_short_reads_and_map_read_failures() -> TestResult {
    let loaded = load(
        "entry struct Root { value: u8, optional: option<string> }",
        default_options(),
    )?;
    let bytes = b"{value: 3, optional: none}";
    let mut provider = Provider {
        deny: true,
        ..Provider::default()
    };
    let mut reader = Reader::new(bytes, None);
    load_value(|out| loaded.read_io(&mut reader, "reader", &mut provider, out))?;
    let mut reader = Reader::new(bytes, None);
    assert_eq!(
        load_value(|mut out| loaded.bind::<Root>(out.reborrow())?.read_io(
            &mut reader,
            "reader",
            &mut provider,
            out
        ))?,
        Root {
            value: 3,
            optional: None
        }
    );
    for kind in [ErrorKind::Other, ErrorKind::WouldBlock] {
        let mut reader = Reader::new(bytes, Some(kind));
        let errors = load_error(|out| loaded.read_io(&mut reader, "reader", &mut provider, out))?;
        assert_eq!(errors.stage(), TypolStage::ConfigRead);
        let host_error = errors.host_error().test_value()?;
        assert_eq!(host_error.kind(), HostErrorKind::ReadFailure);
        assert_eq!(host_error.operation(), HostOperation::ConfigRead);
        assert!(host_error.source_id().is_some());
        assert_eq!(host_error.offset(), Some(bytes.len()));
        assert_eq!(
            host_error
                .source_context()
                .test_expect("reader errors should preserve read context")?
                .failure_point(),
            "host-reader-call"
        );
        assert!(
            !format!(
                "{errors:?}{errors}{}",
                TestOpeo::value(|out| {
                    errors.render(
                        Translator::new(Language::English),
                        RenderOptions::default(),
                        out,
                    )
                })?
            )
            .contains("reader-secret-sentinel")
        );
        let mut reader = Reader::new(bytes, Some(kind));
        let errors = load_error(|mut out| {
            loaded
                .bind::<Root>(out.reborrow())?
                .read_io(&mut reader, "reader", &mut provider, out)
        })?;
        assert_eq!(errors.stage(), TypolStage::ConfigRead);
        assert_eq!(
            errors.host_error().test_value()?.kind(),
            HostErrorKind::ReadFailure
        );
    }
    Ok(())
}

struct OverreportingReader;

impl Read for OverreportingReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        Ok(output.len() + 1)
    }
}

#[test]
fn reader_invariant_errors_keep_their_context_and_offset() -> TestResult {
    let loaded = load(
        "entry struct Root { value: u8, optional: option<string> }",
        default_options(),
    )?;
    let mut reader = OverreportingReader;
    let errors =
        load_error(|out| loaded.read_io_with_process_environment(&mut reader, "reader", out))?;
    let host_error = errors.host_error().test_value()?;
    assert_eq!(errors.stage(), TypolStage::ConfigRead);
    assert_eq!(host_error.kind(), HostErrorKind::InternalError);
    assert_eq!(host_error.offset(), Some(0));
    assert!(errors.to_string().contains("read-count-exceeds-request"));
    assert!(!errors.to_string().contains("reader-budget-clone-failure"));
    assert_eq!(
        host_error
            .source_context()
            .test_expect("read-count invariant context should be preserved")?
            .failure_point(),
        "read-count-exceeds-request"
    );
    Ok(())
}

#[test]
fn binding_failure_does_not_touch_the_reader() -> TestResult {
    let loaded = load(
        "entry struct Root { value: u8, optional: option<string> }",
        default_options(),
    )?;
    let mut reader = Reader::new(b"", Some(ErrorKind::Other));
    let errors = load_error(|mut out| {
        loaded
            .bind::<Different>(out.reborrow())?
            .read_io_with_process_environment(&mut reader, "secret display name", out)
    })?;
    assert_eq!(errors.stage(), TypolStage::Binding);
    assert_eq!(reader.attempts, 0);
    Ok(())
}

#[test]
fn reader_budget_and_parse_limits_keep_distinct_error_stages() -> TestResult {
    let defaults = ConfigParseOptions::default();
    let loaded = load(
        "entry struct Root { value: u8, optional: option<string> }",
        default_options().with_config(ConfigParseOptions::new(
            *defaults.excerpts(),
            *defaults.regex(),
            *defaults.config(),
            ExecutionLimits::new(10_000, defaults.execution().max_workspace_bytes()),
        )),
    )?;
    let mut interrupted = Reader::new(b"", Some(ErrorKind::Interrupted));
    let input_error =
        load_error(|out| loaded.read_io_with_process_environment(&mut interrupted, "reader", out))?;
    assert_eq!(input_error.stage(), TypolStage::ConfigRead);
    assert_eq!(
        input_error.host_error().test_value()?.kind(),
        HostErrorKind::ResourceLimitExceeded
    );
    assert!(interrupted.attempts < 10_000);
    let mut config = *defaults.config();
    config = config.with_max_value_nodes(0);
    let options = default_options().with_config(ConfigParseOptions::new(
        *defaults.excerpts(),
        *defaults.regex(),
        config,
        *defaults.execution(),
    ));
    let loaded = load(
        "entry struct Root { value: u8, optional: option<string> }",
        options,
    )?;
    let mut reader = Reader::new(b"{value: 3}", None);
    let parse_error =
        load_error(|out| loaded.read_io_with_process_environment(&mut reader, "reader", out))?;
    assert_eq!(parse_error.stage(), TypolStage::Decoding);
    assert_eq!(
        parse_error.config_errors().test_value()?.stage(),
        ConfigFailureStage::Parsing
    );
    assert_eq!(
        parse_error
            .diagnostics()
            .first()
            .test_expect("diagnostic list should not be empty")?
            .code(),
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
    );
    Ok(())
}

#[test]
fn reader_source_limit_maps_to_host_resource_error() -> TestResult {
    let defaults = ConfigParseOptions::default();
    let mut config = *defaults.config();
    config = config.with_max_source_bytes(2);
    let loaded = load(
        "entry struct Root { value: u8 = 3, optional: option<string> }",
        default_options().with_config(ConfigParseOptions::new(
            *defaults.excerpts(),
            *defaults.regex(),
            config,
            *defaults.execution(),
        )),
    )?;
    let mut reader = Reader::new(b"{} ", None);
    let errors =
        load_error(|out| loaded.read_io_with_process_environment(&mut reader, "reader", out))?;
    assert_eq!(errors.stage(), TypolStage::ConfigRead);
    assert_eq!(
        errors.host_error().test_value()?.kind(),
        HostErrorKind::ResourceLimitExceeded
    );
    Ok(())
}
