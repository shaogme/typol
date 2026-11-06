use std::{error::Error, fmt};

use opeo::{ErrSlot, OResult, Out};
use typol_core::{
    Diagnostic, DiagnosticMessageKey, DiagnosticTranslationError, ExcerptStatus, SourceSpan,
    ValueSource,
};
use typol_i18n::{Translator, t};
use typol_report::DiagnosticReport as StructuredDiagnosticReport;

use super::{
    DiagnosticContext, DisplayError, DisplayFailurePoint, RenderOptions,
    renderer::{FormatOutput, RenderBuffer},
};
use crate::{HostError, HostErrorKind, HostOperation};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticRenderError {
    Display(DisplayError),
    Translation(DiagnosticTranslationError),
}

impl fmt::Display for DiagnosticRenderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Display(error) => fmt::Display::fmt(error, formatter),
            Self::Translation(_) => formatter.write_str("translation failed"),
        }
    }
}

impl Error for DiagnosticRenderError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Display(error) => Some(error),
            Self::Translation(_) => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticReportTitle {
    OperationFailed,
    SchemaCompilationFailed,
    PayloadStorageFailed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticReportStage {
    SchemaRead,
    SchemaParsing,
    SchemaCompilation,
    SourceNameRegistration,
    TypeBinding,
    ConfigurationReading,
    Decoding,
    Encoding,
}

/// Inputs for rendering a localized report over a diagnostic iterator.
pub struct DiagnosticReport<'context, 'diagnostic, I>
where
    I: Iterator<Item = &'diagnostic Diagnostic>,
{
    pub title: DiagnosticReportTitle,
    pub stage: Option<DiagnosticReportStage>,
    pub diagnostics: I,
    pub diagnostic_count: usize,
    pub context: DiagnosticContext<'context>,
    pub options: RenderOptions,
    pub translator: Translator,
}

struct MessageTranslator {
    translator: Translator,
    failure: Option<DiagnosticTranslationError>,
}

impl MessageTranslator {
    fn new(translator: Translator) -> Self {
        Self {
            translator,
            failure: None,
        }
    }

    fn write_diagnostic_text<'slot>(
        &mut self,
        key: Option<DiagnosticMessageKey>,
        original: &str,
        output: &mut impl FormatOutput,
        mut out: Out<'slot, '_, DisplayError>,
    ) -> OResult<'slot, ()> {
        match key {
            Some(key) => write_translated(
                self.translator.translate_diagnostic(key),
                self,
                output,
                out.reborrow(),
            ),
            None => output.escape(original, false, out.reborrow()),
        }
    }
}

struct ReportTitleRequest {
    title: DiagnosticReportTitle,
    stage: Option<DiagnosticReportStage>,
    diagnostic_count: usize,
}

#[derive(Clone, Copy)]
struct SourceExcerptAnnotation<'a> {
    key: Option<DiagnosticMessageKey>,
    text: &'a str,
}

struct SourceExcerptRequest<'a, 'context> {
    span: SourceSpan,
    annotation: Option<SourceExcerptAnnotation<'a>>,
    context: &'a DiagnosticContext<'context>,
    localizer: &'a mut MessageTranslator,
}

pub struct DiagnosticRenderer;

