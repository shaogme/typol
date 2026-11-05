use crate::{
    BindingTestSupport,
    codec::ValidatedValue,
    diagnostics::CodecErrors,
    session::SessionBudget,
    test_support::{TestOpeo, TestOpeoOutcome},
};
use opeo::{OResult, Out};
use typol_config::{
    ConfigParseOptions, ConfigParseRequest, ConfigParser, Environment, EnvironmentError,
    EnvironmentReadContext,
};
use typol_core::{ConfigValue, ExecutionLimits, SliceSource, ValueKind};
use typol_semantic::CompiledSchema;
use typol_test_support::{TestResult, test_verify as assert, test_verify_eq as assert_eq};

struct Provider;

impl Environment for Provider {
    fn read<'slot, 'borrow, 'env>(
        &'env mut self,
        _: &str,
        _: EnvironmentReadContext<'_>,
        _: Out<'slot, 'borrow, EnvironmentError>,
    ) -> OResult<'slot, Option<&'env [u8]>> {
        Ok(None)
    }
}

fn request<'a>(input: &'a mut SliceSource<'_>) -> ConfigParseRequest<'a> {
    ConfigParseRequest::new(input, "pretty encoding test", ConfigParseOptions::default())
}

fn session(output: usize) -> TestResult<SessionBudget> {
    TestOpeo::value(|out| {
        SessionBudget::new(
            ExecutionLimits::new(100_000_000, 67_108_864),
            100_000_000,
            output,
            out,
        )
    })
}

fn parsed<'schema, 'call>(
    schema: &'schema CompiledSchema,
    request: ConfigParseRequest<'_>,
    session: &'call mut SessionBudget,
) -> TestResult<ValidatedValue<'schema, 'call>> {
    TestOpeo::value(|out| ValidatedValue::parse(schema, request, &mut Provider, session, out))
}

fn parsed_config(
    schema: &CompiledSchema,
    request: ConfigParseRequest<'_>,
) -> TestResult<ConfigValue> {
    TestOpeo::value(|out| ConfigParser::new(schema).parse(request, &mut Provider, out))
        .map(|value| value.into_config_value())
}

fn encoded_with_pretty(validated: &mut ValidatedValue<'_, '_>) -> TestResult<String> {
    let mut output = String::new();
    TestOpeo::value(|out| validated.encode(&mut output, true, out))?;
    Ok(output)
}

fn encode_outcome_with_pretty(
    validated: &mut ValidatedValue<'_, '_>,
) -> TestOpeoOutcome<String, CodecErrors> {
    let mut output = String::new();
    match TestOpeo::outcome(|out| validated.encode(&mut output, true, out)) {
        TestOpeoOutcome::Value(()) => TestOpeoOutcome::Value(output),
        TestOpeoOutcome::Error(error) => TestOpeoOutcome::Error(error),
    }
}

#[test]
fn nested_containers_and_all_enum_payloads_have_pretty_config_text() -> TestResult {
    let schema = BindingTestSupport::compile(
        "struct Empty {} struct Child { enabled: bool, } enum Choice { Unit, Single([u8]), Object { child: Child, } } entry struct Root { empty_struct: Empty, empty_map: map<string, u8>, empty_array: [u8], child: Child, mapping: map<string, [u8]>, choices: [Choice], }",
    )?;
    let input = r#"{choices: [.Object { child: {enabled: false} }, .Single([3, 4]), .Unit], mapping: {"z": [2], "a": []}, child: {enabled: true}, empty_array: [], empty_map: {}, empty_struct: {}}"#;
    let expected = concat!(
        "{\n",
        "  empty_struct: {},\n",
        "  empty_map: {},\n",
        "  empty_array: [],\n",
        "  child: {\n",
        "    enabled: true,\n",
        "  },\n",
        "  mapping: {\n",
        "    \"a\": [],\n",
        "    \"z\": [\n",
        "      2,\n",
        "    ],\n",
        "  },\n",
        "  choices: [\n",
        "    .Object {\n",
        "      child: {\n",
        "        enabled: false,\n",
        "      },\n",
        "    },\n",
        "    .Single([\n",
        "      3,\n",
        "      4,\n",
        "    ]),\n",
        "    .Unit,\n",
        "  ],\n",
        "}"
    );
    let mut call = session(4096)?;
    let mut validated = parsed(
        &schema,
        request(&mut SliceSource::new(input.as_bytes())),
        &mut call,
    )?;
    let output = encoded_with_pretty(&mut validated)?;
    assert_eq!(output, expected);
    let reparsed = parsed_config(&schema, request(&mut SliceSource::new(output.as_bytes())))?;
    assert!(matches!(reparsed.as_ref(), ValueKind::Structure(_)));
    let mut second = session(4096)?;
    let mut validated_again = parsed(
        &schema,
        request(&mut SliceSource::new(output.as_bytes())),
        &mut second,
    )?;
    assert_eq!(encoded_with_pretty(&mut validated_again)?, expected);
    Ok(())
}

#[test]
fn pretty_whitespace_counts_toward_output_and_failure_returns_no_fragment() -> TestResult {
    let schema = BindingTestSupport::compile("entry struct Root { value: [u8], }")?;
    let expected = "{\n  value: [\n    1,\n    2,\n  ],\n}";
    for (limit, succeeds) in [(expected.len(), true), (expected.len() - 1, false)] {
        let mut call = session(limit)?;
        let mut validated = parsed(
            &schema,
            request(&mut SliceSource::new(b"{value: [1, 2]}")),
            &mut call,
        )?;
        match encode_outcome_with_pretty(&mut validated) {
            TestOpeoOutcome::Value(text) => {
                assert!(succeeds);
                assert_eq!(text, expected);
            }
            TestOpeoOutcome::Error(errors) => {
                assert!(!succeeds);
                assert_eq!(BindingTestSupport::code(errors)?, 1);
            }
        }
    }
    Ok(())
}
