#[path = "support/values.rs"]
mod value_support;

use value_support::ValueSupport;

use std::{
    env::{current_exe, temp_dir, var_os},
    error::Error,
    fs::{create_dir, remove_dir_all, write},
    path::{Path, PathBuf},
    process::{Command, id},
};
use typol::{
    BoundSchema, Environment, EnvironmentError, EnvironmentReadContext, LoadedSchema,
    ProcessEnvironment, ProcessEnvironmentLimits, SchemaLoader, Typol, TypolError, TypolOptions,
    TypolStage, ValidatedConfigValue,
    binding::{
        BindOptions, BudgetedTypol, CodecErrors, TypedSchema, Typol as TypolTrait, WriteOptions,
    },
    core::{DiagnosticCode, ValueSource},
    host::{
        DiagnosticContext, DiagnosticRenderError, DiagnosticRenderer, DisplayError, DisplayOptions,
        HostErrorKind, Language, RenderOptions, SafeValueFormatter, Translator,
    },
    opeo::{OResult, Out},
};
use typol_test_support::{
    TestExpect, TestExtract, TestOpeo, TestResult, test_verify as assert,
    test_verify_eq as assert_eq,
};

trait BoundProcessEnvironmentTestExt<T: TypolTrait> {
    fn read_with_process_environment<'slot>(
        &self,
        path: &Path,
        out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, T>;
}

impl<T: TypolTrait> BoundProcessEnvironmentTestExt<T> for BoundSchema<'_, T> {
    fn read_with_process_environment<'slot>(
        &self,
        path: &Path,
        out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, T> {
        let mut environment = ProcessEnvironment::new(ProcessEnvironmentLimits::default());
        self.read_file(path, &mut environment, out)
    }
}

trait LoadedProcessEnvironmentTestExt {
    fn read_value_with_process_environment<'slot>(
        &self,
        path: &Path,
        out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, ValidatedConfigValue>;
}

impl LoadedProcessEnvironmentTestExt for LoadedSchema {
    fn read_value_with_process_environment<'slot>(
        &self,
        path: &Path,
        out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, ValidatedConfigValue> {
        let mut environment = ProcessEnvironment::new(ProcessEnvironmentLimits::default());
        self.read_file(path, &mut environment, out)
    }
}

fn load_value<T>(
    operation: impl for<'slot> FnOnce(Out<'slot, 'slot, TypolError>) -> OResult<'slot, T>,
) -> TestResult<T> {
    TestOpeo::value(operation)
}

fn read_typed<'slot, T: BudgetedTypol>(
    schema: &LoadedSchema,
    path: &Path,
    mut out: Out<'slot, '_, TypolError>,
) -> OResult<'slot, T> {
    schema
        .bind::<T>(out.reborrow())?
        .read_with_process_environment(path, out)
}

fn read_typed_with_environment<'slot, T: BudgetedTypol>(
    schema: &LoadedSchema,
    path: &Path,
    environment: &mut dyn Environment,
    mut out: Out<'slot, '_, TypolError>,
) -> OResult<'slot, T> {
    schema
        .bind::<T>(out.reborrow())?
        .read_file(path, environment, out)
}

fn load_error<T>(
    operation: impl for<'slot> FnOnce(Out<'slot, 'slot, TypolError>) -> OResult<'slot, T>,
) -> TestResult<TypolError> {
    TestOpeo::error(operation)
}

fn codec_value<T>(
    operation: impl for<'slot> FnOnce(Out<'slot, 'slot, CodecErrors>) -> OResult<'slot, T>,
) -> TestResult<T> {
    TestOpeo::value(operation)
}

fn codec_error<T>(
    operation: impl for<'slot> FnOnce(Out<'slot, 'slot, CodecErrors>) -> OResult<'slot, T>,
) -> TestResult<CodecErrors> {
    TestOpeo::error(operation)
}

fn display_value<T>(
    operation: impl for<'slot> FnOnce(Out<'slot, 'slot, DisplayError>) -> OResult<'slot, T>,
) -> TestResult<T> {
    TestOpeo::value(operation)
}

const SENTINELS: [&str; 5] = [
    "HOST-LITERAL-9275",
    "HOST-ENVIRONMENT-3841",
    "HOST-FALLBACK-6183",
    "HOST-DEFAULT-5729",
    "HOST-RUST-8362",
];

struct Directory {
    path: PathBuf,
}