impl DiagnosticRenderer {
    pub fn render<'slot>(
        diagnostics: &[Diagnostic],
        context: DiagnosticContext<'_>,
        options: RenderOptions,
        translator: Translator,
        out: Out<'slot, '_, DiagnosticRenderError>,
    ) -> OResult<'slot, String> {
        Self::render_with(options, translator, |output, localizer, out| {
            write_diagnostics_localized(diagnostics.iter(), context, output, localizer, out)
        })
        .map_err(|error| out.fail(error))
    }

    pub fn render_host<'slot>(
        error: &HostError,
        options: RenderOptions,
        translator: Translator,
        out: Out<'slot, '_, DiagnosticRenderError>,
    ) -> OResult<'slot, String> {
        Self::render_with(options, translator, |output, localizer, out| {
            write_host_error_localized(error, output, localizer, out)
        })
        .map_err(|error| out.fail(error))
    }

    pub fn render_report<'context, 'diagnostic, 'slot, I>(
        report: DiagnosticReport<'context, 'diagnostic, I>,
        out: Out<'slot, '_, DiagnosticRenderError>,
    ) -> OResult<'slot, String>
    where
        I: Iterator<Item = &'diagnostic Diagnostic>,
    {
        Self::render_with(
            report.options,
            report.translator,
            |output, localizer, mut out| {
                write_report_title_localized(
                    ReportTitleRequest {
                        title: report.title,
                        stage: report.stage,
                        diagnostic_count: report.diagnostic_count,
                    },
                    output,
                    localizer,
                    out.reborrow(),
                )?;
                output.append("\n\n", out.reborrow())?;
                write_diagnostics_localized(
                    report.diagnostics,
                    report.context,
                    output,
                    localizer,
                    out.reborrow(),
                )
            },
        )
        .map_err(|error| out.fail(error))
    }

    fn render_with(
        options: RenderOptions,
        translator: Translator,
        write: impl for<'slot, 'borrow> FnOnce(
            &mut RenderBuffer,
            &mut MessageTranslator,
            Out<'slot, 'borrow, DisplayError>,
        ) -> OResult<'slot, ()>,
    ) -> Result<String, DiagnosticRenderError> {
        let mut display_errors = ErrSlot::<DisplayError>::new();
        let mut translations = MessageTranslator::new(translator);
        match display_errors.try_call(|mut out| {
            let mut output = RenderBuffer::new(options, out.reborrow())?;
            write(&mut output, &mut translations, out.reborrow())?;
            output.finish(out.reborrow())
        }) {
            Ok(rendered) => Ok(rendered),
            Err(caught) => match translations.failure {
                Some(error) => Err(DiagnosticRenderError::Translation(error)),
                None => Err(DiagnosticRenderError::Display(caught.take())),
            },
        }
    }
}

fn write_translated<'slot>(
    text: Result<&'static str, DiagnosticTranslationError>,
    localizer: &mut MessageTranslator,
    output: &mut impl FormatOutput,
    mut out: Out<'slot, '_, DisplayError>,
) -> OResult<'slot, ()> {
    let text = match text {
        Ok(text) => text,
        Err(error) => {
            localizer.failure = Some(error);
            return Err(out.fail(DisplayError::internal(
                DisplayFailurePoint::TranslationFailure,
            )));
        }
    };
    output.escape(text, false, out.reborrow())
}

fn write_report_title_localized<'slot>(
    request: ReportTitleRequest,
    output: &mut impl FormatOutput,
    localizer: &mut MessageTranslator,
    mut out: Out<'slot, '_, DisplayError>,
) -> OResult<'slot, ()> {
    let text = match request.title {
        DiagnosticReportTitle::OperationFailed => {
            t!(
                "report.title.operation_failed",
                locale = localizer.translator
            )
        }
        DiagnosticReportTitle::SchemaCompilationFailed => t!(
            "report.title.schema_compilation_failed",
            locale = localizer.translator
        ),
        DiagnosticReportTitle::PayloadStorageFailed => {
            t!(
                "report.title.payload_storage_failed",
                locale = localizer.translator
            )
        }
    }
    .map_err(|_| DiagnosticTranslationError);
    write_translated(text, localizer, output, out.reborrow())?;
    output.append(" (", out.reborrow())?;
    if let Some(stage) = request.stage {
        let text = match stage {
            DiagnosticReportStage::SchemaRead => {
                t!("report.stage.schema_read", locale = localizer.translator)
            }
            DiagnosticReportStage::SchemaParsing => {
                t!("report.stage.schema_parsing", locale = localizer.translator)
            }
            DiagnosticReportStage::SchemaCompilation => {
                t!(
                    "report.stage.schema_compilation",
                    locale = localizer.translator
                )
            }
            DiagnosticReportStage::SourceNameRegistration => t!(
                "report.stage.source_name_registration",
                locale = localizer.translator
            ),
            DiagnosticReportStage::TypeBinding => {
                t!("report.stage.type_binding", locale = localizer.translator)
            }
            DiagnosticReportStage::ConfigurationReading => t!(
                "report.stage.configuration_reading",
                locale = localizer.translator
            ),
            DiagnosticReportStage::Decoding => {
                t!("report.stage.decoding", locale = localizer.translator)
            }
            DiagnosticReportStage::Encoding => {
                t!("report.stage.encoding", locale = localizer.translator)
            }
        }
        .map_err(|_| DiagnosticTranslationError);
        write_translated(text, localizer, output, out.reborrow())?;
        output.append(", ", out.reborrow())?;
    }
    output.number(request.diagnostic_count, out.reborrow())?;
    output.append(" ", out.reborrow())?;
    let text = if request.diagnostic_count == 1 {
        t!("report.diagnostic", locale = localizer.translator)
    } else {
        t!("report.diagnostics", locale = localizer.translator)
    }
    .map_err(|_| DiagnosticTranslationError);
    write_translated(text, localizer, output, out.reborrow())?;
    output.append(")", out.reborrow())
}

