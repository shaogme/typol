mod host_error;
mod source_excerpt;

use std::{error::Error, fmt};

use opeo::{ErrSlot, OResult, Out};
use typol_core::{
    Diagnostic, DiagnosticFailure, DiagnosticFieldStatus, RelatedLocationRole, ValueSource,
};
use typol_i18n::{Translator, t};
use typol_report::{
    DiagnosticReport as StructuredDiagnosticReport, DiagnosticReportField,
    DiagnosticReportFieldName,
};
use u18n::RenderError;

use self::{
    host_error::HostErrorRenderer,
    source_excerpt::{SourceExcerptAnnotation, SourceExcerptRenderer, SourceExcerptRequest},
};

use super::{
    DiagnosticContext, DisplayError, DisplayFailurePoint, RenderOptions,
    renderer::{FormatOutput, FormatterOutput, RenderBuffer},
};
use crate::HostError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticRenderError {
    Display(DisplayError),
    Diagnostic(DiagnosticFailure),
    Translation(RenderError),
}

impl fmt::Display for DiagnosticRenderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Display(error) => fmt::Display::fmt(error, formatter),
            Self::Diagnostic(error) => fmt::Display::fmt(error, formatter),
            Self::Translation(error) => fmt::Display::fmt(error, formatter),
        }
    }
}

impl Error for DiagnosticRenderError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Display(error) => Some(error),
            Self::Diagnostic(error) => Some(error),
            Self::Translation(error) => Some(error),
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

pub(super) struct MessageTranslator {
    translator: Translator,
    failure: Option<DiagnosticRenderError>,
}

impl MessageTranslator {
    fn new(translator: Translator) -> Self {
        Self {
            translator,
            failure: None,
        }
    }

    pub(super) const fn translator(&self) -> Translator {
        self.translator
    }

    pub(super) fn write_translated<'slot>(
        &mut self,
        text: Result<&'static str, RenderError>,
        output: &mut impl FormatOutput,
        mut out: Out<'slot, '_, DisplayError>,
    ) -> OResult<'slot, ()> {
        let text = match text {
            Ok(text) => text,
            Err(error) => {
                self.failure = Some(DiagnosticRenderError::Translation(error));
                return Err(out.fail(DisplayError::internal(
                    DisplayFailurePoint::TranslationFailure,
                )));
            }
        };
        output.escape(text, false, out.reborrow())
    }

    fn write_diagnostic<'slot>(
        &mut self,
        text: Result<&str, DiagnosticFailure>,
        output: &mut impl FormatOutput,
        mut out: Out<'slot, '_, DisplayError>,
    ) -> OResult<'slot, ()> {
        let text = match text {
            Ok(text) => text,
            Err(error) => {
                self.failure = Some(DiagnosticRenderError::Diagnostic(error));
                return Err(out.fail(DisplayError::internal(
                    DisplayFailurePoint::TranslationFailure,
                )));
            }
        };
        output.escape(text, false, out.reborrow())
    }
}

struct ReportTitleRequest {
    title: DiagnosticReportTitle,
    stage: Option<DiagnosticReportStage>,
    diagnostic_count: usize,
}

