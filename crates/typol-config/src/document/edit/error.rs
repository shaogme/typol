use core::{
    error::Error,
    fmt::{self, Debug, Display, Formatter},
};
use typol_core::BudgetError;

use super::super::encoding::ConfigEditErrorKind;
use crate::ConfigParseErrors;

pub struct ConfigEditError {
    kind: ConfigEditErrorKind,
    validation: Option<ConfigParseErrors>,
}

impl ConfigEditError {
    pub const fn kind(&self) -> ConfigEditErrorKind {
        self.kind
    }

    pub fn validation_errors(&self) -> Option<&ConfigParseErrors> {
        self.validation.as_ref()
    }

    pub(crate) fn new(kind: ConfigEditErrorKind) -> Self {
        Self {
            kind,
            validation: None,
        }
    }

    pub(crate) fn validation(errors: ConfigParseErrors) -> Self {
        Self {
            kind: ConfigEditErrorKind::ValidationFailed,
            validation: Some(errors),
        }
    }

    pub(super) fn budget_kind(error: BudgetError) -> ConfigEditErrorKind {
        match error {
            BudgetError::Resource
            | BudgetError::RegexSteps
            | BudgetError::RegexSearchSteps
            | BudgetError::RegexWorkspace
            | BudgetError::CapacityExceeded => ConfigEditErrorKind::ResourceLimitExceeded,
            BudgetError::Allocation | BudgetError::InvariantViolation(_) => {
                ConfigEditErrorKind::AllocationFailed
            }
        }
    }

    fn from_budget(error: BudgetError) -> Self {
        Self::new(Self::budget_kind(error))
    }
}

impl From<BudgetError> for ConfigEditError {
    fn from(error: BudgetError) -> Self {
        Self::from_budget(error)
    }
}

impl Debug for ConfigEditError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConfigEditError")
            .field("kind", &self.kind)
            .field(
                "validation_diagnostic_count",
                &self
                    .validation
                    .as_ref()
                    .map(|errors| errors.diagnostics().len()),
            )
            .finish()
    }
}

impl Display for ConfigEditError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.kind {
            ConfigEditErrorKind::InvalidPath => "configuration edit path is invalid",
            ConfigEditErrorKind::ValueNotExplicit => {
                "configuration edit path has no explicit source value"
            }
            ConfigEditErrorKind::EncodingFailed => "configuration value encoding failed",
            ConfigEditErrorKind::EditConflict => "configuration edits overlap in source",
            ConfigEditErrorKind::InvalidContainer => {
                "configuration edit path does not name a supported container"
            }
            ConfigEditErrorKind::EntryAlreadyExists => "configuration edit entry already exists",
            ConfigEditErrorKind::ValidationFailed => "edited configuration failed validation",
            ConfigEditErrorKind::ResourceLimitExceeded => {
                "configuration edit resource limit exceeded"
            }
            ConfigEditErrorKind::AllocationFailed => "configuration edit allocation failed",
        })
    }
}

impl Error for ConfigEditError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.validation
            .as_ref()
            .map(|errors| errors as &(dyn Error + 'static))
    }
}
