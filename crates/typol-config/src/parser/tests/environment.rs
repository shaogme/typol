use super::{ParserLimits, ParserTestSupport, foundation::Provider};
use crate::{
    ConfigLimits, ConfigParser,
    diagnostics::ConfigParseErrors,
    environment::{Environment, EnvironmentError, EnvironmentReadContext},
};
use alloc::{format, vec::Vec};
use opeo::{OResult, Out};
use typol_core::{ConfigValue, DiagnosticCode, SliceSource, ValueKind, ValueSource, ValueView};
use typol_semantic::CompiledSchema;
use typol_test_support::*;

pub(super) struct EnvironmentTest;

impl EnvironmentTest {
    pub(super) fn run<L: Into<ParserLimits>>(
        schema: &CompiledSchema,
        source: &str,
        provider: &mut dyn Environment,
        limits: L,
    ) -> TestOpeoOutcome<ConfigValue, ConfigParseErrors> {
        let mut errors = TestSlot::default();
        match errors.run(|out| {
            ConfigParser::new(schema).parse(
                ParserTestSupport::config_request(
                    (&mut SliceSource::new(source.as_bytes()), "test"),
                    limits,
                ),
                provider,
                out,
            )
        }) {
            TestOpeoOutcome::Value(value) => TestOpeoOutcome::Value(value.into_config_value()),
            TestOpeoOutcome::Error(error) => TestOpeoOutcome::Error(error),
        }
    }
}

struct ReusingProvider {
    bytes: [u8; 1],
    calls: usize,
}

impl Environment for ReusingProvider {
    fn read<'env, 'slot, 'borrow>(
        &'env mut self,
        name: &str,
        _: EnvironmentReadContext<'_>,
        _: Out<'slot, 'borrow, EnvironmentError>,
    ) -> OResult<'slot, Option<&'env [u8]>> {
        self.calls += 1;
        self.bytes[0] = if name == "X" { b'1' } else { b'2' };
        Ok(Some(&self.bytes))
    }
}

fn provider(result: Result<Option<&'static [u8]>, EnvironmentError>) -> Provider {
    Provider::new(result)
}

fn first<'a>(value: impl Into<ValueViewInput<'a>>) -> TestResult<ValueView<'a>> {
    let mut fields = match value.into().value_view().as_ref() {
        ValueKind::Structure(fields) => fields,
        _ => {
            return Err(TestFailure::new(
                "expected the configuration value to be a struct",
            ));
        }
    };
    fields.next().test_value().map(|field| field.value())
}

#[test]
fn authorization_and_name_validation_precede_fallback_and_reads() -> TestResult<()> {
    let schema = ParserTestSupport::compile(
        r#"entry struct Root { value: u8 @env("PORT"), plain: option<u8>, }"#,
    )?;
    for expression in [
        r#"env("port")"#,
        r#"env("")"#,
        r#"env("1PORT")"#,
        r#"env("PO-RT")"#,
        r#"env("PÖRT")"#,
        r#"env("OTHER", 1)"#,
    ] {
        let mut provider = provider(Ok(Some(b"7")));
        let errors = EnvironmentTest::run(
            &schema,
            &format!("{{ value: {expression} }}"),
            &mut provider,
            &ConfigLimits::default(),
        )
        .test_error()?;
        test_eq!(
            TestCheck::item(errors.diagnostics(), 0)?.code(),
            DiagnosticCode::CONFIG_VALUE_ERROR
        )?;
        test_eq!(provider.calls(), 0)?;
    }
    let mut provider = provider(Ok(None));
    test_assert!(
        EnvironmentTest::run(
            &schema,
            r#"{ value: 1, plain: env("PORT") }"#,
            &mut provider,
            &ConfigLimits::default()
        )
        .is_err()
    )?;
    test_eq!(provider.calls(), 0)?;

    Ok(())
}

#[test]
fn same_name_converts_each_reference_and_caches_failures() -> TestResult<()> {
    let schema = ParserTestSupport::compile(
        r#"entry struct Root { text: string @env("X"), number: u8 @env("X"), }"#,
    )?;
    let source = r#"{ text: env("X"), number: env("X", 9) }"#;
    let mut valid = provider(Ok(Some(b"7")));
    let value =
        EnvironmentTest::run(&schema, source, &mut valid, &ConfigLimits::default()).test_value()?;
    let ValueKind::Structure(mut fields) = value.as_ref() else {
        return Err(TestFailure::new("expected a struct"));
    };
    test_assert!(matches!(
        fields.next().test_value()?.value().as_ref(),
        ValueKind::String("7")
    ))?;
    test_assert!(matches!(
        fields.next().test_value()?.value().as_ref(),
        ValueKind::Unsigned(7)
    ))?;
    test_eq!(valid.calls(), 1)?;
    for (result, codes) in [
        (Ok(Some(&b""[..])), &[11][..]),
        (Ok(Some(&b"bad"[..])), &[11][..]),
        (Ok(Some(&b"\xff"[..])), &[13, 13][..]),
        (Err(EnvironmentError::HostFailure), &[13, 13][..]),
        (Err(EnvironmentError::AllocationFailed), &[2][..]),
        (Err(EnvironmentError::ResourceLimitExceeded), &[1][..]),
    ] {
        let mut provider = provider(result);
        let errors = EnvironmentTest::run(&schema, source, &mut provider, &ConfigLimits::default())
            .test_error()?;
        let actual: Vec<_> = errors
            .diagnostics()
            .iter()
            .map(|error| error.code().get())
            .collect();
        test_eq!(actual, codes)?;
        test_eq!(provider.calls(), 1)?;
        test_assert!(
            errors
                .diagnostics()
                .iter()
                .all(|error| error.value_source() == Some(ValueSource::EnvironmentVariable))
        )?;
    }

    Ok(())
}