#[derive(Clone, Copy)]
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
            HostErrorRenderer::write_localized(error, output, localizer, out)
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

    pub fn write_report<'context, 'diagnostic, 'slot, I, W>(
        report: DiagnosticReport<'context, 'diagnostic, I>,
        writer: &mut W,
        out: Out<'slot, '_, DiagnosticRenderError>,
    ) -> OResult<'slot, ()>
    where
        I: Iterator<Item = &'diagnostic Diagnostic>,
        W: fmt::Write + ?Sized,
    {
        Self::write_with(
            report.options,
            report.translator,
            writer,
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
                    out,
                )
            },
        )
        .map_err(|error| out.fail(error))
    }

    pub fn write_host<'slot, W: fmt::Write + ?Sized>(
        error: &HostError,
        options: RenderOptions,
        translator: Translator,
        writer: &mut W,
        out: Out<'slot, '_, DiagnosticRenderError>,
    ) -> OResult<'slot, ()> {
        Self::write_with(options, translator, writer, |output, localizer, out| {
            HostErrorRenderer::write_localized(error, output, localizer, out)
        })
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
                Some(error) => Err(error),
                None => Err(DiagnosticRenderError::Display(caught.take())),
            },
        }
    }

    fn write_with<W: fmt::Write + ?Sized>(
        options: RenderOptions,
        translator: Translator,
        writer: &mut W,
        write: impl for<'slot, 'borrow> FnOnce(
            &mut FormatterOutput<'_, W>,
            &mut MessageTranslator,
            Out<'slot, 'borrow, DisplayError>,
        ) -> OResult<'slot, ()>,
    ) -> Result<(), DiagnosticRenderError> {
        let mut display_errors = ErrSlot::<DisplayError>::new();
        let mut translations = MessageTranslator::new(translator);
        match display_errors.try_call(|mut out| {
            let mut output = FormatterOutput::new(writer, options, out.reborrow())?;
            write(&mut output, &mut translations, out.reborrow())
        }) {
            Ok(()) => Ok(()),
            Err(caught) => match translations.failure {
                Some(error) => Err(error),
                None => Err(DiagnosticRenderError::Display(caught.take())),
            },
        }
    }
}

fn write_translated<'slot>(
    text: Result<&'static str, RenderError>,
    localizer: &mut MessageTranslator,
    output: &mut impl FormatOutput,
    mut out: Out<'slot, '_, DisplayError>,
) -> OResult<'slot, ()> {
    let text = match text {
        Ok(text) => text,
        Err(error) => {
            localizer.failure = Some(DiagnosticRenderError::Translation(error));
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
            t! { name: "report.title.operation_failed", locale: localizer.translator }
        }
        DiagnosticReportTitle::SchemaCompilationFailed => {
            t! { name: "report.title.schema_compilation_failed", locale: localizer.translator }
        }
        DiagnosticReportTitle::PayloadStorageFailed => {
            t! { name: "report.title.payload_storage_failed", locale: localizer.translator }
        }
    };
    write_translated(text, localizer, output, out.reborrow())?;
    output.append(" (", out.reborrow())?;
    if let Some(stage) = request.stage {
        let text = match stage {
            DiagnosticReportStage::SchemaRead => {
                t! { name: "report.stage.schema_read", locale: localizer.translator }
            }
            DiagnosticReportStage::SchemaParsing => {
                t! { name: "report.stage.schema_parsing", locale: localizer.translator }
            }
            DiagnosticReportStage::SchemaCompilation => {
                t! { name: "report.stage.schema_compilation", locale: localizer.translator }
            }
            DiagnosticReportStage::SourceNameRegistration => {
                t! { name: "report.stage.source_name_registration", locale: localizer.translator }
            }
            DiagnosticReportStage::TypeBinding => {
                t! { name: "report.stage.type_binding", locale: localizer.translator }
            }
            DiagnosticReportStage::ConfigurationReading => {
                t! { name: "report.stage.configuration_reading", locale: localizer.translator }
            }
            DiagnosticReportStage::Decoding => {
                t! { name: "report.stage.decoding", locale: localizer.translator }
            }
            DiagnosticReportStage::Encoding => {
                t! { name: "report.stage.encoding", locale: localizer.translator }
            }
        };
        write_translated(text, localizer, output, out.reborrow())?;
        output.append(", ", out.reborrow())?;
    }
    output.number(request.diagnostic_count, out.reborrow())?;
    output.append(" ", out.reborrow())?;
    let text = if request.diagnostic_count == 1 {
        t! { name: "report.diagnostic", locale: localizer.translator }
    } else {
        t! { name: "report.diagnostics", locale: localizer.translator }
    };
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
        StructuredDiagnosticReport::visit_diagnostic(diagnostic, |field| {
            write_report_field_localized(field, &context, output, localizer, out.reborrow())
        })?;
    }
    Ok(())
}

