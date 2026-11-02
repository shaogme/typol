use std::{env::args_os, path::Path, process::ExitCode};

use typol_i18n::{Language, Translator};
use typol_std::{
    DiagnosticRenderError, DiagnosticRenderer, FileSchemaCompileErrors, FileSchemaCompileFailure,
    FileSchemaCompileOptions, FileSchemaCompiler, RenderOptions, opeo::ErrSlot,
};

fn main() -> ExitCode {
    let Some(path) = args_os().nth(1) else {
        eprintln!("Usage: file_schema <root schema path>");
        return ExitCode::FAILURE;
    };
    let mut compile_errors = ErrSlot::<FileSchemaCompileErrors>::new();
    match compile_errors.try_call(|out| {
        FileSchemaCompiler::compile(Path::new(&path), FileSchemaCompileOptions::default(), out)
    }) {
        Ok(compilation) => {
            println!(
                "Schema compilation and source name registration succeeded; steps: {}, peak payload: {}",
                compilation.usage().steps,
                compilation.usage().peak_workspace_bytes
            );
            ExitCode::SUCCESS
        }
        Err(caught) => {
            let errors = caught.get();
            let rendered = match errors.failure() {
                FileSchemaCompileFailure::Root(error) => {
                    let mut render_errors = ErrSlot::<DiagnosticRenderError>::new();
                    render_errors.call(|out| {
                        DiagnosticRenderer::render_host(
                            error,
                            RenderOptions::default(),
                            Translator::new(Language::English),
                            out,
                        )
                    })
                }
                FileSchemaCompileFailure::Compilation(_) => {
                    eprintln!("{errors}");
                    return ExitCode::FAILURE;
                }
            };
            match rendered {
                Ok(text) => eprintln!("{text}"),
                Err(error) => eprintln!("Diagnostic rendering failed: {error:?}"),
            }
            ExitCode::FAILURE
        }
    }
}
