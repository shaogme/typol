#![no_std]

extern crate alloc;

use core::fmt;
use typol_core::{
    Diagnostic, DiagnosticCode, DiagnosticMessageKey, ExcerptStatus, InstancePath, PathSegment,
    SourceExcerptSet, SourceId, SourceSpan, SpanLocation, ValueSource,
};
use typol_i18n::{Language, Translator, diagnostic_key, t};

/// Provides diagnostic reports with locations, names, and source snapshots already owned by the error.
pub struct ReportContext<'a> {
    source_name: &'a dyn Fn(SourceId) -> Option<&'a str>,
    span_location: &'a dyn Fn(SourceSpan) -> Option<SpanLocation>,
    excerpts: &'a SourceExcerptSet,
}

impl<'a> ReportContext<'a> {
    pub fn new(
        source_name: &'a dyn Fn(SourceId) -> Option<&'a str>,
        span_location: &'a dyn Fn(SourceSpan) -> Option<SpanLocation>,
        excerpts: &'a SourceExcerptSet,
    ) -> Self {
        Self {
            source_name,
            span_location,
            excerpts,
        }
    }
}

/// A unified report view over borrowed diagnostics and their safe snapshots.
pub struct DiagnosticReport<'a> {
    title: &'a str,
    stage: Option<&'a str>,
    diagnostics: &'a [Diagnostic],
    context: ReportContext<'a>,
}

#[derive(Clone, Copy)]
enum ReportText {
    Path,
    Location,
    LocationUnavailable,
    Rule,
    Source,
    ConfigLiteral,
    SchemaDefault,
    EnvironmentVariable,
    ExplicitFallback,
    RustValue,
    DiagnosticMessageUnavailable,
    SourceExcerptUnavailable,
}

struct EscapingWriter<'writer, W: ?Sized> {
    writer: &'writer mut W,
}

impl<W: fmt::Write + ?Sized> fmt::Write for EscapingWriter<'_, W> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        write_escaped(self.writer, text)
    }
}

impl<'a> DiagnosticReport<'a> {
    pub fn new(
        title: &'a str,
        stage: Option<&'a str>,
        diagnostics: &'a [Diagnostic],
        context: ReportContext<'a>,
    ) -> Self {
        Self {
            title,
            stage,
            diagnostics,
            context,
        }
    }

    /// Writes the report directly to the caller-provided writer without creating an owned intermediate buffer.
    pub fn write_to<W: fmt::Write + ?Sized>(&self, writer: &mut W) -> fmt::Result {
        self.write_localized_to(writer, Translator::new(Language::English))
    }

    fn write_localized_to<W: fmt::Write + ?Sized>(
        &self,
        writer: &mut W,
        translator: Translator,
    ) -> fmt::Result {
        write_escaped(writer, self.title)?;
        writer.write_str(" (")?;
        if let Some(stage) = self.stage {
            write_escaped(writer, stage)?;
            writer.write_str(", ")?;
        }
        write!(writer, "{} diagnostics)", self.diagnostics.len())?;
        if self.diagnostics.is_empty() {
            return Ok(());
        }
        writer.write_str("\n\n")?;
        for (index, diagnostic) in self.diagnostics.iter().enumerate() {
            if index > 0 {
                writer.write_str("\n\n")?;
            }
            write_diagnostic(diagnostic, &self.context, translator, writer)?;
        }
        Ok(())
    }

    pub fn message_key(diagnostic: &Diagnostic) -> Option<DiagnosticMessageKey> {
        if let Some(key) = diagnostic.message_key() {
            return Some(key);
        }
        if diagnostic.message().is_empty() {
            return Self::emergency_message(diagnostic).map(|(key, _)| key);
        }
        None
    }

    fn message_text(diagnostic: &Diagnostic) -> Option<&str> {
        if !diagnostic.message().is_empty() {
            return Some(diagnostic.message());
        }
        Self::emergency_message(diagnostic).map(|(_, text)| text)
    }

