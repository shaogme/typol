mod json;

use std::{
    boxed::Box,
    error::Error,
    fmt,
    fs::File,
    io::{self, Read as IoRead, Write},
    path::Path,
    process::ExitCode,
};
use typol::{
    ConfigDocument, ConfigFormatOptions, ConfigFormatter, ConfigIr, ConfigIrErrorKind,
    ConfigIrOptions, EmptyEnvironment, LoadedSchema, ProcessEnvironment, ProcessEnvironmentLimits,
    SchemaFormatError, SchemaFormatOptions, SchemaFormatter, SchemaLoader, SchemaParseOptions,
    SchemaParser, SourceExpectation, SourceWriter, SyntaxErrors, TypolError, TypolOptions,
    ValidatedConfigValue,
    core::{
        DiagnosticCode, DiagnosticFailure, ExecutionBudget, ExecutionLimits, SourceId,
        SourceLocation, SourceSpan,
    },
    host::{
        DiagnosticContext, DiagnosticRenderError, DiagnosticRenderer, DiagnosticReport,
        DiagnosticReportStage, DiagnosticReportTitle, FileConfigInputLimits, FileSourceWriter,
        RenderOptions,
    },
    opeo::ErrSlot,
    semantic::SpanLocation,
};
use typol_i18n::{Translator, t};
use u18n::RenderError;

use crate::{
    arguments::{Command, ConfigRequest, EnvironmentMode, OutputFormat, SchemaFormatRequest},
    commands::json::{DiagnosticJson, SchemaExcerptCapture, SyntaxJsonRequest},
};

#[derive(Debug)]
pub enum CommandError {
    Translation(RenderError),
    Diagnostic(DiagnosticFailure),
}

impl From<RenderError> for CommandError {
    fn from(error: RenderError) -> Self {
        Self::Translation(error)
    }
}

impl From<DiagnosticFailure> for CommandError {
    fn from(error: DiagnosticFailure) -> Self {
        Self::Diagnostic(error)
    }
}

impl fmt::Display for CommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Translation(error) => fmt::Display::fmt(error, formatter),
            Self::Diagnostic(error) => fmt::Display::fmt(error, formatter),
        }
    }
}

impl Error for CommandError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Translation(error) => Some(error),
            Self::Diagnostic(error) => Some(error),
        }
    }
}

type LoadResult<T> = Result<T, Box<TypolError>>;
type TranslationResult<T> = Result<T, CommandError>;
type LocalizedText = Result<&'static str, RenderError>;

struct SchemaSyntaxFailure<'a> {
    errors: &'a SyntaxErrors,
    path: &'a Path,
    source: &'a str,
    budget: &'a ExecutionBudget,
    translator: Translator,
    output_format: OutputFormat,
    detailed_diagnostics: bool,
}

pub struct Commands;

impl Commands {
    pub fn help(
        translator: Translator,
        output_format: OutputFormat,
    ) -> TranslationResult<ExitCode> {
        let help = t! { name: "cli.help", locale: translator }?;
        let mut stdout = io::stdout().lock();
        if stdout.write_all(help.as_bytes()).is_ok() {
            Ok(ExitCode::SUCCESS)
        } else {
            Self::operation_failure(
                t! { name: "cli.write_stdout_error", locale: translator },
                output_format,
            )
        }
    }

    pub fn usage_error(message: &str, translator: Translator) -> TranslationResult<ExitCode> {
        let help = t! { name: "cli.help", locale: translator }?;
        let prefix = t! { name: "cli.error_prefix", locale: translator }?;
        let mut stderr = io::stderr().lock();
        let write_result = stderr
            .write_all(prefix.as_bytes())
            .and_then(|()| stderr.write_all(message.as_bytes()))
            .and_then(|()| stderr.write_all(b"\n\n"))
            .and_then(|()| stderr.write_all(help.as_bytes()));
        Ok(Self::result_code(2, write_result.is_ok()))
    }

    pub fn run(
        command: &Command,
        translator: Translator,
        output_format: OutputFormat,
        detailed_diagnostics: bool,
    ) -> TranslationResult<ExitCode> {
        match command {
            Command::Help => Self::help(translator, output_format),
            Command::SchemaCheck(path) => {
                Self::schema_check(path, translator, output_format, detailed_diagnostics)
            }
            Command::SchemaFmt(request) => {
                Self::schema_fmt(request, translator, output_format, detailed_diagnostics)
            }
            Command::ConfigCheck(request) => {
                Self::config_check(request, translator, output_format, detailed_diagnostics)
            }
            Command::ConfigExportIr(request) => {
                Self::config_export_ir(request, translator, output_format, detailed_diagnostics)
            }
            Command::ConfigFmt(request) => {
                Self::config_fmt(request, translator, output_format, detailed_diagnostics)
            }
        }
    }

