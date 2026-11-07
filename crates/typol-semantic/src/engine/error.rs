use crate::SchemaAccessError;
use typol_core::{BudgetError, DiagnosticContext, SourceSpan};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EvalFailure {
    Internal,
    InternalAt(DiagnosticContext),
    SchemaAccess(SchemaAccessError),
    Resource,
    Arithmetic,
    Allocation,
    Capacity,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EvalError {
    Internal,
    InternalAt {
        context: DiagnosticContext,
        source_span: Option<SourceSpan>,
    },
    SchemaAccess(SchemaAccessError),
    SchemaAccessAt {
        error: SchemaAccessError,
        source_span: SourceSpan,
    },
    Resource,
    Arithmetic,
    Allocation,
    Capacity,
    Located {
        error: EvalFailure,
        source_span: SourceSpan,
    },
}

impl EvalError {
    pub const fn internal(context: DiagnosticContext) -> Self {
        Self::InternalAt {
            context,
            source_span: None,
        }
    }

    pub const fn kind(self) -> EvalFailure {
        match self {
            Self::Internal => EvalFailure::Internal,
            Self::InternalAt { context, .. } => EvalFailure::InternalAt(context),
            Self::SchemaAccess(error) | Self::SchemaAccessAt { error, .. } => {
                EvalFailure::SchemaAccess(error)
            }
            Self::Resource => EvalFailure::Resource,
            Self::Arithmetic => EvalFailure::Arithmetic,
            Self::Allocation => EvalFailure::Allocation,
            Self::Capacity => EvalFailure::Capacity,
            Self::Located { error, .. } => error,
        }
    }

    pub const fn context(self) -> Option<DiagnosticContext> {
        self.kind().context()
    }

    pub const fn is_internal(self) -> bool {
        self.kind().is_internal()
    }

    pub const fn source_span(self) -> Option<SourceSpan> {
        match self {
            Self::InternalAt { source_span, .. } => source_span,
            Self::SchemaAccessAt { source_span, .. } => Some(source_span),
            Self::Located { source_span, .. } => Some(source_span),
            _ => None,
        }
    }

    pub const fn schema_access(self) -> Option<SchemaAccessError> {
        self.kind().schema_access()
    }

    pub const fn with_source_span(self, source_span: SourceSpan) -> Self {
        Self::Located {
            error: self.kind(),
            source_span,
        }
    }
}

impl EvalFailure {
    pub const fn context(self) -> Option<DiagnosticContext> {
        match self {
            Self::InternalAt(context) => Some(context),
            Self::SchemaAccess(error) => Some(error.context()),
            Self::Internal
            | Self::Resource
            | Self::Arithmetic
            | Self::Allocation
            | Self::Capacity => None,
        }
    }

    pub const fn is_internal(self) -> bool {
        matches!(
            self,
            Self::Internal | Self::InternalAt(_) | Self::SchemaAccess(_)
        )
    }

    pub const fn schema_access(self) -> Option<SchemaAccessError> {
        match self {
            Self::SchemaAccess(error) => Some(error),
            _ => None,
        }
    }
}

impl From<BudgetError> for EvalError {
    fn from(error: BudgetError) -> Self {
        match error {
            BudgetError::Resource
            | BudgetError::RegexSteps
            | BudgetError::RegexSearchSteps
            | BudgetError::RegexWorkspace => Self::Resource,
            BudgetError::Allocation => Self::Allocation,
            BudgetError::CapacityExceeded => Self::Capacity,
            BudgetError::InvariantViolation(context) => Self::internal(*context),
        }
    }
}

impl From<SchemaAccessError> for EvalError {
    fn from(error: SchemaAccessError) -> Self {
        Self::SchemaAccess(error)
    }
}
