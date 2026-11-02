mod factory;
mod value_errors;

pub(crate) use factory::{
    BudgetDiagnostic, DetailedDiagnosticRequest, DiagnosticFactory, ExecutionDiagnostic,
    RuleFailureLocation, RuleFailureMessage,
};

use crate::{options::ConfigLimits, secret::SecretPolicy};
use core::{cmp::Ordering, error::Error, fmt, slice::from_ref};
use opeo::{ErrSlot, OResult, Out};
use typol_core::{
    BudgetError, ChargedString, ChargedVec, Diagnostic, DiagnosticCode, DiagnosticContext,
    ExcerptLimits, ExecutionBudget, SourceExcerptSet, SourceId, SourceReadError,
    SourceReadErrorKind, SourceSpan, diagnostic_context,
};
use typol_report::{DiagnosticReport, ReportContext};
use typol_semantic::{CompiledSchema, SpanLocation};

use value_errors::ErrorStorage;
pub use value_errors::{ConfigFailureStage, ConfigValueErrors};

#[cfg(test)]
use core::ops::Deref;

static UNAVAILABLE_EXCERPTS: SourceExcerptSet = SourceExcerptSet::unavailable();
static CONFIG_EMPTY_PARSE_RESULT: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "diagnostic-collection",
    "empty-parse-result",
);
static CONFIG_MISSING_TERMINAL_DIAGNOSTIC: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "diagnostic-collection",
    "missing-terminal-diagnostic",
);
pub(crate) static NAMED_ENTRY_NAME_MISSING: DiagnosticContext =
    diagnostic_context!("typol-config", "value", "named-entry-name-missing");

pub(crate) struct DiagnosticBuffer<'schema> {
    schema: &'schema CompiledSchema,
    diagnostics: ChargedVec<Diagnostic>,
    terminal: Option<Diagnostic>,
    limit: usize,
    stopped: bool,
    truncated: bool,
    budget_errors: ErrSlot<BudgetError>,
    redaction_errors: ErrSlot<Diagnostic>,
}

impl<'schema> DiagnosticBuffer<'schema> {
    pub(super) fn new(schema: &'schema CompiledSchema, limit: usize) -> Self {
        Self {
            schema,
            diagnostics: ChargedVec::unmetered(),
            terminal: None,
            limit: limit.clamp(1, ConfigLimits::MAX_DIAGNOSTICS),
            stopped: false,
            truncated: false,
            budget_errors: ErrSlot::new(),
            redaction_errors: ErrSlot::new(),
        }
    }

