use typol_test_support::ConfigTestSupport as Support;

use typol_config::{
    ConfigLimits, ConfigParseErrors, ConfigParseOptions, ConfigParseRequest, Environment,
    EnvironmentError, EnvironmentReadContext,
    opeo::{OResult, Out},
};
use typol_core::{
    AllocationError, BudgetError, ByteSource, Diagnostic, DiagnosticCode, ExcerptLimits,
    ExcerptStatus, ExecutionBudget, ExecutionLimits, LineIndex, LocatedSpan, LocationError,
    SliceSource, SourceCapture, SourceExcerptSet, SourceId, SourceReadError, SourceReadErrorKind,
    SourceSpan, Span, SpanError, SpanLocation,
};
use typol_policy::ResourceLimits;
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

struct ExcerptCapture<'a> {
    excerpts: &'a mut SourceExcerptSet,
    limits: ExcerptLimits,
    budget: Option<&'a ExecutionBudget>,
    source: &'a str,
    span: SourceSpan,
    location: SpanLocation,
    redactions: &'a [Span],
}

fn capture_excerpt(request: ExcerptCapture<'_>) {
    let ExcerptCapture {
        excerpts,
        limits,
        budget,
        source,
        span,
        location,
        redactions,
    } = request;
    let bytes = source.as_bytes();
    let mut capture = SourceCapture::new(span.source_id(), limits, budget);
    capture.pin(span, Some(location));
    let mut cursor = 0;
    for redaction in redactions {
        let start = redaction.start().min(bytes.len());
        let end = redaction.end().min(bytes.len());
        if start < cursor {
            capture.invalidate();
            break;
        }
        let Some(prefix) = bytes.get(cursor..start) else {
            capture.invalidate();
            break;
        };
        capture.feed(prefix);
        let Some(secret) = capture.begin_secret(redaction.start()) else {
            capture.invalidate();
            break;
        };
        let Some(secret_bytes) = bytes.get(start..end) else {
            capture.invalidate();
            break;
        };
        capture.feed(secret_bytes);
        capture.end_secret(secret, redaction.end());
        cursor = end;
    }
    if let Some(tail) = bytes.get(cursor..) {
        capture.feed(tail);
    } else {
        capture.invalidate();
    }
    capture.end_input();
    let mut diagnostic = Diagnostic::emergency(DiagnosticCode::CONFIG_SYNTAX_ERROR);
    diagnostic.set_located_primary(Some(LocatedSpan::new(span, Some(location))));
    capture.finish(excerpts, &[diagnostic]);
}

struct NoEnvironment;

impl Environment for NoEnvironment {
    fn read<'env, 'slot, 'borrow>(
        &'env mut self,
        _: &str,
        _: EnvironmentReadContext<'_>,
        _: Out<'slot, 'borrow, EnvironmentError>,
    ) -> OResult<'slot, Option<&'env [u8]>> {
        Ok(None)
    }
}

fn compile(source: &str) -> TestResult<CompiledSchema> {
    Support::compile_schema(
        SchemaCompileOptions::default(),
        SchemaSource::from_str(b"test", source),
        None,
        None,
    )
    .test_value()
}

fn parse(
    schema: &CompiledSchema,
    source: &str,
    limits: ExcerptLimits,
) -> TestResult<ConfigParseErrors> {
    let mut resources = ResourceLimits::default();
    resources.set_excerpts(limits);
    Support::parse(
        schema,
        ConfigParseRequest::new(
            &mut SliceSource::new(source.as_bytes()),
            "configuration",
            ConfigParseOptions::default().with_resources(resources),
        ),
        &mut NoEnvironment,
    )
    .test_error()
}

