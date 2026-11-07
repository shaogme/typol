#[macro_use]
#[path = "support/assertions.rs"]
mod assertions;

use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    env::{current_exe, temp_dir, var_os},
    error::Error,
    fs::{create_dir, remove_dir_all, write},
    io,
    path::{Path, PathBuf},
    process::{Command, id},
    ptr,
    time::Instant,
};

use typol_core::{
    BudgetError, DiagnosticCode, ExcerptLimits, ExecutionLimits, RegexExecutionBudget, SourceId,
};
use typol_schema::{SchemaCompileOptions, SchemaCompileStage, SchemaCompileUsage, SchemaLimits};
use typol_std::{
    FileSchemaCompileErrors, FileSchemaCompileOptions, FileSchemaCompiler, FileSchemaLimits,
    SourceNameLimits, opeo::ErrSlot,
};

#[derive(Clone, Copy)]
struct AllocationState {
    active: bool,
    live: isize,
    peak: isize,
    allocation_attempts: usize,
    fail_on_attempt: Option<usize>,
}

thread_local! {
    static STATE: Cell<AllocationState> = const { Cell::new(AllocationState { active: false, live: 0, peak: 0, allocation_attempts: 0, fail_on_attempt: None }) };
}

struct Allocator;

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

fn should_fail_allocation() -> bool {
    STATE
        .try_with(|state| {
            let mut current = state.get();
            if !current.active {
                return false;
            }
            current.allocation_attempts += 1;
            let should_fail = current.fail_on_attempt == Some(current.allocation_attempts);
            state.set(current);
            should_fail
        })
        .unwrap_or(false)
}

// SAFETY: Always delegates allocations and deallocations to `System` with their original layouts; only the current thread’s payload is recorded.
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if should_fail_allocation() {
            return ptr::null_mut();
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
        let pointer = unsafe { System.realloc(pointer, layout, bytes) };
        if !pointer.is_null() {
            account(bytes as isize - layout.size() as isize);
        }
        pointer
    }
}

#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

struct Directory {
    path: PathBuf,
}

fn regex_budget_state_bytes() -> Result<usize, Box<dyn Error>> {
    STATE.with(|state| {
        state.set(AllocationState {
            active: true,
            live: 0,
            peak: 0,
            allocation_attempts: 0,
            fail_on_attempt: None,
        })
    });
    let budget = ErrSlot::<BudgetError>::new().call(|out| RegexExecutionBudget::new(1, 1, 1, out));
    let bytes = match budget {
        Ok(budget) => {
            let bytes = STATE.with(Cell::get).live as usize;
            drop(budget);
            bytes
        }
        Err(error) => {
            STATE.with(|state| {
                state.set(AllocationState {
                    active: false,
                    ..state.get()
                })
            });
            return Err(
                io::Error::other(format!("Regex budget initialization failed: {error:?}")).into(),
            );
        }
    };
    let state = STATE.with(Cell::get);
    STATE.with(|state| {
        state.set(AllocationState {
            active: false,
            ..state.get()
        })
    });
    ensure_eq!(state.live, 0);
    Ok(bytes)
}

impl Directory {
    fn new(count: usize) -> Result<Self, Box<dyn Error>> {
        let path = temp_dir().join(format!("typol-b1-report-{}-{count:02}", id()));
        create_dir(&path)?;
        let mut root = String::new();
        for index in 0..count {
            root.push_str(&format!("extern mod child_{index:02};"));
            write(
                path.join(format!("child_{index:02}.tschem")),
                format!("{}struct Child {{ value: u8 }}", " ".repeat(1024)),
            )?;
        }
        root.push_str("entry struct Root {}");
        write(path.join("root.tschem"), root)?;
        Ok(Self { path })
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = remove_dir_all(&self.path);
    }
}

struct Measurement {
    usage: SchemaCompileUsage,
    heap_peak: usize,
    heap_retained: usize,
    elapsed_ns: u128,
    name_matches: bool,
    fully_released: bool,
    retained_matches_usage: bool,
}

