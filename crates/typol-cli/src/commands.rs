use std::{
    boxed::Box,
    io::{self, Write},
    path::Path,
    process::ExitCode,
};
use typol::{
    ConfigDocument, ConfigFormatOptions, ConfigFormatter, ConfigIr, ConfigIrErrorKind,
    ConfigIrOptions, ConfigIrValueMode, ConfigSourceExpectation, ConfigSourceWriter, ConfigValue,
    EmptyEnvironment, LoadedSchema, ProcessEnvironment, ProcessEnvironmentLimits, SchemaLoader,
    TypolError, TypolOptions,
    host::{DiagnosticRenderError, FileConfigSourceWriter, RenderOptions},
    opeo::ErrSlot,
};
use typol_i18n::{Translator, t};
use u18n::RenderError;

use crate::arguments::{Command, ConfigRequest, EnvironmentMode};

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
        let mut format_errors = ErrSlot::new();
        let formatted = match format_errors
            .try_call(|out| ConfigFormatter::format(&document, ConfigFormatOptions::default(), out))
        {
            Ok(formatted) => formatted,
            Err(_) => return Self::operation_failure(t!("cli.format_error", locale = translator)),
        };
        if request.format_check() {
            if formatted.as_str() == document.source() {
                return Ok(ExitCode::SUCCESS);
            }
            return Self::report_failure(t!("cli.format_check_failed", locale = translator), 1);
        }
        let limits = loaded.options().file_input_limits();
        let max_bytes = loaded
            .options()
            .config()
            .config()
            .max_source_bytes()
            .min(limits.max_file_bytes());
        let mut writer = FileConfigSourceWriter::new(limits.max_path_bytes());
        match writer.replace_atomically(
            request.config_path(),
            ConfigSourceExpectation::ReplaceAny,
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
