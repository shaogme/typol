use crate::diagnostics::{DiagnosticList, ErrorDiagnostics};
use core::{cmp::Ordering, error::Error, fmt, slice::from_ref};
use typol_core::{
    BudgetError, ChargedVec, Diagnostic, DiagnosticCode, DiagnosticContext, SourceExcerptSet,
    SourceId, SourceSpan,
};
use typol_report::{DiagnosticReport, ReportContext};
use typol_semantic::SpanLocation;
use typol_syntax::AstAccessError;

static UNAVAILABLE_EXCERPTS: SourceExcerptSet = SourceExcerptSet::unavailable();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocationFailure {
    AllocationFailure,
    ResourceLimitExceeded,
    CapacityExceeded,
    InvalidSpan,
    SourceUnavailable,
    InvalidOffset,
    MissingEndpoint,
    IndexInvariant,
    InternalError,
}

/// Describes whether source locations for diagnostics are complete.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocationStatus {
    Complete,
    Unavailable(LocationFailure),
}

/// A structured internal failure cause encountered while compiling a Schema.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchemaInternalCause {
    AstAccess(AstAccessError),
}

impl fmt::Display for SchemaInternalCause {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AstAccess(error) => write!(formatter, "{error}"),
        }
    }
}

/// Describes a root source rejected by the context contract or session lifecycle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchemaCompileContextFailure {
    RootContextMismatch,
    SessionFinished,
}

impl fmt::Display for SchemaCompileContextFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message())
    }
}

impl SchemaCompileContextFailure {
    pub const fn message(self) -> &'static str {
        match self {
            Self::RootContextMismatch => {
                "root schema source is bound to a different compilation context"
            }
            Self::SessionFinished => "schema compilation session has ended and cannot continue",
        }
    }
}

/// Identifies which independent execution budget caused a Schema compilation failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchemaCompileBudgetDomain {
    /// Non-Regex Schema parsing, compilation, or diagnostic work.
    Execution,
    /// Regex parsing, compilation, matching, or Regex-owned workspace.
    Regex,
}

enum SchemaErrorStorage {
    Inline(Diagnostic),
    Collected {
        diagnostics: ErrorDiagnostics,
        locations: Option<ChargedVec<(SourceSpan, SpanLocation)>>,
        location_status: LocationStatus,
        excerpts: SourceExcerptSet,
    },
}

/// A non-empty owned compilation error with an allocation-free emergency form.
pub struct SchemaCompileErrors {
    storage: SchemaErrorStorage,
    context_failure: Option<SchemaCompileContextFailure>,
    budget_domain: Option<SchemaCompileBudgetDomain>,
}

impl SchemaCompileErrors {
    pub fn excerpts(&self) -> &SourceExcerptSet {
        match &self.storage {
            SchemaErrorStorage::Inline(_) => &UNAVAILABLE_EXCERPTS,
            SchemaErrorStorage::Collected { excerpts, .. } => excerpts,
        }
    }

    pub(crate) fn with_excerpts(mut self, excerpts: SourceExcerptSet) -> Self {
        if let SchemaErrorStorage::Collected {
            excerpts: stored, ..
        } = &mut self.storage
        {
            *stored = excerpts;
        }
        self
    }

    pub fn budget_failure(error: BudgetError) -> Self {
        let budget_domain = match error {
            BudgetError::RegexSteps
            | BudgetError::RegexSearchSteps
            | BudgetError::RegexWorkspace => SchemaCompileBudgetDomain::Regex,
            BudgetError::Resource
            | BudgetError::Allocation
            | BudgetError::CapacityExceeded
            | BudgetError::InvariantViolation(_) => SchemaCompileBudgetDomain::Execution,
        };
        Self::budget_failure_in_domain(error, budget_domain)
    }

    pub(crate) fn regex_budget_failure(error: BudgetError) -> Self {
        Self::budget_failure_in_domain(error, SchemaCompileBudgetDomain::Regex)
    }

