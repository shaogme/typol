use std::error::Error;
use typol_core::SliceSource;
use typol_test_support::{
    TestExpect, TestExtract, TestOpeo, TestResult, test_verify as assert,
    test_verify_eq as assert_eq,
};

use typol::{
    ConfigParseOptions, Environment, EnvironmentError, EnvironmentReadContext, Typol,
    binding::{BindOptions, CodecOptions, CodecStage, ConversionLimits, TypedSchema},
    config::{ConfigParseErrors, ConfigParseRequest},
    host::{DiagnosticContext, DiagnosticRenderer, Language, RenderOptions, Translator},
    opeo::{OResult, Out},
    schema::{
        ResolverError, SchemaCompileOptions, SchemaCompiler, SchemaResolveRequest, SchemaResolver,
        SchemaSource,
    },
};
use typol_config::ConfigValueErrors;

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

#[derive(Debug, Typol)]
struct Root {
    value: String,
}

#[test]
fn codec_parser_errors_render_owned_two_source_snapshots_after_binding_drop() -> TestResult {
    let errors = {
        let compiler =
            TestOpeo::value(|out| SchemaCompiler::new(SchemaCompileOptions::default(), out))?;
        let schema = TestOpeo::value(|out| {
            compiler.compile(
                SchemaSource::from_str(
                    b"root",
                    "entry struct Root { value: string @length(8, 32) @secret }",
                ),
                &mut NoImports,
                None,
                out,
            )
        })?;
        let binding =
            TestOpeo::value(|out| TypedSchema::<Root>::bind(&schema, BindOptions::default(), out))?;
        TestOpeo::error(|out| {
            binding.deserialize(
                ConfigParseRequest::new(
                    &mut SliceSource::new(("{value: \"private\"}").as_bytes()),
                    "config\u{1b}",
                    ConfigParseOptions::default(),
                ),
                &mut NoEnvironment,
                ConversionLimits::default(),
                out,
            )
        })?
    };
    assert_eq!(errors.stage(), CodecStage::Parsing);
    let config = errors.config_parse_errors().test_value()?;
    let text = TestOpeo::call(|out| {
        DiagnosticRenderer::render(
            errors.diagnostics(),
            DiagnosticContext {
                source_name: &|id| {
                    if Some(id) == config.config_source_id() {
                        config.config_source_name()
                    } else {
                        Some("schema")
                    }
                },
                span_location: &|span| errors.span_location(span),
                excerpts: errors.excerpts(),
            },
            RenderOptions::default(),
            Translator::new(Language::English),
            out,
        )
    })
    .test_value()?;
    assert!(text.contains("CONFIG_VALIDATION_ERROR (12): value is hidden"));
    assert!(text.contains("Location: config\\u{1b}:1:"));
    assert!(text.contains("Rule: schema:1:"));
    assert!(!text.contains("private"));
    assert!(
        errors
            .source()
            .and_then(|source| source.downcast_ref::<ConfigParseErrors>())
            .is_some()
    );
    let parse_errors = errors.source().test_value()?;
    let parse_report = parse_errors.to_string();
    assert!(parse_report.contains("CONFIG_VALIDATION_ERROR (12)"));
    assert!(parse_report.contains("<redacted>"));
    assert!(!parse_report.contains("private"));
    assert!(!format!("{errors:?} {errors}").contains("private"));
    Ok(())
}

#[test]
fn rust_value_diagnostics_keep_schema_labels_without_fabricated_primary() -> TestResult {
    let errors = {
        let compiler =
            TestOpeo::value(|out| SchemaCompiler::new(SchemaCompileOptions::default(), out))?;
        let schema = TestOpeo::value(|out| {
            compiler.compile(
                SchemaSource::from_str(
                    b"root",
                    "entry struct Root { value: string @length(1, 3) @secret }",
                ),
                &mut NoImports,
                None,
                out,
            )
        })?;
        let binding =
            TestOpeo::value(|out| TypedSchema::<Root>::bind(&schema, BindOptions::default(), out))?;
        TestOpeo::error(|out| {
            binding.serialize(
                &Root {
                    value: "PRIVATE-RUST-SENTINEL".into(),
                },
                CodecOptions::default(),
                out,
            )
        })?
    };
    assert_eq!(errors.stage(), CodecStage::Validation);
    let text = TestOpeo::call(|out| {
        DiagnosticRenderer::render(
            errors.diagnostics(),
            DiagnosticContext {
                source_name: &|_| Some("schema"),
                span_location: &|span| errors.span_location(span),
                excerpts: errors.excerpts(),
            },
            RenderOptions::default(),
            Translator::new(Language::English),
            out,
        )
    })
    .test_value()?;
    assert!(text.contains("Source: RustValue"));
    assert!(text.contains("Rule: schema:"));
    assert!(!text.contains("Location:"));
    assert!(!text.contains("PRIVATE"));
    assert!(
        errors
            .diagnostics()
            .first()
            .test_expect("validation errors should include diagnostics")?
            .primary()
            .is_none()
    );
    assert!(
        errors
            .source()
            .and_then(|source| source.downcast_ref::<ConfigValueErrors>())
            .is_some()
    );
    let validation_errors = errors.source().test_value()?;
    assert!(
        validation_errors
            .to_string()
            .contains("CONFIG_VALIDATION_ERROR (12)")
    );
    assert!(
        validation_errors
            .to_string()
            .contains("Location: location unavailable")
    );
    Ok(())
}
