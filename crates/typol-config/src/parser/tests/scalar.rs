use super::{ConfigLimits, ParserTestSupport, environment::EnvironmentTest, foundation::Provider};
use crate::diagnostics::ConfigParseErrors;
use alloc::{format, vec::Vec};
use typol_core::{ConfigValue, ValueKind, ValueSource};
use typol_policy::{ExcerptLimits, RegexExecutionLimits, RegexOptions};
use typol_semantic::{CompiledSchema, SchemaAccessError};
use typol_test_support::*;

struct IntegerWidthCase {
    ty: &'static str,
    min: i128,
    max: i128,
}

struct TextRuleCase {
    rule: &'static str,
    valid: &'static [&'static str],
    invalid: &'static [&'static str],
}

fn validate<L: Into<super::ParserLimits>>(
    schema: &CompiledSchema,
    source: &str,
    provider: &mut Provider,
    limits: L,
) -> TestOpeoOutcome<ConfigValue, ConfigParseErrors> {
    EnvironmentTest::run(schema, source, provider, limits)
}

fn parse(schema: &CompiledSchema, source: &str) -> TestOpeoOutcome<ConfigValue, ConfigParseErrors> {
    validate(
        schema,
        source,
        &mut Provider::new(Ok(None)),
        &ExcerptLimits::default(),
    )
}

#[test]
fn every_integer_width_uses_exact_closed_bounds() -> TestResult<()> {
    for case in [
        IntegerWidthCase {
            ty: "i8",
            min: i8::MIN as i128,
            max: i8::MAX as i128,
        },
        IntegerWidthCase {
            ty: "i16",
            min: i16::MIN as i128,
            max: i16::MAX as i128,
        },
        IntegerWidthCase {
            ty: "i32",
            min: i32::MIN as i128,
            max: i32::MAX as i128,
        },
        IntegerWidthCase {
            ty: "i64",
            min: i64::MIN as i128,
            max: i64::MAX as i128,
        },
        IntegerWidthCase {
            ty: "i128",
            min: i128::MIN,
            max: i128::MAX,
        },
    ] {
        let IntegerWidthCase { ty, min, max } = case;
        let schema = ParserTestSupport::compile(&format!(
            "entry struct Root {{ value: {ty} @range({}, {}), }}",
            min + 1,
            max - 1
        ))?;
        for number in [min + 1, max - 1, 0] {
            parse(&schema, &format!("{{ value: {number} }}")).test_value()?;
        }
        for number in [min, max] {
            test_eq!(
                ParserTestSupport::codes(
                    &parse(&schema, &format!("{{ value: {number} }}")).test_error()?
                ),
                [12]
            )?;
        }
    }
    for (ty, max) in [
        ("u8", u8::MAX as u128),
        ("u16", u16::MAX as u128),
        ("u32", u32::MAX as u128),
        ("u64", u64::MAX as u128),
        ("u128", u128::MAX),
    ] {
        let schema = ParserTestSupport::compile(&format!(
            "entry struct Root {{ value: {ty} @min(1) @max({}), }}",
            max - 1
        ))?;
        for number in [1, max - 1] {
            parse(&schema, &format!("{{ value: {number} }}")).test_value()?;
        }
        for number in [0, max] {
            test_eq!(
                ParserTestSupport::codes(
                    &parse(&schema, &format!("{{ value: {number} }}")).test_error()?
                ),
                [12]
            )?;
        }
    }

    Ok(())
}

