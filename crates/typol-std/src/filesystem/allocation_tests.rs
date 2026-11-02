use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    env::{current_exe, temp_dir, var_os},
    error::Error,
    ffi::OsString,
    fs::{File, remove_file},
    io::{Error as IoError, Write},
    path::{Path, PathBuf},
    process::{Command, id},
    ptr::null_mut,
    rc::Rc,
    sync::atomic::{AtomicUsize, Ordering},
};

use opeo::{ErrSlot, OResult, Out};
use typol_core::{
    BudgetError, ByteSource, DiagnosticCode, ExecutionBudget, ExecutionLimits, SourceId,
    SourceReadError, SourceReadErrorKind,
};
use typol_schema::{
    ResolverError, SchemaCompileErrors, SchemaCompileOptions, SchemaCompileStage, SchemaCompiler,
    SchemaResolveRequest, SchemaResolver, SchemaSource, SourceLoadedRequest, SourceObserver,
    SourceObserverError,
};

use crate::{
    FileConfigInputLimits, HostError, HostErrorKind, HostOperation, SourceNameEditOptions,
    SourceNameLimits, SourceNames,
    environment::{EnvironmentBytes, EnvironmentFailure},
    filesystem::{input::FileSchemaInput, paths::PathCandidate},
};

#[cfg(unix)]
use crate::{
    FileSchemaCompileErrors, FileSchemaCompileFailure, FileSchemaCompileOptions,
    FileSchemaCompiler, FileSchemaLimits,
    filesystem::paths::{NativePath, PathResolutionParams},
};

#[cfg(target_os = "linux")]
use std::{
    fs::{create_dir_all, remove_dir_all},
    os::unix::fs::symlink,
};

#[derive(Clone, Copy)]
struct State {
    active: bool,
    fail_at: Option<usize>,
    persistent: bool,
    attempts: usize,
    live_bytes: isize,
    largest_allocation: usize,
}

thread_local! {
    static STATE: Cell<State> = const { Cell::new(State {
        active: false, fail_at: None, persistent: false, attempts: 0, live_bytes: 0,
        largest_allocation: 0,
    }) };
}

struct CheckedAllocator;

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
                stringify!($left), stringify!($right), left, right,
            )).into());
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

fn should_fail() -> bool {
    STATE
        .try_with(|state| {
            let mut current = state.get();
            if !current.active {
                return false;
            }
            let fail = current.fail_at.is_some_and(|index| {
                index == current.attempts || (current.persistent && index < current.attempts)
            });
            current.attempts += 1;
            state.set(current);
            fail
        })
        .unwrap_or(false)
}

fn account(bytes: isize) {
    let _ = STATE.try_with(|state| {
        let mut current = state.get();
        if current.active {
            current.live_bytes += bytes;
            state.set(current);
        }
    });
}

fn record_allocation(size: usize) {
    let _ = STATE.try_with(|state| {
        let mut current = state.get();
        if current.active {
            current.largest_allocation = current.largest_allocation.max(size);
            state.set(current);
        }
    });
}

// SAFETY: Returns a null pointer only during the failure window on the current test thread; all other allocations are delegated to `System` with their original layouts.
unsafe impl GlobalAlloc for CheckedAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record_allocation(layout.size());
        if should_fail() {
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

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record_allocation(size);
        if should_fail() {
            return null_mut();
        }
        let pointer = unsafe { System.realloc(pointer, layout, size) };
        if !pointer.is_null() {
            account(size as isize - layout.size() as isize);
        }
        pointer
    }
}

#[global_allocator]
static ALLOCATOR: CheckedAllocator = CheckedAllocator;

struct Scope;

impl Scope {
    fn start(fail_at: Option<usize>, persistent: bool) -> Self {
        STATE.with(|state| {
            state.set(State {
                active: true,
                fail_at,
                persistent,
                attempts: 0,
                live_bytes: 0,
                largest_allocation: 0,
            })
        });
        Self
    }

