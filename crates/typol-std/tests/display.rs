#[macro_use]
#[path = "support/assertions.rs"]
mod assertions;

#[path = "support/display.rs"]
mod support;

use std::{error::Error, io, thread::Builder};

use typol_core::{AllocationError, ExecutionLimits, ValueError, ValueKind, ValueSource};
use typol_policy::ValueLimits;
use typol_schema::{SchemaCompileErrors, SchemaCompileOptions, SchemaCompiler, SchemaSource};
use typol_std::{DisplayError, DisplayOptions, opeo::ErrSlot};

use support::{ConfigValue, DisplayTest, EnumPayload, FieldValue, MapEntry, NoImports};

struct NumberCase {
    ty: &'static str,
    value: ConfigValue,
    expected: String,
}

#[test]
fn all_secret_sources_are_hidden_and_original_values_remain_owned() -> Result<(), Box<dyn Error>> {
    let schema = DisplayTest::compile(
        r#"entry struct Root { literal: string @secret, environment: string @secret @env("X"), fallback: string @secret @env("Y"), defaulted: string @secret = "PRIVATE-DEFAULT" }"#,
    )?;
    let value = DisplayTest::parse(
        &schema,
        r#"{literal: "PRIVATE-LITERAL", environment: env("X"), fallback: env("Y", "PRIVATE-FALLBACK")}"#,
    )?;
    let text = DisplayTest::format(&schema, &value)?;
    ensure_eq!(
        text,
        "{literal: ********, environment: ********, fallback: ********, defaulted: ********}"
    );
    let ValueKind::Structure(fields) = value.as_ref() else {
        return Err(io::Error::other("value shape is not a structure").into());
    };
    for (field, source) in fields.zip([
        ValueSource::ConfigLiteral,
        ValueSource::EnvironmentVariable,
        ValueSource::ExplicitFallback,
        ValueSource::SchemaDefault,
    ]) {
        ensure_eq!(field.value().source(), source);
        ensure!(matches!(field.value().as_ref(), ValueKind::String(s) if s.starts_with("PRIVATE")));
    }
    let schema = DisplayTest::compile("entry struct Root { value: string @secret }")?;
    let value = DisplayTest::root(
        ConfigValue::string_unmetered("PRIVATE-RUST".into()).with_source(ValueSource::RustValue),
    )?;
    ensure_eq!(DisplayTest::format(&schema, &value)?, "{value: ********}");

    Ok(())
}

#[test]
fn aliases_options_secret_elements_and_single_payloads_hide_entire_boundaries()
-> Result<(), Box<dyn Error>> {
    for target in [
        "B",
        "option<B>",
        "[B]",
        "[option<B>]",
        "map<string, B>",
        "map<string, option<B>>",
    ] {
        let schema = DisplayTest::compile(&format!(
            "type A = string @secret; type B = A; entry struct Root {{ value: {target} }}"
        ))?;
        // Error shapes inside a secret boundary are not inspected; redaction does not depend on the original value’s shape, length, or presence.
        for value in [
            ConfigValue::none(),
            ConfigValue::boolean(true),
            ConfigValue::string_unmetered("PRIVATE".repeat(1000)),
            ErrSlot::<ValueError>::new().call(|out| {
                ConfigValue::map_unmetered(
                    vec![MapEntry::new_unmetered(
                        "PRIVATE-KEY".into(),
                        ConfigValue::signed(999),
                    )],
                    out,
                )
            })?,
        ] {
            ensure_eq!(
                DisplayTest::format(&schema, &DisplayTest::root(value)?)?,
                "{value: ********}"
            );
        }
    }
    for payload in ["string @secret", "S", "option<S>"] {
        let schema = DisplayTest::compile(&format!(
            "type S = string @secret; enum E {{ Single({payload}), Unit }} entry struct Root {{ value: E }}"
        ))?;
        let value = DisplayTest::root(ConfigValue::enumeration_unmetered(
            "Single".into(),
            EnumPayload::unit(),
        )?)?;
        ensure_eq!(DisplayTest::format(&schema, &value)?, "{value: ********}");
    }
    for target in [
        "[string] @secret",
        "map<string, string> @secret",
        "option<string> @secret",
        "E @secret",
    ] {
        let schema = DisplayTest::compile(&format!(
            "enum E {{ Unit }} entry struct Root {{ value: {target} }}"
        ))?;
        ensure_eq!(
            DisplayTest::format(&schema, &DisplayTest::root(ConfigValue::signed(1))?)?,
            "{value: ********}"
        );
    }
    let schema =
        DisplayTest::compile("enum E { Hidden(u8 @secret), Unit } entry struct Root { value: E }")?;
    ensure_eq!(
        DisplayTest::format(
            &schema,
            &DisplayTest::root(ConfigValue::enumeration_unmetered(
                "Hidden".into(),
                EnumPayload::unit()
            )?)?
        )?,
        "{value: ********}"
    );

    Ok(())
}

