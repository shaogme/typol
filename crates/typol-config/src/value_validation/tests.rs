use crate::{
    ConfigLimits, ConfigParseErrors, ConfigParseOptions, ConfigParseRequest, ConfigParser,
    ConfigValidationOptions, Environment, EnvironmentError, EnvironmentReadContext,
    value_validation::{ValueValidation, ValueValidationInput, ValueValidationRequest},
};
use alloc::{format, vec, vec::Vec};
use opeo::{OResult, Out};
use typol_core::{
    AllocationError, BudgetError, ConfigValue, DiagnosticCode, ExecutionBudget, ExecutionLimits,
    SliceSource, ValueError, ValueKind, ValuePayloadView, ValueSource,
};
use typol_policy::{ExcerptLimits, RegexOptions};
use typol_schema::{
    ResolverError, SchemaCompileErrors, SchemaCompileOptions, SchemaCompiler, SchemaResolveRequest,
    SchemaResolver, SchemaSource,
};
use typol_semantic::CompiledSchema;
use typol_test_support::*;

struct NoImports;

impl SchemaResolver for NoImports {
    fn resolve<'a, 'slot, 'borrow>(
        &'a mut self,
        _request: SchemaResolveRequest<'_>,
        out: Out<'slot, 'borrow, ResolverError>,
    ) -> OResult<'slot, SchemaSource<'a>> {
        Err(out.fail(ResolverError::NotFound))
    }
}

struct NoEnvironment;

impl Environment for NoEnvironment {
    fn read<'env, 'slot, 'borrow>(
        &'env mut self,
        _: &str,
        _: EnvironmentReadContext<'_>,
        out: Out<'slot, 'borrow, EnvironmentError>,
    ) -> OResult<'slot, Option<&'env [u8]>> {
        Err(out.fail(EnvironmentError::InternalError))
    }
}

fn array(values: Vec<ConfigValue>) -> TestResult<ConfigValue> {
    ValueFactory::try_array(values).map_err(TestFailure::from_debug)
}

struct ValidationTest {
    allocation_errors: TestSlot<AllocationError>,
    budget_errors: TestSlot<BudgetError>,
    parse_errors: TestSlot<ConfigParseErrors>,
    schema_errors: TestSlot<SchemaCompileErrors>,
    value_errors: TestSlot<ValueError>,
    default_budget: ExecutionBudget,
}

impl ValidationTest {
    fn new() -> TestResult<Self> {
        let mut budget_errors = TestSlot::default();
        let options = ConfigValidationOptions::default();
        let default_budget = budget_errors
            .run(|out| ExecutionBudget::new(*options.execution(), out))
            .test_value()?;
        Ok(Self {
            allocation_errors: TestSlot::default(),
            budget_errors,
            parse_errors: TestSlot::default(),
            schema_errors: TestSlot::default(),
            value_errors: TestSlot::default(),
            default_budget,
        })
    }

    fn compile(&mut self, text: &str) -> TestResult<CompiledSchema> {
        self.schema_errors
            .run(|mut out| {
                let compiler =
                    SchemaCompiler::new(SchemaCompileOptions::default(), out.reborrow())?;
                compiler.compile(
                    SchemaSource::from_str(b"root", text),
                    &mut NoImports,
                    None,
                    out.reborrow(),
                )
            })
            .test_value()
    }

    fn field(&mut self, name: &str, value: ConfigValue) -> TestResult<FieldValue> {
        self.allocation_errors
            .run(|out| FieldValue::try_new_unmetered(name, value, out))
            .test_value()
    }

    fn object(&mut self, fields: Vec<(&str, ConfigValue)>) -> TestResult<ConfigValue> {
        let mut values = Vec::new();
        for (name, value) in fields {
            values.push(self.field(name, value)?);
        }
        self.value_errors
            .run(|out| ValueFactory::try_structure(values).map_err(|error| out.fail(error)))
            .test_value()
    }

    fn map_entry(&mut self, key: &str, value: ConfigValue) -> TestResult<MapEntry> {
        self.allocation_errors
            .run(|out| MapEntry::try_new_unmetered(key, value, out))
            .test_value()
    }

    fn map(&mut self, entries: Vec<MapEntry>) -> TestResult<ConfigValue> {
        self.value_errors
            .run(|out| ValueFactory::try_map(entries).map_err(|error| out.fail(error)))
            .test_value()
    }

    fn some(&mut self, value: ConfigValue) -> TestResult<ConfigValue> {
        self.value_errors
            .run(|out| ValueFactory::try_some(value).map_err(|error| out.fail(error)))
            .test_value()
    }

