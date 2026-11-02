#[macro_use]
#[path = "support/assertions.rs"]
mod assertions;

use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    env::{current_exe, var_os},
    error::Error,
    io,
    process::Command,
    ptr::null_mut,
};

use opeo::{ErrSlot, OResult, Out};
use typol_config::{
    ConfigParseErrors, ConfigParseOptions, ConfigParseRequest, ConfigParser, Environment,
    EnvironmentError, EnvironmentReadContext,
};
use typol_core::{ConfigValue, ExecutionLimits, SliceSource};
use typol_schema::{
    ResolverError, SchemaCompileErrors, SchemaCompileOptions, SchemaCompiler, SchemaResolveRequest,
    SchemaResolver, SchemaSource,
};
use typol_semantic::CompiledSchema;
use typol_std::{DisplayError, DisplayOptions, SafeValueFormatter};

struct NoImports;

impl SchemaResolver for NoImports {
    fn resolve<'src, 'slot, 'borrow>(
        &'src mut self,
        _: SchemaResolveRequest<'_>,
        out: Out<'slot, 'borrow, ResolverError>,
    ) -> OResult<'slot, SchemaSource<'src>> {
        Err(out.fail(ResolverError::NotFound))
    }
}

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

struct DisplayTest;

impl DisplayTest {
    fn compile(text: &str) -> Result<CompiledSchema, Box<dyn Error>> {
        let compiler = ErrSlot::<SchemaCompileErrors>::new()
            .call(|out| SchemaCompiler::new(SchemaCompileOptions::default(), out))?;
        ErrSlot::<SchemaCompileErrors>::new()
            .call(|out| {
                compiler.compile(
                    SchemaSource::from_str(b"root", text),
                    &mut NoImports,
                    None,
                    out,
                )
            })
            .map_err(|error| {
                io::Error::other(format!("Schema compilation failed: {error:?}")).into()
            })
    }

    fn parse(schema: &CompiledSchema, text: &str) -> Result<ConfigValue, Box<dyn Error>> {
        ErrSlot::<ConfigParseErrors>::new()
            .call(|out| {
                ConfigParser::new(schema).parse(
                    ConfigParseRequest::new(
                        &mut SliceSource::new(text.as_bytes()),
                        "config",
                        ConfigParseOptions::default(),
                    ),
                    &mut NoEnvironment,
                    out,
                )
            })
            .map(|value| value.into_config_value())
            .map_err(|error| {
                io::Error::other(format!("configuration parsing failed: {error:?}")).into()
            })
    }

    fn format(schema: &CompiledSchema, value: &ConfigValue) -> Result<String, Box<dyn Error>> {
        ErrSlot::<DisplayError>::new()
            .call(|out| SafeValueFormatter::format(schema, value, DisplayOptions::default(), out))
            .map_err(|error| io::Error::other(format!("value formatting failed: {error:?}")).into())
    }
}

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
    error: Option<DisplayError>,
    output_capacity: usize,
    retained: usize,
}

fn measure(
    schema: &CompiledSchema,
    value: &ConfigValue,
    fail_at: Option<usize>,
    persistent: bool,
    options: DisplayOptions,
) -> Measurement {
    STATE.with(|cell| {
        cell.set(State {
            active: true,
            fail_at,
            persistent,
            ..State::default()
        })
    });
    let result = ErrSlot::<DisplayError>::new()
        .call(|out| SafeValueFormatter::format(schema, value, options, out));
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
    if var_os("TYPOL_DISPLAY_OOM_CHILD").is_none() {
        let status = Command::new(current_exe()?)
            .args([
                "--exact",
                "each_single_and_persistent_allocation_failure_releases_the_entire_output",
                "--test-threads=1",
            ])
            .env("TYPOL_DISPLAY_OOM_CHILD", "1")
            .status()?;
        ensure!(status.success());
        return Ok(());
    }
    let schema = DisplayTest::compile("entry struct Root { value: map<string, [string]> }")?;
    let value = DisplayTest::parse(
        &schema,
        &format!(
            "{{value: {{\"z\": [\"{}\"], \"a\": [\"second\"]}}}}",
            "Ḁḁ".repeat(1024)
        ),
    )?;
    let expected = DisplayTest::format(&schema, &value)?;
    let baseline = measure(&schema, &value, None, false, DisplayOptions::default());
    ensure!(baseline.error.is_none());
    ensure_eq!(baseline.state.live, 0);
    ensure_eq!(baseline.retained, baseline.output_capacity);
    ensure!(baseline.state.peak > baseline.retained);
    ensure!(baseline.state.attempts > 4);
    let exact = DisplayOptions::new(
        *DisplayOptions::default().resources(),
        ExecutionLimits::new(
            DisplayOptions::default().execution().max_steps(),
            baseline.state.peak,
        ),
        DisplayOptions::default().max_output_bytes(),
    );
    ensure!(measure(&schema, &value, None, false, exact).error.is_none());
    let short = DisplayOptions::new(
        *DisplayOptions::default().resources(),
        ExecutionLimits::new(exact.execution().max_steps(), baseline.state.peak - 1),
        exact.max_output_bytes(),
    );
    let failed = measure(&schema, &value, None, false, short);
    ensure_eq!(failed.error, Some(DisplayError::ResourceLimitExceeded));
    ensure_eq!(failed.state.live, 0);
    for index in 0..baseline.state.attempts {
        for persistent in [false, true] {
            let failed = measure(
                &schema,
                &value,
                Some(index),
                persistent,
                DisplayOptions::default(),
            );
            ensure_eq!(
                failed.error,
                Some(DisplayError::AllocationFailed),
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
    ensure_eq!(DisplayTest::format(&schema, &value)?, expected);
    let hidden = DisplayTest::compile("entry struct Root { value: string @secret }")?;
    let secret = DisplayTest::parse(
        &hidden,
        &format!("{{value: \"{}\"}}", "PRIVATE".repeat(1024)),
    )?;
    ensure_eq!(DisplayTest::format(&hidden, &secret)?, "{value: ********}");

    Ok(())
}
