use std::{
    alloc::{GlobalAlloc, Layout, System},
    ptr::null_mut,
    sync::atomic::{AtomicBool, Ordering},
};

use core::fmt::{Debug, Formatter, Result as FmtResult};
use typol_core::{BudgetError, RegexExecutionBudget};
use typol_policy::RegexLimits;
use typol_regex::{RegexCompileError, RegexCompiler, RegexScratchError};
use typol_semantic::{RegexExecutionError, RegexExecutor, RegexMatchOptions};
use typol_test_support::{TestOpeo, TestOpeoOutcome, TestSlot};

struct FailNextAllocation;

static FAIL_NEXT_ALLOCATION: AtomicBool = AtomicBool::new(false);

unsafe impl GlobalAlloc for FailNextAllocation {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if FAIL_NEXT_ALLOCATION.swap(false, Ordering::SeqCst) {
            null_mut()
        } else {
            unsafe { System.alloc(layout) }
        }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if FAIL_NEXT_ALLOCATION.swap(false, Ordering::SeqCst) {
            null_mut()
        } else {
            unsafe { System.realloc(pointer, layout, new_size) }
        }
    }
}

#[global_allocator]
static ALLOCATOR: FailNextAllocation = FailNextAllocation;

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

#[test]
fn scratch_allocation_failure_is_structured() -> TestResult<()> {
    let compilation = TestOpeo::call::<BudgetError, _>(|out| {
        RegexExecutionBudget::new(67_108_864, 67_108_864, 67_108_864, out)
    })?;
    let limits = RegexLimits::default();
    let regex = TestOpeo::call::<RegexCompileError, _>(|out| {
        RegexCompiler::compile("^secret$", &limits, &compilation, out)
    })?;
    let mut pool = RegexExecutor::new();
    let mut execution_errors = TestSlot::<RegexExecutionError>::default();

    FAIL_NEXT_ALLOCATION.store(true, Ordering::SeqCst);
    match execution_errors.run(|out| {
        pool.is_match_budgeted(
            &regex,
            RegexMatchOptions {
                value: Some("private-input"),
                limits: &limits,
                budget: &compilation,
            },
            out,
        )
    }) {
        TestOpeoOutcome::Value(_) => return Err(TestError::MissingExpectedError),
        TestOpeoOutcome::Error(error) => check_eq!(
            error,
            RegexExecutionError::Scratch(RegexScratchError::AllocationFailed)
        ),
    }

    check_eq!(
        execution_errors.access(|out| {
            pool.is_match_budgeted(
                &regex,
                RegexMatchOptions {
                    value: Some("secret"),
                    limits: &limits,
                    budget: &compilation,
                },
                out,
            )
        }),
        Ok(true)
    );
    let larger = TestOpeo::call::<RegexCompileError, _>(|out| {
        RegexCompiler::compile("x{20}", &limits, &compilation, out)
    })?;
    let retained_bytes = compilation.workspace_bytes();

    FAIL_NEXT_ALLOCATION.store(true, Ordering::SeqCst);
    match execution_errors.run(|out| {
        pool.is_match_budgeted(
            &larger,
            RegexMatchOptions {
                value: Some("xxxxxxxxxxxxxxxxxxxx"),
                limits: &limits,
                budget: &compilation,
            },
            out,
        )
    }) {
        TestOpeoOutcome::Value(_) => return Err(TestError::MissingExpectedError),
        TestOpeoOutcome::Error(error) => check_eq!(
            error,
            RegexExecutionError::Scratch(RegexScratchError::AllocationFailed)
        ),
    }
    check_eq!(compilation.workspace_bytes(), retained_bytes);
    check_eq!(
        execution_errors.access(|out| {
            pool.is_match_budgeted(
                &regex,
                RegexMatchOptions {
                    value: Some("secret"),
                    limits: &limits,
                    budget: &compilation,
                },
                out,
            )
        }),
        Ok(true)
    );
    Ok(())
}
