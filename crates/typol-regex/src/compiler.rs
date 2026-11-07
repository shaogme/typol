use opeo::{OResult, Out, ResultOutExt};
use typol_core::{ChargedVec, RegexExecutionBudget};
use typol_policy::RegexLimits;

use crate::{
    RegexCompileError, RegexErrorContext, RegexMatchError, RegexResource, RegexScratch,
    char_class::CharClass,
    error::{
        COMPILER_FINAL_FRAGMENT_MISSING, COMPILER_PATCH_INSTRUCTION_MISSING,
        COMPILER_PATCH_KIND_MISMATCH, COMPILER_PROGRAM_START_OUT_OF_RANGE,
        COMPILER_PROGRAM_TARGET_OUT_OF_RANGE, MATCH_INSTRUCTION_OUT_OF_RANGE,
    },
    matcher::RegexMatcher,
    parser::{Ast, NodeId, NodeKind, Parser, Repetition},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct StateId(usize);

impl StateId {
    fn new(index: usize) -> Self {
        Self(index)
    }

    pub(crate) fn index(self) -> usize {
        self.0
    }
}

pub(crate) const UNPATCHED: StateId = StateId(usize::MAX);

pub(crate) enum Instruction {
    Consume { class: CharClass, next: StateId },
    Split { first: StateId, second: StateId },
    Jump { next: StateId },
    AssertStart { next: StateId },
    AssertEnd { next: StateId },
    Match,
}

#[derive(Clone, Copy)]
enum PatchField {
    ConsumeNext,
    SplitSecond,
    JumpNext,
    AssertStartNext,
    AssertEndNext,
}

impl Instruction {
    fn patch(&mut self, field: PatchField, target: StateId) -> bool {
        match field {
            PatchField::ConsumeNext => match self {
                Self::Consume { next, .. } => *next = target,
                _ => return false,
            },
            PatchField::SplitSecond => match self {
                Self::Split { second, .. } => *second = target,
                _ => return false,
            },
            PatchField::JumpNext => match self {
                Self::Jump { next } => *next = target,
                _ => return false,
            },
            PatchField::AssertStartNext => match self {
                Self::AssertStart { next } => *next = target,
                _ => return false,
            },
            PatchField::AssertEndNext => match self {
                Self::AssertEnd { next } => *next = target,
                _ => return false,
            },
        }
        true
    }
}

#[derive(Clone, Copy)]
struct Patch {
    instruction: StateId,
    field: PatchField,
}

struct Fragment {
    start: StateId,
    outs: ChargedVec<Patch>,
}

struct CompileFrame {
    node: NodeId,
    next: usize,
    result: Option<Fragment>,
}

struct Compiler<'a> {
    ast: &'a Ast,
    instructions: ChargedVec<Instruction>,
    range_count: usize,
    max_instructions: usize,
    max_ranges: usize,
    execution: &'a RegexExecutionBudget,
}

/// An immutable Thompson NFA produced by compilation.
pub struct Regex {
    instructions: ChargedVec<Instruction>,
    start: StateId,
    range_count: usize,
}

/// Compiles a decoded pattern into an owned Thompson NFA.
pub struct RegexCompiler;

/// Per-pattern and caller-provided aggregate limits for regex compilation.
#[derive(Clone, Copy)]
pub struct RegexCompileOptions<'a> {
    limits: &'a RegexLimits,
    execution: &'a RegexExecutionBudget,
    max_instructions: usize,
    max_ranges: usize,
}

impl<'a> RegexCompileOptions<'a> {
    pub const fn new(
        limits: &'a RegexLimits,
        execution: &'a RegexExecutionBudget,
        max_instructions: usize,
        max_ranges: usize,
    ) -> Self {
        Self {
            limits,
            execution,
            max_instructions,
            max_ranges,
        }
    }
}