#[test]
fn public_layout_uses_declaration_order_sorted_keys_and_three_enum_shapes()
-> Result<(), Box<dyn Error>> {
    let schema = DisplayTest::compile(
        "enum E { Unit, Single(u8), Object { hidden: string @secret, visible: bool } } entry struct Root { a: option<u8>, b: [u8; 2], c: map<string, E> }",
    )?;
    let value = DisplayTest::parse(
        &schema,
        r#"{c: {"Ḁ": .Object {visible: true, hidden: "PRIVATE"}, "a": .Single(2), "z": .Unit}, b: [1, 2], a: 3}"#,
    )?;
    ensure_eq!(
        DisplayTest::format(&schema, &value)?,
        "{a: 3, b: [1, 2], c: {\"a\": .Single(2), \"z\": .Unit, \"Ḁ\": .Object {hidden: ********, visible: true}}}"
    );
    let ValueKind::Structure(mut fields) = value.as_ref() else {
        return Err(io::Error::other("value shape is not a structure").into());
    };
    let _first_field = fields
        .next()
        .ok_or_else(|| io::Error::other("structure is missing its first field"))?;
    ensure_eq!(
        _first_field
            .name()
            .ok_or_else(|| io::Error::other("structure field name is unavailable"))?,
        "a"
    );
    let map_field = fields
        .nth(1)
        .ok_or_else(|| io::Error::other("structure is missing its map field"))?;
    let ValueKind::Map(mut entries) = map_field.value().as_ref() else {
        return Err(io::Error::other("field value shape is not a map").into());
    };
    let first_entry = entries
        .next()
        .ok_or_else(|| io::Error::other("map entry is missing"))?;
    ensure_eq!(
        first_entry
            .name()
            .ok_or_else(|| io::Error::other("map key is unavailable"))?,
        "Ḁ"
    );
    let schema = DisplayTest::compile("entry struct Root { value: option<u8> }")?;
    ensure_eq!(
        DisplayTest::format(&schema, &DisplayTest::root(ConfigValue::none())?)?,
        "{value: none}"
    );

    Ok(())
}

fn mismatch(target: &str, value: ConfigValue) -> Result<(), Box<dyn Error>> {
    let schema = DisplayTest::compile(&format!(
        "enum E {{ Unit, Single(u8), Object {{ x: bool }} }} entry struct Root {{ value: {target} }}"
    ))?;
    let error = DisplayTest::format_with_options(
        &schema,
        &DisplayTest::root(value.with_source(ValueSource::RustValue))?,
        DisplayOptions::default(),
    )
    .err()
    .ok_or_else(|| io::Error::other("expected operation to fail"))?;
    ensure_eq!(error, DisplayError::ShapeMismatch);
    ensure!(!format!("{error:?} {error}").contains("PRIVATE"));
    ensure!(error.source().is_none());
    Ok(())
}

