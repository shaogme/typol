use core::fmt::{Debug, Formatter, Result as FmtResult};
use typol_core::{BudgetError, ExecutionBudget, ExecutionLimits};
use typol_policy::ResourceLimits;
use typol_regex::{
    RegexCompileError, RegexCompiler, RegexMatchError, RegexResource, RegexScratchError,
};
use typol_semantic::{RegexExecutionError, RegexExecutor, opeo::ErrSlot};

enum TestError {
    Budget(BudgetError),
    Compile(RegexCompileError),
    Execution(RegexExecutionError),
    MissingExpectedError,
    Assertion(&'static str),
}

impl Debug for TestError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        match self {
            Self::Budget(error) => write!(formatter, "Budget({error:?})"),
            Self::Compile(error) => write!(formatter, "Compile({error:?})"),
            Self::Execution(error) => write!(formatter, "Execution({error:?})"),
            Self::MissingExpectedError => formatter.write_str("MissingExpectedError"),
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
fn matches_independent_patterns_and_reuses_larger_scratch() -> TestResult<()> {
    let mut budget_errors = ErrSlot::<BudgetError>::new();
    let mut compile_errors = ErrSlot::<RegexCompileError>::new();
    let mut execution_errors = ErrSlot::<RegexExecutionError>::new();
    let compilation = budget_errors
        .call(|out| ExecutionBudget::new(ExecutionLimits::new(67_108_864, 67_108_864), out))?;
    let limits = ResourceLimits::default();
    let mut executor = RegexExecutor::new();
    for (pattern, cases) in [
        (
            "^service-[0-9]+$",
            [("service-42", true), ("prefix-service-42", false)],
        ),
        (
            r"\.internal$",
            [("host.internal", true), ("host.external", false)],
        ),
        ("x{4}", [("xxxx", true), ("xx", false)]),
        ("x", [("text", true), ("absent", false)]),
    ] {
        let regex = compile_errors
            .call(|out| RegexCompiler::compile(pattern, &limits, &compilation, out))?;
        for (input, expected) in cases {
            check_eq!(
                execution_errors.call(|out| executor.is_match(&regex, Some(input), &limits, out)),
                Ok(expected)
            );
        }
    }
    Ok(())
}

#[test]
fn none_skips_scratch_and_resource_errors_stay_structured() -> TestResult<()> {
    let mut budget_errors = ErrSlot::<BudgetError>::new();
    let mut compile_errors = ErrSlot::<RegexCompileError>::new();
    let mut execution_errors = ErrSlot::<RegexExecutionError>::new();
    let compilation = budget_errors
        .call(|out| ExecutionBudget::new(ExecutionLimits::new(67_108_864, 67_108_864), out))?;
    let defaults = ResourceLimits::default();
    let regex = compile_errors
        .call(|out| RegexCompiler::compile("^secret$", &defaults, &compilation, out))?;
    let mut executor = RegexExecutor::new();
    let mut limits = defaults;
    limits.set_max_regex_scratch_states(0);
    check_eq!(
        execution_errors.call(|out| executor.is_match(&regex, None, &limits, out)),
        Ok(true)
    );
    check_eq!(
        execution_errors
            .call(|out| { executor.is_match(&regex, Some("private-input"), &limits, out) }),
        Err(RegexExecutionError::Scratch(
            RegexScratchError::ResourceLimitExceeded(RegexResource::ScratchStateCount)
        ))
    );
    limits = defaults;
    limits.set_max_regex_search_steps(0);
    let error = execution_errors
        .call(|out| executor.is_match(&regex, Some("private-input"), &limits, out))
        .err()
        .ok_or(TestError::MissingExpectedError)?;
    check!(matches!(
        error,
        RegexExecutionError::Match(RegexMatchError::ResourceLimitExceeded(_))
    ));
    let debug = format!("{error:?}");
    check!(!debug.contains("private-input"));
    check!(!debug.contains("secret"));
    Ok(())
}