impl RegexCompiler {
    pub fn compile<'slot>(
        pattern: &str,
        limits: &RegexLimits,
        execution: &RegexExecutionBudget,
        mut out: Out<'slot, '_, RegexCompileError>,
    ) -> OResult<'slot, Regex> {
        Self::compile_with_options(
            pattern,
            RegexCompileOptions::new(
                limits,
                execution,
                limits.max_regex_instructions_per_pattern(),
                limits.max_regex_ranges_per_pattern(),
            ),
            out.reborrow(),
        )
    }

    pub fn compile_with_options<'slot>(
        pattern: &str,
        options: RegexCompileOptions<'_>,
        mut out: Out<'slot, '_, RegexCompileError>,
    ) -> OResult<'slot, Regex> {
        let RegexCompileOptions {
            limits,
            execution,
            max_instructions,
            max_ranges,
        } = options;
        let max_instructions = max_instructions.min(limits.max_regex_instructions_per_pattern());
        let max_ranges = max_ranges.min(limits.max_regex_ranges_per_pattern());
        let ast =
            Parser::parse_with_range_limit(pattern, limits, execution, max_ranges, out.reborrow())?;
        let mut compiler = Compiler {
            ast: &ast,
            instructions: ChargedVec::new_regex(Some(execution), out.reborrow())?,
            range_count: 0,
            max_instructions,
            max_ranges,
            execution,
        };
        let fragment = compiler.compile_node(ast.root(), out.reborrow())?;
        let match_state = compiler.emit(Instruction::Match, out.reborrow())?;
        compiler.patch(&fragment.outs, match_state, out.reborrow())?;

        compiler.program_is_valid(fragment.start, out.reborrow())?;

        Ok(Regex::new(
            compiler.instructions,
            fragment.start,
            compiler.range_count,
        ))
    }
}

