use opeo::{ErrSlot, OResult, Out};
use typol_core::{
    BudgetError, CapacityExceeded, DiagnosticCode, DiagnosticContext, InternalError,
    RegexExecutionBudget, ResourceLimit, SourceSpan, diagnostic_context,
};
use typol_policy::RegexLimits;
use typol_regex::{Regex, RegexErrorContext, RegexMatchError, RegexScratch, RegexScratchError};

use crate::CompiledRegexConstraint;

static REGEX_EXECUTOR_INTERNAL: DiagnosticContext = diagnostic_context!(
    "typol-semantic",
    "regex-execution",
    "scratch-buffer-missing",
);

pub struct RegexMatchOptions<'a> {
    pub value: Option<&'a str>,
    pub limits: &'a RegexLimits,
    pub budget: &'a RegexExecutionBudget,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegexExecutionFailure {
    Scratch(RegexScratchError),
    Match(RegexMatchError),
    Budget(BudgetError),
    Internal(InternalError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegexExecutionError {
    Scratch(RegexScratchError),
    Match(RegexMatchError),
    Budget(BudgetError),
    Internal {
        error: InternalError,
        source_span: Option<SourceSpan>,
    },
    Located {
        error: RegexExecutionFailure,
        source_span: SourceSpan,
    },
}

impl RegexExecutionError {
    pub const fn internal_error(self) -> Option<InternalError> {
        match self {
            Self::Scratch(error) => error.internal_error(),
            Self::Match(error) => error.internal_error(),
            Self::Budget(BudgetError::Internal(error)) | Self::Internal { error, .. } => {
                Some(error)
            }
            Self::Budget(
                BudgetError::ResourceLimit(_)
                | BudgetError::Allocation
                | BudgetError::CapacityExceeded(_),
            ) => None,
            Self::Located { error, .. } => match error {
                RegexExecutionFailure::Scratch(error) => error.internal_error(),
                RegexExecutionFailure::Match(error) => error.internal_error(),
                RegexExecutionFailure::Budget(BudgetError::Internal(error))
                | RegexExecutionFailure::Internal(error) => Some(error),
                RegexExecutionFailure::Budget(
                    BudgetError::ResourceLimit(_)
                    | BudgetError::Allocation
                    | BudgetError::CapacityExceeded(_),
                ) => None,
            },
        }
    }

    pub const fn capacity_exceeded(self) -> Option<CapacityExceeded> {
        match self {
            Self::Scratch(RegexScratchError::CapacityExceeded(capacity))
            | Self::Match(RegexMatchError::CapacityExceeded(capacity))
            | Self::Budget(BudgetError::CapacityExceeded(capacity)) => Some(capacity),
            Self::Scratch(
                RegexScratchError::ResourceLimit(_)
                | RegexScratchError::AllocationFailed
                | RegexScratchError::Internal { .. },
            )
            | Self::Match(
                RegexMatchError::ResourceLimit(_)
                | RegexMatchError::AllocationFailed
                | RegexMatchError::Internal { .. },
            )
            | Self::Budget(
                BudgetError::ResourceLimit(_) | BudgetError::Allocation | BudgetError::Internal(_),
            )
            | Self::Internal { .. } => None,
            Self::Located { error, .. } => match error {
                RegexExecutionFailure::Scratch(RegexScratchError::CapacityExceeded(capacity))
                | RegexExecutionFailure::Match(RegexMatchError::CapacityExceeded(capacity))
                | RegexExecutionFailure::Budget(BudgetError::CapacityExceeded(capacity)) => {
                    Some(capacity)
                }
                RegexExecutionFailure::Scratch(
                    RegexScratchError::ResourceLimit(_)
                    | RegexScratchError::AllocationFailed
                    | RegexScratchError::Internal { .. },
                )
                | RegexExecutionFailure::Match(
                    RegexMatchError::ResourceLimit(_)
                    | RegexMatchError::AllocationFailed
                    | RegexMatchError::Internal { .. },
                )
                | RegexExecutionFailure::Budget(
                    BudgetError::ResourceLimit(_)
                    | BudgetError::Allocation
                    | BudgetError::Internal(_),
                )
                | RegexExecutionFailure::Internal(_) => None,
            },
        }
    }

    pub const fn diagnostic_code(self) -> DiagnosticCode {
        match self {
            Self::Scratch(error) => error.diagnostic_code(),
            Self::Match(error) => error.diagnostic_code(),
            Self::Budget(error) => error.diagnostic_code(),
            Self::Internal { error, .. } => error.diagnostic_code(),
            Self::Located { error, .. } => match error {
                RegexExecutionFailure::Scratch(error) => error.diagnostic_code(),
                RegexExecutionFailure::Match(error) => error.diagnostic_code(),
                RegexExecutionFailure::Budget(error) => error.diagnostic_code(),
                RegexExecutionFailure::Internal(error) => error.diagnostic_code(),
            },
        }
    }

    pub const fn is_internal(self) -> bool {
        match self {
            Self::Internal { .. } => true,
            Self::Scratch(error) => error.internal_error().is_some(),
            Self::Match(error) => error.internal_error().is_some(),
            Self::Budget(error) => matches!(error, BudgetError::Internal(_)),
            Self::Located { error, .. } => match error {
                RegexExecutionFailure::Internal(_) => true,
                RegexExecutionFailure::Budget(error) => matches!(error, BudgetError::Internal(_)),
                RegexExecutionFailure::Scratch(error) => error.internal_error().is_some(),
                RegexExecutionFailure::Match(error) => error.internal_error().is_some(),
            },
        }
    }

    pub const fn context(self) -> Option<DiagnosticContext> {
        match self {
            Self::Scratch(error) => match error.context() {
                Some(context) => Some(context.diagnostic_context()),
                None => None,
            },
            Self::Match(error) => match error.context() {
                Some(context) => Some(context.diagnostic_context()),
                None => None,
            },
            Self::Budget(error) => error.context(),
            Self::Internal { error, .. } => Some(error.context()),
            Self::Located { error, .. } => match error {
                RegexExecutionFailure::Internal(error) => Some(error.context()),
                RegexExecutionFailure::Budget(error) => error.context(),
                RegexExecutionFailure::Scratch(error) => match error.context() {
                    Some(context) => Some(context.diagnostic_context()),
                    None => None,
                },
                RegexExecutionFailure::Match(error) => match error.context() {
                    Some(context) => Some(context.diagnostic_context()),
                    None => None,
                },
            },
        }
    }

    pub const fn regex_context(self) -> Option<RegexErrorContext> {
        match self {
            Self::Scratch(error) => error.context(),
            Self::Match(error) => error.context(),
            Self::Located { error, .. } => match error {
                RegexExecutionFailure::Scratch(error) => error.context(),
                RegexExecutionFailure::Match(error) => error.context(),
                _ => None,
            },
            _ => None,
        }
    }

    pub const fn source_span(self) -> Option<SourceSpan> {
        match self {
            Self::Internal { source_span, .. } => source_span,
            Self::Located { source_span, .. } => Some(source_span),
            _ => None,
        }
    }

    pub const fn with_source_span(self, source_span: SourceSpan) -> Self {
        let error = match self {
            Self::Scratch(error) => RegexExecutionFailure::Scratch(error),
            Self::Match(error) => RegexExecutionFailure::Match(error),
            Self::Budget(error) => RegexExecutionFailure::Budget(error),
            Self::Internal { error, .. } => RegexExecutionFailure::Internal(error),
            Self::Located { error, .. } => error,
        };
        Self::Located { error, source_span }
    }
}

/// Both phases share one growable workspace, avoiding reallocation for smaller programs.
#[derive(Default)]
pub struct RegexExecutor {
    scratch: Option<RegexScratch>,
    capacity: usize,
    budget: Option<RegexExecutionBudget>,
    budget_errors: ErrSlot<BudgetError>,
    scratch_errors: ErrSlot<RegexScratchError>,
    match_errors: ErrSlot<RegexMatchError>,
}

struct MatchRequest<'a> {
    regex: &'a Regex,
    value: Option<&'a str>,
    limits: &'a RegexLimits,
    budget: &'a RegexExecutionBudget,
    source_span: Option<SourceSpan>,
}

