use typol_test_support::{ConfigTestSupport as Support, TestOpeoOutcome};

use typol_config::{
    ConfigLimits, ConfigParseOptions, ConfigParseRequest, ConfigParser, Environment,
    EnvironmentError, EnvironmentReadContext,
    opeo::{OResult, Out},
};
use typol_core::{
    BudgetError, DiagnosticCode, ExecutionBudget, ExecutionLimits, Read, SliceSource,
};
use typol_policy::{ExcerptLimits, RegexExecutionLimits, RegexOptions};
use typol_schema::{SchemaCompileOptions, SchemaSource};
use typol_semantic::{CompiledSchema, SchemaAccessError};
use typol_test_support::*;

struct SharedExecutionCase {
    prefix: &'static str,
    value_type: &'static str,
    value: &'static str,
    expected: bool,
}

struct RegexBudgetObservation {
    diagnostic: Option<DiagnosticCode>,
    steps: usize,
    peak_workspace: usize,
    failure: Option<BudgetError>,
}

struct NoHost;

impl Environment for NoHost {
    fn read<'env, 'slot, 'borrow>(
        &'env mut self,
        _: &str,
        _: EnvironmentReadContext<'_>,
        out: Out<'slot, 'borrow, EnvironmentError>,
    ) -> OResult<'slot, Option<&'env [u8]>> {
        Err(out.fail(EnvironmentError::InternalError))
    }
}

fn test_opeo<T, E>(
    errors: &mut TestSlot<E>,
    operation: impl for<'slot> FnOnce(Out<'slot, 'slot, E>) -> OResult<'slot, T>,
) -> TestResult<T> {
    errors
        .access(operation)
        .map_err(|_| TestFailure::new("test OPEO call failed"))
}

fn parse_variant_with_budget(
    schema: &CompiledSchema,
    variant: &str,
    max_steps: usize,
) -> TestResult<(bool, usize)> {
    let options =
        ConfigParseOptions::default().with_execution(ExecutionLimits::new(max_steps, 1_048_576));
    let mut budget_errors = TestSlot::<BudgetError>::default();
    let budget = test_opeo(&mut budget_errors, |out| {
        ExecutionBudget::new(*options.execution(), out)
    })?;
    let config = format!("{{ e: .{variant} }}");
    let mut input = SliceSource::new(config.as_bytes());
    let mut parse_errors = TestSlot::default();
    let parsed = parse_errors.access(|out| {
        ConfigParser::new(schema).parse_with_budget(
            ConfigParseRequest::new(&mut input, "enum budget", options),
            &mut NoHost,
            &budget,
            out,
        )
    });
    let succeeded = parsed.is_ok();
    Ok((succeeded, budget.steps()))
}

fn observe_regex_parse(
    schema: &CompiledSchema,
    config: &str,
    regex: RegexOptions,
    execution: ExecutionLimits,
) -> TestResult<RegexBudgetObservation> {
    let options = ConfigParseOptions::default()
        .with_regex(regex)
        .with_execution(execution);
    let mut budget_errors = TestSlot::<BudgetError>::default();
    let budget = test_opeo(&mut budget_errors, |out| {
        ExecutionBudget::new(*options.execution(), out)
    })?;
    let mut input = SliceSource::new(config.as_bytes());
    let mut parse_errors = TestSlot::default();
    let parsed = parse_errors.access(|out| {
        ConfigParser::new(schema).parse_with_budget(
            ConfigParseRequest::new(&mut input, "regex budget", options),
            &mut NoHost,
            &budget,
            out,
        )
    });
    let diagnostic = parsed.err().and_then(|errors| {
        errors
            .diagnostics()
            .last()
            .map(|diagnostic| diagnostic.code())
    });
    Ok(RegexBudgetObservation {
        diagnostic,
        steps: budget.steps(),
        peak_workspace: budget.peak_workspace_bytes(),
        failure: budget.failure(),
    })
}