    fn string(&mut self, value: &str) -> TestResult<ConfigValue> {
        self.value_errors
            .run(|out| ValueFactory::try_string(value).map_err(|error| out.fail(error)))
            .test_value()
    }

    fn enumeration(&mut self, variant: &str, payload: Payload) -> TestResult<ConfigValue> {
        self.value_errors
            .run(|out| {
                ValueFactory::try_enumeration(variant, payload).map_err(|error| out.fail(error))
            })
            .test_value()
    }

    fn enum_single(&mut self, value: ConfigValue) -> TestResult<Payload> {
        self.allocation_errors
            .run(|_| Ok(Payload::Single(value)))
            .test_value()
    }

    fn enum_structure(&mut self, fields: Vec<FieldValue>) -> TestResult<Payload> {
        self.value_errors
            .run(|_| Ok(Payload::Structure(fields)))
            .test_value()
    }

    fn budget(&mut self, options: &ConfigValidationOptions) -> TestResult<ExecutionBudget> {
        self.budget_errors
            .run(|out| ExecutionBudget::new(*options.execution(), out))
            .test_value()
    }

    fn budget_consume(&mut self, budget: &ExecutionBudget, steps: usize) -> TestResult<()> {
        self.budget_errors
            .run(|out| budget.consume(steps, out))
            .test_value()
    }

    fn validate(
        &mut self,
        schema: &CompiledSchema,
        value: &ConfigValue,
        options: &ConfigValidationOptions,
        budget: &ExecutionBudget,
    ) -> TestOpeoOutcome<ConfigValue, ConfigParseErrors> {
        self.parse_errors.run(|out| {
            ValueValidation::value(
                ValueValidationRequest {
                    schema,
                    value: ValueValidationInput::External(value),
                    options,
                    budget,
                },
                out,
            )
        })
    }

    fn run(
        &mut self,
        schema: &CompiledSchema,
        value: &ConfigValue,
    ) -> TestOpeoOutcome<ConfigValue, ConfigParseErrors> {
        let options = ConfigValidationOptions::default();
        self.parse_errors.run(|out| {
            ValueValidation::value(
                ValueValidationRequest {
                    schema,
                    value: ValueValidationInput::External(value),
                    options: &options,
                    budget: &self.default_budget,
                },
                out,
            )
        })
    }

    fn parse(
        &mut self,
        schema: &CompiledSchema,
        text: &str,
    ) -> TestOpeoOutcome<ConfigValue, ConfigParseErrors> {
        let mut source = SliceSource::new(text.as_bytes());
        match self.parse_errors.run(|out| {
            ConfigParser::new(schema).parse(
                ConfigParseRequest::new(&mut source, "test", ConfigParseOptions::default()),
                &mut NoEnvironment,
                out,
            )
        }) {
            TestOpeoOutcome::Value(value) => TestOpeoOutcome::Value(value.into_config_value()),
            TestOpeoOutcome::Error(error) => TestOpeoOutcome::Error(error),
        }
    }

    fn text_codes(&mut self, schema: &CompiledSchema, text: &str) -> TestResult<Vec<u32>> {
        let errors = self.parse(schema, text).test_error()?;
        Ok(errors
            .diagnostics()
            .iter()
            .map(|error| error.code().get())
            .collect())
    }
}

