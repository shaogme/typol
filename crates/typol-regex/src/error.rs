use typol_core::{
    BudgetError, CapacityExceeded, DiagnosticCode, DiagnosticContext, InternalError, ResourceLimit,
    diagnostic_context,
};

pub(crate) static COMPILE_BUDGET_ERROR: DiagnosticContext =
    diagnostic_context!("typol-regex", "regex-compile", "budget-error-conversion",);
pub(crate) static PARSER_ALTERNATION_BRANCH_MISSING: DiagnosticContext =
    diagnostic_context!("typol-regex", "regex-compile", "alternation-branch-missing");
pub(crate) static PARSER_SINGLE_CHILD_MISSING: DiagnosticContext =
    diagnostic_context!("typol-regex", "regex-compile", "single-child-missing");
pub(crate) static PARSER_FIRST_CHILD_MISSING: DiagnosticContext =
    diagnostic_context!("typol-regex", "regex-compile", "first-child-missing");
pub(crate) static PARSER_NODE_MISSING: DiagnosticContext =
    diagnostic_context!("typol-regex", "regex-compile", "parser-node-missing");
pub(crate) static AST_NODE_MISSING: DiagnosticContext =
    diagnostic_context!("typol-regex", "regex-compile", "ast-node-missing");
pub(crate) static GROUP_FRAME_POP_MISSING: DiagnosticContext =
    diagnostic_context!("typol-regex", "regex-compile", "group-frame-pop-missing");
pub(crate) static GROUP_FRAME_CURRENT_MISSING: DiagnosticContext = diagnostic_context!(
    "typol-regex",
    "regex-compile",
    "current-group-frame-missing",
);
pub(crate) static CHARACTER_CLASS_RANGE_MISSING: DiagnosticContext = diagnostic_context!(
    "typol-regex",
    "regex-compile",
    "character-class-range-missing",
);
pub(crate) static CHARACTER_CLASS_BUDGET_MISSING: DiagnosticContext = diagnostic_context!(
    "typol-regex",
    "regex-compile",
    "character-class-budget-missing",
);
pub(crate) static CHARACTER_CLASS_SLOT_MISSING: DiagnosticContext = diagnostic_context!(
    "typol-regex",
    "regex-compile",
    "normalized-range-slot-missing",
);
pub(crate) static CHARACTER_CLASS_PREVIOUS_RANGE_MISSING: DiagnosticContext = diagnostic_context!(
    "typol-regex",
    "regex-compile",
    "previous-range-index-missing"
);
pub(crate) static COMPILER_FINAL_FRAGMENT_MISSING: DiagnosticContext =
    diagnostic_context!("typol-regex", "regex-compile", "final-fragment-missing");
pub(crate) static COMPILER_PATCH_INSTRUCTION_MISSING: DiagnosticContext =
    diagnostic_context!("typol-regex", "regex-compile", "patch-instruction-missing",);
pub(crate) static COMPILER_PATCH_KIND_MISMATCH: DiagnosticContext = diagnostic_context!(
    "typol-regex",
    "regex-compile",
    "patch-instruction-kind-mismatch",
);
pub(crate) static COMPILER_PROGRAM_START_OUT_OF_RANGE: DiagnosticContext =
    diagnostic_context!("typol-regex", "regex-compile", "program-start-out-of-range",);
pub(crate) static COMPILER_PROGRAM_TARGET_OUT_OF_RANGE: DiagnosticContext = diagnostic_context!(
    "typol-regex",
    "regex-compile",
    "program-target-out-of-range",
);
pub(crate) static MATCH_BUDGET_ERROR: DiagnosticContext =
    diagnostic_context!("typol-regex", "regex-match", "budget-error-conversion",);
pub(crate) static MATCH_ACTIVE_STATE_MISSING: DiagnosticContext =
    diagnostic_context!("typol-regex", "regex-match", "active-state-missing",);
pub(crate) static MATCH_CURRENT_STATE_OUT_OF_RANGE: DiagnosticContext =
    diagnostic_context!("typol-regex", "regex-match", "current-state-out-of-range",);