#[test]
fn missing_cache_and_all_priorities_preserve_sources() -> TestResult<()> {
    let schema = ParserTestSupport::compile(
        r#"entry struct Root {
        fallback: u8 @env("X") = 2, default: u8 @env("X") = 3,
        optional: option<u8> @env("X"), omitted: u8 @env("X") = 4,
        explicit_none: option<u8> @env("X") = 5,
    }"#,
    )?;
    let source =
        r#"{ fallback: env("X", 1), default: env("X"), optional: env("X"), explicit_none: none }"#;
    for result in [Ok(None), Ok(Some(&b"7"[..]))] {
        let mut provider = provider(result);
        let value = EnvironmentTest::run(&schema, source, &mut provider, &ConfigLimits::default())
            .test_value()?;
        let ValueKind::Structure(fields) = value.as_ref() else {
            return Err(TestFailure::new("expected a struct"));
        };
        let fields: Vec<_> = fields.collect();
        let missing = result == Ok(None);
        test_eq!(
            fields.first().test_value()?.value().source(),
            if missing {
                ValueSource::ExplicitFallback
            } else {
                ValueSource::EnvironmentVariable
            }
        )?;
        test_assert!(
            matches!(fields.first().test_value()?.value().as_ref(), ValueKind::Unsigned(value) if value == if missing {1} else {7})
        )?;
        test_eq!(
            fields.get(1).test_value()?.value().source(),
            if missing {
                ValueSource::SchemaDefault
            } else {
                ValueSource::EnvironmentVariable
            }
        )?;
        test_assert!(
            matches!(
                fields.get(2).test_value()?.value().as_ref(),
                ValueKind::Optional(None)
            ) == missing
        )?;
        test_eq!(
            fields.get(3).test_value()?.value().source(),
            ValueSource::SchemaDefault
        )?;
        test_assert!(matches!(
            fields.get(4).test_value()?.value().as_ref(),
            ValueKind::Optional(None)
        ))?;
        test_eq!(provider.calls(), 1)?;
    }
    let schema = ParserTestSupport::compile(r#"entry struct Root { value: u8 @env("X"), }"#)?;
    let mut provider = provider(Ok(None));
    let errors = EnvironmentTest::run(
        &schema,
        r#"{ value: env("X") }"#,
        &mut provider,
        &ConfigLimits::default(),
    )
    .test_error()?;
    test_eq!(
        TestCheck::item(errors.diagnostics(), 0)?.code(),
        DiagnosticCode::CONFIG_VALUE_ERROR
    )?;
    test_eq!(
        TestCheck::item(errors.diagnostics(), 0)?.value_source(),
        None
    )?;

    Ok(())
}

#[test]
fn fallback_rules_run_before_provider_even_when_unused_and_never_repeat() -> TestResult<()> {
    let schema = ParserTestSupport::compile(
        r#"type A = u8 @min(1); type O = option<A> @max(9); entry struct Root { value: O @env("X") @min(2), }"#,
    )?;
    for result in [Ok(None), Ok(Some(&b"7"[..]))] {
        let mut provider = provider(result);
        let value = EnvironmentTest::run(
            &schema,
            r#"{ value: env("X", 3) }"#,
            &mut provider,
            &ConfigLimits::default(),
        )
        .test_value()?;
        test_assert!(
            matches!(first(&value)?.as_ref(), ValueKind::Optional(Some(inner)) if matches!(inner.as_ref(), ValueKind::Unsigned(number) if number == if result == Ok(None) {3} else {7}))
        )?;
        test_eq!(provider.calls(), 1)?;
    }
    let mut provider = provider(Ok(Some(b"7")));
    let errors = EnvironmentTest::run(
        &schema,
        r#"{value: env("X", 0)}"#,
        &mut provider,
        &ConfigLimits::default(),
    )
    .test_error()?;
    test_eq!(ParserTestSupport::codes(&errors), [12, 12])?;
    test_eq!(provider.calls(), 0)?;
    provider.set_result(Ok(Some(b"10")));
    let errors = EnvironmentTest::run(
        &schema,
        r#"{value: env("X", 3)}"#,
        &mut provider,
        &ConfigLimits::default(),
    )
    .test_error()?;
    test_eq!(ParserTestSupport::codes(&errors), [12])?;
    test_eq!(
        TestCheck::item(errors.diagnostics(), 0)?.value_source(),
        Some(ValueSource::EnvironmentVariable)
    )?;
    test_eq!(provider.calls(), 1)?;

    Ok(())
}

