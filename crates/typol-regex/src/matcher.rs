use crate::{
    Regex, RegexErrorContext, RegexMatchError, RegexResource, RegexScratchError,
    compiler::{Instruction, StateId},
    error::{
        MATCH_ACTIVE_STATE_MISSING, MATCH_CURRENT_STACK_CAPACITY_EXCEEDED,
        MATCH_CURRENT_STATE_OUT_OF_RANGE, MATCH_NEXT_SEEN_SLOT_MISSING,
        MATCH_NEXT_STACK_CAPACITY_EXCEEDED, MATCH_NEXT_STATE_OUT_OF_RANGE,
        MATCH_VISITED_SLOT_MISSING, SCRATCH_MARK_BUFFER_GROWTH, SCRATCH_MARK_BUFFER_INITIALIZATION,
        SCRATCH_STATE_BUFFER_GROWTH, SCRATCH_STATE_BUFFER_INITIALIZATION,
    },
};
use core::{mem::swap, str::Chars};
use opeo::{ErrSlot, OResult, Out, ResultOutExt};
use typol_core::{BudgetError, ChargedVec, DiagnosticContext, ExecutionBudget};
use typol_policy::RegexLimits;

/// A reusable, bounded Thompson NFA search workspace.
pub struct RegexScratch {
    capacity: usize,
    allocation_ceiling: usize,
    active: ChargedVec<StateId>,
    next: ChargedVec<StateId>,
    stack: ChargedVec<StateId>,
    visited: ChargedVec<bool>,
    next_seen: ChargedVec<bool>,
    budget_errors: ErrSlot<BudgetError>,
}

/// Injects a start state at each Unicode scalar boundary and charges for each instruction processed.
pub(crate) struct RegexMatcher;

impl RegexMatcher {
    pub(crate) fn is_match<'slot>(
        regex: &Regex,
        input: &str,
        scratch: &mut RegexScratch,
        limits: (usize, Option<&ExecutionBudget>),
        mut out: Out<'slot, '_, RegexMatchError>,
    ) -> OResult<'slot, bool> {
        let (max_steps, budget) = limits;
        if regex.state_count() > scratch.capacity() {
            return Err(out.fail(RegexMatchError::ResourceLimitExceeded(
                RegexResource::ScratchStateCount,
            )));
        }

        let initial_steps = scratch
            .capacity()
            .checked_mul(2)
            .ok_or(RegexMatchError::ResourceLimitExceeded(
                RegexResource::SearchSteps,
            ))
            .store_err_into(out.reborrow())?;
        consume(budget, initial_steps, out.reborrow())?;
        scratch.reset();
        let mut input_chars = input.chars();
        let mut current = next_scalar(&mut input_chars, budget, out.reborrow())?;
        let mut at_start = true;
        let mut steps = 0usize;

        let matched = 'search: loop {
            scratch.stack.clear();
            consume(budget, scratch.visited.len(), out.reborrow())?;
            scratch.visited.fill(false);

            let active_len = scratch.active.len();
            for index in 0..active_len {
                consume(budget, 1, out.reborrow())?;
                let state = scratch
                    .active
                    .get(index)
                    .copied()
                    .ok_or(RegexMatchError::InternalInvariantFailure(
                        RegexErrorContext::new(MATCH_ACTIVE_STATE_MISSING),
                    ))
                    .store_err_into(out.reborrow())?;
                enqueue(scratch, state, out.reborrow())?;
            }
            enqueue(scratch, regex.start(), out.reborrow())?;

            while let Some(state) = scratch.stack.pop() {
                if steps >= max_steps {
                    return Err(out.fail(RegexMatchError::ResourceLimitExceeded(
                        RegexResource::SearchSteps,
                    )));
                }
                consume(budget, 1, out.reborrow())?;
                steps = steps
                    .checked_add(1)
                    .ok_or(RegexMatchError::ResourceLimitExceeded(
                        RegexResource::SearchSteps,
                    ))
                    .store_err_into(out.reborrow())?;

                match regex.instruction(state, out.reborrow())? {
                    Instruction::Consume { class, next } => {
                        if match current {
                            Some(scalar) => class.matches(scalar, budget, out.reborrow())?,
                            None => false,
                        } {
                            enqueue_next(scratch, *next, out.reborrow())?;
                        }
                    }
                    Instruction::Split { first, second } => {
                        enqueue(scratch, *second, out.reborrow())?;
                        enqueue(scratch, *first, out.reborrow())?;
                    }
                    Instruction::Jump { next } => enqueue(scratch, *next, out.reborrow())?,
                    Instruction::AssertStart { next } => {
                        if at_start {
                            enqueue(scratch, *next, out.reborrow())?;
                        }
                    }
                    Instruction::AssertEnd { next } => {
                        if current.is_none() {
                            enqueue(scratch, *next, out.reborrow())?;
                        }
                    }
                    Instruction::Match => break 'search true,
                }
            }

            if current.is_none() {
                break 'search false;
            }

            current = next_scalar(&mut input_chars, budget, out.reborrow())?;
            at_start = false;
            swap(&mut scratch.active, &mut scratch.next);
            scratch.next.clear();
            consume(budget, scratch.next_seen.len(), out.reborrow())?;
            scratch.next_seen.fill(false);
        };
        Ok(matched)
    }

    pub(crate) fn match_budget_error(error: BudgetError) -> RegexMatchError {
        error.into()
    }
}

