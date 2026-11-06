use std::{
    boxed::Box,
    fs::File,
    io::{self, Read as IoRead, Write},
    path::Path,
    process::ExitCode,
};
use typol::{
    ConfigDocument, ConfigFormatOptions, ConfigFormatter, ConfigIr, ConfigIrErrorKind,
    ConfigIrOptions, ConfigIrValueMode, ConfigValue, EmptyEnvironment, LoadedSchema,
    ProcessEnvironment, ProcessEnvironmentLimits, SchemaFormatError, SchemaFormatOptions,
    SchemaFormatter, SchemaLoader, SchemaParseOptions, SchemaParser, SourceExpectation,
    SourceWriter, SyntaxErrors, TypolError, TypolOptions,
    core::{
        DiagnosticCode, ExcerptLimits, ExecutionBudget, ExecutionLimits, SourceCapture,
        SourceExcerptSet, SourceId, SourceLocation, SourceSpan,
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

use crate::arguments::{Command, ConfigRequest, EnvironmentMode, SchemaFormatRequest};

type LoadResult<T> = Result<T, Box<TypolError>>;
type TranslationResult<T> = Result<T, RenderError>;

pub struct Commands;

impl Commands {
    pub fn help(translator: Translator) -> TranslationResult<ExitCode> {
        let help = t!("cli.help", locale = translator)?;
        let mut stdout = io::stdout().lock();
        if stdout.write_all(help.as_bytes()).is_ok() {
            Ok(ExitCode::SUCCESS)
        } else {
            Self::operation_failure(t!("cli.write_stdout_error", locale = translator))
        }
    }

    pub fn usage_error(message: &str, translator: Translator) -> TranslationResult<ExitCode> {
        let help = t!("cli.help", locale = translator)?;
        let prefix = t!("cli.error_prefix", locale = translator)?;
        let mut stderr = io::stderr().lock();
        let write_result = stderr
            .write_all(prefix.as_bytes())
            .and_then(|()| stderr.write_all(message.as_bytes()))
            .and_then(|()| stderr.write_all(b"\n\n"))
            .and_then(|()| stderr.write_all(help.as_bytes()));
        Ok(Self::result_code(2, write_result.is_ok()))
    }

    pub fn run(command: &Command, translator: Translator) -> TranslationResult<ExitCode> {
        match command {
            Command::Help => Self::help(translator),
            Command::SchemaCheck(path) => Self::schema_check(path, translator),
            Command::SchemaFmt(request) => Self::schema_fmt(request, translator),
            Command::ConfigCheck(request) => Self::config_check(request, translator),
            Command::ConfigExportIr(request) => Self::config_export_ir(request, translator),
            Command::ConfigFmt(request) => Self::config_fmt(request, translator),
        }
    }

    fn write_ir(bytes: &[u8], translator: Translator) -> TranslationResult<ExitCode> {
        let mut stdout = io::stdout().lock();
        if stdout.write_all(bytes).is_ok() {
            Ok(ExitCode::SUCCESS)
        } else {
            Self::operation_failure(t!("cli.write_stdout_error", locale = translator))
        }
    }

    fn load_failure(errors: &TypolError, translator: Translator) -> TranslationResult<ExitCode> {
        let status = Self::load_status(errors);
        let prefix = t!("cli.diagnostic_prefix", locale = translator)?;
        let mut render_errors = ErrSlot::<DiagnosticRenderError>::new();
        let rendered = match render_errors
            .call(|out| errors.render(translator, RenderOptions::default(), out))
        {
            Ok(rendered) => rendered,
            Err(DiagnosticRenderError::Translation(_)) => {
                return Err(RenderError::MessageOutOfRange);
            }
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

    fn operation_failure(message: TranslationResult<&'static str>) -> TranslationResult<ExitCode> {
        Self::report_failure(message, 3)
    }

    fn report_failure(
        message: TranslationResult<&'static str>,
        status: u8,
    ) -> TranslationResult<ExitCode> {
        let message = message?;
        Ok(Self::result_code(status, Self::write_stderr(message)))
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

    fn schema_check(path: &Path, translator: Translator) -> TranslationResult<ExitCode> {
        match Self::load_schema(path) {
            Ok(_) => Ok(ExitCode::SUCCESS),
            Err(errors) => Self::load_failure(&errors, translator),
        }
    }

    fn schema_fmt(
        request: &SchemaFormatRequest,
        translator: Translator,
    ) -> TranslationResult<ExitCode> {
        let options = SchemaParseOptions::default();
        let max_bytes = options.syntax().max_source_bytes();
        let source = match read_schema_source(request.schema_path(), max_bytes) {
            Ok(source) => source,
            Err(()) => {
                return Self::operation_failure(t!("cli.schema_format_error", locale = translator));
            }
        };
        let mut budget_errors = ErrSlot::new();
        let budget = match budget_errors
            .try_call(|out| ExecutionBudget::new(ExecutionLimits::new(16_777_216, 8_388_608), out))
        {
            Ok(budget) => budget,
            Err(_) => {
                return Self::operation_failure(t!("cli.schema_format_error", locale = translator));
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
                    matches!(
                        diagnostic.code(),
                        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
                            | DiagnosticCode::ALLOCATION_FAILED
                            | DiagnosticCode::CAPACITY_EXCEEDED
                            | DiagnosticCode::INTERNAL_ERROR
                    )
                });
                if terminal {
                    return Self::operation_failure(t!(
                        "cli.schema_format_error",
                        locale = translator
                    ));
                }
                return Self::schema_syntax_failure(
                    &errors,
                    request.schema_path(),
                    &source,
                    &budget,
                    translator,
                );
            }
        };
        let mut format_errors = ErrSlot::<SchemaFormatError>::new();
        let formatted = match format_errors
            .try_call(|out| SchemaFormatter::format(document, SchemaFormatOptions::default(), out))
        {
            Ok(formatted) => formatted,
            Err(_) => {
                return Self::operation_failure(t!("cli.schema_format_error", locale = translator));
            }
        };
        if request.check() {
            if formatted.as_str() == source {
                return Ok(ExitCode::SUCCESS);
            }
            return Self::report_failure(
                t!("cli.schema_format_check_failed", locale = translator),
                1,
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
            Err(_) => Self::operation_failure(t!("cli.schema_format_error", locale = translator)),
        }
    }

    fn schema_syntax_failure(
        errors: &SyntaxErrors,
        path: &Path,
        source: &str,
        budget: &ExecutionBudget,
        translator: Translator,
    ) -> TranslationResult<ExitCode> {
        let source_label = path.to_str().unwrap_or("<schema>");
        let source_name = |source_id| (source_id == SourceId::new(0)).then_some(source_label);
        let span_location = |span: SourceSpan| {
            Some(SpanLocation {
                start: source_location(source, span.span().start())?,
                end: source_location(source, span.span().end())?,
            })
        };
        let excerpts = schema_excerpts(errors, source, budget);
        let context = DiagnosticContext {
            source_name: &source_name,
            span_location: &span_location,
            excerpts: &excerpts,
        };
        let prefix = t!("cli.diagnostic_prefix", locale = translator)?;
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
                DiagnosticRenderError::Translation(_) => {
                    return Err(RenderError::MessageOutOfRange);
                }
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
    ) -> TranslationResult<ExitCode> {
        let loaded = match Self::load_schema(request.schema_path()) {
            Ok(loaded) => loaded,
            Err(errors) => return Self::load_failure(&errors, translator),
        };
        match Self::load_config(&loaded, request) {
            Ok(_) => Ok(ExitCode::SUCCESS),
            Err(errors) => Self::load_failure(&errors, translator),
        }
    }

    fn config_export_ir(
        request: &ConfigRequest,
        translator: Translator,
    ) -> TranslationResult<ExitCode> {
        let loaded = match Self::load_schema(request.schema_path()) {
            Ok(loaded) => loaded,
            Err(errors) => return Self::load_failure(&errors, translator),
        };
        let value = match Self::load_config(&loaded, request) {
            Ok(value) => value,
            Err(errors) => return Self::load_failure(&errors, translator),
        };
        let bytes = match ConfigIr::serialize(
            loaded.schema(),
            value,
            ConfigIrOptions::default()
                .with_pretty(request.pretty())
                .with_reject_secrets(request.reject_secrets())
                .with_value_mode(ConfigIrValueMode::AsProvided),
        ) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == ConfigIrErrorKind::SecretValueRejected => {
                return Self::operation_failure(t!("cli.ir_secret_rejected", locale = translator));
            }
            Err(_) => {
                return Self::operation_failure(t!("cli.ir_error", locale = translator));
            }
        };
        Self::write_ir(&bytes, translator)
    }

    fn config_fmt(request: &ConfigRequest, translator: Translator) -> TranslationResult<ExitCode> {
        let loaded = match Self::load_schema(request.schema_path()) {
            Ok(loaded) => loaded,
            Err(errors) => return Self::load_failure(&errors, translator),
        };
        let document = match Self::load_document(&loaded, request) {
            Ok(document) => document,
            Err(errors) => return Self::load_failure(&errors, translator),
        };
        if request.format_check() {
            let mut format_errors = ErrSlot::new();
            let formatted = match format_errors.try_call(|out| {
                ConfigFormatter::is_formatted(&document, ConfigFormatOptions::default(), out)
            }) {
                Ok(formatted) => formatted,
                Err(_) => {
                    return Self::operation_failure(t!("cli.format_error", locale = translator));
                }
            };
            return if formatted {
                Ok(ExitCode::SUCCESS)
            } else {
                Self::report_failure(t!("cli.format_check_failed", locale = translator), 1)
            };
        }
        let mut format_errors = ErrSlot::new();
        let formatted = match format_errors
            .try_call(|out| ConfigFormatter::format(&document, ConfigFormatOptions::default(), out))
        {
            Ok(formatted) => formatted,
            Err(_) => return Self::operation_failure(t!("cli.format_error", locale = translator)),
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
            Err(_) => Self::operation_failure(t!("cli.format_error", locale = translator)),
        }
    }

    fn load_schema(path: &Path) -> LoadResult<LoadedSchema> {
        let loader = SchemaLoader::new(TypolOptions::default());
        let mut errors = ErrSlot::<TypolError>::new();
        match errors.try_call(|out| loader.compile_file(path, out)) {
            Ok(loaded) => Ok(loaded),
            Err(caught) => Err(Box::new(caught.take())),
        }
    }

    fn load_config(loaded: &LoadedSchema, request: &ConfigRequest) -> LoadResult<ConfigValue> {
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

    fn load_inherited(loaded: &LoadedSchema, request: &ConfigRequest) -> LoadResult<ConfigValue> {
        let mut environment = ProcessEnvironment::new(ProcessEnvironmentLimits::default());
        let mut errors = ErrSlot::<TypolError>::new();
        match errors
            .try_call(|out| loaded.read_file_value(request.config_path(), &mut environment, out))
        {
            Ok(value) => Ok(value.into_config_value()),
            Err(caught) => Err(Box::new(caught.take())),
        }
    }

    fn load_empty(loaded: &LoadedSchema, request: &ConfigRequest) -> LoadResult<ConfigValue> {
        let mut environment = EmptyEnvironment;
        let mut errors = ErrSlot::<TypolError>::new();
        match errors
            .try_call(|out| loaded.read_file_value(request.config_path(), &mut environment, out))
        {
            Ok(value) => Ok(value.into_config_value()),
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

fn schema_excerpts(
    errors: &SyntaxErrors,
    source: &str,
    budget: &ExecutionBudget,
) -> SourceExcerptSet {
    let source_id = SourceId::new(0);
    let limits = ExcerptLimits::default();
    let mut capture = SourceCapture::new(source_id, limits, Some(budget));
    for diagnostic in errors.diagnostics() {
        for located in diagnostic
            .located_primary()
            .into_iter()
            .chain(diagnostic.labels().iter().map(|label| label.located_span()))
        {
            let span = located.source_span();
            let location = source_location(source, span.span().start())
                .zip(source_location(source, span.span().end()))
                .map(|(start, end)| SpanLocation { start, end });
            capture.pin(span, location);
        }
    }
    // Syntax failure cannot prove secret-default spans, so excerpts publish only fully redacted source.
    let Some(secret) = capture.begin_secret(0) else {
        return SourceExcerptSet::unavailable();
    };
    capture.feed(source.as_bytes());
    capture.end_secret(secret, source.len());
    capture.end_input();
    let mut excerpts = SourceExcerptSet::new(limits, Some(budget));
    capture.finish_iter_with_locations(&mut excerpts, errors.diagnostics(), |located| {
        let span = located.source_span().span();
        source_location(source, span.start())
            .zip(source_location(source, span.end()))
            .map(|(start, end)| SpanLocation { start, end })
    });
    excerpts
}

fn source_location(source: &str, offset: usize) -> Option<SourceLocation> {
    let prefix = source.get(..offset)?;
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let line_start = prefix.rfind('\n').map_or(0, |index| index + 1);
    let column = prefix.get(line_start..)?.chars().count() + 1;
    SourceLocation::new(line, column)
}
