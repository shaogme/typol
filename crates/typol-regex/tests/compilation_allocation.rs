#[path = "support/clippy.rs"]
mod clippy;

use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    env,
    process::Command,
    ptr::null_mut,
    thread,
};
use typol_core::{BudgetError, ExecutionLimits, RegexExecutionBudget};
use typol_opeo_test_support::TestSlot;
use typol_policy::RegexLimits;
use typol_regex::{Regex, RegexCompileError, RegexCompiler, RegexScratch, RegexScratchError};

#[derive(Clone, Copy, Default)]
struct AllocationState {
    active: bool,
    fail_at: Option<usize>,
    persistent: bool,
    attempts: usize,
    live: isize,
    peak: isize,
}

thread_local! {
    static STATE: Cell<AllocationState> = Cell::new(AllocationState::default());
}

struct CheckedAllocator;

fn fail() -> bool {
    STATE
        .try_with(|state| {
            let mut current = state.get();
            if !current.active {
                return false;
            }
            let fails = current.fail_at.is_some_and(|at| {
                at == current.attempts || (current.persistent && at < current.attempts)
            });
            current.attempts += 1;
            state.set(current);
            fails
        })
        .unwrap_or(false)
}

fn account(bytes: isize) {
    let _ = STATE.try_with(|state| {
        let mut current = state.get();
        if current.active {
            current.live += bytes;
            current.peak = current.peak.max(current.live);
            state.set(current);
        }
    });
}

unsafe impl GlobalAlloc for CheckedAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if fail() {
            return null_mut();
        }
        // SAFETY: Forwards the allocation layout unchanged and accounts only for successful allocations.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            account(layout.size() as isize);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        account(-(layout.size() as isize));
        // SAFETY: The pointer and layout came from a prior system allocation.
        unsafe { System.dealloc(pointer, layout) };
    }
}

#[global_allocator]
static ALLOCATOR: CheckedAllocator = CheckedAllocator;

struct Artifacts {
    regex: Regex,
    scratch: RegexScratch,
}

enum Failure {
    Compile(RegexCompileError),
    Scratch(RegexScratchError),
}

fn run(
    budget: &RegexExecutionBudget,
    compile_errors: &mut TestSlot<RegexCompileError>,
    scratch_errors: &mut TestSlot<RegexScratchError>,
) -> clippy::Attempt<Artifacts, Failure> {
    let limits = RegexLimits::default();
    let regex = match clippy::Clippy::attempt(compile_errors, |out| {
        RegexCompiler::compile("(ab|[^a-z]){2,8}", &limits, budget, out)
    })
    .map_failure(Failure::Compile)
    {
        clippy::Attempt::Success(regex) => regex,
        clippy::Attempt::Failure(failure) => return clippy::Attempt::Failure(failure),
    };
    clippy::Clippy::attempt(scratch_errors, |out| {
        RegexScratch::new(&regex, &limits, budget, out)
    })
    .map_success(|scratch| Artifacts { regex, scratch })
    .map_failure(Failure::Scratch)
}

#[test]
fn compilation_allocation_child() {
    if env::var_os("TYPOL_REGEX_ALLOCATION_CHILD").is_none() {
        return;
    }
    let spawned = thread::Builder::new().stack_size(128 * 1024).spawn(|| {
        let mut attempts = 0;
        let mut budget_errors = TestSlot::<BudgetError>::default();
        let mut compile_errors = TestSlot::<RegexCompileError>::default();
        let mut scratch_errors = TestSlot::<RegexScratchError>::default();
        for fail_at in [None].into_iter().chain((0..usize::MAX).map(Some)) {
            if fail_at.is_some_and(|index| index >= attempts) {
                break;
            }
            for persistent in [false, true] {
                let Some(budget) = clippy::Clippy::succeed_opeo(
                    clippy::Clippy::regex_execution_budget(
                        &mut budget_errors,
                        ExecutionLimits::new(67_108_864, 67_108_864),
                    ),
                    "allocation-failure test budget should be created",
                ) else {
                    return;
                };
                let initial = budget.workspace_bytes();
                STATE.with(|state| {
                    state.set(AllocationState {
                        active: true,
                        fail_at,
                        persistent,
                        ..AllocationState::default()
                    })
                });
                let result = run(&budget, &mut compile_errors, &mut scratch_errors);
                let current = STATE.with(Cell::get);
                let retained = budget.workspace_bytes() - initial;
                let peak = budget.peak_workspace_bytes() - initial;
                let failed = result.failure_matches(|failure| {
                    matches!(
                        failure,
                        Failure::Compile(RegexCompileError::AllocationFailed)
                            | Failure::Scratch(RegexScratchError::AllocationFailed)
                    )
                });
                if let clippy::Attempt::Success(artifacts) = result {
                    drop(artifacts.regex);
                    drop(artifacts.scratch);
                }
                let final_bytes = budget.workspace_bytes();
                let final_state = STATE.with(|state| {
                    let value = state.get();
                    state.set(AllocationState::default());
                    value
                });
                if fail_at.is_none() {
                    attempts = current.attempts;
                } else {
                    assert!(
                        failed,
                        "every one-shot or persistent OOM must return an allocation error"
                    );
                }
                assert_eq!(current.live as usize, retained);
                if fail_at.is_none() {
                    assert_eq!(current.peak as usize, peak);
                } else {
                    assert!(current.peak as usize <= peak);
                }
                assert_eq!(final_state.live, 0);
                assert_eq!(final_bytes, initial);
            }
        }
        assert!(attempts > 20);
    });
    assert!(
        spawned.is_ok(),
        "allocation-failure test thread should start"
    );
    let Some(handle) = spawned.ok() else {
        return;
    };
    let joined = handle.join();
    assert!(
        joined.is_ok(),
        "allocation-failure test thread should exit normally"
    );
}

#[test]
fn every_allocation_failure_releases_exact_payloads_on_a_small_stack() {
    let executable = env::current_exe();
    assert!(
        executable.is_ok(),
        "test executable path should be available"
    );
    let Some(executable) = executable.ok() else {
        return;
    };
    let status = Command::new(executable)
        .args(["--exact", "compilation_allocation_child"])
        .env("TYPOL_REGEX_ALLOCATION_CHILD", "1")
        .status();
    assert!(
        status.is_ok(),
        "allocation-failure child process should start"
    );
    let Some(status) = status.ok() else {
        return;
    };
    assert!(status.success());
}