fn enqueue<'slot>(
    scratch: &mut RegexScratch,
    state: StateId,
    mut out: Out<'slot, '_, RegexMatchError>,
) -> OResult<'slot, ()> {
    let index = state.index();
    let Some(visited) = scratch.visited.get(index).copied() else {
        return Err(out.fail(RegexMatchError::InternalInvariantFailure(
            RegexErrorContext::new(MATCH_CURRENT_STATE_OUT_OF_RANGE).with_object_id(index as u64),
        )));
    };
    if visited {
        return Ok(());
    }
    if scratch.stack.len() >= scratch.capacity || scratch.stack.len() >= scratch.stack.capacity() {
        return Err(out.fail(RegexMatchError::InternalInvariantFailure(
            RegexErrorContext::new(MATCH_CURRENT_STACK_CAPACITY_EXCEEDED),
        )));
    }
    let Some(visited) = scratch.visited.get_mut(index) else {
        return Err(out.fail(RegexMatchError::InternalInvariantFailure(
            RegexErrorContext::new(MATCH_VISITED_SLOT_MISSING).with_object_id(index as u64),
        )));
    };
    *visited = true;
    let stack = &mut scratch.stack;
    let errors = &mut scratch.budget_errors;
    budget_operation(
        errors,
        out.reborrow(),
        RegexMatcher::match_budget_error,
        |out| stack.try_push(state, out),
    )?;
    Ok(())
}

fn enqueue_next<'slot>(
    scratch: &mut RegexScratch,
    state: StateId,
    mut out: Out<'slot, '_, RegexMatchError>,
) -> OResult<'slot, ()> {
    let index = state.index();
    let Some(seen) = scratch.next_seen.get(index).copied() else {
        return Err(out.fail(RegexMatchError::InternalInvariantFailure(
            RegexErrorContext::new(MATCH_NEXT_STATE_OUT_OF_RANGE).with_object_id(index as u64),
        )));
    };
    if seen {
        return Ok(());
    }
    if scratch.next.len() >= scratch.capacity || scratch.next.len() >= scratch.next.capacity() {
        return Err(out.fail(RegexMatchError::InternalInvariantFailure(
            RegexErrorContext::new(MATCH_NEXT_STACK_CAPACITY_EXCEEDED),
        )));
    }
    let Some(seen) = scratch.next_seen.get_mut(index) else {
        return Err(out.fail(RegexMatchError::InternalInvariantFailure(
            RegexErrorContext::new(MATCH_NEXT_SEEN_SLOT_MISSING).with_object_id(index as u64),
        )));
    };
    *seen = true;
    let next = &mut scratch.next;
    let errors = &mut scratch.budget_errors;
    budget_operation(
        errors,
        out.reborrow(),
        RegexMatcher::match_budget_error,
        |out| next.try_push(state, out),
    )?;
    Ok(())
}

