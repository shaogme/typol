use core::{
    fmt::{Debug, Formatter, Result as FmtResult},
    mem::size_of,
};
use typol_core::{BudgetError, ExecutionBudget, ExecutionLimits};
use typol_policy::ResourceLimits;
use typol_regex::{RegexCompileError, RegexCompiler, RegexResource, RegexScratchError};
use typol_semantic::{RegexExecutionError, RegexExecutor, RegexMatchOptions, opeo::ErrSlot};

enum TestError {
    Budget(BudgetError),
    Compile(RegexCompileError),
    Execution(RegexExecutionError),
    Assertion(&'static str),
}

impl Debug for TestError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        match self {
            Self::Budget(error) => write!(formatter, "Budget({error:?})"),
            Self::Compile(error) => write!(formatter, "Compile({error:?})"),
            Self::Execution(error) => write!(formatter, "Execution({error:?})"),
            Self::Assertion(expression) => write!(formatter, "Assertion({expression})"),
        }
    }
}

impl From<BudgetError> for TestError {
    fn from(error: BudgetError) -> Self {
        Self::Budget(error)
    }
}

impl From<RegexCompileError> for TestError {
    fn from(error: RegexCompileError) -> Self {
        Self::Compile(error)
    }
}

impl From<RegexExecutionError> for TestError {
    fn from(error: RegexExecutionError) -> Self {
        Self::Execution(error)
    }
}

type TestResult<T> = Result<T, TestError>;

macro_rules! check_eq {
    ($actual:expr, $expected:expr $(,)?) => {{
        let actual = &$actual;
        let expected = &$expected;
        if actual != expected {
            return Err(TestError::Assertion(stringify!($actual)));
        }
    }};
}

macro_rules! check {
    ($condition:expr $(, $($message:tt)+)? $(,)?) => {{
        if !$condition {
            return Err(TestError::Assertion(stringify!($condition)));
        }
    }};
}

#[test]
fn executor_rebinds_scratch_instead_of_reusing_another_calls_free_capacity() -> TestResult<()> {
    let mut budget_errors = ErrSlot::<BudgetError>::new();
    let mut compile_errors = ErrSlot::<RegexCompileError>::new();
    let mut execution_errors = ErrSlot::<RegexExecutionError>::new();
    let compilation = budget_errors
        .call(|out| ExecutionBudget::new(ExecutionLimits::new(67_108_864, 67_108_864), out))?;
    let limits = ResourceLimits::default();
    let regex =
        compile_errors.call(|out| RegexCompiler::compile("^x$", &limits, &compilation, out))?;
    let mut executor = RegexExecutor::new();
    check_eq!(
        execution_errors.call(|out| executor.is_match(&regex, Some("x"), &limits, out)),
        Ok(true)
    );
    let initial_budget = budget_errors
        .call(|out| ExecutionBudget::new(ExecutionLimits::new(1_000, 1_048_576), out))?;
    let initial = initial_budget.workspace_bytes();
    let small = budget_errors
        .call(|out| ExecutionBudget::new(ExecutionLimits::new(1_000, initial), out))?;
    check_eq!(
        execution_errors.call(|out| {
            executor.is_match_budgeted(
                &regex,
                RegexMatchOptions {
                    value: Some("x"),
                    limits: &limits,
                    budget: Some(&small),
                },
                out,
            )
        }),
        Err(RegexExecutionError::Scratch(
            RegexScratchError::ResourceLimitExceeded(RegexResource::ScratchStateCount)
        ))
    );
    let first = budget_errors
        .call(|out| ExecutionBudget::new(ExecutionLimits::new(1_000, 1_048_576), out))?;
    check_eq!(
        execution_errors.call(|out| {
            executor.is_match_budgeted(
                &regex,
                RegexMatchOptions {
                    value: Some("x"),
                    limits: &limits,
                    budget: Some(&first),
                },
                out,
            )
        }),
        Ok(true)
    );
    check!(first.workspace_bytes() > initial);
    let second = budget_errors
        .call(|out| ExecutionBudget::new(ExecutionLimits::new(1_000, 1_048_576), out))?;
    check_eq!(
        execution_errors.call(|out| {
            executor.is_match_budgeted(
                &regex,
                RegexMatchOptions {
                    value: Some("x"),
                    limits: &limits,
                    budget: Some(&second),
                },
                out,
            )
        }),
        Ok(true)
    );
    check_eq!(first.workspace_bytes(), initial);
    check!(second.workspace_bytes() > initial);
    drop(executor);
    check_eq!(second.workspace_bytes(), initial);
    Ok(())
}