impl Compiler<'_> {
    fn compile_node<'slot>(
        &mut self,
        root: NodeId,
        mut out: Out<'slot, '_, RegexCompileError>,
    ) -> OResult<'slot, Fragment> {
        let mut frames = ChargedVec::new_regex(Some(self.execution), out.reborrow())?;
        frames.try_push(
            CompileFrame {
                node: root,
                next: 0,
                result: None,
            },
            out.reborrow(),
        )?;
        let mut completed = None;
        while let Some(mut frame) = frames.pop() {
            self.execution.consume(1, out.reborrow())?;
            let node = self.ast.node(frame.node, out.reborrow())?;
            let kind = &node.kind;
            if let Some(child) = completed.take() {
                let child = match kind {
                    NodeKind::Repetition { kind, .. } => match kind {
                        Repetition::ZeroOrMore => self.zero_or_more(child, out.reborrow())?,
                        Repetition::OneOrMore => self.one_or_more(child, out.reborrow())?,
                        Repetition::ZeroOrOne => self.zero_or_one(child, out.reborrow())?,
                        Repetition::Bounded { min, max } if frame.next > usize::from(*min) => {
                            if max.is_none() {
                                self.zero_or_more(child, out.reborrow())?
                            } else {
                                self.zero_or_one(child, out.reborrow())?
                            }
                        }
                        Repetition::Bounded { .. } => child,
                    },
                    _ => child,
                };
                frame.result = Some(match frame.result.take() {
                    Some(previous) if matches!(kind, NodeKind::Alternation(_)) => {
                        self.alternate(previous, child, out.reborrow())?
                    }
                    Some(previous) => self.concatenate(previous, child, out.reborrow())?,
                    None => child,
                });
            }
            let next = match kind {
                NodeKind::Group(inner) if frame.next == 0 => Some(*inner),
                NodeKind::Concatenation(children) | NodeKind::Alternation(children) => {
                    children.get(frame.next).copied()
                }
                NodeKind::Repetition { node, kind } => {
                    let count = match kind {
                        Repetition::Bounded { min, max } => {
                            max.map_or(usize::from(*min) + 1, usize::from)
                        }
                        _ => 1,
                    };
                    (frame.next < count).then_some(*node)
                }
                _ => None,
            };
            if let Some(next) = next {
                frame.next += 1;
                frames.try_push(frame, out.reborrow())?;
                frames.try_push(
                    CompileFrame {
                        node: next,
                        next: 0,
                        result: None,
                    },
                    out.reborrow(),
                )?;
                continue;
            }
            completed = Some(match frame.result {
                Some(fragment) => fragment,
                None => match kind {
                    NodeKind::Literal(value) => {
                        let class = CharClass::singleton(*value, self.execution, out.reborrow())?;
                        self.consume(class, out.reborrow())?
                    }
                    NodeKind::CharacterClass(class) => {
                        let class = class.try_clone(out.reborrow())?;
                        self.consume(class, out.reborrow())?
                    }
                    NodeKind::Start => {
                        let instruction = self
                            .emit(Instruction::AssertStart { next: UNPATCHED }, out.reborrow())?;
                        self.fragment(instruction, PatchField::AssertStartNext, out.reborrow())?
                    }
                    NodeKind::End => {
                        let instruction =
                            self.emit(Instruction::AssertEnd { next: UNPATCHED }, out.reborrow())?;
                        self.fragment(instruction, PatchField::AssertEndNext, out.reborrow())?
                    }
                    _ => self.epsilon(out.reborrow())?,
                },
            });
        }
        completed
            .ok_or(RegexCompileError::InternalInvariantFailure(
                RegexErrorContext::new(COMPILER_FINAL_FRAGMENT_MISSING),
            ))
            .store_err_into(out.reborrow())
    }

    fn consume<'slot>(
        &mut self,
        class: CharClass,
        mut out: Out<'slot, '_, RegexCompileError>,
    ) -> OResult<'slot, Fragment> {
        let added_ranges = class.range_count();
        let total_ranges = self
            .range_count
            .checked_add(added_ranges)
            .ok_or(RegexCompileError::ResourceLimitExceeded(
                RegexResource::CharacterRangeCount,
            ))
            .store_err_into(out.reborrow())?;
        if total_ranges > self.max_ranges {
            return Err(out.fail(RegexCompileError::ResourceLimitExceeded(
                RegexResource::CharacterRangeCount,
            )));
        }

        let instruction = self.emit(
            Instruction::Consume {
                class,
                next: UNPATCHED,
            },
            out.reborrow(),
        )?;
        self.range_count = total_ranges;
        self.fragment(instruction, PatchField::ConsumeNext, out.reborrow())
    }

    fn epsilon<'slot>(
        &mut self,
        mut out: Out<'slot, '_, RegexCompileError>,
    ) -> OResult<'slot, Fragment> {
        let instruction = self.emit(Instruction::Jump { next: UNPATCHED }, out.reborrow())?;
        self.fragment(instruction, PatchField::JumpNext, out.reborrow())
    }

    fn zero_or_one<'slot>(
        &mut self,
        child: Fragment,
        mut out: Out<'slot, '_, RegexCompileError>,
    ) -> OResult<'slot, Fragment> {
        let split = self.emit(
            Instruction::Split {
                first: child.start,
                second: UNPATCHED,
            },
            out.reborrow(),
        )?;
        let mut outs = child.outs;
        outs.try_reserve(1, out.reborrow())?;
        outs.try_push(
            Patch {
                instruction: split,
                field: PatchField::SplitSecond,
            },
            out.reborrow(),
        )?;
        Ok(Fragment { start: split, outs })
    }

    fn zero_or_more<'slot>(
        &mut self,
        child: Fragment,
        mut out: Out<'slot, '_, RegexCompileError>,
    ) -> OResult<'slot, Fragment> {
        let split = self.emit(
            Instruction::Split {
                first: child.start,
                second: UNPATCHED,
            },
            out.reborrow(),
        )?;
        self.patch(&child.outs, split, out.reborrow())?;
        self.fragment(split, PatchField::SplitSecond, out.reborrow())
    }

    fn one_or_more<'slot>(
        &mut self,
        child: Fragment,
        mut out: Out<'slot, '_, RegexCompileError>,
    ) -> OResult<'slot, Fragment> {
        let split = self.emit(
            Instruction::Split {
                first: child.start,
                second: UNPATCHED,
            },
            out.reborrow(),
        )?;
        self.patch(&child.outs, split, out.reborrow())?;
        let mut fragment = self.fragment(split, PatchField::SplitSecond, out.reborrow())?;
        fragment.start = child.start;
        Ok(fragment)
    }

    fn concatenate<'slot>(
        &mut self,
        left: Fragment,
        right: Fragment,
        mut out: Out<'slot, '_, RegexCompileError>,
    ) -> OResult<'slot, Fragment> {
        self.patch(&left.outs, right.start, out.reborrow())?;
        Ok(Fragment {
            start: left.start,
            outs: right.outs,
        })
    }

    fn alternate<'slot>(
        &mut self,
        mut left: Fragment,
        right: Fragment,
        mut out: Out<'slot, '_, RegexCompileError>,
    ) -> OResult<'slot, Fragment> {
        let split = self.emit(
            Instruction::Split {
                first: left.start,
                second: right.start,
            },
            out.reborrow(),
        )?;
        left.outs.try_reserve(right.outs.len(), out.reborrow())?;
        for patch in right.outs {
            left.outs.try_push(patch, out.reborrow())?;
        }
        Ok(Fragment {
            start: split,
            outs: left.outs,
        })
    }

    fn fragment<'slot>(
        &mut self,
        instruction: StateId,
        field: PatchField,
        mut out: Out<'slot, '_, RegexCompileError>,
    ) -> OResult<'slot, Fragment> {
        let mut outs = ChargedVec::new_regex(Some(self.execution), out.reborrow())?;
        outs.try_reserve_exact(1, out.reborrow())?;
        outs.try_push(Patch { instruction, field }, out.reborrow())?;
        Ok(Fragment {
            start: instruction,
            outs,
        })
    }

    fn emit<'slot>(
        &mut self,
        instruction: Instruction,
        mut out: Out<'slot, '_, RegexCompileError>,
    ) -> OResult<'slot, StateId> {
        if self.instructions.len() >= self.max_instructions {
            return Err(out.fail(RegexCompileError::ResourceLimitExceeded(
                RegexResource::InstructionCount,
            )));
        }
        self.instructions.try_reserve(1, out.reborrow())?;
        let index = StateId::new(self.instructions.len());
        self.instructions.try_push(instruction, out.reborrow())?;
        Ok(index)
    }

    fn patch<'slot>(
        &mut self,
        patches: &[Patch],
        target: StateId,
        mut out: Out<'slot, '_, RegexCompileError>,
    ) -> OResult<'slot, ()> {
        for patch in patches {
            self.execution.consume(1, out.reborrow())?;
            let instruction = self
                .instructions
                .get_mut(patch.instruction.index())
                .ok_or(RegexCompileError::InternalInvariantFailure(
                    RegexErrorContext::new(COMPILER_PATCH_INSTRUCTION_MISSING),
                ))
                .store_err_into(out.reborrow())?;
            if !instruction.patch(patch.field, target) {
                return Err(out.fail(RegexCompileError::InternalInvariantFailure(
                    RegexErrorContext::new(COMPILER_PATCH_KIND_MISMATCH),
                )));
            }
        }
        Ok(())
    }

    fn program_is_valid<'slot>(
        &self,
        start: StateId,
        mut out: Out<'slot, '_, RegexCompileError>,
    ) -> OResult<'slot, ()> {
        if start.index() >= self.instructions.len() {
            return Err(out.fail(RegexCompileError::InternalInvariantFailure(
                RegexErrorContext::new(COMPILER_PROGRAM_START_OUT_OF_RANGE),
            )));
        }
        for instruction in self.instructions.iter() {
            self.execution.consume(1, out.reborrow())?;
            let valid = match instruction {
                Instruction::Consume { class, next } => {
                    !class.ranges().is_empty() && next.index() < self.instructions.len()
                }
                Instruction::Split { first, second } => {
                    first.index() < self.instructions.len()
                        && second.index() < self.instructions.len()
                }
                Instruction::Jump { next }
                | Instruction::AssertStart { next }
                | Instruction::AssertEnd { next } => next.index() < self.instructions.len(),
                Instruction::Match => true,
            };
            if !valid {
                return Err(out.fail(RegexCompileError::InternalInvariantFailure(
                    RegexErrorContext::new(COMPILER_PROGRAM_TARGET_OUT_OF_RANGE),
                )));
            }
        }
        Ok(())
    }
}