    fn budget_failure_in_domain(
        error: BudgetError,
        budget_domain: SchemaCompileBudgetDomain,
    ) -> Self {
        let code = error.diagnostic_code();
        let mut diagnostic = Diagnostic::emergency(code);
        diagnostic.set_context(error.context());
        Self {
            storage: SchemaErrorStorage::Inline(diagnostic),
            context_failure: None,
            budget_domain: Some(budget_domain),
        }
    }

    pub(crate) fn inline(diagnostic: Diagnostic) -> Self {
        Self {
            storage: SchemaErrorStorage::Inline(diagnostic),
            context_failure: None,
            budget_domain: None,
        }
    }

    pub(crate) fn with_context_failure(
        diagnostic: Diagnostic,
        failure: SchemaCompileContextFailure,
    ) -> Self {
        Self {
            storage: SchemaErrorStorage::Inline(diagnostic),
            context_failure: Some(failure),
            budget_domain: None,
        }
    }

    pub(crate) fn new(
        diagnostics: ErrorDiagnostics,
        locations: ChargedVec<(SourceSpan, SpanLocation)>,
        location_status: LocationStatus,
    ) -> Self {
        Self {
            storage: SchemaErrorStorage::Collected {
                diagnostics,
                locations: Some(locations),
                location_status,
                excerpts: SourceExcerptSet::unavailable(),
            },
            context_failure: None,
            budget_domain: None,
        }
    }

    pub(crate) fn new_without_locations(
        diagnostics: ErrorDiagnostics,
        location_status: LocationStatus,
    ) -> Self {
        Self {
            storage: SchemaErrorStorage::Collected {
                diagnostics,
                locations: None,
                location_status,
                excerpts: SourceExcerptSet::unavailable(),
            },
            context_failure: None,
            budget_domain: None,
        }
    }

    /// Returns the budget domain attached to an allocation-free budget failure.
    pub const fn budget_domain(&self) -> Option<SchemaCompileBudgetDomain> {
        self.budget_domain
    }

    /// Returns a structured reason when the root context or session lifecycle is invalid.
    pub const fn context_failure(&self) -> Option<SchemaCompileContextFailure> {
        self.context_failure
    }

    pub fn location_status(&self) -> LocationStatus {
        match &self.storage {
            SchemaErrorStorage::Inline(diagnostic) => {
                if diagnostic.primary().is_none() && diagnostic.labels().is_empty() {
                    if diagnostic.code() == DiagnosticCode::INTERNAL_ERROR
                        && diagnostic.context().is_some()
                    {
                        return LocationStatus::Unavailable(location_failure_from_context(
                            diagnostic.context(),
                        ));
                    }
                    return LocationStatus::Complete;
                }
                let reason = match diagnostic.code() {
                    DiagnosticCode::ALLOCATION_FAILED => LocationFailure::AllocationFailure,
                    DiagnosticCode::RESOURCE_LIMIT_EXCEEDED => {
                        LocationFailure::ResourceLimitExceeded
                    }
                    DiagnosticCode::CAPACITY_EXCEEDED => LocationFailure::CapacityExceeded,
                    DiagnosticCode::SCHEMA_CONTEXT_MISMATCH
                    | DiagnosticCode::SCHEMA_SESSION_FINISHED => LocationFailure::SourceUnavailable,
                    DiagnosticCode::INTERNAL_ERROR => {
                        location_failure_from_context(diagnostic.context())
                    }
                    _ => LocationFailure::InternalError,
                };
                LocationStatus::Unavailable(reason)
            }
            SchemaErrorStorage::Collected {
                location_status, ..
            } => *location_status,
        }
    }

