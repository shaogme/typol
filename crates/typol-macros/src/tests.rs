use crate::{
    expand::Expander,
    model::{BodyModel, Model},
};
use std::fmt::Display;

use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::{Data, DeriveInput, Error, Fields, Result, parse_str, parse2};

macro_rules! ensure {
    ($condition:expr $(,)?) => {
        if !$condition {
            return Err(test_error(format!(
                "assertion failed: {}",
                stringify!($condition)
            )));
        }
    };
    ($condition:expr, $($argument:tt)+) => {
        if !$condition {
            return Err(test_error(format!($($argument)+)));
        }
    };
}

macro_rules! ensure_eq {
    ($left:expr, $right:expr $(,)?) => {
        let left = $left;
        let right = $right;
        if left != right {
            return Err(test_error(format!(
                "assertion failed: left={left:?}, right={right:?}"
            )));
        }
    };
}

fn test_error(message: impl Display) -> Error {
    Error::new(Span::call_site(), message)
}

fn expand(source: &str) -> Result<String> {
    let input: DeriveInput = parse_str(source)?;
    let model = Model::parse(input)?;
    Ok(Expander::expand(&model)?.to_string())
}

fn reject(source: &str, message: &str) -> Result<()> {
    let input = parse_str(source)?;
    let error = Model::parse(input)
        .err()
        .ok_or_else(|| test_error(format!("invalid declaration was accepted: {source}")))?;
    ensure!(error.to_string().contains(message), "{error}");
    let tokens = error.to_compile_error().to_string();
    ensure!(tokens.contains("compile_error"));
    Ok(())
}

#[test]
fn named_structs_empty_structs_and_all_payloads_expand() -> Result<()> {
    for source in [
        "struct Empty {}",
        "struct UnitEmpty;",
        "struct Root { value: u8, nested: module::Alias }",
        "enum Choice { Unit, Single(u8), Object { label: String }, Empty {} }",
    ] {
        let output = expand(source)?;
        ensure!(output.contains("fn describe"));
        ensure!(output.contains("fn to_config_value"));
        ensure!(output.contains("fn from_config_value"));
        ensure!(output.contains("start :: < Self >"));
        ensure!(!output.contains("allow"));
        ensure!(!output.contains("typol_core"));
        ensure!(!output.contains("session"));
        let tokens: TokenStream = parse_str(&output)?;
        ensure_eq!(tokens.to_string(), output);
    }
    let output = expand("enum Choice { Unit, Single(u8), Object { label: String } }")?;
    for method in [
        "single_variant",
        "structure_variant",
        "read_single",
        "read_field",
        "read_enumeration",
    ] {
        ensure!(output.contains(method));
    }
    Ok(())
}

#[test]
fn unit_structs_expand_as_zero_field_structures_and_decode_as_unit_values() -> Result<()> {
    let output = expand("struct Empty;")?;

    ensure!(output.contains("TypeShape :: Structure"));
    ensure!(output.contains("context . structure"));
    ensure!(output.contains("read_structure"));
    ensure!(output.contains("Ok (Self)"));
    ensure!(!output.contains("Self {"));
    Ok(())
}

#[test]
fn renamed_types_fields_variants_and_raw_identifiers_keep_rust_names() -> Result<()> {
    let input: DeriveInput = parse2(quote! {
        #[typol(rename = "Renamed")]
        struct r#type {
            #[typol(rename = "value")]
            r#type: u8,
            r#match: String,
        }
    })?;
    let model = Model::parse(input)?;
    let BodyModel::Structure { fields, .. } = model.body() else {
        return Err(test_error("expected a structure model"));
    };
    let first_field = fields
        .first()
        .ok_or_else(|| test_error("expected the first field"))?;
    let second_field = fields
        .get(1)
        .ok_or_else(|| test_error("expected the second field"))?;
    ensure_eq!(first_field.name.value(), "value");
    ensure_eq!(second_field.name.value(), "match");
    let output = Expander::expand(&model)?.to_string();
    ensure!(output.contains("r#type"));
    ensure!(output.contains("\"Renamed\""));
    let output = expand(
        "enum Choice { #[typol(rename = \"Object\")] Old { #[typol(rename = \"value\")] old: u8 } }",
    )?;
    ensure!(output.contains("Self :: Old"));
    ensure!(output.contains("\"Object\""));
    ensure!(output.contains("\"value\""));
    Ok(())
}