fn measure(path: &Path, count: usize) -> Result<Measurement, Box<dyn Error>> {
    let expected = format!("schema#{count}");
    let regex_state_bytes = regex_budget_state_bytes()?;
    STATE.with(|state| {
        state.set(AllocationState {
            active: true,
            live: 0,
            peak: 0,
            allocation_attempts: 0,
            fail_on_attempt: None,
        })
    });
    let start = Instant::now();
    let result = ErrSlot::<FileSchemaCompileErrors>::new()
        .call(|out| FileSchemaCompiler::compile(path, FileSchemaCompileOptions::default(), out));
    let elapsed_ns = start.elapsed().as_nanos();
    let usage = result
        .as_ref()
        .map_err(|error| io::Error::other(format!("Schema compilation failed: {error:?}")))?
        .usage();
    let retained = STATE.with(Cell::get);
    let name_matches = result
        .as_ref()
        .map_err(|error| io::Error::other(format!("Schema compilation failed: {error:?}")))?
        .names()
        .name(SourceId::new(count as u32))
        == Some(expected.as_str());
    drop(result);
    let finished = STATE.with(Cell::get);
    STATE.with(|state| {
        state.set(AllocationState {
            active: false,
            ..state.get()
        })
    });
    Ok(Measurement {
        usage,
        heap_peak: finished.peak as usize,
        heap_retained: retained.live as usize,
        elapsed_ns,
        name_matches,
        fully_released: finished.live == 0,
        retained_matches_usage: retained.live as usize
            == usage.workspace_bytes + usage.regex_workspace_bytes + regex_state_bytes,
    })
}

#[test]
fn regex_budget_initialization_allocation_failure_is_reported_in_regex_usage()
-> Result<(), Box<dyn Error>> {
    STATE.with(|state| {
        state.set(AllocationState {
            active: true,
            live: 0,
            peak: 0,
            allocation_attempts: 0,
            fail_on_attempt: Some(2),
        })
    });
    let result = ErrSlot::<FileSchemaCompileErrors>::new().call(|out| {
        FileSchemaCompiler::compile(
            Path::new("/path-does-not-exist/regex-budget-init.tschem"),
            FileSchemaCompileOptions::new(
                SchemaCompileOptions::default(),
                FileSchemaLimits::default(),
                SourceNameLimits::default(),
            ),
            out,
        )
    });
    let allocation = STATE.with(Cell::get);
    STATE.with(|state| {
        state.set(AllocationState {
            active: false,
            ..state.get()
        })
    });

    ensure!(result.is_err());
    ensure_eq!(allocation.allocation_attempts, 2);
    ensure_eq!(allocation.live, 0);
    let error = result
        .err()
        .ok_or_else(|| io::Error::other("the Regex budget state allocation should have failed"))?;
    let usage = error.usage();
    ensure_eq!(usage.last_stage, SchemaCompileStage::Initialization);
    ensure_eq!(usage.failure, None);
    ensure_eq!(usage.regex_failure, Some(BudgetError::Allocation));
    ensure_eq!(usage.workspace_bytes, 0);
    ensure_eq!(usage.regex_workspace_bytes, 0);
    ensure_eq!(
        error
            .diagnostics()
            .ok_or_else(|| io::Error::other("compilation diagnostics are missing"))?
            .first()
            .map(|diagnostic| diagnostic.code()),
        Some(DiagnosticCode::ALLOCATION_FAILED)
    );
    Ok(())
}

