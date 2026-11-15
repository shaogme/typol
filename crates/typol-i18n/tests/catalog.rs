use std::fmt;

use typol_i18n::{Language, Translator, t};

struct FailingWriter;

impl fmt::Write for FailingWriter {
    fn write_str(&mut self, _: &str) -> fmt::Result {
        Err(fmt::Error)
    }
}

struct PartialWriter {
    output: String,
    writes_left: usize,
}

impl fmt::Write for PartialWriter {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        if self.writes_left == 0 {
            return Err(fmt::Error);
        }
        self.output.push_str(text);
        self.writes_left -= 1;
        Ok(())
    }
}

#[test]
fn renders_catalog_messages_with_the_translation_macro() {
    let english_translator = Translator::new(Language::English);
    let chinese_translator = Translator::new(Language::Chinese);
    let english = t! {
        name: "cli.help",
        locale: english_translator,
    };
    let chinese = t! {
        name: "cli.help",
        locale: chinese_translator,
    };

    assert!(matches!(english.as_ref(), Ok(text) if text.starts_with("Usage:")));
    assert!(matches!(chinese.as_ref(), Ok(text) if text.starts_with("用法：")));
    assert!(matches!(chinese.as_ref(), Ok(text) if text.contains("选择英文或简体中文")));
}

#[test]
fn translates_diagnostic_messages_at_the_call_site() {
    let english = t! {
        name: "diagnostic.schema.root_missing_entry",
        locale: Translator::new(Language::English),
    };
    let chinese = t! {
        name: "diagnostic.schema.root_missing_entry",
        locale: Translator::new(Language::Chinese),
    };

    assert_eq!(
        english.as_deref(),
        Ok("root schema must declare an entry struct")
    );
    assert_eq!(chinese.as_deref(), Ok("根 Schema 必须声明 entry 结构体"));
}

#[test]
fn returns_sink_failures_without_a_text_fallback() {
    let translator = Translator::new(Language::English);
    let mut failing_writer = FailingWriter;
    let failure = t! {
        name: "diagnostic.config.fixed_array_length_mismatch",
        locale: translator,
        args: { expected: 4usize, actual: 3usize },
        sink: &mut failing_writer,
    };
    assert!(failure.is_err());

    let mut partial_writer = PartialWriter {
        output: String::new(),
        writes_left: 1,
    };
    let failure = t! {
        name: "diagnostic.config.fixed_array_length_mismatch",
        locale: translator,
        args: { expected: 4usize, actual: 3usize },
        sink: &mut partial_writer,
    };
    assert!(failure.is_err());
    assert!(!partial_writer.output.is_empty());
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