fn text(errors: &ConfigParseErrors) -> String {
    errors
        .excerpts()
        .records()
        .iter()
        .map(|record| record.text())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn adjacent_secrets_are_masked_and_schema_labels_survive_drop() -> TestResult {
    let schema = compile("entry struct Root { value: u8 @min(2), password: string @secret }")?;
    let errors = parse(
        &schema,
        r#"{value: 1, password: "PRIVATE-SENTINEL"}"#,
        ExcerptLimits::default(),
    )?;
    let span = errors
        .diagnostics()
        .first()
        .test_value()?
        .primary()
        .test_value()?;
    let label = errors
        .diagnostics()
        .first()
        .test_value()?
        .labels()
        .first()
        .test_value()?
        .span();
    drop(schema);
    test_assert!(text(&errors).contains("value: 1"))?;
    test_assert!(text(&errors).contains("<redacted>"))?;
    test_assert!(!text(&errors).contains("PRIVATE-SENTINEL"))?;
    let report = errors.to_string();
    test_assert!(report.contains("CONFIG_VALIDATION_ERROR (12)"))?;
    test_assert!(!report.contains("PRIVATE-SENTINEL"))?;
    test_assert!(report.contains("<redacted>"))?;
    test_assert!(errors.excerpts().columns(span, 1).is_some())?;
    test_ne!(
        errors.excerpts().span_status(label),
        ExcerptStatus::Unavailable
    )?;
    test_assert!(!format!("{:?}", errors.excerpts()).contains("value:"))?;
    Ok(())
}

#[test]
fn aliases_containers_and_secret_map_keys_are_masked() -> TestResult {
    let cases = [
        (
            "type S = string @secret; type A = S; entry struct Root { value: A @length(2, 4) }",
            r#"{value: "PRIVATE-SENTINEL"}"#,
        ),
        (
            "entry struct Root { value: map<string, u8> @secret }",
            r#"{value: {"PRIVATE-SENTINEL": 999}}"#,
        ),
        (
            "type S = u8 @secret; entry struct Root { value: option<map<string, S>> }",
            r#"{value: {"PRIVATE-SENTINEL": 999}}"#,
        ),
        (
            "entry struct Root { value: [u8] @secret }",
            "{value: [999]}",
        ),
    ];
    for (declaration, input) in cases {
        let schema = compile(declaration)?;
        let errors = parse(&schema, input, ExcerptLimits::default())?;
        test_assert!(!text(&errors).contains("PRIVATE-SENTINEL"))?;
        test_assert!(!text(&errors).contains("999"))?;
    }
    Ok(())
}

#[test]
fn unfinished_secrets_and_unknown_fields_never_publish_readahead() -> TestResult {
    let schema = compile("entry struct Root { password: string @secret, value: u8 }")?;
    for input in [
        r#"{password: "PRIVATE-SENTINEL""#,
        r#"{unknown: "PRIVATE-SENTINEL", password: "PRIVATE-NEXT", value: 999}"#,
        r#"{password: "PRIVATE-SENTINEL" } EXTRA-PRIVATE"#,
    ] {
        let errors = parse(&schema, input, ExcerptLimits::default())?;
        test_assert!(!text(&errors).contains("PRIVATE"))?;
        test_assert!(!errors.diagnostics().is_empty())?;
    }
    Ok(())
}

#[test]
fn unicode_crlf_tab_eof_and_tiny_line_limits_remain_valid() -> TestResult {
    let schema = compile("entry struct Root { value: string @length(1, 1) }")?;
    for limit in [1, 2, 3, 4, 9, 512] {
        let errors = parse(
            &schema,
            "{\r\n\tvalue: \"éø🙂\"\r\n}",
            ExcerptLimits::default().with_max_line_bytes(limit),
        )?;
        for record in errors.excerpts().records() {
            test_assert!(record.text().len() <= limit)?;
            test_assert!(!record.text().contains('\r'))?;
        }
        test_eq!(
            errors.diagnostics().first().test_value()?.code(),
            DiagnosticCode::CONFIG_VALIDATION_ERROR
        )?;
    }
    let errors = parse(&schema, "{value:", ExcerptLimits::default())?;
    test_assert!(!errors.diagnostics().is_empty())?;
    Ok(())
}

#[test]
fn zero_and_exhausted_snapshot_limits_preserve_diagnostics() -> TestResult {
    let schema = compile("entry struct Root { value: u8 @min(2) }")?;
    for limits in [
        ExcerptLimits::new(0, 512, 128, 2),
        ExcerptLimits::new(1, 512, 128, 2),
        ExcerptLimits::new(4096, 512, 0, 2),
    ] {
        let errors = parse(&schema, "{value: 1}", limits)?;
        test_eq!(errors.excerpts().status(), ExcerptStatus::Unavailable)?;
        test_assert!(errors.excerpts().records().is_empty())?;
        test_eq!(
            errors.diagnostics().first().test_value()?.code(),
            DiagnosticCode::CONFIG_VALIDATION_ERROR
        )?;
    }
    Ok(())
}

struct Chunks<'a> {
    bytes: &'a [u8],
    fail: bool,
}