    fn emergency_message(diagnostic: &Diagnostic) -> Option<(DiagnosticMessageKey, &'static str)> {
        let failure_point = diagnostic.context().map(|context| context.failure_point());
        match (diagnostic.code(), failure_point) {
            (DiagnosticCode::RESOURCE_LIMIT_EXCEEDED, Some("conversion-depth-limit")) => Some((
                diagnostic_key!("diagnostic.binding.conversion_depth_exceeded"),
                "type depth exceeds the synchronous conversion limit",
            )),
            (DiagnosticCode::TYPE_BINDING_ERROR, Some("recursive-rust-type")) => Some((
                diagnostic_key!("diagnostic.binding.recursive_rust_type"),
                "recursive Rust types are unsupported",
            )),
            (DiagnosticCode::TYPE_BINDING_ERROR, Some("array-length-unrepresentable")) => Some((
                diagnostic_key!("diagnostic.binding.array_length_unrepresentable"),
                "fixed array length cannot be mapped to the schema",
            )),
            (DiagnosticCode::TYPE_BINDING_ERROR, Some("schema-type-mismatch")) => Some((
                diagnostic_key!("diagnostic.binding.type_mismatch"),
                "Rust type description is incompatible with the schema",
            )),
            (DiagnosticCode::RUST_CONVERSION_ERROR, _) => Some((
                diagnostic_key!("diagnostic.conversion.failed"),
                "Rust value conversion failed",
            )),
            (DiagnosticCode::CONFIG_ENCODING_ERROR, _) => Some((
                diagnostic_key!("diagnostic.encoding.unrepresentable"),
                "value cannot be expressed in the configuration grammar",
            )),
            (DiagnosticCode::SCHEMA_CONTEXT_MISMATCH, _) => Some((
                diagnostic_key!("diagnostic.schema.root_context_mismatch"),
                "root schema source is bound to a different compilation context",
            )),
            (DiagnosticCode::SCHEMA_SESSION_FINISHED, _) => Some((
                diagnostic_key!("diagnostic.schema.session_finished"),
                "schema compilation session has ended and cannot continue",
            )),
            (DiagnosticCode::CAPACITY_EXCEEDED, Some("error-payload-capacity-exceeded")) => Some((
                diagnostic_key!("diagnostic.internal.error_payload_capacity_exceeded"),
                "failed to store error payload: capacity exceeded",
            )),
            (DiagnosticCode::RESOURCE_LIMIT_EXCEEDED, _) => Some((
                diagnostic_key!("diagnostic.emergency.resource_limit_exceeded"),
                "resource limit exceeded",
            )),
            (DiagnosticCode::ALLOCATION_FAILED, _) => Some((
                diagnostic_key!("diagnostic.emergency.allocation_failed"),
                "memory allocation failed",
            )),
            (DiagnosticCode::CAPACITY_EXCEEDED, _) => Some((
                diagnostic_key!("diagnostic.emergency.capacity_exceeded"),
                "workspace capacity exceeded",
            )),
            (DiagnosticCode::DIAGNOSTIC_LIMIT_REACHED, _) => Some((
                diagnostic_key!("diagnostic.emergency.diagnostic_limit_reached"),
                "diagnostic limit reached; subsequent errors were not collected",
            )),
            (DiagnosticCode::INTERNAL_ERROR, _) => Some((
                diagnostic_key!("diagnostic.emergency.internal_error"),
                "an internal error prevented the operation",
            )),
            (DiagnosticCode::SOURCE_INPUT_ERROR, _) => Some((
                diagnostic_key!("diagnostic.emergency.source_input_error"),
                "source input could not be read",
            )),
            _ => None,
        }
    }
}

