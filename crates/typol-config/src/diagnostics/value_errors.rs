use crate::diagnostics::ConfigParseErrors;
use alloc::vec::Vec;
use core::{error::Error, fmt};
use typol_core::{
    BudgetError, ChargedString, Diagnostic, SourceExcerptSet, SourceId, SourceSpan, SpanLocation,
};
use typol_report::{DiagnosticReport, ReportContext};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigFailureStage {
    Input,
    Parsing,
}

pub(super) enum ErrorStorage {
    Emergency {
        diagnostic: Diagnostic,
        source_id: SourceId,
    },
    InputEmergency {
        diagnostic: Diagnostic,
        source_id: SourceId,
    },
    UnlocatedEmergency(Diagnostic),
    UnlocatedInputEmergency(Diagnostic),
    Collected {
        stage: ConfigFailureStage,
        diagnostics: Vec<Diagnostic>,
        source_id: Option<SourceId>,
        source_name: Option<ChargedString>,
        locations: Vec<(SourceSpan, SpanLocation)>,
        excerpts: SourceExcerptSet,
        truncated: bool,
    },
}

/// Text-free validation errors own safe diagnostics and Schema location snapshots.
pub struct ConfigValueErrors {
    errors: ConfigParseErrors,
}

impl ConfigValueErrors {
    pub fn excerpts(&self) -> &SourceExcerptSet {
        self.errors.excerpts()
    }

    pub(crate) fn from_errors(errors: ConfigParseErrors) -> Self {
        Self { errors }
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        self.errors.diagnostics()
    }

    /// Returns whether collection stopped because the diagnostic limit was reached.
    pub fn is_truncated(&self) -> bool {
        self.errors.is_truncated()
    }

    pub fn config_source_id(&self) -> Option<SourceId> {
        self.errors.config_source_id()
    }

    pub fn config_source_name(&self) -> Option<&str> {
        self.errors.config_source_name()
    }

    pub fn span_location(&self, span: SourceSpan) -> Option<SpanLocation> {
        self.errors.span_location(span)
    }
}

impl From<ConfigParseErrors> for ConfigValueErrors {
    fn from(errors: ConfigParseErrors) -> Self {
        Self::from_errors(errors)
    }
}

impl From<BudgetError> for ConfigValueErrors {
    fn from(error: BudgetError) -> Self {
        Self::from_errors(ConfigParseErrors::from(error))
    }
}

impl fmt::Debug for ConfigValueErrors {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConfigValueErrors")
            .field("diagnostic_count", &self.diagnostics().len())
            .field("truncated", &self.is_truncated())
            .finish()
    }
}

impl fmt::Display for ConfigValueErrors {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let source_name = |source_id| {
            (self.config_source_id() == Some(source_id))
                .then(|| self.config_source_name())
                .flatten()
        };
        let span_location = |span| self.span_location(span);
        DiagnosticReport::new(
            "configuration validation failed",
            Some("validation"),
            self.diagnostics(),
            ReportContext::new(&source_name, &span_location, self.excerpts()),
        )
        .write_to(formatter)?;
        if self.is_truncated() {
            formatter.write_str(
                "\n\nDiagnostic collection was truncated; subsequent errors were not collected.",
            )?;
        }
        Ok(())
    }
}

impl Error for ConfigValueErrors {}