pub(crate) static MATCH_CURRENT_STACK_CAPACITY_EXCEEDED: DiagnosticContext = diagnostic_context!(
    "typol-regex",
    "regex-match",
    "current-stack-capacity-exceeded"
);
pub(crate) static MATCH_VISITED_SLOT_MISSING: DiagnosticContext =
    diagnostic_context!("typol-regex", "regex-match", "visited-slot-missing",);
pub(crate) static MATCH_NEXT_STATE_OUT_OF_RANGE: DiagnosticContext =
    diagnostic_context!("typol-regex", "regex-match", "next-state-out-of-range",);
pub(crate) static MATCH_NEXT_STACK_CAPACITY_EXCEEDED: DiagnosticContext =
    diagnostic_context!("typol-regex", "regex-match", "next-stack-capacity-exceeded");
pub(crate) static MATCH_NEXT_SEEN_SLOT_MISSING: DiagnosticContext =
    diagnostic_context!("typol-regex", "regex-match", "next-seen-slot-missing",);
pub(crate) static MATCH_CHARACTER_CLASS_RANGE_MISSING: DiagnosticContext = diagnostic_context!(
    "typol-regex",
    "regex-match",
    "character-class-range-missing",
);
pub(crate) static MATCH_INSTRUCTION_OUT_OF_RANGE: DiagnosticContext =
    diagnostic_context!("typol-regex", "regex-match", "instruction-out-of-range",);
pub(crate) static SCRATCH_STATE_BUFFER_INITIALIZATION: DiagnosticContext = diagnostic_context!(
    "typol-regex",
    "regex-scratch-init",
    "state-buffer-initialization"
);
pub(crate) static SCRATCH_STATE_BUFFER_GROWTH: DiagnosticContext =
    diagnostic_context!("typol-regex", "regex-scratch-growth", "state-buffer-growth");
pub(crate) static SCRATCH_MARK_BUFFER_INITIALIZATION: DiagnosticContext = diagnostic_context!(
    "typol-regex",
    "regex-scratch-init",
    "mark-buffer-initialization"
);
pub(crate) static SCRATCH_MARK_BUFFER_GROWTH: DiagnosticContext =
    diagnostic_context!("typol-regex", "regex-scratch-growth", "mark-buffer-growth");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RegexErrorContext {
    context: DiagnosticContext,
    source_context: Option<DiagnosticContext>,
}

impl RegexErrorContext {
    pub(crate) const fn new(context: DiagnosticContext) -> Self {
        Self {
            context,
            source_context: None,
        }
    }

    pub(crate) const fn with_source_context(mut self, context: DiagnosticContext) -> Self {
        self.source_context = Some(context);
        self
    }

    pub(crate) const fn with_object_id(mut self, object_id: u64) -> Self {
        self.context = self.context.with_object_id(object_id);
        self
    }

    pub const fn diagnostic_context(&self) -> DiagnosticContext {
        self.context
    }

