use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    error::Error,
    fmt::{self, Display, Formatter},
    time::Instant,
};
use typol_config::{
    ConfigParseErrors, ConfigParseOptions, ConfigParseRequest, ConfigParser, Environment,
    EnvironmentError, EnvironmentReadContext, StreamLimits,
    opeo::{ErrSlot, OResult, Out},
};
use typol_core::{
    BudgetError, ExecutionBudget, Read, SliceSource, SourceId, SourceReadError, SourceReadErrorKind,
};
use typol_schema::{
    ResolverError, SchemaCompileErrors, SchemaCompileOptions, SchemaCompiler, SchemaResolveRequest,
    SchemaResolver, SchemaSource,
};
use typol_semantic::CompiledSchema;

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

struct ProfileSlots {
    schema_errors: ErrSlot<SchemaCompileErrors>,
    budget_errors: ErrSlot<BudgetError>,
    parse_errors: ErrSlot<ConfigParseErrors>,
}

impl ProfileSlots {
    const fn new() -> Self {
        Self {
            schema_errors: ErrSlot::new(),
            budget_errors: ErrSlot::new(),
            parse_errors: ErrSlot::new(),
        }
    }
}

fn require(condition: bool, message: &str) -> Result<(), ProfileError> {
    if condition {
        Ok(())
    } else {
        Err(ProfileError::new(message))
    }
}

struct NoImports;

impl SchemaResolver for NoImports {
    fn resolve<'a, 'slot, 'borrow>(
        &'a mut self,
        _: SchemaResolveRequest<'_>,
        out: Out<'slot, 'borrow, ResolverError>,
    ) -> OResult<'slot, SchemaSource<'a>> {
        Err(out.fail(ResolverError::NotFound))
    }
}

struct ObservedInput<'a> {
    input: SliceSource<'a>,
    bytes: &'a Cell<usize>,
    calls: &'a Cell<usize>,
}

impl Read for ObservedInput<'_> {
    fn read<'slot>(
        &mut self,
        output: &mut [u8],
        source_id: SourceId,
        offset: usize,
        mut out: Out<'slot, '_, SourceReadError>,
    ) -> OResult<'slot, usize> {
        self.calls.set(self.calls.get() + 1);
        let count = self.input.read(output, source_id, offset, out.reborrow())?;
        let Some(target) = output.get_mut(..count) else {
            return Err(out.fail(
                SourceReadError::new(SourceReadErrorKind::InternalInvariantFailure)
                    .with_input(source_id, offset),
            ));
        };
        self.bytes.set(self.bytes.get() + target.len());
        Ok(count)
    }
}

struct Provider<'a> {
    bytes: &'a Cell<usize>,
    first: usize,
}

impl Environment for Provider<'_> {
    fn read<'env, 'slot, 'borrow>(
        &'env mut self,
        _: &str,
        _: EnvironmentReadContext<'_>,
        _: Out<'slot, 'borrow, EnvironmentError>,
    ) -> OResult<'slot, Option<&'env [u8]>> {
        self.first = self.bytes.get();
        Ok(Some(b"7"))
    }
}

#[derive(Clone, Copy)]
enum WorkloadCase {
    Small,
    LargeToken,
    LongComment,
    DenseNewlines,
    Deep,
    Array,
}

impl WorkloadCase {
    const fn name(self) -> &'static str {
        match self {
            Self::Small => "small",
            Self::LargeToken => "large_token",
            Self::LongComment => "long_comment",
            Self::DenseNewlines => "dense_newlines",
            Self::Deep => "deep",
            Self::Array => "array",
        }
    }
}

struct Workload {
    case: WorkloadCase,
    schema: String,
    text: String,
}

impl Workload {
    fn new(case: WorkloadCase) -> Self {
        let (data_type, data) = match case {
            WorkloadCase::LargeToken => ("string", format!("\"{}\"", "a".repeat(65536))),
            WorkloadCase::Array => ("[u8]", format!("[{}]", vec!["1"; 8192].join(","))),
            _ => ("u8", "1".into()),
        };
        let mut schema =
            format!("entry struct Root {{ first: u8 @env(\"X\"), data: {data_type} }}");
        let mut text = format!("{{first: env(\"X\"), data: {data}}}");
        match case {
            WorkloadCase::LongComment => text.push_str(&format!("/*{}*/", "x".repeat(262144))),
            WorkloadCase::DenseNewlines => text.push_str(&format!("/*{}*/", "\n".repeat(262144))),
            WorkloadCase::Deep => {
                schema = "entry struct Root { first: u8 @env(\"X\"), data: N0 }".into();
                for depth in 0..63 {
                    schema.push_str(&format!("struct N{depth} {{ child: N{} }}", depth + 1));
                }
                schema.push_str("struct N63 { child: u8 }");
                text = format!(
                    "{{first: env(\"X\"), data: {}1{}}}",
                    "{child:".repeat(64),
                    "}".repeat(64)
                );
            }
            WorkloadCase::Small | WorkloadCase::LargeToken | WorkloadCase::Array => {}
        }
        Self { case, schema, text }
    }
}