fn write_diagnostics_localized<'diagnostic, 'slot>(
    diagnostics: impl Iterator<Item = &'diagnostic Diagnostic>,
    context: DiagnosticContext<'_>,
    output: &mut impl FormatOutput,
    localizer: &mut MessageTranslator,
    mut out: Out<'slot, '_, DisplayError>,
) -> OResult<'slot, ()> {
    for (index, diagnostic) in diagnostics.enumerate() {
        output.step(1, out.reborrow())?;
        if index != 0 {
            output.append("\n\n", out.reborrow())?;
        }
        output.append(diagnostic.code().name(), out.reborrow())?;
        output.append(" (", out.reborrow())?;
        output.number(diagnostic.code().get() as usize, out.reborrow())?;
        output.append("): ", out.reborrow())?;
        let diagnostic_key = StructuredDiagnosticReport::message_key(diagnostic);
        if diagnostic.message().is_empty() && diagnostic_key.is_none() {
            let text = t!(
                "report.diagnostic_message_unavailable",
                locale = localizer.translator
            )
            .map_err(|_| DiagnosticTranslationError);
            write_translated(text, localizer, output, out.reborrow())?;
        } else {
            localizer.write_diagnostic_text(
                diagnostic_key,
                diagnostic.message(),
                output,
                out.reborrow(),
            )?;
        }
        if let Some(path) = diagnostic.path() {
            output.append("\n", out.reborrow())?;
            let text = t!("report.path", locale = localizer.translator)
                .map_err(|_| DiagnosticTranslationError);
            write_translated(text, localizer, output, out.reborrow())?;
            output.append(": ", out.reborrow())?;
            output.path(path, out.reborrow())?;
        }
        if let Some(primary) = diagnostic.primary() {
            output.append("\n", out.reborrow())?;
            let text = t!("report.location", locale = localizer.translator)
                .map_err(|_| DiagnosticTranslationError);
            write_translated(text, localizer, output, out.reborrow())?;
            output.append(": ", out.reborrow())?;
            write_location_localized(primary, &context, output, localizer, out.reborrow())?;
            write_source_excerpt_localized(
                SourceExcerptRequest {
                    span: primary,
                    annotation: None,
                    context: &context,
                    localizer,
                },
                output,
                out.reborrow(),
            )?;
        }
        for label in diagnostic.labels() {
            output.step(1, out.reborrow())?;
            output.append("\n", out.reborrow())?;
            let text = t!("report.rule", locale = localizer.translator)
                .map_err(|_| DiagnosticTranslationError);
            write_translated(text, localizer, output, out.reborrow())?;
            output.append(": ", out.reborrow())?;
            write_location_localized(label.span(), &context, output, localizer, out.reborrow())?;
            output.append(": ", out.reborrow())?;
            localizer.write_diagnostic_text(
                label.key(),
                label.message(),
                output,
                out.reborrow(),
            )?;
            write_source_excerpt_localized(
                SourceExcerptRequest {
                    span: label.span(),
                    annotation: Some(SourceExcerptAnnotation {
                        key: label.key(),
                        text: label.message(),
                    }),
                    context: &context,
                    localizer,
                },
                output,
                out.reborrow(),
            )?;
        }
        if let Some(source) = diagnostic.value_source() {
            output.append("\n", out.reborrow())?;
            let text = t!("report.source", locale = localizer.translator)
                .map_err(|_| DiagnosticTranslationError);
            write_translated(text, localizer, output, out.reborrow())?;
            output.append(": ", out.reborrow())?;
            let text = match source {
                ValueSource::ConfigLiteral => {
                    t!(
                        "report.source.config_literal",
                        locale = localizer.translator
                    )
                }
                ValueSource::SchemaDefault => {
                    t!(
                        "report.source.schema_default",
                        locale = localizer.translator
                    )
                }
                ValueSource::EnvironmentVariable => t!(
                    "report.source.environment_variable",
                    locale = localizer.translator
                ),
                ValueSource::ExplicitFallback => t!(
                    "report.source.explicit_fallback",
                    locale = localizer.translator
                ),
                ValueSource::RustValue => {
                    t!("report.source.rust_value", locale = localizer.translator)
                }
            }
            .map_err(|_| DiagnosticTranslationError);
            write_translated(text, localizer, output, out.reborrow())?;
        }
    }
    Ok(())
}