    pub(super) fn new_budgeted<'slot, 'borrow>(
        schema: &'schema CompiledSchema,
        limit: usize,
        budget: &ExecutionBudget,
        out: Out<'slot, 'borrow, Diagnostic>,
    ) -> OResult<'slot, Self> {
        let mut buffer = Self::new(schema, limit);
        buffer.diagnostics = match buffer
            .budget_errors
            .try_call(|budget_out| ChargedVec::new(Some(budget), budget_out))
        {
            Ok(diagnostics) => diagnostics,
            Err(caught) => {
                return Err(out.fail(BudgetDiagnostic::from_error(caught.take())));
            }
        };
        Ok(buffer)
    }

    pub(super) fn push(&mut self, mut diagnostic: Diagnostic) -> bool {
        if self.stopped {
            return false;
        }
        // This is the sole redaction boundary before storage; later stages only organize safe diagnostics.
        if let Err(caught) = self
            .redaction_errors
            .try_call(|out| SecretPolicy::redact(self.schema, &mut diagnostic, out))
        {
            diagnostic = caught.take();
        }
        let code = diagnostic.code();
        if matches!(
            code,
            DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
                | DiagnosticCode::ALLOCATION_FAILED
                | DiagnosticCode::CAPACITY_EXCEEDED
                | DiagnosticCode::SOURCE_INPUT_ERROR
                | DiagnosticCode::INTERNAL_ERROR
                | DiagnosticCode::DIAGNOSTIC_LIMIT_REACHED
        ) {
            if self.len() >= self.limit {
                self.truncated = true;
                self.stopped = true;
                return false;
            }
            self.terminal = Some(diagnostic);
            self.truncated = code == DiagnosticCode::DIAGNOSTIC_LIMIT_REACHED;
            self.stopped = true;
            return false;
        }
        if self.limit == 1 {
            if let Err(caught) = self
                .budget_errors
                .try_call(|budget_out| self.diagnostics.try_reserve(1, budget_out))
            {
                self.terminal = Some(BudgetDiagnostic::from_error(caught.take()));
                self.stopped = true;
                return false;
            }
            if let Err(caught) = self
                .budget_errors
                .try_call(|budget_out| self.diagnostics.try_push(diagnostic, budget_out))
            {
                self.terminal = Some(BudgetDiagnostic::from_error(caught.take()));
                self.stopped = true;
                return false;
            }
            self.truncated = code != DiagnosticCode::LEXICAL_ERROR;
            self.stopped = true;
            return false;
        }
        if self.diagnostics.len() >= self.limit - 1 {
            self.terminal = Some(DiagnosticFactory::limit_diagnostic(
                diagnostic.located_primary(),
            ));
            self.truncated = true;
            self.stopped = true;
            return false;
        }
        // Ordinary errors grow on demand while retaining room to merge the inline terminal slot when returning a contiguous slice.
        if let Err(caught) = self
            .budget_errors
            .try_call(|budget_out| self.diagnostics.try_reserve(2, budget_out))
        {
            let error = caught.take();
            self.terminal = Some(BudgetDiagnostic::from_error(error));
            self.stopped = true;
            return false;
        }
        self.stopped = code == DiagnosticCode::LEXICAL_ERROR;
        if let Err(caught) = self
            .budget_errors
            .try_call(|budget_out| self.diagnostics.try_push(diagnostic, budget_out))
        {
            self.terminal = Some(BudgetDiagnostic::from_error(caught.take()));
            self.stopped = true;
        }
        !self.stopped
    }

    pub(super) fn stopped(&self) -> bool {
        self.stopped
    }

    pub(super) const fn is_truncated(&self) -> bool {
        self.truncated
    }

    pub(super) fn iter(&self) -> impl DoubleEndedIterator<Item = &Diagnostic> {
        self.diagnostics.iter().chain(self.terminal.iter())
    }

    pub(super) fn len(&self) -> usize {
        self.diagnostics.len() + usize::from(self.terminal.is_some())
    }

    pub(super) fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// An owned, nonempty set of configuration errors that does not retain source text or raw environment bytes.
pub struct ConfigParseErrors {
    storage: ErrorStorage,
}

impl ConfigParseErrors {
    pub fn excerpts(&self) -> &SourceExcerptSet {
        match &self.storage {
            ErrorStorage::Collected { excerpts, .. } => excerpts,
            _ => &UNAVAILABLE_EXCERPTS,
        }
    }

    /// Returns the underlying byte-source read error reported during parsing, if any.
    pub fn source_read_error(&self) -> Option<SourceReadError> {
        if self.stage() != ConfigFailureStage::Input {
            return None;
        }
        let diagnostic = self.diagnostics().last()?;
        let kind = match diagnostic.code() {
            DiagnosticCode::RESOURCE_LIMIT_EXCEEDED => SourceReadErrorKind::ResourceLimitExceeded,
            DiagnosticCode::ALLOCATION_FAILED => SourceReadErrorKind::AllocationFailed,
            DiagnosticCode::CAPACITY_EXCEEDED => SourceReadErrorKind::CapacityExceeded,
            DiagnosticCode::SOURCE_INPUT_ERROR => SourceReadErrorKind::HostFailure,
            DiagnosticCode::INTERNAL_ERROR => SourceReadErrorKind::InternalInvariantFailure,
            _ => return None,
        };
        let mut error = SourceReadError::new(kind);
        if let Some(context) = diagnostic.context() {
            error = error.with_context(context);
        }
        if let Some(span) = diagnostic.primary() {
            error = error.with_input(span.source_id(), span.span().start());
        }
        Some(error)
    }

