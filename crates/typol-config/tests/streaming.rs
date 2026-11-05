use typol_test_support::{ConfigTestSupport as Support, TestOpeoOutcome};

use std::{cell::Cell, rc::Rc};
use typol_config::{
    ConfigFailureStage, ConfigParseErrors, ConfigParseOptions, ConfigParseRequest, ConfigParser,
    Environment, EnvironmentError, EnvironmentReadContext, StreamLimits,
    opeo::{OResult, Out},
};
use typol_core::{
    AllocationError, BudgetError, ConfigValue, DiagnosticCode, ExcerptLimits, ExecutionBudget,
    ExecutionLimits, LineIndex, LocationError, Read, SliceSource, SourceId, SourceReadError,
    SourceReadErrorKind, ValueKind, ValuePayloadView,
};
use typol_schema::{SchemaCompileOptions, SchemaSource};
use typol_semantic::CompiledSchema;
use typol_test_support::*;

fn require_opeo<T, E>(
    errors: &mut TestSlot<E>,
    operation: impl for<'slot> FnOnce(Out<'slot, 'slot, E>) -> OResult<'slot, T>,
) -> TestResult<T> {
    errors
        .access(operation)
        .map_err(|_| TestFailure::new("OPEO call failed"))
}

fn compile(text: &str) -> TestResult<CompiledSchema> {
    Support::compile_schema(
        SchemaCompileOptions::default(),
        SchemaSource::from_str(b"stream", text),
        None,
        None,
    )
    .test_value()
}

struct Source<'a> {
    bytes: &'a [u8],
    offset: usize,
    cuts: Vec<usize>,
    calls: usize,
    fail_at: Option<usize>,
    consumed: Rc<Cell<usize>>,
}

impl<'a> Source<'a> {
    fn new(bytes: &'a [u8], cuts: Vec<usize>) -> Self {
        Self {
            bytes,
            offset: 0,
            cuts,
            calls: 0,
            fail_at: None,
            consumed: Rc::new(Cell::new(0)),
        }
    }
}

impl Read for Source<'_> {
    fn read<'slot, 'borrow>(
        &mut self,
        output: &mut [u8],
        source_id: SourceId,
        offset: usize,
        out: Out<'slot, 'borrow, SourceReadError>,
    ) -> OResult<'slot, usize> {
        self.offset = offset;
        self.calls += 1;
        if self.fail_at == Some(self.offset) {
            return Err(out.fail(
                SourceReadError::new(SourceReadErrorKind::HostFailure)
                    .with_input(source_id, offset),
            ));
        }
        let end = self
            .cuts
            .iter()
            .copied()
            .find(|cut| *cut > self.offset)
            .unwrap_or(self.bytes.len());
        let end = end.min(self.fail_at.unwrap_or(self.bytes.len()));
        let count = output.len().min(end - self.offset);
        let Some(end_offset) = self.offset.checked_add(count) else {
            return Err(out.fail(
                SourceReadError::new(SourceReadErrorKind::InternalInvariantFailure)
                    .with_input(source_id, offset),
            ));
        };
        let Some(target) = output.get_mut(..count) else {
            return Err(out.fail(
                SourceReadError::new(SourceReadErrorKind::InternalInvariantFailure)
                    .with_input(source_id, offset),
            ));
        };
        let Some(source) = self.bytes.get(self.offset..end_offset) else {
            return Err(out.fail(
                SourceReadError::new(SourceReadErrorKind::InternalInvariantFailure)
                    .with_input(source_id, offset),
            ));
        };
        target.copy_from_slice(source);
        self.offset += count;
        self.consumed.set(self.offset);
        Ok(count)
    }
}

#[derive(Default)]
struct Provider {
    calls: usize,
    source_bytes: Option<Rc<Cell<usize>>>,
    first_read_at: Option<usize>,
}

impl Environment for Provider {
    fn read<'env, 'slot, 'borrow>(
        &'env mut self,
        _: &str,
        _: EnvironmentReadContext<'_>,
        _: Out<'slot, 'borrow, EnvironmentError>,
    ) -> OResult<'slot, Option<&'env [u8]>> {
        self.calls += 1;
        self.first_read_at
            .get_or_insert_with(|| self.source_bytes.as_ref().map_or(0, |bytes| bytes.get()));
        Ok(Some(b"7"))
    }
}