#[test]
fn foreign_shapes_names_widths_and_payloads_fail_closed() -> Result<(), Box<dyn Error>> {
    mismatch("u8", ConfigValue::signed(1))?;
    mismatch("i8", ConfigValue::signed(128))?;
    mismatch("u8", ConfigValue::unsigned(256))?;
    mismatch("bool", ConfigValue::unsigned(1))?;
    mismatch("string", ConfigValue::boolean(true))?;
    mismatch("f32", ConfigValue::float64(1.0))?;
    mismatch("f64", ConfigValue::float32(1.0))?;
    mismatch("f32", ConfigValue::float32(f32::NAN))?;
    mismatch("f64", ConfigValue::float64(f64::INFINITY))?;
    mismatch("option<u8>", ConfigValue::unsigned(1))?;
    mismatch(
        "[u8; 2]",
        ConfigValue::array_unmetered(vec![ConfigValue::unsigned(1)])?,
    )?;
    mismatch(
        "E",
        ConfigValue::enumeration_unmetered("PRIVATE-VARIANT".into(), EnumPayload::unit())?,
    )?;
    mismatch(
        "E",
        ConfigValue::enumeration_unmetered("Single".into(), EnumPayload::unit())?,
    )?;
    mismatch(
        "E",
        ConfigValue::enumeration_unmetered(
            "Unit".into(),
            ErrSlot::<AllocationError>::new()
                .call(|out| EnumPayload::single_unmetered(ConfigValue::unsigned(1), out))?,
        )?,
    )?;
    mismatch(
        "E",
        ConfigValue::enumeration_unmetered(
            "Object".into(),
            ErrSlot::<ValueError>::new().call(|out| {
                EnumPayload::structure_unmetered(
                    vec![FieldValue::new_unmetered(
                        "PRIVATE".into(),
                        ConfigValue::boolean(true),
                    )],
                    out,
                )
            })?,
        )?,
    )?;
    let schema = DisplayTest::compile("entry struct Root { value: string @secret }")?;
    for value in [
        ErrSlot::<ValueError>::new().call(|out| ConfigValue::structure_unmetered(vec![], out))?,
        ErrSlot::<ValueError>::new().call(|out| {
            ConfigValue::structure_unmetered(
                vec![FieldValue::new_unmetered(
                    "PRIVATE".into(),
                    ConfigValue::string_unmetered("PRIVATE".into()),
                )],
                out,
            )
        })?,
    ] {
        let value = DisplayTest::build(value)?;
        ensure_eq!(
            DisplayTest::format_with_options(&schema, &value, DisplayOptions::default()),
            Err(DisplayError::ShapeMismatch)
        );
    }
    let other = DisplayTest::compile("entry struct Other { foreign: string @secret }")?;
    let value = DisplayTest::parse(&other, "{foreign: \"PRIVATE\"}")?;
    ensure_eq!(
        DisplayTest::format_with_options(&schema, &value, DisplayOptions::default()),
        Err(DisplayError::ShapeMismatch)
    );

    Ok(())
}

#[test]
fn display_escapes_all_terminal_controls_and_formats_target_width_numbers()
-> Result<(), Box<dyn Error>> {
    let schema = DisplayTest::compile("entry struct Root { value: string }")?;
    let raw = "\"\\\n\r\t\0\u{1b}\u{85}\u{2028}\u{202e}\u{2069}";
    ensure_eq!(
        DisplayTest::format(
            &schema,
            &DisplayTest::root(ConfigValue::string_unmetered(raw.into()))?
        )?,
        "{value: \"\\\"\\\\\\n\\r\\t\\u{0}\\u{1b}\\u{85}\\u{2028}\\u{202e}\\u{2069}\"}"
    );
    for code in (0..=31)
        .chain(127..=159)
        .chain(0x2028..=0x202e)
        .chain(0x2066..=0x2069)
    {
        let c = char::from_u32(code)
            .ok_or_else(|| io::Error::other("control code conversion failed"))?;
        ensure!(
            !DisplayTest::format(
                &schema,
                &DisplayTest::root(ConfigValue::string_unmetered(c.to_string()))?
            )?
            .contains(c)
        );
    }
    let schema = DisplayTest::compile("entry struct Root { value: map<string, string> }")?;
    let formatted = DisplayTest::format(
        &schema,
        &DisplayTest::root(ErrSlot::<ValueError>::new().call(|out| {
            ConfigValue::map_unmetered(
                vec![MapEntry::new_unmetered(
                    raw.into(),
                    ConfigValue::string_unmetered(raw.into()),
                )],
                out,
            )
        })?)?,
    )?;
    ensure!(!formatted.contains('\u{1b}'));
    let cases = [
        NumberCase {
            ty: "i128",
            value: ConfigValue::signed(i128::MIN),
            expected: i128::MIN.to_string(),
        },
        NumberCase {
            ty: "u128",
            value: ConfigValue::unsigned(u128::MAX),
            expected: u128::MAX.to_string(),
        },
        NumberCase {
            ty: "f32",
            value: ConfigValue::float32(-0.0),
            expected: "-0.0".into(),
        },
        NumberCase {
            ty: "f64",
            value: ConfigValue::float64(1e-10),
            expected: "0.0000000001".into(),
        },
    ];
    for case in cases {
        let schema = DisplayTest::compile(&format!("entry struct Root {{ value: {} }}", case.ty))?;
        ensure_eq!(
            DisplayTest::format(&schema, &DisplayTest::root(case.value)?)?,
            format!("{{value: {}}}", case.expected)
        );
    }

    Ok(())
}