impl ByteSource for Chunks<'_> {
    fn read<'slot, 'borrow>(
        &mut self,
        output: &mut [u8],
        source_id: SourceId,
        offset: usize,
        out: Out<'slot, 'borrow, SourceReadError>,
    ) -> OResult<'slot, usize> {
        if self.bytes.is_empty() {
            return if self.fail {
                Err(out.fail(
                    SourceReadError::new(SourceReadErrorKind::HostFailure)
                        .with_input(source_id, offset),
                ))
            } else {
                Ok(0)
            };
        }
        let count = output.len().min(1).min(self.bytes.len());
        let Some(bytes) = self.bytes.get(..count) else {
            return Err(out.fail(
                SourceReadError::new(SourceReadErrorKind::HostFailure)
                    .with_input(source_id, offset),
            ));
        };
        let Some(destination) = output.get_mut(..count) else {
            return Err(out.fail(
                SourceReadError::new(SourceReadErrorKind::HostFailure)
                    .with_input(source_id, offset),
            ));
        };
        destination.copy_from_slice(bytes);
        let Some(bytes) = self.bytes.get(count..) else {
            return Err(out.fail(
                SourceReadError::new(SourceReadErrorKind::HostFailure)
                    .with_input(source_id, offset),
            ));
        };
        self.bytes = bytes;
        Ok(count)
    }
}

#[test]
fn nonrewindable_chunks_and_input_failure_keep_owned_safe_snapshots() -> TestResult {
    let schema = compile("entry struct Root { value: u8 @min(2), password: string @secret }")?;
    for fail in [false, true] {
        let errors = {
            let mut source = Chunks {
                bytes: b"{value: 1,\npassword: \"PRIVATE-SENTINEL\"}\n",
                fail,
            };
            Support::parse(
                &schema,
                ConfigParseRequest::new(&mut source, "stream", ConfigParseOptions::default()),
                &mut NoEnvironment,
            )
            .test_error()?
        };
        test_assert!(!text(&errors).contains("PRIVATE-SENTINEL"))?;
        test_assert!(text(&errors).contains("value: 1"))?;
    }
    Ok(())
}

#[test]
fn delayed_collection_errors_retain_early_candidates() -> TestResult {
    let schema = compile("entry struct Root { values: [u8] @unique }")?;
    let errors = parse(
        &schema,
        "{values: [\n1,\n2,\n1\n]}",
        ExcerptLimits::default(),
    )?;
    test_assert!(text(&errors).contains('1'))?;
    test_assert!(!errors.excerpts().records().is_empty())?;
    Ok(())
}

#[test]
fn masking_adjusts_columns_and_invalid_ranges_are_unavailable() -> TestResult {
    let source = "\té password=\"PRIVATE-SENTINEL\" tail=bad\r\n";
    let start = source.find("bad").test_value()?;
    let secret_start = source.find('"').test_value()?;
    let secret_end = source.rfind('"').test_value()? + 1;
    let mut allocation_errors = TestSlot::<AllocationError>::default();
    let mut location_errors = TestSlot::<LocationError>::default();
    let mut span_errors = TestSlot::<SpanError>::default();
    let index = require_opeo(&mut allocation_errors, |out| LineIndex::new(source, out))?;
    let span = SourceSpan::new(
        SourceId::new(0),
        require_opeo(&mut span_errors, |out| Span::new(start, start + 3, out))?,
    );
    let location = SpanLocation {
        start: require_opeo(&mut location_errors, |out| index.location(start, out))?,
        end: require_opeo(&mut location_errors, |out| index.location(start + 3, out))?,
    };
    let ranges = [require_opeo(&mut span_errors, |out| {
        Span::new(secret_start, secret_end, out)
    })?];
    let limits = ExcerptLimits::default().with_context_lines(0);
    let mut excerpts = SourceExcerptSet::new(limits, None);
    capture_excerpt(ExcerptCapture {
        excerpts: &mut excerpts,
        limits,
        budget: None,
        source,
        span,
        location,
        redactions: &ranges,
    });
    let line = excerpts.records().first().test_value()?;
    test_eq!(line.text(), "\té password=<redacted> tail=bad")?;
    let expected_start = "\té password=<redacted> tail=".chars().count() + 1;
    test_eq!(
        excerpts.columns(span, 1),
        Some((expected_start, expected_start + 3))
    )?;
    let invalid_limits = ExcerptLimits::default();
    let mut invalid = SourceExcerptSet::new(invalid_limits, None);
    let invalid_redactions = [Span::from_zero_to(source.len() + 1)];
    capture_excerpt(ExcerptCapture {
        excerpts: &mut invalid,
        limits: invalid_limits,
        budget: None,
        source,
        span,
        location,
        redactions: &invalid_redactions,
    });
    test_eq!(invalid.span_status(span), ExcerptStatus::Unavailable)?;
    test_assert!(invalid.records().is_empty())?;
    Ok(())
}

