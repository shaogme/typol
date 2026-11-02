#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::string::String;
use core::fmt;

use typol_core::DiagnosticTranslationError;
#[doc(hidden)]
pub use typol_core::{DiagnosticMessage, DiagnosticMessageKey};
use u18n::{Locale, LocaleProvider};

#[cfg(feature = "std")]
use std::env;

include!(concat!(env!("OUT_DIR"), "/typol_messages.rs"));

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Language {
    English,
    Chinese,
}

#[derive(Clone, Copy)]
pub struct Translator {
    language: Language,
}

impl Language {
    pub fn from_locale(locale: &str) -> Option<Self> {
        let normalized: String = locale
            .split('@')
            .next()?
            .split('.')
            .next()?
            .replace('_', "-");
        let language = normalized.split('-').next()?;
        let language = if language.eq_ignore_ascii_case("en") {
            Self::English
        } else if language.eq_ignore_ascii_case("zh") {
            Self::Chinese
        } else {
            return None;
        };
        let resolved = LocaleTag::fallback(&normalized).ok()?;
        if language == Self::Chinese {
            Some(Self::Chinese)
        } else {
            match resolved {
                LocaleTag::En => Some(Self::English),
                LocaleTag::ZhCn => Some(Self::Chinese),
            }
        }
    }

    #[cfg(feature = "std")]
    pub fn detect() -> Self {
        for variable in ["TYPOL_LANG", "LC_ALL", "LC_MESSAGES", "LANG"] {
            if let Ok(value) = env::var(variable)
                && let Some(language) = Self::from_locale(&value)
            {
                return language;
            }
        }
        Self::English
    }
}

impl Translator {
    pub const fn new(language: Language) -> Self {
        Self { language }
    }

    pub const fn language(self) -> Language {
        self.language
    }
}

impl LocaleProvider for Translator {
    fn provide_locale(&self) -> &Locale {
        match self.language {
            Language::English => LocaleTag::En.as_locale(),
            Language::Chinese => LocaleTag::ZhCn.as_locale(),
        }
    }
}

include!(concat!(env!("OUT_DIR"), "/typol_diagnostic_messages.rs"));