    fn write_ir(
        bytes: &[u8],
        translator: Translator,
        output_format: OutputFormat,
    ) -> TranslationResult<ExitCode> {
        let mut stdout = io::stdout().lock();
        if stdout.write_all(bytes).is_ok() {
            Ok(ExitCode::SUCCESS)
        } else {
            Self::operation_failure(
                t! { name: "cli.write_stdout_error", locale: translator },
                output_format,
            )
        }
    }

    fn load_failure(
        errors: &TypolError,
        translator: Translator,
        output_format: OutputFormat,
        detailed_diagnostics: bool,
    ) -> TranslationResult<ExitCode> {
        let status = Self::load_status(errors);
        if output_format == OutputFormat::Json {
            let written = DiagnosticJson::write_diagnostics(errors, detailed_diagnostics)?;
            return Ok(Self::result_code(status, written));
        }
        let prefix = t! { name: "cli.diagnostic_prefix", locale: translator }?;
        let mut render_errors = ErrSlot::<DiagnosticRenderError>::new();
        let rendered = match render_errors.call(|out| errors.render(RenderOptions::default(), out))
        {
            Ok(rendered) => rendered,
            Err(DiagnosticRenderError::Translation(error)) => return Err(error.into()),
            Err(DiagnosticRenderError::Diagnostic(error)) => return Err(error.into()),
            Err(DiagnosticRenderError::Display(_)) => {
                return Ok(ExitCode::from(3));
            }
        };
        let mut stderr = io::stderr().lock();
        let write_result = stderr
            .write_all(prefix.as_bytes())
            .and_then(|()| stderr.write_all(rendered.as_bytes()))
            .and_then(|()| stderr.write_all(b"\n"));
        Ok(Self::result_code(status, write_result.is_ok()))
    }

    fn operation_failure(
        message: LocalizedText,
        output_format: OutputFormat,
    ) -> TranslationResult<ExitCode> {
        Self::report_failure(message, 3, output_format)
    }

    fn report_failure(
        message: LocalizedText,
        status: u8,
        output_format: OutputFormat,
    ) -> TranslationResult<ExitCode> {
        let message = message?;
        let written = match output_format {
            OutputFormat::Text => Self::write_stderr(message),
            OutputFormat::Json => DiagnosticJson::write_operation_failure(message),
        };
        Ok(Self::result_code(status, written))
    }

    fn load_status(errors: &TypolError) -> u8 {
        if errors.host_error().is_some() { 3 } else { 1 }
    }

    fn result_code(status: u8, output_written: bool) -> ExitCode {
        if output_written {
            ExitCode::from(status)
        } else {
            ExitCode::from(3)
        }
    }

    fn write_stderr(message: &str) -> bool {
        let mut stderr = io::stderr().lock();
        if stderr.write_all(message.as_bytes()).is_err() {
            return false;
        }
        message.ends_with('\n') || stderr.write_all(b"\n").is_ok()
    }

    fn schema_check(
        path: &Path,
        translator: Translator,
        output_format: OutputFormat,
        detailed_diagnostics: bool,
    ) -> TranslationResult<ExitCode> {
        match Self::load_schema(path, translator) {
            Ok(_) => Ok(ExitCode::SUCCESS),
            Err(errors) => {
                Self::load_failure(&errors, translator, output_format, detailed_diagnostics)
            }
        }
    }