    pub(crate) fn with_excerpts(mut self, excerpts: SourceExcerptSet) -> Self {
        if let ErrorStorage::Collected {
            excerpts: saved, ..
        } = &mut self.storage
        {
            *saved = excerpts;
        }
        self
    }

    pub(crate) fn schema_excerpts(
        &self,
        schema: &CompiledSchema,
        limits: ExcerptLimits,
        budget: &ExecutionBudget,
    ) -> SourceExcerptSet {
        let mut excerpts = SourceExcerptSet::new(limits, Some(budget));
        for diagnostic in self.diagnostics() {
            for span in diagnostic
                .primary()
                .into_iter()
                .chain(diagnostic.labels().iter().map(|label| label.span()))
            {
                if Some(span.source_id()) != self.config_source_id() {
                    excerpts.copy_span(schema.excerpts(), span);
                }
            }
        }
        excerpts
    }

    pub(crate) fn emergency(mut diagnostic: Diagnostic, source_id: Option<SourceId>) -> Self {
        let mut path_errors = ErrSlot::new();
        if let Err(caught) =
            path_errors.try_call(|path_out| diagnostic.try_set_path(None, None, path_out))
        {
            let _ = caught.take();
        }
        Self {
            storage: match source_id {
                Some(source_id) => ErrorStorage::Emergency {
                    diagnostic,
                    source_id,
                },
                None => ErrorStorage::UnlocatedEmergency(diagnostic),
            },
        }
    }

    pub(crate) fn from_buffer(
        buffer: DiagnosticBuffer,
        schema: &CompiledSchema,
        source_id: SourceId,
        source_name: Option<ChargedString>,
    ) -> Self {
        Self::finish(buffer, schema, Some(source_id), source_name)
    }

