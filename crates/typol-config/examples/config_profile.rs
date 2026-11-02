//! Measures parsing and release through the public API, along with peak actual allocations and logical-budget thresholds.

#[path = "scaling_regressions/workloads.rs"]
mod workloads;

use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    env::args,
    error::Error,
    fmt::{self, Display, Formatter},
    hint::black_box,
    time::Instant,
};
use typol_config::{ConfigParseErrors, ConfigParseOptions, opeo::ErrSlot};
use typol_core::{ConfigValue, DiagnosticCode, ExecutionLimits};
use workloads::{Provider, Workload, WorkloadBuildError, WorkloadCase};

#[derive(Clone, Copy, Default)]
struct Statistics {
    active: bool,
    allocations: usize,
    requested: usize,
    live: usize,
    peak: usize,
}

thread_local! {
    static STATISTICS: Cell<Statistics> = const { Cell::new(Statistics {
        active: false, allocations: 0, requested: 0, live: 0, peak: 0,
    }) };
}

fn account(old: usize, new: usize) {
    let _ = STATISTICS.try_with(|cell| {
        let mut stats = cell.get();
        if stats.active {
            stats.allocations += usize::from(new != 0);
            stats.requested += new;
            stats.live = stats.live - old + new;
            stats.peak = stats.peak.max(stats.live);
            cell.set(stats);
        }
    });
}