    pub fn diagnostics(&self) -> DiagnosticList<'_> {
        match &self.storage {
            SchemaErrorStorage::Inline(diagnostic) => {
                DiagnosticList::new(from_ref(diagnostic), None)
            }
            SchemaErrorStorage::Collected { diagnostics, .. } => diagnostics.list(),
        }
    }

    pub fn internal_context(&self) -> Option<DiagnosticContext> {
        self.diagnostics()
            .iter()
            .rev()
            .find(|diagnostic| diagnostic.code() == DiagnosticCode::INTERNAL_ERROR)
            .and_then(Diagnostic::context)
    }

    /// Returns an internal failure payload without lossy index conversion.
    pub fn internal_cause(&self) -> Option<SchemaInternalCause> {
        self.diagnostics()
            .iter()
            .rev()
            .find_map(schema_internal_cause_from_diagnostic)
    }

    pub fn regex_pattern_start_offset(&self, diagnostic_index: usize) -> Option<usize> {
        let context = self.diagnostics().get(diagnostic_index)?.context()?;
        if context.subsystem() != "typol-schema"
            || context.stage() != "regex-pattern"
            || !context.failure_point().starts_with("pattern-start-offset")
        {
            return None;
        }
        usize::try_from(context.object_id()?).ok()
    }

    pub fn span_location(&self, span: SourceSpan) -> Option<SpanLocation> {
        match &self.storage {
            SchemaErrorStorage::Inline(diagnostic) => diagnostic_span_location(diagnostic, span),
            SchemaErrorStorage::Collected {
                diagnostics,
                locations,
                ..
            } => locations
                .as_deref()
                .and_then(|locations| find_location(locations, span))
                .or_else(|| {
                    diagnostics
                        .iter()
                        .find_map(|diagnostic| diagnostic_span_location(diagnostic, span))
                }),
        }
    }

    /// Writes a full Schema diagnostic report with names registered by the caller.
    pub fn write_to_with_source_names<'name, W: fmt::Write + ?Sized>(
        &'name self,
        source_name: &dyn Fn(SourceId) -> Option<&'name str>,
        writer: &mut W,
    ) -> fmt::Result {
        let names = source_name;
        let source_name = |source_id| names(source_id);
        let span_location = |span| self.span_location(span);
        let diagnostics = self.diagnostics();
        let regular = diagnostics.regular();
        if let Some(terminal) = diagnostics.terminal() {
            if !regular.is_empty() {
                DiagnosticReport::new(
                    "schema compilation failed",
                    None,
                    regular,
                    ReportContext::new(&source_name, &span_location, self.excerpts()),
                )
                .write_to(writer)?;
                writer.write_str("\n\n")?;
            }
            let title = if regular.is_empty() {
                "schema compilation failed"
            } else {
                "schema compilation terminated"
            };
            DiagnosticReport::new(
                title,
                None,
                from_ref(terminal),
                ReportContext::new(&source_name, &span_location, self.excerpts()),
            )
            .write_to(writer)?;
        } else {
            DiagnosticReport::new(
                "schema compilation failed",
                None,
                regular,
                ReportContext::new(&source_name, &span_location, self.excerpts()),
            )
            .write_to(writer)?;
        }
        if let Some(cause) = self.internal_cause() {
            write!(writer, "\ninternal cause: {cause}")?;
        }
        if let Some(failure) = self.context_failure() {
            write!(writer, "\ncontext error: {failure}")?;
        }
        Ok(())
    }
}

fn diagnostic_span_location(diagnostic: &Diagnostic, span: SourceSpan) -> Option<SpanLocation> {
    if let Some(located) = diagnostic.located_primary() {
        if located.source_span() == span {
            return located.location();
        }
    }
    diagnostic
        .labels()
        .iter()
        .find(|label| label.span() == span)
        .and_then(|label| label.located_span().location())
}