fn write_diagnostic<W: fmt::Write + ?Sized>(
    diagnostic: &Diagnostic,
    context: &ReportContext<'_>,
    translator: Translator,
    writer: &mut W,
) -> fmt::Result {
    write_code(writer, diagnostic.code())?;
    writer.write_str(": ")?;
    let message_key = DiagnosticReport::message_key(diagnostic);
    let message = DiagnosticReport::message_text(diagnostic).unwrap_or("");
    write_diagnostic_text(writer, message_key, message, translator)?;
    if let Some(path) = diagnostic.path() {
        writer.write_str("\n")?;
        write_report_text(writer, ReportText::Path, translator)?;
        writer.write_str(": ")?;
        write_path(writer, path)?;
    }
    if let Some(primary) = diagnostic.primary() {
        writer.write_str("\n")?;
        write_report_text(writer, ReportText::Location, translator)?;
        writer.write_str(": ")?;
        write_location(writer, primary, context, translator)?;
        write_excerpt(writer, primary, None, context, translator)?;
    } else {
        writer.write_str("\n")?;
        write_report_text(writer, ReportText::Location, translator)?;
        writer.write_str(": ")?;
        write_report_text(writer, ReportText::LocationUnavailable, translator)?;
    }
    for label in diagnostic.labels() {
        let span = label.span();
        writer.write_str("\n")?;
        write_report_text(writer, ReportText::Rule, translator)?;
        writer.write_str(": ")?;
        write_location(writer, span, context, translator)?;
        writer.write_str(": ")?;
        write_diagnostic_text(writer, label.key(), label.message(), translator)?;
        write_excerpt(
            writer,
            span,
            Some((label.key(), label.message())),
            context,
            translator,
        )?;
    }
    if let Some(source) = diagnostic.value_source() {
        writer.write_str("\n")?;
        write_report_text(writer, ReportText::Source, translator)?;
        writer.write_str(": ")?;
        match source {
            ValueSource::ConfigLiteral => {
                write_report_text(writer, ReportText::ConfigLiteral, translator)?
            }
            ValueSource::SchemaDefault => {
                write_report_text(writer, ReportText::SchemaDefault, translator)?
            }
            ValueSource::EnvironmentVariable => {
                write_report_text(writer, ReportText::EnvironmentVariable, translator)?
            }
            ValueSource::ExplicitFallback => {
                write_report_text(writer, ReportText::ExplicitFallback, translator)?
            }
            ValueSource::RustValue => write_report_text(writer, ReportText::RustValue, translator)?,
        }
    }
    Ok(())
}

fn write_code<W: fmt::Write + ?Sized>(writer: &mut W, code: DiagnosticCode) -> fmt::Result {
    write!(writer, "{} ({})", code.name(), code.get())
}

fn write_report_text<W: fmt::Write + ?Sized>(
    writer: &mut W,
    text: ReportText,
    translator: Translator,
) -> fmt::Result {
    if translator.language() == Language::English {
        let english = match text {
            ReportText::Path => "Path",
            ReportText::Location => "Location",
            ReportText::LocationUnavailable => "location unavailable",
            ReportText::Rule => "Rule",
            ReportText::Source => "Source",
            ReportText::ConfigLiteral => "ConfigLiteral",
            ReportText::SchemaDefault => "SchemaDefault",
            ReportText::EnvironmentVariable => "EnvironmentVariable",
            ReportText::ExplicitFallback => "ExplicitFallback",
            ReportText::RustValue => "RustValue",
            ReportText::DiagnosticMessageUnavailable => "diagnostic message unavailable",
            ReportText::SourceExcerptUnavailable => "source excerpt unavailable",
        };
        writer.write_str(english)
    } else {
        let text = match text {
            ReportText::Path => t!("report.path", locale = translator),
            ReportText::Location => t!("report.location", locale = translator),
            ReportText::LocationUnavailable => {
                t!("report.location_unavailable", locale = translator)
            }
            ReportText::Rule => t!("report.rule", locale = translator),
            ReportText::Source => t!("report.source", locale = translator),
            ReportText::ConfigLiteral => {
                t!("report.source.config_literal", locale = translator)
            }
            ReportText::SchemaDefault => {
                t!("report.source.schema_default", locale = translator)
            }
            ReportText::EnvironmentVariable => {
                t!("report.source.environment_variable", locale = translator)
            }
            ReportText::ExplicitFallback => {
                t!("report.source.explicit_fallback", locale = translator)
            }
            ReportText::RustValue => t!("report.source.rust_value", locale = translator),
            ReportText::DiagnosticMessageUnavailable => {
                t!("report.diagnostic_message_unavailable", locale = translator)
            }
            ReportText::SourceExcerptUnavailable => {
                t!("report.source_excerpt_unavailable", locale = translator)
            }
        }
        .map_err(|_| fmt::Error)?;
        writer.write_str(text)
    }
}