#[test]
fn executor_grows_scratch_and_keeps_the_old_cache_when_growth_fails() -> TestResult<()> {
    let mut budget_errors = ErrSlot::<BudgetError>::new();
    let mut compile_errors = ErrSlot::<RegexCompileError>::new();
    let mut execution_errors = ErrSlot::<RegexExecutionError>::new();
    let compilation = budget_errors
        .call(|out| ExecutionBudget::new(ExecutionLimits::new(67_108_864, 67_108_864), out))?;
    let limits = ResourceLimits::default();
    let small =
        compile_errors.call(|out| RegexCompiler::compile("^x$", &limits, &compilation, out))?;
    let large =
        compile_errors.call(|out| RegexCompiler::compile("x{4}", &limits, &compilation, out))?;
    check!(large.scratch_state_count() > small.scratch_state_count());

    let small_capacity = small.scratch_state_count();
    let bytes_per_state = 3 * size_of::<usize>() + 2 * size_of::<bool>();
    let baseline = budget_errors
        .call(|out| ExecutionBudget::new(ExecutionLimits::new(1_000_000, 1_048_576), out))?
        .workspace_bytes();
    let cached_limit = baseline + small_capacity * bytes_per_state;
    let constrained = budget_errors
        .call(|out| ExecutionBudget::new(ExecutionLimits::new(1_000_000, cached_limit), out))?;
    let mut executor = RegexExecutor::new();
    check_eq!(
        execution_errors.call(|out| {
            executor.is_match_budgeted(
                &small,
                RegexMatchOptions {
                    value: Some("x"),
                    limits: &limits,
                    budget: Some(&constrained),
                },
                out,
            )
        }),
        Ok(true)
    );
    let cached_bytes = constrained.workspace_bytes();
    check_eq!(cached_bytes, cached_limit);
    check_eq!(
        execution_errors.call(|out| {
            executor.is_match_budgeted(
                &large,
                RegexMatchOptions {
                    value: Some("xxxx"),
                    limits: &limits,
                    budget: Some(&constrained),
                },
                out,
            )
        }),
        Err(RegexExecutionError::Scratch(
            RegexScratchError::ResourceLimitExceeded(RegexResource::ScratchStateCount)
        ))
    );
    check_eq!(constrained.workspace_bytes(), cached_bytes);
    check!(constrained.exhausted());

    let mut lowered_limits = limits;
    lowered_limits.set_max_regex_scratch_states(small_capacity - 1);
    check_eq!(
        execution_errors.call(|out| {
            executor.is_match_budgeted(
                &small,
                RegexMatchOptions {
                    value: None,
                    limits: &lowered_limits,
                    budget: Some(&constrained),
                },
                out,
            )
        }),
        Ok(true)
    );
    check_eq!(constrained.workspace_bytes(), baseline);
    Ok(())
}

#[test]
fn empty_value_releases_scratch_when_the_budget_changes_without_allocating_again() -> TestResult<()>
{
    let mut budget_errors = ErrSlot::<BudgetError>::new();
    let mut compile_errors = ErrSlot::<RegexCompileError>::new();
    let mut execution_errors = ErrSlot::<RegexExecutionError>::new();
    let compilation = budget_errors
        .call(|out| ExecutionBudget::new(ExecutionLimits::new(67_108_864, 67_108_864), out))?;
    let limits = ResourceLimits::default();
    let regex =
        compile_errors.call(|out| RegexCompiler::compile("^x$", &limits, &compilation, out))?;
    let first = budget_errors
        .call(|out| ExecutionBudget::new(ExecutionLimits::new(1_000, 1_048_576), out))?;
    let second = budget_errors
        .call(|out| ExecutionBudget::new(ExecutionLimits::new(1_000, 1_048_576), out))?;
    let first_initial = first.workspace_bytes();
    let second_initial = second.workspace_bytes();
    let mut executor = RegexExecutor::new();
    check_eq!(
        execution_errors.call(|out| {
            executor.is_match_budgeted(
                &regex,
                RegexMatchOptions {
                    value: Some("x"),
                    limits: &limits,
                    budget: Some(&first),
                },
                out,
            )
        }),
        Ok(true)
    );
    check!(first.workspace_bytes() > first_initial);
    check_eq!(
        execution_errors.call(|out| {
            executor.is_match_budgeted(
                &regex,
                RegexMatchOptions {
                    value: None,
                    limits: &limits,
                    budget: Some(&second),
                },
                out,
            )
        }),
        Ok(true)
    );
    check_eq!(first.workspace_bytes(), first_initial);
    check_eq!(second.workspace_bytes(), second_initial);
    Ok(())
}