    fn schema_fmt(
        request: &SchemaFormatRequest,
        translator: Translator,
        output_format: OutputFormat,
        detailed_diagnostics: bool,
    ) -> TranslationResult<ExitCode> {
        let options = SchemaParseOptions::default().with_translator(translator);
        let max_bytes = options.syntax().max_source_bytes();
        let source = match read_schema_source(request.schema_path(), max_bytes) {
            Ok(source) => source,
            Err(()) => {
                return Self::operation_failure(
                    t! { name: "cli.schema_format_error", locale: translator },
                    output_format,
                );
            }
        };
        let mut budget_errors = ErrSlot::new();
        let budget = match budget_errors
            .try_call(|out| ExecutionBudget::new(ExecutionLimits::new(16_777_216, 8_388_608), out))
        {
            Ok(budget) => budget,
            Err(_) => {
                return Self::operation_failure(
                    t! { name: "cli.schema_format_error", locale: translator },
                    output_format,
                );
            }
        };
        let mut syntax_errors = ErrSlot::<SyntaxErrors>::new();
        let document = match syntax_errors.try_call(|out| {
            SchemaParser::parse_document(SourceId::new(0), &source, &options, &budget, out)
        }) {
            Ok(document) => document,
            Err(caught) => {
                let errors = caught.take();
                let terminal = errors.diagnostics().any(|diagnostic| {
                    let code = diagnostic.code();
                    code.is_resource_limit()
                        || code == DiagnosticCode::ALLOCATION_FAILED
                        || code.is_capacity_exceeded()
                        || code.is_internal_error()
                });
                if terminal {
                    return Self::operation_failure(
                        t! { name: "cli.schema_format_error", locale: translator },
                        output_format,
                    );
                }
                return Self::schema_syntax_failure(SchemaSyntaxFailure {
                    errors: &errors,
                    path: request.schema_path(),
                    source: &source,
                    budget: &budget,
                    translator,
                    output_format,
                    detailed_diagnostics,
                });
            }
        };
        let mut format_errors = ErrSlot::<SchemaFormatError>::new();
        let formatted = match format_errors
            .try_call(|out| SchemaFormatter::format(document, SchemaFormatOptions::default(), out))
        {
            Ok(formatted) => formatted,
            Err(_) => {
                return Self::operation_failure(
                    t! { name: "cli.schema_format_error", locale: translator },
                    output_format,
                );
            }
        };
        if request.check() {
            if formatted.as_str() == source {
                return Ok(ExitCode::SUCCESS);
            }
            return Self::report_failure(
                t! { name: "cli.schema_format_check_failed", locale: translator },
                1,
                output_format,
            );
        }
        let limits = FileConfigInputLimits::default();
        let mut writer = FileSourceWriter::new(limits.max_path_bytes());
        match writer.replace_atomically(
            request.schema_path(),
            SourceExpectation::ReplaceAny,
            formatted.as_str().as_bytes(),
            max_bytes,
        ) {
            Ok(_) => Ok(ExitCode::SUCCESS),
            Err(_) => Self::operation_failure(
                t! { name: "cli.schema_format_error", locale: translator },
                output_format,
            ),
        }
    }

    fn schema_syntax_failure(failure: SchemaSyntaxFailure<'_>) -> TranslationResult<ExitCode> {
        let SchemaSyntaxFailure {
            errors,
            path,
            source,
            budget,
            translator,
            output_format,
            detailed_diagnostics,
        } = failure;
        if output_format == OutputFormat::Json {
            let written = DiagnosticJson::write_syntax_diagnostics(SyntaxJsonRequest {
                errors,
                source,
                budget,
                detailed_diagnostics,
            })?;
            return Ok(Self::result_code(1, written));
        }
        let source_label = path.to_str().unwrap_or("<schema>");
        let source_name = |source_id| (source_id == SourceId::new(0)).then_some(source_label);
        let span_location = |span: SourceSpan| {
            Some(SpanLocation {
                start: source_location(source, span.span().start())?,
                end: source_location(source, span.span().end())?,
            })
        };
        let excerpts = SchemaExcerptCapture::capture(errors, source, budget);
        let context = DiagnosticContext {
            source_name: &source_name,
            span_location: &span_location,
            excerpts: &excerpts,
        };
        let prefix = t! { name: "cli.diagnostic_prefix", locale: translator }?;
        let mut render_errors = ErrSlot::<DiagnosticRenderError>::new();
        let rendered = match render_errors.try_call(|out| {
            DiagnosticRenderer::render_report(
                DiagnosticReport {
                    title: DiagnosticReportTitle::OperationFailed,
                    stage: Some(DiagnosticReportStage::SchemaParsing),
                    diagnostics: errors.diagnostics(),
                    diagnostic_count: errors.diagnostics().len(),
                    context,
                    options: RenderOptions::default(),
                    translator,
                },
                out,
            )
        }) {
            Ok(rendered) => rendered,
            Err(caught) => match caught.take() {
                DiagnosticRenderError::Translation(error) => return Err(error.into()),
                DiagnosticRenderError::Diagnostic(error) => return Err(error.into()),
                DiagnosticRenderError::Display(_) => return Ok(ExitCode::from(3)),
            },
        };
        let mut stderr = io::stderr().lock();
        let written = stderr
            .write_all(prefix.as_bytes())
            .and_then(|()| stderr.write_all(rendered.as_bytes()))
            .and_then(|()| stderr.write_all(b"\n"))
            .is_ok();
        Ok(Self::result_code(1, written))
    }

