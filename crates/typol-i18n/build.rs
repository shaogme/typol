use std::{env, error::Error, fs, path::PathBuf};

use u18n_compiler::{CompilationInput, Compiler, CompilerOptions, SourceFile};

fn main() -> Result<(), Box<dyn Error>> {
    let input = CompilationInput::new(
        SourceFile::new("schema.u18n", include_str!("schema.u18n")),
        vec![
            SourceFile::new("locales/en.u18n", include_str!("locales/en.u18n")),
            SourceFile::new("locales/zh-CN.u18n", include_str!("locales/zh-CN.u18n")),
        ],
    );
    let generated = Compiler::new(
        CompilerOptions::default()
            .with_locale_tag_fallback(true)
            .with_cow_macro_output(true),
    )
    .compile(input)?;
    let output =
        PathBuf::from(env::var_os("OUT_DIR").ok_or("OUT_DIR is unset")?).join("typol_messages.rs");
    fs::write(output, generated.source())?;

    println!("cargo:rerun-if-changed=schema.u18n");
    println!("cargo:rerun-if-changed=locales/en.u18n");
    println!("cargo:rerun-if-changed=locales/zh-CN.u18n");
    Ok(())
}
