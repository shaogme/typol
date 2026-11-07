use typol_test_support::ConfigTestSupport as Support;

use typol_config::{
    ConfigDiagnosticRedactor, ConfigLimits, ConfigParseOptions, ConfigParseRequest, Environment,
    EnvironmentError, EnvironmentReadContext,
    opeo::{OResult, Out},
};
use typol_core::{
    BudgetError, Diagnostic, DiagnosticCode, ExcerptLimits, ExecutionBudget, ExecutionLimits,
    InstancePath, Read, SliceSource, SourceSpan, ValueKind, ValueSource, ValueView,
};
use typol_policy::{RegexExecutionLimits, RegexOptions};
use typol_schema::{SchemaCompileOptions, SchemaSource};
use typol_semantic::CompiledSchema;
use typol_test_support::*;

#[derive(Clone, Copy)]
struct TestLimits {
    excerpts: ExcerptLimits,
    regex: RegexOptions,
    config: ConfigLimits,
}

impl From<&ExcerptLimits> for TestLimits {
    fn from(excerpts: &ExcerptLimits) -> Self {
        Self {
            excerpts: *excerpts,
            regex: RegexOptions::default(),
            config: ConfigLimits::default(),
        }
    }
}

impl From<&RegexOptions> for TestLimits {
    fn from(regex: &RegexOptions) -> Self {
        Self {
            excerpts: ExcerptLimits::default(),
            regex: *regex,
            config: ConfigLimits::default(),
        }
    }
}

impl From<&ConfigLimits> for TestLimits {
    fn from(config: &ConfigLimits) -> Self {
        Self {
            excerpts: ExcerptLimits::default(),
            regex: RegexOptions::default(),
            config: *config,
        }
    }
}

impl From<&(&ExcerptLimits, &ConfigLimits)> for TestLimits {
    fn from(limits: &(&ExcerptLimits, &ConfigLimits)) -> Self {
        Self {
            excerpts: *limits.0,
            regex: RegexOptions::default(),
            config: *limits.1,
        }
    }
}

impl TestLimits {
    fn excerpts(self) -> ExcerptLimits {
        self.excerpts
    }

    fn regex(self) -> RegexOptions {
        self.regex
    }

    fn config(self) -> ConfigLimits {
        self.config
    }
}

struct Provider {
    bytes: Option<Vec<u8>>,
    names: Vec<String>,
}

impl Environment for Provider {
    fn read<'env, 'slot, 'borrow>(
        &'env mut self,
        name: &str,
        _: EnvironmentReadContext<'_>,
        _: Out<'slot, 'borrow, EnvironmentError>,
    ) -> OResult<'slot, Option<&'env [u8]>> {
        self.names.push(name.to_owned());
        Ok(self.bytes.as_deref())
    }
}