fn consume<'slot>(
    budget: Option<&ExecutionBudget>,
    steps: usize,
    mut out: Out<'slot, '_, RegexMatchError>,
) -> OResult<'slot, ()> {
    if let Some(budget) = budget {
        budget.consume_with(steps, out.reborrow(), RegexMatcher::match_budget_error)?;
    }
    Ok(())
}

fn next_scalar<'slot>(
    chars: &mut Chars<'_>,
    budget: Option<&ExecutionBudget>,
    mut out: Out<'slot, '_, RegexMatchError>,
) -> OResult<'slot, Option<char>> {
    let Some(first) = chars.as_str().as_bytes().first() else {
        return Ok(None);
    };
    consume(budget, 1, out.reborrow())?;
    let width = if *first < 0x80 {
        1
    } else if *first < 0xe0 {
        2
    } else if *first < 0xf0 {
        3
    } else {
        4
    };
    consume(budget, width - 1, out.reborrow())?;
    Ok(chars.next())
}

impl RegexScratch {
    /// Creates a workspace sized for the regex states; no buffers grow during a search.
    pub fn new<'slot, 'borrow>(
        regex: &Regex,
        limits: &RegexLimits,
        out: Out<'slot, 'borrow, RegexScratchError>,
    ) -> OResult<'slot, Self> {
        Self::new_budgeted(regex, limits, None, out)
    }

    pub fn new_budgeted<'slot>(
        regex: &Regex,
        limits: &RegexLimits,
        budget: Option<&ExecutionBudget>,
        mut out: Out<'slot, '_, RegexScratchError>,
    ) -> OResult<'slot, Self> {
        let capacity = regex.scratch_state_count();
        if capacity > limits.max_regex_scratch_states() {
            return Err(out.fail(RegexScratchError::ResourceLimitExceeded(
                RegexResource::ScratchStateCount,
            )));
        }

        let mut budget_errors = ErrSlot::<BudgetError>::new();
        let active = state_buffer(capacity, budget, &mut budget_errors, out.reborrow())?;
        let next = state_buffer(capacity, budget, &mut budget_errors, out.reborrow())?;
        let stack = state_buffer(capacity, budget, &mut budget_errors, out.reborrow())?;
        let visited = mark_buffer(capacity, budget, &mut budget_errors, out.reborrow())?;
        let next_seen = mark_buffer(capacity, budget, &mut budget_errors, out.reborrow())?;
        Ok(Self {
            capacity,
            allocation_ceiling: capacity,
            active,
            next,
            stack,
            visited,
            next_seen,
            budget_errors,
        })
    }

    pub(crate) fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn allocation_ceiling(&self) -> usize {
        self.allocation_ceiling
    }

    /// Grows existing buffers under the execution budget before matching starts.
    pub fn grow<'slot>(
        &mut self,
        regex: &Regex,
        limits: &RegexLimits,
        mut out: Out<'slot, '_, RegexScratchError>,
    ) -> OResult<'slot, ()> {
        let required = regex.scratch_state_count();
        if required > limits.max_regex_scratch_states() {
            return Err(out.fail(RegexScratchError::ResourceLimitExceeded(
                RegexResource::ScratchStateCount,
            )));
        }
        if required <= self.capacity {
            return Ok(());
        }

        grow_state_buffer(
            &mut self.active,
            required,
            &mut self.budget_errors,
            out.reborrow(),
        )?;
        self.allocation_ceiling = self.allocation_ceiling.max(required);
        grow_state_buffer(
            &mut self.next,
            required,
            &mut self.budget_errors,
            out.reborrow(),
        )?;
        self.allocation_ceiling = self.allocation_ceiling.max(required);
        grow_state_buffer(
            &mut self.stack,
            required,
            &mut self.budget_errors,
            out.reborrow(),
        )?;
        self.allocation_ceiling = self.allocation_ceiling.max(required);
        grow_mark_buffer(
            &mut self.visited,
            required,
            &mut self.budget_errors,
            out.reborrow(),
        )?;
        self.allocation_ceiling = self.allocation_ceiling.max(required);
        grow_mark_buffer(
            &mut self.next_seen,
            required,
            &mut self.budget_errors,
            out.reborrow(),
        )?;
        self.allocation_ceiling = self.allocation_ceiling.max(required);
        self.capacity = required;
        Ok(())
    }

    pub(crate) fn reset(&mut self) {
        self.active.clear();
        self.next.clear();
        self.stack.clear();
        self.visited.fill(false);
        self.next_seen.fill(false);
    }

    pub(crate) fn scratch_error(
        error: BudgetError,
        context: DiagnosticContext,
    ) -> RegexScratchError {
        match error {
            BudgetError::Allocation => RegexScratchError::AllocationFailed,
            BudgetError::Resource => {
                RegexScratchError::ResourceLimitExceeded(RegexResource::ScratchStateCount)
            }
            BudgetError::CapacityExceeded => RegexScratchError::CapacityExceeded,
            BudgetError::InvariantViolation(source_context) => {
                RegexScratchError::InternalInvariantFailure(
                    RegexErrorContext::new(context).with_source_context(*source_context),
                )
            }
        }
    }
}

