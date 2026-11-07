#[cfg(test)]
mod tests;

use core::cmp::Ordering;
use opeo::{ErrSlot, OResult, Out, ResultOutExt};
use typol_core::{
    BudgetError, ChargedVec, Diagnostic, DiagnosticCode, DiagnosticContext, ExcerptLimits,
    ExecutionBudget, SourceId, SourceSpan, Span, SpanLocation, diagnostic_context,
};
use typol_syntax::SourcePositionIndex;

use crate::{
    LocationFailure, LocationStatus, SchemaCompileErrors, SchemaCompileStage,
    diagnostics::{BudgetDiagnostics, EmergencyDiagnostics, ErrorDiagnostics},
    excerpts::SchemaExcerpts,
    loader::LoadedFile,
};

pub(crate) enum LocationError {
    Budget(BudgetError),
    Invalid {
        span: SourceSpan,
        failure: LocationFailure,
        context: DiagnosticContext,
    },
}

static LOCATION_FAILURE_CLASS_MISMATCH: DiagnosticContext = diagnostic_context!(
    "typol-schema",
    "location-indexing",
    "failure-class-mismatch"
);

macro_rules! location_invalid {
    ($span:expr, $failure:ident, $point:literal) => {{
        let span = $span;
        let context = schema_context!("location-indexing", $point)
            .with_object_id(span.source_id().get() as u64);
        LocationError::Invalid {
            span,
            failure: LocationFailure::$failure,
            context,
        }
    }};
}

pub(crate) struct SourceLocations;

impl SourceLocations {
    pub(crate) fn errors_for_sources(
        diagnostics: ErrorDiagnostics,
        files: &[LoadedFile],
        execution: &ExecutionBudget,
        limits: ExcerptLimits,
    ) -> SchemaCompileErrors {
        let errors = Self::errors_without_locations(diagnostics, files, execution);
        SchemaExcerpts::errors(errors, files, limits, execution)
    }

    fn errors_without_locations(
        mut diagnostics: ErrorDiagnostics,
        files: &[LoadedFile],
        execution: &ExecutionBudget,
    ) -> SchemaCompileErrors {
        let fallback = files
            .first()
            .map(|file| source_span(file.source_id(), file.source_len()))
            .unwrap_or_else(|| SourceSpan::new(SourceId::new(0), Span::empty()));
        let (code, context) = match diagnostics.last() {
            Some(diagnostic) => (diagnostic.code(), diagnostic.context()),
            None => {
                diagnostics.append_terminal(schema_internal!(fallback));
                (DiagnosticCode::INTERNAL_ERROR, None)
            }
        };
        let failure = match code {
            DiagnosticCode::RESOURCE_LIMIT_EXCEEDED => Some(LocationFailure::ResourceLimitExceeded),
            DiagnosticCode::ALLOCATION_FAILED => Some(LocationFailure::AllocationFailure),
            DiagnosticCode::CAPACITY_EXCEEDED => Some(LocationFailure::CapacityExceeded),
            DiagnosticCode::INTERNAL_ERROR => Some(location_failure_from_context(context)),
            _ => execution.failure().map(location_failure),
        };
        if let Some(failure) = failure {
            return SchemaCompileErrors::new_without_locations(
                diagnostics,
                LocationStatus::Unavailable(failure),
            );
        }
        execution.set_stage(SchemaCompileStage::Locations as usize);
        let mut location_errors = ErrSlot::<LocationError>::new();
        let collected = location_errors.try_call(|mut out| {
            let mut spans = ChargedVec::new(Some(execution), out.reborrow())?;
            for diagnostic in diagnostics.iter() {
                execution.consume(1, out.reborrow())?;
                if let Some(span) = diagnostic.primary() {
                    spans.try_push(span, out.reborrow())?;
                }
                for label in diagnostic.labels() {
                    spans.try_push(label.span(), out.reborrow())?;
                }
            }
            Self::collect(spans, files, Some(&diagnostics), out.reborrow())
        });
        match collected {
            Ok(locations) => {
                SchemaCompileErrors::new(diagnostics, locations, LocationStatus::Complete)
            }
            Err(caught) => {
                let error = caught.take();
                if let LocationError::Invalid { failure, .. } = error {
                    return SchemaCompileErrors::new_without_locations(
                        diagnostics,
                        LocationStatus::Unavailable(failure),
                    );
                }
                let diagnostic = Self::diagnostic(error, fallback);
                let reason = match diagnostic.code() {
                    DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
                    | DiagnosticCode::ALLOCATION_FAILED
                    | DiagnosticCode::CAPACITY_EXCEEDED
                    | DiagnosticCode::INTERNAL_ERROR => diagnostic_failure(&diagnostic),
                    _ => LocationFailure::InternalError,
                };
                diagnostics.append_terminal(diagnostic);
                SchemaCompileErrors::new_without_locations(
                    diagnostics,
                    LocationStatus::Unavailable(reason),
                )
            }
        }
    }