fn write_location<W: fmt::Write + ?Sized>(
    writer: &mut W,
    span: SourceSpan,
    context: &ReportContext<'_>,
    translator: Translator,
) -> fmt::Result {
    let Some(location) = (context.span_location)(span) else {
        return write_report_text(writer, ReportText::LocationUnavailable, translator);
    };
    let source_name = (context.source_name)(span.source_id());
    if let Some(source_name) = source_name {
        write_escaped(writer, source_name)?;
    } else {
        write!(writer, "source#{}", span.source_id().get())?;
    }
    write!(
        writer,
        ":{}:{}-{}:{}",
        location.start.line(),
        location.start.column(),
        location.end.line(),
        location.end.column()
    )
}

fn write_excerpt<W: fmt::Write + ?Sized>(
    writer: &mut W,
    span: SourceSpan,
    annotation: Option<(Option<DiagnosticMessageKey>, &str)>,
    context: &ReportContext<'_>,
    translator: Translator,
) -> fmt::Result {
    let Some(location) = (context.span_location)(span) else {
        writer.write_str("\n  = ")?;
        write_report_text(writer, ReportText::SourceExcerptUnavailable, translator)?;
        return Ok(());
    };
    let excerpts = context.excerpts;
    let mut found = false;
    for record in excerpts.records().iter().filter(|record| {
        record.source_id() == span.source_id()
            && record.line() >= location.start.line()
            && record.line() <= location.end.line()
    }) {
        found = true;
        let text = record.text();
        write!(writer, "\n  | {} | ", record.line())?;
        write_safe_line(writer, text)?;
        if let Some((start, end)) = excerpts.columns(span, record.line()) {
            writer.write_str("\n  |   ")?;
            write_spaces(writer, visible_column(text, start).saturating_sub(1))?;
            write_repeat(
                writer,
                '^',
                visible_column(text, end)
                    .saturating_sub(visible_column(text, start))
                    .max(1),
            )?;
            if let Some((key, annotation)) = annotation {
                writer.write_str(" ")?;
                write_diagnostic_text(writer, key, annotation, translator)?;
            }
        }
    }
    if !found || excerpts.span_status(span) == ExcerptStatus::Unavailable {
        writer.write_str("\n  = ")?;
        write_report_text(writer, ReportText::SourceExcerptUnavailable, translator)?;
    }
    Ok(())
}

fn write_diagnostic_text<W: fmt::Write + ?Sized>(
    writer: &mut W,
    key: Option<DiagnosticMessageKey>,
    original: &str,
    translator: Translator,
) -> fmt::Result {
    match key {
        Some(key) => {
            let mut escaped = EscapingWriter { writer };
            translator
                .write_diagnostic(key, &mut escaped)
                .map_err(|_| fmt::Error)
        }
        None if original.is_empty() => {
            write_report_text(writer, ReportText::DiagnosticMessageUnavailable, translator)
        }
        None => write_escaped(writer, original),
    }
}

fn write_path<W: fmt::Write + ?Sized>(writer: &mut W, path: &InstancePath) -> fmt::Result {
    writer.write_char('$')?;
    for segment in path.segments() {
        match segment {
            PathSegment::Field(name) => {
                writer.write_char('.')?;
                if InstancePath::is_schema_identifier(name) {
                    write_escaped(writer, name)?;
                } else {
                    writer.write_char('"')?;
                    write_escaped_quoted(writer, name)?;
                    writer.write_char('"')?;
                }
            }
            PathSegment::Index(index) => write!(writer, "[{index}]")?,
            PathSegment::MapKey(key) => {
                writer.write_str("[\"")?;
                write_escaped_quoted(writer, key)?;
                writer.write_str("\"]")?;
            }
            PathSegment::EnumPayload(variant) => {
                writer.write_str("::")?;
                if InstancePath::is_schema_identifier(variant) {
                    write_escaped(writer, variant)?;
                } else {
                    writer.write_char('"')?;
                    write_escaped_quoted(writer, variant)?;
                    writer.write_char('"')?;
                }
            }
        }
    }
    Ok(())
}

