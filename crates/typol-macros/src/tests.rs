use crate::{
    expand::Expander,
    model::{BodyModel, Model},
};
use proc_macro2::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Fields, parse_str, parse2};

fn expand(source: &str) -> String {
    let input: DeriveInput = parse_str(source).unwrap();
    let model = Model::parse(input).unwrap();
    Expander::expand(&model).unwrap().to_string()
}

fn reject(source: &str, message: &str) {
    let input = parse_str(source).unwrap();
    let error = match Model::parse(input) {
        Ok(_) => panic!("invalid declaration was accepted: {source}"),
        Err(error) => error,
    };
    assert!(error.to_string().contains(message), "{error}");
    let tokens = error.to_compile_error().to_string();
    assert!(tokens.contains("compile_error"));
}

#[test]
fn named_structs_empty_structs_and_all_payloads_expand() {
    for source in [
        "struct Empty {}",
        "struct UnitEmpty;",
        "struct Root { value: u8, nested: module::Alias }",
        "enum Choice { Unit, Single(u8), Object { label: String }, Empty {} }",
    ] {
        let output = expand(source);
        assert!(output.contains("fn describe"));
        assert!(output.contains("fn to_config_value"));
        assert!(output.contains("fn from_config_value"));
        assert!(output.contains("start :: < Self >"));
        assert!(!output.contains("allow"));
        assert!(!output.contains("typol_core"));
        assert!(!output.contains("session"));
        let tokens: TokenStream = parse_str(&output).unwrap();
        assert_eq!(tokens.to_string(), output);
    }
    let output = expand("enum Choice { Unit, Single(u8), Object { label: String } }");
    for method in [
        "single_variant",
        "structure_variant",
        "read_single",
        "read_field",
        "read_enumeration",
    ] {
        assert!(output.contains(method));
    }
}

#[test]
fn unit_structs_expand_as_zero_field_structures_and_decode_as_unit_values() {
    let output = expand("struct Empty;");

    assert!(output.contains("TypeShape :: Structure"));
    assert!(output.contains("context . structure"));
    assert!(output.contains("read_structure"));
    assert!(output.contains("Ok (Self)"));
    assert!(!output.contains("Self {"));
}

#[test]
fn renamed_types_fields_variants_and_raw_identifiers_keep_rust_names() {
    let input: DeriveInput = parse2(quote! {
        #[typol(rename = "Renamed")]
        struct r#type {
            #[typol(rename = "value")]
            r#type: u8,
            r#match: String,
        }
    })
    .unwrap();
    let model = Model::parse(input).unwrap();
    let BodyModel::Structure { fields, .. } = model.body() else {
        panic!()
    };
    assert_eq!(fields[0].name.value(), "value");
    assert_eq!(fields[1].name.value(), "match");
    let output = Expander::expand(&model).unwrap().to_string();
    assert!(output.contains("r#type"));
    assert!(output.contains("\"Renamed\""));
    let output = expand(
        "enum Choice { #[typol(rename = \"Object\")] Old { #[typol(rename = \"value\")] old: u8 } }",
    );
    assert!(output.contains("Self :: Old"));
    assert!(output.contains("\"Object\""));
    assert!(output.contains("\"value\""));
}

#[test]
fn generics_const_parameters_where_clauses_and_paths_are_preserved() {
    let output = expand(
        "struct Root<T, U, const N: usize> where T: Clone, U: Copy { values: [T; N], other: module::Wrapper<U> }",
    );
    for text in [
        "T : Clone",
        "U : Copy",
        "const N : usize",
        "[T ; N]",
        "module :: Wrapper < U >",
        "Self : 'static",
    ] {
        assert!(output.contains(text), "missing {text}: {output}");
    }
    assert!(output.contains("T : __typol_0_Typol"));
    assert!(output.contains("U : __typol_0_Typol"));
    assert!(!output.contains("Default"));
    let output = expand("struct Root<T> where T: Iterator { value: T::Item }");
    assert!(output.contains("T :: Item : __typol_0_Typol"));
    let output = expand("struct Root<T = u8, const N: usize = 2> { values: [T; N] }");
    assert!(!output.contains("T = u8"));
}

