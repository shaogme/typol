use std::io::{self, Write};

use serde_json::{Value, json, to_writer};
use typol::{
    SyntaxErrors, TypolError, TypolStage,
    core::{
        Diagnostic, DiagnosticFailure, DiagnosticFieldStatus, DiagnosticParameterValue,
        ErrorContext as CoreDiagnosticContext, ExcerptLimits, ExcerptStatus, ExecutionBudget,
        PathSegment, SourceCapture, SourceExcerptSet, SourceId, SourceLocation, SourceSpan,
        ValueSource,
    },
    host::{HostErrorKind, HostFailureStep, HostOperation},
    semantic::SpanLocation,
};

const MAX_DIAGNOSTIC_JSON_BYTES: usize = 1_048_576;
const MAX_DIAGNOSTIC_JSON_ENTRIES: usize = 16;
const MAX_DIAGNOSTIC_PATH_SEGMENTS: usize = 32;
const MAX_DIAGNOSTIC_PATH_SEGMENT_BYTES: usize = 128;

struct BoundedJsonWriter {
    bytes: Vec<u8>,
}

impl BoundedJsonWriter {
    fn new() -> Self {
        Self { bytes: Vec::new() }
    }

    fn push_newline(&mut self) -> io::Result<()> {
        self.write_all(b"\n")
    }
}

impl Write for BoundedJsonWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let Some(length) = self.bytes.len().checked_add(bytes.len()) else {
            return Err(io::Error::other("diagnostic JSON output limit exceeded"));
        };
        if length > MAX_DIAGNOSTIC_JSON_BYTES {
            return Err(io::Error::other("diagnostic JSON output limit exceeded"));
        }
        self.bytes
            .try_reserve_exact(bytes.len())
            .map_err(|_| io::Error::other("diagnostic JSON allocation failed"))?;
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(super) struct DiagnosticJson;

pub(super) struct SchemaExcerptCapture;

pub(super) struct SyntaxJsonRequest<'a> {
    pub(super) errors: &'a SyntaxErrors,
    pub(super) source: &'a str,
    pub(super) budget: &'a ExecutionBudget,
    pub(super) detailed_diagnostics: bool,
}

impl DiagnosticJson {
    pub(super) fn write_operation_failure(message: &str) -> bool {
        Self::write_json_stderr(&json!({
            "schema_version": 1,
            "error": {
            "kind": "operation_failed",
                "category": "CLI_OPERATION_ERROR",
                "stage": "host_io",
            "reason": message,
            }
        }))
    }