#[test]
fn generics_const_parameters_where_clauses_and_paths_are_preserved() -> Result<()> {
    let output = expand(
        "struct Root<T, U, const N: usize> where T: Clone, U: Copy { values: [T; N], other: module::Wrapper<U> }",
    )?;
    for text in [
        "T : Clone",
        "U : Copy",
        "const N : usize",
        "[T ; N]",
        "module :: Wrapper < U >",
        "Self : 'static",
    ] {
        ensure!(output.contains(text), "missing {text}: {output}");
    }
    ensure!(output.contains("T : __typol_0_Typol"));
    ensure!(output.contains("U : __typol_0_Typol"));
    ensure!(!output.contains("Default"));
    let output = expand("struct Root<T> where T: Iterator { value: T::Item }")?;
    ensure!(output.contains("T :: Item : __typol_0_Typol"));
    let output = expand("struct Root<T = u8, const N: usize = 2> { values: [T; N] }")?;
    ensure!(!output.contains("T = u8"));
    Ok(())
}

#[test]
fn facade_paths_and_hygiene_are_explicit() -> Result<()> {
    for path in ["renamed_typol", "crate", "::renamed::facade"] {
        let output = expand(&format!(
            "#[typol(crate = \"{path}\")] struct Root {{ value: ::std::vec::Vec<module::Alias> }}"
        ))?;
        ensure!(output.contains(&path.replace("::", " :: ").trim().to_string()));
        ensure!(!output.contains("use :: typol"));
        ensure!(output.contains(":: std :: vec :: Vec < module :: Alias >"));
    }
    ensure!(expand("struct Root { value: u8 }")?.contains("use :: typol"));
    let output = expand("struct Root<__typol_0_Typol> { value: __typol_0_Typol }")?;
    ensure!(output.contains("__typol_1_Typol"));
    let output = expand("struct Root { __typol_0_context: u8 }")?;
    ensure!(output.contains("__typol_1_context"));
    Ok(())
}

#[test]
fn internal_prefix_selection_scans_nested_identifiers_and_skips_gaps() -> Result<()> {
    let mut fields: Vec<_> = (0..64)
        .map(|index| format!("__typol_{index}_field: u8"))
        .collect();
    fields.push("__typol_065_padded: u8".to_owned());
    let source = format!(
        "#[other(inner(__typol_64_nested))] struct Root {{ {} }}",
        fields.join(",")
    );

    let output = expand(&source)?;

    ensure!(output.contains("__typol_65_Typol"));
    let gap_output = expand(
        "#[other(inner(__typol_2_nested))] struct Root { __typol_0_field: u8, __typol_01_padded: u8 }",
    )?;
    ensure!(gap_output.contains("__typol_1_Typol"));
    Ok(())
}

#[test]
fn invalid_shapes_and_borrowed_targets_are_rejected() -> Result<()> {
    for (source, message) in [
        ("union Root { value: u8 }", "union"),
        ("struct Root(u8);", "named-field"),
        ("enum Root {}", "empty enums"),
        ("enum Root { Value(u8, u16) }", "exactly one payload"),
        ("enum Root { Value() }", "exactly one payload"),
        ("struct Root<'a> { value: &'a str }", "lifetime generics"),
        ("struct Root { value: &'static str }", "own its data"),
        ("struct Root { value: Vec<&'static str> }", "own its data"),
        ("struct Root { value: [(&'static str); 2] }", "own its data"),
        ("struct Root { value: *const u8 }", "own its data"),
        ("struct Root { value: (u8, u8) }", "own its data"),
        ("struct Root { value: fn(u8) }", "own its data"),
        ("struct Root { value: dyn Send }", "own its data"),
        ("struct Root { value: Holder<'static> }", "borrowed targets"),
    ] {
        reject(source, message)?;
    }
    Ok(())
}

