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

pub struct ConfigRequest {
    schema_path: PathBuf,
    config_path: PathBuf,
    environment: EnvironmentMode,
    pretty: bool,
    reject_secrets: bool,
}

pub enum Command {
    Help,
    SchemaCheck(PathBuf),
    ConfigCheck(ConfigRequest),
    ConfigExportIr(ConfigRequest),
}

pub struct ParsedCommand {
    command: Command,
    language: Language,
}

pub struct ArgumentError {
    message: TranslationResult<&'static str>,
    language: Language,
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
}

impl ParsedCommand {
    pub fn command(&self) -> &Command {
        &self.command
    }

    pub const fn language(&self) -> Language {
        self.language
    }
}

impl ArgumentError {
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
        let (language, arguments) = Self::language_arguments(arguments, default_language)?;
        let command = Self::parse_command(arguments, language)?;
        Ok(ParsedCommand { command, language })
    }

    fn language_arguments(
        arguments: impl IntoIterator<Item = OsString>,
        default_language: Language,
    ) -> Result<(Language, Vec<OsString>), ArgumentError> {
        let translator = Translator::new(default_language);
        let arguments: Vec<OsString> = arguments.into_iter().collect();
        let first = arguments.first().and_then(|argument| argument.to_str());
        if first == Some("--language") {
            let Some(value) = arguments.get(1).and_then(|argument| argument.to_str()) else {
                return Err(Self::error(
                    t!("cli.language_missing", locale = translator),
                    default_language,
                ));
            };
            let Some(language) = parse_language_option(value) else {
                return Err(Self::error(
                    t!("cli.language_invalid", locale = translator),
                    default_language,
                ));
            };
            return Ok((language, arguments.into_iter().skip(2).collect()));
        }
        if let Some(value) = first.and_then(|value| value.strip_prefix("--language=")) {
            let Some(language) = parse_language_option(value) else {
                return Err(Self::error(
                    t!("cli.language_invalid", locale = translator),
                    default_language,
                ));
            };
            return Ok((language, arguments.into_iter().skip(1).collect()));
        }
        Ok((default_language, arguments))
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
        let command = command
            .to_str()
            .ok_or_else(|| Self::error(t!("cli.command_utf8", locale = translator), language))?;
        match command {
            "--help" | "-h" | "help" => Ok(Command::Help),
            "schema" => Self::parse_schema(arguments, language),
            "config" => Self::parse_configuration_arguments(arguments, language),
            _ => Err(Self::error(
                t!("cli.unknown_command", locale = translator),
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
                t!("cli.schema_action_missing", locale = translator),
                language,
            ));
        };
        let action = action.to_str().ok_or_else(|| {
            Self::error(t!("cli.schema_action_utf8", locale = translator), language)
        })?;
        if matches!(action, "--help" | "-h") {
            return Ok(Command::Help);
        }
        if action != "check" {
            return Err(Self::error(
                t!("cli.schema_action_invalid", locale = translator),
                language,
            ));
        }
        let Some(path) = arguments.next() else {
            return Err(Self::error(
                t!("cli.schema_path_missing", locale = translator),
                language,
            ));
        };
        if matches!(path.to_str(), Some("--help" | "-h")) {
            return Ok(Command::Help);
        }
        if arguments.next().is_some() {
            return Err(Self::error(
                t!("cli.schema_path_extra", locale = translator),
                language,
            ));
        }
        Ok(Command::SchemaCheck(PathBuf::from(path)))
    }

    fn parse_configuration_arguments(
        mut arguments: impl Iterator<Item = OsString>,
        language: Language,
    ) -> Result<Command, ArgumentError> {
        let translator = Translator::new(language);
        let Some(action) = arguments.next() else {
            return Err(Self::error(
                t!("cli.config_action_missing", locale = translator),
                language,
            ));
        };
        let action = action.to_str().ok_or_else(|| {
            Self::error(t!("cli.config_action_utf8", locale = translator), language)
        })?;
        if matches!(action, "--help" | "-h") {
            return Ok(Command::Help);
        }
        let export = match action {
            "check" => false,
            "export-ir" => true,
            _ => {
                return Err(Self::error(
                    t!("cli.config_action_invalid", locale = translator),
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
                    t!("cli.option_utf8", locale = translator),
                    language,
                ));
            };
            if !positional_only && option == "--schema" {
                if schema_path.is_some() {
                    return Err(Self::error(
                        t!("cli.schema_option_repeated", locale = translator),
                        language,
                    ));
                }
                let Some(value) = arguments.next() else {
                    return Err(Self::error(
                        t!("cli.schema_option_value_missing", locale = translator),
                        language,
                    ));
                };
                schema_path = Some(PathBuf::from(value));
            } else if !positional_only && option == "--environment" {
                if environment_seen {
                    return Err(Self::error(
                        t!("cli.environment_option_repeated", locale = translator),
                        language,
                    ));
                }
                let Some(value) = arguments.next() else {
                    return Err(Self::error(
                        t!("cli.environment_value_missing", locale = translator),
                        language,
                    ));
                };
                environment = match value.to_str() {
                    Some("inherit") => EnvironmentMode::Inherit,
                    Some("empty") => EnvironmentMode::Empty,
                    _ => {
                        return Err(Self::error(
                            t!("cli.environment_value_invalid", locale = translator),
                            language,
                        ));
                    }
                };
                environment_seen = true;
            } else if !positional_only && option == "--pretty" {
                if !export {
                    return Err(Self::error(
                        t!("cli.pretty_invalid", locale = translator),
                        language,
                    ));
                }
                if pretty {
                    return Err(Self::error(
                        t!("cli.pretty_repeated", locale = translator),
                        language,
                    ));
                }
                pretty = true;
            } else if !positional_only && option == "--reject-secrets" {
                if !export {
                    return Err(Self::error(
                        t!("cli.reject_secrets_invalid", locale = translator),
                        language,
                    ));
                }
                if reject_secrets {
                    return Err(Self::error(
                        t!("cli.reject_secrets_repeated", locale = translator),
                        language,
                    ));
                }
                reject_secrets = true;
            } else if !positional_only && matches!(option, "--help" | "-h") {
                return Ok(Command::Help);
            } else if !positional_only && option.starts_with('-') {
                return Err(Self::error(
                    t!("cli.option_invalid", locale = translator),
                    language,
                ));
            } else {
                Self::set_config_path(&mut config_path, argument, language)?;
            }
        }

        let schema_path = schema_path.ok_or_else(|| {
            Self::error(
                t!("cli.schema_option_missing", locale = translator),
                language,
            )
        })?;
        let config_path = config_path.ok_or_else(|| {
            Self::error(t!("cli.config_path_missing", locale = translator), language)
        })?;
        let request = ConfigRequest {
            schema_path,
            config_path,
            environment,
            pretty,
            reject_secrets,
        };
        if export {
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
                t!("cli.config_path_extra", locale = Translator::new(language)),
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