    pub(super) fn write_diagnostics(
        errors: &TypolError,
        detailed_diagnostics: bool,
    ) -> Result<bool, DiagnosticFailure> {
        let diagnostics = errors
            .diagnostics()
            .iter()
            .take(MAX_DIAGNOSTIC_JSON_ENTRIES)
            .map(|diagnostic| {
                Self::diagnostic_json(diagnostic, detailed_diagnostics, |span| {
                    Self::span_json(span, errors)
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let host_error = errors.host_error().map(|error| {
            json!({
                "operation": Self::host_operation_id(error.operation()),
                "category": Self::host_kind_id(error.kind()),
                "failure_point": Self::host_step_id(error.failure_step()),
                "source_id": error.source_id().map(SourceId::get),
                "offset": error.offset(),
            })
        });
        Ok(Self::write_json_stderr(&json!({
            "schema_version": 1,
            "detail_level": if detailed_diagnostics { "detailed" } else { "standard" },
            "operation_stage": Self::operation_stage_id(errors.stage()),
            "diagnostics": diagnostics,
            "diagnostics_truncated": errors.diagnostic_count() > MAX_DIAGNOSTIC_JSON_ENTRIES,
            "host_error": host_error,
        })))
    }

    pub(super) fn write_syntax_diagnostics(
        request: SyntaxJsonRequest<'_>,
    ) -> Result<bool, DiagnosticFailure> {
        let SyntaxJsonRequest {
            errors,
            source,
            budget,
            detailed_diagnostics,
        } = request;
        let excerpts = SchemaExcerptCapture::capture(errors, source, budget);
        let diagnostic_count = errors.diagnostics().len();
        let diagnostics = errors
            .diagnostics()
            .take(MAX_DIAGNOSTIC_JSON_ENTRIES)
            .map(|diagnostic| {
                Self::diagnostic_json(diagnostic, detailed_diagnostics, |span| {
                    let excerpt_status = match excerpts.span_status(span) {
                        ExcerptStatus::Complete => "complete",
                        ExcerptStatus::Truncated => "truncated",
                        ExcerptStatus::Unavailable => "unavailable",
                    };
                    json!({
                        "status": "available",
                        "source_id": span.source_id().get(),
                        "source_name": {"status": "unavailable"},
                        "byte_range": {"start": span.span().start(), "end": span.span().end()},
                        "location": Self::span_location(source, span),
                        "excerpt": {"status": excerpt_status},
                    })
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self::write_json_stderr(&json!({
            "schema_version": 1,
            "detail_level": if detailed_diagnostics { "detailed" } else { "standard" },
            "operation_stage": "schema_parsing",
            "diagnostics": diagnostics,
            "diagnostics_truncated": diagnostic_count > MAX_DIAGNOSTIC_JSON_ENTRIES,
        })))
    }

    fn diagnostic_json(
        diagnostic: &Diagnostic,
        detailed_diagnostics: bool,
        mut span_json: impl FnMut(SourceSpan) -> Value,
    ) -> Result<Value, DiagnosticFailure> {
        let code = diagnostic.code();
        let code = json!({"id": code.name(), "value": code.get()});
        let kind = diagnostic.kind();
        let reason = diagnostic.message()?;
        let parameters = diagnostic
            .parameters()
            .iter()
            .map(|parameter| match parameter.value() {
                DiagnosticParameterValue::Unsigned(value) => json!({
                    "name": parameter.name(),
                    "type": "number",
                    "value": value,
                }),
            })
            .collect::<Vec<_>>();
        let primary = match diagnostic.primary() {
            Some(span) => {
                let mut value = span_json(span);
                let status = diagnostic.primary_location_status();
                if matches!(
                    status,
                    DiagnosticFieldStatus::Unavailable | DiagnosticFieldStatus::Redacted
                ) && let Some(object) = value.as_object_mut()
                {
                    let _ = object.insert(
                        String::from("location"),
                        json!({"status": Self::field_status_id(status)}),
                    );
                }
                value
            }
            None => json!({"status": Self::field_status_id(diagnostic.primary_status())}),
        };
        let path = match diagnostic.path() {
            Some(instance_path) => {
                let segments = instance_path.segments();
                json!({
                    "status": "available",
                    "segments": segments.iter().take(MAX_DIAGNOSTIC_PATH_SEGMENTS).map(|segment| {
                        match segment {
                            PathSegment::Field(name) => Self::path_text("field", name.as_str()),
                            PathSegment::Index(index) => json!({"kind": "index", "value": index}),
                            PathSegment::MapKey(key) => Self::path_text("map_key", key.as_str()),
                            PathSegment::EnumPayload(name) => Self::path_text("enum_payload", name.as_str()),
                        }
                    }).collect::<Vec<_>>(),
                    "truncated": segments.len() > MAX_DIAGNOSTIC_PATH_SEGMENTS,
                })
            }
            None => json!({"status": Self::field_status_id(diagnostic.path_status())}),
        };
        let value_source = diagnostic.value_source().map_or_else(
            || json!({"status": Self::field_status_id(diagnostic.value_source_status())}),
            |source| json!({"status": "available", "value": Self::value_source_id(source)}),
        );
        let related_locations = diagnostic
            .related_locations()
            .iter()
            .map(|related| {
                json!({
                    "role": related.role().id(),
                    "location": span_json(related.span()),
                })
            })
            .collect::<Vec<_>>();
        let context_status = diagnostic.context_status();
        let context = diagnostic.context();
        let mut entry = json!({
            "code": code,
            "kind": kind.id(),
            "stage": kind.stage().id(),
            "reason": reason,
            "parameters": parameters,
            "parameters_status": Self::field_status_id(diagnostic.parameters_status()),
            "primary": primary,
            "related_locations": related_locations,
            "related_locations_status": Self::field_status_id(diagnostic.related_locations_status()),
            "related_locations_truncated": diagnostic.related_locations_truncated(),
            "path": path,
            "value_source": value_source,
        });
        if detailed_diagnostics {
            let context_value = match (context_status, context) {
                (DiagnosticFieldStatus::Available, Some(context)) => json!({
                    "status": "available",
                    "value": Self::context_json(context),
                }),
                (status, _) => json!({"status": Self::field_status_id(status)}),
            };
            if let Some(object) = entry.as_object_mut() {
                let _ = object.insert(String::from("context"), context_value);
            }
        }
        Ok(entry)
    }

    fn path_text(kind: &str, value: &str) -> Value {
        let (value, truncated) = bounded_text(value, MAX_DIAGNOSTIC_PATH_SEGMENT_BYTES);
        json!({
            "kind": kind,
            "value": value,
            "truncated": truncated,
        })
    }

    fn span_json(span: SourceSpan, errors: &TypolError) -> Value {
        let source_id = span.source_id();
        let source_name_status = errors.source_name_status(source_id);
        let source_name = if source_name_status == DiagnosticFieldStatus::Available {
            json!({"status": "redacted"})
        } else {
            json!({"status": Self::field_status_id(source_name_status)})
        };
        let location = errors.span_location(span).map(|location| {
            json!({
                "status": "available",
                "start": {"line": location.start.line(), "column": location.start.column()},
                "end": {"line": location.end.line(), "column": location.end.column()},
            })
        });
        let excerpt_status = match errors.excerpt_status(span) {
            ExcerptStatus::Complete => "complete",
            ExcerptStatus::Truncated => "truncated",
            ExcerptStatus::Unavailable => "unavailable",
        };
        json!({
            "status": "available",
            "source_id": source_id.get(),
            "source_name": source_name,
            "byte_range": {"start": span.span().start(), "end": span.span().end()},
            "location": location.unwrap_or_else(|| json!({"status": "unavailable"})),
            "excerpt": {"status": excerpt_status},
        })
    }

    fn span_location(source: &str, span: SourceSpan) -> Value {
        let location = |offset| source_location(source, offset);
        match (location(span.span().start()), location(span.span().end())) {
            (Some(start), Some(end)) => json!({
                "status": "available",
                "start": {"line": start.line(), "column": start.column()},
                "end": {"line": end.line(), "column": end.column()},
            }),
            _ => json!({"status": "unavailable"}),
        }
    }

    fn context_json(context: CoreDiagnosticContext) -> Value {
        let object = if let Some((start, end)) = context.object_range() {
            json!({"kind": "range", "start": start, "end": end})
        } else if let Some(index) = context.object_index() {
            json!({"kind": "index", "value": index})
        } else if let Some(id) = context.object_id() {
            json!({"kind": "id", "value": id})
        } else {
            Value::Null
        };
        json!({
            "subsystem": context.subsystem(),
            "stage": context.stage(),
            "failure_point": context.failure_point(),
            "object": object,
        })
    }

    fn field_status_id(status: DiagnosticFieldStatus) -> &'static str {
        match status {
            DiagnosticFieldStatus::NotApplicable => "not_applicable",
            DiagnosticFieldStatus::Available => "available",
            DiagnosticFieldStatus::Unavailable => "unavailable",
            DiagnosticFieldStatus::Redacted => "redacted",
        }
    }

    fn value_source_id(source: ValueSource) -> &'static str {
        match source {
            ValueSource::ConfigLiteral => "config_literal",
            ValueSource::SchemaDefault => "schema_default",
            ValueSource::EnvironmentVariable => "environment_variable",
            ValueSource::ExplicitFallback => "explicit_fallback",
            ValueSource::RustValue => "rust_value",
        }
    }

    fn operation_stage_id(stage: TypolStage) -> &'static str {
        match stage {
            TypolStage::SchemaRead => "schema_read",
            TypolStage::SchemaCompile => "schema_compilation",
            TypolStage::SourceNames => "source_name_registration",
            TypolStage::Binding => "type_binding",
            TypolStage::ConfigRead => "configuration_reading",
            TypolStage::Decoding => "decoding",
            TypolStage::Encoding => "encoding",
        }
    }

    fn host_operation_id(operation: HostOperation) -> &'static str {
        match operation {
            HostOperation::SchemaRead => "schema_read",
            HostOperation::ConfigRead => "configuration_read",
            HostOperation::SourceNames => "source_names",
        }
    }

    fn host_kind_id(kind: HostErrorKind) -> &'static str {
        match kind {
            HostErrorKind::NotFound => "not_found",
            HostErrorKind::PermissionDenied => "permission_denied",
            HostErrorKind::NotRegularFile => "not_regular_file",
            HostErrorKind::InvalidRequest => "invalid_request",
            HostErrorKind::ReadFailure => "read_failure",
            HostErrorKind::ResourceLimitExceeded => "resource_limit_exceeded",
            HostErrorKind::AllocationFailed => "allocation_failed",
            HostErrorKind::CapacityExceeded => "capacity_exceeded",
            HostErrorKind::InternalError => "internal_error",
        }
    }

    fn host_step_id(step: HostFailureStep) -> &'static str {
        match step {
            HostFailureStep::RequestValidation => "request_validation",
            HostFailureStep::BudgetAccounting => "budget_accounting",
            HostFailureStep::BudgetInvariant => "budget_invariant",
            HostFailureStep::PathProcessing => "path_processing",
            HostFailureStep::FileMetadata => "file_metadata",
            HostFailureStep::FileOpen => "file_open",
            HostFailureStep::FileRead => "file_read",
            HostFailureStep::FileBufferInvariant => "file_buffer_invariant",
            HostFailureStep::FileUtf8Validation => "file_utf8_validation",
            HostFailureStep::SourceNameOperation => "source_name_operation",
            HostFailureStep::SourceNameFormatting => "source_name_formatting",
            HostFailureStep::SourceObserverValidation => "source_observer_validation",
            HostFailureStep::SourceRead => "source_read",
            HostFailureStep::SourceResourceLimit => "source_resource_limit",
            HostFailureStep::SourceAllocation => "source_allocation",
            HostFailureStep::SourceCapacity => "source_capacity",
            HostFailureStep::SourceInvariant => "source_invariant",
        }
    }

    fn write_json_stderr(value: &Value) -> bool {
        let mut output = BoundedJsonWriter::new();
        if to_writer(&mut output, value).is_err() || output.push_newline().is_err() {
            return false;
        }
        io::stderr().lock().write_all(&output.bytes).is_ok()
    }
}

impl SchemaExcerptCapture {
    pub(super) fn capture(
        errors: &SyntaxErrors,
        source: &str,
        budget: &ExecutionBudget,
    ) -> SourceExcerptSet {
        let source_id = SourceId::new(0);
        let limits = ExcerptLimits::default();
        let mut capture = SourceCapture::new(source_id, limits, Some(budget));
        for diagnostic in errors.diagnostics() {
            for located in diagnostic
                .located_primary()
                .into_iter()
                .chain(diagnostic.labels().iter().map(|label| label.located_span()))
            {
                let span = located.source_span();
                let location = source_location(source, span.span().start())
                    .zip(source_location(source, span.span().end()))
                    .map(|(start, end)| SpanLocation { start, end });
                capture.pin(span, location);
            }
        }
        // Syntax failure cannot prove secret-default spans, so excerpts publish only fully redacted source.
        let Some(secret) = capture.begin_secret(0) else {
            return SourceExcerptSet::unavailable();
        };
        capture.feed(source.as_bytes());
        capture.end_secret(secret, source.len());
        capture.end_input();
        let mut excerpts = SourceExcerptSet::new(limits, Some(budget));
        capture.finish_iter_with_locations(&mut excerpts, errors.diagnostics(), |located| {
            let span = located.source_span().span();
            source_location(source, span.start())
                .zip(source_location(source, span.end()))
                .map(|(start, end)| SpanLocation { start, end })
        });
        excerpts
    }
}

fn source_location(source: &str, offset: usize) -> Option<SourceLocation> {
    let prefix = source.get(..offset)?;
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let line_start = prefix.rfind('\n').map_or(0, |index| index + 1);
    let column = prefix.get(line_start..)?.chars().count() + 1;
    SourceLocation::new(line, column)
}

#[cfg(test)]
#[test]
fn bounded_writer_rejects_bytes_after_its_limit() {
    let mut writer = BoundedJsonWriter::new();
    let bytes = vec![b'x'; MAX_DIAGNOSTIC_JSON_BYTES];
    assert!(writer.write_all(&bytes).is_ok());

    assert!(writer.write_all(b"x").is_err());
    assert_eq!(writer.bytes.len(), MAX_DIAGNOSTIC_JSON_BYTES);
}

#[cfg(test)]
#[test]
fn bounded_text_stops_at_a_utf8_boundary() {
    let source = "界".repeat(50);
    let (bounded, truncated) = bounded_text(&source, 128);

    assert!(truncated);
    assert!(bounded.len() <= 128);
    assert!(source.starts_with(bounded));
}

fn bounded_text(value: &str, max_bytes: usize) -> (&str, bool) {
    if value.len() <= max_bytes {
        return (value, false);
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    (&value[..end], true)
}
