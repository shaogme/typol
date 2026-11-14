#![no_std]

extern crate alloc;

use core::fmt;
use typol_core::{
    Diagnostic, DiagnosticCode, DiagnosticContext as CoreDiagnosticContext, DiagnosticFailure,
    DiagnosticFieldStatus, DiagnosticParameter, ExcerptStatus, InstancePath, PathSegment,
    RelatedLocationRole, SourceExcerptSet, SourceId, SourceSpan, SpanLocation, ValueSource,
};
use typol_i18n::{Translator, t};

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
    translator: Translator,
}

#[derive(Clone, Copy)]
pub enum DiagnosticReportFieldName {
    Path,
    Primary,
    ValueSource,
}

pub enum DiagnosticReportField<'a> {
    Code(DiagnosticCode),
    Reason {
        text: Result<&'a str, DiagnosticFailure>,
        parameters: &'a [DiagnosticParameter],
        parameters_status: DiagnosticFieldStatus,
    },
    Kind(&'a str),
    Stage(&'a str),
    Path(&'a InstancePath),
    Primary {
        span: SourceSpan,
        location_status: DiagnosticFieldStatus,
    },
    Related {
        role: RelatedLocationRole,
        span: SourceSpan,
        text: &'a str,
    },
    RelatedStatus {
        status: DiagnosticFieldStatus,
        truncated: bool,
    },
    ValueSource(ValueSource),
    Context {
        status: DiagnosticFieldStatus,
        value: Option<CoreDiagnosticContext>,
    },
    Status {
        field: DiagnosticReportFieldName,
        status: DiagnosticFieldStatus,
    },
}

#[derive(Clone, Copy)]
enum ReportText {
    Path,
    Location,
    LocationUnavailable,
    RelatedLocation,
    RelatedLocations,
    RelatedLocationsUnavailable,
    RelatedLocationsTruncated,
    Kind,
    DiagnosticStage,
    FieldUnavailable,
    Redacted,
    Source,
    ConfigLiteral,
    SchemaDefault,
    EnvironmentVariable,
    ExplicitFallback,
    RustValue,
    SourceExcerptUnavailable,
}

impl<'a> DiagnosticReport<'a> {
    pub fn new(
        title: &'a str,
        stage: Option<&'a str>,
        diagnostics: &'a [Diagnostic],
        context: ReportContext<'a>,
        translator: Translator,
    ) -> Self {
        Self {
            title,
            stage,
            diagnostics,
            context,
            translator,
        }
    }

    /// Visits one diagnostic's fields in the canonical report order.
    pub fn visit_diagnostic<E>(
        diagnostic: &'a Diagnostic,
        mut visit: impl FnMut(DiagnosticReportField<'a>) -> Result<(), E>,
    ) -> Result<(), E> {
        visit(DiagnosticReportField::Code(diagnostic.code()))?;
        visit(DiagnosticReportField::Reason {
            text: diagnostic.message(),
            parameters: diagnostic.parameters(),
            parameters_status: diagnostic.parameters_status(),
        })?;
        visit(DiagnosticReportField::Kind(diagnostic.kind().id()))?;
        visit(DiagnosticReportField::Stage(diagnostic.kind().stage().id()))?;
        if let Some(path) = diagnostic.path() {
            visit(DiagnosticReportField::Path(path))?;
        } else if diagnostic.path_status() != DiagnosticFieldStatus::NotApplicable {
            visit(DiagnosticReportField::Status {
                field: DiagnosticReportFieldName::Path,
                status: diagnostic.path_status(),
            })?;
        }
        if let Some(span) = diagnostic.primary() {
            visit(DiagnosticReportField::Primary {
                span,
                location_status: diagnostic.primary_location_status(),
            })?;
        } else if diagnostic.primary_status() != DiagnosticFieldStatus::NotApplicable {
            visit(DiagnosticReportField::Status {
                field: DiagnosticReportFieldName::Primary,
                status: diagnostic.primary_status(),
            })?;
        }
        for related in diagnostic.related_locations() {
            visit(DiagnosticReportField::Related {
                role: related.role(),
                span: related.span(),
                text: related.message(),
            })?;
        }
        visit(DiagnosticReportField::RelatedStatus {
            status: diagnostic.related_locations_status(),
            truncated: diagnostic.related_locations_truncated(),
        })?;
        if let Some(source) = diagnostic.value_source() {
            visit(DiagnosticReportField::ValueSource(source))?;
        } else if diagnostic.value_source_status() != DiagnosticFieldStatus::NotApplicable {
            visit(DiagnosticReportField::Status {
                field: DiagnosticReportFieldName::ValueSource,
                status: diagnostic.value_source_status(),
            })?;
        }
        visit(DiagnosticReportField::Context {
            status: diagnostic.context_status(),
            value: diagnostic.context(),
        })?;
        Ok(())
    }

    /// Writes the report directly to the caller-provided writer without creating an owned intermediate buffer.
    pub fn write_to<W: fmt::Write + ?Sized>(&self, writer: &mut W) -> fmt::Result {
        let translator = self.translator;
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
}

fn write_diagnostic<W: fmt::Write + ?Sized>(
    diagnostic: &Diagnostic,
    context: &ReportContext<'_>,
    translator: Translator,
    writer: &mut W,
) -> fmt::Result {
    DiagnosticReport::visit_diagnostic(diagnostic, |field| {
        write_report_field(field, context, translator, writer)
    })
}

fn write_report_field<W: fmt::Write + ?Sized>(
    field: DiagnosticReportField<'_>,
    context: &ReportContext<'_>,
    translator: Translator,
    writer: &mut W,
) -> fmt::Result {
    match field {
        DiagnosticReportField::Code(code) => {
            write_code(writer, code)?;
            writer.write_str(": ")
        }
        DiagnosticReportField::Reason { text, .. } => {
            let text = text.map_err(|_| fmt::Error)?;
            write_escaped(writer, text)?;
            writer.write_str("\n")
        }
        DiagnosticReportField::Kind(id) => {
            write_report_text(writer, ReportText::Kind, translator)?;
            writer.write_str(": ")?;
            write_escaped(writer, id)?;
            writer.write_str("\n")
        }
        DiagnosticReportField::Stage(stage) => {
            write_report_text(writer, ReportText::DiagnosticStage, translator)?;
            writer.write_str(": ")?;
            write_escaped(writer, stage)
        }
        DiagnosticReportField::Path(path) => {
            writer.write_str("\n")?;
            write_report_text(writer, ReportText::Path, translator)?;
            writer.write_str(": ")?;
            write_path(writer, path)
        }
        DiagnosticReportField::Primary {
            span,
            location_status,
        } => {
            writer.write_str("\n")?;
            write_report_text(writer, ReportText::Location, translator)?;
            writer.write_str(": ")?;
            if matches!(
                location_status,
                DiagnosticFieldStatus::Unavailable | DiagnosticFieldStatus::Redacted
            ) {
                write_field_status(writer, location_status, translator)?;
            } else {
                write_location(writer, span, context, translator)?;
            }
            write_excerpt(writer, span, None, context, translator)
        }
        DiagnosticReportField::Related { role, span, text } => {
            writer.write_str("\n")?;
            write_report_text(writer, ReportText::RelatedLocation, translator)?;
            writer.write_str(" (")?;
            write_related_role(writer, role, translator)?;
            writer.write_str("): ")?;
            write_location(writer, span, context, translator)?;
            writer.write_str(": ")?;
            write_escaped(writer, text)?;
            write_excerpt(writer, span, Some(text), context, translator)
        }
        DiagnosticReportField::RelatedStatus { status, truncated } => {
            let description = match (status, truncated) {
                (DiagnosticFieldStatus::Unavailable, _) => {
                    Some(ReportText::RelatedLocationsUnavailable)
                }
                (DiagnosticFieldStatus::Redacted, _) => Some(ReportText::Redacted),
                (DiagnosticFieldStatus::Available, true) => {
                    Some(ReportText::RelatedLocationsTruncated)
                }
                (DiagnosticFieldStatus::NotApplicable, _)
                | (DiagnosticFieldStatus::Available, false) => None,
            };
            if let Some(description) = description {
                writer.write_str("\n")?;
                write_report_text(writer, ReportText::RelatedLocations, translator)?;
                writer.write_str(": ")?;
                write_report_text(writer, description, translator)?;
            }
            Ok(())
        }
        DiagnosticReportField::ValueSource(source) => {
            writer.write_str("\n")?;
            write_report_text(writer, ReportText::Source, translator)?;
            writer.write_str(": ")?;
            match source {
                ValueSource::ConfigLiteral => {
                    write_report_text(writer, ReportText::ConfigLiteral, translator)
                }
                ValueSource::SchemaDefault => {
                    write_report_text(writer, ReportText::SchemaDefault, translator)
                }
                ValueSource::EnvironmentVariable => {
                    write_report_text(writer, ReportText::EnvironmentVariable, translator)
                }
                ValueSource::ExplicitFallback => {
                    write_report_text(writer, ReportText::ExplicitFallback, translator)
                }
                ValueSource::RustValue => {
                    write_report_text(writer, ReportText::RustValue, translator)
                }
            }
        }
        DiagnosticReportField::Status { field, status } => {
            writer.write_str("\n")?;
            let label = match field {
                DiagnosticReportFieldName::Path => ReportText::Path,
                DiagnosticReportFieldName::Primary => ReportText::Location,
                DiagnosticReportFieldName::ValueSource => ReportText::Source,
            };
            write_report_text(writer, label, translator)?;
            writer.write_str(": ")?;
            write_field_status(writer, status, translator)
        }
        DiagnosticReportField::Context { .. } => Ok(()),
    }
}

fn write_code<W: fmt::Write + ?Sized>(writer: &mut W, code: DiagnosticCode) -> fmt::Result {
    write!(writer, "{} ({})", code.name(), code.get())
}

fn write_report_text<W: fmt::Write + ?Sized>(
    writer: &mut W,
    text: ReportText,
    translator: Translator,
) -> fmt::Result {
    let text = match text {
        ReportText::Path => t! { name: "report.path", locale: translator },
        ReportText::Location => t! { name: "report.location", locale: translator },
        ReportText::LocationUnavailable => {
            t! { name: "report.location_unavailable", locale: translator }
        }
        ReportText::RelatedLocation => {
            t! { name: "report.related_location", locale: translator }
        }
        ReportText::RelatedLocations => {
            t! { name: "report.related_locations", locale: translator }
        }
        ReportText::RelatedLocationsUnavailable => {
            t! { name: "report.related_locations_unavailable", locale: translator }
        }
        ReportText::RelatedLocationsTruncated => {
            t! { name: "report.related_locations_truncated", locale: translator }
        }
        ReportText::Kind => t! { name: "report.kind", locale: translator },
        ReportText::DiagnosticStage => {
            t! { name: "report.diagnostic_stage", locale: translator }
        }
        ReportText::FieldUnavailable => {
            t! { name: "report.field_unavailable", locale: translator }
        }
        ReportText::Redacted => t! { name: "report.redacted", locale: translator },
        ReportText::Source => t! { name: "report.source", locale: translator },
        ReportText::ConfigLiteral => {
            t! { name: "report.source.config_literal", locale: translator }
        }
        ReportText::SchemaDefault => {
            t! { name: "report.source.schema_default", locale: translator }
        }
        ReportText::EnvironmentVariable => {
            t! { name: "report.source.environment_variable", locale: translator }
        }
        ReportText::ExplicitFallback => {
            t! { name: "report.source.explicit_fallback", locale: translator }
        }
        ReportText::RustValue => {
            t! { name: "report.source.rust_value", locale: translator }
        }
        ReportText::SourceExcerptUnavailable => {
            t! { name: "report.source_excerpt_unavailable", locale: translator }
        }
    }
    .map_err(|_| fmt::Error)?;
    writer.write_str(text.as_ref())
}

fn write_related_role<W: fmt::Write + ?Sized>(
    writer: &mut W,
    role: RelatedLocationRole,
    translator: Translator,
) -> fmt::Result {
    let text = match role {
        RelatedLocationRole::PreviousDeclaration => {
            t! { name: "report.related_role.previous_declaration", locale: translator }
        }
        RelatedLocationRole::FirstDeclaration => t! {
            name: "report.related_role.first_declaration",
            locale: translator,
        },
        RelatedLocationRole::TypeOrFieldDefinition => {
            t! { name: "report.related_role.type_or_field_definition", locale: translator }
        }
        RelatedLocationRole::ConstraintDefinition => {
            t! { name: "report.related_role.constraint_definition", locale: translator }
        }
        RelatedLocationRole::AssociatedInput => t! {
            name: "report.related_role.associated_input",
            locale: translator,
        },
    }
    .map_err(|_| fmt::Error)?;
    writer.write_str(text.as_ref())
}

fn write_field_status<W: fmt::Write + ?Sized>(
    writer: &mut W,
    status: DiagnosticFieldStatus,
    translator: Translator,
) -> fmt::Result {
    let text = match status {
        DiagnosticFieldStatus::Unavailable | DiagnosticFieldStatus::Available => {
            ReportText::FieldUnavailable
        }
        DiagnosticFieldStatus::Redacted => ReportText::Redacted,
        DiagnosticFieldStatus::NotApplicable => return Ok(()),
    };
    write_report_text(writer, text, translator)
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
    annotation: Option<&str>,
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
            if let Some(annotation) = annotation {
                writer.write_str(" ")?;
                write_escaped(writer, annotation)?;
            }
        }
    }
    if !found || excerpts.span_status(span) == ExcerptStatus::Unavailable {
        writer.write_str("\n  = ")?;
        write_report_text(writer, ReportText::SourceExcerptUnavailable, translator)?;
    }
    Ok(())
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
        Diagnostic, DiagnosticCode, DiagnosticFailure, SourceExcerptSet, SourceId, SourceSpan,
    };
    use typol_i18n::{Language, Translator};
    use u18n::RenderError;

    use super::{DiagnosticReport, ReportContext};

    #[test]
    fn report_propagates_a_frozen_translation_failure() {
        let diagnostic = Diagnostic::failure(
            DiagnosticCode::CONFIG_VALUE_ERROR,
            DiagnosticFailure::Translation(RenderError::InvalidCatalog),
        );
        let source_name = |_: SourceId| None;
        let span_location = |_: SourceSpan| None;
        let excerpts = SourceExcerptSet::unavailable();
        let context = ReportContext::new(&source_name, &span_location, &excerpts);
        let report = DiagnosticReport::new(
            "",
            None,
            slice::from_ref(&diagnostic),
            context,
            Translator::new(Language::English),
        );
        let mut output = String::new();

        assert!(report.write_to(&mut output).is_err());
        assert!(!output.contains("English fallback"));
    }
}