impl Directory {
    fn new() -> TestResult<Self> {
        let path = temp_dir().join(format!("typol-host-faults-{}", id()));
        create_dir(&path).test_value()?;
        Ok(Self { path })
    }

    fn put(&self, name: &str, text: impl AsRef<[u8]>) -> TestResult<PathBuf> {
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
    literal: String,
    environment: String,
    fallback: String,
    defaulted: String,
}

struct Denied;

impl Environment for Denied {
    fn read<'slot, 'borrow, 'env>(
        &'env mut self,
        _: &str,
        _: EnvironmentReadContext<'_>,
        out: Out<'slot, 'borrow, EnvironmentError>,
    ) -> OResult<'slot, Option<&'env [u8]>> {
        Err(out.fail(EnvironmentError::HostFailure))
    }
}

struct Fault(EnvironmentError);

impl Environment for Fault {
    fn read<'slot, 'borrow, 'env>(
        &'env mut self,
        _: &str,
        _: EnvironmentReadContext<'_>,
        out: Out<'slot, 'borrow, EnvironmentError>,
    ) -> OResult<'slot, Option<&'env [u8]>> {
        Err(out.fail(self.0))
    }
}

struct Bytes<'a>(&'a [u8]);

impl Environment for Bytes<'_> {
    fn read<'slot, 'borrow, 'env>(
        &'env mut self,
        _: &str,
        _: EnvironmentReadContext<'_>,
        _: Out<'slot, 'borrow, EnvironmentError>,
    ) -> OResult<'slot, Option<&'env [u8]>> {
        Ok(Some(self.0))
    }
}

fn safe_errors(errors: &TypolError) -> TestResult {
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
    for diagnostic in errors.diagnostics().iter() {
        output.push_str(&format!(
            "{diagnostic:?}{}{:?}{:?}",
            diagnostic.message(),
            diagnostic.path(),
            diagnostic.labels()
        ));
        for label in diagnostic.labels() {
            output.push_str(label.message());
        }
    }
    let mut source = errors.source();
    while let Some(error) = source {
        output.push_str(&format!("{error:?}{error}"));
        source = error.source();
    }
    for sentinel in SENTINELS {
        assert!(!output.contains(sentinel));
    }
    assert!(!output.contains('\x1b'));
    let codes: Vec<_> = errors.diagnostics().iter().map(|d| d.code()).collect();
    assert_eq!(
        TestOpeo::call(|out| {
            errors.render(
                Translator::new(Language::English),
                RenderOptions::new(RenderOptions::default().execution(), 0),
                out,
            )
        }),
        Err(DiagnosticRenderError::Display(
            DisplayError::ResourceLimitExceeded
        ))
    );
    assert_eq!(
        codes,
        errors
            .diagnostics()
            .iter()
            .map(|d| d.code())
            .collect::<Vec<_>>()
    );
    Ok(())
}

fn sentinel(index: usize) -> TestResult<&'static str> {
    SENTINELS
        .get(index)
        .copied()
        .test_expect("test sentinel index should be valid")
}

