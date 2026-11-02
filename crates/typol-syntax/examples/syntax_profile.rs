//! Measures Schema parsing and teardown time, allocation requests, and logical live bytes.

use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    hint::black_box,
    iter::repeat_n,
    time::Instant,
};

use opeo::ErrSlot;
use typol_core::{BudgetError, DiagnosticCode, ExecutionBudget, ExecutionLimits, SourceId};
use typol_syntax::{AstAccessError, SchemaParseOptions, SchemaParser, SyntaxErrors};

#[derive(Clone, Copy, Default)]
struct Statistics {
    active: bool,
    allocations: usize,
    requested_bytes: usize,
    live_bytes: usize,
    peak_bytes: usize,
}

thread_local! {
    static STATISTICS: Cell<Statistics> = const { Cell::new(Statistics {
        active: false, allocations: 0, requested_bytes: 0, live_bytes: 0, peak_bytes: 0,
    }) };
}

fn account(old_size: usize, new_size: usize) {
    let _ = STATISTICS.try_with(|cell| {
        let mut stats = cell.get();
        if stats.active {
            if new_size != 0 {
                stats.allocations += 1;
                stats.requested_bytes += new_size;
            }
            stats.live_bytes = stats.live_bytes - old_size + new_size;
            stats.peak_bytes = stats.peak_bytes.max(stats.live_bytes);
            cell.set(stats);
        }
    });
}

