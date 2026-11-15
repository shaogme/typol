use crate::{
    BudgetError, CapacityExceeded, DiagnosticContext, InternalError, ResourceLimit,
    diagnostic_context,
};
use core::{
    error::Error,
    fmt::{self, Display},
};

static ENVIRONMENT_REFERENCE_REQUIRED: DiagnosticContext = diagnostic_context!(
    "typol-core",
    "value-arena",
    "environment-source-requires-reference",
);

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

/// A structural error found while constructing a value container.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValueErrorKind {
    DuplicateMapKey,
    DuplicateFieldName,
    EnvironmentReferenceRequired,
    AllocationFailed,
    ResourceLimit(ResourceLimit),
    CapacityExceeded(CapacityExceeded),
    Internal(InternalError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ValueError {
    kind: ValueErrorKind,
    duplicate_indices: Option<(usize, usize)>,
}

impl ValueError {
    pub const fn kind(self) -> ValueErrorKind {
        self.kind
    }

    pub const fn duplicate_indices(self) -> Option<(usize, usize)> {
        self.duplicate_indices
    }

    pub const fn context(self) -> Option<DiagnosticContext> {
        match self.kind {
            ValueErrorKind::Internal(error) => Some(error.context()),
            ValueErrorKind::EnvironmentReferenceRequired => Some(ENVIRONMENT_REFERENCE_REQUIRED),
            ValueErrorKind::DuplicateMapKey
            | ValueErrorKind::DuplicateFieldName
            | ValueErrorKind::AllocationFailed
            | ValueErrorKind::ResourceLimit(_)
            | ValueErrorKind::CapacityExceeded(_) => None,
        }
    }

    pub(super) const fn duplicate(kind: ValueErrorKind, first: usize, second: usize) -> Self {
        Self {
            kind,
            duplicate_indices: Some((first, second)),
        }
    }

    pub(super) const fn internal(context: DiagnosticContext) -> Self {
        Self {
            kind: ValueErrorKind::Internal(InternalError::ValueStructureInvariantFailure(context)),
            duplicate_indices: None,
        }
    }

    pub(super) const fn environment_reference_required() -> Self {
        Self {
            kind: ValueErrorKind::EnvironmentReferenceRequired,
            duplicate_indices: None,
        }
    }
}

impl Display for ValueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self.kind {
            ValueErrorKind::DuplicateMapKey => "duplicate map key",
            ValueErrorKind::DuplicateFieldName => "duplicate field name",
            ValueErrorKind::EnvironmentReferenceRequired => {
                "environment source requires structured reference metadata"
            }
            ValueErrorKind::AllocationFailed => "memory allocation failed",
            ValueErrorKind::ResourceLimit(limit) => {
                return write!(formatter, "{limit} limit exceeded");
            }
            ValueErrorKind::CapacityExceeded(_) => "workspace capacity exceeded",
            ValueErrorKind::Internal(_) => "config value invariant violated",
        };
        formatter.write_str(message)
    }
}

impl Error for ValueError {}

impl From<BudgetError> for ValueError {
    fn from(error: BudgetError) -> Self {
        let kind = match error {
            BudgetError::ResourceLimit(limit) => ValueErrorKind::ResourceLimit(limit),
            BudgetError::Allocation => ValueErrorKind::AllocationFailed,
            BudgetError::CapacityExceeded(capacity) => ValueErrorKind::CapacityExceeded(capacity),
            BudgetError::Internal(error) => ValueErrorKind::Internal(error),
        };
        Self {
            kind,
            duplicate_indices: None,
        }
    }
}