    fn config_check(
        request: &ConfigRequest,
        translator: Translator,
        output_format: OutputFormat,
        detailed_diagnostics: bool,
    ) -> TranslationResult<ExitCode> {
        let loaded = match Self::load_schema(request.schema_path(), translator) {
            Ok(loaded) => loaded,
            Err(errors) => {
                return Self::load_failure(
                    &errors,
                    translator,
                    output_format,
                    detailed_diagnostics,
                );
            }
        };
        match Self::load_config(&loaded, request) {
            Ok(_) => Ok(ExitCode::SUCCESS),
            Err(errors) => {
                Self::load_failure(&errors, translator, output_format, detailed_diagnostics)
            }
        }
    }

    fn config_export_ir(
        request: &ConfigRequest,
        translator: Translator,
        output_format: OutputFormat,
        detailed_diagnostics: bool,
    ) -> TranslationResult<ExitCode> {
        let loaded = match Self::load_schema(request.schema_path(), translator) {
            Ok(loaded) => loaded,
            Err(errors) => {
                return Self::load_failure(
                    &errors,
                    translator,
                    output_format,
                    detailed_diagnostics,
                );
            }
        };
        let value = match Self::load_config(&loaded, request) {
            Ok(value) => value,
            Err(errors) => {
                return Self::load_failure(
                    &errors,
                    translator,
                    output_format,
                    detailed_diagnostics,
                );
            }
        };
        let bytes = match ConfigIr::serialize_validated(
            loaded.schema(),
            &value,
            ConfigIrOptions::default()
                .with_pretty(request.pretty())
                .with_reject_secrets(request.reject_secrets()),
        ) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == ConfigIrErrorKind::SecretValueRejected => {
                return Self::operation_failure(
                    t! { name: "cli.ir_secret_rejected", locale: translator },
                    output_format,
                );
            }
            Err(_) => {
                return Self::operation_failure(
                    t! { name: "cli.ir_error", locale: translator },
                    output_format,
                );
            }
        };
        Self::write_ir(&bytes, translator, output_format)
    }

    fn config_fmt(
        request: &ConfigRequest,
        translator: Translator,
        output_format: OutputFormat,
        detailed_diagnostics: bool,
    ) -> TranslationResult<ExitCode> {
        let loaded = match Self::load_schema(request.schema_path(), translator) {
            Ok(loaded) => loaded,
            Err(errors) => {
                return Self::load_failure(
                    &errors,
                    translator,
                    output_format,
                    detailed_diagnostics,
                );
            }
        };
        let document = match Self::load_document(&loaded, request) {
            Ok(document) => document,
            Err(errors) => {
                return Self::load_failure(
                    &errors,
                    translator,
                    output_format,
                    detailed_diagnostics,
                );
            }
        };
        if request.format_check() {
            let mut format_errors = ErrSlot::new();
            let formatted = match format_errors.try_call(|out| {
                ConfigFormatter::is_formatted(&document, ConfigFormatOptions::default(), out)
            }) {
                Ok(formatted) => formatted,
                Err(_) => {
                    return Self::operation_failure(
                        t! { name: "cli.format_error", locale: translator },
                        output_format,
                    );
                }
            };
            return if formatted {
                Ok(ExitCode::SUCCESS)
            } else {
                Self::report_failure(
                    t! { name: "cli.format_check_failed", locale: translator },
                    1,
                    output_format,
                )
            };
        }
        let mut format_errors = ErrSlot::new();
        let formatted = match format_errors
            .try_call(|out| ConfigFormatter::format(&document, ConfigFormatOptions::default(), out))
        {
            Ok(formatted) => formatted,
            Err(_) => {
                return Self::operation_failure(
                    t! { name: "cli.format_error", locale: translator },
                    output_format,
                );
            }
        };
        let limits = loaded.options().file_input_limits();
        let max_bytes = loaded
            .options()
            .config()
            .config()
            .max_source_bytes()
            .min(limits.max_file_bytes());
        let mut writer = FileSourceWriter::new(limits.max_path_bytes());
        match writer.replace_atomically(
            request.config_path(),
            SourceExpectation::ReplaceAny,
            formatted.as_str().as_bytes(),
            max_bytes,
        ) {
            Ok(_) => Ok(ExitCode::SUCCESS),
            Err(_) => Self::operation_failure(
                t! { name: "cli.format_error", locale: translator },
                output_format,
            ),
        }
    }

    fn load_schema(path: &Path, translator: Translator) -> LoadResult<LoadedSchema> {
        let loader = SchemaLoader::new(TypolOptions::default().with_translator(translator));
        let mut errors = ErrSlot::<TypolError>::new();
        match errors.try_call(|out| loader.compile_file(path, out)) {
            Ok(loaded) => Ok(loaded),
            Err(caught) => Err(Box::new(caught.take())),
        }
    }

    fn load_config(
        loaded: &LoadedSchema,
        request: &ConfigRequest,
    ) -> LoadResult<ValidatedConfigValue> {
        match request.environment() {
            EnvironmentMode::Inherit => Self::load_inherited(loaded, request),
            EnvironmentMode::Empty => Self::load_empty(loaded, request),
        }
    }

    fn load_document<'loaded>(
        loaded: &'loaded LoadedSchema,
        request: &ConfigRequest,
    ) -> LoadResult<ConfigDocument<'loaded>> {
        match request.environment() {
            EnvironmentMode::Inherit => {
                let mut environment = ProcessEnvironment::new(ProcessEnvironmentLimits::default());
                let mut errors = ErrSlot::<TypolError>::new();
                match errors
                    .try_call(|out| loaded.read_file(request.config_path(), &mut environment, out))
                {
                    Ok(document) => Ok(document),
                    Err(caught) => Err(Box::new(caught.take())),
                }
            }
            EnvironmentMode::Empty => {
                let mut environment = EmptyEnvironment;
                let mut errors = ErrSlot::<TypolError>::new();
                match errors
                    .try_call(|out| loaded.read_file(request.config_path(), &mut environment, out))
                {
                    Ok(document) => Ok(document),
                    Err(caught) => Err(Box::new(caught.take())),
                }
            }
        }
    }

    fn load_inherited(
        loaded: &LoadedSchema,
        request: &ConfigRequest,
    ) -> LoadResult<ValidatedConfigValue> {
        let mut environment = ProcessEnvironment::new(ProcessEnvironmentLimits::default());
        let mut errors = ErrSlot::<TypolError>::new();
        match errors
            .try_call(|out| loaded.read_file_value(request.config_path(), &mut environment, out))
        {
            Ok(value) => Ok(value),
            Err(caught) => Err(Box::new(caught.take())),
        }
    }

    fn load_empty(
        loaded: &LoadedSchema,
        request: &ConfigRequest,
    ) -> LoadResult<ValidatedConfigValue> {
        let mut environment = EmptyEnvironment;
        let mut errors = ErrSlot::<TypolError>::new();
        match errors
            .try_call(|out| loaded.read_file_value(request.config_path(), &mut environment, out))
        {
            Ok(value) => Ok(value),
            Err(caught) => Err(Box::new(caught.take())),
        }
    }
}

fn read_schema_source(path: &Path, max_bytes: usize) -> Result<String, ()> {
    let file = File::open(path).map_err(|_| ())?;
    let metadata = file.metadata().map_err(|_| ())?;
    let max_bytes_u64 = u64::try_from(max_bytes).map_err(|_| ())?;
    if !metadata.is_file() || metadata.len() > max_bytes_u64 {
        return Err(());
    }
    let capacity = max_bytes.checked_add(1).ok_or(())?;
    let limit = u64::try_from(capacity).map_err(|_| ())?;
    let mut bytes = Vec::new();
    let initial_capacity = usize::try_from(metadata.len()).map_err(|_| ())?;
    bytes.try_reserve_exact(initial_capacity).map_err(|_| ())?;
    file.take(limit).read_to_end(&mut bytes).map_err(|_| ())?;
    if bytes.len() > max_bytes {
        return Err(());
    }
    String::from_utf8(bytes).map_err(|_| ())
}

fn source_location(source: &str, offset: usize) -> Option<SourceLocation> {
    let prefix = source.get(..offset)?;
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let line_start = prefix.rfind('\n').map_or(0, |index| index + 1);
    let column = prefix.get(line_start..)?.chars().count() + 1;
    SourceLocation::new(line, column)
}
