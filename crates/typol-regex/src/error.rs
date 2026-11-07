use typol_core::{BudgetError, DiagnosticContext, diagnostic_context};

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
pub enum RegexResource {
    CompilationBudget,
    ExecutionSteps,
    WorkspaceBytes,
    PatternBytes,
    NestingDepth,
    InstructionCount,
    CharacterRangeCount,
    ScratchStateCount,
    SearchSteps,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegexCompileError {
    Syntax(RegexSyntaxError),
    ResourceLimitExceeded(RegexResource),
    AllocationFailed,
    CapacityExceeded,
    InternalInvariantFailure(RegexErrorContext),
}

impl RegexCompileError {
    pub const fn context(&self) -> Option<RegexErrorContext> {
        match self {
            Self::InternalInvariantFailure(context) => Some(*context),
            Self::Syntax(_)
            | Self::ResourceLimitExceeded(_)
            | Self::AllocationFailed
            | Self::CapacityExceeded => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegexScratchError {
    ResourceLimitExceeded(RegexResource),
    AllocationFailed,
    CapacityExceeded,
    InternalInvariantFailure(RegexErrorContext),
}

impl RegexScratchError {
    pub const fn context(&self) -> Option<RegexErrorContext> {
        match self {
            Self::InternalInvariantFailure(context) => Some(*context),
            Self::ResourceLimitExceeded(_) | Self::AllocationFailed | Self::CapacityExceeded => {
                None
            }
        }
    }
}

impl From<BudgetError> for RegexScratchError {
    fn from(error: BudgetError) -> Self {
        match error {
            BudgetError::Resource | BudgetError::RegexSteps => {
                Self::ResourceLimitExceeded(RegexResource::ExecutionSteps)
            }
            BudgetError::RegexWorkspace => {
                Self::ResourceLimitExceeded(RegexResource::WorkspaceBytes)
            }
            BudgetError::RegexSearchSteps => {
                Self::ResourceLimitExceeded(RegexResource::SearchSteps)
            }
            BudgetError::Allocation => Self::AllocationFailed,
            BudgetError::CapacityExceeded => Self::CapacityExceeded,
            BudgetError::InvariantViolation(source_context) => Self::InternalInvariantFailure(
                RegexErrorContext::new(SCRATCH_STATE_BUFFER_INITIALIZATION)
                    .with_source_context(*source_context),
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegexMatchError {
    ResourceLimitExceeded(RegexResource),
    AllocationFailed,
    CapacityExceeded,
    InternalInvariantFailure(RegexErrorContext),
}

impl RegexMatchError {
    pub const fn context(&self) -> Option<RegexErrorContext> {
        match self {
            Self::InternalInvariantFailure(context) => Some(*context),
            Self::ResourceLimitExceeded(_) | Self::AllocationFailed | Self::CapacityExceeded => {
                None
            }
        }
    }
}

impl From<BudgetError> for RegexCompileError {
    fn from(error: BudgetError) -> Self {
        match error {
            BudgetError::Resource | BudgetError::RegexSteps => {
                Self::ResourceLimitExceeded(RegexResource::CompilationBudget)
            }
            BudgetError::RegexWorkspace => {
                Self::ResourceLimitExceeded(RegexResource::WorkspaceBytes)
            }
            BudgetError::RegexSearchSteps => {
                Self::ResourceLimitExceeded(RegexResource::SearchSteps)
            }
            BudgetError::Allocation => Self::AllocationFailed,
            BudgetError::CapacityExceeded => Self::CapacityExceeded,
            BudgetError::InvariantViolation(source_context) => Self::InternalInvariantFailure(
                RegexErrorContext::new(COMPILE_BUDGET_ERROR).with_source_context(*source_context),
            ),
        }
    }
}

impl From<BudgetError> for RegexMatchError {
    fn from(error: BudgetError) -> Self {
        match error {
            BudgetError::Resource | BudgetError::RegexSteps => {
                Self::ResourceLimitExceeded(RegexResource::ExecutionSteps)
            }
            BudgetError::RegexSearchSteps => {
                Self::ResourceLimitExceeded(RegexResource::SearchSteps)
            }
            BudgetError::RegexWorkspace => {
                Self::ResourceLimitExceeded(RegexResource::WorkspaceBytes)
            }
            BudgetError::Allocation => Self::AllocationFailed,
            BudgetError::CapacityExceeded => Self::CapacityExceeded,
            BudgetError::InvariantViolation(source_context) => Self::InternalInvariantFailure(
                RegexErrorContext::new(MATCH_BUDGET_ERROR).with_source_context(*source_context),
            ),
        }
    }
}
