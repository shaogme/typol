use typol_test_support::{ConfigTestSupport as Support, TestOpeoOutcome};

use typol_config::{
    ConfigLimits, ConfigParseOptions, ConfigParseRequest, ConfigValidationOptions,
    ConfigValidationRequest, ConfigValueProcessor, Environment, EnvironmentError,
    EnvironmentReadContext,
    opeo::{OResult, Out},
};
use typol_core::{ConfigValue, DiagnosticCode, ExecutionLimits, SliceSource};
use typol_policy::ResourceLimits;
use typol_schema::{SchemaCompileOptions, SchemaSource};
use typol_semantic::CompiledSchema;
use typol_test_support::*;

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
        SchemaSource::from_str(b"limits", text),
        None,
        None,
    )
    .test_value()
}

fn options(config: ConfigLimits, execution: ExecutionLimits) -> ConfigParseOptions {
    ConfigParseOptions::new(ResourceLimits::default(), config, execution)
}

fn generous_config() -> ConfigLimits {
    ConfigLimits::default()
        .with_max_environment_entries(100)
        .with_max_environment_bytes(1_048_576)
        .with_max_constructed_text_bytes(1_048_576)
        .with_max_display_name_bytes(64)
}

fn generous_execution() -> ExecutionLimits {
    ExecutionLimits::new(1_000_000, 1_048_576)
}

fn provider() -> Provider {
    Provider {
        bytes: None,
        names: Vec::new(),
    }
}

fn parse(
    schema: &CompiledSchema,
    text: &str,
    options: ConfigParseOptions,
    provider: &mut Provider,
) -> TestOpeoOutcome<(), DiagnosticCode> {
    match Support::parse(
        schema,
        ConfigParseRequest::new(&mut SliceSource::new((text).as_bytes()), "limits", options),
        provider,
    ) {
        TestOpeoOutcome::Value(_) => TestOpeoOutcome::Value(()),
        TestOpeoOutcome::Error(errors) => TestOpeoOutcome::Error(
            errors
                .diagnostics()
                .last()
                .map_or(DiagnosticCode::INTERNAL_ERROR, |diagnostic| {
                    diagnostic.code()
                }),
        ),
    }
}

fn validation_options(config: ConfigLimits) -> ConfigValidationOptions {
    ConfigValidationOptions::new(ResourceLimits::default(), config, generous_execution())
}

fn external_value(schema: &CompiledSchema, source: &str) -> TestResult<ConfigValue> {
    Support::parse(
        schema,
        ConfigParseRequest::new(
            &mut SliceSource::new(source.as_bytes()),
            "depth",
            options(generous_config(), generous_execution()),
        ),
        &mut provider(),
    )
    .test_value()
}

fn normalization_error(
    schema: &CompiledSchema,
    value: ConfigValue,
    config: ConfigLimits,
) -> Option<DiagnosticCode> {
    match TestOpeo::outcome(|out| {
        ConfigValueProcessor::new(schema).normalize(
            value,
            ConfigValidationRequest::new(validation_options(config)),
            out,
        )
    }) {
        TestOpeoOutcome::Value(_) => None,
        TestOpeoOutcome::Error(errors) => errors
            .diagnostics()
            .last()
            .map(|diagnostic| diagnostic.code()),
    }
}

fn validation_error(
    schema: &CompiledSchema,
    value: ConfigValue,
    config: ConfigLimits,
) -> Option<DiagnosticCode> {
    match TestOpeo::outcome(|out| {
        ConfigValueProcessor::new(schema).validate(
            value,
            ConfigValidationRequest::new(validation_options(config)),
            out,
        )
    }) {
        TestOpeoOutcome::Value(_) => None,
        TestOpeoOutcome::Error(errors) => errors
            .diagnostics()
            .last()
            .map(|diagnostic| diagnostic.code()),
    }
}

struct DepthCase {
    schema: &'static str,
    source: &'static str,
    depth: usize,
}