#[test]
fn named_fields_without_identifiers_return_spanned_errors() -> Result<()> {
    let mut input: DeriveInput = parse_str("struct Root { value: u8 }")?;
    let Data::Struct(data) = &mut input.data else {
        return Err(test_error("expected a struct declaration"));
    };
    let Fields::Named(fields) = &mut data.fields else {
        return Err(test_error("expected named fields"));
    };
    let field = fields
        .named
        .first_mut()
        .ok_or_else(|| test_error("expected a field"))?;
    field.ident = None;

    let error = Model::parse(input).err().ok_or_else(|| {
        test_error("named field without an identifier should return a macro error")
    })?;

    ensure_eq!(error.to_string(), "named field is missing an identifier");
    ensure!(error.span().start().column > 0);
    Ok(())
}

#[test]
fn attributes_names_positions_and_collisions_are_checked() -> Result<()> {
    for (source, message) in [
        (
            "#[typol(rename = \"X\")] #[typol(crate = \"crate\")] struct Root {}",
            "cannot be repeated",
        ),
        (
            "#[typol(rename = \"X\", rename = \"Y\")] struct Root {}",
            "cannot be repeated",
        ),
        (
            "#[typol(crate = \"crate\", crate = \"other\")] struct Root {}",
            "cannot be repeated",
        ),
        (
            "struct Root { #[typol(trusted)] value: u8 }",
            "only allowed on types",
        ),
        (
            "#[typol(trusted, trusted)] struct Root {}",
            "cannot be repeated",
        ),
        ("#[typol(skip)] struct Root {}", "unknown"),
        ("#[typol()] struct Root {}", "cannot be empty"),
        (
            "struct Root { #[typol(crate = \"crate\")] value: u8 }",
            "only allowed",
        ),
        (
            "enum Root { #[typol(crate = \"crate\")] Value }",
            "only allowed",
        ),
        (
            "enum Root { Value(#[typol(rename = \"value\")] u8) }",
            "not allowed here",
        ),
        (
            "struct Root<#[typol(rename = \"value\")] T> { value: T }",
            "not allowed here",
        ),
        (
            "struct Root<const N: usize> { #[typol(rename = \"x\")] a: u8, x: u8 }",
            "collision",
        ),
        ("enum Root { X, #[typol(rename = \"X\")] Y }", "collision"),
        (
            "enum Root { X { a: u8, #[typol(rename = \"a\")] b: u8 } }",
            "collision",
        ),
        (
            "#[typol(crate = \"facade<u8>\")] struct Root {}",
            "module path",
        ),
        ("struct r#type {}", "non-reserved"),
        ("struct Root { r#type: u8 }", "non-reserved"),
        ("struct café {}", "ASCII"),
    ] {
        reject(source, message)?;
    }
    for name in [
        "", "a-b", "a::b", "9a", "café", "extern", "mod", "type", "struct", "entry", "enum",
        "option", "map", "i8", "i16", "i32", "i64", "i128", "u8", "u16", "u32", "u64", "u128",
        "f32", "f64", "string", "bool", "true", "false", "none",
    ] {
        reject(
            &format!("#[typol(rename = \"{name}\")] struct Root {{}}"),
            "non-reserved",
        )?;
    }
    Ok(())
}

#[test]
fn aliases_and_unsupported_path_types_are_left_to_the_trait_contract() -> Result<()> {
    for ty in [
        "usize",
        "isize",
        "Box<u8>",
        "Rc<u8>",
        "Arc<u8>",
        "HashMap<u8, u8>",
        "Option<Option<u8>>",
        "custom::Vec<u8>",
        "Alias",
    ] {
        let output = expand(&format!("struct Root {{ value: {ty} }}"))?;
        ensure!(output.contains("DescriptorProvider"));
        ensure!(!output.contains("BuiltinType"));
    }
    let output = expand("struct Root { children: Vec<Self> }")?;
    ensure!(output.contains("Vec < Self >"));
    ensure!(!output.contains("Vec < Self > :"));
    Ok(())
}