    fn finish(self) -> State {
        STATE.with(Cell::get)
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        STATE.with(|state| {
            let mut current = state.get();
            current.active = false;
            state.set(current);
        });
    }
}

fn exercise(path: &Path) -> Result<usize, HostError> {
    let compiler = ErrSlot::<SchemaCompileErrors>::new()
        .call(|out| SchemaCompiler::new(SchemaCompileOptions::default(), out))
        .map_err(|_| HostError::new(HostOperation::SourceNames, HostErrorKind::AllocationFailed))?;
    let context = ErrSlot::<BudgetError>::new()
        .call(|out| compiler.context(out))
        .map_err(|error| HostError::budget(HostOperation::SourceNames, error))?;
    let mut names = ErrSlot::<BudgetError>::new()
        .call(|out| SourceNames::new(SourceNameLimits::default(), &context, out))
        .map_err(|error| HostError::budget(HostOperation::SourceNames, error))?;
    for id in [0, 1, 2, 3, 4, 12] {
        if ErrSlot::<SourceObserverError>::new()
            .call(|out| {
                names.source_loaded(
                    SourceLoadedRequest {
                        source_id: SourceId::new(id),
                        identity: b"ignored",
                        context: &context,
                    },
                    out,
                )
            })
            .is_err()
        {
            let kind = names.failure().ok_or(HostError::new(
                HostOperation::SourceNames,
                HostErrorKind::InternalError,
            ))?;
            return Err(HostError::new(HostOperation::SourceNames, kind));
        }
    }
    drop(compiler);
    ErrSlot::<HostError>::new().call(|out| {
        names.set_name(
            SourceId::new(1),
            "public name",
            SourceNameEditOptions::default(),
            out,
        )
    })?;
    let copy = ErrSlot::<HostError>::new()
        .call(|out| names.try_copy(SourceNameEditOptions::default(), out))?;
    let budget = names_budget()?;
    let mut input = ErrSlot::<HostError>::new().call(|out| {
        FileSchemaInput::open(
            path,
            FileConfigInputLimits::default().max_file_bytes(),
            &budget,
            HostOperation::SchemaRead,
            out,
        )
    })?;
    input.claim();
    let length = stream_source_len(&mut input)?;
    drop(input);
    drop(copy);
    Ok(length)
}

fn stream_source_len(input: &mut FileSchemaInput) -> Result<usize, HostError> {
    let mut buffer = [0; 8192];
    let mut length = 0_usize;
    loop {
        let count = ErrSlot::<SourceReadError>::new()
            .call(|out| input.read(&mut buffer, SourceId::new(0), length, out))
            .map_err(|error| {
                let kind = match error.kind() {
                    SourceReadErrorKind::ResourceLimitExceeded => {
                        HostErrorKind::ResourceLimitExceeded
                    }
                    SourceReadErrorKind::AllocationFailed => HostErrorKind::AllocationFailed,
                    SourceReadErrorKind::CapacityExceeded => HostErrorKind::CapacityExceeded,
                    SourceReadErrorKind::HostFailure => HostErrorKind::ReadFailure,
                    SourceReadErrorKind::InternalInvariantFailure => HostErrorKind::InternalError,
                };
                HostError::new(HostOperation::SchemaRead, kind)
            })?;
        if count == 0 {
            return Ok(length);
        }
        length = length.checked_add(count).ok_or(HostError::new(
            HostOperation::SchemaRead,
            HostErrorKind::ResourceLimitExceeded,
        ))?;
    }
}

static SOURCE_FILE_COUNTER: AtomicUsize = AtomicUsize::new(0);

struct AllocationSourceFile {
    path: PathBuf,
}