    pub(crate) fn finish(
        mut buffer: DiagnosticBuffer,
        schema: &CompiledSchema,
        source_id: Option<SourceId>,
        source_name: Option<ChargedString>,
    ) -> Self {
        if buffer
            .diagnostics
            .budget()
            .is_some_and(ExecutionBudget::exhausted)
        {
            return Self::emergency(
                BudgetDiagnostic::from_error(BudgetError::Resource),
                source_id,
            );
        }
        if buffer
            .terminal
            .as_ref()
            .is_some_and(|diagnostic| diagnostic.code() == DiagnosticCode::ALLOCATION_FAILED)
        {
            return match buffer.terminal.take() {
                Some(diagnostic) => Self::emergency(diagnostic, source_id),
                None => Self::emergency(
                    DiagnosticFactory::internal(
                        CONFIG_MISSING_TERMINAL_DIAGNOSTIC,
                        None,
                        None,
                        None,
                    ),
                    source_id,
                ),
            };
        }
        if buffer.is_empty() {
            return Self::emergency(
                DiagnosticFactory::internal(CONFIG_EMPTY_PARSE_RESULT, None, None, None),
                source_id,
            );
        }
        let budget = match buffer.diagnostics.budget() {
            Some(budget) => match buffer
                .budget_errors
                .try_call(|budget_out| budget.try_clone(budget_out))
            {
                Ok(budget) => Some(budget),
                Err(caught) => {
                    return Self::emergency(BudgetDiagnostic::from_error(caught.take()), source_id);
                }
            },
            None => None,
        };
        let sort_budget = budget.as_ref();
        let sort_error = {
            let diagnostics = &mut buffer.diagnostics;
            match buffer.budget_errors.try_call(|mut sort_out| {
                for index in 1..diagnostics.len() {
                    let mut position = index;
                    while position > 0 {
                        if let Some(budget) = sort_budget {
                            budget.consume(1, sort_out.reborrow())?;
                        }
                        let Some((previous, current)) =
                            diagnostics.get(position - 1).zip(diagnostics.get(position))
                        else {
                            break;
                        };
                        if Self::compare_diagnostics(previous, current) != Ordering::Greater {
                            break;
                        }
                        diagnostics.swap(position - 1, position);
                        position -= 1;
                    }
                }
                Ok(())
            }) {
                Ok(()) => None,
                Err(caught) => Some(caught.take()),
            }
        };
        if let Some(error) = sort_error {
            return Self::emergency(BudgetDiagnostic::from_error(error), source_id);
        }
        let mut spans = match buffer
            .budget_errors
            .try_call(|budget_out| ChargedVec::new(budget.as_ref(), budget_out))
        {
            Ok(spans) => spans,
            Err(caught) => {
                return Self::emergency(BudgetDiagnostic::from_error(caught.take()), source_id);
            }
        };
        for diagnostic in buffer.diagnostics.iter().chain(buffer.terminal.iter()) {
            if let Some(budget) = budget.as_ref() {
                if let Err(caught) = buffer
                    .budget_errors
                    .try_call(|budget_out| budget.consume(1, budget_out))
                {
                    return Self::emergency(BudgetDiagnostic::from_error(caught.take()), source_id);
                }
            }
            for span in diagnostic
                .located_primary()
                .into_iter()
                .chain(diagnostic.labels().iter().map(|label| label.located_span()))
            {
                if let Some(budget) = budget.as_ref() {
                    if let Err(caught) = buffer
                        .budget_errors
                        .try_call(|budget_out| budget.consume(1, budget_out))
                    {
                        return Self::emergency(
                            BudgetDiagnostic::from_error(caught.take()),
                            source_id,
                        );
                    }
                }
                if let Err(caught) = buffer
                    .budget_errors
                    .try_call(|budget_out| spans.try_reserve(1, budget_out))
                {
                    return Self::emergency(BudgetDiagnostic::from_error(caught.take()), source_id);
                }
                if let Err(caught) = buffer
                    .budget_errors
                    .try_call(|budget_out| spans.try_push(span, budget_out))
                {
                    return Self::emergency(BudgetDiagnostic::from_error(caught.take()), source_id);
                }
            }
        }
        if let Err(caught) = buffer.budget_errors.try_call(|sort_out| {
            spans.sort_by(
                |left, right, mut compare_out| {
                    if let Some(budget) = budget.as_ref() {
                        budget.consume(1, compare_out.reborrow())?;
                    }
                    Ok(Self::compare_spans(
                        &left.source_span(),
                        &right.source_span(),
                    ))
                },
                sort_out,
            )
        }) {
            return Self::emergency(BudgetDiagnostic::from_error(caught.take()), source_id);
        }
        if let Some(budget) = budget.as_ref() {
            if let Err(caught) = buffer
                .budget_errors
                .try_call(|budget_out| budget.consume(spans.len().saturating_sub(1), budget_out))
            {
                return Self::emergency(BudgetDiagnostic::from_error(caught.take()), source_id);
            }
        }
        spans.dedup();
        let mut locations = match buffer
            .budget_errors
            .try_call(|budget_out| ChargedVec::new(budget.as_ref(), budget_out))
        {
            Ok(locations) => locations,
            Err(caught) => {
                return Self::emergency(BudgetDiagnostic::from_error(caught.take()), source_id);
            }
        };
        if let Err(caught) = buffer
            .budget_errors
            .try_call(|budget_out| locations.try_reserve_exact(spans.len(), budget_out))
        {
            return Self::emergency(BudgetDiagnostic::from_error(caught.take()), source_id);
        }
        for located in spans {
            let span = located.source_span();
            if let Some(budget) = budget.as_ref() {
                if let Err(caught) = buffer
                    .budget_errors
                    .try_call(|budget_out| budget.consume(1, budget_out))
                {
                    return Self::emergency(BudgetDiagnostic::from_error(caught.take()), source_id);
                }
            }
            let location = if Some(span.source_id()) == source_id {
                located.location()
            } else {
                if let Some(budget) = budget.as_ref() {
                    match buffer.budget_errors.try_call(|budget_out| {
                        schema.span_location_budgeted(span, budget, budget_out)
                    }) {
                        Ok(location) => location,
                        Err(caught) => {
                            return Self::emergency(
                                BudgetDiagnostic::from_error(caught.take()),
                                source_id,
                            );
                        }
                    }
                } else {
                    schema.span_location(span)
                }
            };
            let Some(location) = location else {
                continue;
            };
            if let Err(caught) = buffer
                .budget_errors
                .try_call(|budget_out| locations.try_push((span, location), budget_out))
            {
                return Self::emergency(BudgetDiagnostic::from_error(caught.take()), source_id);
            }
        }
        if let Some(terminal) = buffer.terminal.take() {
            if let Err(caught) = buffer
                .budget_errors
                .try_call(|budget_out| buffer.diagnostics.try_reserve(1, budget_out))
            {
                return Self::emergency(BudgetDiagnostic::from_error(caught.take()), source_id);
            }
            if let Err(caught) = buffer
                .budget_errors
                .try_call(|budget_out| buffer.diagnostics.try_push(terminal, budget_out))
            {
                return Self::emergency(BudgetDiagnostic::from_error(caught.take()), source_id);
            }
        }
        let truncated = buffer.is_truncated();
        Self {
            storage: ErrorStorage::Collected {
                stage: ConfigFailureStage::Parsing,
                diagnostics: buffer.diagnostics.into_unmetered_vec(),
                source_id,
                source_name,
                locations: locations.into_unmetered_vec(),
                excerpts: SourceExcerptSet::unavailable(),
                truncated,
            },
        }
    }

