use super::{ConfigLimits, ParserTestSupport, foundation::Provider};
use crate::{ConfigParser, diagnostics::ConfigParseErrors};
use alloc::{format, string::ToString, vec, vec::Vec};
use typol_core::{ConfigValue, SliceSource, ValueKind, ValueSource};
use typol_semantic::CompiledSchema;
use typol_test_support::*;

fn run(
    schema: &CompiledSchema,
    source: &str,
    limits: &ConfigLimits,
) -> TestOpeoOutcome<ConfigValue, ConfigParseErrors> {
    let mut errors = TestSlot::default();
    match errors.run(|out| {
        ConfigParser::new(schema).parse(
            ParserTestSupport::config_request(
                (&mut SliceSource::new(source.as_bytes()), "test"),
                limits,
            ),
            &mut Provider::new(Ok(None)),
            out,
        )
    }) {
        TestOpeoOutcome::Value(value) => TestOpeoOutcome::Value(value.into_config_value()),
        TestOpeoOutcome::Error(error) => TestOpeoOutcome::Error(error),
    }
}

fn parse(schema: &CompiledSchema, source: &str) -> TestOpeoOutcome<ConfigValue, ConfigParseErrors> {
    ParserTestSupport::parse(schema, source, &ConfigLimits::default())
}

fn codes(errors: &ConfigParseErrors) -> Vec<u32> {
    errors
        .diagnostics()
        .iter()
        .map(|error| error.code().get())
        .collect()
}