#[test]
fn excerpt_status_and_copy_are_scoped_to_each_span() -> TestResult {
    let source = "first\nsecond\n";
    let mut allocation_errors = TestSlot::<AllocationError>::default();
    let mut location_errors = TestSlot::<LocationError>::default();
    let mut span_errors = TestSlot::<SpanError>::default();
    let index = require_opeo(&mut allocation_errors, |out| LineIndex::new(source, out))?;
    let complete_span = SourceSpan::new(
        SourceId::new(0),
        require_opeo(&mut span_errors, |out| Span::new(0, 5, out))?,
    );
    let incomplete_span = SourceSpan::new(
        SourceId::new(0),
        require_opeo(&mut span_errors, |out| Span::new(6, source.len() + 1, out))?,
    );
    let first_location = SpanLocation {
        start: require_opeo(&mut location_errors, |out| index.location(0, out))?,
        end: require_opeo(&mut location_errors, |out| index.location(5, out))?,
    };
    let second_location = SpanLocation {
        start: require_opeo(&mut location_errors, |out| index.location(6, out))?,
        end: require_opeo(&mut location_errors, |out| index.location(12, out))?,
    };
    let limits = ExcerptLimits::default().with_context_lines(0);
    let mut source_excerpts = SourceExcerptSet::new(limits, None);
    capture_excerpt(ExcerptCapture {
        excerpts: &mut source_excerpts,
        limits,
        budget: None,
        source,
        span: complete_span,
        location: first_location,
        redactions: &[],
    });
    capture_excerpt(ExcerptCapture {
        excerpts: &mut source_excerpts,
        limits,
        budget: None,
        source,
        span: incomplete_span,
        location: second_location,
        redactions: &[],
    });
    test_eq!(source_excerpts.status(), ExcerptStatus::Truncated)?;
    test_eq!(
        source_excerpts.span_status(complete_span),
        ExcerptStatus::Complete
    )?;
    test_eq!(
        source_excerpts.span_status(incomplete_span),
        ExcerptStatus::Unavailable
    )?;

    let mut copied = SourceExcerptSet::new(limits, None);
    copied.copy_span(&source_excerpts, complete_span);
    test_eq!(copied.span_status(complete_span), ExcerptStatus::Complete)?;
    test_eq!(copied.status(), ExcerptStatus::Complete)?;
    test_assert!(
        copied
            .records()
            .iter()
            .any(|record| record.text() == "first")
    )?;
    Ok(())
}

#[test]
fn optional_budget_exhaustion_does_not_poison_business_budget() -> TestResult {
    let mut budget_errors = TestSlot::<BudgetError>::default();
    let budget = require_opeo(&mut budget_errors, |out| {
        ExecutionBudget::new(ExecutionLimits::new(100, 4096), out)
    })?;
    let retained = budget.workspace_bytes();
    test_assert!(!budget.consume_optional(101))?;
    test_assert!(!budget.exhausted())?;
    test_eq!(budget.failure(), None)?;
    require_opeo(&mut budget_errors, |out| {
        budget.consume_with(1, out, |error| error)
    })?;
    let limits = ExcerptLimits::new(1_048_576, 512, 128, 2);
    let snapshots = SourceExcerptSet::new(limits, Some(&budget));
    test_assert!(snapshots.storage_bytes() <= limits.max_bytes())?;
    drop(snapshots);
    test_eq!(budget.workspace_bytes(), retained)?;
    test_eq!(budget.failure(), None)?;
    Ok(())
}