#[test]
fn arbitrary_shapes_are_value_errors_without_config_positions() -> TestResult<()> {
    let mut test = ValidationTest::new()?;
    let schema = test.compile(
        "enum E { Unit, One(u8), Data { n: u8 } } entry struct Root { n: u8, e: E, a: [u8; 2] }",
    )?;
    let valid_e_a = test.enumeration("Unit", Payload::Unit)?;
    let valid_e_b = test.enumeration("Unit", Payload::Unit)?;
    let one = test.enumeration("One", Payload::Unit)?;
    let unknown = test.enumeration("Unknown", Payload::Unit)?;
    for value in [
        ValueFactory::signed(1)?,
        test.object(vec![])?,
        test.object(vec![("unknown", ValueFactory::unsigned(1)?)])?,
        test.object(vec![
            ("n", ValueFactory::unsigned(256)?),
            ("e", valid_e_a),
            ("a", array(vec![])?),
        ])?,
        test.object(vec![
            ("n", ValueFactory::unsigned(1)?),
            ("e", one),
            (
                "a",
                array(vec![ValueFactory::unsigned(1)?, ValueFactory::unsigned(2)?])?,
            ),
        ])?,
        test.object(vec![
            ("n", ValueFactory::unsigned(1)?),
            ("e", valid_e_b),
            ("a", array(vec![])?),
        ])?,
        test.object(vec![
            ("n", ValueFactory::unsigned(1)?),
            ("e", unknown),
            (
                "a",
                array(vec![ValueFactory::unsigned(1)?, ValueFactory::unsigned(2)?])?,
            ),
        ])?,
    ] {
        let errors = test.run(&schema, &value).test_error()?;
        test_eq!(
            TestCheck::item(errors.diagnostics(), 0)?.code(),
            DiagnosticCode::CONFIG_VALUE_ERROR
        )?;
        test_assert!(errors.config_source_id().is_none())?;
        for error in errors.diagnostics() {
            test_assert!(error.primary().is_none())?;
            test_assert!(matches!(
                error.value_source(),
                None | Some(ValueSource::RustValue)
            ))?;
            for label in error.labels() {
                test_assert!(errors.span_location(label.span()).is_some())?;
            }
        }
    }

    Ok(())
}

#[test]
fn fields_defaults_options_and_map_order_are_normalized() -> TestResult<()> {
    let mut test = ValidationTest::new()?;
    let schema = test.compile(
        "entry struct Root { a: u8 = 3, b: option<u8> = 7, c: option<u8>, d: map<string, u8> }",
    )?;
    let entries = vec![
        test.map_entry("", ValueFactory::unsigned(0)?)?,
        test.map_entry("z", ValueFactory::unsigned(2)?)?,
        test.map_entry("a", ValueFactory::unsigned(1)?)?,
    ];
    let map = ValueFactory::with_source(test.map(entries)?, ValueSource::SchemaDefault)?;
    let input = test.object(vec![
        ("d", map),
        ("b", ValueFactory::none()?),
        ("a", ValueFactory::unsigned(9)?),
    ])?;
    let output = test.run(&schema, &input).test_value()?;
    let ValueKind::Structure(fields) = output.as_ref() else {
        return Err(TestFailure::new(
            "value shape does not match the test expectation",
        ));
    };
    let fields: Vec<_> = fields.collect();
    test_eq!(
        fields
            .iter()
            .map(|field| field.as_named().map(|named| named.name()))
            .collect::<Option<Vec<_>>>()
            .test_value()?,
        ["a", "b", "c", "d"]
    )?;
    test_assert!(matches!(
        TestCheck::item(&fields, 0)?.value().as_ref(),
        ValueKind::Unsigned(9)
    ))?;
    test_assert!(matches!(
        TestCheck::item(&fields, 1)?.value().as_ref(),
        ValueKind::Optional(None)
    ))?;
    test_assert!(matches!(
        TestCheck::item(&fields, 2)?.value().as_ref(),
        ValueKind::Optional(None)
    ))?;
    let ValueKind::Map(entries) = TestCheck::item(&fields, 3)?.value().as_ref() else {
        return Err(TestFailure::new(
            "value shape does not match the test expectation",
        ));
    };
    let entries: Vec<_> = entries.collect();
    test_eq!(
        entries
            .iter()
            .map(|entry| entry.as_named().map(|named| named.name()))
            .collect::<Option<Vec<_>>>()
            .test_value()?,
        ["", "a", "z"]
    )?;
    let ValueKind::Structure(mut original) = input.as_ref() else {
        return Err(TestFailure::new(
            "value shape does not match the test expectation",
        ));
    };
    test_eq!(original.next().test_value()?.name(), Some("d"))?;

    Ok(())
}

#[test]
fn option_and_alias_rules_have_the_same_owner_deduplication() -> TestResult<()> {
    let mut test = ValidationTest::new()?;
    let schema = test.compile(
        "type A = u8 @range(2, 5); type O = option<A> @range(3, 4); entry struct Root { n: O }",
    )?;
    let some = test.some(ValueFactory::unsigned(1)?)?;
    let value = test.object(vec![("n", some)])?;
    let errors = test.run(&schema, &value).test_error()?;
    test_eq!(
        errors
            .diagnostics()
            .iter()
            .map(|error| error.code().get())
            .collect::<Vec<_>>(),
        test.text_codes(&schema, "{n: 1}")?
    )?;
    test_eq!(errors.diagnostics().len(), 2)?;

    Ok(())
}