impl Regex {
    pub(crate) fn new(
        instructions: ChargedVec<Instruction>,
        start: StateId,
        range_count: usize,
    ) -> Self {
        Self {
            instructions,
            start,
            range_count,
        }
    }

    pub(crate) fn instruction<'slot>(
        &self,
        state: StateId,
        mut out: Out<'slot, '_, RegexMatchError>,
    ) -> OResult<'slot, &Instruction> {
        self.instructions
            .get(state.index())
            .ok_or(RegexMatchError::InternalInvariantFailure(
                RegexErrorContext::new(MATCH_INSTRUCTION_OUT_OF_RANGE),
            ))
            .store_err_into(out.reborrow())
    }

    pub(crate) fn start(&self) -> StateId {
        self.start
    }

    pub(crate) fn state_count(&self) -> usize {
        self.instructions.len()
    }

    /// Returns the total number of instructions in the program.
    pub fn instruction_count(&self) -> usize {
        self.instructions.len()
    }

    /// Returns the number of closed Unicode scalar ranges held by the program.
    pub fn range_count(&self) -> usize {
        self.range_count
    }

    /// Each instruction corresponds to one matcher state slot.
    pub fn scratch_state_count(&self) -> usize {
        self.instructions.len()
    }

    /// Runs a budgeted fragment search using the caller-provided workspace.
    pub fn is_match<'slot, 'borrow>(
        &self,
        input: &str,
        scratch: &mut RegexScratch,
        budget: &RegexExecutionBudget,
        out: Out<'slot, 'borrow, RegexMatchError>,
    ) -> OResult<'slot, bool> {
        RegexMatcher::is_match(self, input, scratch, budget, out)
    }
}

