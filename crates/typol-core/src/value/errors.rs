use crate::{BudgetError, DiagnosticContext};
use core::{
    error::Error,
    fmt::{self, Display},
};

/// Dynamic allocation failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AllocationError;

impl Display for AllocationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("memory allocation failed")
    }
}

impl Error for AllocationError {}

impl From<BudgetError> for AllocationError {
    fn from(_: BudgetError) -> Self {
        Self
    }
}

/// The category of a configuration value source, excluding source text and environment values.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ValueSource {
    ConfigLiteral,
    SchemaDefault,
    EnvironmentVariable,
    ExplicitFallback,
    RustValue,
}

/// A structural error found while constructing a value container.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValueErrorKind {
    DuplicateMapKey,
    DuplicateFieldName,
    AllocationFailed,
    ResourceLimitExceeded,
    CapacityExceeded,
    InternalInvariantFailure,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ValueError {
    kind: ValueErrorKind,
    first_index: Option<usize>,
    second_index: Option<usize>,
    context: Option<DiagnosticContext>,
}

impl ValueError {
    pub const fn kind(self) -> ValueErrorKind {
        self.kind
    }

    pub const fn duplicate_indices(self) -> Option<(usize, usize)> {
        match (self.first_index, self.second_index) {
            (Some(first), Some(second)) => Some((first, second)),
            _ => None,
        }
    }

    pub const fn context(self) -> Option<DiagnosticContext> {
        self.context
    }

    pub(super) const fn duplicate(kind: ValueErrorKind, first: usize, second: usize) -> Self {
        Self {
            kind,
            first_index: Some(first),
            second_index: Some(second),
            context: None,
        }
    }

    pub(super) const fn internal(context: DiagnosticContext) -> Self {
        Self {
            kind: ValueErrorKind::InternalInvariantFailure,
            first_index: None,
            second_index: None,
            context: Some(context),
        }
    }
}

impl Display for ValueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self.kind {
            ValueErrorKind::DuplicateMapKey => "duplicate map key",
            ValueErrorKind::DuplicateFieldName => "duplicate field name",
            ValueErrorKind::AllocationFailed => "memory allocation failed",
            ValueErrorKind::ResourceLimitExceeded => "execution resource limit exceeded",
            ValueErrorKind::CapacityExceeded => "workspace capacity exceeded",
            ValueErrorKind::InternalInvariantFailure => "config value invariant violated",
        };
        formatter.write_str(message)
    }
}

impl Error for ValueError {}

impl From<BudgetError> for ValueError {
    fn from(error: BudgetError) -> Self {
        let kind = match error {
            BudgetError::Resource
            | BudgetError::RegexSteps
            | BudgetError::RegexSearchSteps
            | BudgetError::RegexWorkspace => ValueErrorKind::ResourceLimitExceeded,
            BudgetError::Allocation => ValueErrorKind::AllocationFailed,
            BudgetError::CapacityExceeded => ValueErrorKind::CapacityExceeded,
            BudgetError::InvariantViolation(_) => ValueErrorKind::InternalInvariantFailure,
        };
        Self {
            kind,
            first_index: None,
            second_index: None,
            context: error.context(),
        }
    }
}
