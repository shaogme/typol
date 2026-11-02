use proc_macro2::Span;
use quote::ToTokens;
use std::collections::BTreeMap;
use syn::{Attribute, Error, Ident, LitStr, Path, Result, parse_str, parse2};

pub(crate) struct Attributes {
    rename: Option<LitStr>,
    facade: Option<Path>,
    trusted: bool,
}

impl Attributes {
    pub(crate) fn parse(attributes: &[Attribute], type_level: bool) -> Result<Self> {
        let mut result = Self {
            rename: None,
            facade: None,
            trusted: false,
        };
        let mut seen = false;
        for attribute in attributes {
            if !attribute.path().is_ident("typol") {
                continue;
            }
            if seen {
                return Err(Error::new_spanned(
                    attribute,
                    "typol attribute cannot be repeated",
                ));
            }
            seen = true;
            let mut count = 0;
            attribute.parse_nested_meta(|meta| {
                count += 1;
                if meta.path.is_ident("rename") {
                    if result.rename.is_some() {
                        return Err(meta.error("rename cannot be repeated"));
                    }
                    let value: LitStr = meta.value()?.parse()?;
                    Self::validate_name(&value.value(), value.span())?;
                    result.rename = Some(value);
                } else if meta.path.is_ident("crate") {
                    if !type_level {
                        return Err(meta.error("crate is only allowed on types"));
                    }
                    if result.facade.is_some() {
                        return Err(meta.error("crate cannot be repeated"));
                    }
                    let value: LitStr = meta.value()?.parse()?;
                    let path: Path = value.parse()?;
                    if path
                        .segments
                        .iter()
                        .any(|segment| !segment.arguments.is_empty())
                    {
                        return Err(Error::new(value.span(), "crate must be a module path"));
                    }
                    result.facade = Some(path);
                } else if meta.path.is_ident("trusted") {
                    if !type_level {
                        return Err(meta.error("trusted is only allowed on types"));
                    }
                    if result.trusted {
                        return Err(meta.error("trusted cannot be repeated"));
                    }
                    result.trusted = true;
                } else {
                    return Err(meta.error("unknown typol attribute key"));
                }
                Ok(())
            })?;
            if count == 0 {
                return Err(Error::new_spanned(
                    attribute,
                    "typol attribute cannot be empty",
                ));
            }
        }
        Ok(result)
    }

    pub(crate) fn reject(attributes: &[Attribute]) -> Result<()> {
        for attribute in attributes {
            if attribute.path().is_ident("typol") {
                return Err(Error::new_spanned(
                    attribute,
                    "typol attributes are not allowed here",
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn name(&self, ident: &Ident) -> Result<LitStr> {
        if let Some(rename) = &self.rename {
            return Ok(LitStr::new(&rename.value(), rename.span()));
        }
        let identifier_text = ident.to_string();
        let name = identifier_text
            .strip_prefix("r#")
            .unwrap_or(&identifier_text);
        Self::validate_name(name, ident.span())?;
        Ok(LitStr::new(name, ident.span()))
    }

    pub(crate) fn facade(&self) -> Result<Path> {
        match &self.facade {
            Some(path) => parse2(path.to_token_stream()),
            None => parse_str("::typol"),
        }
    }

    pub(crate) const fn trusted(&self) -> bool {
        self.trusted
    }

    pub(crate) fn unique<'a>(names: impl IntoIterator<Item = &'a LitStr>) -> Result<()> {
        let mut seen = BTreeMap::new();
        for name in names {
            if let Some(previous) = seen.insert(name.value(), name.span()) {
                let mut error = Error::new(name.span(), "DSL name collision");
                error.combine(Error::new(
                    previous,
                    "previous member with the same name is here",
                ));
                return Err(error);
            }
        }
        Ok(())
    }

    fn validate_name(name: &str, span: Span) -> Result<()> {
        let mut bytes = name.bytes();
        let valid = bytes
            .next()
            .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
            && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
        let reserved = matches!(
            name,
            "extern"
                | "mod"
                | "type"
                | "struct"
                | "entry"
                | "enum"
                | "option"
                | "map"
                | "i8"
                | "i16"
                | "i32"
                | "i64"
                | "i128"
                | "u8"
                | "u16"
                | "u32"
                | "u64"
                | "u128"
                | "f32"
                | "f64"
                | "string"
                | "bool"
                | "true"
                | "false"
                | "none"
        );
        if !valid || reserved {
            return Err(Error::new(
                span,
                "DSL name must be a non-reserved ASCII identifier",
            ));
        }
        Ok(())
    }
}