fn with_source_span(
    error: RegexExecutionError,
    source_span: Option<SourceSpan>,
) -> RegexExecutionError {
    match source_span {
        Some(source_span) => error.with_source_span(source_span),
        None => error,
    }
}

impl RegexExecutor {
    pub const fn new() -> Self {
        Self {
            scratch: None,
            capacity: 0,
            budget: None,
            budget_errors: ErrSlot::new(),
            scratch_errors: ErrSlot::new(),
            match_errors: ErrSlot::new(),
        }
    }

    pub fn is_match_budgeted<'slot>(
        &mut self,
        regex: &Regex,
        options: RegexMatchOptions<'_>,
        mut out: Out<'slot, '_, RegexExecutionError>,
    ) -> OResult<'slot, bool> {
        self.is_match_inner(
            MatchRequest {
                regex,
                value: options.value,
                limits: options.limits,
                budget: options.budget,
                source_span: None,
            },
            out.reborrow(),
        )
    }

    pub fn is_match_constraint<'slot>(
        &mut self,
        constraint: &CompiledRegexConstraint,
        options: RegexMatchOptions<'_>,
        mut out: Out<'slot, '_, RegexExecutionError>,
    ) -> OResult<'slot, bool> {
        self.is_match_inner(
            MatchRequest {
                regex: constraint.regex(),
                value: options.value,
                limits: options.limits,
                budget: options.budget,
                source_span: Some(constraint.span()),
            },
            out.reborrow(),
        )
    }

    fn is_match_inner<'slot>(
        &mut self,
        request: MatchRequest<'_>,
        out: Out<'slot, '_, RegexExecutionError>,
    ) -> OResult<'slot, bool> {
        let same_call = self
            .budget
            .as_ref()
            .is_some_and(|old| old.same_call(request.budget));
        if !same_call {
            self.scratch = None;
            self.capacity = 0;
            self.budget = None;
        }
        let scratch_limit = request.limits.max_regex_scratch_states();
        let cached_scratch_exceeds_limit = self
            .scratch
            .as_ref()
            .is_some_and(|scratch| scratch.allocation_ceiling() > scratch_limit);
        if self.capacity > scratch_limit || cached_scratch_exceeds_limit {
            self.scratch = None;
            self.capacity = 0;
        }
        if !same_call {
            self.budget = match self
                .budget_errors
                .try_call(|budget_out| request.budget.try_clone(budget_out))
            {
                Ok(budget) => Some(budget),
                Err(caught) => {
                    return Err(out.fail(with_source_span(
                        RegexExecutionError::Budget(caught.take()),
                        request.source_span,
                    )));
                }
            };
        }
        let Some(value) = request.value else {
            return Ok(true);
        };
        let required = request.regex.scratch_state_count();
        if required > request.limits.max_regex_scratch_states() {
            return Err(out.fail(with_source_span(
                RegexExecutionError::Scratch(RegexScratchError::ResourceLimit(
                    ResourceLimit::RegexScratchStates,
                )),
                request.source_span,
            )));
        }
        if self.capacity < required {
            if let Some(scratch) = self.scratch.as_mut() {
                if let Err(caught) = self.scratch_errors.try_call(|scratch_out| {
                    scratch.grow(request.regex, request.limits, request.budget, scratch_out)
                }) {
                    return Err(out.fail(with_source_span(
                        RegexExecutionError::Scratch(caught.take()),
                        request.source_span,
                    )));
                }
                self.capacity = required;
            } else {
                let scratch = match self.scratch_errors.try_call(|scratch_out| {
                    RegexScratch::new(request.regex, request.limits, request.budget, scratch_out)
                }) {
                    Ok(scratch) => scratch,
                    Err(caught) => {
                        return Err(out.fail(with_source_span(
                            RegexExecutionError::Scratch(caught.take()),
                            request.source_span,
                        )));
                    }
                };
                self.scratch = Some(scratch);
                self.capacity = required;
            }
        }
        let Some(scratch) = self.scratch.as_mut() else {
            return Err(out.fail(with_source_span(
                RegexExecutionError::Internal {
                    error: InternalError::RegexExecutionInvariantFailure(REGEX_EXECUTOR_INTERNAL),
                    source_span: None,
                },
                request.source_span,
            )));
        };
        let result = self.match_errors.try_call(|match_out| {
            request
                .regex
                .is_match(value, scratch, request.budget, match_out)
        });
        match result {
            Ok(matched) => Ok(matched),
            Err(caught) => Err(out.fail(with_source_span(
                RegexExecutionError::Match(caught.take()),
                request.source_span,
            ))),
        }
    }
}