    pub(crate) fn collect_locations<'slot>(
        spans: ChargedVec<SourceSpan>,
        files: &[LoadedFile],
        out: Out<'slot, '_, LocationError>,
    ) -> OResult<'slot, ChargedVec<(SourceSpan, SpanLocation)>> {
        Self::collect(spans, files, None, out)
    }

    fn collect<'slot>(
        mut spans: ChargedVec<SourceSpan>,
        files: &[LoadedFile],
        diagnostics: Option<&ErrorDiagnostics>,
        mut out: Out<'slot, '_, LocationError>,
    ) -> OResult<'slot, ChargedVec<(SourceSpan, SpanLocation)>> {
        let execution = spans
            .budget()
            .ok_or(BudgetError::invariant(schema_context_ref!(
                "location-indexing",
                "span-budget-missing"
            )))
            .store_err_into(out.reborrow())?
            .try_clone(out.reborrow())?;
        spans.sort_by(
            |left, right, mut sort_out| {
                execution.consume(1, sort_out.reborrow())?;
                Ok(compare_spans(*left, *right))
            },
            out.reborrow(),
        )?;
        let mut locations = ChargedVec::new(Some(&execution), out.reborrow())?;
        locations.try_reserve_exact(spans.len(), out.reborrow())?;
        let mut previous = None;
        for span in spans.iter().copied() {
            execution.consume(1, out.reborrow())?;
            if previous == Some(span) {
                continue;
            }
            let location = diagnostics
                .and_then(|diagnostics| attached_location(diagnostics, span))
                .or_else(|| indexed_location(files, span));
            let Some(location) = location else {
                let failure = if source_index(files, span.source_id()).is_none() {
                    LocationFailure::SourceUnavailable
                } else {
                    LocationFailure::InvalidOffset
                };
                let error = match failure {
                    LocationFailure::SourceUnavailable => {
                        location_invalid!(span, SourceUnavailable, "source-not-found")
                    }
                    _ => location_invalid!(span, InvalidOffset, "invalid-source-offset"),
                };
                return Err(out.fail(error));
            };
            locations.try_push((span, location), out.reborrow())?;
            previous = Some(span);
        }
        Ok(locations)
    }

    pub(crate) fn diagnostic(error: LocationError, fallback: SourceSpan) -> Diagnostic {
        match error {
            LocationError::Budget(error) => BudgetDiagnostics::make(error, fallback),
            LocationError::Invalid {
                span,
                failure,
                context,
            } => {
                let context = if location_failure_from_context(Some(context)) == failure {
                    context
                } else {
                    LOCATION_FAILURE_CLASS_MISMATCH.with_object_id(span.source_id().get() as u64)
                };
                EmergencyDiagnostics::internal(span, context)
            }
        }
    }
}

fn source_index(files: &[LoadedFile], source_id: SourceId) -> Option<&SourcePositionIndex> {
    files
        .get(source_id.get() as usize)
        .filter(|file| file.source_id() == source_id)
        .and_then(LoadedFile::positions)
}

fn indexed_location(files: &[LoadedFile], span: SourceSpan) -> Option<SpanLocation> {
    let positions = source_index(files, span.source_id())?;
    if !positions.is_valid_span(span.span().start(), span.span().end()) {
        return None;
    }
    Some(SpanLocation {
        start: positions.location(span.span().start())?,
        end: positions.location(span.span().end())?,
    })
}

fn attached_location(diagnostics: &ErrorDiagnostics, span: SourceSpan) -> Option<SpanLocation> {
    for diagnostic in diagnostics.iter() {
        if let Some(located) = diagnostic.located_primary() {
            if located.source_span() == span {
                if let Some(location) = located.location() {
                    return Some(location);
                }
            }
        }
        for label in diagnostic.labels() {
            let located = label.located_span();
            if located.source_span() == span {
                if let Some(location) = located.location() {
                    return Some(location);
                }
            }
        }
    }
    None
}

fn location_failure(error: BudgetError) -> LocationFailure {
    match error {
        BudgetError::Resource
        | BudgetError::RegexSteps
        | BudgetError::RegexSearchSteps
        | BudgetError::RegexWorkspace => LocationFailure::ResourceLimitExceeded,
        BudgetError::Allocation => LocationFailure::AllocationFailure,
        BudgetError::CapacityExceeded => LocationFailure::CapacityExceeded,
        BudgetError::InvariantViolation(_) => LocationFailure::InternalError,
    }
}

fn diagnostic_failure(diagnostic: &Diagnostic) -> LocationFailure {
    match diagnostic.code() {
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED => LocationFailure::ResourceLimitExceeded,
        DiagnosticCode::ALLOCATION_FAILED => LocationFailure::AllocationFailure,
        DiagnosticCode::CAPACITY_EXCEEDED => LocationFailure::CapacityExceeded,
        DiagnosticCode::INTERNAL_ERROR => location_failure_from_context(diagnostic.context()),
        _ => LocationFailure::InternalError,
    }
}

fn location_failure_from_context(context: Option<DiagnosticContext>) -> LocationFailure {
    let Some(context) = context else {
        return LocationFailure::InternalError;
    };
    if context.stage() != "location-indexing" {
        return LocationFailure::InternalError;
    }
    let point = context.failure_point();
    if point.starts_with("invalid-source-span") || point.starts_with("invalid-initial-location") {
        LocationFailure::InvalidSpan
    } else if point.starts_with("source-not-found") {
        LocationFailure::SourceUnavailable
    } else if point.starts_with("invalid-source-offset") {
        LocationFailure::InvalidOffset
    } else if point.starts_with("endpoint-not-found") {
        LocationFailure::MissingEndpoint
    } else if point.starts_with("span-index")
        || point.starts_with("retained-span-index")
        || point.starts_with("endpoint-index")
    {
        LocationFailure::IndexInvariant
    } else {
        LocationFailure::InternalError
    }
}

impl From<BudgetError> for LocationError {
    fn from(error: BudgetError) -> Self {
        Self::Budget(error)
    }
}

fn compare_spans(left: SourceSpan, right: SourceSpan) -> Ordering {
    left.source_id()
        .get()
        .cmp(&right.source_id().get())
        .then_with(|| left.span().start().cmp(&right.span().start()))
        .then_with(|| left.span().end().cmp(&right.span().end()))
}

fn source_span(source_id: SourceId, source_len: usize) -> SourceSpan {
    SourceSpan::new(source_id, Span::from_zero_to(source_len))
}
