use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    collections::HashMap,
    env::{current_exe, var_os},
    path::Path,
    process::Command,
    ptr::null_mut,
};
use typol::{
    ConfigInput, ConfigParseOptions, Environment, EnvironmentError, EnvironmentReadContext,
    LoadedSchema, SchemaLoader, Typol, TypolError, TypolOptions,
    binding::BudgetedTypol,
    core::{DiagnosticCode, ExecutionLimits},
    host::HostErrorKind,
    opeo::{OResult, Out},
    schema::{SchemaCompileOptions, SchemaLimits},
};
use typol_core::ExcerptLimits;
use typol_test_support::{
    TestExtract, TestOpeo, TestOpeoOutcome, TestResult, test_verify as assert,
    test_verify_eq as assert_eq,
};

#[derive(Clone, Copy, Default)]
struct State {
    active: bool,
    attempts: usize,
    fail_at: Option<usize>,
    persistent: bool,
    live: isize,
    peak: isize,
}

thread_local! {
    static STATE: Cell<State> = const { Cell::new(State {
        active: false, attempts: 0, fail_at: None, persistent: false, live: 0, peak: 0,
    }) };
}

struct Allocator;

fn reject() -> bool {
    STATE
        .try_with(|cell| {
            let mut state = cell.get();
            if !state.active {
                return false;
            }
            let fail = state.fail_at.is_some_and(|index| {
                state.attempts == index || (state.persistent && state.attempts > index)
            });
            state.attempts += 1;
            cell.set(state);
            fail
        })
        .unwrap_or(false)
}

fn account(bytes: isize) {
    let _ = STATE.try_with(|cell| {
        let mut state = cell.get();
        if state.active {
            state.live += bytes;
            state.peak = state.peak.max(state.live);
            cell.set(state);
        }
    });
}

// SAFETY: Preserves the original `System` layout; the fault window rejects only recoverable allocations on the current thread.
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if reject() {
            return null_mut();
        }
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            account(layout.size() as isize);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        account(-(layout.size() as isize));
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, bytes: usize) -> *mut u8 {
        if reject() {
            return null_mut();
        }
        let pointer = unsafe { System.realloc(pointer, layout, bytes) };
        if !pointer.is_null() {
            account(bytes as isize - layout.size() as isize);
        }
        pointer
    }
}

#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

struct Scope;

impl Scope {
    fn start(fail_at: Option<usize>, persistent: bool) -> Self {
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

    fn state(&self) -> State {
        STATE.with(Cell::get)
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        STATE.with(|cell| {
            cell.set(State {
                active: false,
                ..cell.get()
            })
        });
    }
}

struct Empty;

impl Environment for Empty {
    fn read<'slot, 'borrow, 'env>(
        &'env mut self,
        _: &str,
        _: EnvironmentReadContext<'_>,
        _: Out<'slot, 'borrow, EnvironmentError>,
    ) -> OResult<'slot, Option<&'env [u8]>> {
        Ok(None)
    }
}

#[derive(Debug, PartialEq, Typol)]
struct Root {
    value: HashMap<String, String>,
}

fn compile(text: &str) -> TestResult<LoadedSchema> {
    let loader = SchemaLoader::new(TypolOptions::default());
    TestOpeo::value(|out| loader.compile_text(b"root", text, out))
}

fn allocation_test_options() -> TypolOptions {
    let mut limits = SchemaLimits::default();
    limits.set_excerpts(ExcerptLimits::new(0, 0, 0, 0));
    TypolOptions::default().with_schema(SchemaCompileOptions::new(limits))
}

fn read_typed_with_environment<'slot, T: BudgetedTypol>(
    loaded: &LoadedSchema,
    path: &Path,
    environment: &mut dyn Environment,
    mut out: Out<'slot, '_, TypolError>,
) -> OResult<'slot, T> {
    loaded
        .bind::<T>(out.reborrow())?
        .read(ConfigInput::file(path), environment, out)
}

fn allocation_code(errors: &TypolError) -> bool {
    errors
        .diagnostics()
        .iter()
        .any(|d| d.code() == DiagnosticCode::ALLOCATION_FAILED)
        || errors
            .host_error()
            .is_some_and(|e| e.kind() == HostErrorKind::AllocationFailed)
        || errors.names_failure() == Some(HostErrorKind::AllocationFailed)
}

fn injected_error_code<T>(outcome: TestOpeoOutcome<T, TypolError>) -> Option<DiagnosticCode> {
    match outcome {
        TestOpeoOutcome::Value(_) => None,
        TestOpeoOutcome::Error(errors) if allocation_code(&errors) => {
            Some(DiagnosticCode::ALLOCATION_FAILED)
        }
        TestOpeoOutcome::Error(errors) => Some(
            errors
                .diagnostics()
                .first()
                .map_or(DiagnosticCode::INTERNAL_ERROR, |diagnostic| {
                    diagnostic.code()
                }),
        ),
    }
}

fn unit_outcome<T>(outcome: TestOpeoOutcome<T, TypolError>) -> TestOpeoOutcome<(), TypolError> {
    match outcome {
        TestOpeoOutcome::Value(_) => TestOpeoOutcome::Value(()),
        TestOpeoOutcome::Error(errors) => TestOpeoOutcome::Error(errors),
    }
}