#[test]
fn children_fail_before_parent_collection_rules_and_assertions() -> TestResult<()> {
    let mut test = ValidationTest::new()?;
    let schema = test.compile(
        "type N = u8 @range(2, 5); entry struct Root { a: [N] @size(3, 4) } @assert(self.a.len() == 99, \"root\")",
    )?;
    let value = test.object(vec![("a", array(vec![ValueFactory::unsigned(1)?])?)])?;
    let errors = test.run(&schema, &value).test_error()?;
    test_eq!(errors.diagnostics().len(), 1)?;
    test_eq!(
        TestCheck::item(errors.diagnostics(), 0)?.code(),
        DiagnosticCode::CONFIG_VALIDATION_ERROR
    )?;

    Ok(())
}

#[test]
fn defaults_collections_and_active_variants_assert_against_actual_root() -> TestResult<()> {
    let mut test = ValidationTest::new()?;
    let schema = test.compile(
        r#"struct Child { n: u8 } @assert(self.n > root.limit, "child")
    enum E { Unit, One(Child), Data { n: u8 } @assert(self.n > root.limit, "payload") }
    entry struct Root { limit: u8, children: [Child] = [{n: 1}], dict: map<string, Child> = {"z": {n: 1}}, e: E }
    @assert(self.limit == 0, "root")"#,
    )?;
    let data_field = test.field("n", ValueFactory::unsigned(1)?)?;
    let data_fields = test.enum_structure(vec![data_field])?;
    let data = test.enumeration("Data", data_fields)?;
    let value = test.object(vec![("e", data), ("limit", ValueFactory::unsigned(2)?)])?;
    let errors = test.run(&schema, &value).test_error()?;
    test_eq!(
        errors
            .diagnostics()
            .iter()
            .map(|error| error.code().get())
            .collect::<Vec<_>>(),
        test.text_codes(&schema, "{limit: 2, e: .Data {n: 1}}")?
    )?;
    test_eq!(errors.diagnostics().len(), 4)?;
    let labels = errors
        .diagnostics()
        .iter()
        .flat_map(|error| error.labels())
        .map(|label| label.span())
        .collect::<Vec<_>>();
    drop(schema);
    for span in labels {
        test_assert!(errors.span_location(span).is_some())?;
    }

    Ok(())
}

#[test]
fn shared_budget_and_diagnostic_terminal_slots_are_preserved() -> TestResult<()> {
    let mut test = ValidationTest::new()?;
    let schema = test.compile("entry struct Root { a: [u8] @all(item > 0, \"positive\") }")?;
    let value = test.object(vec![(
        "a",
        array(vec![
            ValueFactory::unsigned(0)?,
            ValueFactory::unsigned(0)?,
            ValueFactory::unsigned(0)?,
        ])?,
    )])?;
    for limit in [0, 1, 2] {
        let mut config = ConfigLimits::default();
        config = config.with_max_diagnostics(limit);
        let options = ConfigValidationOptions::new(
            ExcerptLimits::default(),
            RegexOptions::default(),
            config,
            *ConfigValidationOptions::default().execution(),
        );
        let budget = test.budget(&options)?;
        let errors = test
            .validate(&schema, &value, &options, &budget)
            .test_error()?;
        test_eq!(errors.diagnostics().len(), limit.max(1))?;
        if limit <= 1 {
            test_eq!(
                TestCheck::item(errors.diagnostics(), 0)?.code(),
                DiagnosticCode::CONFIG_VALIDATION_ERROR
            )?;
            test_assert!(format!("{errors}").contains("subsequent errors were not collected"))?;
        } else {
            test_eq!(
                errors.diagnostics().last().test_value()?.code(),
                DiagnosticCode::DIAGNOSTIC_LIMIT_REACHED
            )?;
        }
        test_assert!(errors.is_truncated())?;
    }
    let options = ConfigValidationOptions::default();
    let budget = test.budget(&options)?;
    test.budget_consume(&budget, options.execution().max_steps())?;
    let errors = test
        .validate(&schema, &value, &options, &budget)
        .test_error()?;
    test_eq!(
        TestCheck::item(errors.diagnostics(), 0)?.code(),
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
    )?;

    Ok(())
}