fn run(
    schema: &CompiledSchema,
    source: &mut dyn Read,
    chunk: usize,
) -> TestResult<TestOpeoOutcome<ConfigValue, ConfigParseErrors>> {
    let stream = StreamLimits::try_new(chunk, 1_048_576).test_value()?;
    Ok(Support::parse(
        schema,
        ConfigParseRequest::new(
            source,
            "stream-config",
            ConfigParseOptions::default().with_stream(stream),
        ),
        &mut Provider::default(),
    ))
}

#[test]
fn every_single_split_and_short_read_preserves_values_and_origins() -> TestResult<()> {
    let schema = compile(
        r#"enum E { Unit, Single(u8), Fields { n: u8 } }
        entry struct Root { text: string, n: i128, f: f64, hex: u8, values: [u8], pair: [u8; 2], labels: map<string, string>, optional: option<u8>, variant: E, from_env: u8 @env("X"), again: u8 @env("X"), default: u8 = 9 }"#,
    )?;
    let text = "/* before😀 */\r\n{text: \"éø😀é\\n\\\"\\\\\", n: -00012, f: 0012.50, hex: 0x2a, values: [1,2], pair: [3,4], labels: {\"ø\": \"v\"}, optional: none, variant: .Fields{n: 5}, from_env: env(\"X\"), again: env(\"X\", 8)}//tail\n";
    let expected = snapshot(
        &run(&schema, &mut SliceSource::new(text.as_bytes()), 8192)
            .test_value()?
            .test_value()?,
    );
    for chunk in [1, 2, 3, 7, 8192] {
        let value = run(
            &schema,
            &mut Source::new(text.as_bytes(), (1..text.len()).collect()),
            chunk,
        )
        .test_value()?
        .test_value()?;
        test_eq!(snapshot(&value), expected)?;
    }
    for cut in 1..text.len() {
        test_eq!(
            snapshot(
                &run(&schema, &mut Source::new(text.as_bytes(), vec![cut]), 8192)
                    .test_value()?
                    .test_value()?
            ),
            expected
        )?;
    }
    let cuts = (1..text.len())
        .filter(|index| (index * 1103515245 + 12345) % 11 < 3)
        .collect();
    test_eq!(
        snapshot(
            &run(&schema, &mut Source::new(text.as_bytes(), cuts), 7)
                .test_value()?
                .test_value()?
        ),
        expected
    )?;

    Ok(())
}

#[test]
fn delayed_collection_errors_match_line_index_after_windows_and_schema_are_dropped()
-> TestResult<()> {
    let text = "{\r\n values: [0, /*😀ø\nwindow coverage*/ 1, 1],\r\n labels: {\"éø😀\": 0}\r\n}\n";
    let errors = {
        let schema = compile(
            r#"entry struct Root { values: [u8] @all(item > 0, "positive") @unique, labels: map<string, u8> @all(value > 0, "positive") }"#,
        )?;
        run(
            &schema,
            &mut Source::new(text.as_bytes(), (1..text.len()).collect()),
            1,
        )
        .test_value()?
        .test_error()?
    };
    let mut allocation_errors = TestSlot::<AllocationError>::default();
    let mut location_errors = TestSlot::<LocationError>::default();
    let index = require_opeo(&mut allocation_errors, |out| LineIndex::new(text, out))?;
    let config_id = errors.config_source_id().test_value()?;
    test_assert!(errors.diagnostics().len() >= 3)?;
    for error in errors.diagnostics() {
        for span in error
            .primary()
            .into_iter()
            .chain(error.labels().iter().map(|label| label.span()))
        {
            let location = errors.span_location(span).test_value()?;
            if span.source_id() == config_id {
                test_eq!(
                    location.start,
                    require_opeo(&mut location_errors, |out| {
                        index.location(span.span().start(), out)
                    })?
                )?;
                test_eq!(
                    location.end,
                    require_opeo(&mut location_errors, |out| {
                        index.location(span.span().end(), out)
                    })?
                )?;
            }
        }
    }

    Ok(())
}