impl AllocationSourceFile {
    fn new() -> Result<Self, Box<dyn Error>> {
        let sequence = SOURCE_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = temp_dir().join(format!("typol-std-allocation-{}-{sequence}.tschem", id()));
        let mut file = File::create(&path)?;
        let bytes = [b' '; 32_769];
        file.write_all(&bytes)?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for AllocationSourceFile {
    fn drop(&mut self) {
        let _ = remove_file(&self.path);
    }
}

#[cfg(target_os = "linux")]
struct AllocationPathTree {
    path: PathBuf,
}

#[cfg(target_os = "linux")]
impl AllocationPathTree {
    fn new() -> Result<Self, Box<dyn Error>> {
        let sequence = SOURCE_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = temp_dir().join(format!("typol-std-path-bound-{}-{sequence}", id()));
        create_dir_all(&path)?;
        Ok(Self { path })
    }
}

#[cfg(target_os = "linux")]
impl Drop for AllocationPathTree {
    fn drop(&mut self) {
        let _ = remove_dir_all(&self.path);
    }
}

const TEST_EXECUTION: ExecutionLimits = ExecutionLimits::new(16_777_216, 16_777_216);

fn names_budget() -> Result<ExecutionBudget, HostError> {
    ErrSlot::<BudgetError>::new()
        .call(|out| ExecutionBudget::new(TEST_EXECUTION, out))
        .map_err(|error| HostError::budget(HostOperation::ConfigRead, error))
}

#[test]
fn recoverable_allocations_fail_cleanly_at_every_allocation() -> Result<(), Box<dyn Error>> {
    if var_os("TYPOL_B1_OOM_CHILD").is_none() {
        let output = Command::new(current_exe()?).args(["--exact", "filesystem::allocation_tests::recoverable_allocations_fail_cleanly_at_every_allocation", "--test-threads=1"])
            .env("TYPOL_B1_OOM_CHILD", "1").output()?;
        ensure!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return Ok(());
    }
    let source_file = AllocationSourceFile::new()?;
    let scope = Scope::start(None, false);
    let result = exercise(source_file.path());
    let baseline = scope.finish();
    ensure!(
        result.is_ok(),
        "baseline allocation flow failed: {result:?}"
    );
    let length = result.map_err(|error| IoError::other(error.to_string()))?;
    ensure_eq!(length, 32_769);
    ensure_eq!(baseline.live_bytes, 0);
    for persistent in [false, true] {
        for index in 0..baseline.attempts {
            let scope = Scope::start(Some(index), persistent);
            let result = exercise(source_file.path());
            let state = scope.finish();
            let error = result
                .err()
                .ok_or_else(|| IoError::other("expected allocation to fail"))?;
            ensure_eq!(
                error.kind(),
                HostErrorKind::AllocationFailed,
                "allocation {index} failed; persistent={persistent}"
            );
            ensure_eq!(
                state.live_bytes,
                0,
                "allocation {index} failure must not leak"
            );
        }
    }
    Ok(())
}

#[test]
fn failed_registration_copies_keep_allocation_latch() -> Result<(), Box<dyn Error>> {
    for prefix in [false, true] {
        let compiler = ErrSlot::<SchemaCompileErrors>::new()
            .call(|out| SchemaCompiler::new(SchemaCompileOptions::default(), out))
            .map_err(|errors| {
                IoError::other(format!("Schema initialization failed: {errors:?}"))
            })?;
        let context = ErrSlot::<BudgetError>::new()
            .call(|out| compiler.context(out))
            .map_err(|error| HostError::budget(HostOperation::SourceNames, error))?;
        let mut names = ErrSlot::<BudgetError>::new()
            .call(|out| SourceNames::new(SourceNameLimits::default(), &context, out))
            .map_err(|error| HostError::budget(HostOperation::SourceNames, error))?;
        if prefix {
            ErrSlot::<SourceObserverError>::new()
                .call(|out| {
                    names.source_loaded(
                        SourceLoadedRequest {
                            source_id: SourceId::new(0),
                            identity: b"root",
                            context: &context,
                        },
                        out,
                    )
                })
                .map_err(|error| {
                    IoError::other(format!("source name registration failed: {error:?}"))
                })?;
        }
        let scope = Scope::start(Some(0), true);
        let result = ErrSlot::<SourceObserverError>::new().call(|out| {
            names.source_loaded(
                SourceLoadedRequest {
                    source_id: SourceId::new(1),
                    identity: b"child",
                    context: &context,
                },
                out,
            )
        });
        let state = scope.finish();
        ensure_eq!(result, Err(SourceObserverError::AllocationFailed));
        ensure_eq!(state.live_bytes, 0);
        drop(compiler);
        let mut copy = ErrSlot::<HostError>::new()
            .call(|out| names.try_copy(SourceNameEditOptions::default(), out))?;
        drop(names);
        ensure_eq!(copy.failure(), Some(HostErrorKind::AllocationFailed));
        ensure_eq!(copy.name(SourceId::new(0)), prefix.then_some("schema#0"));
        ensure_eq!(copy.name(SourceId::new(1)), None);
        let error = ErrSlot::<HostError>::new()
            .call(|out| {
                copy.set_name(
                    SourceId::new(0),
                    "alias",
                    SourceNameEditOptions::default(),
                    out,
                )
            })
            .err()
            .ok_or_else(|| IoError::other("expected alias copy to fail"))?;
        ensure_eq!(error.kind(), HostErrorKind::AllocationFailed);
        let error = ErrSlot::<HostError>::new()
            .call(|out| {
                copy.try_copy(
                    SourceNameEditOptions::new(
                        SourceNameLimits::default(),
                        ExecutionLimits::new(0, 0),
                    ),
                    out,
                )
            })
            .err()
            .ok_or_else(|| IoError::other("expected limited name copy to fail"))?;
        ensure_eq!(error.kind(), HostErrorKind::ResourceLimitExceeded);
        ensure_eq!(copy.failure(), Some(HostErrorKind::AllocationFailed));
    }
    Ok(())
}

#[test]
fn failed_alias_allocation_preserves_name_without_observer_latch() -> Result<(), Box<dyn Error>> {
    let compiler = ErrSlot::<SchemaCompileErrors>::new()
        .call(|out| SchemaCompiler::new(SchemaCompileOptions::default(), out))
        .map_err(|errors| IoError::other(format!("Schema initialization failed: {errors:?}")))?;
    let context = ErrSlot::<BudgetError>::new()
        .call(|out| compiler.context(out))
        .map_err(|error| HostError::budget(HostOperation::SourceNames, error))?;
    let mut names = ErrSlot::<BudgetError>::new()
        .call(|out| SourceNames::new(SourceNameLimits::default(), &context, out))
        .map_err(|error| HostError::budget(HostOperation::SourceNames, error))?;
    ErrSlot::<SourceObserverError>::new()
        .call(|out| {
            names.source_loaded(
                SourceLoadedRequest {
                    source_id: SourceId::new(0),
                    identity: b"root",
                    context: &context,
                },
                out,
            )
        })
        .map_err(|error| IoError::other(format!("source name registration failed: {error:?}")))?;
    drop(compiler);
    let scope = Scope::start(Some(0), true);
    let result = ErrSlot::<HostError>::new().call(|out| {
        names.set_name(
            SourceId::new(0),
            "new-name",
            SourceNameEditOptions::default(),
            out,
        )
    });
    let state = scope.finish();
    let error = result
        .err()
        .ok_or_else(|| IoError::other("expected alias allocation to fail"))?;
    ensure_eq!(error.kind(), HostErrorKind::AllocationFailed);
    ensure_eq!(state.live_bytes, 0);
    ensure_eq!(names.failure(), None);
    ensure_eq!(names.name(SourceId::new(0)), Some("schema#0"));
    Ok(())
}

struct NoResolver {
    calls: Rc<Cell<usize>>,
}

impl SchemaResolver for NoResolver {
    fn resolve<'src, 'slot, 'borrow>(
        &'src mut self,
        _request: SchemaResolveRequest<'_>,
        out: Out<'slot, 'borrow, ResolverError>,
    ) -> OResult<'slot, SchemaSource<'src>> {
        self.calls.set(self.calls.get() + 1);
        Err(out.fail(ResolverError::HostFailure))
    }
}

struct InitializationOutcome {
    code: Option<DiagnosticCode>,
    name_missing: bool,
    failure_missing: bool,
}

fn first_diagnostic_code(errors: &SchemaCompileErrors) -> Option<DiagnosticCode> {
    errors
        .diagnostics()
        .first()
        .map(|diagnostic| diagnostic.code())
}

#[test]
fn core_initialization_and_root_copy_failures_do_not_invent_source_names()
-> Result<(), Box<dyn Error>> {
    let calls = Rc::new(Cell::new(0));
    for index in 0..4 {
        calls.set(0);
        let resolver_calls = Rc::clone(&calls);
        let scope = Scope::start(Some(index), true);
        let mut outcome = InitializationOutcome {
            code: None,
            name_missing: true,
            failure_missing: true,
        };
        match ErrSlot::<SchemaCompileErrors>::new()
            .call(|out| SchemaCompiler::new(SchemaCompileOptions::default(), out))
        {
            Err(error) => outcome.code = first_diagnostic_code(&error),
            Ok(compiler) => match ErrSlot::<BudgetError>::new().call(|out| compiler.context(out)) {
                Err(error) => {
                    outcome.code =
                        first_diagnostic_code(&SchemaCompileErrors::budget_failure(error));
                }
                Ok(context) => match ErrSlot::<BudgetError>::new()
                    .call(|out| SourceNames::new(SourceNameLimits::default(), &context, out))
                {
                    Err(error) => {
                        outcome.code =
                            first_diagnostic_code(&SchemaCompileErrors::budget_failure(error));
                    }
                    Ok(mut names) => {
                        let mut resolver = NoResolver {
                            calls: resolver_calls,
                        };
                        outcome.code = ErrSlot::<SchemaCompileErrors>::new()
                            .call(|out| {
                                compiler.compile(
                                    SchemaSource::from_str(b"root", "entry struct Root {}"),
                                    &mut resolver,
                                    Some(&mut names),
                                    out,
                                )
                            })
                            .err()
                            .as_ref()
                            .and_then(first_diagnostic_code);
                        outcome.name_missing = names.name(SourceId::new(0)).is_none();
                        outcome.failure_missing = names.failure().is_none();
                    }
                },
            },
        }
        let state = scope.finish();
        ensure!(
            outcome.name_missing,
            "source name invariant was missing after allocation failure"
        );
        ensure!(
            outcome.failure_missing,
            "observer failure was not latched after allocation failure"
        );
        ensure_eq!(outcome.code, Some(DiagnosticCode::ALLOCATION_FAILED));
        ensure_eq!(calls.get(), 0);
        ensure_eq!(state.live_bytes, 0);
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn recoverable_native_path_copies_and_candidates_release_storage() -> Result<(), Box<dyn Error>> {
    let max_path_bytes = FileSchemaLimits::default().max_path_bytes();
    let budget = names_budget()?;
    let path = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"));
    let file = File::open(path)?;
    let native = ErrSlot::<HostError>::new().call(|out| {
        NativePath::from_opened_file(
            path,
            &file,
            PathResolutionParams {
                max_path_bytes,
                budget: &budget,
                operation: HostOperation::SchemaRead,
            },
            out,
        )
    })?;
    for persistent in [false, true] {
        for index in 0..2 {
            let scope = Scope::start(Some(index), persistent);
            let result = ErrSlot::<HostError>::new()
                .call(|out| native.copy(&budget, HostOperation::SchemaRead, out))
                .and_then(|copy| {
                    ErrSlot::<HostError>::new().call(|out| {
                        copy.candidate(
                            PathCandidate {
                                module: "child",
                                alternate: true,
                            },
                            max_path_bytes,
                            &budget,
                            out,
                        )
                    })
                });
            let kind = result.err().map(|error| error.kind());
            let state = scope.finish();
            ensure_eq!(kind, Some(HostErrorKind::AllocationFailed));
            ensure_eq!(state.live_bytes, 0);
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn opened_path_limit_bounds_handle_path_buffer_allocation() -> Result<(), Box<dyn Error>> {
    let tree = AllocationPathTree::new()?;
    let target_directory = tree.path.join("a".repeat(90)).join("b".repeat(90));
    create_dir_all(&target_directory)?;
    let target = target_directory.join("target.tschem");
    File::create(&target)?;
    let link = tree.path.join("link.tschem");
    symlink(&target, &link)?;
    let path_limit = link.as_os_str().as_encoded_bytes().len() + 8;
    ensure!(
        target.as_os_str().as_encoded_bytes().len() > path_limit,
        "test target path must exceed the limit"
    );
    let limits = FileSchemaLimits::default().with_max_path_bytes(path_limit);
    let budget = names_budget()?;
    let file = File::open(&link)?;
    let scope = Scope::start(None, false);
    let result = ErrSlot::<HostError>::new().call(|out| {
        NativePath::from_opened_file(
            &link,
            &file,
            PathResolutionParams {
                max_path_bytes: limits.max_path_bytes(),
                budget: &budget,
                operation: HostOperation::SchemaRead,
            },
            out,
        )
    });
    let state = scope.finish();
    ensure_eq!(
        result.err().map(|error| error.kind()),
        Some(HostErrorKind::ResourceLimitExceeded)
    );
    ensure!(
        state.largest_allocation <= path_limit + 1,
        "canonicalization buffer allocation exceeded the path budget"
    );
    ensure_eq!(state.live_bytes, 0);
    Ok(())
}

#[test]
fn environment_native_storage_is_charged_and_conversion_failures_are_recoverable()
-> Result<(), Box<dyn Error>> {
    #[cfg(unix)]
    for persistent in [false, true] {
        let budget = names_budget()?;
        let overhead = budget.workspace_bytes();
        let native = OsString::from("value");
        let native_capacity = native.capacity();
        let scope = Scope::start(Some(0), persistent);
        let result = ErrSlot::<EnvironmentFailure>::new()
            .call(|out| EnvironmentBytes::from_native(native, 5, &budget, out));
        let state = scope.finish();
        ensure!(
            result.is_ok(),
            "taking ownership of the native Unix environment buffer should not allocate again"
        );
        ensure_eq!(state.attempts, 0);
        ensure_eq!(state.live_bytes, 0);
        ensure_eq!(budget.workspace_bytes(), overhead + native_capacity);
        drop(result);
        ensure_eq!(budget.workspace_bytes(), overhead);
    }
    #[cfg(windows)]
    for persistent in [false, true] {
        let budget = names_budget()?;
        let overhead = budget.workspace_bytes();
        let native = OsString::from("value");
        let native_capacity = native.capacity();
        let scope = Scope::start(Some(0), persistent);
        let result = ErrSlot::<EnvironmentFailure>::new()
            .call(|out| EnvironmentBytes::from_native(native, 5, &budget, out));
        let state = scope.finish();
        ensure!(matches!(
            result,
            Err(EnvironmentFailure::Budget(BudgetError::Allocation))
        ));
        ensure_eq!(state.live_bytes, -(native_capacity as isize));
        ensure_eq!(budget.workspace_bytes(), overhead);
    }
    #[cfg(windows)]
    {
        for persistent in [false, true] {
            let budget = names_budget()?;
            let overhead = budget.workspace_bytes();
            let native = OsString::from("Ḁ😀");
            let native_capacity = native.capacity();
            let scope = Scope::start(Some(0), persistent);
            let result = ErrSlot::<EnvironmentFailure>::new()
                .call(|out| EnvironmentBytes::from_native(native, 7, &budget, out));
            let state = scope.finish();
            ensure!(matches!(
                result,
                Err(EnvironmentFailure::Budget(BudgetError::Allocation))
            ));
            // The native buffer is allocated before the failure window but freed inside it by the conversion-failure branch.
            ensure_eq!(state.live_bytes, -(native_capacity as isize));
            ensure_eq!(budget.workspace_bytes(), overhead);
        }
    }
    Ok(())
}

#[test]
fn shared_host_storage_matches_live_layout_payloads() -> Result<(), Box<dyn Error>> {
    let source_file = AllocationSourceFile::new()?;
    let scope = Scope::start(None, false);
    let compiler = ErrSlot::<SchemaCompileErrors>::new()
        .call(|out| SchemaCompiler::new(SchemaCompileOptions::default(), out))
        .map_err(|errors| IoError::other(format!("Schema initialization failed: {errors:?}")))?;
    let context = ErrSlot::<BudgetError>::new()
        .call(|out| compiler.context(out))
        .map_err(|error| HostError::budget(HostOperation::SourceNames, error))?;
    let mut names = ErrSlot::<BudgetError>::new()
        .call(|out| SourceNames::new(SourceNameLimits::default(), &context, out))
        .map_err(|error| HostError::budget(HostOperation::SourceNames, error))?;
    for id in 0..9 {
        ErrSlot::<SourceObserverError>::new()
            .call(|out| {
                names.source_loaded(
                    SourceLoadedRequest {
                        source_id: SourceId::new(id),
                        identity: b"root",
                        context: &context,
                    },
                    out,
                )
            })
            .map_err(|error| {
                IoError::other(format!("source name registration failed: {error:?}"))
            })?;
        ensure_eq!(
            STATE.with(Cell::get).live_bytes as usize,
            context.usage().workspace_bytes
        );
    }
    let mut input = ErrSlot::<HostError>::new().call(|out| {
        FileSchemaInput::open(
            source_file.path(),
            FileConfigInputLimits::default().max_file_bytes(),
            context.execution(),
            HostOperation::SchemaRead,
            out,
        )
    })?;
    input.claim();
    ensure_eq!(stream_source_len(&mut input)?, 32_769);
    ensure_eq!(
        STATE.with(Cell::get).live_bytes as usize,
        context.usage().workspace_bytes
    );
    let held = context.usage().workspace_bytes;
    drop(input);
    ensure_eq!(context.usage().workspace_bytes, held);
    ensure_eq!(
        STATE.with(Cell::get).live_bytes as usize,
        context.usage().workspace_bytes
    );
    drop(names);
    drop(context);
    drop(compiler);
    ensure_eq!(scope.finish().live_bytes, 0);
    Ok(())
}

#[cfg(unix)]
#[test]
fn file_entry_initialization_oom_precedes_native_io() -> Result<(), Box<dyn Error>> {
    for index in 0..2 {
        for persistent in [false, true] {
            let scope = Scope::start(Some(index), persistent);
            let result = ErrSlot::<FileSchemaCompileErrors>::new().call(|out| {
                FileSchemaCompiler::compile(
                    Path::new("/path-does-not-exist.tschem"),
                    FileSchemaCompileOptions::default(),
                    out,
                )
            });
            let error = result
                .err()
                .ok_or_else(|| IoError::other("expected file entry initialization to fail"))?;
            ensure!(
                matches!(error.failure(), FileSchemaCompileFailure::Compilation(error) if error.diagnostics().first().is_some_and(|diagnostic| diagnostic.code() == DiagnosticCode::ALLOCATION_FAILED))
            );
            ensure!(error.names().is_none());
            let usage = error.usage();
            ensure_eq!(usage.last_stage, SchemaCompileStage::Initialization);
            ensure_eq!(usage.failure, Some(BudgetError::Allocation));
            if index == 0 {
                ensure_eq!(usage.workspace_bytes, 0);
                ensure_eq!(usage.peak_workspace_bytes, 0);
            } else {
                ensure!(usage.workspace_bytes > 0);
                ensure!(usage.peak_workspace_bytes >= usage.workspace_bytes);
            }
            drop(error);
            ensure_eq!(scope.finish().live_bytes, 0);
        }
    }
    Ok(())
}