    pub const fn source_context(&self) -> Option<DiagnosticContext> {
        self.source_context
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PatternSpan {
    start: usize,
    end: usize,
}

impl PatternSpan {
    pub(crate) const fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    pub const fn start(&self) -> usize {
        self.start
    }

    pub const fn end(&self) -> usize {
        self.end
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegexSyntaxErrorKind {
    UnexpectedToken,
    InvalidEscape,
    EmptyAlternationBranch,
    UnclosedGroup,
    UnclosedCharacterClass,
    EmptyCharacterClass,
    InvalidCharacterRange,
    InvalidRepetition,
    RepetitionBoundExceeded,
    RepeatedQuantifier,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RegexSyntaxError {
    kind: RegexSyntaxErrorKind,
    span: PatternSpan,
}

impl RegexSyntaxError {
    pub(crate) const fn new(kind: RegexSyntaxErrorKind, span: PatternSpan) -> Self {
        Self { kind, span }
    }

    pub const fn kind(&self) -> RegexSyntaxErrorKind {
        self.kind
    }

    pub const fn span(&self) -> PatternSpan {
        self.span
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegexCompileError {
    Syntax(RegexSyntaxError),
    ResourceLimit(ResourceLimit),
    AllocationFailed,
    CapacityExceeded(CapacityExceeded),
    Internal {
        error: InternalError,
        context: RegexErrorContext,
    },
}

impl RegexCompileError {
    pub const fn context(&self) -> Option<RegexErrorContext> {
        match self {
            Self::Internal { context, .. } => Some(*context),
            Self::Syntax(_)
            | Self::ResourceLimit(_)
            | Self::AllocationFailed
            | Self::CapacityExceeded(_) => None,
        }
    }

    pub const fn internal_error(&self) -> Option<InternalError> {
        match self {
            Self::Internal { error, .. } => Some(*error),
            Self::Syntax(_)
            | Self::ResourceLimit(_)
            | Self::AllocationFailed
            | Self::CapacityExceeded(_) => None,
        }
    }

    pub const fn diagnostic_code(&self) -> DiagnosticCode {
        match self {
            Self::Syntax(_) => DiagnosticCode::REGEX_SYNTAX_ERROR,
            Self::ResourceLimit(limit) => limit.diagnostic_code(),
            Self::AllocationFailed => DiagnosticCode::ALLOCATION_FAILED,
            Self::CapacityExceeded(capacity) => capacity.diagnostic_code(),
            Self::Internal { error, .. } => error.diagnostic_code(),
        }
    }

    pub(crate) const fn internal(context: RegexErrorContext) -> Self {
        Self::Internal {
            error: InternalError::RegexCompilationInvariantFailure(context.diagnostic_context()),
            context,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegexScratchError {
    ResourceLimit(ResourceLimit),
    AllocationFailed,
    CapacityExceeded(CapacityExceeded),
    Internal {
        error: InternalError,
        context: RegexErrorContext,
    },
}

impl RegexScratchError {
    pub const fn context(&self) -> Option<RegexErrorContext> {
        match self {
            Self::Internal { context, .. } => Some(*context),
            Self::ResourceLimit(_) | Self::AllocationFailed | Self::CapacityExceeded(_) => None,
        }
    }

    pub const fn internal_error(&self) -> Option<InternalError> {
        match self {
            Self::Internal { error, .. } => Some(*error),
            Self::ResourceLimit(_) | Self::AllocationFailed | Self::CapacityExceeded(_) => None,
        }
    }

    pub const fn diagnostic_code(&self) -> DiagnosticCode {
        match self {
            Self::ResourceLimit(limit) => limit.diagnostic_code(),
            Self::AllocationFailed => DiagnosticCode::ALLOCATION_FAILED,
            Self::CapacityExceeded(capacity) => capacity.diagnostic_code(),
            Self::Internal { error, .. } => error.diagnostic_code(),
        }
    }

    pub(crate) const fn internal(context: RegexErrorContext) -> Self {
        Self::Internal {
            error: InternalError::RegexExecutionInvariantFailure(context.diagnostic_context()),
            context,
        }
    }
}

impl From<BudgetError> for RegexScratchError {
    fn from(error: BudgetError) -> Self {
        match error {
            BudgetError::ResourceLimit(limit) => Self::ResourceLimit(match limit {
                ResourceLimit::ExecutionSteps | ResourceLimit::RegexExecutionSteps => {
                    ResourceLimit::RegexExecutionSteps
                }
                ResourceLimit::WorkspaceBytes | ResourceLimit::RegexWorkspaceBytes => {
                    ResourceLimit::RegexWorkspaceBytes
                }
                ResourceLimit::RegexSearchSteps => ResourceLimit::RegexSearchSteps,
                other => other,
            }),
            BudgetError::Allocation => Self::AllocationFailed,
            BudgetError::CapacityExceeded(CapacityExceeded::BudgetBookkeeping) => {
                Self::CapacityExceeded(CapacityExceeded::RegexExecution)
            }
            BudgetError::CapacityExceeded(capacity) => Self::CapacityExceeded(capacity),
            BudgetError::Internal(error) => Self::Internal {
                error,
                context: RegexErrorContext::new(SCRATCH_STATE_BUFFER_INITIALIZATION)
                    .with_source_context(error.context()),
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegexMatchError {
    ResourceLimit(ResourceLimit),
    AllocationFailed,
    CapacityExceeded(CapacityExceeded),
    Internal {
        error: InternalError,
        context: RegexErrorContext,
    },
}

impl RegexMatchError {
    pub const fn context(&self) -> Option<RegexErrorContext> {
        match self {
            Self::Internal { context, .. } => Some(*context),
            Self::ResourceLimit(_) | Self::AllocationFailed | Self::CapacityExceeded(_) => None,
        }
    }

    pub const fn internal_error(&self) -> Option<InternalError> {
        match self {
            Self::Internal { error, .. } => Some(*error),
            Self::ResourceLimit(_) | Self::AllocationFailed | Self::CapacityExceeded(_) => None,
        }
    }

    pub const fn diagnostic_code(&self) -> DiagnosticCode {
        match self {
            Self::ResourceLimit(limit) => limit.diagnostic_code(),
            Self::AllocationFailed => DiagnosticCode::ALLOCATION_FAILED,
            Self::CapacityExceeded(capacity) => capacity.diagnostic_code(),
            Self::Internal { error, .. } => error.diagnostic_code(),
        }
    }

    pub(crate) const fn internal(context: RegexErrorContext) -> Self {
        Self::Internal {
            error: InternalError::RegexExecutionInvariantFailure(context.diagnostic_context()),
            context,
        }
    }
}

impl From<BudgetError> for RegexCompileError {
    fn from(error: BudgetError) -> Self {
        match error {
            BudgetError::ResourceLimit(limit) => Self::ResourceLimit(match limit {
                ResourceLimit::ExecutionSteps | ResourceLimit::RegexExecutionSteps => {
                    ResourceLimit::RegexCompilationBudget
                }
                ResourceLimit::WorkspaceBytes | ResourceLimit::RegexWorkspaceBytes => {
                    ResourceLimit::RegexWorkspaceBytes
                }
                ResourceLimit::RegexSearchSteps => ResourceLimit::RegexSearchSteps,
                other => other,
            }),
            BudgetError::Allocation => Self::AllocationFailed,
            BudgetError::CapacityExceeded(CapacityExceeded::BudgetBookkeeping) => {
                Self::CapacityExceeded(CapacityExceeded::RegexCompilation)
            }
            BudgetError::CapacityExceeded(capacity) => Self::CapacityExceeded(capacity),
            BudgetError::Internal(error) => Self::Internal {
                error,
                context: RegexErrorContext::new(COMPILE_BUDGET_ERROR)
                    .with_source_context(error.context()),
            },
        }
    }
}

impl From<BudgetError> for RegexMatchError {
    fn from(error: BudgetError) -> Self {
        match error {
            BudgetError::ResourceLimit(limit) => Self::ResourceLimit(match limit {
                ResourceLimit::ExecutionSteps | ResourceLimit::RegexExecutionSteps => {
                    ResourceLimit::RegexExecutionSteps
                }
                ResourceLimit::WorkspaceBytes | ResourceLimit::RegexWorkspaceBytes => {
                    ResourceLimit::RegexWorkspaceBytes
                }
                ResourceLimit::RegexSearchSteps => ResourceLimit::RegexSearchSteps,
                other => other,
            }),
            BudgetError::Allocation => Self::AllocationFailed,
            BudgetError::CapacityExceeded(CapacityExceeded::BudgetBookkeeping) => {
                Self::CapacityExceeded(CapacityExceeded::RegexExecution)
            }
            BudgetError::CapacityExceeded(capacity) => Self::CapacityExceeded(capacity),
            BudgetError::Internal(error) => Self::Internal {
                error,
                context: RegexErrorContext::new(MATCH_BUDGET_ERROR)
                    .with_source_context(error.context()),
            },
        }
    }
}
