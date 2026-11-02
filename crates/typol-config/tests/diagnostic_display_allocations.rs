use typol_test_support::ConfigTestSupport as Support;

use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    fmt::{self, Write},
    str::from_utf8,
};

use typol_config::{
    ConfigLimits, ConfigParseErrors, ConfigParseOptions, ConfigParseRequest, Environment,
    EnvironmentError, EnvironmentReadContext,
    opeo::{OResult, Out},
};
use typol_core::{ExcerptLimits, SliceSource};
use typol_policy::ResourceLimits;
use typol_schema::{SchemaCompileOptions, SchemaLimits, SchemaSource};
use typol_semantic::CompiledSchema;
use typol_test_support::*;

#[derive(Clone, Copy, Default)]
struct AllocationState {
    active: bool,
    attempts: usize,
}

thread_local! {
    static ALLOCATIONS: Cell<AllocationState> = const { Cell::new(AllocationState {
        active: false,
        attempts: 0,
    }) };
}

struct TrackingAllocator;

fn record_allocation() {
    let _ = ALLOCATIONS.try_with(|state| {
        let mut current = state.get();
        if current.active {
            current.attempts += 1;
            state.set(current);
        }
    });
}

// SAFETY: All operations delegate to the system allocator with the original layout; allocation counts are tracked only on this thread.
unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record_allocation();
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record_allocation();
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record_allocation();
        unsafe { System.realloc(pointer, layout, size) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: TrackingAllocator = TrackingAllocator;

struct NoEnvironment;

impl Environment for NoEnvironment {
    fn read<'env, 'slot, 'borrow>(
        &'env mut self,
        _: &str,
        _: EnvironmentReadContext<'_>,
        _: Out<'slot, 'borrow, EnvironmentError>,
    ) -> OResult<'slot, Option<&'env [u8]>> {
        Ok(None)
    }
}

struct DisplayBuffer {
    bytes: [u8; 131_072],
    length: usize,
}

impl DisplayBuffer {
    const fn new() -> Self {
        Self {
            bytes: [0; 131_072],
            length: 0,
        }
    }

    fn as_str(&self) -> TestResult<&str> {
        let bytes = self.bytes.get(..self.length).test_value()?;
        from_utf8(bytes).map_err(|error| TestFailure::new(error.to_string()))
    }
}

impl Write for DisplayBuffer {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let Some(end) = self.length.checked_add(text.len()) else {
            return Err(fmt::Error);
        };
        let Some(output) = self.bytes.get_mut(self.length..end) else {
            return Err(fmt::Error);
        };
        output.copy_from_slice(text.as_bytes());
        self.length = end;
        Ok(())
    }
}

fn compile(source: &str, limits: SchemaLimits) -> TestResult<CompiledSchema> {
    Support::compile_schema(
        SchemaCompileOptions::new(limits),
        SchemaSource::from_str(b"display-allocation", source),
        None,
        None,
    )
    .test_value()
}

fn build_many_diagnostic_error() -> TestResult<ConfigParseErrors> {
    let count = 96;
    let mut declarations = String::from("entry struct Root {\n");
    let mut input = "\n".repeat(100_000);
    input.push_str("{\n");
    for index in 0..count {
        declarations.push_str(&format!("field_{index:03}: u8,\n"));
        input.push_str(&format!("field_{index:03}: \"{}\",\n", "x".repeat(64)));
    }
    declarations.push('}');
    input.push('}');

    let mut resources = ResourceLimits::default();
    let excerpts = ExcerptLimits::new(16_384, 512, 32, 0);
    resources.set_excerpts(excerpts);
    let mut config = ConfigLimits::default();
    config = config.with_max_diagnostics(128);
    let schema = compile(&declarations, SchemaLimits::default())?;
    let mut source = SliceSource::new(input.as_bytes());
    let errors = Support::parse(
        &schema,
        ConfigParseRequest::new(
            &mut source,
            "allocation-test",
            ConfigParseOptions::default()
                .with_resources(resources)
                .with_config(config),
        ),
        &mut NoEnvironment,
    )
    .test_error()?;
    test_assert!(errors.diagnostics().len() >= 90)?;
    test_assert!(errors.excerpts().storage_bytes() <= excerpts.max_bytes())?;
    test_assert!(errors.excerpts().records().len() <= excerpts.max_records())?;
    Ok(errors)
}

#[test]
fn formatting_many_diagnostics_from_a_large_input_does_not_allocate() -> TestResult<()> {
    let errors = build_many_diagnostic_error()?;
    let mut output = DisplayBuffer::new();
    ALLOCATIONS.with(|state| {
        state.set(AllocationState {
            active: true,
            attempts: 0,
        })
    });
    let result = fmt::write(&mut output, format_args!("{errors}"));
    let attempts = ALLOCATIONS.with(|state| state.get().attempts);
    ALLOCATIONS.with(|state| {
        state.set(AllocationState {
            active: false,
            attempts,
        })
    });

    result.map_err(|error| TestFailure::new(error.to_string()))?;
    let report = output.as_str()?;
    test_eq!(attempts, 0)?;
    test_eq!(
        report.matches("CONFIG_VALUE_ERROR").count(),
        errors.diagnostics().len()
    )?;
    test_assert!(report.contains("configuration load failed (parsing, 96 diagnostics)"))?;
    Ok(())
}
