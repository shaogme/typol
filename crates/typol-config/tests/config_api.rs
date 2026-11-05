use typol_test_support::ConfigTestSupport as Support;

use typol_config::{
    ConfigLimits, ConfigParseOptions, ConfigParseRequest, ConfigParser, ConfigValidationOptions,
    ConfigValidationRequest, ConfigValueErrors, ConfigValueProcessor, Environment,
    EnvironmentError, EnvironmentReadContext, NormalizedConfigValue, ValidatedConfigValue,
    opeo::{OResult, Out},
};
use typol_core::{
    DiagnosticCode, ExecutionLimits, Read, SliceSource, SourceId, SourceSpan, Span, ValueKind,
    ValueSource, ValueView,
};
use typol_i18n::{Language, Translator};
use typol_policy::ResourceLimits;
use typol_schema::{SchemaCompileOptions, SchemaSource};
use typol_semantic::CompiledSchema;
use typol_test_support::*;

struct Provider {
    bytes: Vec<u8>,
    calls: usize,
    error: Option<EnvironmentError>,
}

impl Environment for Provider {
    fn read<'env, 'slot, 'borrow>(
        &'env mut self,
        _: &str,
        _: EnvironmentReadContext<'_>,
        out: Out<'slot, 'borrow, EnvironmentError>,
    ) -> OResult<'slot, Option<&'env [u8]>> {
        self.calls += 1;
        match self.error {
            Some(error) => Err(out.fail(error)),
            None => Ok(Some(&self.bytes)),
        }
    }
}

fn compile(text: &str) -> TestResult<CompiledSchema> {
    Support::compile_schema(
        SchemaCompileOptions::default(),
        SchemaSource::from_str(b"root", text),
        None,
        None,
    )
    .test_value()
}

fn field<'a>(value: impl Into<ValueViewInput<'a>>, name: &str) -> TestResult<ValueView<'a>> {
    let ValueKind::Structure(mut fields) = value.into().value_view().as_ref() else {
        return Err(TestFailure::new(
            "expected the configuration root node to be a struct",
        ));
    };
    fields
        .find(|field| field.name().unwrap_or_default() == name)
        .test_value()
        .map(|field| field.value())
}

#[test]
fn text_parse_returns_an_explicit_validation_marker() -> TestResult<()> {
    let schema = compile("entry struct Root { value: u8 }")?;
    let mut provider = Provider {
        bytes: Vec::new(),
        calls: 0,
        error: None,
    };
    let source = "{ value: 7 }";
    let parsed: ValidatedConfigValue = TestOpeo::value(|out| {
        ConfigParser::new(&schema).parse(
            config_request(
                (&mut SliceSource::new(source.as_bytes()), "validated"),
                &ConfigLimits::default(),
            ),
            &mut provider,
            out,
        )
    })
    .map_err(|_| TestFailure::new("text configuration parsing failed"))?;
    test_assert!(matches!(
        field(parsed.as_config_value(), "value")?.as_ref(),
        ValueKind::Unsigned(7)
    ))?;
    test_eq!(provider.calls, 0)?;

    Ok(())
}

#[test]
fn config_limit_builders_update_all_fields_from_defaults() -> TestResult<()> {
    let defaults = ConfigLimits::default();
    test_eq!(defaults.max_source_bytes(), 1_048_576)?;
    test_eq!(defaults.max_diagnostics(), 100)?;
    test_eq!(defaults.max_nesting_depth(), 128)?;
    test_eq!(defaults.max_string_bytes(), 262_144)?;
    test_eq!(defaults.max_value_nodes(), 100_000)?;
    test_eq!(defaults.max_environment_entries(), 1024)?;
    test_eq!(defaults.max_environment_bytes(), 8_388_608)?;
    test_eq!(defaults.max_constructed_text_bytes(), 16_777_216)?;
    test_eq!(defaults.max_display_name_bytes(), 4096)?;
    test_eq!(ConfigLimits::MAX_DIAGNOSTICS, 1024)?;
    test_eq!(
        defaults.with_max_diagnostics(usize::MAX).max_diagnostics(),
        ConfigLimits::MAX_DIAGNOSTICS
    )?;

    let built = defaults
        .with_max_source_bytes(11)
        .with_max_diagnostics(12)
        .with_max_nesting_depth(13)
        .with_max_string_bytes(14)
        .with_max_value_nodes(15)
        .with_max_environment_entries(16)
        .with_max_environment_bytes(17)
        .with_max_constructed_text_bytes(18)
        .with_max_display_name_bytes(19);
    test_eq!(built.max_source_bytes(), 11)?;
    test_eq!(built.max_diagnostics(), 12)?;
    test_eq!(built.max_nesting_depth(), 13)?;
    test_eq!(built.max_string_bytes(), 14)?;
    test_eq!(built.max_value_nodes(), 15)?;
    test_eq!(built.max_environment_entries(), 16)?;
    test_eq!(built.max_environment_bytes(), 17)?;
    test_eq!(built.max_constructed_text_bytes(), 18)?;
    test_eq!(built.max_display_name_bytes(), 19)?;

    Ok(())
}