#[test]
fn facade_paths_and_hygiene_are_explicit() {
    for path in ["renamed_typol", "crate", "::renamed::facade"] {
        let output = expand(&format!(
            "#[typol(crate = \"{path}\")] struct Root {{ value: ::std::vec::Vec<module::Alias> }}"
        ));
        assert!(output.contains(&path.replace("::", " :: ").trim().to_string()));
        assert!(!output.contains("use :: typol"));
        assert!(output.contains(":: std :: vec :: Vec < module :: Alias >"));
    }
    assert!(expand("struct Root { value: u8 }").contains("use :: typol"));
    let output = expand("struct Root<__typol_0_Typol> { value: __typol_0_Typol }");
    assert!(output.contains("__typol_1_Typol"));
    let output = expand("struct Root { __typol_0_context: u8 }");
    assert!(output.contains("__typol_1_context"));
}

#[test]
fn internal_prefix_selection_scans_nested_identifiers_and_skips_gaps() {
    let mut fields: Vec<_> = (0..64)
        .map(|index| format!("__typol_{index}_field: u8"))
        .collect();
    fields.push("__typol_065_padded: u8".to_owned());
    let source = format!(
        "#[other(inner(__typol_64_nested))] struct Root {{ {} }}",
        fields.join(",")
    );

    let output = expand(&source);

    assert!(output.contains("__typol_65_Typol"));
    let gap_output = expand(
        "#[other(inner(__typol_2_nested))] struct Root { __typol_0_field: u8, __typol_01_padded: u8 }",
    );
    assert!(gap_output.contains("__typol_1_Typol"));
}

#[test]
fn invalid_shapes_and_borrowed_targets_are_rejected() {
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
        reject(source, message);
    }
}

#[test]
fn named_fields_without_identifiers_return_spanned_errors() {
    let mut input: DeriveInput = parse_str("struct Root { value: u8 }").unwrap();
    let Data::Struct(data) = &mut input.data else {
        panic!()
    };
    let Fields::Named(fields) = &mut data.fields else {
        panic!()
    };
    let Some(field) = fields.named.first_mut() else {
        panic!()
    };
    field.ident = None;

    let error = match Model::parse(input) {
        Ok(_) => panic!("named field without an identifier should return a macro error"),
        Err(error) => error,
    };

    assert_eq!(error.to_string(), "named field is missing an identifier");
    assert!(error.span().start().column > 0);
}

#[test]
fn attributes_names_positions_and_collisions_are_checked() {
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
        reject(source, message);
    }
    for name in [
        "", "a-b", "a::b", "9a", "café", "extern", "mod", "type", "struct", "entry", "enum",
        "option", "map", "i8", "i16", "i32", "i64", "i128", "u8", "u16", "u32", "u64", "u128",
        "f32", "f64", "string", "bool", "true", "false", "none",
    ] {
        reject(
            &format!("#[typol(rename = \"{name}\")] struct Root {{}}"),
            "non-reserved",
        );
    }
}

#[test]
fn aliases_and_unsupported_path_types_are_left_to_the_trait_contract() {
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
        let output = expand(&format!("struct Root {{ value: {ty} }}"));
        assert!(output.contains("DescriptorProvider"));
        assert!(!output.contains("BuiltinType"));
    }
    let output = expand("struct Root { children: Vec<Self> }");
    assert!(output.contains("Vec < Self >"));
    assert!(!output.contains("Vec < Self > :"));
}