fn grow_state_buffer<'slot>(
    buffer: &mut ChargedVec<StateId>,
    capacity: usize,
    errors: &mut ErrSlot<BudgetError>,
    mut out: Out<'slot, '_, RegexScratchError>,
) -> OResult<'slot, ()> {
    let additional = capacity.saturating_sub(buffer.len());
    budget_operation(
        errors,
        out.reborrow(),
        |error| RegexScratch::scratch_error(error, SCRATCH_STATE_BUFFER_GROWTH),
        |out| buffer.try_reserve_exact(additional, out),
    )
}

fn grow_mark_buffer<'slot>(
    buffer: &mut ChargedVec<bool>,
    capacity: usize,
    errors: &mut ErrSlot<BudgetError>,
    mut out: Out<'slot, '_, RegexScratchError>,
) -> OResult<'slot, ()> {
    budget_operation(
        errors,
        out.reborrow(),
        |error| RegexScratch::scratch_error(error, SCRATCH_MARK_BUFFER_GROWTH),
        |out| buffer.resize(capacity, false, out),
    )
}

fn state_buffer<'slot>(
    capacity: usize,
    budget: Option<&ExecutionBudget>,
    errors: &mut ErrSlot<BudgetError>,
    mut out: Out<'slot, '_, RegexScratchError>,
) -> OResult<'slot, ChargedVec<StateId>> {
    let mut buffer = budget_operation(
        errors,
        out.reborrow(),
        |error| RegexScratch::scratch_error(error, SCRATCH_STATE_BUFFER_INITIALIZATION),
        |out| ChargedVec::new(budget, out),
    )?;
    budget_operation(
        errors,
        out.reborrow(),
        |error| RegexScratch::scratch_error(error, SCRATCH_STATE_BUFFER_INITIALIZATION),
        |out| buffer.try_reserve_exact(capacity, out),
    )?;
    Ok(buffer)
}