    fn compare_spans(left: &SourceSpan, right: &SourceSpan) -> Ordering {
        left.source_id()
            .get()
            .cmp(&right.source_id().get())
            .then_with(|| left.span().start().cmp(&right.span().start()))
            .then_with(|| left.span().end().cmp(&right.span().end()))
    }

    fn compare_diagnostics(left: &Diagnostic, right: &Diagnostic) -> Ordering {
        match (left.located_primary(), right.located_primary()) {
            (Some(left), Some(right)) => {
                Self::compare_spans(&left.source_span(), &right.source_span())
            }
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        }
    }

    pub const fn stage(&self) -> ConfigFailureStage {
        match &self.storage {
            ErrorStorage::Emergency { .. } | ErrorStorage::UnlocatedEmergency(_) => {
                ConfigFailureStage::Parsing
            }
            ErrorStorage::InputEmergency { .. } | ErrorStorage::UnlocatedInputEmergency(_) => {
                ConfigFailureStage::Input
            }
            ErrorStorage::Collected { stage, .. } => *stage,
        }
    }

    pub(crate) fn with_stage(mut self, stage: ConfigFailureStage) -> Self {
        self.storage = match self.storage {
            ErrorStorage::Emergency {
                diagnostic,
                source_id,
            }
            | ErrorStorage::InputEmergency {
                diagnostic,
                source_id,
            } => match stage {
                ConfigFailureStage::Input => ErrorStorage::InputEmergency {
                    diagnostic,
                    source_id,
                },
                ConfigFailureStage::Parsing => ErrorStorage::Emergency {
                    diagnostic,
                    source_id,
                },
            },
            ErrorStorage::UnlocatedEmergency(diagnostic)
            | ErrorStorage::UnlocatedInputEmergency(diagnostic) => match stage {
                ConfigFailureStage::Input => ErrorStorage::UnlocatedInputEmergency(diagnostic),
                ConfigFailureStage::Parsing => ErrorStorage::UnlocatedEmergency(diagnostic),
            },
            ErrorStorage::Collected {
                diagnostics,
                source_id,
                source_name,
                locations,
                excerpts,
                truncated,
                ..
            } => ErrorStorage::Collected {
                stage,
                diagnostics,
                source_id,
                source_name,
                locations,
                excerpts,
                truncated,
            },
        };
        self
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        match &self.storage {
            ErrorStorage::Emergency { diagnostic, .. }
            | ErrorStorage::InputEmergency { diagnostic, .. }
            | ErrorStorage::UnlocatedEmergency(diagnostic)
            | ErrorStorage::UnlocatedInputEmergency(diagnostic) => from_ref(diagnostic),
            ErrorStorage::Collected { diagnostics, .. } => diagnostics,
        }
    }