#[test]
fn diagnostic_tokens_keep_the_offending_rust_span() {
    for source in [
        "struct Root { #[typol(rename = \"type\")] value: u8 }",
        "struct Root { #[typol(skip)] value: u8 }",
        "struct Root { value: &'static str }",
        "struct Root<'a> { value: u8 }",
    ] {
        let error = match Model::parse(parse_str(source).unwrap()) {
            Ok(_) => panic!(),
            Err(error) => error,
        };
        let text = error.span().source_text().unwrap();
        assert!(!text.is_empty());
        assert!(source.contains(&text));
        assert!(error.span().start().column > 0);
    }
    let error = match Model::parse(
        parse_str("struct Root { #[typol(rename = \"type\")] value: u8 }").unwrap(),
    ) {
        Ok(_) => panic!(),
        Err(error) => error,
    };
    assert_eq!(error.span().source_text().as_deref(), Some("\"type\""));
    let input: DeriveInput =
        parse_str("struct Root { a: u8, #[typol(rename = \"a\")] b: u8 }").unwrap();
    let error = match Model::parse(input) {
        Ok(_) => panic!(),
        Err(error) => error,
    };
    let errors: Vec<_> = error.into_iter().collect();
    assert_eq!(errors.len(), 2);
    assert_eq!(errors[0].span().source_text().as_deref(), Some("\"a\""));
    assert_eq!(errors[1].span().source_text().as_deref(), Some("a"));
}

#[test]
fn bounds_follow_type_positions_and_generated_imports_follow_usage() {
    let output = expand("struct Root<T> { value: module::T }");
    assert!(output.contains("impl < T >"));
    let output = expand("struct Root<T> where T: Iterator { value: <T as Iterator>::Item }");
    assert!(output.contains("T : __typol_0_Typol"));
    for source in ["struct Empty {}", "enum Empty { Unit, Object {} }"] {
        assert!(!expand(source).contains("DescriptorProvider as"));
    }
    let output = expand("enum Choice { Value(u8), Object { value: u8 } }");
    assert!(!output.contains("EnumPayload as"));
    assert!(output.contains("ConversionPayload as"));
    let output = expand("struct Root { second: u16, first: u8 }");
    assert!(output.contains(
        "read_field :: < u16 > (__typol_0_members , 0usize , __typol_0_out . reborrow ())"
    ));
    assert!(output.contains(
        "read_field :: < u8 > (__typol_0_members , 1usize , __typol_0_out . reborrow ())"
    ));
}

#[test]
fn budgeted_marker_bounds_follow_nested_and_recursive_types() {
    let output = expand("struct Parent<T> { child: Option<Vec<T>> }");
    assert!(output.contains("T : __typol_0_BudgetedTypol"));
    assert!(!output.contains("Option < Vec < T > > : __typol_0_BudgetedTypol"));

    let output = expand("struct Parent<T> where T: Iterator { child: Vec<T::Item> }");
    assert!(output.contains("T :: Item : __typol_0_BudgetedTypol"));

    let output = expand("struct Recursive { children: Vec<Self> }");
    assert!(output.contains("impl __typol_0_BudgetedTypol for Recursive"));
    assert!(!output.contains("Vec < Self > : __typol_0_BudgetedTypol"));
}

#[test]
fn trusted_derive_omits_the_budgeted_marker() {
    let output = expand("#[typol(trusted)] struct Parent { child: Manual }");
    assert!(output.contains("impl __typol_0_Typol for Parent"));
    assert!(!output.contains("BudgetedTypol"));
}

#[test]
fn rename_overrides_invalid_defaults_and_foreign_attributes_are_preserved() {
    let output = expand(
        "#[serde(rename = \"Other\")] #[typol(rename = \"Root\")] struct café { #[serde(skip)] #[typol(rename = \"value\")] r#type: u8 }",
    );
    assert!(output.contains("impl __typol_0_Typol for café"));
    assert!(output.contains("self . r#type"));
    assert!(!output.contains("serde"));
    reject("#[typol(rename = 7)] struct Root {}", "string literal");
    reject("#[typol(crate = \"\")] struct Root {}", "unexpected end");
    reject(
        "struct Root<#[typol(rename = \"Count\")] const N: usize> { values: [u8; N] }",
        "not allowed here",
    );
}