fn atomic(mut run: impl FnMut() -> TestOpeoOutcome<(), TypolError>) -> TestResult {
    let scope = Scope::start(None, false);
    let result = run();
    let succeeded = matches!(&result, TestOpeoOutcome::Value(()));
    drop(result);
    let baseline = scope.state();
    drop(scope);
    assert!(succeeded);
    assert_eq!(baseline.live, 0);
    assert!(baseline.attempts > 2);
    for persistent in [false, true] {
        for index in 0..baseline.attempts {
            let scope = Scope::start(Some(index), persistent);
            let error_code = injected_error_code(run());
            let state = scope.state();
            drop(scope);
            assert!(
                error_code.is_none_or(|code| code == DiagnosticCode::ALLOCATION_FAILED),
                "allocation point {index}, persistent={persistent}, diagnostic code={error_code:?}"
            );
            assert_eq!(
                state.live, 0,
                "allocation point {index}, persistent={persistent}"
            );
        }
    }
    Ok(())
}

#[test]
fn loading_allocation_failures_preserve_errors_and_release_payloads() -> TestResult {
    if var_os("TYPOL_HOST_OOM_CHILD").is_none() {
        let status = Command::new(current_exe().test_value()?)
            .args([
                "--exact",
                "loading_allocation_failures_preserve_errors_and_release_payloads",
                "--test-threads=1",
            ])
            .env("TYPOL_HOST_OOM_CHILD", "1")
            .status()
            .test_value()?;
        assert!(status.success());
        return Ok(());
    }
    atomic(|| {
        let loader = SchemaLoader::new(allocation_test_options());
        match TestOpeo::outcome(|out| {
            loader.compile_text(
                b"root",
                "entry struct Root { value: map<string, string> }",
                out,
            )
        }) {
            TestOpeoOutcome::Value(_) => TestOpeoOutcome::Value(()),
            TestOpeoOutcome::Error(errors) => TestOpeoOutcome::Error(errors),
        }
    })?;
    let loaded = compile("entry struct Root { value: map<string, string> }")?;
    // Binding is guaranteed to fail, exercising the error slot, binding errors, and copied names before native file operations.
    let scope = Scope::start(None, false);
    let errors = TestOpeo::error(|out| {
        read_typed_with_environment::<bool>(
            &loaded,
            Path::new("/must-not-read.tconf"),
            &mut Empty,
            out,
        )
    })?;
    let attempts = scope.state().attempts;
    drop(errors);
    assert_eq!(scope.state().live, 0);
    drop(scope);
    for persistent in [false, true] {
        for index in 0..attempts {
            let scope = Scope::start(Some(index), persistent);
            let errors = TestOpeo::error(|out| {
                read_typed_with_environment::<bool>(
                    &loaded,
                    Path::new("/must-not-read.tconf"),
                    &mut Empty,
                    out,
                )
            })?;
            let is_allocation = allocation_code(&errors);
            let binding_retained = errors
                .diagnostics()
                .iter()
                .any(|d| d.code() == DiagnosticCode::TYPE_BINDING_ERROR);
            drop(errors);
            let state = scope.state();
            drop(scope);
            assert!(is_allocation || binding_retained);
            assert_eq!(state.live, 0);
        }
    }
    let defaults = TypolOptions::default();
    let config = ConfigParseOptions::default().with_execution(ExecutionLimits::new(
        0,
        defaults.config().execution().max_workspace_bytes(),
    ));
    let options = defaults.with_config(config);
    let loader = SchemaLoader::new(options);
    let limited = TestOpeo::value(|out| {
        loader.compile_text(
            b"root",
            "entry struct Root { value: map<string, string> }",
            out,
        )
    })?;
    for typed in [false, true] {
        let run = || {
            if typed {
                unit_outcome(TestOpeo::outcome(|out| {
                    read_typed_with_environment::<Root>(
                        &limited,
                        Path::new("/must-not-read.tconf"),
                        &mut Empty,
                        out,
                    )
                }))
            } else {
                unit_outcome(TestOpeo::outcome(|out| {
                    limited.read_value(
                        ConfigInput::file(Path::new("/must-not-read.tconf")),
                        &mut Empty,
                        out,
                    )
                }))
            }
        };
        let scope = Scope::start(None, false);
        let errors = match run() {
            TestOpeoOutcome::Value(_) => {
                return Err("a zero-step budget should fail before reading".into());
            }
            TestOpeoOutcome::Error(errors) => errors,
        };
        let attempts = scope.state().attempts;
        drop(errors);
        assert_eq!(scope.state().live, 0);
        drop(scope);
        for persistent in [false, true] {
            for index in 0..attempts {
                let scope = Scope::start(Some(index), persistent);
                let error_code = injected_error_code(run());
                let state = scope.state();
                drop(scope);
                assert_eq!(
                    error_code,
                    Some(DiagnosticCode::ALLOCATION_FAILED),
                    "config setup allocation point {index}, persistent={persistent}"
                );
                assert_eq!(state.live, 0);
            }
        }
    }
    assert_eq!(loaded.source_names().failure(), None);
    Ok(())
}
