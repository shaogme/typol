use core::fmt::{Debug, Formatter, Result as FmtResult};
use typol_core::{BudgetError, RegexExecutionBudget, ResourceLimit};
use typol_policy::RegexLimits;
use typol_regex::{RegexCompileError, RegexCompiler, RegexMatchError, RegexScratchError};
use typol_semantic::{RegexExecutionError, RegexExecutor, RegexMatchOptions, opeo::ErrSlot};
use typol_test_support::{check, check_eq};

enum TestError {
    Budget(BudgetError),
    Compile(RegexCompileError),
    Execution(Box<RegexExecutionError>),
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
        Self::Execution(Box::new(error))
    }
}

type TestResult<T> = Result<T, TestError>;

#[test]
fn matches_independent_patterns_and_reuses_larger_scratch() -> TestResult<()> {
    let mut budget_errors = ErrSlot::<BudgetError>::new();
    let mut compile_errors = ErrSlot::<RegexCompileError>::new();
    let mut execution_errors = ErrSlot::<RegexExecutionError>::new();
    let compilation = budget_errors
        .call(|out| RegexExecutionBudget::new(67_108_864, 67_108_864, 67_108_864, out))?;
    let limits = RegexLimits::default();
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
            check_eq!(TestError::Assertion;
                execution_errors.call(|out| {
                    executor.is_match_budgeted(
                        &regex,
                        RegexMatchOptions {
                            value: Some(input),
                            limits: &limits,
                            budget: &compilation,
                        },
                        out,
                    )
                }),
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
        .call(|out| RegexExecutionBudget::new(67_108_864, 67_108_864, 67_108_864, out))?;
    let defaults = RegexLimits::default();
    let regex = compile_errors
        .call(|out| RegexCompiler::compile("^secret$", &defaults, &compilation, out))?;
    let mut executor = RegexExecutor::new();
    let mut limits = defaults;
    limits.set_max_regex_scratch_states(0);
    check_eq!(TestError::Assertion;
        execution_errors.call(|out| {
            executor.is_match_budgeted(
                &regex,
                RegexMatchOptions {
                    value: None,
                    limits: &limits,
                    budget: &compilation,
                },
                out,
            )
        }),
        Ok(true)
    );
    check_eq!(TestError::Assertion;
        execution_errors.call(|out| {
            executor.is_match_budgeted(
                &regex,
                RegexMatchOptions {
                    value: Some("private-input"),
                    limits: &limits,
                    budget: &compilation,
                },
                out,
            )
        }),
        Err(RegexExecutionError::Scratch(
            RegexScratchError::ResourceLimit(ResourceLimit::RegexScratchStates)
        ))
    );
    limits = defaults;
    let zero_search =
        budget_errors.call(|out| RegexExecutionBudget::new(67_108_864, 0, 67_108_864, out))?;
    let error = execution_errors
        .call(|out| {
            executor.is_match_budgeted(
                &regex,
                RegexMatchOptions {
                    value: Some("private-input"),
                    limits: &limits,
                    budget: &zero_search,
                },
                out,
            )
        })
        .err()
        .ok_or(TestError::MissingExpectedError)?;
    check!(TestError::Assertion; matches!(
        error,
        RegexExecutionError::Match(RegexMatchError::ResourceLimit(_))
    ));
    let debug = format!("{error:?}");
    check!(TestError::Assertion; !debug.contains("private-input"));
    check!(TestError::Assertion; !debug.contains("secret"));
    Ok(())
}
