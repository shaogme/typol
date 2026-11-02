mod arguments;
mod commands;

use arguments::Arguments;
use commands::Commands;
use std::{env, process::ExitCode};
use typol_i18n::Translator;

fn main() -> ExitCode {
    match Arguments::parse(env::args_os().skip(1)) {
        Ok(parsed) => match Commands::run(parsed.command(), Translator::new(parsed.language())) {
            Ok(status) => status,
            Err(_) => ExitCode::from(3),
        },
        Err(error) => {
            let translator = Translator::new(error.language());
            match error.message() {
                Ok(message) => match Commands::usage_error(message, translator) {
                    Ok(status) => status,
                    Err(_) => ExitCode::from(3),
                },
                Err(_) => ExitCode::from(3),
            }
        }
    }
}