#[test]
fn floating_constraints_compare_after_target_rounding() -> TestResult<()> {
    for ty in ["f32", "f64"] {
        let schema = ParserTestSupport::compile(&format!(
            "entry struct Root {{ value: {ty} @range(-1.5, 1.5), }}"
        ))?;
        for number in ["-1.5", "1.5", "-0.0", "0.0"] {
            parse(&schema, &format!("{{ value: {number} }}")).test_value()?;
        }
        for number in ["-1.6", "1.6"] {
            test_eq!(
                ParserTestSupport::codes(
                    &parse(&schema, &format!("{{ value: {number} }}")).test_error()?
                ),
                [12]
            )?;
        }
    }
    let schema = ParserTestSupport::compile(
        "entry struct Root { value: f32 @range(16777216.0, 16777216.0), }",
    )?;
    let value = parse(&schema, "{ value: 16777217.0 }").test_value()?;
    let ValueKind::Structure(fields) = value.as_ref() else {
        return Err(TestFailure::new("expected a struct"));
    };
    test_assert!(matches!(
        fields.into_iter().next().test_value()?.value().as_ref(),
        ValueKind::Float32(16777216.0)
    ))?;
    test_eq!(
        ParserTestSupport::codes(&parse(&schema, "{ value: 16777218.0 }").test_error()?),
        [12]
    )?;

    let schema = ParserTestSupport::compile("entry struct Root { value: f32, }")?;
    let value = parse(&schema, "{ value: 1.000000059604644775390626 }").test_value()?;
    let ValueKind::Structure(mut fields) = value.as_ref() else {
        return Err(TestFailure::new("expected a struct"));
    };
    let value = fields.next().test_value()?.value();
    test_assert!(matches!(
        value.as_ref(),
        ValueKind::Float32(number) if number.to_bits() == f32::from_bits(0x3f80_0001).to_bits()
    ))?;

    Ok(())
}

#[test]
fn unicode_length_counts_scalars_including_combining_marks() -> TestResult<()> {
    let schema = ParserTestSupport::compile("entry struct Root { value: string @length(2, 2), }")?;
    for value in ["éø", "🙂a", "e\u{301}"] {
        parse(&schema, &format!("{{ value: \"{value}\" }}")).test_value()?;
    }
    for value in ["🙂", "abc", ""] {
        test_eq!(
            ParserTestSupport::codes(
                &parse(&schema, &format!("{{ value: \"{value}\" }}")).test_error()?
            ),
            [12]
        )?;
    }

    Ok(())
}

#[test]
fn alias_fields_elements_and_payloads_report_each_effective_rule_once() -> TestResult<()> {
    let schema_text = r#"
        type A = string @length(2, 8);
        type B = A @regex("^a");
        type O = option<B> @length(3, 6);
        enum E { One(O @length(4, 5)), }
        entry struct Root { field: B @length(3, 6), array: [O], dict: map<string, O>, payload: E, }
    "#;
    let schema = ParserTestSupport::compile(schema_text)?;
    let errors = parse(
        &schema,
        r#"{ field: "x", array: ["x"], dict: { "key": "x" }, payload: .One("x") }"#,
    )
    .test_error()?;
    test_eq!(ParserTestSupport::codes(&errors), [12; 13])?;
    let alias_length = schema_text.find("@length(2, 8)").test_value()?;
    let regex = schema_text.find("@regex").test_value()?;
    let optional_length = schema_text.find("@length(3, 6)").test_value()?;
    let field_length = schema_text.rfind("@length(3, 6)").test_value()?;
    let payload_length = schema_text.find("@length(4, 5)").test_value()?;
    let actual: Vec<_> = errors
        .diagnostics()
        .iter()
        .map(|error| TestCheck::item(error.labels(), 0).map(|label| label.span().span().start()))
        .collect::<TestResult<Vec<_>>>()?;
    test_eq!(
        actual,
        [
            regex,
            alias_length,
            field_length,
            optional_length,
            regex,
            alias_length,
            optional_length,
            regex,
            alias_length,
            optional_length,
            regex,
            alias_length,
            payload_length,
        ]
    )?;

    test_assert!(
        errors
            .diagnostics()
            .iter()
            .all(|error| error.labels().len() == 1)
    )?;
    for errors in errors
        .diagnostics()
        .chunk_by(|left, right| left.primary() == right.primary())
    {
        for (index, error) in errors.iter().enumerate() {
            let span = TestCheck::item(error.labels(), 0)?.span();
            for prior in errors.iter().take(index) {
                test_ne!(TestCheck::item(prior.labels(), 0)?.span(), span)?;
            }
        }
    }

    Ok(())
}