#[test]
fn exact_output_nodes_depth_string_steps_and_zero_limits_are_enforced() -> Result<(), Box<dyn Error>>
{
    let schema = DisplayTest::compile("entry struct Root { value: option<string> }")?;
    let value = DisplayTest::root(ErrSlot::<AllocationError>::new().call(|out| {
        ConfigValue::some_unmetered(ConfigValue::string_unmetered("Ḁ".into()), out)
    })?)?;
    let text = DisplayTest::format(&schema, &value)?;
    let mut resources = ValueLimits::default();
    resources.set_max_nesting_depth(2);
    resources.set_max_value_nodes(3);
    resources.set_max_string_bytes(3);
    let exact = DisplayOptions::new(resources, DisplayOptions::default().execution(), text.len());
    ensure_eq!(
        DisplayTest::format_with_options(&schema, &value, exact)?,
        text
    );
    for resources in [
        {
            let mut r = resources;
            r.set_max_nesting_depth(1);
            r
        },
        {
            let mut r = resources;
            r.set_max_value_nodes(2);
            r
        },
        {
            let mut r = resources;
            r.set_max_string_bytes(2);
            r
        },
    ] {
        ensure_eq!(
            DisplayTest::format_with_options(
                &schema,
                &value,
                DisplayOptions::new(resources, exact.execution(), text.len())
            ),
            Err(DisplayError::ResourceLimitExceeded)
        );
    }
    ensure_eq!(
        DisplayTest::format_with_options(
            &schema,
            &value,
            DisplayOptions::new(resources, exact.execution(), text.len() - 1)
        ),
        Err(DisplayError::ResourceLimitExceeded)
    );
    for execution in [ExecutionLimits::new(0, 8192), ExecutionLimits::new(8192, 0)] {
        ensure_eq!(
            DisplayTest::format_with_options(
                &schema,
                &value,
                DisplayOptions::new(resources, execution, text.len())
            ),
            Err(DisplayError::ResourceLimitExceeded)
        );
    }
    let mut low = 0;
    let mut high = 10_000;
    while low < high {
        let middle = low + (high - low) / 2;
        let result = DisplayTest::format_with_options(
            &schema,
            &value,
            DisplayOptions::new(resources, ExecutionLimits::new(middle, 8192), text.len()),
        );
        if result.is_ok() {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    ensure!(
        DisplayTest::format_with_options(
            &schema,
            &value,
            DisplayOptions::new(resources, ExecutionLimits::new(low, 8192), text.len())
        )
        .is_ok()
    );
    ensure_eq!(
        DisplayTest::format_with_options(
            &schema,
            &value,
            DisplayOptions::new(resources, ExecutionLimits::new(low - 1, 8192), text.len())
        ),
        Err(DisplayError::ResourceLimitExceeded)
    );

    Ok(())
}

#[test]
fn deep_tree_uses_explicit_frames_on_a_small_stack() -> Result<(), Box<dyn Error>> {
    let depth = 512;
    let declarations = (0..depth)
        .map(|i| format!("struct N{i} {{ child: N{} }} ", i + 1))
        .collect::<String>();
    let mut resources = ValueLimits::default();
    resources.set_max_nesting_depth(2048);
    let compiler = ErrSlot::<SchemaCompileErrors>::new()
        .call(|out| SchemaCompiler::new(SchemaCompileOptions::default(), out))
        .map_err(|error| io::Error::other(format!("{error:?}")))?;
    let schema = ErrSlot::<SchemaCompileErrors>::new().call(|out| compiler.compile(
            SchemaSource::from_str(
                b"root",
                &format!("{declarations} struct N{depth} {{ child: string @secret }} entry struct Root {{ value: N0 }}"),
            ),
            &mut NoImports,
            None,
            out,
        ))
        .map_err(|error| io::Error::other(format!("{error:?}")))?;
    let mut value = ConfigValue::string_unmetered("PRIVATE".into());
    for _ in 0..=depth {
        value = ErrSlot::<ValueError>::new().call(|out| {
            ConfigValue::structure_unmetered(
                vec![FieldValue::new_unmetered("child".into(), value)],
                out,
            )
        })?;
    }
    let value = DisplayTest::root(value)?;
    Builder::new()
        .stack_size(128 * 1024)
        .spawn(move || -> Result<(), io::Error> {
            let text = DisplayTest::format_with_options(
                &schema,
                &value,
                DisplayOptions::new(resources, DisplayOptions::default().execution(), 1_048_576),
            )
            .map_err(|error| io::Error::other(format!("{error:?}")))?;
            ensure!(!text.contains("PRIVATE"));
            ensure_eq!(text.matches("********").count(), 1);
            Ok(())
        })?
        .join()
        .map_err(|_| io::Error::other("deep formatting thread panicked"))??;

    Ok(())
}

#[test]
fn map_keys_count_as_nodes_and_secret_interiors_consume_no_structural_allowance()
-> Result<(), Box<dyn Error>> {
    let schema = DisplayTest::compile("entry struct Root { value: map<string, bool> }")?;
    let value = DisplayTest::root(ErrSlot::<ValueError>::new().call(|out| {
        ConfigValue::map_unmetered(
            vec![MapEntry::new_unmetered(
                "key".into(),
                ConfigValue::boolean(true),
            )],
            out,
        )
    })?)?;
    let mut resources = ValueLimits::default();
    resources.set_max_value_nodes(4);
    let options = DisplayOptions::new(resources, DisplayOptions::default().execution(), 1024);
    ensure!(DisplayTest::format_with_options(&schema, &value, options).is_ok());
    resources.set_max_value_nodes(3);
    ensure_eq!(
        DisplayTest::format_with_options(
            &schema,
            &value,
            DisplayOptions::new(resources, options.execution(), 1024)
        ),
        Err(DisplayError::ResourceLimitExceeded)
    );
    let hidden = DisplayTest::compile("entry struct Root { value: map<string, bool> @secret }")?;
    resources.set_max_value_nodes(2);
    resources.set_max_nesting_depth(1);
    resources.set_max_string_bytes(0);
    ensure_eq!(
        DisplayTest::format_with_options(
            &hidden,
            &value,
            DisplayOptions::new(resources, options.execution(), 1024)
        )?,
        "{value: ********}"
    );

    Ok(())
}

#[test]
fn arbitrary_public_field_order_is_mapped_and_business_constraints_are_not_reexecuted()
-> Result<(), Box<dyn Error>> {
    let schema =
        DisplayTest::compile("entry struct Root { first: u8 @min(10), second: string @secret }")?;
    let value = ErrSlot::<ValueError>::new().call(|out| {
        ConfigValue::structure_unmetered(
            vec![
                FieldValue::new_unmetered(
                    "second".into(),
                    ConfigValue::string_unmetered("PRIVATE".into()),
                ),
                FieldValue::new_unmetered("first".into(), ConfigValue::unsigned(1)),
            ],
            out,
        )
    })?;
    let value = DisplayTest::build(value)?;
    ensure_eq!(
        DisplayTest::format(&schema, &value)?,
        "{first: 1, second: ********}"
    );
    let schema =
        DisplayTest::compile("entry struct Root { value: E } enum E { Object { x: bool } }")?;
    let value = DisplayTest::root(ConfigValue::enumeration_unmetered(
        "Object".into(),
        ErrSlot::<ValueError>::new().call(|out| EnumPayload::structure_unmetered(vec![], out))?,
    )?)?;
    ensure_eq!(
        DisplayTest::format_with_options(&schema, &value, DisplayOptions::default()),
        Err(DisplayError::ShapeMismatch)
    );

    Ok(())
}