fn write_location_localized<'slot>(
    span: SourceSpan,
    context: &DiagnosticContext<'_>,
    output: &mut impl FormatOutput,
    localizer: &mut MessageTranslator,
    mut out: Out<'slot, '_, DisplayError>,
) -> OResult<'slot, ()> {
    let Some(location) = (context.span_location)(span) else {
        let text = t!("report.location_unavailable", locale = localizer.translator)
            .map_err(|_| DiagnosticTranslationError);
        return write_translated(text, localizer, output, out.reborrow());
    };
    if let Some(source_name) = (context.source_name)(span.source_id()) {
        output.escape(source_name, false, out.reborrow())?;
    } else {
        output.append("source#", out.reborrow())?;
        output.number(span.source_id().get() as usize, out.reborrow())?;
    }
    output.append(":", out.reborrow())?;
    output.number(location.start.line(), out.reborrow())?;
    output.append(":", out.reborrow())?;
    output.number(location.start.column(), out.reborrow())?;
    output.append("-", out.reborrow())?;
    output.number(location.end.line(), out.reborrow())?;
    output.append(":", out.reborrow())?;
    output.number(location.end.column(), out.reborrow())
}

fn write_source_excerpt_localized<'slot>(
    request: SourceExcerptRequest<'_, '_>,
    output: &mut impl FormatOutput,
    mut out: Out<'slot, '_, DisplayError>,
) -> OResult<'slot, ()> {
    let SourceExcerptRequest {
        span,
        annotation,
        context,
        localizer,
    } = request;
    let Some(location) = (context.span_location)(span) else {
        output.append("\n  = ", out.reborrow())?;
        let text = t!(
            "report.source_excerpt_unavailable",
            locale = localizer.translator
        )
        .map_err(|_| DiagnosticTranslationError);
        write_translated(text, localizer, output, out.reborrow())?;
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
        output.append("\n  | ", out.reborrow())?;
        output.number(record.line(), out.reborrow())?;
        output.append(" | ", out.reborrow())?;
        write_source_line(text, output, out.reborrow())?;
        if let Some((start, end)) = excerpts.columns(span, record.line()) {
            output.append("\n  |   ", out.reborrow())?;
            write_repeat(
                output,
                ' ',
                visible_column(text, start).saturating_sub(1),
                out.reborrow(),
            )?;
            write_repeat(
                output,
                '^',
                visible_column(text, end)
                    .saturating_sub(visible_column(text, start))
                    .max(1),
                out.reborrow(),
            )?;
            if let Some(annotation) = annotation {
                output.append(" ", out.reborrow())?;
                localizer.write_diagnostic_text(
                    annotation.key,
                    annotation.text,
                    output,
                    out.reborrow(),
                )?;
            }
        }
    }
    if !found || excerpts.span_status(span) == ExcerptStatus::Unavailable {
        output.append("\n  = ", out.reborrow())?;
        let text = t!(
            "report.source_excerpt_unavailable",
            locale = localizer.translator
        )
        .map_err(|_| DiagnosticTranslationError);
        write_translated(text, localizer, output, out.reborrow())?;
    }
    Ok(())
}

fn write_source_line<'slot>(
    text: &str,
    output: &mut impl FormatOutput,
    mut out: Out<'slot, '_, DisplayError>,
) -> OResult<'slot, ()> {
    for character in text.chars() {
        output.step(character.len_utf8(), out.reborrow())?;
        match character {
            '\t' => output.append("\\t", out.reborrow())?,
            '\0'..='\u{1f}'
            | '\u{7f}'..='\u{9f}'
            | '\u{2028}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}' => {
                for escaped in character.escape_unicode() {
                    let mut encoded = [0; 4];
                    output.append(escaped.encode_utf8(&mut encoded), out.reborrow())?;
                }
            }
            _ => {
                let mut encoded = [0; 4];
                output.append(character.encode_utf8(&mut encoded), out.reborrow())?;
            }
        }
    }
    Ok(())
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