fn compile(text: &str) -> TestResult<CompiledSchema> {
    Support::compile_schema(
        SchemaCompileOptions::default(),
        SchemaSource::from_str(b"contracts", text),
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

fn primary_text(text: &str, span: SourceSpan) -> TestResult<&str> {
    text.get(span.span().start()..span.span().end())
        .test_value()
}

fn conversion_diagnostic_with_path(
    budget: &ExecutionBudget,
    fields: &[&str],
) -> TestResult<Diagnostic> {
    TestOpeo::call(|mut out| {
        let mut path = InstancePath::new(Some(budget), out.reborrow())?;
        for field in fields {
            path.push_field(field, out.reborrow())?;
        }
        let mut diagnostic = Diagnostic::try_new(
            DiagnosticCode::RUST_CONVERSION_ERROR,
            "Rust value conversion failed",
            Some(budget),
            out.reborrow(),
        )?;
        diagnostic.try_set_path(Some(path), Some(budget), out.reborrow())?;
        Ok(diagnostic)
    })
    .map_err(|error| TestFailure::new(format!("failed to create conversion diagnostic: {error:?}")))
}

fn execution_budget(limits: ExecutionLimits) -> TestResult<ExecutionBudget> {
    TestOpeo::call(|out| ExecutionBudget::new(limits, out))
        .map_err(|error| TestFailure::new(format!("failed to create execution budget: {error:?}")))
}

#[test]
fn config_regex_diagnostic_hides_secret_map_key() -> TestResult<()> {
    let schema = compile(
        r#"type Item = string @regex("x");
        entry struct Root { values: map<string, Item> @secret }"#,
    )?;
    let text = r#"{values: {"PRIVATE-KEY": "x"}}"#;
    let limits = RegexOptions::default()
        .with_execution(RegexExecutionLimits::default().with_max_search_steps(0));
    let mut provider = Provider {
        bytes: None,
        names: vec![],
    };
    let errors = Support::parse(
        &schema,
        config_request((&mut SliceSource::new((text).as_bytes()), "probe"), &limits),
        &mut provider,
    )
    .test_error()?;
    let safe = TestCheck::item(errors.diagnostics(), 0)?;
    test_eq!(safe.code(), DiagnosticCode::RESOURCE_LIMIT_EXCEEDED)?;
    test_eq!(safe.path().test_value()?.len(), 1)?;
    test_eq!(safe.message(), "value is hidden")?;

    test_eq!(safe.labels().len(), 1)?;
    test_assert!(!format!("{errors:?} {safe:?}").contains("PRIVATE-KEY"))?;
    test_assert!(provider.names.is_empty())?;

    Ok(())
}

#[test]
fn conversion_redaction_clears_unresolved_paths_and_fails_closed_on_budget() -> TestResult<()> {
    let unresolved_schema = compile("entry struct Root { value: u8 }")?;
    let unresolved_budget = execution_budget(ExecutionLimits::new(1_000_000, 1_048_576))?;
    let mut diagnostic = conversion_diagnostic_with_path(&unresolved_budget, &["unknown"])?;
    let result = TestOpeo::call(|out| {
        ConfigDiagnosticRedactor::new(&unresolved_schema).redact_conversion(&mut diagnostic, out)
    });
    if let Err(error) = result {
        return Err(TestFailure::new(format!(
            "unknown path should be safely cleared: {:?}",
            error
        )));
    }
    test_assert!(diagnostic.path().is_none())?;

    let secret_schema = compile("entry struct Root { value: u8 @secret }")?;
    let budget = execution_budget(ExecutionLimits::new(1_000_000, 1_048_576))?;
    let mut diagnostic = conversion_diagnostic_with_path(&budget, &["value"])?;
    let remaining = budget.limits().max_steps().saturating_sub(budget.steps());
    TestOpeo::call::<BudgetError, _>(|out| budget.consume(remaining, out))
        .map_err(|error| TestFailure::new(format!("failed to prepare budget: {error:?}")))?;
    let safe = match TestOpeo::call(|out| {
        ConfigDiagnosticRedactor::new(&secret_schema).redact_conversion(&mut diagnostic, out)
    }) {
        Ok(()) => {
            return Err(TestFailure::new(
                "budget exhaustion should return a safe resource diagnostic",
            ));
        }
        Err(error) => error,
    };
    test_eq!(safe.code(), DiagnosticCode::RESOURCE_LIMIT_EXCEEDED)?;
    test_assert!(safe.path().is_none())?;
    Ok(())
}

#[test]
fn source_debug_hides_text_and_display_name() -> TestResult<()> {
    let mut source = SliceSource::new(br#"{password: "PRIVATE-SENTINEL"}"#);
    let request =
        ConfigParseRequest::new(&mut source, "PRIVATE-NAME", ConfigParseOptions::default());
    let debug = format!("{request:?}");
    test_assert!(!debug.contains("PRIVATE-SENTINEL"))?;
    test_assert!(!debug.contains("PRIVATE-NAME"))?;

    Ok(())
}

struct FallbackCase {
    declaration: &'static str,
    literal: &'static str,
    code: DiagnosticCode,
}

#[test]
fn fallback_errors_point_to_literal_before_provider_read() -> TestResult<()> {
    let cases = [
        FallbackCase {
            declaration: "u8 @min(2)",
            literal: "1",
            code: DiagnosticCode::CONFIG_VALIDATION_ERROR,
        },
        FallbackCase {
            declaration: "i8 @min(0)",
            literal: "-1",
            code: DiagnosticCode::CONFIG_VALIDATION_ERROR,
        },
        FallbackCase {
            declaration: r#"string @regex("^x$")"#,
            literal: r#""a\n""#,
            code: DiagnosticCode::CONFIG_VALIDATION_ERROR,
        },
        FallbackCase {
            declaration: "option<u8> @min(2)",
            literal: "1",
            code: DiagnosticCode::CONFIG_VALIDATION_ERROR,
        },
        FallbackCase {
            declaration: "u8",
            literal: "256",
            code: DiagnosticCode::CONFIG_VALUE_ERROR,
        },
        FallbackCase {
            declaration: "bool",
            literal: "1",
            code: DiagnosticCode::CONFIG_VALUE_ERROR,
        },
        FallbackCase {
            declaration: "u8",
            literal: "true",
            code: DiagnosticCode::CONFIG_VALUE_ERROR,
        },
    ];
    for case in cases {
        let schema = compile(&format!(
            r#"entry struct Root {{ value: {} @env("X") }}"#,
            case.declaration
        ))?;
        let call = format!(r#"env("X", {})"#, case.literal);
        let text = format!("{{value: {call}}}");
        for bytes in [None, Some(b"3".to_vec())] {
            let mut provider = Provider {
                bytes,
                names: vec![],
            };
            let errors = Support::parse(
                &schema,
                config_request(
                    (&mut SliceSource::new(text.as_bytes()), "fallback"),
                    &ExcerptLimits::default(),
                ),
                &mut provider,
            )
            .test_error()?;
            test_eq!(errors.diagnostics().len(), 1)?;
            let diagnostic = TestCheck::item(errors.diagnostics(), 0)?;
            test_eq!(diagnostic.code(), case.code)?;
            test_eq!(
                diagnostic.value_source(),
                Some(ValueSource::ExplicitFallback)
            )?;
            test_eq!(
                primary_text(&text, diagnostic.primary().test_value()?)?,
                case.literal
            )?;
            test_assert!(provider.names.is_empty())?;
        }
    }

    Ok(())
}

#[test]
fn environment_bytes_preserve_per_name_limit_and_cache_reuse() -> TestResult<()> {
    let schema = compile(
        r#"entry struct Root {
        a: string @env("A"), b: string @env("B"),
        c: string @env("C"), again: string @env("A")
    }"#,
    )?;
    let text = r#"{a: env("A"), b: env("B"), c: env("C"), again: env("A")}"#;
    let mut limits = ConfigLimits::default();
    limits = limits.with_max_string_bytes(64);
    let mut provider = Provider {
        bytes: Some(vec![b'x'; 64]),
        names: vec![],
    };
    let value = Support::parse(
        &schema,
        config_request((&mut SliceSource::new((text).as_bytes()), "bytes"), &limits),
        &mut provider,
    )
    .test_value()?;
    test_eq!(provider.names, ["A", "B", "C"])?;
    let mut constructed_bytes = 0;
    for name in ["a", "b", "c", "again"] {
        let ValueKind::String(text) = field(&value, name)?.as_ref() else {
            return Err(TestFailure::new("configuration field should be a string"));
        };
        constructed_bytes += text.len();
    }
    test_eq!(constructed_bytes, 256)?;
    test_assert!(constructed_bytes > limits.max_string_bytes())?;

    provider.bytes = Some(vec![b'x'; 65]);
    provider.names.clear();
    let errors = Support::parse(
        &schema,
        config_request((&mut SliceSource::new((text).as_bytes()), "bytes"), &limits),
        &mut provider,
    )
    .test_error()?;
    test_eq!(
        TestCheck::item(errors.diagnostics(), 0)?.code(),
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
    )?;
    test_eq!(provider.names, ["A"])?;

    Ok(())
}

#[test]
fn single_regex_limit_is_independent_of_shared_budget() -> TestResult<()> {
    let schema =
        compile(r#"type Item = string @regex("^x$"); entry struct Root { values: [Item] }"#)?;
    let mut provider = Provider {
        bytes: None,
        names: vec![],
    };
    let mut limits = RegexOptions::default();
    let minimum = (0..128)
        .find(|steps| {
            limits = limits.with_execution(limits.execution().with_max_search_steps(*steps));
            Support::parse(
                &schema,
                config_request(
                    (
                        &mut SliceSource::new((r#"{values: ["x"]}"#).as_bytes()),
                        "one",
                    ),
                    &limits,
                ),
                &mut provider,
            )
            .is_ok()
        })
        .test_expect("simple mode should finish within a bounded number of steps")?;
    test_assert!(minimum > 0)?;
    limits = limits.with_execution(limits.execution().with_max_search_steps(minimum));
    let value = Support::parse(
        &schema,
        config_request(
            (
                &mut SliceSource::new((r#"{values: ["x", "x", "x", "x"]}"#).as_bytes()),
                "many",
            ),
            &limits,
        ),
        &mut provider,
    )
    .test_value()?;
    let ValueKind::Array(items) = field(&value, "values")?.as_ref() else {
        return Err(TestFailure::new("values field should be an array"));
    };
    test_eq!(items.len(), 4)?;
    limits = limits.with_execution(limits.execution().with_max_search_steps(minimum - 1));
    let errors = Support::parse(
        &schema,
        config_request(
            (
                &mut SliceSource::new((r#"{values: ["x"]}"#).as_bytes()),
                "one",
            ),
            &limits,
        ),
        &mut provider,
    )
    .test_error()?;
    test_eq!(
        TestCheck::item(errors.diagnostics(), 0)?.code(),
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
    )?;
    test_assert!(provider.names.is_empty())?;

    Ok(())
}

#[test]
fn maximum_diagnostic_limit_accepts_valid_empty_config() -> TestResult<()> {
    let schema = compile("entry struct Root {}")?;
    let mut provider = Provider {
        bytes: None,
        names: vec![],
    };
    test_assert!(
        Support::parse(
            &schema,
            config_request(
                (&mut SliceSource::new(("{}").as_bytes()), "valid"),
                &ExcerptLimits::default()
            ),
            &mut provider
        )
        .is_ok()
    )?;
    for budget in [0, 1, usize::MAX] {
        let mut limits = ConfigLimits::default();
        limits = limits.with_max_diagnostics(budget);
        test_assert!(
            Support::parse(
                &schema,
                config_request((&mut SliceSource::new(("{}").as_bytes()), "valid"), &limits),
                &mut provider
            )
            .is_ok()
        )?;
    }
    test_assert!(provider.names.is_empty())?;

    Ok(())
}

#[test]
fn assertion_diagnostics_follow_source_order_and_keep_terminal_slot_last() -> TestResult<()> {
    let schema = compile(
        r#"struct Item { value: u8 } @assert(self.value > 0, "positive")
        entry struct Root { first: Item, second: Item }"#,
    )?;
    let text = "{ second: { value: 0 }, first: { value: 0 } }";
    let mut provider = Provider {
        bytes: None,
        names: vec![],
    };
    let errors = Support::parse(
        &schema,
        config_request(
            (&mut SliceSource::new(text.as_bytes()), "order"),
            &ConfigLimits::default(),
        ),
        &mut provider,
    )
    .test_error()?;
    let first_start = TestCheck::item(errors.diagnostics(), 0)?
        .primary()
        .test_value()?
        .span()
        .start();
    let second_start = TestCheck::item(errors.diagnostics(), 1)?
        .primary()
        .test_value()?
        .span()
        .start();
    test_assert!(first_start < second_start)?;
    test_assert!(first_start < text.find("first").test_value()?)?;
    test_assert!(second_start > text.find("first").test_value()?)?;

    let limited = ConfigLimits::default().with_max_diagnostics(2);
    let errors = Support::parse(
        &schema,
        config_request((&mut SliceSource::new(text.as_bytes()), "order"), &limited),
        &mut provider,
    )
    .test_error()?;
    test_eq!(errors.diagnostics().len(), 2)?;
    test_eq!(
        errors.diagnostics().last().test_value()?.code(),
        DiagnosticCode::DIAGNOSTIC_LIMIT_REACHED
    )?;

    Ok(())
}

#[test]
fn inherited_option_rules_keep_identity_order_and_each_instance_runs_once() -> TestResult<()> {
    let schema_text = r#"type Inner = u8 @min(4);
        type Outer = Inner @min(3);
        entry struct Root { first: option<Outer> @min(2), second: Outer @min(2) }"#;
    let schema = compile(schema_text)?;
    let text = "{second: 1, first: 1}";
    let mut provider = Provider {
        bytes: None,
        names: vec![],
    };
    let errors = Support::parse(
        &schema,
        config_request(
            (&mut SliceSource::new((text).as_bytes()), "rules"),
            &ExcerptLimits::default(),
        ),
        &mut provider,
    )
    .test_error()?;
    test_eq!(errors.diagnostics().len(), 6)?;
    for (diagnostics, name) in errors
        .diagnostics()
        .chunks_exact(3)
        .zip(["second", "first"])
    {
        for (diagnostic, rule) in diagnostics.iter().zip(["@min(3)", "@min(4)", "@min(2)"]) {
            test_eq!(diagnostic.code(), DiagnosticCode::CONFIG_VALIDATION_ERROR)?;
            test_eq!(
                primary_text(schema_text, TestCheck::item(diagnostic.labels(), 0)?.span())?,
                rule
            )?;
            test_eq!(
                TestCheck::item(diagnostic.path().test_value()?.segments(), 0)?.as_field(),
                Some(name)
            )?;
            test_eq!(primary_text(text, diagnostic.primary().test_value()?)?, "1")?;
        }
    }

    Ok(())
}

struct FailingProvider {
    error: EnvironmentError,
    calls: usize,
}

impl Environment for FailingProvider {
    fn read<'env, 'slot, 'borrow>(
        &'env mut self,
        _: &str,
        _: EnvironmentReadContext<'_>,
        out: Out<'slot, 'borrow, EnvironmentError>,
    ) -> OResult<'slot, Option<&'env [u8]>> {
        self.calls += 1;
        Err(out.fail(self.error))
    }
}

#[test]
fn terminal_resource_and_allocation_causes_take_priority_over_summary() -> TestResult<()> {
    let schema = compile(r#"entry struct Root { a: u8 @env("A"), b: u8 @env("B") }"#)?;
    for budget in [0, 1, 2, 3] {
        for (error, code) in [
            (
                EnvironmentError::ResourceLimitExceeded,
                DiagnosticCode::RESOURCE_LIMIT_EXCEEDED,
            ),
            (
                EnvironmentError::AllocationFailed,
                DiagnosticCode::ALLOCATION_FAILED,
            ),
        ] {
            let mut provider = FailingProvider { error, calls: 0 };
            let mut limits = ConfigLimits::default();
            limits = limits.with_max_diagnostics(budget);
            let text = if budget > 1 {
                r#"{unknown: 0, a: env("A"), b: env("B")}"#
            } else {
                r#"{a: env("A"), b: env("B")}"#
            };
            let errors = Support::parse(
                &schema,
                config_request(
                    (&mut SliceSource::new((text).as_bytes()), "terminal"),
                    &limits,
                ),
                &mut provider,
            )
            .test_error()?;
            test_eq!(provider.calls, 1)?;
            test_assert!(!errors.diagnostics().is_empty())?;
            test_assert!(errors.diagnostics().len() <= budget.max(1))?;
            test_eq!(errors.diagnostics().last().test_value()?.code(), code)?;
            test_assert!(errors.diagnostics().iter().all(|diagnostic| diagnostic.code() != DiagnosticCode::DIAGNOSTIC_LIMIT_REACHED))?;
            if budget > 1 && code == DiagnosticCode::RESOURCE_LIMIT_EXCEEDED {
                test_eq!(errors.diagnostics().len(), 2)?;
                test_eq!(
                    TestCheck::item(errors.diagnostics(), 0)?.code(),
                    DiagnosticCode::CONFIG_VALUE_ERROR
                )?;
            }
        }
    }

    Ok(())
}

#[test]
fn schema_label_locations_survive_schema_drop() -> TestResult<()> {
    let schema = compile("entry struct Root { value: u8 @min(2) }")?;
    let mut provider = Provider {
        bytes: None,
        names: vec![],
    };
    let errors = Support::parse(
        &schema,
        config_request(
            (&mut SliceSource::new(("{value: 1}").as_bytes()), "location"),
            &ExcerptLimits::default(),
        ),
        &mut provider,
    )
    .test_error()?;
    let diagnostic = TestCheck::item(errors.diagnostics(), 0)?;
    let primary = diagnostic.primary().test_value()?;
    let label = TestCheck::item(diagnostic.labels(), 0)?.span();
    let primary_location = errors.span_location(primary).test_value()?;
    let label_location = errors.span_location(label).test_value()?;
    drop(schema);
    let other = compile("entry struct Root {\nvalue: u8 @min(2) }")?;
    test_eq!(errors.span_location(primary), Some(primary_location))?;
    test_eq!(errors.span_location(label), Some(label_location))?;
    test_ne!(other.span_location(label), Some(label_location))?;

    Ok(())
}

fn config_request<'a>(
    source: (&'a mut impl Read, &'a str),
    limits: impl Into<TestLimits>,
) -> ConfigParseRequest<'a> {
    let limits = limits.into();
    ConfigParseRequest::new(
        source.0,
        source.1,
        ConfigParseOptions::new(
            limits.excerpts(),
            limits.regex(),
            limits.config(),
            ExecutionLimits::new(1_000_000_000, 268_435_456),
        ),
    )
}