#[cfg(test)]
mod tests {
    use alloc::{format, string::String};
    use core::fmt::Debug;
    use opeo::{ErrSlot, OResult, Out};

    use typol_core::{BudgetError, RegexExecutionBudget};
    use typol_policy::RegexLimits;

    use crate::{RegexCompileError, RegexResource};

    use super::{Regex, RegexCompiler};

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

    fn compile(
        pattern: &str,
        limits: &RegexLimits,
        execution_errors: &mut ErrSlot<BudgetError>,
        compile_errors: &mut ErrSlot<RegexCompileError>,
    ) -> Attempt<Regex, RegexCompileError> {
        let execution = match attempt(execution_errors, |out| {
            RegexExecutionBudget::new(67_108_864, 67_108_864, 67_108_864, out)
        }) {
            Attempt::Success(execution) => execution,
            Attempt::Failure(error) => {
                return Attempt::Failure(RegexCompileError::from(error));
            }
        };
        attempt(compile_errors, |out| {
            RegexCompiler::compile(pattern, limits, &execution, out)
        })
    }

    fn compiled(
        pattern: &str,
        limits: &RegexLimits,
        execution_errors: &mut ErrSlot<BudgetError>,
        compile_errors: &mut ErrSlot<RegexCompileError>,
    ) -> Result<Regex, String> {
        match compile(pattern, limits, execution_errors, compile_errors) {
            Attempt::Success(regex) => Ok(regex),
            Attempt::Failure(error) => Err(format!("{error:?}")),
        }
    }

