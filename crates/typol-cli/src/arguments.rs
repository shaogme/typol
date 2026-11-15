use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};
use typol_i18n::{Language, Translator, t};
use u18n::RenderError;

type TranslationResult<T> = Result<T, RenderError>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EnvironmentMode {
    Inherit,
    Empty,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum OutputFormat {
    #[default]
    Text,
    Json,
}

pub struct ConfigRequest {
    schema_path: PathBuf,
    config_path: PathBuf,
    environment: EnvironmentMode,
    pretty: bool,
    reject_secrets: bool,
    format_check: bool,
}

pub struct SchemaFormatRequest {
    schema_path: PathBuf,
    check: bool,
}

pub enum Command {
    Help,
    SchemaCheck(PathBuf),
    SchemaFmt(SchemaFormatRequest),
    ConfigCheck(ConfigRequest),
    ConfigExportIr(ConfigRequest),
    ConfigFmt(ConfigRequest),
}

pub struct ParsedCommand {
    command: Command,
    language: Language,
    output_format: OutputFormat,
    detailed_diagnostics: bool,
}

pub struct ArgumentError {
    message: TranslationResult<&'static str>,
    language: Language,
}

struct GlobalArguments {
    language: Language,
    output_format: OutputFormat,
    detailed_diagnostics: bool,
    arguments: Vec<OsString>,
}

pub struct Arguments;

fn parse_language_option(value: &str) -> Option<Language> {
    match value {
        "en" => Some(Language::English),
        "zh-CN" => Some(Language::Chinese),
        _ => None,
    }
}

impl ConfigRequest {
    pub fn schema_path(&self) -> &Path {
        &self.schema_path
    }

    pub fn config_path(&self) -> &Path {
        &self.config_path
    }

    pub const fn environment(&self) -> EnvironmentMode {
        self.environment
    }

    pub const fn pretty(&self) -> bool {
        self.pretty
    }

    pub const fn reject_secrets(&self) -> bool {
        self.reject_secrets
    }

    pub const fn format_check(&self) -> bool {
        self.format_check
    }
}

impl SchemaFormatRequest {
    pub fn schema_path(&self) -> &Path {
        &self.schema_path
    }

    pub const fn check(&self) -> bool {
        self.check
    }
}

impl ParsedCommand {
    pub fn command(&self) -> &Command {
        &self.command
    }

    pub const fn language(&self) -> Language {
        self.language
    }

    pub const fn output_format(&self) -> OutputFormat {
        self.output_format
    }

    pub const fn detailed_diagnostics(&self) -> bool {
        self.detailed_diagnostics
    }
}

impl ArgumentError {
    /// Returns the localized static usage message or its translation failure.
    pub const fn message(&self) -> &TranslationResult<&'static str> {
        &self.message
    }

    pub const fn language(&self) -> Language {
        self.language
    }
}

impl Arguments {
    pub fn parse(
        arguments: impl IntoIterator<Item = OsString>,
    ) -> Result<ParsedCommand, ArgumentError> {
        let default_language = Language::detect();
        let global = Self::global_arguments(arguments, default_language)?;
        let command = Self::parse_command(global.arguments, global.language)?;
        Ok(ParsedCommand {
            command,
            language: global.language,
            output_format: global.output_format,
            detailed_diagnostics: global.detailed_diagnostics,
        })
    }

    fn global_arguments(
        arguments: impl IntoIterator<Item = OsString>,
        default_language: Language,
    ) -> Result<GlobalArguments, ArgumentError> {
        let translator = Translator::new(default_language);
        let arguments: Vec<OsString> = arguments.into_iter().collect();
        let mut language = default_language;
        let mut output_format = OutputFormat::Text;
        let mut detailed_diagnostics = false;
        let mut index = 0;
        while let Some(argument) = arguments.get(index).and_then(|argument| argument.to_str()) {
            let option_value = if argument == "--language" || argument == "--format" {
                index += 1;
                let Some(value) = arguments.get(index).and_then(|argument| argument.to_str())
                else {
                    let message = if argument == "--language" {
                        t! { name: "cli.language_missing", locale: translator }
                    } else {
                        t! { name: "cli.format_missing", locale: translator }
                    };
                    return Err(Self::error(message, language));
                };
                Some((argument, value))
            } else if let Some(value) = argument.strip_prefix("--language=") {
                Some(("--language", value))
            } else if let Some(value) = argument.strip_prefix("--format=") {
                Some(("--format", value))
            } else if argument == "--diagnostic-detail" {
                if detailed_diagnostics {
                    return Err(Self::error(
                        t! {
                            name: "cli.diagnostic_detail_repeated",
                            locale: Translator::new(language),
                        },
                        language,
                    ));
                }
                detailed_diagnostics = true;
                index += 1;
                continue;
            } else {
                None
            };
            let Some((option, value)) = option_value else {
                break;
            };
            if option == "--language" {
                let Some(parsed) = parse_language_option(value) else {
                    return Err(Self::error(
                        t! { name: "cli.language_invalid", locale: Translator::new(language) },
                        language,
                    ));
                };
                language = parsed;
            } else {
                output_format = match value {
                    "text" => OutputFormat::Text,
                    "json" => OutputFormat::Json,
                    _ => {
                        return Err(Self::error(
                            t! { name: "cli.format_invalid", locale: Translator::new(language) },
                            language,
                        ));
                    }
                };
            }
            index += 1;
        }
        if detailed_diagnostics && output_format != OutputFormat::Json {
            return Err(Self::error(
                t! {
                    name: "cli.diagnostic_detail_requires_json",
                    locale: Translator::new(language),
                },
                language,
            ));
        }
        Ok(GlobalArguments {
            language,
            output_format,
            detailed_diagnostics,
            arguments: arguments.into_iter().skip(index).collect(),
        })
    }