fn write_escaped<W: fmt::Write + ?Sized>(writer: &mut W, text: &str) -> fmt::Result {
    for character in text.chars() {
        match character {
            '\\' => writer.write_str("\\\\")?,
            '\n' => writer.write_str("\\n")?,
            '\r' => writer.write_str("\\r")?,
            '\t' => writer.write_str("\\t")?,
            '\0'..='\u{1f}'
            | '\u{7f}'..='\u{9f}'
            | '\u{2028}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}' => write_unicode_escape(writer, character)?,
            _ => writer.write_char(character)?,
        }
    }
    Ok(())
}

fn write_escaped_quoted<W: fmt::Write + ?Sized>(writer: &mut W, text: &str) -> fmt::Result {
    for character in text.chars() {
        if character == '"' {
            writer.write_str("\\\"")?;
        } else {
            let mut buffer = [0; 4];
            write_escaped(writer, character.encode_utf8(&mut buffer))?;
        }
    }
    Ok(())
}

fn write_safe_line<W: fmt::Write + ?Sized>(writer: &mut W, text: &str) -> fmt::Result {
    for character in text.chars() {
        match character {
            '\t' => writer.write_str("\\t")?,
            '\0'..='\u{1f}'
            | '\u{7f}'..='\u{9f}'
            | '\u{2028}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}' => write_unicode_escape(writer, character)?,
            _ => writer.write_char(character)?,
        }
    }
    Ok(())
}

fn write_unicode_escape<W: fmt::Write + ?Sized>(writer: &mut W, character: char) -> fmt::Result {
    writer.write_str("\\u{")?;
    write!(writer, "{:x}", character as u32)?;
    writer.write_char('}')
}

fn visible_column(text: &str, column: usize) -> usize {
    text.chars()
        .take(column.saturating_sub(1))
        .map(|character| match character {
            '\t' => 2,
            '\0'..='\u{1f}'
            | '\u{7f}'..='\u{9f}'
            | '\u{2028}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}' => character.escape_unicode().count(),
            _ => 1,
        })
        .sum::<usize>()
        .saturating_add(1)
}

fn write_spaces<W: fmt::Write + ?Sized>(writer: &mut W, count: usize) -> fmt::Result {
    write_repeat(writer, ' ', count)
}

fn write_repeat<W: fmt::Write + ?Sized>(
    writer: &mut W,
    character: char,
    count: usize,
) -> fmt::Result {
    for _ in 0..count {
        writer.write_char(character)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use alloc::string::String;
    use core::slice;
    use typol_core::{
        BudgetError, Diagnostic, DiagnosticCode, DiagnosticMessage, DiagnosticMessageKey,
        SourceExcerptSet, SourceId, SourceSpan, opeo::ErrSlot,
    };

    use super::{DiagnosticReport, ReportContext};

    #[test]
    fn english_report_rejects_an_invalid_diagnostic_key() {
        let mut errors = ErrSlot::<BudgetError>::new();
        let result = errors.try_call(|out| {
            Diagnostic::try_new(
                DiagnosticCode::CONFIG_VALUE_ERROR,
                DiagnosticMessage::localized(
                    "English fallback",
                    DiagnosticMessageKey::__from_index(u16::MAX),
                ),
                None,
                out,
            )
        });
        assert!(result.is_ok(), "diagnostic construction should succeed");
        let Ok(diagnostic) = result else {
            return;
        };
        let source_name = |_: SourceId| None;
        let span_location = |_: SourceSpan| None;
        let excerpts = SourceExcerptSet::unavailable();
        let context = ReportContext::new(&source_name, &span_location, &excerpts);
        let report = DiagnosticReport::new("", None, slice::from_ref(&diagnostic), context);
        let mut output = String::new();

        assert!(report.write_to(&mut output).is_err());
        assert!(!output.contains("English fallback"));
    }
}