#[test]
fn real_host_secret_sources_and_faults_are_safe() -> TestResult {
    if var_os("TYPOL_HOST_FAULT_CHILD").is_none() {
        let status = Command::new(current_exe().test_value()?)
            .args([
                "--exact",
                "real_host_secret_sources_and_faults_are_safe",
                "--test-threads=1",
            ])
            .env("TYPOL_HOST_FAULT_CHILD", "1")
            .env("HOST_SECRET", sentinel(1)?)
            .env_remove("HOST_MISSING")
            .status()
            .test_value()?;
        assert!(status.success());
        return Ok(());
    }
    let directory = Directory::new()?;
    let schema_path = directory.put("root.tschem", format!(
        "extern mod rules; entry struct Root {{ literal: rules::Secret, environment: rules::Secret @env(\"HOST_SECRET\"), fallback: rules::Secret @env(\"HOST_MISSING\"), defaulted: rules::Secret = \"{}\" }}", sentinel(3)?
    ))?;
    let rules = directory.put(
        "rules.tschem",
        "type Secret = string @secret @length(1, 64);",
    )?;
    let config = directory.put("root.tconf", format!(
        "{{literal: \"{}\", environment: env(\"HOST_SECRET\"), fallback: env(\"HOST_MISSING\", \"{}\")}}", sentinel(0)?, sentinel(2)?
    ))?;
    let loader = SchemaLoader::new(TypolOptions::default());
    let loaded = load_value(|out| loader.compile_file(&schema_path, out))?;
    let value = load_value(|out| read_typed::<Root>(&loaded, &config, out))?;
    let dynamic = load_value(|out| loaded.read_value_with_process_environment(&config, out))?;
    let display = display_value(|out| {
        SafeValueFormatter::format(
            loaded.schema(),
            dynamic.as_config_value(),
            DisplayOptions::default(),
            out,
        )
    })?;
    assert_eq!(display.matches("********").count(), 4);
    for secret in SENTINELS {
        assert!(!display.contains(secret));
    }
    let binding =
        codec_value(|out| TypedSchema::<Root>::bind(loaded.schema(), BindOptions::default(), out))?;
    let encoded =
        codec_value(|out| binding.serialize_to_string(&value, WriteOptions::default(), out))?;
    for secret in SENTINELS.iter().take(4) {
        assert!(encoded.contains(secret));
    }
    let frozen = directory.put("frozen.tconf", &encoded)?;
    assert_eq!(
        value,
        load_value(|out| read_typed_with_environment::<Root>(&loaded, &frozen, &mut Denied, out,))?
    );
    let rust_sentinels = [sentinel(4)?, sentinel(1)?, sentinel(2)?, sentinel(3)?];
    let rust = TestOpeo::value(|out| {
        ValueSupport::structure(
            vec![
                ValueSupport::field(
                    "literal".into(),
                    ValueSupport::scalar(|builder, out| {
                        builder.add_string(rust_sentinels[0], ValueSource::RustValue, out)
                    }),
                ),
                ValueSupport::field(
                    "environment".into(),
                    ValueSupport::scalar(|builder, out| {
                        builder.add_string(rust_sentinels[1], ValueSource::RustValue, out)
                    }),
                ),
                ValueSupport::field(
                    "fallback".into(),
                    ValueSupport::scalar(|builder, out| {
                        builder.add_string(rust_sentinels[2], ValueSource::RustValue, out)
                    }),
                ),
                ValueSupport::field(
                    "defaulted".into(),
                    ValueSupport::scalar(|builder, out| {
                        builder.add_string(rust_sentinels[3], ValueSource::RustValue, out)
                    }),
                ),
            ],
            None,
            out,
        )
    })?;
    assert_eq!(
        display,
        display_value(|out| SafeValueFormatter::format(
            loaded.schema(),
            &rust,
            DisplayOptions::default(),
            out
        ))?
    );
    let mut invalid = value;
    invalid.literal = sentinel(4)?.repeat(10);
    let errors =
        codec_error(|out| binding.serialize_to_string(&invalid, WriteOptions::default(), out))?;
    assert!(!format!("{errors:?}{errors}{:?}", errors.diagnostics()).contains(sentinel(4)?));
    let rendered = TestOpeo::call(|out| {
        DiagnosticRenderer::render(
            errors.diagnostics(),
            DiagnosticContext {
                source_name: &|id| loaded.source_names().name(id),
                span_location: &|span| errors.span_location(span),
                excerpts: errors.excerpts(),
            },
            RenderOptions::default(),
            Translator::new(Language::English),
            out,
        )
    })
    .test_value()?;
    let mut all_outputs = format!("{rendered}{errors:?}{errors}{:?}", errors.diagnostics());
    for diagnostic in errors.diagnostics().iter() {
        all_outputs.push_str(diagnostic.message());
        for label in diagnostic.labels() {
            all_outputs.push_str(label.message());
        }
    }
    let mut source = errors.source();
    while let Some(error) = source {
        all_outputs.push_str(&format!("{error:?}{error}"));
        source = error.source();
    }
    for secret in SENTINELS {
        assert!(!all_outputs.contains(secret));
    }
    let invalid_env = sentinel(1)?.repeat(10);
    for (text, bytes) in [
        (
            "{literal: \"ok\", environment: env(\"HOST_SECRET\"), fallback: \"ok\"}".to_owned(),
            invalid_env.as_bytes(),
        ),
        (
            format!(
                "{{literal: \"ok\", environment: \"ok\", fallback: env(\"HOST_MISSING\", \"{}\")}}",
                sentinel(2)?.repeat(10)
            ),
            &[][..],
        ),
    ] {
        write(&config, text).test_value()?;
        let mut provider = Bytes(bytes);
        // A missing fallback must be reported as `None` by the real process provider.
        let errors = if bytes.is_empty() {
            load_error(|out| loaded.read_value_with_process_environment(&config, out))?
        } else {
            load_error(|out| loaded.read_file(&config, &mut provider, out))?
        };
        safe_errors(&errors)?;
    }
    for field in ["literal", "environment", "fallback", "defaulted"] {
        let other = if field == "literal" {
            ""
        } else {
            "literal: \"ok\", "
        };
        let text = format!("{{{other}{field}: \"{}\"}}", sentinel(0)?.repeat(10));
        write(&config, text).test_value()?;
        for errors in [
            load_error(|out| loaded.read_value_with_process_environment(&config, out))?,
            load_error(|out| read_typed::<Root>(&loaded, &config, out))?,
        ] {
            safe_errors(&errors)?;
            assert!(
                errors
                    .diagnostics()
                    .iter()
                    .any(|d| d.code() == DiagnosticCode::CONFIG_VALIDATION_ERROR)
            );
        }
    }
    write(
        &config,
        "{literal: \"ok\", environment: env(\"HOST_SECRET\"), fallback: \"ok\"}",
    )
    .test_value()?;
    for fault in [
        EnvironmentError::HostFailure,
        EnvironmentError::ResourceLimitExceeded,
        EnvironmentError::AllocationFailed,
    ] {
        for errors in [
            load_error(|out| loaded.read_file(&config, &mut Fault(fault), out))?,
            load_error(|out| {
                read_typed_with_environment::<Root>(&loaded, &config, &mut Fault(fault), out)
            })?,
        ] {
            safe_errors(&errors)?;
            assert_eq!(errors.stage(), TypolStage::Decoding);
        }
    }
    write(&config, [0xff]).test_value()?;
    let errors =
        load_error(|out| read_typed_with_environment::<Root>(&loaded, &config, &mut Denied, out))?;
    assert_eq!(errors.stage(), TypolStage::Decoding);
    assert!(errors.host_error().is_none());
    assert_eq!(
        errors
            .diagnostics()
            .first()
            .test_expect("decode errors should include diagnostics")?
            .code()
            .get(),
        3
    );
    safe_errors(&errors)?;

    write(&config, vec![b' '; 1_048_577]).test_value()?;
    let errors =
        load_error(|out| read_typed_with_environment::<Root>(&loaded, &config, &mut Denied, out))?;
    assert_eq!(errors.stage(), TypolStage::ConfigRead);
    assert_eq!(
        errors.host_error().test_value()?.kind(),
        HostErrorKind::ResourceLimitExceeded
    );
    safe_errors(&errors)?;
    let errors = load_error(|out| {
        read_typed_with_environment::<Root>(&loaded, &directory.path, &mut Denied, out)
    })?;
    assert_eq!(
        errors.host_error().test_value()?.kind(),
        HostErrorKind::NotRegularFile
    );
    safe_errors(&errors)?;
    drop(binding);
    drop(loaded);
    for bytes in [vec![0xff], vec![b' '; 1_048_577]] {
        write(&rules, bytes).test_value()?;
        let errors = load_error(|out| loader.compile_file(&schema_path, out))?;
        assert_eq!(errors.stage(), TypolStage::SchemaCompile);
        safe_errors(&errors)?;
    }
    let default_path = directory.put(
        "default.tschem",
        format!(
            "entry struct Root {{ value: string @secret @length(1, 64) = \"{}\" }}",
            sentinel(3)?.repeat(10)
        ),
    )?;
    let errors = load_error(|out| loader.compile_file(&default_path, out))?;
    assert_eq!(errors.stage(), TypolStage::SchemaCompile);
    safe_errors(&errors)?;
    write(&schema_path, [0xff]).test_value()?;
    let errors = load_error(|out| loader.compile_file(&schema_path, out))?;
    assert_eq!(errors.stage(), TypolStage::SchemaCompile);
    assert!(errors.host_error().is_none());
    assert_eq!(
        errors
            .diagnostics()
            .first()
            .test_expect("invalid UTF-8 in the schema should produce a lexical diagnostic")?
            .code(),
        DiagnosticCode::LEXICAL_ERROR
    );
    safe_errors(&errors)?;
    let errors =
        load_error(|out| loader.compile_file(Path::new("HOST-LITERAL-9275-missing.tschem"), out))?;
    assert_eq!(errors.stage(), TypolStage::SchemaRead);
    directory.cleanup()?;
    safe_errors(&errors)?;
    Ok(())
}