fn write_report_field_localized<'slot>(
    field: DiagnosticReportField<'_>,
    context: &DiagnosticContext<'_>,
    output: &mut impl FormatOutput,
    localizer: &mut MessageTranslator,
    mut out: Out<'slot, '_, DisplayError>,
) -> OResult<'slot, ()> {
    match field {
        DiagnosticReportField::Code(code) => {
            output.append(code.name(), out.reborrow())?;
            output.append(" (", out.reborrow())?;
            output.number(code.get() as usize, out.reborrow())?;
            output.append("): ", out.reborrow())
        }
        DiagnosticReportField::Reason { text, .. } => {
            localizer.write_diagnostic(text, output, out.reborrow())?;
            output.append("\n", out.reborrow())
        }
        DiagnosticReportField::Kind(id) => {
            let label = t! { name: "report.kind", locale: localizer.translator };
            write_translated(label, localizer, output, out.reborrow())?;
            output.append(": ", out.reborrow())?;
            output.escape(id, false, out.reborrow())?;
            output.append("\n", out.reborrow())
        }
        DiagnosticReportField::Stage(stage) => {
            let label = t! { name: "report.diagnostic_stage", locale: localizer.translator };
            write_translated(label, localizer, output, out.reborrow())?;
            output.append(": ", out.reborrow())?;
            output.escape(stage, false, out.reborrow())
        }
        DiagnosticReportField::Path(path) => {
            output.append("\n", out.reborrow())?;
            let label = t! { name: "report.path", locale: localizer.translator };
            write_translated(label, localizer, output, out.reborrow())?;
            output.append(": ", out.reborrow())?;
            output.path(path, out.reborrow())
        }
        DiagnosticReportField::Primary {
            span,
            location_status,
        } => {
            output.append("\n", out.reborrow())?;
            let label = t! { name: "report.location", locale: localizer.translator };
            write_translated(label, localizer, output, out.reborrow())?;
            output.append(": ", out.reborrow())?;
            if matches!(
                location_status,
                DiagnosticFieldStatus::Unavailable | DiagnosticFieldStatus::Redacted
            ) {
                write_field_status_localized(location_status, output, localizer, out.reborrow())?;
            } else {
                SourceExcerptRenderer::write_location(
                    span,
                    context,
                    output,
                    localizer,
                    out.reborrow(),
                )?;
            }
            SourceExcerptRenderer::write_source_excerpt(
                SourceExcerptRequest {
                    span,
                    annotation: None,
                    context,
                    localizer,
                },
                output,
                out.reborrow(),
            )
        }
        DiagnosticReportField::Related { role, span, text } => {
            output.step(1, out.reborrow())?;
            output.append("\n", out.reborrow())?;
            let label = t! { name: "report.related_location", locale: localizer.translator };
            write_translated(label, localizer, output, out.reborrow())?;
            output.append(" (", out.reborrow())?;
            write_related_role_localized(role, output, localizer, out.reborrow())?;
            output.append("): ", out.reborrow())?;
            SourceExcerptRenderer::write_location(
                span,
                context,
                output,
                localizer,
                out.reborrow(),
            )?;
            output.append(": ", out.reborrow())?;
            output.escape(text, false, out.reborrow())?;
            SourceExcerptRenderer::write_source_excerpt(
                SourceExcerptRequest {
                    span,
                    annotation: Some(SourceExcerptAnnotation { text }),
                    context,
                    localizer,
                },
                output,
                out.reborrow(),
            )
        }
        DiagnosticReportField::RelatedStatus { status, truncated } => {
            let description = match (status, truncated) {
                (DiagnosticFieldStatus::Unavailable, _) => Some(t! {
                    name: "report.related_locations_unavailable",
                    locale: localizer.translator
                }),
                (DiagnosticFieldStatus::Redacted, _) => {
                    Some(t! { name: "report.redacted", locale: localizer.translator })
                }
                (DiagnosticFieldStatus::Available, true) => Some(t! {
                    name: "report.related_locations_truncated",
                    locale: localizer.translator
                }),
                (DiagnosticFieldStatus::NotApplicable, _)
                | (DiagnosticFieldStatus::Available, false) => None,
            };
            if let Some(description) = description {
                output.append("\n", out.reborrow())?;
                let label = t! { name: "report.related_locations", locale: localizer.translator };
                localizer.write_translated(label, output, out.reborrow())?;
                output.append(": ", out.reborrow())?;
                localizer.write_translated(description, output, out.reborrow())?;
            }
            Ok(())
        }
        DiagnosticReportField::ValueSource(source) => {
            output.append("\n", out.reborrow())?;
            let label = t! { name: "report.source", locale: localizer.translator };
            write_translated(label, localizer, output, out.reborrow())?;
            output.append(": ", out.reborrow())?;
            let value = match source {
                ValueSource::ConfigLiteral => {
                    t! { name: "report.source.config_literal", locale: localizer.translator }
                }
                ValueSource::SchemaDefault => {
                    t! { name: "report.source.schema_default", locale: localizer.translator }
                }
                ValueSource::EnvironmentVariable => {
                    t! { name: "report.source.environment_variable", locale: localizer.translator }
                }
                ValueSource::ExplicitFallback => {
                    t! { name: "report.source.explicit_fallback", locale: localizer.translator }
                }
                ValueSource::RustValue => {
                    t! { name: "report.source.rust_value", locale: localizer.translator }
                }
            };
            write_translated(value, localizer, output, out.reborrow())
        }
        DiagnosticReportField::Status { field, status } => {
            output.append("\n", out.reborrow())?;
            let label = match field {
                DiagnosticReportFieldName::Path => {
                    t! { name: "report.path", locale: localizer.translator }
                }
                DiagnosticReportFieldName::Primary => {
                    t! { name: "report.location", locale: localizer.translator }
                }
                DiagnosticReportFieldName::ValueSource => {
                    t! { name: "report.source", locale: localizer.translator }
                }
            };
            write_translated(label, localizer, output, out.reborrow())?;
            output.append(": ", out.reborrow())?;
            write_field_status_localized(status, output, localizer, out.reborrow())
        }
        DiagnosticReportField::Context { .. } => Ok(()),
    }
}