#[test]
fn multiline_spans_and_unbounded_line_setting_still_use_bounded_context() -> TestResult {
    let source = format!("begin\n{}end\n", "line\n".repeat(100));
    let mut allocation_errors = TestSlot::<AllocationError>::default();
    let mut location_errors = TestSlot::<LocationError>::default();
    let index = require_opeo(&mut allocation_errors, |out| LineIndex::new(&source, out))?;
    let span = SourceSpan::new(SourceId::new(0), Span::from_zero_to(source.len() - 1));
    let location = SpanLocation {
        start: require_opeo(&mut location_errors, |out| index.location(0, out))?,
        end: require_opeo(&mut location_errors, |out| {
            index.location(source.len() - 1, out)
        })?,
    };
    let limits = ExcerptLimits::new(4096, usize::MAX, 8, 2);
    let mut excerpts = SourceExcerptSet::new(limits, None);
    capture_excerpt(ExcerptCapture {
        excerpts: &mut excerpts,
        limits,
        budget: None,
        source: &source,
        span,
        location,
        redactions: &[],
    });
    test_assert!(excerpts.storage_bytes() <= limits.max_bytes())?;
    test_assert!(excerpts.records().len() <= 2)?;
    test_eq!(excerpts.span_status(span), ExcerptStatus::Truncated)?;
    Ok(())
}

#[test]
fn distant_excerpt_capture_keeps_the_requested_line_within_limits() -> TestResult {
    let source = format!("{}target = 1\n", "padding\n".repeat(8192));
    let start = source.find("target").test_value()?;
    let end = start + "target".len();
    let mut allocation_errors = TestSlot::<AllocationError>::default();
    let mut location_errors = TestSlot::<LocationError>::default();
    let mut span_errors = TestSlot::<SpanError>::default();
    let mut budget_errors = TestSlot::<BudgetError>::default();
    let span = SourceSpan::new(
        SourceId::new(0),
        require_opeo(&mut span_errors, |out| Span::new(start, end, out))?,
    );
    let index = require_opeo(&mut allocation_errors, |out| LineIndex::new(&source, out))?;
    let location = SpanLocation {
        start: require_opeo(&mut location_errors, |out| index.location(start, out))?,
        end: require_opeo(&mut location_errors, |out| index.location(end, out))?,
    };
    let limits = ExcerptLimits::default().with_context_lines(0);
    let budget = require_opeo(&mut budget_errors, |out| {
        ExecutionBudget::new(ExecutionLimits::new(source.len() * 2, 4096), out)
    })?;
    let mut excerpts = SourceExcerptSet::new(limits, Some(&budget));
    capture_excerpt(ExcerptCapture {
        excerpts: &mut excerpts,
        limits,
        budget: Some(&budget),
        source: &source,
        span,
        location,
        redactions: &[],
    });

    test_eq!(excerpts.span_status(span), ExcerptStatus::Complete)?;
    test_assert!(
        excerpts
            .records()
            .iter()
            .any(|record| record.text() == "target = 1")
    )?;
    test_assert!(!budget.exhausted())?;
    Ok(())
}

#[test]
fn missing_window_candidates_and_diagnostic_limit_keep_original_errors() -> TestResult {
    let schema = compile("entry struct Root { values: [u8] @unique }")?;
    let source = format!("{{values: [\n{}1\n]}}", "1,\n".repeat(20));
    let errors = parse(&schema, &source, ExcerptLimits::new(4096, 64, 1, 0))?;
    test_assert!(!errors.diagnostics().is_empty())?;
    test_ne!(errors.excerpts().status(), ExcerptStatus::Complete)?;
    test_assert!(errors.excerpts().records().len() <= 1)?;

    let mut config = ConfigLimits::default();
    config = config.with_max_diagnostics(1);
    let resources = ResourceLimits::default();
    let errors = Support::parse(
        &schema,
        ConfigParseRequest::new(
            &mut SliceSource::new(b"{values: [1,1]}"),
            "configuration",
            ConfigParseOptions::default()
                .with_resources(resources)
                .with_config(config),
        ),
        &mut NoEnvironment,
    )
    .test_error()?;
    test_eq!(
        errors.diagnostics().last().test_value()?.code(),
        DiagnosticCode::CONFIG_VALIDATION_ERROR
    )?;
    test_assert!(errors.is_truncated())?;
    Ok(())
}