#[test]
fn nesting_depth_boundaries_match_for_text_normalization_and_validation() -> TestResult<()> {
    for case in [
        DepthCase {
            schema: "entry struct Root {}",
            source: "{}",
            depth: 0,
        },
        DepthCase {
            schema: "struct Empty {} entry struct Root { child: Empty }",
            source: "{ child: {} }",
            depth: 1,
        },
        DepthCase {
            schema: "entry struct Root { items: [u8] }",
            source: "{ items: [] }",
            depth: 1,
        },
        DepthCase {
            schema: "entry struct Root { value: option<u8> }",
            source: "{ value: 1 }",
            depth: 2,
        },
        DepthCase {
            schema: "entry struct Root { items: [u8] = [] }",
            source: "{}",
            depth: 1,
        },
        DepthCase {
            schema: "enum E { One([u8]), } entry struct Root { value: E }",
            source: "{ value: .One([]) }",
            depth: 2,
        },
        DepthCase {
            schema: "enum E { Record { items: [u8] }, } entry struct Root { value: E }",
            source: "{ value: .Record { items: [] } }",
            depth: 3,
        },
    ] {
        let schema = compile(case.schema)?;
        let exact = generous_config().with_max_nesting_depth(case.depth);
        test_eq!(
            parse(
                &schema,
                case.source,
                options(exact, generous_execution()),
                &mut provider(),
            ),
            TestOpeoOutcome::Value(())
        )?;
        test_eq!(
            normalization_error(&schema, external_value(&schema, case.source)?, exact),
            None
        )?;
        test_eq!(
            validation_error(&schema, external_value(&schema, case.source)?, exact),
            None
        )?;

        if case.depth > 0 {
            let below = generous_config().with_max_nesting_depth(case.depth - 1);
            test_eq!(
                parse(
                    &schema,
                    case.source,
                    options(below, generous_execution()),
                    &mut provider(),
                ),
                TestOpeoOutcome::Error(DiagnosticCode::RESOURCE_LIMIT_EXCEEDED)
            )?;
            test_eq!(
                normalization_error(&schema, external_value(&schema, case.source)?, below),
                Some(DiagnosticCode::RESOURCE_LIMIT_EXCEEDED)
            )?;
            test_eq!(
                validation_error(&schema, external_value(&schema, case.source)?, below),
                Some(DiagnosticCode::RESOURCE_LIMIT_EXCEEDED)
            )?;
        }
    }

    Ok(())
}

#[test]
fn environment_cache_and_constructed_copies_have_independent_exact_limits() -> TestResult<()> {
    let schema = compile(
        r#"entry struct Root { a: string @env("A"), b: string @env("B"), c: string @env("C"), again: string @env("A") }"#,
    )?;
    let text = r#"{a: env("A"), b: env("B"), c: env("C"), again: env("A")}"#;
    let mut provider = Provider {
        bytes: Some(vec![b'x'; 64]),
        names: Vec::new(),
    };
    test_eq!(
        parse(
            &schema,
            text,
            options(
                ConfigLimits::default()
                    .with_max_environment_entries(3)
                    .with_max_environment_bytes(192)
                    .with_max_constructed_text_bytes(268)
                    .with_max_display_name_bytes(6),
                generous_execution()
            ),
            &mut provider
        ),
        TestOpeoOutcome::Value(())
    )?;
    test_eq!(provider.names, ["A", "B", "C"])?;
    for config in [
        ConfigLimits::default()
            .with_max_environment_entries(2)
            .with_max_environment_bytes(192)
            .with_max_constructed_text_bytes(268)
            .with_max_display_name_bytes(6),
        ConfigLimits::default()
            .with_max_environment_entries(3)
            .with_max_environment_bytes(191)
            .with_max_constructed_text_bytes(268)
            .with_max_display_name_bytes(6),
        ConfigLimits::default()
            .with_max_environment_entries(3)
            .with_max_environment_bytes(192)
            .with_max_constructed_text_bytes(267)
            .with_max_display_name_bytes(6),
    ] {
        provider.names.clear();
        test_eq!(
            parse(
                &schema,
                text,
                options(config, generous_execution()),
                &mut provider
            ),
            TestOpeoOutcome::Error(DiagnosticCode::RESOURCE_LIMIT_EXCEEDED)
        )?;
        test_assert!(provider.names.len() <= 3)?;
    }
    provider.names.clear();
    test_eq!(
        parse(
            &schema,
            text,
            options(
                ConfigLimits::default()
                    .with_max_environment_entries(0)
                    .with_max_environment_bytes(192)
                    .with_max_constructed_text_bytes(268)
                    .with_max_display_name_bytes(6),
                generous_execution()
            ),
            &mut provider
        ),
        TestOpeoOutcome::Error(DiagnosticCode::RESOURCE_LIMIT_EXCEEDED)
    )?;
    test_assert!(provider.names.is_empty())?;

    Ok(())
}