    fn parse_command(
        arguments: impl IntoIterator<Item = OsString>,
        language: Language,
    ) -> Result<Command, ArgumentError> {
        let translator = Translator::new(language);
        let mut arguments = arguments.into_iter();
        let Some(command) = arguments.next() else {
            return Ok(Command::Help);
        };
        let command = command.to_str().ok_or_else(|| {
            Self::error(
                t! { name: "cli.command_utf8", locale: translator },
                language,
            )
        })?;
        match command {
            "--help" | "-h" | "help" => Ok(Command::Help),
            "schema" => Self::parse_schema(arguments, language),
            "config" => Self::parse_configuration_arguments(arguments, language),
            _ => Err(Self::error(
                t! { name: "cli.unknown_command", locale: translator },
                language,
            )),
        }
    }

    fn parse_schema(
        mut arguments: impl Iterator<Item = OsString>,
        language: Language,
    ) -> Result<Command, ArgumentError> {
        let translator = Translator::new(language);
        let Some(action) = arguments.next() else {
            return Err(Self::error(
                t! { name: "cli.schema_action_missing", locale: translator },
                language,
            ));
        };
        let action = action.to_str().ok_or_else(|| {
            Self::error(
                t! { name: "cli.schema_action_utf8", locale: translator },
                language,
            )
        })?;
        if matches!(action, "--help" | "-h") {
            return Ok(Command::Help);
        }
        if action == "fmt" {
            return Self::parse_schema_format(arguments, language);
        }
        if action != "check" {
            return Err(Self::error(
                t! { name: "cli.schema_action_invalid", locale: translator },
                language,
            ));
        }
        let Some(path) = arguments.next() else {
            return Err(Self::error(
                t! { name: "cli.schema_path_missing", locale: translator },
                language,
            ));
        };
        if matches!(path.to_str(), Some("--help" | "-h")) {
            return Ok(Command::Help);
        }
        if arguments.next().is_some() {
            return Err(Self::error(
                t! { name: "cli.schema_path_extra", locale: translator },
                language,
            ));
        }
        Ok(Command::SchemaCheck(PathBuf::from(path)))
    }

    fn parse_schema_format(
        arguments: impl Iterator<Item = OsString>,
        language: Language,
    ) -> Result<Command, ArgumentError> {
        let translator = Translator::new(language);
        let mut path = None;
        let mut check = false;
        let mut check_seen = false;
        let mut positional_only = false;
        for argument in arguments {
            if !positional_only && argument == "--" {
                positional_only = true;
                continue;
            }
            let Some(option) = argument.to_str() else {
                if positional_only || !argument.to_string_lossy().starts_with('-') {
                    Self::set_schema_format_path(&mut path, argument, language)?;
                    continue;
                }
                return Err(Self::error(
                    t! { name: "cli.option_utf8", locale: translator },
                    language,
                ));
            };
            if !positional_only && option == "--check" {
                if check_seen {
                    return Err(Self::error(
                        t! { name: "cli.format_check_repeated", locale: translator },
                        language,
                    ));
                }
                check = true;
                check_seen = true;
            } else if !positional_only && matches!(option, "--help" | "-h") {
                return Ok(Command::Help);
            } else if !positional_only && option.starts_with('-') {
                return Err(Self::error(
                    t! { name: "cli.schema_option_invalid", locale: translator },
                    language,
                ));
            } else {
                Self::set_schema_format_path(&mut path, argument, language)?;
            }
        }
        let schema_path = path.ok_or_else(|| {
            Self::error(
                t! { name: "cli.schema_path_missing", locale: translator },
                language,
            )
        })?;
        Ok(Command::SchemaFmt(SchemaFormatRequest {
            schema_path,
            check,
        }))
    }

    fn set_schema_format_path(
        path: &mut Option<PathBuf>,
        value: OsString,
        language: Language,
    ) -> Result<(), ArgumentError> {
        if path.is_some() {
            return Err(Self::error(
                t! { name: "cli.schema_path_extra", locale: Translator::new(language) },
                language,
            ));
        }
        *path = Some(PathBuf::from(value));
        Ok(())
    }