#[test]
fn root_api_validates_full_tree_and_returns_owned_values() -> TestResult<()> {
    let value = {
        let schema = compile(
            r#"
            type Name = string @length(3, 8) @regex("^abc");
            struct Child { n: u8 = 2, } @assert(self.n > root.limit, "child")
            entry struct Root {
                first: Name @env("X"), second: Name @env("X"),
                values: [u8] @unique @all(item > 0, "positive") @sum(min = 3),
                child: Child = {}, limit: u8,
            } @assert(self.values.len() == 2, "root")
            "#,
        )?;
        let text =
            String::from(r#"{ first: env("X"), second: env("X"), values: [1, 2], limit: 1 }"#);
        let name = String::from("config.tconf");
        let source = (&mut SliceSource::new(text.as_bytes()), name.as_str());
        test_eq!(source.1, name)?;
        let mut provider = Provider {
            bytes: b"abcde".to_vec(),
            calls: 0,
            error: None,
        };
        let value = Support::parse(
            &schema,
            config_request(source, &ConfigLimits::default()),
            &mut provider,
        )
        .test_value()?;
        test_eq!(provider.calls, 1)?;
        for invalid in [
            r#"{ first: env("X"), second: env("X"), values: [1, 1], limit: 1 }"#,
            r#"{ first: env("X"), second: env("X"), values: [1, 2], limit: 2 }"#,
            r#"{ first: env("X"), second: env("X"), values: [3], limit: 1 }"#,
        ] {
            let errors = Support::parse(
                &schema,
                config_request(
                    (&mut SliceSource::new((invalid).as_bytes()), "invalid"),
                    &ConfigLimits::default(),
                ),
                &mut provider,
            )
            .test_error()?;
            test_assert!(
                errors
                    .diagnostics()
                    .iter()
                    .any(|error| error.code() == DiagnosticCode::CONFIG_VALIDATION_ERROR)
            )?;
        }
        value
    };
    test_assert!(matches!(
        field(&value, "first")?.as_ref(),
        ValueKind::String("abcde")
    ))?;
    test_eq!(
        field(&value, "first")?.source(),
        ValueSource::EnvironmentVariable
    )?;
    test_eq!(
        field(field(&value, "child")?, "n")?.source(),
        ValueSource::SchemaDefault
    )?;

    Ok(())
}

#[test]
fn external_value_operations_return_distinct_normalized_and_validated_types() -> TestResult<()> {
    let schema = compile("entry struct Root { value: u8 @range(1, 3), fallback: u8 = 2 }")?;
    let make_value = |value| {
        ValueFactory::structure(vec![ValueFactory::named(
            "value",
            ValueFactory::unsigned(value)?,
        )])
    };
    let options = ConfigValidationRequest::new(ConfigValidationOptions::default());
    let mut errors = TestSlot::<ConfigValueErrors>::default();
    let input = make_value(0)?;
    let normalized: NormalizedConfigValue = errors
        .access(|out| ConfigValueProcessor::new(&schema).normalize(input, options, out))
        .test_value()?;
    let ValueKind::Structure(mut fields) = normalized.as_config_value().root().as_ref() else {
        return Err(TestFailure::new("normalized result should be a struct"));
    };
    let explicit = fields
        .find(|field| field.name() == Some("value"))
        .test_value()?;
    test_assert!(matches!(explicit.value().as_ref(), ValueKind::Unsigned(0)))?;
    let fallback = fields
        .find(|field| field.name() == Some("fallback"))
        .test_value()?;
    test_assert!(matches!(fallback.value().as_ref(), ValueKind::Unsigned(2)))?;

    let input = make_value(0)?;
    let invalid_caught = errors
        .access(|out| ConfigValueProcessor::new(&schema).validate(input, options, out))
        .test_error()?;
    let invalid_errors = invalid_caught;
    test_assert!(
        invalid_errors
            .diagnostics()
            .iter()
            .any(|diagnostic| { diagnostic.code() == DiagnosticCode::CONFIG_VALIDATION_ERROR })
    )?;

    let input = make_value(2)?;
    let validated: ValidatedConfigValue = errors
        .access(|out| ConfigValueProcessor::new(&schema).validate(input, options, out))
        .test_value()?;
    test_assert!(matches!(
        validated.as_config_value().root().as_ref(),
        ValueKind::Structure(_)
    ))?;
    Ok(())
}

#[test]
fn normalization_shape_errors_keep_their_localized_or_redacted_message() -> TestResult<()> {
    let schema = compile("entry struct Root { password: u8 @secret }")?;
    let value = ValueFactory::structure(vec![ValueFactory::named(
        "password",
        ValueFactory::string("private-value")?,
    )])?;
    let options = ConfigValidationRequest::new(ConfigValidationOptions::default());
    let mut errors = TestSlot::<ConfigValueErrors>::default();
    let errors = errors
        .access(|out| ConfigValueProcessor::new(&schema).normalize(value, options, out))
        .test_error()?;
    let diagnostic = errors
        .diagnostics()
        .first()
        .test_value()
        .map_err(|_| TestFailure::new("normalization should return a diagnostic"))?;
    let key = diagnostic
        .message_key()
        .ok_or_else(|| TestFailure::new("hidden normalization diagnostics need a message key"))?;
    let message = Translator::new(Language::Chinese)
        .translate_diagnostic(key)
        .map_err(|_| TestFailure::new("hidden normalization message should translate"))?;
    test_eq!(message, "值已隐藏")?;
    test_assert!(!diagnostic.message().contains("private-value"))?;
    Ok(())
}

#[test]
fn errors_keep_safe_paths_and_both_source_locations_after_inputs_drop() -> TestResult<()> {
    let schema = compile(
        "entry struct Root { values: map<string, u8> @secret @all(value > 0, \"positive\"), }",
    )?;
    let errors = {
        let text = String::from("{\r\n values: { \"private-key\": 0 }\r\n}");
        let name = String::from("invalid.tconf");
        let mut provider = Provider {
            bytes: vec![],
            calls: 0,
            error: None,
        };
        Support::parse(
            &schema,
            config_request(
                (&mut SliceSource::new(text.as_bytes()), &name),
                &ConfigLimits::default(),
            ),
            &mut provider,
        )
        .test_error()?
    };
    test_eq!(errors.config_source_name(), Some("invalid.tconf"))?;
    test_eq!(errors.config_source_id(), schema.next_source_id())?;
    let diagnostic = &TestCheck::item(errors.diagnostics(), 0)?;
    test_eq!(diagnostic.message(), "value is hidden")?;
    test_eq!(diagnostic.path().test_value()?.len(), 1)?;
    let location = errors
        .span_location(diagnostic.primary().test_value()?)
        .test_value()?;
    test_eq!(location.start.line(), 2)?;
    test_assert!(!diagnostic.labels().is_empty())?;
    for label in diagnostic.labels() {
        test_eq!(
            errors.span_location(label.span()),
            schema.span_location(label.span())
        )?;
    }
    for span in [
        SourceSpan::new(SourceId::new(99), Span::point(0)),
        SourceSpan::new(
            errors.config_source_id().test_value()?,
            Span::from_zero_to(usize::MAX),
        ),
    ] {
        test_assert!(errors.span_location(span).is_none())?;
    }
    test_assert!(!format!("{errors:?} {diagnostic:?}").contains("private-key"))?;

    Ok(())
}

#[test]
fn provider_failures_and_diagnostic_budget_share_the_public_error_type() -> TestResult<()> {
    let schema = compile(r#"entry struct Root { value: u8 @env("X"), }"#)?;
    for (error, code) in [
        (
            EnvironmentError::HostFailure,
            DiagnosticCode::CONFIG_ENVIRONMENT_ERROR,
        ),
        (
            EnvironmentError::AllocationFailed,
            DiagnosticCode::ALLOCATION_FAILED,
        ),
        (
            EnvironmentError::ResourceLimitExceeded,
            DiagnosticCode::RESOURCE_LIMIT_EXCEEDED,
        ),
    ] {
        let mut provider = Provider {
            bytes: vec![],
            calls: 0,
            error: Some(error),
        };
        let errors = Support::parse(
            &schema,
            config_request(
                (
                    &mut SliceSource::new((r#"{ value: env("X", 1) }"#).as_bytes()),
                    "provider",
                ),
                &ConfigLimits::default(),
            ),
            &mut provider,
        )
        .test_error()?;
        test_eq!(TestCheck::item(errors.diagnostics(), 0)?.code(), code)?;
        test_eq!(provider.calls, 1)?;
    }
    for budget in [0, 1, 2] {
        let mut limits = ConfigLimits::default();
        limits = limits.with_max_diagnostics(budget);
        let mut provider = Provider {
            bytes: vec![],
            calls: 0,
            error: None,
        };
        let errors = Support::parse(
            &schema,
            config_request(
                (&mut SliceSource::new(("{} {}").as_bytes()), "budget"),
                &limits,
            ),
            &mut provider,
        )
        .test_error()?;
        test_assert!(!errors.diagnostics().is_empty())?;
        test_assert!(errors.diagnostics().len() <= budget.max(1))?;
        if budget <= 1 {
            test_assert!(errors.is_truncated())?;
            test_assert!(
                TestCheck::item(errors.diagnostics(), 0)?.code()
                    != DiagnosticCode::DIAGNOSTIC_LIMIT_REACHED
            )?;
        }
        test_eq!(provider.calls, 0)?;
    }

    Ok(())
}

#[test]
fn source_limit_stops_before_provider_and_trailing_tokens_fail() -> TestResult<()> {
    let schema = compile("entry struct Root {}")?;
    let mut provider = Provider {
        bytes: vec![],
        calls: 0,
        error: None,
    };
    let mut limits = ConfigLimits::default();
    limits = limits.with_max_source_bytes(1);
    let errors = Support::parse(
        &schema,
        config_request(
            (&mut SliceSource::new(("{}").as_bytes()), "too-large"),
            &limits,
        ),
        &mut provider,
    )
    .test_error()?;
    test_eq!(
        TestCheck::item(errors.diagnostics(), 0)?.code(),
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
    )?;
    test_eq!(provider.calls, 0)?;
    let errors = Support::parse(
        &schema,
        config_request(
            (&mut SliceSource::new(("{} {}").as_bytes()), "trailing"),
            &ConfigLimits::default(),
        ),
        &mut provider,
    )
    .test_error()?;
    test_eq!(
        TestCheck::item(errors.diagnostics(), 0)?.code(),
        DiagnosticCode::CONFIG_SYNTAX_ERROR
    )?;

    Ok(())
}

#[test]
fn all_reported_default_and_rule_locations_survive_schema_drop() -> TestResult<()> {
    let (errors, expected) = {
        let text = r#"struct Child { n: u8 = 1 } @assert(self.n > root.limit, "child")
entry struct Root { child: Child = {}, limit: u8 }"#;
        let schema = compile(text)?;
        let mut provider = Provider {
            bytes: vec![],
            calls: 0,
            error: None,
        };
        let errors = Support::parse(
            &schema,
            config_request(
                (&mut SliceSource::new(("{limit: 2}").as_bytes()), "defaults"),
                &ConfigLimits::default(),
            ),
            &mut provider,
        )
        .test_error()?;
        let mut expected = Vec::new();
        let mut access_errors = TestSlot::default();
        for diagnostic in errors.diagnostics() {
            let field = access_errors
                .access(|out| schema.entry().field_named("child", out))
                .map_err(|_| TestFailure::new("failed to read schema field"))?
                .test_expect("schema is missing the child field")?;
            let field_span = access_errors
                .access(|out| field.span(out))
                .map_err(|_| TestFailure::new("failed to read schema field span"))?;
            test_eq!(diagnostic.primary().test_value()?, field_span,)?;
            for span in diagnostic
                .primary()
                .into_iter()
                .chain(diagnostic.labels().iter().map(|label| label.span()))
            {
                expected.push((span, schema.span_location(span).test_value()?));
            }
        }
        test_assert!(!expected.is_empty())?;
        (errors, expected)
    };
    for (span, location) in expected {
        test_eq!(errors.span_location(span), Some(location))?;
    }

    Ok(())
}

fn config_request<'a>(
    source: (&'a mut impl Read, &'a str),
    limits: &ConfigLimits,
) -> ConfigParseRequest<'a> {
    ConfigParseRequest::new(
        source.0,
        source.1,
        ConfigParseOptions::new(
            ResourceLimits::default(),
            *limits,
            ExecutionLimits::new(1_000_000_000, 268_435_456),
        ),
    )
}