#[test]
fn missing_environment_entries_count_and_repeat_names_use_cache() -> TestResult<()> {
    let schema = compile(
        r#"entry struct Root { a: option<string> @env("A"), b: option<string> @env("A"), c: option<string> @env("C") }"#,
    )?;
    let text = r#"{a: env("A"), b: env("A"), c: env("C")}"#;
    let mut provider = provider();
    test_eq!(
        parse(
            &schema,
            text,
            options(
                ConfigLimits::default()
                    .with_max_environment_entries(1)
                    .with_max_environment_bytes(0)
                    .with_max_constructed_text_bytes(100)
                    .with_max_display_name_bytes(6),
                generous_execution()
            ),
            &mut provider
        ),
        TestOpeoOutcome::Error(DiagnosticCode::RESOURCE_LIMIT_EXCEEDED)
    )?;
    test_eq!(provider.names, ["A"])?;
    provider.names.clear();
    test_eq!(
        parse(
            &schema,
            text,
            options(
                ConfigLimits::default()
                    .with_max_environment_entries(2)
                    .with_max_environment_bytes(0)
                    .with_max_constructed_text_bytes(100)
                    .with_max_display_name_bytes(6),
                generous_execution()
            ),
            &mut provider
        ),
        TestOpeoOutcome::Value(())
    )?;
    test_eq!(provider.names, ["A", "C"])?;

    Ok(())
}