    fn parse_configuration_arguments(
        mut arguments: impl Iterator<Item = OsString>,
        language: Language,
    ) -> Result<Command, ArgumentError> {
        let translator = Translator::new(language);
        let Some(action) = arguments.next() else {
            return Err(Self::error(
                t! { name: "cli.config_action_missing", locale: translator },
                language,
            ));
        };
        let action = action.to_str().ok_or_else(|| {
            Self::error(
                t! { name: "cli.config_action_utf8", locale: translator },
                language,
            )
        })?;
        if matches!(action, "--help" | "-h") {
            return Ok(Command::Help);
        }
        let export = match action {
            "check" | "fmt" => false,
            "export-ir" => true,
            _ => {
                return Err(Self::error(
                    t! { name: "cli.config_action_invalid", locale: translator },
                    language,
                ));
            }
        };

        let mut schema_path = None;
        let mut config_path = None;
        let mut environment = EnvironmentMode::Inherit;
        let mut environment_seen = false;
        let mut pretty = false;
        let mut reject_secrets = false;
        let mut format_check = false;
        let mut format_check_seen = false;
        let mut positional_only = false;
        while let Some(argument) = arguments.next() {
            if !positional_only && argument == "--" {
                positional_only = true;
                continue;
            }
            let Some(option) = argument.to_str() else {
                if positional_only || !argument.to_string_lossy().starts_with('-') {
                    Self::set_config_path(&mut config_path, argument, language)?;
                    continue;
                }
                return Err(Self::error(
                    t! { name: "cli.option_utf8", locale: translator },
                    language,
                ));
            };
            if !positional_only && option == "--schema" {
                if schema_path.is_some() {
                    return Err(Self::error(
                        t! { name: "cli.schema_option_repeated", locale: translator },
                        language,
                    ));
                }
                let Some(value) = arguments.next() else {
                    return Err(Self::error(
                        t! { name: "cli.schema_option_value_missing", locale: translator },
                        language,
                    ));
                };
                schema_path = Some(PathBuf::from(value));
            } else if !positional_only && option == "--environment" {
                if environment_seen {
                    return Err(Self::error(
                        t! { name: "cli.environment_option_repeated", locale: translator },
                        language,
                    ));
                }
                let Some(value) = arguments.next() else {
                    return Err(Self::error(
                        t! { name: "cli.environment_value_missing", locale: translator },
                        language,
                    ));
                };
                environment = match value.to_str() {
                    Some("inherit") => EnvironmentMode::Inherit,
                    Some("empty") => EnvironmentMode::Empty,
                    _ => {
                        return Err(Self::error(
                            t! { name: "cli.environment_value_invalid", locale: translator },
                            language,
                        ));
                    }
                };
                environment_seen = true;
            } else if !positional_only && option == "--pretty" {
                if !export {
                    return Err(Self::error(
                        t! { name: "cli.pretty_invalid", locale: translator },
                        language,
                    ));
                }
                if pretty {
                    return Err(Self::error(
                        t! { name: "cli.pretty_repeated", locale: translator },
                        language,
                    ));
                }
                pretty = true;
            } else if !positional_only && option == "--check" {
                if action != "fmt" {
                    return Err(Self::error(
                        t! { name: "cli.format_check_invalid", locale: translator },
                        language,
                    ));
                }
                if format_check_seen {
                    return Err(Self::error(
                        t! { name: "cli.format_check_repeated", locale: translator },
                        language,
                    ));
                }
                format_check = true;
                format_check_seen = true;
            } else if !positional_only && option == "--reject-secrets" {
                if !export {
                    return Err(Self::error(
                        t! { name: "cli.reject_secrets_invalid", locale: translator },
                        language,
                    ));
                }
                if reject_secrets {
                    return Err(Self::error(
                        t! { name: "cli.reject_secrets_repeated", locale: translator },
                        language,
                    ));
                }
                reject_secrets = true;
            } else if !positional_only && matches!(option, "--help" | "-h") {
                return Ok(Command::Help);
            } else if !positional_only && option.starts_with('-') {
                return Err(Self::error(
                    t! { name: "cli.option_invalid", locale: translator },
                    language,
                ));
            } else {
                Self::set_config_path(&mut config_path, argument, language)?;
            }
        }

        let schema_path = schema_path.ok_or_else(|| {
            Self::error(
                t! { name: "cli.schema_option_missing", locale: translator },
                language,
            )
        })?;
        let config_path = config_path.ok_or_else(|| {
            Self::error(
                t! { name: "cli.config_path_missing", locale: translator },
                language,
            )
        })?;
        let request = ConfigRequest {
            schema_path,
            config_path,
            environment,
            pretty,
            reject_secrets,
            format_check,
        };
        if action == "fmt" {
            Ok(Command::ConfigFmt(request))
        } else if export {
            Ok(Command::ConfigExportIr(request))
        } else {
            Ok(Command::ConfigCheck(request))
        }
    }

    fn set_config_path(
        config_path: &mut Option<PathBuf>,
        path: OsString,
        language: Language,
    ) -> Result<(), ArgumentError> {
        if config_path.is_some() {
            return Err(Self::error(
                t! { name: "cli.config_path_extra", locale: Translator::new(language) },
                language,
            ));
        }
        *config_path = Some(PathBuf::from(path));
        Ok(())
    }

    fn error(message: TranslationResult<&'static str>, language: Language) -> ArgumentError {
        ArgumentError { message, language }
    }
}
