#[path = "support/clippy.rs"]
mod clippy;

use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    ptr::null_mut,
    thread_local,
};

use typol_core::{BudgetError, ExecutionLimits};
use typol_opeo_test_support::TestSlot;
use typol_policy::RegexLimits;
use typol_regex::{RegexCompileError, RegexCompiler, RegexScratch, RegexScratchError};

struct FailNextAllocation;

thread_local! {
    static FAIL_NEXT_ALLOCATION: Cell<bool> = const { Cell::new(false) };
}

fn should_fail() -> bool {
    FAIL_NEXT_ALLOCATION
        .try_with(|fail_next| fail_next.replace(false))
        .unwrap_or(false)
}

unsafe impl GlobalAlloc for FailNextAllocation {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if should_fail() {
            null_mut()
        } else {
            // The layout comes from Vec's allocator and preserves the memory layout defined by the system allocator.
            unsafe { System.alloc(layout) }
        }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // Both the pointer and layout were previously returned by the system allocator.
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if should_fail() {
            null_mut()
        } else {
            // Both the pointer and layout were previously returned by the system allocator.
            unsafe { System.realloc(pointer, layout, new_size) }
        }
    }
}

#[global_allocator]
static ALLOCATOR: FailNextAllocation = FailNextAllocation;

#[test]
fn reports_compile_and_scratch_allocation_failures() {
    let mut budget_errors = TestSlot::<BudgetError>::default();
    let Some(compilation) = clippy::Clippy::succeed_opeo(
        clippy::Clippy::regex_execution_budget(
            &mut budget_errors,
            ExecutionLimits::new(67_108_864, 67_108_864),
        ),
        "compilation budget should be created",
    ) else {
        return;
    };
    let limits = RegexLimits::default();
    let mut compile_errors = TestSlot::<RegexCompileError>::default();
    let mut scratch_errors = TestSlot::<RegexScratchError>::default();

    FAIL_NEXT_ALLOCATION.with(|fail_next| fail_next.set(true));
    assert!(matches!(
        clippy::Clippy::attempt(&mut compile_errors, |out| {
            RegexCompiler::compile("a", &limits, &compilation, out)
        }),
        clippy::Attempt::Failure(RegexCompileError::AllocationFailed)
    ));

    let Some(regex) = clippy::Clippy::succeed_opeo(
        clippy::Clippy::attempt(&mut compile_errors, |out| {
            RegexCompiler::compile("a", &limits, &compilation, out)
        }),
        "compilation should succeed after allocator recovery",
    ) else {
        return;
    };
    FAIL_NEXT_ALLOCATION.with(|fail_next| fail_next.set(true));
    assert!(matches!(
        clippy::Clippy::attempt(&mut scratch_errors, |out| {
            RegexScratch::new(&regex, &limits, &compilation, out)
        }),
        clippy::Attempt::Failure(RegexScratchError::AllocationFailed)
    ));
}