#[test]
fn file_compilation_releases_all_tracked_payloads_and_reports_growth() -> Result<(), Box<dyn Error>>
{
    if var_os("TYPOL_B1_REPORT_CHILD").is_none() {
        let output = Command::new(current_exe()?)
            .args([
                "--exact",
                "file_compilation_releases_all_tracked_payloads_and_reports_growth",
                "--test-threads=1",
            ])
            .env("TYPOL_B1_REPORT_CHILD", "1")
            .output()?;
        ensure!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return Ok(());
    }
    let report = var_os("TYPOL_B1_PROFILE_PATH");
    let mut rows = String::from(
        "files\tsteps\tbudget_peak\tbudget_retained\tactual_heap_peak\tactual_heap_retained\telapsed_ns\n",
    );
    let mut previous = None;
    for count in [8, 16, 32] {
        let directory = Directory::new(count)?;
        let path = directory.path.join("root.tschem");
        for _ in 0..2 {
            let sample = measure(&path, count)?;
            ensure!(sample.name_matches);
            ensure!(sample.fully_released);
            ensure!(sample.retained_matches_usage);
        }
        let mut samples = Vec::new();
        for _ in 0..if report.is_some() { 31 } else { 1 } {
            let sample = measure(&path, count)?;
            ensure!(sample.name_matches);
            ensure!(sample.fully_released);
            ensure!(sample.retained_matches_usage);
            samples.push(sample);
        }
        let sample = samples
            .first()
            .ok_or_else(|| io::Error::other("allocation sample is missing"))?;
        for other in &samples {
            ensure_eq!(other.usage, sample.usage);
            ensure_eq!(other.heap_retained, sample.heap_retained);
        }
        let mut times: Vec<_> = samples.iter().map(|sample| sample.elapsed_ns).collect();
        times.sort_unstable();
        rows.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            count + 1,
            sample.usage.steps,
            sample.usage.peak_workspace_bytes,
            sample.usage.workspace_bytes,
            sample.heap_peak,
            sample.heap_retained,
            *times
                .get(times.len() / 2)
                .ok_or_else(|| io::Error::other("median duration is missing"))?
        ));
        if let Some(previous) = previous {
            ensure!(sample.usage.steps > previous && sample.usage.steps < previous * 4);
        }
        previous = Some(sample.usage.steps);
        // The exact failure boundary excludes the optional snapshot, while payload accounting still includes the snapshot that is enabled by default.
        let mut resources = SchemaLimits::default();
        resources.set_excerpts(ExcerptLimits::default().with_max_bytes(0));
        let required_options = SchemaCompileOptions::new(resources);
        let required_steps = ErrSlot::<FileSchemaCompileErrors>::new()
            .call(|out| {
                FileSchemaCompiler::compile(
                    &path,
                    FileSchemaCompileOptions::new(
                        required_options,
                        FileSchemaLimits::default(),
                        SourceNameLimits::default(),
                    ),
                    out,
                )
            })?
            .usage()
            .steps;
        let errors = ErrSlot::<FileSchemaCompileErrors>::new()
            .call(|out| {
                FileSchemaCompiler::compile(
                    &path,
                    FileSchemaCompileOptions::new(
                        required_options
                            .with_execution(ExecutionLimits::new(required_steps - 1, 67_108_864)),
                        FileSchemaLimits::default(),
                        SourceNameLimits::default(),
                    ),
                    out,
                )
            })
            .err()
            .ok_or_else(|| io::Error::other("expected operation to fail"))?;
        ensure!(errors.usage().failure.is_some());
    }
    if let Some(path) = report {
        write(path, rows)?;
    }

    Ok(())
}

#[test]
fn original_fixtures_use_the_complete_file_session_defaults() -> Result<(), Box<dyn Error>> {
    if var_os("TYPOL_B5_FIXTURE_CHILD").is_none() {
        let output = Command::new(current_exe()?)
            .args([
                "--exact",
                "original_fixtures_use_the_complete_file_session_defaults",
                "--test-threads=1",
            ])
            .env("TYPOL_B5_FIXTURE_CHILD", "1")
            .output()?;
        ensure!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return Ok(());
    }
    let report = var_os("TYPOL_B5_FIXTURE_PATH");
    let mut rows = String::from(
        "fixture\tsteps\tbudget_peak\tbudget_retained\tactual_heap_peak\tactual_heap_retained\tmedian_ns\n",
    );
    for name in ["server", "database", "all_types"] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/config")
            .join(format!("{name}.tschem"));
        for _ in 0..2 {
            let sample = measure(&path, 0)?;
            ensure!(sample.name_matches);
            ensure!(sample.fully_released);
            ensure!(sample.retained_matches_usage);
        }
        let sample = measure(&path, 0)?;
        ensure!(sample.name_matches);
        ensure!(sample.fully_released);
        ensure!(sample.retained_matches_usage);
        let mut times = Vec::new();
        for _ in 0..if report.is_some() { 31 } else { 1 } {
            let current = measure(&path, 0)?;
            ensure!(current.name_matches);
            ensure!(current.fully_released);
            ensure!(current.retained_matches_usage);
            ensure_eq!(current.usage, sample.usage);
            times.push(current.elapsed_ns);
        }
        times.sort_unstable();
        ensure!(sample.usage.steps < 67_108_864);
        ensure!(sample.usage.peak_workspace_bytes < 67_108_864);
        rows.push_str(&format!(
            "{name}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            sample.usage.steps,
            sample.usage.peak_workspace_bytes,
            sample.usage.workspace_bytes,
            sample.heap_peak,
            sample.heap_retained,
            *times
                .get(times.len() / 2)
                .ok_or_else(|| io::Error::other("median duration is missing"))?
        ));
    }
    if let Some(path) = report {
        write(path, rows)?;
    }

    Ok(())
}