#[test]
fn size_closed_bounds_empty_collections_and_optional_alias_rules() -> TestResult<()> {
    let schema = ParserTestSupport::compile(
        r#"
        type A = [i8] @size(1, 3);
        type B = A @size(2, 2);
        entry struct Root { a: option<B>, dict: map<string, i8> @size(max = 2), }
    "#,
    )?;
    for source in [
        "{ dict: {} }",
        "{ a: none, dict: {} }",
        "{ a: [1, 2], dict: {} }",
    ] {
        parse(&schema, source).test_value()?;
    }
    test_eq!(
        codes(&parse(&schema, "{ a: [], dict: {} }").test_error()?),
        [12, 12]
    )?;
    test_eq!(
        codes(&parse(&schema, r#"{ a: [1,2], dict: { "a": 1, "b": 2, "c": 3 } }"#).test_error()?),
        [12]
    )?;
    let schema = ParserTestSupport::compile(
        "entry struct Root { a: [i8] @unique @all(item > 0, \"positive\") @sum(min = 0, max = 0), }",
    )?;
    parse(&schema, "{ a: [] }").test_value()?;

    Ok(())
}

#[test]
fn unique_recurses_preserves_storage_and_reports_duplicate_source_order() -> TestResult<()> {
    let schema = ParserTestSupport::compile("entry struct Root { a: [[i8]] @unique, }")?;
    let source = "{ a: [[2], [1], [2], [1], [2]] }";
    let errors = parse(&schema, source).test_error()?;
    test_eq!(codes(&errors), [12; 3])?;
    let indices: Vec<_> = errors
        .diagnostics()
        .iter()
        .map(|error| -> TestResult<_> {
            TestCheck::item(error.path().test_value()?.segments(), 1)?
                .as_index()
                .test_value()
        })
        .collect::<TestResult<Vec<_>>>()?;
    test_eq!(indices, [2, 3, 4])?;
    let schema = ParserTestSupport::compile("entry struct Root { a: [map<string, i8>] @unique, }")?;
    test_eq!(
        codes(
            &parse(
                &schema,
                r#"{ a: [{ "a": 1, "b": 2 }, { "b": 2, "a": 1 }] }"#
            )
            .test_error()?
        ),
        [12]
    )?;
    let result = parse(&schema, r#"{ a: [{ "b": 2, "a": 1 }, { "a": 3 }] }"#).test_value()?;
    let ValueKind::Structure(fields) = result.as_ref() else {
        return Err(TestFailure::new(
            "parsed value shape does not match the test expectation",
        ));
    };
    let ValueKind::Array(mut values) = fields.into_iter().next().test_value()?.value().as_ref()
    else {
        return Err(TestFailure::new(
            "parsed value shape does not match the test expectation",
        ));
    };
    let ValueKind::Map(mut entries) = values.next().test_value()?.value().as_ref() else {
        return Err(TestFailure::new(
            "parsed value shape does not match the test expectation",
        ));
    };
    test_eq!(entries.next().test_value()?.name(), Some("b"))?;
    let schema = ParserTestSupport::compile(
        "enum E { A(i8), B { x: i8, }, C, } entry struct Root { a: [E] @unique, }",
    )?;
    parse(&schema, "{ a: [.A(1), .A(2), .B { x: 1 }, .C] }").test_value()?;
    test_eq!(
        codes(
            &parse(
                &schema,
                "{ a: [.A(1), .B { x: 1 }, .C, .A(1), .B { x: 1 }, .C] }"
            )
            .test_error()?
        ),
        [12; 3]
    )?;
    let schema = ParserTestSupport::compile("entry struct Root { a: [f64] @unique, }")?;
    test_eq!(
        codes(&parse(&schema, "{ a: [0.0, -0.0] }").test_error()?),
        [12]
    )?;

    Ok(())
}

#[test]
fn unique_by_nested_optional_host_uses_exact_text_identity() -> TestResult<()> {
    let schema = ParserTestSupport::compile(
        r#"
        struct Key { host: option<string> @host, }
        struct Item { key: Key, other: i8, }
        entry struct Root { a: [Item] @unique(by = "key.host"), }
    "#,
    )?;
    for (pair, duplicate) in [
        (r#""Example.COM.", "example.com""#, false),
        (r#""2001:db8::1", "2001:0db8:0:0:0:0:0:1""#, false),
        ("none, none", true),
    ] {
        let (left, right) = pair.split_once(", ").test_value()?;
        let parsed = parse(
            &schema,
            &format!(
                "{{ a: [{{ key: {{ host: {left} }}, other: 1 }}, {{ key: {{ host: {right} }}, other: 2 }}] }}"
            ),
        );
        if duplicate {
            test_eq!(codes(&parsed.test_error()?), [12])?;
        } else {
            parsed.test_value()?;
        }
    }
    let schema = ParserTestSupport::compile(
        r#"type H = string @host; entry struct Root { a: [option<H>] @unique, }"#,
    )?;
    parse(&schema, r#"{ a: ["EXAMPLE.com", "example.com."] }"#).test_value()?;

    Ok(())
}

#[test]
fn all_map_errors_follow_rules_then_source_order_with_item_spans() -> TestResult<()> {
    let schema = ParserTestSupport::compile(
        r#"type M = map<string, i8> @all(value > 0, "positive"); entry struct Root { a: M @all(key.starts_with("x"), "key"), }"#,
    )?;
    let source = r#"{ a: { "z": -1, "a": -2, "x": 3 } }"#;
    let errors = parse(&schema, source).test_error()?;
    test_eq!(codes(&errors), [12; 4])?;
    let keys: Vec<_> = errors
        .diagnostics()
        .iter()
        .map(|error| -> TestResult<_> {
            TestCheck::item(error.path().test_value()?.segments(), 1)?
                .as_map_key()
                .test_value()
        })
        .collect::<TestResult<Vec<_>>>()?;
    test_eq!(keys, ["z", "z", "a", "a"])?;
    let starts: Vec<_> = errors
        .diagnostics()
        .iter()
        .map(|error| error.primary().map(|span| span.span().start()))
        .collect();
    let mut sorted_starts = starts.clone();
    sorted_starts.sort_unstable();
    test_eq!(starts, sorted_starts)?;
    for error in errors.diagnostics() {
        test_eq!(error.labels().len(), 1)?;
        test_eq!(error.value_source(), Some(ValueSource::ConfigLiteral))?;
    }
    let schema = ParserTestSupport::compile(
        r#"entry struct Root { a: [i8] @all(item == 0 || 1 / item > 0, "division"), }"#,
    )?;
    parse(&schema, "{ a: [0, 1] }").test_value()?;
    let errors = parse(&schema, "{ a: [-1, -2] }").test_error()?;
    test_eq!(codes(&errors), [12; 2])?;

    Ok(())
}

#[test]
fn sum_uses_wide_intermediate_and_float_target_rounding() -> TestResult<()> {
    let schema =
        ParserTestSupport::compile("entry struct Root { a: [i8] @sum(min = 127, max = 127), }")?;
    parse(&schema, "{ a: [127, 1, -1] }").test_value()?;
    test_eq!(
        codes(&parse(&schema, "{ a: [127, 1] }").test_error()?),
        [12]
    )?;
    let schema = ParserTestSupport::compile(&format!(
        "entry struct Root {{ a: [i128] @sum(min = {}, max = {}), }}",
        i128::MAX,
        i128::MAX
    ))?;
    parse(&schema, &format!("{{ a: [{0}, {0}, -{0}] }}", i128::MAX)).test_value()?;
    let schema = ParserTestSupport::compile("entry struct Root { a: [u128] @sum(min = 0), }")?;
    test_eq!(
        codes(&parse(&schema, &format!("{{ a: [{}, 1] }}", u128::MAX)).test_error()?),
        [12]
    )?;
    let schema =
        ParserTestSupport::compile("entry struct Root { a: [f32] @sum(min = 0.0, max = 0.0), }")?;
    parse(&schema, "{ a: [16777216.0, 1.0, -16777216.0] }").test_value()?;
    test_eq!(codes(&parse(&schema, "{ a: [340000000000000000000000000000000000000.0, 340000000000000000000000000000000000000.0, -340000000000000000000000000000000000000.0] }").test_error()?), [12])?;

    Ok(())
}

#[test]
fn full_tree_assertions_visit_every_instance_default_and_active_payload() -> TestResult<()> {
    let schema = ParserTestSupport::compile(
        r#"
        struct Child { n: i8, } @assert(self.n > root.limit, "child")
        type Alias = Child;
        enum E {
            Unit @assert(root.limit < 0, "unit"),
            Single(Child) @assert(self.n > root.limit, "single"),
            Named { n: i8, } @assert(self.n > root.limit, "named"),
            Inactive @assert(false, "inactive"),
        }
        entry struct Root {
            limit: i8,
            default: Child = { n: 1 },
            optional: option<Alias>,
            array: [Child],
            dict: map<string, Child>,
            e: E,
        } @assert(root.limit < 0, "root")
    "#,
    )?;
    let source = r#"{ e: .Single({ n: 1 }), dict: { "z": { n: 1 } }, array: [{ n: 1 }, { n: 2 }], optional: { n: 1 }, limit: 3 }"#;
    let errors = parse(&schema, source).test_error()?;
    test_eq!(codes(&errors), [12; 8])?;
    let messages: Vec<_> = errors
        .diagnostics()
        .iter()
        .map(|error| error.message())
        .collect();
    let mut sorted_messages = messages;
    sorted_messages.sort_unstable();
    test_eq!(
        sorted_messages,
        [
            "child", "child", "child", "child", "child", "child", "root", "single"
        ]
    )?;
    test_assert!(
        errors
            .diagnostics()
            .iter()
            .all(|error| error.value_source().is_some())
    )?;
    test_eq!(
        errors
            .diagnostics()
            .iter()
            .filter(|error| error.value_source() == Some(ValueSource::SchemaDefault))
            .count(),
        1
    )?;
    let default_diagnostic = errors
        .diagnostics()
        .iter()
        .find(|error| error.value_source() == Some(ValueSource::SchemaDefault))
        .test_value()?;
    test_ne!(
        default_diagnostic.primary().test_value()?.source_id(),
        errors.config_source_id().test_value()?
    )?;
    for (variant, message) in [(".Unit", "unit"), (".Named { n: 1 }", "named")] {
        let errors = parse(
            &schema,
            &format!("{{ e: {variant}, dict: {{}}, array: [], limit: 3 }}"),
        )
        .test_error()?;
        let messages: Vec<_> = errors
            .diagnostics()
            .iter()
            .map(|error| error.message())
            .collect();
        let mut sorted_messages = messages;
        sorted_messages.sort_unstable();
        let mut expected = vec!["root", message, "child"];
        expected.sort_unstable();
        test_eq!(sorted_messages, expected)?;
    }
    parse(&schema, "{ e: .Unit, dict: {}, array: [], limit: -1 }").test_value()?;

    Ok(())
}

#[test]
fn incomplete_roots_skip_assertions_and_shared_budget_stops() -> TestResult<()> {
    let schema = ParserTestSupport::compile(
        r#"entry struct Root { a: [i8] @all(item > 0, "all"), b: i8, } @assert(false, "assertion") @assert(1 / 0 > 0, "division")"#,
    )?;
    test_eq!(
        codes(&parse(&schema, "{ a: [-1, -2], b: 1 }").test_error()?),
        [12; 2]
    )?;
    test_eq!(
        codes(&parse(&schema, "{ a: [], b: 128 }").test_error()?),
        [11]
    )?;
    test_eq!(
        codes(&parse(&schema, "{ a: [], b: 1 } extra").test_error()?),
        [10]
    )?;
    test_eq!(
        codes(&parse(&schema, "{ a: [], b: 1 }").test_error()?),
        [12; 2]
    )?;
    for (budget, expected) in [(0, &[12][..]), (1, &[12][..]), (2, &[12, 5][..])] {
        let mut limits = ConfigLimits::default();
        limits = limits.with_max_diagnostics(budget);
        for source in ["{ a: [-1, -2], b: 1 }", "{ a: [], b: 1 }"] {
            let errors = run(&schema, source, &limits).test_error()?;
            test_eq!(codes(&errors), expected)?;
            test_assert!(errors.is_truncated())?;
        }
    }

    Ok(())
}

#[test]
fn large_unique_container_measurement() -> TestResult<()> {
    let schema = ParserTestSupport::compile("entry struct Root { a: [u64] @unique, }")?;
    for count in [1000_usize, 2000] {
        let values = (0..count)
            .map(|number| number.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let source = format!("{{ a: [{values}] }}");
        parse(&schema, &source).test_value()?;
    }

    Ok(())
}

#[test]
fn default_containers_and_variant_payloads_keep_root_and_schema_locations() -> TestResult<()> {
    let schema = ParserTestSupport::compile(
        r#"
        struct Child { n: i8, } @assert(self.n > root.limit, "child")
        enum E { One(Child) @assert(self.n > root.limit, "variant"), }
        entry struct Root {
            limit: i8,
            objects: [option<Child>] = [{ n: 1 }, none, { n: 2 }],
            dict: map<string, Child> = { "z": { n: 1 }, "a": { n: 2 } },
            e: E = .One({ n: 1 }),
        }
    "#,
    )?;
    let errors = parse(&schema, "{ limit: 3 }").test_error()?;
    test_eq!(codes(&errors), [12; 6])?;
    let config_source_id = errors.config_source_id().test_value()?;
    for error in errors.diagnostics() {
        test_eq!(error.value_source(), Some(ValueSource::SchemaDefault))?;
        test_ne!(error.primary().test_value()?.source_id(), config_source_id)?;
    }
    let paths: Vec<_> = errors
        .diagnostics()
        .iter()
        .map(|error| -> TestResult<_> { Ok(error.path().test_value()?.segments()) })
        .collect::<TestResult<Vec<_>>>()?;
    test_eq!(
        TestCheck::item(TestCheck::item(&paths, 0)?, 1)?.as_index(),
        Some(0)
    )?;
    test_eq!(
        TestCheck::item(TestCheck::item(&paths, 1)?, 1)?.as_index(),
        Some(2)
    )?;
    test_eq!(
        TestCheck::item(TestCheck::item(&paths, 2)?, 1)?.as_map_key(),
        Some("z")
    )?;
    test_eq!(
        TestCheck::item(TestCheck::item(&paths, 3)?, 1)?.as_map_key(),
        Some("a")
    )?;
    test_eq!(TestCheck::item(&paths, 4)?.len(), 1)?;
    test_eq!(
        TestCheck::item(TestCheck::item(&paths, 5)?, 1)?.as_enum_payload(),
        Some("One")
    )?;
    parse(&schema, "{ limit: 0 }").test_value()?;

    Ok(())
}

#[test]
fn inherited_all_rules_and_arithmetic_failures_execute_once_per_item() -> TestResult<()> {
    let schema = ParserTestSupport::compile(
        r#"
        type A = [i8] @all(item > 0, "alias");
        type B = A @all(item < 2, "outer");
        entry struct Root { a: option<B>, }
    "#,
    )?;
    let errors = parse(&schema, "{ a: [-1, 3] }").test_error()?;
    test_eq!(codes(&errors), [12; 2])?;
    let messages: Vec<_> = errors
        .diagnostics()
        .iter()
        .map(|error| error.message())
        .collect();
    test_eq!(messages, ["alias", "outer"])?;
    let starts: Vec<_> = errors
        .diagnostics()
        .iter()
        .map(|error| error.primary().map(|span| span.span().start()))
        .collect();
    let mut sorted_starts = starts.clone();
    sorted_starts.sort_unstable();
    test_eq!(starts, sorted_starts)?;
    let schema = ParserTestSupport::compile(
        r#"entry struct Root { a: [i8] @all(1 / item > 0, "division"), }"#,
    )?;
    test_eq!(
        codes(&parse(&schema, "{ a: [0, 0] }").test_error()?),
        [12; 2]
    )?;

    Ok(())
}

#[test]
fn single_payload_assertions_bind_scalar_option_and_host_metadata() -> TestResult<()> {
    let schema = ParserTestSupport::compile(
        r#"
        enum E {
            Host(string @host) @assert(self == root.host, "host"),
            Optional(option<i8>) @assert(self.is_some(), "optional"),
            Number(i8) @assert(self > root.limit, "number"),
        }
        entry struct Root { host: string @host, limit: i8, e: E, }
    "#,
    )?;
    for variant in [r#".Host("example.com")"#, ".Optional(1)", ".Number(1)"] {
        parse(
            &schema,
            &format!(r#"{{ host: "example.com", limit: 0, e: {variant} }}"#),
        )
        .test_value()?;
    }
    for variant in [r#".Host("other.com")"#, ".Optional(none)", ".Number(0)"] {
        let errors = parse(
            &schema,
            &format!(r#"{{ host: "example.com", limit: 0, e: {variant} }}"#),
        )
        .test_error()?;
        test_eq!(codes(&errors), [12])?;
        test_eq!(
            TestCheck::item(errors.diagnostics(), 0)?.value_source(),
            Some(ValueSource::ConfigLiteral)
        )?;
    }

    Ok(())
}