#[test]
fn discarded_fallback_still_consumes_constructed_text() -> TestResult<()> {
    let schema = compile(r#"entry struct Root { value: string @env("X") }"#)?;
    let mut provider = Provider {
        bytes: Some(b"actual".to_vec()),
        names: Vec::new(),
    };
    let text = r#"{value: env("X", "fallback")}"#;
    test_eq!(
        parse(
            &schema,
            text,
            options(
                ConfigLimits::default()
                    .with_max_environment_entries(1)
                    .with_max_environment_bytes(6)
                    .with_max_constructed_text_bytes(20)
                    .with_max_display_name_bytes(6),
                generous_execution()
            ),
            &mut provider
        ),
        TestOpeoOutcome::Value(())
    )?;
    provider.names.clear();
    test_eq!(
        parse(
            &schema,
            text,
            options(
                ConfigLimits::default()
                    .with_max_environment_entries(1)
                    .with_max_environment_bytes(6)
                    .with_max_constructed_text_bytes(19)
                    .with_max_display_name_bytes(6),
                generous_execution()
            ),
            &mut provider
        ),
        TestOpeoOutcome::Error(DiagnosticCode::RESOURCE_LIMIT_EXCEEDED)
    )?;
    test_eq!(provider.names, ["X"])?;

    Ok(())
}

#[derive(Clone, Copy)]
enum Quota {
    Steps,
    Workspace,
}

fn execution(quota: Quota, value: usize) -> ExecutionLimits {
    match quota {
        Quota::Steps => ExecutionLimits::new(value, 1_048_576),
        Quota::Workspace => ExecutionLimits::new(1_000_000, value),
    }
}

fn minimum(schema: &CompiledSchema, text: &str, quota: Quota) -> TestResult<usize> {
    let mut low = 0;
    let mut high = 1_048_576;
    while low < high {
        let middle = low + (high - low) / 2;
        match parse(
            schema,
            text,
            options(generous_config(), execution(quota, middle)),
            &mut provider(),
        ) {
            TestOpeoOutcome::Value(()) => high = middle,
            TestOpeoOutcome::Error(code) => {
                test_eq!(code, DiagnosticCode::RESOURCE_LIMIT_EXCEEDED)?;
                low = middle + 1;
            }
        }
    }
    Ok(low)
}

#[test]
fn shared_regex_steps_and_workspace_accept_exact_boundary() -> TestResult<()> {
    let schema =
        compile(r#"type Item = string @regex("^x$"); entry struct Root { values: [Item] }"#)?;
    let text = r#"{values: ["x", "x", "x", "x"]}"#;
    for quota in [Quota::Steps, Quota::Workspace] {
        let limit = minimum(&schema, text, quota)?;
        test_assert!(limit > 0)?;
        test_eq!(
            parse(
                &schema,
                text,
                options(generous_config(), execution(quota, limit)),
                &mut provider()
            ),
            TestOpeoOutcome::Value(())
        )?;
        test_eq!(
            parse(
                &schema,
                text,
                options(generous_config(), execution(quota, limit - 1)),
                &mut provider()
            ),
            TestOpeoOutcome::Error(DiagnosticCode::RESOURCE_LIMIT_EXCEEDED)
        )?;
    }
    let one = minimum(&schema, r#"{values: ["x"]}"#, Quota::Steps)?;
    test_eq!(
        parse(
            &schema,
            text,
            options(generous_config(), execution(Quota::Steps, one)),
            &mut provider()
        ),
        TestOpeoOutcome::Error(DiagnosticCode::RESOURCE_LIMIT_EXCEEDED)
    )?;

    Ok(())
}

#[test]
fn reused_options_start_a_new_call_and_request_debug_hides_source() -> TestResult<()> {
    let schema = compile("entry struct Root {}")?;
    let limit = minimum(&schema, "{}", Quota::Steps)?;
    let options = options(generous_config(), execution(Quota::Steps, limit));
    for _ in 0..2 {
        test_eq!(
            parse(&schema, "{}", options, &mut provider()),
            TestOpeoOutcome::Value(())
        )?;
    }
    let mut request_input = SliceSource::new(("PRIVATE-TEXT").as_bytes());
    let request = ConfigParseRequest::new(&mut request_input, "PRIVATE-NAME", options);
    let debug = format!("{request:?}");
    test_assert!(!debug.contains("PRIVATE-TEXT"))?;
    test_assert!(!debug.contains("PRIVATE-NAME"))?;

    Ok(())
}

#[test]
fn display_name_limit_precedes_provider_read() -> TestResult<()> {
    let schema = compile(r#"entry struct Root { a: string @env("A") }"#)?;
    let mut provider = provider();
    test_eq!(
        parse(
            &schema,
            r#"{a: env("A")}"#,
            options(
                ConfigLimits::default()
                    .with_max_environment_entries(1)
                    .with_max_environment_bytes(10)
                    .with_max_constructed_text_bytes(100)
                    .with_max_display_name_bytes(1),
                generous_execution()
            ),
            &mut provider
        ),
        TestOpeoOutcome::Error(DiagnosticCode::RESOURCE_LIMIT_EXCEEDED)
    )?;
    test_assert!(provider.names.is_empty())?;

    Ok(())
}

#[test]
fn defaults_count_each_owned_key_field_variant_and_string_copy() -> TestResult<()> {
    let cases = [
        (
            r#"entry struct Root { a: string = "hi", b: string = "hi" }"#,
            6,
        ),
        (
            r#"entry struct Root { a: map<string, string> = {"k": "v"}, b: map<string, string> = {"k": "v"} }"#,
            6,
        ),
        (
            r#"enum E { V { x: string } } entry struct Root { a: E = .V {x: "hi"}, b: E = .V {x: "hi"} }"#,
            10,
        ),
    ];
    for (source, text_bytes) in cases {
        let schema = compile(source)?;
        for quota in [text_bytes, text_bytes - 1] {
            let result = parse(
                &schema,
                "{}",
                options(
                    ConfigLimits::default()
                        .with_max_environment_entries(0)
                        .with_max_environment_bytes(0)
                        .with_max_constructed_text_bytes(quota)
                        .with_max_display_name_bytes(6),
                    generous_execution(),
                ),
                &mut provider(),
            );
            test_eq!(
                result,
                if quota == text_bytes {
                    TestOpeoOutcome::Value(())
                } else {
                    TestOpeoOutcome::Error(DiagnosticCode::RESOURCE_LIMIT_EXCEEDED)
                }
            )?;
        }
    }

    Ok(())
}

#[test]
fn nested_quantifiers_unique_and_sum_share_one_balance() -> TestResult<()> {
    let schema = compile(
        r#"entry struct Root { values: [[u8]] @all(item.all(|x| x > 0) && item.any(|x| x == 2), "rule"), unique: [string] @unique, sum: [u8] @sum(max = 12) }"#,
    )?;
    let text = r#"{values: [[1, 2], [2]], unique: ["a", "b", "c"], sum: [3, 4, 5]}"#;
    let limit = minimum(&schema, text, Quota::Steps)?;
    test_eq!(
        parse(
            &schema,
            text,
            options(generous_config(), execution(Quota::Steps, limit)),
            &mut provider()
        ),
        TestOpeoOutcome::Value(())
    )?;
    test_eq!(
        parse(
            &schema,
            text,
            options(generous_config(), execution(Quota::Steps, limit - 1)),
            &mut provider()
        ),
        TestOpeoOutcome::Error(DiagnosticCode::RESOURCE_LIMIT_EXCEEDED)
    )?;

    Ok(())
}

#[test]
fn unicode_error_finalization_uses_remaining_steps_and_safe_emergency_exit() -> TestResult<()> {
    let schema = compile("entry struct Root { value: string @length(1, 1) }")?;
    let text = format!("{{value: \"{}\"}}", "é".repeat(100));
    let mut low = 0;
    let mut high = 1_048_576;
    while low < high {
        let middle = low + (high - low) / 2;
        let code = parse(
            &schema,
            &text,
            options(generous_config(), execution(Quota::Steps, middle)),
            &mut provider(),
        )
        .test_error()?;
        if code == DiagnosticCode::CONFIG_VALIDATION_ERROR {
            high = middle;
        } else {
            test_eq!(code, DiagnosticCode::RESOURCE_LIMIT_EXCEEDED)?;
            low = middle + 1;
        }
    }
    test_eq!(
        parse(
            &schema,
            &text,
            options(generous_config(), execution(Quota::Steps, low)),
            &mut provider()
        ),
        TestOpeoOutcome::Error(DiagnosticCode::CONFIG_VALIDATION_ERROR)
    )?;
    let mut provider = provider();
    let errors = Support::parse(
        &schema,
        ConfigParseRequest::new(
            &mut SliceSource::new(text.as_bytes()),
            "limits",
            options(generous_config(), execution(Quota::Steps, low - 1)),
        ),
        &mut provider,
    )
    .test_error()?;
    test_eq!(
        errors.diagnostics().last().test_value()?.code(),
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
    )?;
    test_assert!(errors.diagnostics().last().test_value()?.path().is_none())?;
    test_assert!(!format!("{errors:?}").contains(&"é".repeat(10)))?;

    Ok(())
}

#[test]
fn enum_syntax_and_container_recovery_share_diagnostic_budget() -> TestResult<()> {
    let cases = [
        ("enum E { V } entry struct Root { e: E }", "{e: .}"),
        ("entry struct Root { a: [u8] }", "{a: [unknown}"),
    ];
    for (source, text) in cases {
        let schema = compile(source)?;
        let mut low = 0;
        let mut high = 1_048_576;
        while low < high {
            let middle = low + (high - low) / 2;
            let code = parse(
                &schema,
                text,
                options(generous_config(), execution(Quota::Steps, middle)),
                &mut provider(),
            )
            .test_error()?;
            if code == DiagnosticCode::CONFIG_SYNTAX_ERROR {
                high = middle;
            } else {
                test_eq!(code, DiagnosticCode::RESOURCE_LIMIT_EXCEEDED)?;
                low = middle + 1;
            }
        }
        for (quota, expected) in [
            (low, DiagnosticCode::CONFIG_SYNTAX_ERROR),
            (low - 1, DiagnosticCode::RESOURCE_LIMIT_EXCEEDED),
        ] {
            test_eq!(
                parse(
                    &schema,
                    text,
                    options(generous_config(), execution(Quota::Steps, quota)),
                    &mut provider(),
                ),
                TestOpeoOutcome::Error(expected)
            )?;
        }
    }

    Ok(())
}