    #[test]
    fn compiles_empty_pattern_and_alternation() -> Result<(), String> {
        let limits = RegexLimits::default();
        let mut execution_errors = ErrSlot::<BudgetError>::new();
        let mut compile_errors = ErrSlot::<RegexCompileError>::new();
        let empty = compiled("", &limits, &mut execution_errors, &mut compile_errors)?;
        check_eq(
            empty.instruction_count(),
            2,
            "empty-pattern instruction count does not match",
        )?;
        check_eq(
            empty.range_count(),
            0,
            "empty-pattern range count does not match",
        )?;

        let alternation = compiled("a|b", &limits, &mut execution_errors, &mut compile_errors)?;
        check_eq(
            alternation.instruction_count(),
            4,
            "alternation instruction count does not match",
        )?;
        check_eq(
            alternation.range_count(),
            2,
            "alternation range count does not match",
        )
    }

    #[test]
    fn counts_repeated_classes_after_expansion() -> Result<(), String> {
        let limits = RegexLimits::default();
        let mut execution_errors = ErrSlot::<BudgetError>::new();
        let mut compile_errors = ErrSlot::<RegexCompileError>::new();
        let regex = compiled(
            "[a-z]{2,3}",
            &limits,
            &mut execution_errors,
            &mut compile_errors,
        )?;
        check_eq(
            regex.instruction_count(),
            5,
            "bounded repetition instruction count does not match",
        )?;
        check_eq(
            regex.range_count(),
            3,
            "bounded repetition range count does not match",
        )?;
        check_eq(
            regex.scratch_state_count(),
            regex.instruction_count(),
            "scratch state count should match the instruction count",
        )
    }

    #[test]
    fn enforces_instruction_limit_at_exact_boundary() -> Result<(), String> {
        let mut limits = RegexLimits::default();
        let mut execution_errors = ErrSlot::<BudgetError>::new();
        let mut compile_errors = ErrSlot::<RegexCompileError>::new();
        limits.set_max_regex_instructions_per_pattern(256);
        check(
            compiled(
                "a{255}",
                &limits,
                &mut execution_errors,
                &mut compile_errors,
            )
            .is_ok(),
            "boundary instruction count should be accepted",
        )?;

        limits.set_max_regex_instructions_per_pattern(255);
        check(
            compiled(
                "a{255}",
                &limits,
                &mut execution_errors,
                &mut compile_errors,
            )
            .is_err(),
            "exceeding the instruction count should fail",
        )
    }

    #[test]
    fn enforces_range_limit_after_repetition_expansion() -> Result<(), String> {
        let mut limits = RegexLimits::default();
        let mut execution_errors = ErrSlot::<BudgetError>::new();
        let mut compile_errors = ErrSlot::<RegexCompileError>::new();
        limits.set_max_regex_ranges_per_pattern(2);
        check(
            matches!(
                compile(
                    "[a-z]{3}",
                    &limits,
                    &mut execution_errors,
                    &mut compile_errors
                ),
                Attempt::Failure(RegexCompileError::ResourceLimitExceeded(
                    RegexResource::CharacterRangeCount
                ))
            ),
            "range count after repetition expansion should be limited",
        )
    }

    #[test]
    fn dot_uses_three_scalar_ranges_and_rejects_two_available_ranges() {
        let mut limits = RegexLimits::default();
        limits.set_max_regex_ranges_per_pattern(3);
        let mut execution_errors = ErrSlot::<BudgetError>::new();
        let mut compile_errors = ErrSlot::<RegexCompileError>::new();
        assert!(matches!(
            compile(".", &limits, &mut execution_errors, &mut compile_errors),
            Attempt::Success(regex) if regex.range_count() == 3
        ));

        limits.set_max_regex_ranges_per_pattern(2);
        assert!(matches!(
            compile(".", &limits, &mut execution_errors, &mut compile_errors),
            Attempt::Failure(RegexCompileError::ResourceLimitExceeded(
                RegexResource::CharacterRangeCount
            ))
        ));
    }
}