impl From<BudgetError> for SchemaCompileErrors {
    fn from(error: BudgetError) -> Self {
        Self::budget_failure(error)
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
    if point.starts_with("invalid-source-span") {
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

impl fmt::Debug for SchemaCompileErrors {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SchemaCompileErrors")
            .field("diagnostic_count", &self.diagnostics().len())
            .field("location_status", &self.location_status())
            .finish()
    }
}

impl fmt::Display for SchemaCompileErrors {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let source_name = |_source_id| None;
        self.write_to_with_source_names(&source_name, formatter)
    }
}

impl Error for SchemaCompileErrors {}

fn find_location(records: &[(SourceSpan, SpanLocation)], span: SourceSpan) -> Option<SpanLocation> {
    records
        .binary_search_by(|(candidate, _)| compare_spans(*candidate, span))
        .ok()
        .and_then(|index| records.get(index).map(|(_, location)| *location))
}

fn compare_spans(left: SourceSpan, right: SourceSpan) -> Ordering {
    left.source_id()
        .get()
        .cmp(&right.source_id().get())
        .then_with(|| left.span().start().cmp(&right.span().start()))
        .then_with(|| left.span().end().cmp(&right.span().end()))
}

fn schema_internal_cause_from_diagnostic(diagnostic: &Diagnostic) -> Option<SchemaInternalCause> {
    if diagnostic.code() != DiagnosticCode::INTERNAL_ERROR {
        return None;
    }
    let context = diagnostic.context()?;
    if !is_ast_access_context(context) {
        return None;
    }
    match context.failure_point() {
        "invalid-node-id" => Some(SchemaInternalCause::AstAccess(
            AstAccessError::InvalidNodeId {
                id: context.object_index()?,
            },
        )),
        "invalid-list-range" => {
            let (start, end) = context.object_range()?;
            Some(SchemaInternalCause::AstAccess(
                AstAccessError::InvalidListRange { start, end },
            ))
        }
        "invalid-text-range" => {
            let (start, end) = context.object_range()?;
            Some(SchemaInternalCause::AstAccess(
                AstAccessError::InvalidTextRange { start, end },
            ))
        }
        "invalid-decoded-text-id" => Some(SchemaInternalCause::AstAccess(
            AstAccessError::InvalidDecodedTextId {
                id: context.object_index()?,
            },
        )),
        _ => None,
    }
}

fn is_ast_access_context(context: DiagnosticContext) -> bool {
    (context.subsystem() == "typol-schema" && context.stage() == "ast-access")
        || (context.subsystem() == "typol-syntax" && context.stage() == "ast-storage")
}

#[cfg(test)]
mod tests {
    use super::{SchemaCompileErrors, SchemaInternalCause};
    use crate::diagnostics::EmergencyDiagnostics;
    use alloc::string::ToString;
    use typol_core::{Diagnostic, DiagnosticCode};
    use typol_syntax::AstAccessError;

    fn errors_for_ast_access(error: AstAccessError) -> SchemaCompileErrors {
        let mut diagnostic = Diagnostic::emergency(DiagnosticCode::INTERNAL_ERROR);
        diagnostic.set_context(Some(EmergencyDiagnostics::ast_access_context(error)));
        SchemaCompileErrors::inline(diagnostic)
    }

    #[test]
    fn internal_cause_keeps_ast_access_variants_and_full_usize_values() {
        let node_id = usize::MAX;
        let node_errors = errors_for_ast_access(AstAccessError::InvalidNodeId { id: node_id });
        assert_eq!(
            node_errors.internal_cause(),
            Some(SchemaInternalCause::AstAccess(
                AstAccessError::InvalidNodeId { id: node_id }
            ))
        );

        let list_start = usize::MAX - 1;
        let list_end = usize::MAX;
        let list_errors = errors_for_ast_access(AstAccessError::InvalidListRange {
            start: list_start,
            end: list_end,
        });
        assert_eq!(
            list_errors.internal_cause(),
            Some(SchemaInternalCause::AstAccess(
                AstAccessError::InvalidListRange {
                    start: list_start,
                    end: list_end,
                }
            ))
        );

        let text_errors = errors_for_ast_access(AstAccessError::InvalidTextRange {
            start: 3,
            end: usize::MAX,
        });
        assert_eq!(
            text_errors.internal_cause(),
            Some(SchemaInternalCause::AstAccess(
                AstAccessError::InvalidTextRange {
                    start: 3,
                    end: usize::MAX,
                }
            ))
        );

        let decoded_id = usize::MAX - 2;
        let decoded_errors =
            errors_for_ast_access(AstAccessError::InvalidDecodedTextId { id: decoded_id });
        assert_eq!(
            decoded_errors.internal_cause(),
            Some(SchemaInternalCause::AstAccess(
                AstAccessError::InvalidDecodedTextId { id: decoded_id }
            ))
        );
        assert!(decoded_errors.to_string().contains(&decoded_id.to_string()));
    }
}