struct CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: Delegates unchanged to the system allocator and accounts for the actual layout only on success.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            account(0, layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        account(layout.size(), 0);
        // SAFETY: The caller guarantees that the pointer and layout match the original allocation.
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        // SAFETY: Passes the valid pointer and layout through unchanged; on failure, accounting for the old storage is retained.
        let result = unsafe { System.realloc(pointer, layout, size) };
        if !result.is_null() {
            account(layout.size(), size);
        }
        result
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[derive(Clone, Copy)]
enum Quota {
    Steps,
    Workspace,
}

#[derive(Debug)]
struct ProfileError(String);

impl ProfileError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl Display for ProfileError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for ProfileError {}

impl From<WorkloadBuildError> for ProfileError {
    fn from(error: WorkloadBuildError) -> Self {
        Self::new(error.to_string())
    }
}

fn require(condition: bool, message: &str) -> Result<(), ProfileError> {
    if condition {
        Ok(())
    } else {
        Err(ProfileError::new(message))
    }
}

enum ProfileOutcome {
    Success(ConfigValue),
    Failure(ConfigParseErrors),
}

struct ProfileSlots {
    parse_errors: ErrSlot<ConfigParseErrors>,
}

impl ProfileSlots {
    const fn new() -> Self {
        Self {
            parse_errors: ErrSlot::new(),
        }
    }

    fn run(
        &mut self,
        workload: &Workload,
        options: ConfigParseOptions,
        provider: &mut Provider,
    ) -> ProfileOutcome {
        match self
            .parse_errors
            .try_call(|out| workload.run(options, provider, out))
        {
            Ok(value) => ProfileOutcome::Success(value),
            Err(caught) => ProfileOutcome::Failure(caught.take()),
        }
    }
}

fn limits(quota: Quota, value: usize) -> ExecutionLimits {
    match quota {
        Quota::Steps => ExecutionLimits::new(value, 268_435_456),
        Quota::Workspace => ExecutionLimits::new(1_000_000_000, value),
    }
}

fn verify_result(
    workload: &Workload,
    result: &ProfileOutcome,
    provider: &Provider,
) -> Result<(), ProfileError> {
    require(
        provider.calls() == workload.reads(),
        "provider call count does not match the workload expectation",
    )?;
    match result {
        ProfileOutcome::Success(_) => require(
            workload.errors() == 0,
            "workload did not return the expected diagnostics",
        )?,
        ProfileOutcome::Failure(errors) => {
            require(
                errors.diagnostics().len() == workload.errors(),
                "parse diagnostic count does not match the workload expectation",
            )?;
            for diagnostic in errors.diagnostics() {
                require(
                    diagnostic.code() == DiagnosticCode::CONFIG_VALIDATION_ERROR,
                    "workload returned a non-validation diagnostic",
                )?;
            }
        }
    }
    Ok(())
}

fn minimum(
    workload: &Workload,
    quota: Quota,
    slots: &mut ProfileSlots,
) -> Result<usize, ProfileError> {
    let mut low: usize = 0;
    let mut high: usize = match quota {
        Quota::Steps => 1_000_000_000,
        Quota::Workspace => 268_435_456,
    };
    while low < high {
        let middle = low + (high - low) / 2;
        let result = slots.run(
            workload,
            workload.options(limits(quota, middle)),
            &mut Provider::new(),
        );
        let exhausted = match result {
            ProfileOutcome::Success(value) => {
                drop(value);
                false
            }
            ProfileOutcome::Failure(errors) => match errors.diagnostics().last() {
                Some(diagnostic)
                    if diagnostic.code() == DiagnosticCode::RESOURCE_LIMIT_EXCEEDED =>
                {
                    true
                }
                Some(diagnostic) => {
                    return Err(ProfileError::new(format!(
                        "unexpected diagnostic while finding the budget threshold: {:?}",
                        diagnostic.code()
                    )));
                }
                None => return Err(ProfileError::new("budget error is missing a diagnostic")),
            },
        };
        if exhausted {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    Ok(low)
}

fn profile(workload: &Workload, size: usize, slots: &mut ProfileSlots) -> Result<(), ProfileError> {
    let options = workload.options(ExecutionLimits::new(1_000_000_000, 268_435_456));
    let mut warmup_provider = Provider::new();
    let warmup = slots.run(workload, options, &mut warmup_provider);
    verify_result(workload, &warmup, &warmup_provider)?;
    drop(warmup);

    let mut parse_times = Vec::with_capacity(31);
    let mut drop_times = Vec::with_capacity(31);
    let mut stats = Statistics::default();
    for _ in 0..31 {
        let mut provider = Provider::new();
        STATISTICS.with(|cell| {
            cell.set(Statistics {
                active: true,
                ..Statistics::default()
            })
        });
        let start = Instant::now();
        let result = slots.run(workload, options, &mut provider);
        let parse_ns = start.elapsed().as_nanos();
        verify_result(workload, &result, &provider)?;
        let start = Instant::now();
        drop(black_box(result));
        let drop_ns = start.elapsed().as_nanos();
        stats = STATISTICS.with(|cell| {
            let stats = cell.get();
            cell.set(Statistics::default());
            stats
        });
        require(
            stats.live == 0,
            "allocations remain active after dropping the parse result",
        )?;
        parse_times.push(parse_ns);
        drop_times.push(drop_ns);
    }
    parse_times.sort_unstable();
    drop_times.sort_unstable();
    let parse_median = parse_times
        .get(15)
        .ok_or_else(|| ProfileError::new("not enough parse timing samples"))?;
    let drop_median = drop_times
        .get(15)
        .ok_or_else(|| ProfileError::new("not enough drop timing samples"))?;
    let step_minimum = minimum(workload, Quota::Steps, slots)?;
    let workspace_minimum = minimum(workload, Quota::Workspace, slots)?;
    println!(
        "{}\t{size}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
        workload.name(),
        workload.source_bytes(),
        workload.errors(),
        workload.reads(),
        stats.allocations,
        stats.requested,
        stats.peak,
        parse_median,
        drop_median,
        step_minimum,
        workspace_minimum
    );
    Ok(())
}

fn parse_size(value: &str) -> Result<usize, ProfileError> {
    value
        .parse()
        .map_err(|error| ProfileError::new(format!("invalid workload size: {error}")))
}

fn main() -> Result<(), ProfileError> {
    let mut arguments = args().skip(1);
    let filter = arguments.next();
    let base = arguments
        .next()
        .map_or(Ok(128), |value| parse_size(&value))?;
    if arguments.next().is_some() {
        return Err(ProfileError::new(
            "too many arguments: expected a workload name and size at most",
        ));
    }
    let doubled = base
        .checked_mul(2)
        .ok_or_else(|| ProfileError::new("workload size overflowed when doubled"))?;
    let cases = [
        WorkloadCase::WideFields,
        WorkloadCase::EnvironmentNames,
        WorkloadCase::CollectionErrors,
        WorkloadCase::AssertionChain,
        WorkloadCase::DefaultMap,
        WorkloadCase::UniqueCaseVariants,
        WorkloadCase::LongLineLocations,
        WorkloadCase::LargeEnvironmentValues,
    ];
    let selected = match filter.as_deref() {
        Some(name) => Some(
            cases
                .iter()
                .copied()
                .find(|case| case.name() == name)
                .ok_or_else(|| ProfileError::new(format!("unknown workload name: {name}")))?,
        ),
        None => None,
    };
    println!(
        "case\tn\tsource_bytes\terrors\tprovider_calls\tallocations\trequested_bytes\tpeak_heap_bytes\tparse_ns_median\tdrop_ns_median\tminimum_steps\tminimum_workspace_bytes"
    );
    let mut build_errors = ErrSlot::<WorkloadBuildError>::new();
    let mut slots = ProfileSlots::new();
    for case in cases {
        if selected.is_some_and(|selected| selected != case) {
            continue;
        }
        if matches!(case, WorkloadCase::LargeEnvironmentValues) && base > 128 {
            continue;
        }
        for size in [base, doubled] {
            let workload = build_errors
                .try_call(|out| Workload::new(case, size, out))
                .map_err(|caught| ProfileError::new(caught.take().to_string()))?;
            profile(&workload, size, &mut slots)?;
        }
    }
    Ok(())
}