#[test]
fn secret_container_errors_hide_dynamic_keys_and_values() -> TestResult<()> {
    let mut test = ValidationTest::new()?;
    let schema = test.compile("entry struct Root { data: map<string, [u8]> @secret }")?;
    let entry = test.map_entry("private-key", array(vec![ValueFactory::signed(-1)?])?)?;
    let data = test.map(vec![entry])?;
    let value = test.object(vec![("data", data)])?;
    let errors = test.run(&schema, &value).test_error()?;
    let debug = format!("{errors:?} {:?}", errors.diagnostics());
    test_assert!(!debug.contains("private-key"))?;
    test_assert!(!debug.contains("-1"))?;
    test_eq!(
        TestCheck::item(errors.diagnostics(), 0)?
            .path()
            .test_value()?
            .segments()
            .len(),
        1
    )?;

    Ok(())
}

#[test]
fn regex_host_unique_sum_and_all_match_text_validation() -> TestResult<()> {
    let mut test = ValidationTest::new()?;
    let schema = test.compile(
        r#"type H = string @host @regex("^[a-zA-Z.]+$");
    entry struct Root { hosts: [H] @unique, numbers: [i8] @sum(min = 1) @all(item > 0, "positive") }"#,
    )?;
    let host_a = test.string("EXAMPLE.org")?;
    let host_b = test.string("example.ORG")?;
    let value = test.object(vec![
        ("hosts", array(vec![host_a, host_b])?),
        (
            "numbers",
            array(vec![ValueFactory::signed(0)?, ValueFactory::signed(0)?])?,
        ),
    ])?;
    let errors = test.run(&schema, &value).test_error()?;
    test_eq!(
        errors
            .diagnostics()
            .iter()
            .map(|error| error.code().get())
            .collect::<Vec<_>>(),
        test.text_codes(
            &schema,
            r#"{hosts: ["EXAMPLE.org", "example.ORG"], numbers: [0, 0]}"#
        )?
    )?;
    test_eq!(errors.diagnostics().len(), 3)?;
    let bad_host = test.string("bad host!")?;
    let bad = test.object(vec![
        ("hosts", array(vec![bad_host])?),
        ("numbers", array(vec![ValueFactory::signed(1)?])?),
    ])?;
    test_eq!(test.run(&schema, &bad).test_error()?.diagnostics().len(), 2)?;

    Ok(())
}

#[test]
fn schema_default_map_order_and_all_three_enum_payloads_survive() -> TestResult<()> {
    let mut test = ValidationTest::new()?;
    let schema = test.compile(
        r#"enum E { Unit, One(u8), Data { a: u8 = 3, b: option<u8> } }
    entry struct Root { values: [E], defaults: map<string, u8> = {"z": 1, "a": 2} }"#,
    )?;
    let unit = test.enumeration("Unit", Payload::Unit)?;
    let one_payload = test.enum_single(ValueFactory::unsigned(2)?)?;
    let one = test.enumeration("One", one_payload)?;
    let data_payload = test.enum_structure(vec![])?;
    let data = test.enumeration("Data", data_payload)?;
    let value = test.object(vec![("values", array(vec![unit, one, data])?)])?;
    let output = test.run(&schema, &value).test_value()?;
    let ValueKind::Structure(fields) = output.as_ref() else {
        return Err(TestFailure::new(
            "value shape does not match the test expectation",
        ));
    };
    let fields: Vec<_> = fields.collect();
    let ValueKind::Map(entries) = TestCheck::item(&fields, 1)?.value().as_ref() else {
        return Err(TestFailure::new(
            "value shape does not match the test expectation",
        ));
    };
    let entries: Vec<_> = entries.collect();
    test_eq!(
        entries
            .iter()
            .map(|entry| entry.as_named().map(|named| named.name()))
            .collect::<Option<Vec<_>>>()
            .test_value()?,
        ["z", "a"]
    )?;
    let ValueKind::Array(values) = TestCheck::item(&fields, 0)?.value().as_ref() else {
        return Err(TestFailure::new(
            "value shape does not match the test expectation",
        ));
    };
    let values: Vec<_> = values.collect();
    let ValueKind::Enumeration {
        payload: ValuePayloadView::Structure(fields),
        ..
    } = TestCheck::item(&values, 2)?.value().as_ref()
    else {
        return Err(TestFailure::new(
            "value shape does not match the test expectation",
        ));
    };
    let fields: Vec<_> = fields.collect();
    test_assert!(matches!(
        TestCheck::item(&fields, 0)?.value().as_ref(),
        ValueKind::Unsigned(3)
    ))?;
    test_assert!(matches!(
        TestCheck::item(&fields, 1)?.value().as_ref(),
        ValueKind::Optional(None)
    ))?;

    Ok(())
}