fn write_related_role_localized<'slot>(
    role: RelatedLocationRole,
    output: &mut impl FormatOutput,
    localizer: &mut MessageTranslator,
    mut out: Out<'slot, '_, DisplayError>,
) -> OResult<'slot, ()> {
    let text = match role {
        RelatedLocationRole::PreviousDeclaration => {
            t! { name: "report.related_role.previous_declaration", locale: localizer.translator }
        }
        RelatedLocationRole::FirstDeclaration => {
            t! { name: "report.related_role.first_declaration", locale: localizer.translator }
        }
        RelatedLocationRole::TypeOrFieldDefinition => {
            t! { name: "report.related_role.type_or_field_definition", locale: localizer.translator }
        }
        RelatedLocationRole::ConstraintDefinition => {
            t! { name: "report.related_role.constraint_definition", locale: localizer.translator }
        }
        RelatedLocationRole::AssociatedInput => {
            t! { name: "report.related_role.associated_input", locale: localizer.translator }
        }
    };
    write_translated(text, localizer, output, out.reborrow())
}

fn write_field_status_localized<'slot>(
    status: DiagnosticFieldStatus,
    output: &mut impl FormatOutput,
    localizer: &mut MessageTranslator,
    mut out: Out<'slot, '_, DisplayError>,
) -> OResult<'slot, ()> {
    let text = match status {
        DiagnosticFieldStatus::Unavailable | DiagnosticFieldStatus::Available => {
            t! { name: "report.field_unavailable", locale: localizer.translator }
        }
        DiagnosticFieldStatus::Redacted => {
            t! { name: "report.redacted", locale: localizer.translator }
        }
        DiagnosticFieldStatus::NotApplicable => return Ok(()),
    };
    write_translated(text, localizer, output, out.reborrow())
}