#[test]
fn environment_is_evaluated_before_the_tail_is_read_and_failed_tail_skips_assertions()
-> TestResult<()> {
    let schema =
        compile(r#"entry struct Root { n: u8 @env("X") } @assert(self.n == 0, "postcondition")"#)?;
    let text = format!("{{n: env(\"X\")}} /*{}*/", "comment\n".repeat(1000));
    for fail_at in [None, Some(text.len())] {
        let mut source = Source::new(text.as_bytes(), vec![]);
        source.fail_at = fail_at;
        let mut provider = Provider {
            source_bytes: Some(source.consumed.clone()),
            ..Provider::default()
        };
        let errors = Support::parse(
            &schema,
            ConfigParseRequest::new(
                &mut source,
                "stream-config",
                ConfigParseOptions::default()
                    .with_stream(StreamLimits::try_new(7, 1024).test_value()?),
            ),
            &mut provider,
        )
        .test_error()?;
        test_eq!(provider.calls, 1)?;
        test_assert!(provider.first_read_at.test_value()? < 32)?;
        test_eq!(
            errors.stage(),
            if fail_at.is_some() {
                ConfigFailureStage::Input
            } else {
                ConfigFailureStage::Parsing
            }
        )?;
        test_eq!(
            TestCheck::item(errors.diagnostics(), 0)?.code(),
            if fail_at.is_some() {
                DiagnosticCode::SOURCE_INPUT_ERROR
            } else {
                DiagnosticCode::CONFIG_VALIDATION_ERROR
            }
        )?;
    }

    Ok(())
}

#[test]
fn source_limit_requires_exactly_one_byte_eof_probe() -> TestResult<()> {
    let schema = compile("entry struct Root {}")?;
    for length in [0, 1, 2, 3] {
        let mut source = Source::new(b"{} ", vec![]);
        let mut options = ConfigParseOptions::default();
        let mut config = *options.config();
        config = config.with_max_source_bytes(length);
        options = ConfigParseOptions::new(*options.resources(), config, *options.execution())
            .with_stream(StreamLimits::try_new(7, 1024).test_value()?);
        let result = Support::parse(
            &schema,
            ConfigParseRequest::new(&mut source, "probe", options),
            &mut Provider::default(),
        );
        if length == 3 {
            test_assert!(result.is_ok())?;
        } else {
            test_eq!(
                TestCheck::item(result.test_error()?.diagnostics(), 0)?.code(),
                DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
            )?;
            test_eq!(source.offset, length + 1)?;
        }
        test_eq!(source.calls, if length == 0 { 1 } else { 2 })?;
    }

    Ok(())
}

#[test]
fn trailing_failures_are_atomic_and_invalid_utf8_has_no_fabricated_location() -> TestResult<()> {
    let schema = compile("entry struct Root {}")?;
    for tail in [
        b"?".as_slice(),
        b"\r",
        b"/*",
        b"\xc0\xaf",
        b"\xed\xa0\x80",
        b"\xf4\x90\x80\x80",
        b"\xf0\x9f\x98",
        b"//\xff",
    ] {
        let bytes = [b"{} ".as_slice(), tail].concat();
        for chunk in [1, 2, 3, 7, 8192] {
            let errors = run(
                &schema,
                &mut Source::new(&bytes, (1..bytes.len()).collect()),
                chunk,
            )
            .test_value()?
            .test_error()?;
            test_eq!(
                TestCheck::item(errors.diagnostics(), 0)?.code(),
                DiagnosticCode::LEXICAL_ERROR
            )?;
            if tail.iter().any(|byte| *byte >= 0x80) {
                test_assert!(
                    errors
                        .span_location(
                            TestCheck::item(errors.diagnostics(), 0)?
                                .primary()
                                .test_value()?
                        )
                        .is_none()
                )?;
            }
        }
    }

    Ok(())
}

#[test]
fn diagnostic_limit_and_token_limit_stop_reads_and_environment_calls() -> TestResult<()> {
    let schema = compile(r#"entry struct Root { n: u8 @env("X") }"#)?;
    let text = "{unknown: 0, n: env(\"X\")}";
    let mut source = Source::new(text.as_bytes(), vec![]);
    let mut provider = Provider::default();
    let defaults = ConfigParseOptions::default();
    let mut config = *defaults.config();
    config = config.with_max_diagnostics(1);
    let options = ConfigParseOptions::new(*defaults.resources(), config, *defaults.execution())
        .with_stream(StreamLimits::try_new(1, 1024).test_value()?);
    let errors = Support::parse(
        &schema,
        ConfigParseRequest::new(&mut source, "capped", options),
        &mut provider,
    )
    .test_error()?;
    test_eq!(
        TestCheck::item(errors.diagnostics(), 0)?.code(),
        DiagnosticCode::CONFIG_VALUE_ERROR
    )?;
    test_assert!(errors.is_truncated())?;
    test_assert!(source.offset < text.len())?;
    test_eq!(provider.calls, 0)?;
    let long = format!("{{n: {}}}", "0".repeat(1000));
    let mut source = Source::new(long.as_bytes(), vec![]);
    let errors = Support::parse(
        &schema,
        ConfigParseRequest::new(
            &mut source,
            "token",
            defaults.with_stream(StreamLimits::try_new(1, 4).test_value()?),
        ),
        &mut provider,
    )
    .test_error()?;
    test_eq!(
        TestCheck::item(errors.diagnostics(), 0)?.code(),
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
    )?;
    test_assert!(source.offset <= 10)?;

    Ok(())
}

#[test]
fn long_comments_and_dense_newlines_do_not_accumulate_source_storage() -> TestResult<()> {
    let schema = compile("entry struct Root {}")?;
    let mut budget_errors = TestSlot::<BudgetError>::default();
    let mut parse_errors = TestSlot::<ConfigParseErrors>::default();
    let mut baseline = None;
    let limits = ExcerptLimits::new(4096, 128, 16, 2);
    for enabled in [false, true] {
        let mut peaks = Vec::new();
        for length in [1, 1000, 100_000] {
            let text = format!("/*{}*/{{}}//{}", "\n".repeat(length), "ø".repeat(length));
            let defaults = ConfigParseOptions::default();
            let mut resources = *defaults.resources();
            resources.set_excerpts(if enabled {
                limits
            } else {
                limits.with_max_bytes(0)
            });
            let options = ConfigParseOptions::new(
                resources,
                *defaults.config(),
                ExecutionLimits::new(16_777_216, 65536),
            )
            .with_stream(StreamLimits::try_new(7, 1024).test_value()?);
            let budget = require_opeo(&mut budget_errors, |out| {
                ExecutionBudget::new(*options.execution(), out)
            })?;
            let initial = budget.workspace_bytes();
            let value = require_opeo(&mut parse_errors, |out| {
                ConfigParser::new(&schema).parse_with_budget(
                    ConfigParseRequest::new(
                        &mut SliceSource::new(text.as_bytes()),
                        "memory",
                        options,
                    ),
                    &mut Provider::default(),
                    &budget,
                    out,
                )
            })?;
            peaks.push(budget.peak_workspace_bytes());
            drop(value);
            test_eq!(budget.workspace_bytes(), initial)?;
        }
        let first_peak = *peaks.first().test_value()?;
        if enabled {
            // The optional window allows limited extra capacity; both old and new capacities count toward the peak during reallocation.
            let maximum = baseline.test_value()? + limits.max_bytes() * 2;
            test_assert!(peaks.iter().all(|peak| *peak <= maximum))?;
        } else {
            test_assert!(peaks.iter().all(|peak| *peak == first_peak))?;
            baseline = Some(first_peak);
        }
    }

    Ok(())
}

#[test]
fn input_failure_preserves_earlier_diagnostics_and_uses_explicit_stage() -> TestResult<()> {
    let schema = compile("entry struct Root { n: u8 }")?;
    let text = b"{n: 999, ";
    let mut source = Source::new(text, vec![]);
    source.fail_at = Some(text.len());
    let errors = run(&schema, &mut source, 1).test_value()?.test_error()?;
    test_eq!(errors.stage(), ConfigFailureStage::Input)?;
    test_eq!(errors.diagnostics().len(), 2)?;
    test_eq!(
        TestCheck::item(errors.diagnostics(), 0)?.code(),
        DiagnosticCode::CONFIG_VALUE_ERROR
    )?;
    test_eq!(
        TestCheck::item(errors.diagnostics(), 1)?.code(),
        DiagnosticCode::SOURCE_INPUT_ERROR
    )?;

    Ok(())
}

fn snapshot<'a>(input: impl Into<ValueViewInput<'a>>) -> String {
    let value = input.into().value_view();
    let content = match value.as_ref() {
        ValueKind::Signed(number) => format!("i{number}"),
        ValueKind::Unsigned(number) => format!("u{number}"),
        ValueKind::Float32(number) => format!("f32:{}", number.to_bits()),
        ValueKind::Float64(number) => format!("f64:{}", number.to_bits()),
        ValueKind::String(text) => format!("{text:?}"),
        ValueKind::Bool(value) => format!("b{value}"),
        ValueKind::Optional(value) => value.map_or_else(|| "none".into(), snapshot),
        ValueKind::Array(values) => values
            .map(|entry| snapshot(entry.value()))
            .collect::<Vec<_>>()
            .join(","),
        ValueKind::Map(entries) => entries
            .map(|entry| {
                format!(
                    "{:?}:{}",
                    entry.name().unwrap_or_default(),
                    snapshot(entry.value())
                )
            })
            .collect::<Vec<_>>()
            .join(","),
        ValueKind::Structure(fields) => fields
            .map(|field| {
                format!(
                    "{}:{}",
                    field.name().unwrap_or_default(),
                    snapshot(field.value())
                )
            })
            .collect::<Vec<_>>()
            .join(","),
        ValueKind::Enumeration { variant, payload } => {
            let payload = match payload {
                ValuePayloadView::Unit => "unit".into(),
                ValuePayloadView::Single(value) => snapshot(value),
                ValuePayloadView::Structure(fields) => fields
                    .map(|field| {
                        format!(
                            "{}:{}",
                            field.name().unwrap_or_default(),
                            snapshot(field.value())
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(","),
            };
            format!(".{variant}({payload})")
        }
    };
    format!("{:?}:{content}", value.source())
}

#[test]
fn ordinary_errors_preserve_order_paths_and_locations_at_every_cut() -> TestResult<()> {
    let schema = compile(
        r#"entry struct Root { n: u8 @env("X") = 1, values: [u8] @all(item > 0, "positive") @unique = [], text: option<string> }"#,
    )?;
    for text in [
        "{n: \"éø😀\", values: [1]}",
        "{\r\nvalues: [0, /*éø😀*/1, 1]}\n",
        "{n: env(\"X\", 999), values: [1]}",
        "{text: \"éø😀\\q\", values: [1]}",
        "{n: ?}",
    ] {
        let expected = run(&schema, &mut SliceSource::new(text.as_bytes()), 8192)
            .test_value()?
            .test_error()?;
        for chunk in [1, 2, 3, 7, 8192] {
            for cut in 1..text.len() {
                let actual = run(&schema, &mut Source::new(text.as_bytes(), vec![cut]), chunk)
                    .test_value()?
                    .test_error()?;
                test_eq!(actual.diagnostics().len(), expected.diagnostics().len())?;
                test_eq!(actual.stage(), expected.stage())?;
                for (actual_error, expected_error) in
                    actual.diagnostics().iter().zip(expected.diagnostics())
                {
                    test_eq!(actual_error.code(), expected_error.code())?;
                    test_eq!(actual_error.message(), expected_error.message())?;
                    test_eq!(actual_error.primary(), expected_error.primary())?;
                    test_eq!(actual_error.value_source(), expected_error.value_source())?;
                    test_eq!(
                        format!("{:?}", actual_error.path()),
                        format!("{:?}", expected_error.path())
                    )?;
                    test_eq!(actual_error.labels().len(), expected_error.labels().len())?;
                    for (actual_label, expected_label) in
                        actual_error.labels().iter().zip(expected_error.labels())
                    {
                        test_eq!(actual_label.span(), expected_label.span())?;
                        test_eq!(actual_label.message(), expected_label.message())?;
                    }
                    for span in actual_error
                        .primary()
                        .into_iter()
                        .chain(actual_error.labels().iter().map(|label| label.span()))
                    {
                        test_eq!(actual.span_location(span), expected.span_location(span))?;
                    }
                }
            }
        }
    }

    Ok(())
}