fn compile_schema(text: &str, slots: &mut ProfileSlots) -> Result<CompiledSchema, ProfileError> {
    slots
        .schema_errors
        .try_call(|mut out| {
            let compiler = SchemaCompiler::new(SchemaCompileOptions::default(), out.reborrow())?;
            compiler.compile(
                SchemaSource::from_str(b"profile", text),
                &mut NoImports,
                None,
                out.reborrow(),
            )
        })
        .map_err(|caught| {
            ProfileError::new(format!("schema compilation failed: {:?}", caught.take()))
        })
}

fn profile(
    workload: &Workload,
    chunk: usize,
    slots: &mut ProfileSlots,
) -> Result<(), ProfileError> {
    let schema = compile_schema(&workload.schema, slots)?;
    let mut timings = Vec::with_capacity(7);
    let mut report = String::new();
    for _ in 0..7 {
        let stream = StreamLimits::try_new(chunk, 1_048_576)
            .map_err(|error| ProfileError::new(error.to_string()))?;
        let options = ConfigParseOptions::default().with_stream(stream);
        let budget = slots
            .budget_errors
            .try_call(|out| ExecutionBudget::new(*options.execution(), out))
            .map_err(|caught| {
                ProfileError::new(format!(
                    "failed to create execution budget: {:?}",
                    caught.take()
                ))
            })?;
        let initial = budget.workspace_bytes();
        let bytes = Cell::new(0);
        let calls = Cell::new(0);
        let mut input = ObservedInput {
            input: SliceSource::new(workload.text.as_bytes()),
            bytes: &bytes,
            calls: &calls,
        };
        let mut provider = Provider {
            bytes: &bytes,
            first: 0,
        };
        STATISTICS.with(|cell| {
            cell.set(Statistics {
                active: true,
                ..Statistics::default()
            })
        });
        let started = Instant::now();
        let value = slots
            .parse_errors
            .try_call(|out| {
                ConfigParser::new(&schema).parse_with_budget(
                    ConfigParseRequest::new(&mut input, "profile", options),
                    &mut provider,
                    &budget,
                    out,
                )
            })
            .map_err(|caught| {
                ProfileError::new(format!("configuration parsing failed: {:?}", caught.take()))
            })?;
        drop(value);
        let elapsed = started.elapsed().as_nanos();
        let stats = STATISTICS.with(|cell| {
            let stats = cell.get();
            cell.set(Statistics::default());
            stats
        });
        require(
            stats.live == 0,
            "allocations remain active after dropping the parse result",
        )?;
        require(
            budget.workspace_bytes() == initial,
            "execution budget was not restored after parsing",
        )?;
        timings.push(elapsed);
        report = format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}",
            workload.case.name(),
            chunk,
            workload.text.len(),
            provider.first,
            calls.get(),
            budget.steps(),
            budget.peak_workspace_bytes()
        );
        report.push_str(&format!("\t{}\t{}", stats.allocations, stats.peak));
    }
    timings.sort_unstable();
    let median = timings
        .get(3)
        .ok_or_else(|| ProfileError::new("not enough streaming parse timing samples"))?
        .to_string();
    let mut columns: Vec<_> = report.split('\t').collect();
    require(
        columns.len() >= 7,
        "streaming parse report has too few columns",
    )?;
    columns.insert(7, &median);
    println!("{}", columns.join("\t"));
    Ok(())
}

fn main() -> Result<(), ProfileError> {
    println!(
        "workload\tchunk_bytes\tsource_bytes\tfirst_field_read_bytes\tread_calls\tsteps\tpeak_workspace_bytes\tmedian_ns\tallocations\tactual_peak_bytes"
    );
    let mut slots = ProfileSlots::new();
    for case in [
        WorkloadCase::Small,
        WorkloadCase::LargeToken,
        WorkloadCase::LongComment,
        WorkloadCase::DenseNewlines,
        WorkloadCase::Deep,
        WorkloadCase::Array,
    ] {
        let workload = Workload::new(case);
        for chunk in [1, 7, 8192] {
            profile(&workload, chunk, &mut slots)?;
        }
    }
    Ok(())
}
