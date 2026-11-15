use super::regex::RegexExecutionError;
use crate::SchemaAccessError;
use typol_core::{
    BudgetError, CapacityExceeded, DiagnosticContext, InternalError, ResourceLimit, SourceSpan,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EvalFailure {
    Internal(InternalError),
    SchemaAccess(SchemaAccessError),
    RegexExecution(RegexExecutionError),
    ResourceLimit(ResourceLimit),
    Arithmetic,
    Allocation,
    CapacityExceeded(CapacityExceeded),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EvalError {
    Internal {
        error: InternalError,
        source_span: Option<SourceSpan>,
    },
    SchemaAccess(SchemaAccessError),
    SchemaAccessAt {
        error: SchemaAccessError,
        source_span: SourceSpan,
    },
    RegexExecutionAt {
        error: RegexExecutionError,
        source_span: SourceSpan,
    },
    ResourceLimit(ResourceLimit),
    Arithmetic,
    Allocation,
    CapacityExceeded(CapacityExceeded),
    Located {
        error: EvalFailure,
        source_span: SourceSpan,
    },
}

impl EvalError {
    pub const fn internal(context: DiagnosticContext) -> Self {
        Self::Internal {
            error: InternalError::SemanticEvaluationInvariantFailure(context),
            source_span: None,
        }
    }

    pub const fn kind(self) -> EvalFailure {
        match self {
            Self::Internal { error, .. } => EvalFailure::Internal(error),
            Self::SchemaAccess(error) | Self::SchemaAccessAt { error, .. } => {
                EvalFailure::SchemaAccess(error)
            }
            Self::RegexExecutionAt { error, .. } => EvalFailure::RegexExecution(error),
            Self::ResourceLimit(limit) => EvalFailure::ResourceLimit(limit),
            Self::Arithmetic => EvalFailure::Arithmetic,
            Self::Allocation => EvalFailure::Allocation,
            Self::CapacityExceeded(capacity) => EvalFailure::CapacityExceeded(capacity),
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
            Self::Internal { source_span, .. } => source_span,
            Self::SchemaAccessAt { source_span, .. } => Some(source_span),
            Self::RegexExecutionAt { source_span, .. } => Some(source_span),
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
            Self::Internal(error) => Some(error.context()),
            Self::SchemaAccess(error) => Some(error.context()),
            Self::RegexExecution(error) => error.context(),
            Self::ResourceLimit(_)
            | Self::Arithmetic
            | Self::Allocation
            | Self::CapacityExceeded(_) => None,
        }
    }

    pub const fn is_internal(self) -> bool {
        matches!(self, Self::Internal(_) | Self::SchemaAccess(_))
            || matches!(self, Self::RegexExecution(error) if error.is_internal())
    }

    pub const fn schema_access(self) -> Option<SchemaAccessError> {
        match self {
            Self::SchemaAccess(error) => Some(error),
            _ => None,
        }
    }
}

impl From<DiagnosticContext> for EvalError {
    fn from(context: DiagnosticContext) -> Self {
        Self::internal(context)
    }
}

pub(crate) struct EvaluationInvariant {
    context: DiagnosticContext,
    source_span: Option<SourceSpan>,
}

impl EvaluationInvariant {
    pub(crate) const fn new(context: DiagnosticContext) -> Self {
        Self {
            context,
            source_span: None,
        }
    }

    pub(crate) const fn with_source_span(mut self, source_span: SourceSpan) -> Self {
        self.source_span = Some(source_span);
        self
    }
}

impl From<EvaluationInvariant> for EvalError {
    fn from(failure: EvaluationInvariant) -> Self {
        Self::Internal {
            error: InternalError::SemanticEvaluationInvariantFailure(failure.context),
            source_span: failure.source_span,
        }
    }
}

impl From<BudgetError> for EvalError {
    fn from(error: BudgetError) -> Self {
        match error {
            BudgetError::ResourceLimit(limit) => Self::ResourceLimit(limit),
            BudgetError::Allocation => Self::Allocation,
            BudgetError::CapacityExceeded(capacity) => Self::CapacityExceeded(capacity),
            BudgetError::Internal(error) => Self::Internal {
                error,
                source_span: None,
            },
        }
    }
}

impl From<SchemaAccessError> for EvalError {
    fn from(error: SchemaAccessError) -> Self {
        Self::SchemaAccess(error)
    }
}