#[test]
fn diagnostic_tokens_keep_the_offending_rust_span() -> Result<()> {
    for source in [
        "struct Root { #[typol(rename = \"type\")] value: u8 }",
        "struct Root { #[typol(skip)] value: u8 }",
        "struct Root { value: &'static str }",
        "struct Root<'a> { value: u8 }",
    ] {
        let error = Model::parse(parse_str(source)?)
            .err()
            .ok_or_else(|| test_error("expected a diagnostic for an invalid declaration"))?;
        let text = error
            .span()
            .source_text()
            .ok_or_else(|| test_error("expected diagnostic source text"))?;
        ensure!(!text.is_empty());
        ensure!(source.contains(&text));
        ensure!(error.span().start().column > 0);
    }
    let error = Model::parse(parse_str(
        "struct Root { #[typol(rename = \"type\")] value: u8 }",
    )?)
    .err()
    .ok_or_else(|| test_error("expected a diagnostic for a reserved rename"))?;
    ensure!(error.span().source_text().as_deref() == Some("\"type\""));
    let input: DeriveInput = parse_str("struct Root { a: u8, #[typol(rename = \"a\")] b: u8 }")?;
    let error = Model::parse(input)
        .err()
        .ok_or_else(|| test_error("expected diagnostics for colliding field names"))?;
    let errors: Vec<_> = error.into_iter().collect();
    ensure_eq!(errors.len(), 2);
    let first_error = errors
        .first()
        .ok_or_else(|| test_error("expected the first collision diagnostic"))?;
    let second_error = errors
        .get(1)
        .ok_or_else(|| test_error("expected the second collision diagnostic"))?;
    ensure!(first_error.span().source_text().as_deref() == Some("\"a\""));
    ensure!(second_error.span().source_text().as_deref() == Some("a"));
    Ok(())
}

#[test]
fn bounds_follow_type_positions_and_generated_imports_follow_usage() -> Result<()> {
    let output = expand("struct Root<T> { value: module::T }")?;
    ensure!(output.contains("impl < T >"));
    let output = expand("struct Root<T> where T: Iterator { value: <T as Iterator>::Item }")?;
    ensure!(output.contains("T : __typol_0_Typol"));
    for source in ["struct Empty {}", "enum Empty { Unit, Object {} }"] {
        ensure!(!expand(source)?.contains("DescriptorProvider as"));
    }
    let output = expand("enum Choice { Value(u8), Object { value: u8 } }")?;
    ensure!(!output.contains("EnumPayload as"));
    ensure!(output.contains("ConversionPayload as"));
    let output = expand("struct Root { second: u16, first: u8 }")?;
    ensure!(output.contains(
        "read_field :: < u16 > (__typol_0_members , 0usize , __typol_0_out . reborrow ())"
    ));
    ensure!(output.contains(
        "read_field :: < u8 > (__typol_0_members , 1usize , __typol_0_out . reborrow ())"
    ));
    Ok(())
}

#[test]
fn budgeted_marker_bounds_follow_nested_and_recursive_types() -> Result<()> {
    let output = expand("struct Parent<T> { child: Option<Vec<T>> }")?;
    ensure!(output.contains("T : __typol_0_BudgetedTypol"));
    ensure!(!output.contains("Option < Vec < T > > : __typol_0_BudgetedTypol"));

    let output = expand("struct Parent<T> where T: Iterator { child: Vec<T::Item> }")?;
    ensure!(output.contains("T :: Item : __typol_0_BudgetedTypol"));

    let output = expand("struct Recursive { children: Vec<Self> }")?;
    ensure!(output.contains("impl __typol_0_BudgetedTypol for Recursive"));
    ensure!(!output.contains("Vec < Self > : __typol_0_BudgetedTypol"));
    Ok(())
}

#[test]
fn trusted_derive_omits_the_budgeted_marker() -> Result<()> {
    let output = expand("#[typol(trusted)] struct Parent { child: Manual }")?;
    ensure!(output.contains("impl __typol_0_Typol for Parent"));
    ensure!(!output.contains("BudgetedTypol"));
    Ok(())
}

#[test]
fn rename_overrides_invalid_defaults_and_foreign_attributes_are_preserved() -> Result<()> {
    let output = expand(
        "#[serde(rename = \"Other\")] #[typol(rename = \"Root\")] struct café { #[serde(skip)] #[typol(rename = \"value\")] r#type: u8 }",
    )?;
    ensure!(output.contains("impl __typol_0_Typol for café"));
    ensure!(output.contains("self . r#type"));
    ensure!(!output.contains("serde"));
    reject("#[typol(rename = 7)] struct Root {}", "string literal")?;
    reject("#[typol(crate = \"\")] struct Root {}", "unexpected end")?;
    reject(
        "struct Root<#[typol(rename = \"Count\")] const N: usize> { values: [u8; N] }",
        "not allowed here",
    )?;
    Ok(())
}