fn write_repeat<'slot>(
    output: &mut impl FormatOutput,
    character: char,
    count: usize,
    mut out: Out<'slot, '_, DisplayError>,
) -> OResult<'slot, ()> {
    let mut encoded = [0; 4];
    let character = character.encode_utf8(&mut encoded);
    for _ in 0..count {
        output.append(character, out.reborrow())?;
    }
    Ok(())
}

fn write_host_error_localized<'slot>(
    error: &HostError,
    output: &mut impl FormatOutput,
    localizer: &mut MessageTranslator,
    mut out: Out<'slot, '_, DisplayError>,
) -> OResult<'slot, ()> {
    let text = t!("report.host_operation", locale = localizer.translator)
        .map_err(|_| DiagnosticTranslationError);
    write_translated(text, localizer, output, out.reborrow())?;
    output.append(" ", out.reborrow())?;
    let text = match error.operation() {
        HostOperation::SchemaRead => {
            t!(
                "report.host_operation.schema_read",
                locale = localizer.translator
            )
        }
        HostOperation::ConfigRead => {
            t!(
                "report.host_operation.config_read",
                locale = localizer.translator
            )
        }
        HostOperation::SourceNames => {
            t!(
                "report.host_operation.source_names",
                locale = localizer.translator
            )
        }
    }
    .map_err(|_| DiagnosticTranslationError);
    write_translated(text, localizer, output, out.reborrow())?;
    output.append(" ", out.reborrow())?;
    let text =
        t!("report.failed", locale = localizer.translator).map_err(|_| DiagnosticTranslationError);
    write_translated(text, localizer, output, out.reborrow())?;
    output.append(" (", out.reborrow())?;
    output.append(error.context().failure_point(), out.reborrow())?;
    if let Some(source_id) = error.source_id() {
        output.append(", ", out.reborrow())?;
        let text = t!("report.source_id", locale = localizer.translator)
            .map_err(|_| DiagnosticTranslationError);
        write_translated(text, localizer, output, out.reborrow())?;
        output.append(" ", out.reborrow())?;
        output.number(source_id.get() as usize, out.reborrow())?;
    }
    if let Some(offset) = error.offset() {
        output.append(", ", out.reborrow())?;
        let text = t!("report.offset", locale = localizer.translator)
            .map_err(|_| DiagnosticTranslationError);
        write_translated(text, localizer, output, out.reborrow())?;
        output.append(" ", out.reborrow())?;
        output.number(offset, out.reborrow())?;
    }
    output.append("): ", out.reborrow())?;
    let text = match error.kind() {
        HostErrorKind::NotFound => {
            t!("report.host_kind.not_found", locale = localizer.translator)
        }
        HostErrorKind::PermissionDenied => {
            t!(
                "report.host_kind.permission_denied",
                locale = localizer.translator
            )
        }
        HostErrorKind::NotRegularFile => {
            t!(
                "report.host_kind.not_regular_file",
                locale = localizer.translator
            )
        }
        HostErrorKind::InvalidRequest => {
            t!(
                "report.host_kind.invalid_request",
                locale = localizer.translator
            )
        }
        HostErrorKind::ReadFailure => {
            t!(
                "report.host_kind.read_failure",
                locale = localizer.translator
            )
        }
        HostErrorKind::ResourceLimitExceeded => t!(
            "report.host_kind.resource_limit_exceeded",
            locale = localizer.translator
        ),
        HostErrorKind::AllocationFailed => {
            t!(
                "report.host_kind.allocation_failed",
                locale = localizer.translator
            )
        }
        HostErrorKind::CapacityExceeded => {
            t!(
                "report.host_kind.capacity_exceeded",
                locale = localizer.translator
            )
        }
        HostErrorKind::InternalError => {
            t!(
                "report.host_kind.internal_error",
                locale = localizer.translator
            )
        }
    }
    .map_err(|_| DiagnosticTranslationError);
    write_translated(text, localizer, output, out.reborrow())
}