fn mark_buffer<'slot>(
    capacity: usize,
    budget: Option<&ExecutionBudget>,
    errors: &mut ErrSlot<BudgetError>,
    mut out: Out<'slot, '_, RegexScratchError>,
) -> OResult<'slot, ChargedVec<bool>> {
    let mut buffer = budget_operation(
        errors,
        out.reborrow(),
        |error| RegexScratch::scratch_error(error, SCRATCH_MARK_BUFFER_INITIALIZATION),
        |out| ChargedVec::new(budget, out),
    )?;
    budget_operation(
        errors,
        out.reborrow(),
        |error| RegexScratch::scratch_error(error, SCRATCH_MARK_BUFFER_INITIALIZATION),
        |out| buffer.try_reserve_exact(capacity, out),
    )?;
    budget_operation(
        errors,
        out.reborrow(),
        |error| RegexScratch::scratch_error(error, SCRATCH_MARK_BUFFER_INITIALIZATION),
        |out| buffer.resize(capacity, false, out),
    )?;
    Ok(buffer)
}

fn budget_operation<'slot, 'borrow, T, E>(
    errors: &mut ErrSlot<BudgetError>,
    out: Out<'slot, 'borrow, E>,
    convert: impl FnOnce(BudgetError) -> E,
    operation: impl for<'inner> FnOnce(Out<'inner, 'inner, BudgetError>) -> OResult<'inner, T>,
) -> OResult<'slot, T> {
    match errors.try_call(operation) {
        Ok(value) => Ok(value),
        Err(error) => Err(out.fail(convert(error.take()))),
    }
}

#[cfg(test)]
mod tests {
    use alloc::{format, string::String};
    use core::fmt::Debug;
    use opeo::{ErrSlot, OResult, Out};

    use typol_core::{
        BudgetError, DiagnosticContext, ExecutionBudget, ExecutionLimits, diagnostic_context,
    };
    use typol_policy::ResourceLimits;

    use crate::{
        RegexCompileError, RegexCompiler, RegexMatchError, RegexScratch, RegexScratchError,
    };

    use super::{RegexMatcher, SCRATCH_MARK_BUFFER_INITIALIZATION};

    static SOURCE_CONTEXT: DiagnosticContext =
        diagnostic_context!("typol-core", "budget-storage", "reserved-push",);

    #[test]
    fn budget_invariant_context_keeps_compile_and_scratch_stages() -> Result<(), String> {
        let compile_error = RegexCompileError::from(BudgetError::invariant(&SOURCE_CONTEXT));
        let compile_context = require(
            compile_error.context(),
            "regex compile error is missing its context",
        )?;
        check_eq(
            compile_context.diagnostic_context().subsystem(),
            "typol-regex",
            "regex subsystem identifier does not match",
        )?;
        check_eq(
            compile_context.diagnostic_context().stage(),
            "regex-compile",
            "regex compile stage identifier does not match",
        )?;
        check_eq(
            compile_context.diagnostic_context().failure_point(),
            "budget-error-conversion",
            "regex compile failure point does not match",
        )?;
        let compile_source = require(
            compile_context.source_context(),
            "budget error source context was lost",
        )?;
        check_eq(
            compile_source.failure_point(),
            "reserved-push",
            "budget source failure point does not match",
        )?;

        let scratch_error = RegexScratch::scratch_error(
            BudgetError::invariant(&SOURCE_CONTEXT),
            SCRATCH_MARK_BUFFER_INITIALIZATION,
        );
        let scratch_context = require(
            scratch_error.context(),
            "scratch error is missing its context",
        )?;
        check_eq(
            scratch_context.diagnostic_context().stage(),
            "regex-scratch-init",
            "scratch stage identifier does not match",
        )?;
        check_eq(
            scratch_context.diagnostic_context().failure_point(),
            "mark-buffer-initialization",
            "scratch initialization failure point does not match",
        )?;
        let scratch_source = require(
            scratch_context.source_context(),
            "scratch budget source context was lost",
        )?;
        check_eq(
            scratch_source.failure_point(),
            "reserved-push",
            "scratch budget source failure point does not match",
        )?;
        check(
            matches!(
                scratch_error,
                RegexScratchError::InternalInvariantFailure(_)
            ),
            "scratch internal error category does not match",
        )
    }