    /// Returns whether collection stopped because the diagnostic limit was reached.
    pub fn is_truncated(&self) -> bool {
        match &self.storage {
            ErrorStorage::Collected { truncated, .. } => *truncated,
            _ => self
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code() == DiagnosticCode::DIAGNOSTIC_LIMIT_REACHED),
        }
    }

    pub const fn config_source_id(&self) -> Option<SourceId> {
        match &self.storage {
            ErrorStorage::Emergency { source_id, .. }
            | ErrorStorage::InputEmergency { source_id, .. } => Some(*source_id),
            ErrorStorage::UnlocatedEmergency(_) | ErrorStorage::UnlocatedInputEmergency(_) => None,
            ErrorStorage::Collected { source_id, .. } => *source_id,
        }
    }

    pub fn config_source_name(&self) -> Option<&str> {
        match &self.storage {
            ErrorStorage::Emergency { .. }
            | ErrorStorage::InputEmergency { .. }
            | ErrorStorage::UnlocatedEmergency(_)
            | ErrorStorage::UnlocatedInputEmergency(_) => None,
            ErrorStorage::Collected { source_name, .. } => source_name.as_deref(),
        }
    }

    /// Returns the location actually reported for this error, without depending on the original Schema or source text.
    pub fn span_location(&self, span: SourceSpan) -> Option<SpanLocation> {
        match &self.storage {
            ErrorStorage::Emergency { .. }
            | ErrorStorage::InputEmergency { .. }
            | ErrorStorage::UnlocatedEmergency(_)
            | ErrorStorage::UnlocatedInputEmergency(_) => None,
            ErrorStorage::Collected { locations, .. } => locations
                .binary_search_by(|(saved, _)| Self::compare_spans(saved, &span))
                .ok()
                .and_then(|index| locations.get(index).map(|(_, location)| *location)),
        }
    }
}

impl From<BudgetError> for ConfigParseErrors {
    fn from(error: BudgetError) -> Self {
        Self::emergency(BudgetDiagnostic::from_error(error), None)
    }
}

impl From<Diagnostic> for ConfigParseErrors {
    fn from(diagnostic: Diagnostic) -> Self {
        Self::emergency(diagnostic, None)
    }
}

impl fmt::Debug for ConfigParseErrors {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConfigParseErrors")
            .field("stage", &self.stage())
            .field("diagnostic_count", &self.diagnostics().len())
            .field("truncated", &self.is_truncated())
            .finish()
    }
}

impl fmt::Display for ConfigParseErrors {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let source_name = |source_id| {
            (self.config_source_id() == Some(source_id))
                .then(|| self.config_source_name())
                .flatten()
        };
        let span_location = |span| self.span_location(span);
        let stage = match self.stage() {
            ConfigFailureStage::Input => "input",
            ConfigFailureStage::Parsing => "parsing",
        };
        DiagnosticReport::new(
            "configuration load failed",
            Some(stage),
            self.diagnostics(),
            ReportContext::new(&source_name, &span_location, self.excerpts()),
        )
        .write_to(formatter)?;
        write_truncation_notice(formatter, self.is_truncated())
    }
}

fn write_truncation_notice(formatter: &mut fmt::Formatter<'_>, truncated: bool) -> fmt::Result {
    if truncated {
        formatter.write_str(
            "\n\nDiagnostic collection was truncated; subsequent errors were not collected.",
        )?;
    }
    Ok(())
}

impl Error for ConfigParseErrors {}

#[cfg(test)]
impl Deref for ConfigParseErrors {
    type Target = [Diagnostic];

    fn deref(&self) -> &Self::Target {
        self.diagnostics()
    }
}