#[test]
fn temporary_fallback_nodes_and_option_wrappers_obey_limits() -> TestResult<()> {
    let schema =
        ParserTestSupport::compile(r#"entry struct Root { value: option<string> @env("X"), }"#)?;
    for result in [Ok(None), Ok(Some(&b"set"[..]))] {
        let required = if result == Ok(None) { 3 } else { 5 };
        for budget in [required - 1, required] {
            let mut limits = ConfigLimits::default();
            limits = limits.with_max_value_nodes(budget);
            let mut provider = provider(result);
            let value = EnvironmentTest::run(
                &schema,
                r#"{ value: env("X", "fallback") }"#,
                &mut provider,
                &limits,
            );
            if budget < required {
                test_eq!(
                    TestCheck::item(value.test_error()?.diagnostics(), 0)?.code(),
                    DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
                )?;
            } else {
                test_assert!(value.is_ok())?;
            }
        }
    }

    Ok(())
}

#[test]
fn empty_strings_are_set_and_each_call_has_its_own_cache() -> TestResult<()> {
    let schema =
        ParserTestSupport::compile(r#"entry struct Root { value: option<string> @env("X"), }"#)?;
    let mut provider = provider(Ok(Some(b"")));
    for _ in 0..2 {
        let value = EnvironmentTest::run(
            &schema,
            r#"{ value: env("X", "fallback") }"#,
            &mut provider,
            &ConfigLimits::default(),
        )
        .test_value()?;
        test_assert!(
            matches!(first(&value)?.as_ref(), ValueKind::Optional(Some(inner)) if matches!(inner.as_ref(), ValueKind::String("")) && inner.source() == ValueSource::EnvironmentVariable)
        )?;
    }
    test_eq!(provider.calls(), 2)?;
    for source in [r#"{ value: none }"#, "{}"] {
        EnvironmentTest::run(&schema, source, &mut provider, &ConfigLimits::default())
            .test_value()?;
    }
    test_eq!(provider.calls(), 2)?;

    Ok(())
}

#[test]
fn cached_bytes_survive_provider_buffer_reuse() -> TestResult<()> {
    let schema = ParserTestSupport::compile(
        r#"entry struct Root { a: u8 @env("X"), b: u8 @env("Y"), c: u8 @env("X"), }"#,
    )?;
    let limits = ConfigLimits::default();
    let mut provider = ReusingProvider {
        bytes: [0],
        calls: 0,
    };
    let value = EnvironmentTest::run(
        &schema,
        r#"{ a: env("X"), b: env("Y"), c: env("X") }"#,
        &mut provider,
        &limits,
    )
    .test_value()?;
    let ValueKind::Structure(fields) = value.as_ref() else {
        return Err(TestFailure::new("expected a struct"));
    };
    for (field, expected) in fields.into_iter().zip([1, 2, 1]) {
        test_assert!(
            matches!(field.value().as_ref(), ValueKind::Unsigned(value) if value == expected)
        )?;
    }
    test_eq!(provider.calls, 2)?;

    Ok(())
}

#[test]
fn environment_entry_limits_precede_provider_reads() -> TestResult<()> {
    let schema = ParserTestSupport::compile(
        r#"entry struct Root { first: u8 @env("X"), second: u8 @env("Y"), }"#,
    )?;
    let source = r#"{ first: env("X"), second: env("Y") }"#;
    for (maximum, expected_calls) in [(0, 0), (1, 1)] {
        let limits = ConfigLimits::default().with_max_environment_entries(maximum);
        let mut provider = provider(Ok(Some(b"7")));
        let errors = EnvironmentTest::run(&schema, source, &mut provider, &limits).test_error()?;
        test_eq!(
            TestCheck::item(errors.diagnostics(), 0)?.code(),
            DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
        )?;
        test_eq!(provider.calls(), expected_calls)?;
    }

    Ok(())
}

#[test]
fn selected_fallback_and_environment_nodes_keep_distinct_positions() -> TestResult<()> {
    let schema = ParserTestSupport::compile(
        r#"entry struct Root {a: option<i8> @env("X"), b: option<u8> @env("Y")}"#,
    )?;
    let text = r#"{a: env("X", -2), b: env("Y", 3)}"#;
    for result in [Ok(None), Ok(Some(&b"7"[..]))] {
        let mut provider = provider(result);
        let (_, locations) = ParserTestSupport::parse_local_value(
            &schema,
            text,
            &mut provider,
            &ConfigLimits::default(),
        )
        .test_value()?;
        for (_, node) in locations.nodes().skip(1) {
            let actual = &text
                [node.span().test_value()?.span().start()..node.span().test_value()?.span().end()];
            if result == Ok(None) {
                test_assert!(matches!(actual, "-2" | "3"))?;
            } else {
                test_assert!(matches!(actual, r#"env("X", -2)"# | r#"env("Y", 3)"#))?;
            }
        }
        test_eq!(locations.nodes().count(), 5)?;
    }

    Ok(())
}
