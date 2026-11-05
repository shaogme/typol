use std::{error::Error, fmt, io};

use typol_i18n::{DiagnosticMessageKey, Language, Translator, diagnostic_message, t};

struct FailingWriter;

impl fmt::Write for FailingWriter {
    fn write_str(&mut self, _: &str) -> fmt::Result {
        Err(fmt::Error)
    }
}

#[test]
fn renders_catalog_messages_with_the_translation_macro() {
    let english_translator = Translator::new(Language::English);
    let chinese_translator = Translator::new(Language::Chinese);
    let english: Result<&'static str, _> = t!("cli.help", locale = english_translator);
    let chinese: Result<&'static str, _> = t!("cli.help", locale = chinese_translator);

    assert!(matches!(english.as_ref(), Ok(text) if text.starts_with("Usage:")));
    assert!(matches!(chinese.as_ref(), Ok(text) if text.starts_with("用法：")));
    assert!(matches!(chinese.as_ref(), Ok(text) if text.contains("选择英文或简体中文")));
}

#[test]
fn translates_schema_generated_diagnostic_keys() -> Result<(), Box<dyn Error>> {
    let message = diagnostic_message!(
        "diagnostic.schema.root_missing_entry",
        "root schema must declare an entry struct",
    );
    let key = message
        .key()
        .ok_or_else(|| io::Error::other("generated messages have a key"))?;
    let english = Translator::new(Language::English)
        .translate_diagnostic(key)
        .map_err(|_| io::Error::other("English diagnostic entry is present"))?;
    let chinese = Translator::new(Language::Chinese)
        .translate_diagnostic(key)
        .map_err(|_| io::Error::other("Chinese diagnostic entry is present"))?;

    if message.text() != "root schema must declare an entry struct"
        || english != "root schema must declare an entry struct"
        || chinese != "根 Schema 必须声明 entry 结构体"
    {
        return Err(io::Error::other("diagnostic catalog output did not match").into());
    }
    if Translator::new(Language::English)
        .translate_diagnostic(DiagnosticMessageKey::__from_index(u16::MAX))
        .is_ok()
    {
        return Err(io::Error::other("invalid diagnostic key was accepted").into());
    }
    Ok(())
}

#[test]
fn writes_diagnostic_text_without_owning_the_translation() -> Result<(), Box<dyn Error>> {
    let message = diagnostic_message!(
        "diagnostic.schema.root_missing_entry",
        "root schema must declare an entry struct",
    );
    let key = message
        .key()
        .ok_or_else(|| io::Error::other("generated messages have a key"))?;
    let translator = Translator::new(Language::English);
    let mut output = String::new();
    translator
        .write_diagnostic(key, &mut output)
        .map_err(|_| io::Error::other("English diagnostic entry is present"))?;
    if output != "root schema must declare an entry struct" {
        return Err(io::Error::other("diagnostic writer output did not match").into());
    }

    let mut failing_writer = FailingWriter;
    if translator
        .write_diagnostic(key, &mut failing_writer)
        .is_ok()
    {
        return Err(io::Error::other("writer failure was ignored").into());
    }
    let mut invalid_output = String::new();
    if translator
        .write_diagnostic(
            DiagnosticMessageKey::__from_index(u16::MAX),
            &mut invalid_output,
        )
        .is_ok()
        || !invalid_output.is_empty()
    {
        return Err(io::Error::other("invalid translation wrote partial output").into());
    }
    Ok(())
}

#[test]
fn recognizes_locale_names_and_system_locale_suffixes() {
    assert_eq!(
        Language::from_locale("EN_us.UTF-8@euro"),
        Some(Language::English)
    );
    assert_eq!(
        Language::from_locale("zh_CN.UTF-8"),
        Some(Language::Chinese)
    );
    assert_eq!(Language::from_locale("zh-Hant"), Some(Language::Chinese));
    assert_eq!(Language::from_locale("en-150"), Some(Language::English));
    assert_eq!(Language::from_locale("fr_FR"), None);
    assert_eq!(Language::from_locale("en-@@"), None);
}
