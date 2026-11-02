use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    env,
    error::Error,
    fmt,
    io::Error as IoError,
    process::Command,
    ptr::null_mut,
};
use typol_core::{
    BudgetError, ChargedString, ChargedVec, ConfigValueBuilder, DiagnosticContext, ExecutionBudget,
    ExecutionLimits, ValueError, ValueErrorKind, ValueSource, diagnostic_context,
};
use typol_opeo_test_support::TestOpeo;

macro_rules! ensure {
    ($condition:expr $(,)?) => {
        if !$condition {
            return Err(IoError::other(concat!("assertion failed: ", stringify!($condition))).into());
        }
    };
    ($condition:expr, $($argument:tt)+) => {
        if !$condition {
            return Err(IoError::other(format!($($argument)+)).into());
        }
    };
}

macro_rules! ensure_eq {
    ($left:expr, $right:expr $(,)?) => {{
        let left = &$left;
        let right = &$right;
        if left != right {
            return Err(IoError::other(format!(
                "assertion failed: {} and {} differ (actual {:?}, expected {:?})",
                stringify!($left),
                stringify!($right),
                left,
                right,
            ))
            .into());
        }
    }};
    ($left:expr, $right:expr, $($argument:tt)+) => {{
        let left = &$left;
        let right = &$right;
        if left != right {
            return Err(IoError::other(format!($($argument)+)).into());
        }
    }};
}

static MISSING_TEST_ITEM: DiagnosticContext = diagnostic_context!(
    "typol-core-tests",
    "budget-storage-allocation",
    "expected-item-missing",
);

#[derive(Clone, Copy, Default)]
struct State {
    active: bool,
    fail_at: Option<usize>,
    persistent: bool,
    attempts: usize,
    live: usize,
    peak: usize,
}

thread_local! {
    static STATE: Cell<State> = const { Cell::new(State {
        active: false, fail_at: None, persistent: false, attempts: 0, live: 0, peak: 0,
    }) };
}

struct Allocator;

unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let reject = STATE.with(|cell| {
            let mut state = cell.get();
            if !state.active {
                return false;
            }
            let reject = state.fail_at.is_some_and(|index| {
                state.attempts == index || (state.persistent && state.attempts > index)
            });
            state.attempts += 1;
            cell.set(state);
            reject
        });
        if reject {
            return null_mut();
        }
        // SAFETY: Passes the caller’s layout directly to the system allocator without changing its alignment or size.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            STATE.with(|cell| {
                let mut state = cell.get();
                if state.active {
                    state.live += layout.size();
                    state.peak = state.peak.max(state.live);
                    cell.set(state);
                }
            });
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        STATE.with(|cell| {
            let mut state = cell.get();
            if state.active {
                state.live -= layout.size();
                cell.set(state);
            }
        });
        // SAFETY: The raw pointer and original layout are passed together and deallocated exactly once.
        unsafe { System.dealloc(pointer, layout) };
    }
}

#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

struct Scope;

impl Scope {
    fn new(fail_at: Option<usize>, persistent: bool) -> Self {
        STATE.with(|cell| {
            cell.set(State {
                active: true,
                fail_at,
                persistent,
                ..State::default()
            })
        });
        Self
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        STATE.with(|cell| {
            let mut state = cell.get();
            state.active = false;
            cell.set(state);
        });
    }
}

struct Usage {
    peak: usize,
    retained: usize,
    same_live: bool,
}

#[derive(Debug)]
enum WorkloadError {
    Budget(BudgetError),
    Value(ValueError),
}

impl fmt::Display for WorkloadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Budget(error) => fmt::Display::fmt(error, formatter),
            Self::Value(error) => fmt::Display::fmt(error, formatter),
        }
    }
}

impl Error for WorkloadError {}

impl From<BudgetError> for WorkloadError {
    fn from(error: BudgetError) -> Self {
        Self::Budget(error)
    }
}

impl From<ValueError> for WorkloadError {
    fn from(error: ValueError) -> Self {
        Self::Value(error)
    }
}

fn workload() -> Result<Usage, WorkloadError> {
    let budget = TestOpeo::call(|out| {
        ExecutionBudget::new(ExecutionLimits::new(1_000_000, 1_048_576), out)
    })?;
    let mut values = TestOpeo::call::<BudgetError, _>(|out| ChargedVec::new(Some(&budget), out))?;
    for text in ["€€", "🙂", "hello", "world", "x"] {
        let value =
            TestOpeo::call::<BudgetError, _>(|out| ChargedString::copy(text, Some(&budget), out))?;
        TestOpeo::call::<BudgetError, _>(|out| values.try_push(value, out))?;
    }
    let first = values
        .first_mut()
        .ok_or_else(|| BudgetError::invariant(&MISSING_TEST_ITEM))?;
    TestOpeo::call::<BudgetError, _>(|out| first.try_push_str("€uro text!", out))?;
    values.truncate(2);
    let mut builder = TestOpeo::call(|out| ConfigValueBuilder::new(Some(&budget), out))?;
    let first =
        TestOpeo::call(|out| builder.add_string("€config", ValueSource::SchemaDefault, out))?;
    let second = TestOpeo::call(|out| builder.add_string("🙂", ValueSource::RustValue, out))?;
    let third = TestOpeo::call(|out| builder.add_string("hello", ValueSource::ConfigLiteral, out))?;
    let root = TestOpeo::call(|out| {
        builder.add_array(&[first, second, third], ValueSource::RustValue, out)
    })?;
    let copied = TestOpeo::call(|out| builder.finish(root, out))?;
    let text = TestOpeo::call::<BudgetError, _>(|out| {
        ChargedString::copy("€size test", Some(&budget), out)
    })?;
    let same_live = STATE.with(|cell| cell.get().live == budget.workspace_bytes());
    let peak = budget.peak_workspace_bytes();
    drop(text);
    drop(copied);
    drop(values);
    Ok(Usage {
        peak,
        retained: budget.workspace_bytes(),
        same_live,
    })
}

#[test]
fn every_allocation_failure_releases_exact_storage_in_child_process() -> Result<(), Box<dyn Error>>
{
    if env::var_os("TYPOL_BUDGET_STORAGE_OOM").is_none() {
        let status = Command::new(env::current_exe()?)
            .args([
                "--exact",
                "every_allocation_failure_releases_exact_storage_in_child_process",
            ])
            .env("TYPOL_BUDGET_STORAGE_OOM", "1")
            .status()?;
        ensure!(status.success());
        return Ok(());
    }
    let run = |fail_at, persistent| {
        let scope = Scope::new(fail_at, persistent);
        let result = workload();
        let state = STATE.with(Cell::get);
        drop(scope);
        (result, state)
    };
    let (baseline, state) = run(None, false);
    let baseline = baseline?;
    ensure!(baseline.same_live);
    ensure_eq!(baseline.peak, state.peak);
    ensure_eq!(
        baseline.retained,
        TestOpeo::call(|out| ExecutionBudget::new(ExecutionLimits::new(1, 1024), out))?
            .workspace_bytes()
    );
    ensure_eq!(state.live, 0);
    for index in 0..state.attempts {
        for persistent in [false, true] {
            let (result, state) = run(Some(index), persistent);
            let allocation_failed = match result {
                Err(WorkloadError::Budget(BudgetError::Allocation)) => true,
                Err(WorkloadError::Value(error)) => {
                    error.kind() == ValueErrorKind::AllocationFailed
                }
                _ => false,
            };
            ensure!(allocation_failed, "allocation {index}");
            ensure_eq!(
                state.live,
                0,
                "allocation {index} failed; persistent={persistent}"
            );
        }
    }
    Ok(())
}