#[test]
fn exact_nodes_text_and_shared_execution_limits_are_enforced() -> TestResult<()> {
    let mut test = ValidationTest::new()?;
    let schema = test.compile("entry struct Root { value: map<string, string> }")?;
    let text = test.string("text")?;
    let entry = test.map_entry("key", text)?;
    let value_map = test.map(vec![entry])?;
    let value = test.object(vec![("value", value_map)])?;
    let defaults = ConfigValidationOptions::default();
    for nodes in [4, 3] {
        let mut limits = ConfigLimits::default();
        limits = limits.with_max_value_nodes(nodes);
        let options = ConfigValidationOptions::new(
            *defaults.excerpts(),
            *defaults.regex(),
            limits,
            *defaults.execution(),
        );
        let budget = test.budget(&options)?;
        test_eq!(
            test.validate(&schema, &value, &options, &budget).is_ok(),
            nodes == 4
        )?;
    }
    for bytes in [12, 11] {
        let options = ConfigValidationOptions::new(
            *defaults.excerpts(),
            *defaults.regex(),
            ConfigLimits::default()
                .with_max_environment_entries(0)
                .with_max_environment_bytes(0)
                .with_max_constructed_text_bytes(bytes)
                .with_max_display_name_bytes(0),
            *defaults.execution(),
        );
        let budget = test.budget(&options)?;
        test_eq!(
            test.validate(&schema, &value, &options, &budget).is_ok(),
            bytes == 12
        )?;
    }
    let budget = test.budget(&defaults)?;
    drop(
        test.validate(&schema, &value, &defaults, &budget)
            .test_value()?,
    );
    let steps = budget.steps();
    for maximum in [steps, steps - 1] {
        let options = ConfigValidationOptions::new(
            *defaults.excerpts(),
            *defaults.regex(),
            *defaults.config(),
            ExecutionLimits::new(maximum, 67_108_864),
        );
        let budget = test.budget(&options)?;
        test_eq!(
            test.validate(&schema, &value, &options, &budget).is_ok(),
            maximum == steps
        )?;
    }

    Ok(())
}

#[test]
fn secret_element_struct_assertions_hide_messages_before_path_copy() -> TestResult<()> {
    let mut test = ValidationTest::new()?;
    let schema = test.compile(
        r#"struct Child { n: u8 } @assert(false, "confidential-message")
    type Secret = Child @secret;
    entry struct Root { values: map<string, Secret> }"#,
    )?;
    let child = test.object(vec![("n", ValueFactory::unsigned(1)?)])?;
    let entry = test.map_entry("private-key", child)?;
    let values = test.map(vec![entry])?;
    let value = test.object(vec![("values", values)])?;
    let errors = test.run(&schema, &value).test_error()?;
    test_eq!(
        TestCheck::item(errors.diagnostics(), 0)?.message(),
        "value is hidden"
    )?;
    test_eq!(
        TestCheck::item(errors.diagnostics(), 0)?
            .path()
            .test_value()?
            .len(),
        1
    )?;
    test_assert!(!format!("{:?}", errors.diagnostics()).contains("confidential"))?;
    let text = test
        .parse(&schema, r#"{values: {"private-key": {n: 1}}}"#)
        .test_error()?;
    test_eq!(
        TestCheck::item(text.diagnostics(), 0)?.message(),
        "value is hidden"
    )?;
    let schema = test.compile(
        r#"type Secret = string @secret @regex("^a+$"); entry struct Root { values: map<string, Secret> }"#,
    )?;
    let secret_text = test.string("aaa")?;
    let secret_entry = test.map_entry("private-key", secret_text)?;
    let secret_values = test.map(vec![secret_entry])?;
    let input = test.object(vec![("values", secret_values)])?;
    let defaults = ConfigValidationOptions::default();
    let mut regex_limits = *defaults.regex().limits();
    regex_limits.set_max_regex_scratch_states(0);
    let regex = defaults.regex().with_limits(regex_limits);
    let options = ConfigValidationOptions::new(
        *defaults.excerpts(),
        regex,
        *defaults.config(),
        *defaults.execution(),
    );
    let budget = test.budget(&options)?;
    let errors = test
        .validate(&schema, &input, &options, &budget)
        .test_error()?;
    test_eq!(
        TestCheck::item(errors.diagnostics(), 0)?.code(),
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
    )?;
    test_eq!(
        TestCheck::item(errors.diagnostics(), 0)?.message(),
        "value is hidden"
    )?;
    test_eq!(
        TestCheck::item(errors.diagnostics(), 0)?
            .path()
            .test_value()?
            .len(),
        1
    )?;

    Ok(())
}