#[test]
fn absent_options_skip_regex_even_with_zero_scratch_budget() -> TestResult<()> {
    let schema = ParserTestSupport::compile(
        r#"type A = string @regex("^a") @length(2, 4) @host; entry struct Root { value: option<A>, }"#,
    )?;
    let mut regex_limits = *RegexOptions::default().limits();
    regex_limits.set_max_regex_scratch_states(0);
    let limits = RegexOptions::default().with_limits(regex_limits);
    for source in ["{}", "{ value: none }"] {
        validate(&schema, source, &mut Provider::new(Ok(None)), &limits).test_value()?;
    }
    let errors = validate(
        &schema,
        r#"{ value: "abc" }"#,
        &mut Provider::new(Ok(None)),
        &limits,
    )
    .test_error()?;
    test_eq!(ParserTestSupport::codes(&errors), [1])?;

    Ok(())
}

#[test]
fn regex_mismatch_and_resource_exhaustion_are_distinct_and_budgeted() -> TestResult<()> {
    let schema = ParserTestSupport::compile(
        r#"entry struct Root { a: string @regex("^a+$"), b: string @regex("^b+$"), }"#,
    )?;
    parse(&schema, r#"{ a: "aaa", b: "bbb" }"#).test_value()?;
    let source = r#"{ a: "PRIVATE-TEXT", b: "PRIVATE-TEXT" }"#;
    let errors = parse(&schema, source).test_error()?;
    test_eq!(ParserTestSupport::codes(&errors), [12, 12])?;
    for error in errors.diagnostics() {
        test_assert!(!error.message().contains("PRIVATE-TEXT"))?;
        test_eq!(error.value_source(), Some(ValueSource::ConfigLiteral))?;
        test_eq!(error.labels().len(), 1)?;
    }
    let mut limits = RegexOptions::default()
        .with_execution(RegexExecutionLimits::default().with_max_search_steps(0));
    let errors = validate(&schema, source, &mut Provider::new(Ok(None)), &limits).test_error()?;
    test_eq!(ParserTestSupport::codes(&errors), [1])?;
    test_eq!(TestCheck::item(errors.diagnostics(), 0)?.labels().len(), 1)?;
    let mut config_limits = ConfigLimits::default();
    for (budget, expected) in [(0, &[12][..]), (1, &[12][..]), (2, &[12, 5][..])] {
        limits = limits.with_execution(
            limits
                .execution()
                .with_max_search_steps(RegexExecutionLimits::default().max_search_steps()),
        );
        config_limits = config_limits.with_max_diagnostics(budget);
        let combined = (&limits, &config_limits);
        let errors =
            validate(&schema, source, &mut Provider::new(Ok(None)), &combined).test_error()?;
        test_eq!(ParserTestSupport::codes(&errors), expected)?;
        test_assert!(errors.is_truncated())?;
    }

    Ok(())
}

#[test]
fn environment_fallbacks_validate_before_reads_and_set_values_do_not_fall_back() -> TestResult<()> {
    let schema = ParserTestSupport::compile(
        r#"entry struct Root { value: option<string> @env("X") @host, }"#,
    )?;
    let mut provider = Provider::new(Ok(Some(b"example.com")));
    let errors = validate(
        &schema,
        r#"{ value: env("X", "01.2.3.4") }"#,
        &mut provider,
        &ExcerptLimits::default(),
    )
    .test_error()?;
    test_eq!(ParserTestSupport::codes(&errors), [12])?;
    test_eq!(
        TestCheck::item(errors.diagnostics(), 0)?.value_source(),
        Some(ValueSource::ExplicitFallback)
    )?;
    test_eq!(provider.calls(), 0)?;
    provider.set_result(Ok(Some(b"256.2.3.4")));
    let errors = validate(
        &schema,
        r#"{ value: env("X", "example.com") }"#,
        &mut provider,
        &ExcerptLimits::default(),
    )
    .test_error()?;
    test_eq!(ParserTestSupport::codes(&errors), [12])?;
    test_eq!(
        TestCheck::item(errors.diagnostics(), 0)?.value_source(),
        Some(ValueSource::EnvironmentVariable)
    )?;
    test_eq!(provider.calls(), 1)?;
    provider.set_result(Ok(None));
    let value = validate(
        &schema,
        r#"{ value: env("X", "Example.COM.") }"#,
        &mut provider,
        &ExcerptLimits::default(),
    )
    .test_value()?;
    let ValueKind::Structure(mut fields) = value.as_ref() else {
        return Err(TestFailure::new("expected a struct"));
    };
    let value = fields.next().test_value()?.value();
    test_eq!(value.source(), ValueSource::ExplicitFallback)?;
    test_assert!(
        matches!(value.as_ref(), ValueKind::Optional(Some(child)) if matches!(child.as_ref(), ValueKind::String("Example.COM.")))
    )?;

    Ok(())
}

#[test]
fn nested_defaults_run_real_regex_validation_at_the_owning_schema_span() -> TestResult<()> {
    let schema = ParserTestSupport::compile(
        r#"
        type A = string @regex("^a");
        struct Child { values: map<string, [option<A>]> = { "key": ["abc", none] }, }
        enum E { One(Child), }
        entry struct Root { value: E = .One({}), }
    "#,
    )?;
    parse(&schema, "{}").test_value()?;
    let limits = RegexOptions::default()
        .with_execution(RegexExecutionLimits::default().with_max_search_steps(0));
    let errors = validate(&schema, "{}", &mut Provider::new(Ok(None)), &limits).test_error()?;
    test_eq!(ParserTestSupport::codes(&errors), [1])?;
    let mut schema_errors = TestSlot::<SchemaAccessError>::default();
    let field = ParserTestSupport::required(&mut schema_errors, |out| {
        schema.entry().field_named("value", out)
    })?;
    let error = TestCheck::item(errors.diagnostics(), 0)?;
    test_eq!(
        error.primary(),
        Some(ParserTestSupport::access(&mut schema_errors, |out| field.span(out))?)
    )?;
    test_eq!(error.value_source(), Some(ValueSource::SchemaDefault))?;
    test_assert!(
        errors
            .span_location(error.primary().test_value()?)
            .is_some()
    )?;
    test_eq!(error.path().test_value()?.segments().len(), 5)?;

    Ok(())
}

#[test]
fn text_rules_apply_through_the_parser_without_changing_strings() -> TestResult<()> {
    for case in [
        TextRuleCase {
            rule: "@host",
            valid: &["Example.COM.", "::1", "192.0.2.9"],
            invalid: &["01.2.3.4", "256.2.3.4", "a_b", "[::1]"],
        },
        TextRuleCase {
            rule: "@email",
            valid: &["a+b@example.com", "a_b@example.com"],
            invalid: &[".a@example.com", "a@localhost", "a@example.com."],
        },
        TextRuleCase {
            rule: "@cidr(v4_only = false)",
            valid: &["192.0.2.9/24", "::1/128"],
            invalid: &["::1/129", "192.0.2.9/33", "host/24"],
        },
        TextRuleCase {
            rule: "@cidr(v4_only = true)",
            valid: &["192.0.2.9/24"],
            invalid: &["::1/128"],
        },
    ] {
        let schema = ParserTestSupport::compile(&format!(
            "entry struct Root {{ value: string {}, }}",
            case.rule
        ))?;
        for text in case.valid {
            let value = parse(&schema, &format!("{{ value: \"{text}\" }}")).test_value()?;
            let ValueKind::Structure(fields) = value.as_ref() else {
                return Err(TestFailure::new("expected a struct"));
            };
            test_assert!(
                matches!(fields.into_iter().next().test_value()?.value().as_ref(), ValueKind::String(value) if value == *text)
            )?;
        }
        for text in case.invalid {
            test_eq!(
                ParserTestSupport::codes(
                    &parse(&schema, &format!("{{ value: \"{text}\" }}")).test_error()?
                ),
                [12]
            )?;
        }
    }

    Ok(())
}
