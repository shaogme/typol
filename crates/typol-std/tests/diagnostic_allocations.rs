#[macro_use]
#[path = "support/assertions.rs"]
mod assertions;

use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    env::{current_exe, var_os},
    error::Error,
    process::Command,
    ptr::null_mut,
};

use opeo::ErrSlot;
use typol_core::{BudgetError, Diagnostic, DiagnosticCode, ExecutionLimits, SourceExcerptSet};
use typol_i18n::{Language, Translator};
use typol_std::{
    DiagnosticContext, DiagnosticRenderError, DiagnosticRenderer, DisplayError, RenderOptions,
};

static UNAVAILABLE_TEST_EXCERPTS: SourceExcerptSet = SourceExcerptSet::unavailable();

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

// SAFETY: Delegates all allocations and deallocations to `System` with their original layouts; only the current thread’s allocations are recorded or rejected.
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
        unsafe { System.dealloc(pointer, layout) };
    }
}

#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

struct Measurement {
    state: State,
    error: Option<DiagnosticRenderError>,
    output_capacity: usize,
    retained: usize,
}

fn measure(
    diagnostics: &[Diagnostic],
    fail_at: Option<usize>,
    persistent: bool,
    options: RenderOptions,
) -> Measurement {
    STATE.with(|cell| {
        cell.set(State {
            active: true,
            fail_at,
            persistent,
            ..State::default()
        })
    });
    let result = ErrSlot::<DiagnosticRenderError>::new().call(|out| {
        DiagnosticRenderer::render(
            diagnostics,
            DiagnosticContext {
                source_name: &|_| None,
                span_location: &|_| None,
                excerpts: &UNAVAILABLE_TEST_EXCERPTS,
            },
            options,
            Translator::new(Language::English),
            out,
        )
    });
    let output_capacity = result.as_ref().map_or(0, String::capacity);
    let retained = STATE.with(|cell| cell.get().live);
    let error = result.as_ref().err().copied();
    drop(result);
    let state = STATE.with(Cell::get);
    STATE.with(|cell| {
        cell.set(State {
            active: false,
            ..cell.get()
        })
    });
    Measurement {
        state,
        error,
        output_capacity,
        retained,
    }
}

#[test]
fn each_single_and_persistent_allocation_failure_releases_the_entire_output()
-> Result<(), Box<dyn Error>> {
    if var_os("TYPOL_RENDER_OOM_CHILD").is_none() {
        let status = Command::new(current_exe()?)
            .args([
                "--exact",
                "each_single_and_persistent_allocation_failure_releases_the_entire_output",
                "--test-threads=1",
            ])
            .env("TYPOL_RENDER_OOM_CHILD", "1")
            .status()?;
        ensure!(status.success());
        return Ok(());
    }
    let message = "Ḁḁ\u{1b}\n".repeat(1024);
    let diagnostics = [ErrSlot::<BudgetError>::new().call(|out| {
        Diagnostic::try_new(DiagnosticCode::CONFIG_VALIDATION_ERROR, &message, None, out)
    })?];
    let baseline = measure(&diagnostics, None, false, RenderOptions::default());
    ensure!(baseline.error.is_none());
    ensure_eq!(baseline.state.live, 0);
    ensure_eq!(baseline.retained, baseline.output_capacity);
    ensure!(baseline.state.peak > baseline.retained);
    ensure!(baseline.state.attempts > 4);
    let exact = RenderOptions::new(
        ExecutionLimits::new(
            RenderOptions::default().execution().max_steps(),
            baseline.state.peak,
        ),
        RenderOptions::default().max_output_bytes(),
    );
    ensure!(measure(&diagnostics, None, false, exact).error.is_none());
    let short = RenderOptions::new(
        ExecutionLimits::new(exact.execution().max_steps(), baseline.state.peak - 1),
        exact.max_output_bytes(),
    );
    let failed = measure(&diagnostics, None, false, short);
    ensure_eq!(
        failed.error,
        Some(DiagnosticRenderError::Display(
            DisplayError::ResourceLimitExceeded
        ))
    );
    ensure_eq!(failed.state.live, 0);
    for index in 0..baseline.state.attempts {
        for persistent in [false, true] {
            let failed = measure(
                &diagnostics,
                Some(index),
                persistent,
                RenderOptions::default(),
            );
            ensure_eq!(
                failed.error,
                Some(DiagnosticRenderError::Display(
                    DisplayError::AllocationFailed
                )),
                "allocation point {index}"
            );
            ensure_eq!(
                failed.state.live,
                0,
                "allocation point {index}; persistent failure {persistent}"
            );
            ensure_eq!(failed.retained, 0);
        }
    }
    ensure_eq!(
        diagnostics[0].code(),
        DiagnosticCode::CONFIG_VALIDATION_ERROR
    );

    Ok(())
}
