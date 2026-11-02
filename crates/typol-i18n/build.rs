use std::{
    collections::BTreeSet,
    env,
    error::Error,
    fs,
    io::{self, Write},
    path::PathBuf,
};

use u18n_compiler::{CompilationInput, Compiler, CompilerOptions, SourceFile};

fn main() -> Result<(), Box<dyn Error>> {
    let schema = include_str!("schema.u18n");
    let input = CompilationInput::new(
        SourceFile::new("schema.u18n", schema),
        vec![
            SourceFile::new("locales/en.u18n", include_str!("locales/en.u18n")),
            SourceFile::new("locales/zh-CN.u18n", include_str!("locales/zh-CN.u18n")),
        ],
    );
    let generated =
        Compiler::new(CompilerOptions::default().with_locale_tag_fallback(true)).compile(input)?;
    let output =
        PathBuf::from(env::var_os("OUT_DIR").ok_or("OUT_DIR is unset")?).join("typol_messages.rs");
    let generated_source = generated.source();
    let diagnostic_output = PathBuf::from(env::var_os("OUT_DIR").ok_or("OUT_DIR is unset")?)
        .join("typol_diagnostic_messages.rs");
    let diagnostic_source = generate_diagnostic_messages(schema)?;
    fs::write(output, generated_source)?;
    fs::write(diagnostic_output, diagnostic_source)?;

    let mut stdout = io::stdout().lock();
    writeln!(stdout, "cargo:rerun-if-changed=schema.u18n")?;
    writeln!(stdout, "cargo:rerun-if-changed=locales/en.u18n")?;
    writeln!(stdout, "cargo:rerun-if-changed=locales/zh-CN.u18n")?;
    Ok(())
}

fn generate_diagnostic_messages(schema: &str) -> Result<String, io::Error> {
    let mut keys = Vec::new();
    let mut unique_keys = BTreeSet::new();
    for (line_number, source_line) in schema.lines().enumerate() {
        let line = source_line.trim();
        let Some(after_message) = line.strip_prefix("message") else {
            continue;
        };
        if !after_message.trim_start().starts_with("diagnostic") {
            continue;
        }
        let declaration = after_message.strip_prefix(' ').ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "invalid diagnostic message declaration at line {}",
                    line_number + 1
                ),
            )
        })?;
        if !declaration.starts_with("diagnostic.") {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "invalid diagnostic message declaration at line {}",
                    line_number + 1
                ),
            ));
        }
        let key = declaration.strip_suffix("();").ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "invalid diagnostic message declaration at line {}",
                    line_number + 1
                ),
            )
        })?;
        if !key.split('.').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        }) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "invalid diagnostic message key {key:?} at line {}",
                    line_number + 1
                ),
            ));
        }
        if !unique_keys.insert(key.to_owned()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("duplicate diagnostic message key {key:?}"),
            ));
        }
        if keys.len() > usize::from(u16::MAX) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "diagnostic message key count exceeds u16 index capacity",
            ));
        }
        keys.push(key.to_owned());
    }
    if keys.is_empty() || keys.len() != unique_keys.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "diagnostic message key generation produced an invalid entry count",
        ));
    }

    let mut source = String::from(
        "#[doc = \"Returns the generated key for a schema-declared diagnostic message.\"]\n#[macro_export]\nmacro_rules! diagnostic_key {\n",
    );
    for (index, key) in keys.iter().enumerate() {
        source.push_str(&format!(
            "    ({key:?}) => {{ $crate::DiagnosticMessageKey::__from_index({index}) }};\n"
        ));
    }
    source.push_str(
        "}\n\n#[doc = \"Creates a localized diagnostic message from a schema key and English fallback.\"]\n#[macro_export]\nmacro_rules! diagnostic_message {\n",
    );
    source.push_str(
        "    ($key:tt, $fallback:expr $(,)?) => {\n        $crate::DiagnosticMessage::localized($fallback, $crate::diagnostic_key!($key))\n    };\n}\n\n",
    );
    source.push_str(
        "impl Translator {\n    #[doc = \"Writes a generated diagnostic translation directly to the caller's sink without allocating.\"]\n    pub fn write_diagnostic<W: fmt::Write>(\n        &self,\n        key: DiagnosticMessageKey,\n        writer: &mut W,\n    ) -> Result<(), DiagnosticTranslationError> {\n        match key.__index() {\n",
    );
    for (index, key) in keys.iter().enumerate() {
        let message_macro = message_macro_name(key);
        source.push_str(&format!(
            "            {index} => {{ let text = {message_macro}!(self).map_err(|_| DiagnosticTranslationError)?; writer.write_str(text).map_err(|_| DiagnosticTranslationError) }},\n"
        ));
    }
    source.push_str(
        "            _ => Err(DiagnosticTranslationError),\n        }\n    }\n\n    #[doc = \"Translates a generated diagnostic message key for this translator's language.\"]\n    pub fn translate_diagnostic(\n        &self,\n        key: DiagnosticMessageKey,\n    ) -> Result<&'static str, DiagnosticTranslationError> {\n        match key.__index() {\n",
    );
    for (index, key) in keys.iter().enumerate() {
        let message_macro = message_macro_name(key);
        source.push_str(&format!(
            "            {index} => {message_macro}!(self).map_err(|_| DiagnosticTranslationError),\n"
        ));
    }
    source.push_str("            _ => Err(DiagnosticTranslationError),\n        }\n    }\n}\n");
    let write_dispatch_count = source.matches("let text = m_diagnostic_").count();
    let translate_dispatch_count = source.matches("=> m_diagnostic_").count();
    if write_dispatch_count != keys.len() || translate_dispatch_count != keys.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "diagnostic message generation produced an invalid translation dispatch count",
        ));
    }
    Ok(source)
}

fn message_macro_name(key: &str) -> String {
    let mut name = String::from("m_");
    for character in key.chars() {
        match character {
            '.' => name.push_str("_s"),
            '_' => name.push_str("_u"),
            character => name.push(character),
        }
    }
    name
}