fn minimum_regex_steps(
    schema: &CompiledSchema,
    config: &str,
    execution: ExecutionLimits,
) -> TestResult<usize> {
    let mut high = 1;
    loop {
        let regex = RegexOptions::default()
            .with_execution(RegexExecutionLimits::default().with_max_steps(high));
        if observe_regex_parse(schema, config, regex, execution)?
            .diagnostic
            .is_none()
        {
            break;
        }
        high = high
            .checked_mul(2)
            .ok_or_else(|| TestFailure::new("Regex step threshold overflowed"))?;
    }
    let mut low = 0;
    while low < high {
        let middle = low + (high - low) / 2;
        let regex = RegexOptions::default()
            .with_execution(RegexExecutionLimits::default().with_max_steps(middle));
        if observe_regex_parse(schema, config, regex, execution)?
            .diagnostic
            .is_none()
        {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    Ok(low)
}

fn minimum_regex_workspace(
    schema: &CompiledSchema,
    config: &str,
    execution: ExecutionLimits,
) -> TestResult<usize> {
    let mut high = 1;
    loop {
        let regex = RegexOptions::default()
            .with_execution(RegexExecutionLimits::default().with_max_workspace_bytes(high));
        if observe_regex_parse(schema, config, regex, execution)?
            .diagnostic
            .is_none()
        {
            break;
        }
        high = high
            .checked_mul(2)
            .ok_or_else(|| TestFailure::new("Regex workspace threshold overflowed"))?;
    }
    let mut low = 0;
    while low < high {
        let middle = low + (high - low) / 2;
        let regex = RegexOptions::default()
            .with_execution(RegexExecutionLimits::default().with_max_workspace_bytes(middle));
        if observe_regex_parse(schema, config, regex, execution)?
            .diagnostic
            .is_none()
        {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    Ok(low)
}

#[test]
fn config_regex_steps_and_workspace_are_isolated_from_caller_budget() -> TestResult<()> {
    let alternatives = (0..240)
        .map(|index| format!("a{index:03}"))
        .collect::<Vec<_>>()
        .join("|");
    let pattern = format!("^({alternatives})+$");
    let schema_text = format!("entry struct Root {{ value: string @regex(\"{pattern}\") }}");
    let schema = Support::compile_schema(
        SchemaCompileOptions::default(),
        SchemaSource::from_str(b"regex-budget", &schema_text),
        None,
        None,
    )
    .test_value()?;
    let value = "a000".repeat(12);
    let config = format!("{{ value: \"{value}\" }}");
    let generous = ExecutionLimits::new(16_777_216, 67_108_864);
    let baseline = observe_regex_parse(&schema, &config, RegexOptions::default(), generous)?;
    test_assert!(baseline.diagnostic.is_none())?;
    test_assert!(baseline.failure.is_none())?;

    let regex_steps = minimum_regex_steps(&schema, &config, generous)?;
    let regex_workspace = minimum_regex_workspace(&schema, &config, generous)?;
    test_assert!(baseline.steps < regex_steps)?;
    test_assert!(baseline.peak_workspace < regex_workspace)?;

    let bounded_execution = ExecutionLimits::new(baseline.steps, baseline.peak_workspace);
    let bounded =
        observe_regex_parse(&schema, &config, RegexOptions::default(), bounded_execution)?;
    test_assert!(bounded.diagnostic.is_none())?;
    test_assert!(bounded.failure.is_none())?;
    test_eq!(bounded.steps, baseline.steps)?;
    test_eq!(bounded.peak_workspace, baseline.peak_workspace)?;

    let regex_limited =
        RegexOptions::default().with_execution(RegexExecutionLimits::default().with_max_steps(0));
    let failed = observe_regex_parse(&schema, &config, regex_limited, generous)?;
    test_eq!(
        failed.diagnostic,
        Some(DiagnosticCode::RESOURCE_LIMIT_EXCEEDED)
    )?;
    test_assert!(failed.failure.is_none())?;

    let workspace_limited = RegexOptions::default().with_execution(
        RegexExecutionLimits::default().with_max_workspace_bytes(regex_workspace - 1),
    );
    let failed = observe_regex_parse(&schema, &config, workspace_limited, generous)?;
    test_eq!(
        failed.diagnostic,
        Some(DiagnosticCode::RESOURCE_LIMIT_EXCEEDED)
    )?;
    test_assert!(failed.failure.is_none())?;
    Ok(())
}

#[test]
fn config_regex_steps_accumulate_across_constraints() -> TestResult<()> {
    let single_schema = Support::compile_schema(
        SchemaCompileOptions::default(),
        SchemaSource::from_str(
            b"single-regex-budget",
            r#"entry struct Root { first: string @regex("^a+$") }"#,
        ),
        None,
        None,
    )
    .test_value()?;
    let pair_schema = Support::compile_schema(
        SchemaCompileOptions::default(),
        SchemaSource::from_str(
            b"pair-regex-budget",
            r#"entry struct Root { first: string @regex("^a+$"), second: string @regex("^a+$") }"#,
        ),
        None,
        None,
    )
    .test_value()?;
    let single_config = r#"{ first: "aaaaaaaaaaaaaaaa" }"#;
    let pair_config = r#"{ first: "aaaaaaaaaaaaaaaa", second: "aaaaaaaaaaaaaaaa" }"#;
    let execution = ExecutionLimits::new(16_777_216, 67_108_864);
    let threshold = minimum_regex_steps(&single_schema, single_config, execution)?;
    let exact = RegexOptions::default()
        .with_execution(RegexExecutionLimits::default().with_max_steps(threshold));
    test_assert!(
        observe_regex_parse(&single_schema, single_config, exact, execution)?
            .diagnostic
            .is_none()
    )?;
    let exhausted = observe_regex_parse(&pair_schema, pair_config, exact, execution)?;
    test_eq!(
        exhausted.diagnostic,
        Some(DiagnosticCode::RESOURCE_LIMIT_EXCEEDED)
    )?;
    test_assert!(exhausted.failure.is_none())?;
    Ok(())
}

#[test]
fn compiled_enum_index_bounds_lookup_work_and_preserves_budget_errors() -> TestResult<()> {
    let variants = (0..256)
        .map(|index| format!("V{index:03},"))
        .collect::<String>();
    let schema = Support::compile_schema(
        SchemaCompileOptions::default(),
        SchemaSource::from_str(
            b"wide-enum",
            &format!("enum E {{{variants}}} entry struct Root {{ e: E, }}"),
        ),
        None,
        None,
    )
    .test_value()?;
    for variant in ["V000", "V128", "V255"] {
        let config = format!("{{ e: .{variant} }}");
        let parsed = Support::parse(
            &schema,
            ConfigParseRequest::new(
                &mut SliceSource::new(config.as_bytes()),
                "wide-enum",
                ConfigParseOptions::default(),
            ),
            &mut NoHost,
        );
        test_assert!(parsed.is_ok())?;
    }
    let unknown = Support::parse(
        &schema,
        ConfigParseRequest::new(
            &mut SliceSource::new(b"{ e: .Unknown }"),
            "wide-enum",
            ConfigParseOptions::default(),
        ),
        &mut NoHost,
    )
    .test_error()?;
    test_eq!(
        TestCheck::item(unknown.diagnostics(), 0)?.code(),
        DiagnosticCode::CONFIG_VALUE_ERROR
    )?;

    let (first_succeeded, first_steps) = parse_variant_with_budget(&schema, "V000", 1_000_000)?;
    let (last_succeeded, last_steps) = parse_variant_with_budget(&schema, "V255", 1_000_000)?;
    test_assert!(first_succeeded && last_succeeded)?;
    test_assert!(last_steps <= first_steps + 128)?;

    let mut schema_errors = TestSlot::<SchemaAccessError>::default();
    let field = test_opeo(&mut schema_errors, |out| {
        schema.entry().field_named("e", out)
    })?
    .test_value()?;
    let enum_type = test_opeo(&mut schema_errors, |out| field.ty(out))?;
    let mut lookup_errors = TestSlot::<BudgetError>::default();
    let index_budget = test_opeo(&mut lookup_errors, |out| {
        ExecutionBudget::new(ExecutionLimits::new(128, 1_048_576), out)
    })?;
    let lookup = test_opeo(&mut lookup_errors, |out| {
        enum_type.variant_named_budgeted("V255", &index_budget, out)
    })?;
    test_assert!(lookup.is_some())?;
    test_assert!(index_budget.steps() <= 128)?;

    let exhausted_budget = test_opeo(&mut lookup_errors, |out| {
        ExecutionBudget::new(ExecutionLimits::new(0, 1_048_576), out)
    })?;
    let exhausted = match lookup_errors
        .access(|out| enum_type.variant_named_budgeted("V255", &exhausted_budget, out))
    {
        Ok(_) => {
            return Err(TestFailure::new(
                "zero-step budget should not complete the enum index lookup",
            ));
        }
        Err(error) => error,
    };
    test_assert!(exhausted == BudgetError::Resource)?;

    Ok(())
}

#[test]
fn independent_examples_have_the_same_default_and_explicit_value_results() -> TestResult<()> {
    let limits = ExcerptLimits::default();
    let cases = [
        SharedExecutionCase {
            prefix: "",
            value_type: "[i128] @sum(min = 0, max = 0)",
            value: "[170141183460469231731687303715884105727, 1, -1, -170141183460469231731687303715884105727]",
            expected: true,
        },
        SharedExecutionCase {
            prefix: "",
            value_type: "[i8] @all(item / 2 == -1, \"rule\")",
            value: "[-3, -2]",
            expected: true,
        },
        SharedExecutionCase {
            prefix: "",
            value_type: "[i8] @all(item == 0 || 10 / item > 0, \"rule\")",
            value: "[0, 1]",
            expected: true,
        },
        SharedExecutionCase {
            prefix: "",
            value_type: "[[u8]] @all(item.all(|x| x > 0) && item.any(|x| x == 2), \"rule\")",
            value: "[[1, 2], [2]]",
            expected: true,
        },
        SharedExecutionCase {
            prefix: "",
            value_type: "map<string, u8> @all(key.starts_with(\"ok\") && value > 0, \"rule\")",
            value: "{\"ok_a\": 1, \"ok_b\": 2}",
            expected: true,
        },
        SharedExecutionCase {
            prefix: "",
            value_type: "option<string> @email",
            value: "none",
            expected: true,
        },
        SharedExecutionCase {
            prefix: "",
            value_type: "string @length(2, 2)",
            value: "\"aé\"",
            expected: true,
        },
        SharedExecutionCase {
            prefix: "",
            value_type: "string @host",
            value: "\"2001:db8::1\"",
            expected: true,
        },
        SharedExecutionCase {
            prefix: "",
            value_type: "string @cidr(v4_only = false)",
            value: "\"2001:db8::/128\"",
            expected: true,
        },
        SharedExecutionCase {
            prefix: "",
            value_type: "string @regex(\"^ok$\")",
            value: "\"ok\"",
            expected: true,
        },
        SharedExecutionCase {
            prefix: "",
            value_type: "[map<string, u8>] @unique",
            value: "[{\"a\": 1, \"b\": 2}, {\"b\": 2, \"a\": 1}]",
            expected: false,
        },
        SharedExecutionCase {
            prefix: "type Host = string @host;",
            value_type: "[Host] @unique",
            value: "[\"Example.COM.\", \"example.com\"]",
            expected: true,
        },
        SharedExecutionCase {
            prefix: "type Host = string @host; enum E { V(Host) }",
            value_type: "[E] @unique",
            value: "[.V(\"2001:db8::1\"), .V(\"2001:0db8:0:0:0:0:0:1\")]",
            expected: true,
        },
        SharedExecutionCase {
            prefix: "struct Row { key: option<string> @host, n: u8 }",
            value_type: "[Row] @unique(by = \"key\")",
            value: "[{key: \"EXAMPLE.com\", n: 1}, {key: \"example.COM.\", n: 2}]",
            expected: true,
        },
        SharedExecutionCase {
            prefix: "",
            value_type: "[f32] @unique",
            value: "[0.0, -0.0]",
            expected: false,
        },
        SharedExecutionCase {
            prefix: "",
            value_type: "[f32] @sum(min = 1.0, max = 1.0)",
            value: "[16777216.0, 1.0, -16777216.0]",
            expected: false,
        },
        SharedExecutionCase {
            prefix: "",
            value_type: "[i8] @sum(max = 127)",
            value: "[127, 1]",
            expected: false,
        },
        SharedExecutionCase {
            prefix: "",
            value_type: "[u8] @all(item / 0 > 0, \"rule\")",
            value: "[1]",
            expected: false,
        },
        SharedExecutionCase {
            prefix: "",
            value_type: "string @email",
            value: "\"a..b@example.com\"",
            expected: false,
        },
        SharedExecutionCase {
            prefix: "",
            value_type: "string @cidr(v4_only = true)",
            value: "\"::1/128\"",
            expected: false,
        },
        SharedExecutionCase {
            prefix: "type Host = string @host; struct Row { a: Host, b: Host }",
            value_type: "[Row] @all(item.a == item.b, \"rule\")",
            value: "[{a: \"Example.COM.\", b: \"example.com\"}]",
            expected: false,
        },
        SharedExecutionCase {
            prefix: "",
            value_type: "[string] @unique",
            value: "[\"Example.COM.\", \"example.com\"]",
            expected: true,
        },
        SharedExecutionCase {
            prefix: "type Host = string @host;",
            value_type: "[Host] @unique",
            value: "[\"127.0.0.1.\", \"127.0.0.1\"]",
            expected: true,
        },
    ];
    for case in cases {
        let defaults = format!(
            "{} entry struct Root {{ value: {} = {} }}",
            case.prefix, case.value_type, case.value
        );
        let result = Support::compile_schema(
            SchemaCompileOptions::default(),
            SchemaSource::from_str(b"defaults", &defaults),
            None,
            None,
        );
        test_eq!(result.is_ok(), case.expected)?;
        if let TestOpeoOutcome::Error(errors) = result {
            test_assert!(
                errors
                    .diagnostics()
                    .iter()
                    .all(|d| d.code() == DiagnosticCode::SCHEMA_SEMANTIC_ERROR)
            )?;
        }
        let source = format!(
            "{} entry struct Root {{ value: {} }}",
            case.prefix, case.value_type
        );
        let schema = Support::compile_schema(
            SchemaCompileOptions::default(),
            SchemaSource::from_str(b"runtime", &source),
            None,
            None,
        )
        .test_value()?;
        let config = format!("{{value: {}}}", case.value);
        let result = Support::parse(
            &schema,
            config_request(
                (&mut SliceSource::new(config.as_bytes()), "explicit"),
                &limits,
            ),
            &mut NoHost,
        );
        test_eq!(result.is_ok(), case.expected)?;
        if let TestOpeoOutcome::Error(errors) = result {
            test_assert!(
                errors
                    .diagnostics()
                    .iter()
                    .all(|d| d.code() == DiagnosticCode::CONFIG_VALIDATION_ERROR)
            )?;
        }
    }

    Ok(())
}

#[test]
fn defaults_skip_assertions_and_runtime_uses_the_real_root() -> TestResult<()> {
    let limits = ExcerptLimits::default();
    let source = "struct Child { n: u8 = 1 } @assert(self.n > root.limit, \"child rule\") entry struct Root { child: Child = {}, limit: u8 = 2 }";
    let schema = Support::compile_schema(
        SchemaCompileOptions::default(),
        SchemaSource::from_str(b"schema", source),
        None,
        None,
    )
    .test_value()?;
    test_assert!(
        Support::parse(
            &schema,
            config_request(
                (&mut SliceSource::new(("{limit: 0}").as_bytes()), "pass"),
                &limits
            ),
            &mut NoHost
        )
        .is_ok()
    )?;
    let errors = Support::parse(
        &schema,
        config_request((&mut SliceSource::new(("{}").as_bytes()), "fail"), &limits),
        &mut NoHost,
    )
    .test_error()?;
    test_eq!(errors.diagnostics().len(), 1)?;
    test_eq!(
        TestCheck::item(errors.diagnostics(), 0)?.message(),
        "child rule"
    )?;

    Ok(())
}

#[test]
fn shared_all_keeps_phase_specific_messages_and_element_locations() -> TestResult<()> {
    let limits = ExcerptLimits::default();
    for secret in ["", "@secret"] {
        let source = format!(
            "entry struct Root {{ values: [u8] {secret} @all(item > 0, \"PRIVATE\") = [0, 0] }}"
        );
        let errors = Support::compile_schema(
            SchemaCompileOptions::default(),
            SchemaSource::from_str(b"schema", &source),
            None,
            None,
        )
        .test_error()?;
        test_eq!(errors.diagnostics().len(), 2)?;
        for d in errors.diagnostics() {
            test_assert!(!d.message().contains("PRIVATE"))?;
            test_eq!(d.labels().len(), 1)?;
            let span = d.primary().test_value()?.span();
            test_eq!(source.get(span.start()..span.end()).test_value()?, "0")?;
        }
        let source =
            format!("entry struct Root {{ values: [u8] {secret} @all(item > 0, \"PRIVATE\") }}");
        let schema = Support::compile_schema(
            SchemaCompileOptions::default(),
            SchemaSource::from_str(b"schema", &source),
            None,
            None,
        )
        .test_value()?;
        let text = "{values: [0, 0]}";
        let errors = Support::parse(
            &schema,
            config_request(
                (&mut SliceSource::new((text).as_bytes()), "config"),
                &limits,
            ),
            &mut NoHost,
        )
        .test_error()?;
        test_eq!(errors.diagnostics().len(), 2)?;
        for d in errors.diagnostics() {
            test_eq!(
                d.message(),
                if secret.is_empty() {
                    "PRIVATE"
                } else {
                    "value is hidden"
                }
            )?;
            test_eq!(d.labels().len(), 1)?;
            let span = d.primary().test_value()?.span();
            test_eq!(text.get(span.start()..span.end()).test_value()?, "0")?;
        }
    }

    Ok(())
}

#[test]
fn recovered_schema_uses_exact_host_text_in_independent_defaults() -> TestResult<()> {
    for (right, expected_count) in [("Example.COM.", 1), ("other.com", 2)] {
        let text = format!(
            "type Host = string @host; struct Row {{ a: Host, b: Host }} entry struct Root {{ broken: Missing, rows: [Row] @all(item.a == item.b, \"PRIVATE\") = [{{a: \"Example.COM.\", b: \"{right}\"}}] }}"
        );
        let errors = Support::compile_schema(
            SchemaCompileOptions::default(),
            SchemaSource::from_str(b"schema", &text),
            None,
            None,
        )
        .test_error()?;
        test_eq!(errors.diagnostics().len(), expected_count)?;
        test_assert!(
            errors
                .diagnostics()
                .iter()
                .all(|d| d.code() == DiagnosticCode::SCHEMA_SEMANTIC_ERROR
                    && !d.message().contains("PRIVATE"))
        )?;
    }

    Ok(())
}

#[test]
fn large_array_and_map_all_constraints_fit_default_execution_budget() -> TestResult<()> {
    let count = 6000;
    let array = (0..count).map(|_| "1").collect::<Vec<_>>().join(",");
    let map = (0..count)
        .map(|index| format!("\"item{index:05}\": 1"))
        .collect::<Vec<_>>()
        .join(",");
    let schema_source = r#"
        entry struct Root {
            values: [u32] @all(item > 0, "positive"),
            mapping: map<string, u32> @all(key.starts_with("item") && value > 0, "positive"),
        }
    "#;
    let schema = Support::compile_schema(
        SchemaCompileOptions::default(),
        SchemaSource::from_str(b"schema", schema_source),
        None,
        None,
    )
    .test_value()?;
    let config = format!("{{ values: [{array}], mapping: {{ {map} }} }}");
    let mut input = SliceSource::new(config.as_bytes());
    let parsed = Support::parse(
        &schema,
        ConfigParseRequest::new(&mut input, "large-config", ConfigParseOptions::default()),
        &mut NoHost,
    );
    parsed.test_value()?;

    Ok(())
}

fn config_request<'a>(
    source: (&'a mut impl Read, &'a str),
    limits: &ExcerptLimits,
) -> ConfigParseRequest<'a> {
    ConfigParseRequest::new(
        source.0,
        source.1,
        ConfigParseOptions::new(
            *limits,
            RegexOptions::default(),
            ConfigLimits::default()
                .with_max_environment_entries(100_000)
                .with_max_environment_bytes(268_435_456)
                .with_max_constructed_text_bytes(268_435_456)
                .with_max_display_name_bytes(1_048_576),
            ExecutionLimits::new(1_000_000_000, 268_435_456),
        ),
    )
}
