use super::{Parser, ParserTestSupport};
use crate::{
    ConfigLimits, ConfigParseOptions, ConfigParseRequest, ConfigParser,
    diagnostics::{ConfigParseErrors, DiagnosticBuffer, DiagnosticFactory},
    environment::{
        CachedEnvironment, Environment, EnvironmentCache, EnvironmentError, EnvironmentReadContext,
        EnvironmentReadRequest,
    },
    session::ConfigSession,
};
use alloc::{format, string::String, vec::Vec};
use core::mem::size_of;
use opeo::{ErrSlot, OResult, Out};
use typol_core::{
    AllocationError, BudgetError, ChargedVec, Diagnostic, DiagnosticCode, DiagnosticMessage,
    ExecutionBudget, ExecutionLimits, InstancePath, LineIndex, LocatedSpan, LocationError,
    PathSegment, Read, SliceSource, SourceId, Span, SpanError, SpanLocation,
};
use typol_policy::{ExcerptLimits, RegexOptions};
use typol_semantic::{ConstraintKind, TypeKind};
use typol_test_support::*;

pub(super) struct Provider {
    calls: usize,
    result: Result<Option<&'static [u8]>, EnvironmentError>,
}

impl Provider {
    pub(super) fn new(result: Result<Option<&'static [u8]>, EnvironmentError>) -> Self {
        Self { calls: 0, result }
    }

    pub(super) fn calls(&self) -> usize {
        self.calls
    }

    pub(super) fn set_result(&mut self, result: Result<Option<&'static [u8]>, EnvironmentError>) {
        self.result = result;
    }
}

fn test_source_span(
    source_id: SourceId,
    start: usize,
    end: usize,
    errors: &mut TestSlot<SpanError>,
) -> LocatedSpan {
    let span = errors
        .run(|out| Span::new(start, end, out))
        .value()
        .copied()
        .unwrap_or(Span::empty());
    DiagnosticFactory::source_span(source_id, span)
}

impl Environment for Provider {
    fn read<'env, 'slot, 'borrow>(
        &'env mut self,
        _: &str,
        _: EnvironmentReadContext<'_>,
        out: Out<'slot, 'borrow, EnvironmentError>,
    ) -> OResult<'slot, Option<&'env [u8]>> {
        self.calls += 1;
        match self.result {
            Ok(value) => Ok(value),
            Err(error) => Err(out.fail(error)),
        }
    }
}

#[test]
fn stable_locations_survive_schema_field_reordering_and_value_moves() -> TestResult<()> {
    let schema = ParserTestSupport::compile(
        r#"
        struct Child { text: string }
        enum E { One(Child), Object { text: string } }
        entry struct Root {
            first: [Child], second: map<string, option<Child>>, variant: E,
            default: Child = {text: "default"}, default_variant: E = .Object {text: "default"}
        }
    "#,
    )?;
    let mut provider = Provider::new(Ok(None));
    let (value, locations) = ParserTestSupport::parse_local_value(&schema,
        r#"{variant: .One({text: "payload"}), second: {"two": {text: "second"}, "one": none}, first: [{text: "first"}]}"#,
        &mut provider, &ConfigLimits::default()).test_value()?;
    let moved = Some(value);
    let root = moved.as_ref().test_value()?;
    let mut schema_errors = TestSlot::default();
    let mut path_errors = TestSlot::default();
    let paths: Vec<_> = locations
        .nodes()
        .map(|(id, node)| {
            test_assert!(node.complete())?;
            let path = path_errors
                .run(|out| locations.path(id, root, out))
                .test_value()?;
            Ok(path_text(path.segments()))
        })
        .collect::<TestResult<Vec<_>>>()?;
    test_eq!(
        paths,
        [
            "",
            ".variant",
            ".variant:One",
            ".variant:One.text",
            ".second",
            ".second[two]",
            ".second[two]",
            ".second[two].text",
            ".second[one]",
            ".first",
            ".first[0]",
            ".first[0].text",
            ".default",
            ".default.text",
            ".default_variant",
            ".default_variant:Object.text"
        ]
    )?;
    let field = ParserTestSupport::required(&mut schema_errors, |out| {
        schema.entry().field_named("default", out)
    })?;
    let span = ParserTestSupport::access(&mut schema_errors, |out| field.span(out))?;
    test_assert!(
        locations
            .nodes()
            .any(|(_, node)| node.span().map(|span| span.source_span()) == Some(span))
    )?;
    test_eq!(provider.calls(), 0)?;

    Ok(())
}