struct CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            account(0, layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        account(layout.size(), 0);
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let result = unsafe { System.realloc(pointer, layout, size) };
        if !result.is_null() {
            account(layout.size(), size);
        }
        result
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

struct Scope;

impl Scope {
    fn begin() -> Self {
        STATISTICS.with(|cell| {
            cell.set(Statistics {
                active: true,
                ..Statistics::default()
            })
        });
        Self
    }

    fn finish(self) -> Statistics {
        STATISTICS.with(Cell::get)
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        STATISTICS.with(|cell| {
            let mut stats = cell.get();
            stats.active = false;
            cell.set(stats);
        });
    }
}

struct ProfileErrors {
    budget: ErrSlot<BudgetError>,
    syntax: ErrSlot<SyntaxErrors>,
    ast_access: ErrSlot<AstAccessError>,
}

impl ProfileErrors {
    fn new() -> Self {
        Self {
            budget: ErrSlot::new(),
            syntax: ErrSlot::new(),
            ast_access: ErrSlot::new(),
        }
    }

    fn execution(&mut self) -> Result<ExecutionBudget, String> {
        match self
            .budget
            .try_call(|out| ExecutionBudget::new(ExecutionLimits::new(67_108_864, 67_108_864), out))
        {
            Ok(execution) => Ok(execution),
            Err(error) => Err(format!(
                "failed to create execution budget: {:?}",
                error.take()
            )),
        }
    }
}

fn profile(
    errors: &mut ProfileErrors,
    name: &str,
    source: &str,
    succeeds: bool,
) -> Result<(), String> {
    let execution = errors.execution()?;

    let options = SchemaParseOptions::default();
    if let Err(error) = errors.syntax.try_call(|out| {
        SchemaParser::parse_str(SourceId::new(7), source, &options, &execution, out)
    }) {
        drop(error.take());
    }
    drop(execution);
    let mut parse_times = Vec::with_capacity(101);
    let mut drop_times = Vec::with_capacity(101);
    let mut statistics = Statistics::default();
    let mut steps = 0;
    let mut budget_peak = 0;
    let mut retained = 0;
    let mut cleaned = 0;
    let mut nodes = 0;
    let mut height = 0;
    let mut error_code = 0;
    for _ in 0..101 {
        let scope = Scope::begin();
        let execution = errors.execution()?;
        let start = Instant::now();
        let options = SchemaParseOptions::default();
        let result = match errors.syntax.try_call(|out| {
            SchemaParser::parse_str(
                SourceId::new(7),
                black_box(source),
                &options,
                &execution,
                out,
            )
        }) {
            Ok(ast) => Ok(ast),
            Err(error) => Err(error.take()),
        };
        let parse_time = start.elapsed();
        let actual_success = result.is_ok();
        error_code = result.as_ref().err().map_or(0, |errors| {
            errors
                .diagnostics()
                .next_back()
                .map_or(DiagnosticCode::INTERNAL_ERROR.get(), |diagnostic| {
                    diagnostic.code().get()
                })
        });
        if let Ok(ast) = &result {
            nodes = ast.node_count();
            match errors.ast_access.try_call(|out| ast.structure_height(out)) {
                Ok(value) => height = value,
                Err(_) => error_code = DiagnosticCode::INTERNAL_ERROR.get(),
            }
        }
        steps = execution.steps();
        budget_peak = execution.peak_workspace_bytes();
        retained = execution.workspace_bytes();
        let start = Instant::now();
        drop(black_box(result));
        let drop_time = start.elapsed();
        cleaned = execution.workspace_bytes();
        drop(execution);
        statistics = scope.finish();
        if statistics.peak_bytes != budget_peak {
            return Err("peak budget must cover every requested payload".to_owned());
        }
        if actual_success != succeeds {
            return Err(format!(
                "parse result for {name} did not match expectations"
            ));
        }
        if name == "nodes_over_limit" && error_code != 1 {
            return Err("node-limit case should return a resource-limit diagnostic".to_owned());
        }
        if statistics.live_bytes != 0 {
            return Err("logical live bytes remain after parsing and drop".to_owned());
        }
        parse_times.push(parse_time.as_nanos());
        drop_times.push(drop_time.as_nanos());
    }
    parse_times.sort_unstable();
    drop_times.sort_unstable();
    println!(
        "{name}\t{}\t{nodes}\t{height}\t{succeeds}\t{error_code}\t{}\t{}\t{}\t{}\t{}\t{steps}\t{budget_peak}\t{retained}\t{cleaned}",
        source.len(),
        statistics.allocations,
        statistics.requested_bytes,
        statistics.peak_bytes,
        parse_times
            .get(50)
            .ok_or_else(|| "not enough parse timing samples".to_owned())?,
        drop_times
            .get(50)
            .ok_or_else(|| "not enough drop timing samples".to_owned())?
    );
    Ok(())
}

fn main() -> Result<(), String> {
    let mut errors = ProfileErrors::new();
    println!(
        "case\tsource_bytes\tast_nodes\tast_height\tsuccess\terror_code\tallocations\trequested_bytes\tpeak_bytes\tparse_ns_median\tdrop_ns_median\tsteps\tbudget_peak\tretained_after_parse\tretained_after_drop"
    );
    profile(
        &mut errors,
        "ordinary",
        "entry struct Root { name: string = \"hello\", values: [u8] @size(0, 8) = [1, 2] }",
        true,
    )?;
    for width in [256, 512, 1024] {
        let mut fields = String::from("entry struct Root {");
        let mut annotations = fields.clone();
        for index in 0..width {
            fields.push_str(&format!("field_{index}: u32,"));
            annotations.push_str(&format!("field_{index}: u32 @min(0) @max(1000),"));
        }
        fields.push('}');
        annotations.push('}');
        profile(&mut errors, &format!("fields_{width}"), &fields, true)?;
        profile(
            &mut errors,
            &format!("annotations_{width}"),
            &annotations,
            true,
        )?;
        profile(
            &mut errors,
            &format!("recovery_{width}"),
            &"struct A { x: [u8 } ".repeat(width),
            false,
        )?;
    }
    for (name, string) in [
        ("plain_string", "a".repeat(65536)),
        ("escaped_string", "\\n".repeat(65536)),
    ] {
        profile(
            &mut errors,
            name,
            &format!("type A = string @regex(\"{string}\");"),
            true,
        )?;
    }
    profile(
        &mut errors,
        "expression_default_boundary",
        &format!(
            "entry struct Root {{}} @assert({}, \"rule\")",
            repeat_n("true", 126).collect::<Vec<_>>().join(" && ")
        ),
        true,
    )?;
    profile(
        &mut errors,
        "type_default_boundary",
        &format!("type A = {}u8{};", "option<".repeat(127), ">".repeat(127)),
        true,
    )?;
    let mut exact = String::from("entry struct Root {");
    for index in 0..33332 {
        exact.push_str(&format!("f_{index}:u8,"));
    }
    exact.push_str("} extern mod X;");
    profile(&mut errors, "nodes_100000_fields", &exact, true)?;
    exact.push_str("extern mod Y;");
    profile(&mut errors, "nodes_over_limit", &exact, false)?;
    let mut annotations = String::from("entry struct Root {");
    for index in 0..11110 {
        annotations.push_str(&format!("f_{index}:u8 @min(0) @max(100),"));
    }
    annotations.push_str("} extern mod A; extern mod B; extern mod C; extern mod D;");
    profile(&mut errors, "nodes_100000_annotations", &annotations, true)?;
    let value = "0,".repeat(49996);
    profile(
        &mut errors,
        "nodes_100000_defaults",
        &format!("entry struct Root {{ x: [u8] = [{value}] }}"),
        true,
    )?;
    profile(
        &mut errors,
        "invalid",
        &"struct A { x: [u8 } ".repeat(100),
        false,
    )?;
    Ok(())
}