    #[test]
    fn match_invariant_is_returned_as_an_error_with_context() -> Result<(), String> {
        let limits = ResourceLimits::default();
        let mut budget_errors = ErrSlot::<BudgetError>::new();
        let mut compile_errors = ErrSlot::<RegexCompileError>::new();
        let mut scratch_errors = ErrSlot::<RegexScratchError>::new();
        let mut match_errors = ErrSlot::<RegexMatchError>::new();
        let execution = succeed(
            attempt(&mut budget_errors, |out| {
                ExecutionBudget::new(ExecutionLimits::new(100_000, 1_000_000), out)
            }),
            "failed to create execution budget",
        )?;
        let regex = succeed(
            attempt(&mut compile_errors, |out| {
                RegexCompiler::compile("a", &limits, &execution, out)
            }),
            "regex compilation failed",
        )?;
        let mut scratch = succeed(
            attempt(&mut scratch_errors, |out| {
                RegexScratch::new(&regex, &limits, out)
            }),
            "failed to create scratch space",
        )?;
        scratch.visited.clear();

        let result = attempt(&mut match_errors, |out| {
            regex.is_match("sensitive-subject", &mut scratch, 100, out)
        });
        let context = match result {
            Attempt::Failure(RegexMatchError::InternalInvariantFailure(context)) => context,
            other => {
                return Err(format!(
                    "match invariant failure was not returned as an error: {other:?}"
                ));
            }
        };
        check_eq(
            context.diagnostic_context().stage(),
            "regex-match",
            "match stage identifier does not match",
        )?;
        check_eq(
            context.diagnostic_context().failure_point(),
            "current-state-out-of-range",
            "match failure point does not match",
        )?;
        check_eq(
            context.diagnostic_context().object_id(),
            Some(0),
            "match state index was not preserved",
        )?;

        let budget_error =
            RegexMatcher::match_budget_error(BudgetError::invariant(&SOURCE_CONTEXT));
        let budget_context = require(
            budget_error.context(),
            "match budget error is missing its context",
        )?;
        check_eq(
            budget_context.diagnostic_context().stage(),
            "regex-match",
            "budget error match stage identifier does not match",
        )?;
        check_eq(
            budget_context.diagnostic_context().failure_point(),
            "budget-error-conversion",
            "budget error match failure point does not match",
        )?;
        let match_source = require(
            budget_context.source_context(),
            "match budget source context was lost",
        )?;
        check_eq(
            match_source.failure_point(),
            "reserved-push",
            "match budget source failure point does not match",
        )
    }

    fn check(condition: bool, message: &str) -> Result<(), String> {
        if condition {
            Ok(())
        } else {
            Err(message.into())
        }
    }

    fn check_eq<T>(actual: T, expected: T, message: &str) -> Result<(), String>
    where
        T: Debug + PartialEq,
    {
        if actual == expected {
            Ok(())
        } else {
            Err(format!(
                "{message}: actual value {actual:?}, expected {expected:?}"
            ))
        }
    }

    fn require<T>(value: Option<T>, message: &str) -> Result<T, String> {
        value.ok_or_else(|| message.into())
    }

    #[derive(Debug)]
    enum Attempt<T, E> {
        Success(T),
        Failure(E),
    }

    fn attempt<T, E>(
        errors: &mut ErrSlot<E>,
        operation: impl for<'slot> FnOnce(Out<'slot, 'slot, E>) -> OResult<'slot, T>,
    ) -> Attempt<T, E> {
        match errors.try_call(operation) {
            Ok(value) => Attempt::Success(value),
            Err(error) => Attempt::Failure(error.take()),
        }
    }

    fn succeed<T, E: Debug>(attempt: Attempt<T, E>, message: &str) -> Result<T, String> {
        match attempt {
            Attempt::Success(value) => Ok(value),
            Attempt::Failure(error) => Err(format!("{message}: {error:?}")),
        }
    }
}