fn path_text(segments: &[PathSegment]) -> String {
    let mut text = String::new();
    for segment in segments {
        match segment {
            PathSegment::Field(name) => {
                text.push('.');
                text.push_str(name);
            }
            PathSegment::Index(index) => text.push_str(&format!("[{index}]")),
            PathSegment::MapKey(key) => {
                text.push('[');
                text.push_str(key);
                text.push(']');
            }
            PathSegment::EnumPayload(name) => {
                text.push(':');
                text.push_str(name);
            }
        }
    }
    text
}

#[test]
fn mutable_provider_cache_and_real_assertions_share_one_budget() -> TestResult<()> {
    let schema = ParserTestSupport::compile(
        r#"entry struct Root { first: u8 @env("X"), second: u8 @env("X") }
        @assert(false, "one") @assert(false, "two") @assert(false, "three")"#,
    )?;
    let mut provider = Provider::new(Ok(Some(b"7")));
    let mut limits = ConfigLimits::default();
    limits = limits.with_max_diagnostics(3);
    let mut parse_errors = TestSlot::default();
    let errors = parse_errors
        .run(|out| {
            ConfigParser::new(&schema).parse(
                config_request(
                    (
                        &mut SliceSource::new(
                            (r#"{first: env("X"), second: env("X")}"#).as_bytes(),
                        ),
                        "test",
                    ),
                    &limits,
                ),
                &mut provider,
                out,
            )
        })
        .test_error()?;
    test_eq!(ParserTestSupport::codes(&errors), [12, 12, 5])?;
    test_eq!(provider.calls(), 1)?;

    Ok(())
}

#[test]
fn owned_errors_keep_unicode_crlf_and_label_locations_after_source_drop() -> TestResult<()> {
    let schema = ParserTestSupport::compile("entry struct Root { value: u8, }")?;
    let limits = ConfigLimits::default();
    let errors = {
        let source = String::from("{\r\n/*é*/ value: 999,\r\n}");
        let name = String::from("memory.tconf");
        let mut provider = Provider::new(Ok(None));
        let options = session_options(&limits);
        let mut budget_errors = ErrSlot::new();
        let mut session_errors = TestSlot::default();
        let session = session_errors
            .run(|out| ConfigSession::new(&mut provider, &options, &mut budget_errors, out))
            .test_value()?;
        let mut parser_input_1 = SliceSource::new(source.as_bytes());
        let mut parse_errors = TestSlot::default();
        let mut parser = parse_errors
            .run(|out| Parser::new(&schema, &mut parser_input_1, &limits, session, out))
            .test_value()?;
        parse_errors
            .run(|out| parser.set_source_name(&name, out))
            .test_value()?;
        parse_errors.run(|out| parser.parse(out)).test_error()?
    };
    test_eq!(errors.config_source_name(), Some("memory.tconf"))?;
    test_eq!(errors.config_source_id(), schema.next_source_id())?;
    let span = TestCheck::item(errors.diagnostics(), 0)?
        .primary()
        .test_value()?;
    let location = errors.span_location(span).test_value()?;
    test_eq!((location.start.line(), location.start.column()), (2, 14))?;
    test_eq!((location.end.line(), location.end.column()), (2, 17))?;

    Ok(())
}

#[test]
fn cache_distinguishes_missing_empty_invalid_utf8_and_host_failure() -> TestResult<()> {
    let limits = ConfigLimits::default();
    let mut span_errors = TestSlot::default();
    let span = test_source_span(SourceId::new(1), 0, 0, &mut span_errors);
    let schema = ParserTestSupport::compile(r#"entry struct Root { x: u8 @env("X") }"#)?;
    let mut schema_errors = TestSlot::default();
    let field = ParserTestSupport::required(&mut schema_errors, |out| {
        schema.entry().field_named("x", out)
    })?;
    let authorization =
        ParserTestSupport::required(&mut schema_errors, |out| field.environment(out))?;
    let mut budget_errors = TestSlot::<BudgetError>::default();
    let mut diagnostic_errors = TestSlot::<Diagnostic>::default();
    for result in [
        Ok(None),
        Ok(Some(&b""[..])),
        Ok(Some(&b"\xff"[..])),
        Err(EnvironmentError::HostFailure),
    ] {
        let mut provider = Provider::new(result);
        let budget = budget_errors
            .run(|out| ExecutionBudget::new(ExecutionLimits::new(1_000_000, 1_000_000), out))
            .test_value()?;
        let mut cache = budget_errors
            .run(|out| EnvironmentCache::new(&budget, *session_options(&limits).config(), out))
            .test_value()?;
        for _ in 0..2 {
            let cached = diagnostic_errors
                .run(|out| {
                    cache.read(
                        EnvironmentReadRequest {
                            provider: &mut provider,
                            authorization,
                            limits: &limits,
                            span,
                        },
                        out,
                    )
                })
                .test_value()?;
            match (result, cached) {
                (Ok(None), CachedEnvironment::Missing)
                | (Err(EnvironmentError::HostFailure), CachedEnvironment::HostFailure) => {}
                (Ok(Some(expected)), CachedEnvironment::Bytes(bytes)) => {
                    test_eq!(bytes.as_slice(), expected)?
                }
                _ => {
                    return Err(TestFailure::new(
                        "environment cache did not preserve the original state",
                    ));
                }
            }
        }
        test_eq!(provider.calls(), 1)?;
    }

    Ok(())
}

#[test]
fn environment_cache_storage_tracks_only_loaded_schema_slots() -> TestResult<()> {
    let mut schema_source = String::from("entry struct Root {");
    for slot in 0..256 {
        schema_source.push_str(&format!(" field_{slot}: u8 @env(\"ENV_{slot}\"),"));
    }
    schema_source.push('}');
    let schema = ParserTestSupport::compile(&schema_source)?;
    let mut schema_errors = TestSlot::default();
    let field = ParserTestSupport::required(&mut schema_errors, |out| {
        schema.entry().field_named("field_0", out)
    })?;
    let authorization =
        ParserTestSupport::required(&mut schema_errors, |out| field.environment(out))?;
    let limits = ConfigLimits::default();
    let mut budget_errors = TestSlot::default();
    let budget = budget_errors
        .run(|out| ExecutionBudget::new(ExecutionLimits::new(1_000_000, 1_048_576), out))
        .test_value()?;
    let mut cache = budget_errors
        .run(|out| EnvironmentCache::new(&budget, limits, out))
        .test_value()?;
    let before_read = budget.workspace_bytes();
    let mut span_errors = TestSlot::default();
    let span = test_source_span(SourceId::new(1), 0, 0, &mut span_errors);
    let mut provider = Provider::new(Ok(None));
    let mut diagnostic_errors = TestSlot::<Diagnostic>::default();
    for _ in 0..2 {
        let cached = diagnostic_errors
            .run(|out| {
                cache.read(
                    EnvironmentReadRequest {
                        provider: &mut provider,
                        authorization,
                        limits: &limits,
                        span,
                    },
                    out,
                )
            })
            .test_value()?;
        test_assert!(matches!(cached, CachedEnvironment::Missing))?;
    }
    let allocated = budget.workspace_bytes() - before_read;
    test_assert!(allocated < 256 * size_of::<usize>())?;
    test_eq!(provider.calls(), 1)?;

    Ok(())
}

#[test]
fn environment_cache_capacity_failure_precedes_provider_reads() -> TestResult<()> {
    let schema = ParserTestSupport::compile(r#"entry struct Root { value: u8 @env("X") }"#)?;
    let mut schema_errors = TestSlot::default();
    let field = ParserTestSupport::required(&mut schema_errors, |out| {
        schema.entry().field_named("value", out)
    })?;
    let authorization =
        ParserTestSupport::required(&mut schema_errors, |out| field.environment(out))?;
    let limits = ConfigLimits::default();
    let execution_limits = ExecutionLimits::new(1_000_000, 4096);
    let mut budget_errors = TestSlot::default();
    let budget = budget_errors
        .run(|out| ExecutionBudget::new(execution_limits, out))
        .test_value()?;
    let mut cache = budget_errors
        .run(|out| EnvironmentCache::new(&budget, limits, out))
        .test_value()?;
    let mut filler = budget_errors
        .run(|out| ChargedVec::<u8>::new(Some(&budget), out))
        .test_value()?;
    let remaining = execution_limits
        .max_workspace_bytes()
        .checked_sub(budget.workspace_bytes())
        .test_value()?;
    budget_errors
        .run(|out| filler.try_reserve_exact(remaining - 1, out))
        .test_value()?;
    let before_read = budget.workspace_bytes();
    let mut span_errors = TestSlot::default();
    let span = test_source_span(SourceId::new(1), 0, 0, &mut span_errors);
    let mut provider = Provider::new(Ok(Some(b"7")));
    let mut diagnostic_errors = TestSlot::<Diagnostic>::default();
    let error = diagnostic_errors
        .run(|out| {
            cache.read(
                EnvironmentReadRequest {
                    provider: &mut provider,
                    authorization,
                    limits: &limits,
                    span,
                },
                out,
            )
        })
        .test_error()?;
    test_eq!(error.code(), DiagnosticCode::RESOURCE_LIMIT_EXCEEDED)?;
    test_eq!(provider.calls(), 0)?;
    test_eq!(budget.workspace_bytes(), before_read)?;

    Ok(())
}

#[test]
fn alias_struct_assertions_and_regex_handles_are_borrowed_from_compiled_schema() -> TestResult<()> {
    let schema = ParserTestSupport::compile(
        r#"
        struct Child { value: u8, } @assert(self.value > 0, "positive")
        type Alias = Child;
        type A = string @length(2, 8);
        type B = A @regex("^a");
        entry struct Root { children: [Alias], strings: [B], }
    "#,
    )?;
    let root = schema.entry();
    let mut schema_errors = TestSlot::default();
    let field =
        ParserTestSupport::required(&mut schema_errors, |out| root.field_named("children", out))?;
    let info = ParserTestSupport::access(&mut schema_errors, |out| field.ty(out))?;
    let TypeKind::Array { element, .. } =
        ParserTestSupport::access(&mut schema_errors, |out| info.kind(out))?
    else {
        return Err(TestFailure::new("children field should be an array"));
    };
    test_eq!(
        ParserTestSupport::access(&mut schema_errors, |out| element.assertions(out))?.len(),
        1
    )?;
    let field =
        ParserTestSupport::required(&mut schema_errors, |out| root.field_named("strings", out))?;
    let info = ParserTestSupport::access(&mut schema_errors, |out| field.ty(out))?;
    let TypeKind::Array { element, .. } =
        ParserTestSupport::access(&mut schema_errors, |out| info.kind(out))?
    else {
        return Err(TestFailure::new("strings field should be an array"));
    };
    let info = element;
    let constraints = ParserTestSupport::access(&mut schema_errors, |out| info.constraints(out))?;
    let kinds = schema_errors
        .run(|mut out| {
            let mut kinds = Vec::new();
            for constraint in constraints.iter(out.reborrow()) {
                let constraint = match constraint {
                    Ok(constraint) => constraint,
                    Err(failure) => return Err(failure),
                };
                kinds.push(constraint.kind());
            }
            Ok(kinds)
        })
        .test_value()?;
    test_eq!(kinds, [ConstraintKind::Regex, ConstraintKind::Length])?;
    test_assert!(core::ptr::eq(
        schema.regex_programs().next().test_value()?.regex(),
        schema.regex_programs().next().test_value()?.regex()
    ))?;
    test_eq!(schema.regex_programs().count(), 1)?;

    Ok(())
}

#[test]
fn semantic_diagnostics_stop_at_zero_one_and_two_effective_slots() -> TestResult<()> {
    let schema =
        ParserTestSupport::compile("type A = u8 @min(3); entry struct Root { value: A @min(4) }")?;
    for (budget, expected) in [(0, &[12][..]), (1, &[12][..]), (2, &[12, 5][..])] {
        let mut limits = ConfigLimits::default();
        limits = limits.with_max_diagnostics(budget);
        let errors =
            ParserTestSupport::parse(&schema, "{value: 2, later: @}", &limits).test_error()?;
        test_eq!(ParserTestSupport::codes(&errors), expected)?;
        test_assert!(errors.is_truncated())?;
    }

    Ok(())
}

#[test]
fn position_snapshots_cover_config_labels_eof_and_reject_invalid_spans() -> TestResult<()> {
    let schema = ParserTestSupport::compile("entry struct Root {}")?;
    let source_id = schema.next_source_id().test_value()?;
    let mut span_errors = TestSlot::default();
    let mut allocation_errors = TestSlot::<AllocationError>::default();
    let mut location_errors = TestSlot::<LocationError>::default();
    let primary = test_source_span(source_id, 3, 7, &mut span_errors);
    let label = test_source_span(source_id, 9, 9, &mut span_errors);
    let errors = {
        let source = String::from("a\r\n🙂\nz");
        let index = allocation_errors
            .run(|out| LineIndex::new(&source, out))
            .test_value()?;
        let mut buffer = DiagnosticBuffer::new(&schema, 4);
        let primary = primary.with_location(Some(SpanLocation {
            start: location_errors
                .run(|out| index.location(3, out))
                .test_value()?,
            end: location_errors
                .run(|out| index.location(7, out))
                .test_value()?,
        }));
        let label = label.with_location(Some(SpanLocation {
            start: location_errors
                .run(|out| index.location(9, out))
                .test_value()?,
            end: location_errors
                .run(|out| index.location(9, out))
                .test_value()?,
        }));
        let mut diagnostic = DiagnosticFactory::make_diagnostic(
            DiagnosticCode::CONFIG_VALIDATION_ERROR,
            "validation failed",
            Some(primary),
            None,
            None,
        );
        let mut label_errors = TestSlot::<Diagnostic>::default();
        label_errors
            .run(|out| {
                DiagnosticFactory::add_label_message(
                    &mut diagnostic,
                    label,
                    DiagnosticMessage::custom(""),
                    out,
                )
            })
            .test_value()?;
        test_assert!(buffer.push(diagnostic))?;
        test_assert!(location_errors.run(|out| index.location(4, out)).is_err())?;
        ConfigParseErrors::from_buffer(buffer, &schema, source_id, None)
    };
    test_assert!(
        errors
            .span_location(test_source_span(source_id, 0, 0, &mut span_errors).source_span())
            .is_none()
    )?;
    let location = errors.span_location(primary.source_span()).test_value()?;
    test_eq!((location.start.line(), location.start.column()), (2, 1))?;
    test_eq!((location.end.line(), location.end.column()), (2, 2))?;
    let eof = errors.span_location(label.source_span()).test_value()?;
    test_eq!((eof.start.line(), eof.start.column()), (3, 2))?;
    test_assert!(
        errors
            .span_location(
                test_source_span(SourceId::new(99), 0, 0, &mut span_errors).source_span(),
            )
            .is_none()
    )?;
    test_assert!(
        errors
            .span_location(test_source_span(source_id, 0, 99, &mut span_errors).source_span())
            .is_none()
    )?;

    Ok(())
}

#[test]
fn cache_limits_and_fatal_provider_errors_stop_before_later_references() -> TestResult<()> {
    let schema = ParserTestSupport::compile(
        r#"entry struct Root { first: u8 @env("X"), second: u8 @env("Y"), }"#,
    )?;
    let mut budget_errors = ErrSlot::new();
    let mut session_errors = TestSlot::default();
    let mut parse_errors = TestSlot::default();
    for (result, code) in [
        (Err(EnvironmentError::ResourceLimitExceeded), 1),
        (Err(EnvironmentError::AllocationFailed), 2),
        (Ok(Some(&b"oversized"[..])), 1),
    ] {
        let mut limits = ConfigLimits::default();
        limits = limits.with_max_string_bytes(5);
        let mut provider = Provider::new(result);
        let options = session_options(&limits);
        let session = session_errors
            .run(|out| ConfigSession::new(&mut provider, &options, &mut budget_errors, out))
            .test_value()?;
        let mut parser_input_2 =
            SliceSource::new(("{ first: env(\"X\"), second: env(\"Y\") }").as_bytes());
        let parser = parse_errors
            .run(|out| Parser::new(&schema, &mut parser_input_2, &limits, session, out))
            .test_value()?;
        let errors = parse_errors.run(|out| parser.parse(out)).test_error()?;
        test_eq!(ParserTestSupport::codes(&errors), [code])?;
        test_eq!(provider.calls(), 1)?;
    }

    Ok(())
}

#[test]
fn environment_optional_children_have_stable_locations_without_double_counting() -> TestResult<()> {
    let schema =
        ParserTestSupport::compile(r#"entry struct Root { value: option<u8> @env("X") }"#)?;
    let mut limits = ConfigLimits::default();
    limits = limits.with_max_value_nodes(3);
    let mut provider = Provider::new(Ok(Some(b"7")));
    let (root, locations) = ParserTestSupport::parse_local_value(
        &schema,
        r#"{value: env("X")}"#,
        &mut provider,
        &limits,
    )
    .test_value()?;
    let mut path_errors = TestSlot::default();
    let paths: Vec<_> = locations
        .nodes()
        .map(|(id, _)| {
            let path = path_errors
                .run(|out| locations.path(id, &root, out))
                .test_value()?;
            Ok(path_text(path.segments()))
        })
        .collect::<TestResult<Vec<_>>>()?;
    test_eq!(paths, ["", ".value", ".value"])?;
    test_eq!(provider.calls(), 1)?;

    Ok(())
}

#[test]
fn missing_config_snapshots_remain_queryable_without_fabricated_positions() -> TestResult<()> {
    let schema = ParserTestSupport::compile("entry struct Root {}")?;
    let source_id = schema.next_source_id().test_value()?;
    let mut span_errors = TestSlot::default();
    for span in [
        test_source_span(source_id, 1, 2, &mut span_errors),
        test_source_span(source_id, 0, 99, &mut span_errors),
        test_source_span(SourceId::new(99), 0, 0, &mut span_errors),
    ] {
        let mut buffer = DiagnosticBuffer::new(&schema, 4);
        test_assert!(buffer.push(DiagnosticFactory::make_diagnostic(
            DiagnosticCode::CONFIG_VALIDATION_ERROR,
            "validation failed",
            Some(span),
            None,
            None
        )))?;
        let errors = ConfigParseErrors::from_buffer(buffer, &schema, source_id, None);
        test_eq!(errors.diagnostics().len(), 1)?;
        test_eq!(
            TestCheck::item(errors.diagnostics(), 0)?.code(),
            DiagnosticCode::CONFIG_VALIDATION_ERROR
        )?;
        test_assert!(errors.span_location(span.source_span()).is_none())?;
    }

    Ok(())
}

#[test]
fn reporter_hides_secret_paths_before_storage_and_rejects_unknown_dynamic_keys() -> TestResult<()> {
    let schema =
        ParserTestSupport::compile("entry struct Root { secret: map<string, u8> @secret }")?;
    for name in ["secret", "unknown"] {
        let mut path = InstancePath::unmetered();
        let mut path_errors = TestSlot::<BudgetError>::default();
        path_errors
            .run(|out| path.push_field(name, out))
            .test_value()?;
        path_errors
            .run(|out| path.push_map_key("PRIVATE-KEY", out))
            .test_value()?;
        let mut buffer = DiagnosticBuffer::new(&schema, 4);
        let keep_going = buffer.push(DiagnosticFactory::make_diagnostic(
            DiagnosticCode::CONFIG_VALIDATION_ERROR,
            "PRIVATE-MESSAGE",
            None,
            Some(path),
            None,
        ));
        let diagnostic = buffer.iter().next().test_value()?;
        test_assert!(!format!("{diagnostic:?}").contains("PRIVATE-"))?;
        if name == "secret" {
            test_assert!(keep_going)?;
            test_eq!(diagnostic.message(), "value is hidden")?;
            test_eq!(diagnostic.path().test_value()?.len(), 1)?;
        } else {
            test_assert!(!keep_going)?;
            test_assert!(buffer.stopped())?;
            test_eq!(diagnostic.code(), DiagnosticCode::INTERNAL_ERROR)?;
            test_assert!(diagnostic.path().is_none())?;
        }
    }

    Ok(())
}

#[test]
fn finish_does_not_repeat_secret_path_redaction() -> TestResult<()> {
    let schema =
        ParserTestSupport::compile("entry struct Root { secret: map<string, u8> @secret }")?;
    let build_errors = |max_steps| -> TestResult<(DiagnosticCode, usize)> {
        let mut budget_errors = TestSlot::default();
        let budget = budget_errors
            .run(|out| ExecutionBudget::new(ExecutionLimits::new(max_steps, 1_048_576), out))
            .test_value()?;
        let mut path = budget_errors
            .run(|out| InstancePath::new(Some(&budget), out))
            .test_value()?;
        budget_errors
            .run(|out| path.push_field("secret", out))
            .test_value()?;
        budget_errors
            .run(|out| path.push_map_key("PRIVATE-KEY", out))
            .test_value()?;
        let mut buffer = DiagnosticBuffer::new(&schema, 4);
        test_assert!(buffer.push(DiagnosticFactory::make_diagnostic(
            DiagnosticCode::CONFIG_VALIDATION_ERROR,
            "PRIVATE-MESSAGE",
            None,
            Some(path),
            None,
        )))?;
        let steps_after_push = budget.steps();
        let source_id = schema.next_source_id().test_value()?;
        let errors = ConfigParseErrors::from_buffer(buffer, &schema, source_id, None);
        let diagnostic = TestCheck::item(errors.diagnostics(), 0)?;
        test_assert!(!format!("{diagnostic:?}").contains("PRIVATE-"))?;
        test_eq!(diagnostic.message(), "value is hidden")?;
        test_eq!(diagnostic.path().test_value()?.len(), 1)?;
        Ok((diagnostic.code(), steps_after_push))
    };

    let (_, steps_after_push) = build_errors(1_000_000)?;
    let (code, _) = build_errors(steps_after_push + 1)?;
    test_eq!(code, DiagnosticCode::CONFIG_VALIDATION_ERROR)?;

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
            ExcerptLimits::default(),
            RegexOptions::default(),
            *limits,
            ExecutionLimits::new(1_000_000_000, 268_435_456),
        ),
    )
}

fn session_options(limits: &ConfigLimits) -> ConfigParseOptions {
    ConfigParseOptions::new(
        ExcerptLimits::default(),
        RegexOptions::default(),
        *limits,
        ExecutionLimits::new(1_000_000_000, 268_435_456),
    )
}
