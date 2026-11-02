use std::{
    error::Error,
    fmt::{self, Display, Formatter, Write},
};
use typol_config::{
    ConfigLimits, ConfigParseErrors, ConfigParseOptions, ConfigParseRequest, ConfigParser,
    Environment, EnvironmentError, EnvironmentReadContext,
    opeo::{ErrSlot, OResult, Out, ResultOutExt},
};
use typol_core::{ConfigValue, ExecutionLimits, SliceSource};
use typol_policy::ResourceLimits;
use typol_schema::{
    ResolverError, SchemaCompileOptions, SchemaCompiler, SchemaLimits, SchemaResolveRequest,
    SchemaResolver, SchemaSource,
};
use typol_semantic::CompiledSchema;

pub struct Workload {
    name: &'static str,
    schema: CompiledSchema,
    text: String,
    size: usize,
    error_count: usize,
    reads: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkloadCase {
    WideFields,
    EnvironmentNames,
    CollectionErrors,
    AssertionChain,
    DefaultMap,
    UniqueCaseVariants,
    LongLineLocations,
    LargeEnvironmentValues,
}

impl WorkloadCase {
    pub const fn name(&self) -> &'static str {
        match self {
            Self::WideFields => "wide_fields",
            Self::EnvironmentNames => "environment_names",
            Self::CollectionErrors => "collection_errors",
            Self::AssertionChain => "assertion_chain",
            Self::DefaultMap => "default_map",
            Self::UniqueCaseVariants => "unique_case_variants",
            Self::LongLineLocations => "long_line_locations",
            Self::LargeEnvironmentValues => "large_environment_values",
        }
    }
}

#[derive(Debug)]
pub enum WorkloadBuildError {
    Formatting(fmt::Error),
    Schema(String),
    EmptyAssertionChain,
}

impl Display for WorkloadBuildError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Formatting(error) => write!(formatter, "failed to format workload text: {error}"),
            Self::Schema(error) => write!(formatter, "failed to compile workload schema: {error}"),
            Self::EmptyAssertionChain => {
                formatter.write_str("assertion-chain workload length must be greater than zero")
            }
        }
    }
}

impl Error for WorkloadBuildError {}

impl From<fmt::Error> for WorkloadBuildError {
    fn from(error: fmt::Error) -> Self {
        Self::Formatting(error)
    }
}

struct NoImports;

impl SchemaResolver for NoImports {
    fn resolve<'a, 'slot, 'borrow>(
        &'a mut self,
        _request: SchemaResolveRequest<'_>,
        out: Out<'slot, 'borrow, ResolverError>,
    ) -> OResult<'slot, SchemaSource<'a>> {
        Err(out.fail(ResolverError::NotFound))
    }
}

pub struct Provider {
    calls: usize,
    bytes: &'static [u8],
}

impl Provider {
    pub const fn new() -> Self {
        Self {
            calls: 0,
            bytes: b"value",
        }
    }

    pub const fn calls(&self) -> usize {
        self.calls
    }
}

impl Environment for Provider {
    fn read<'env, 'slot, 'borrow>(
        &'env mut self,
        _: &str,
        _: EnvironmentReadContext<'_>,
        _: Out<'slot, 'borrow, EnvironmentError>,
    ) -> OResult<'slot, Option<&'env [u8]>> {
        self.calls += 1;
        Ok(Some(self.bytes))
    }
}

impl Workload {
    pub fn new<'slot>(
        case: WorkloadCase,
        size: usize,
        mut out: Out<'slot, '_, WorkloadBuildError>,
    ) -> OResult<'slot, Self> {
        if matches!(case, WorkloadCase::AssertionChain) && size == 0 {
            return Err(out.fail(WorkloadBuildError::EmptyAssertionChain));
        }
        let name = case.name();
        let mut schema = String::new();
        let mut text = String::new();
        let mut error_count = 0;
        let mut reads = 0;
        match case {
            WorkloadCase::WideFields
            | WorkloadCase::EnvironmentNames
            | WorkloadCase::LargeEnvironmentValues => {
                schema.push_str("entry struct Root {");
                text.push('{');
                for index in 0..size {
                    write!(schema, "f{index:05}: string").store_err_into(out.reborrow())?;
                    if !matches!(case, WorkloadCase::WideFields) {
                        write!(schema, " @env(\"E{index:05}\")").store_err_into(out.reborrow())?;
                    }
                    schema.push(',');
                }
                // Reverse-order input exercises both Schema slot reordering and name lookup.
                for index in (0..size).rev() {
                    write!(text, "f{index:05}: ").store_err_into(out.reborrow())?;
                    if !matches!(case, WorkloadCase::WideFields) {
                        write!(text, "env(\"E{index:05}\"),").store_err_into(out.reborrow())?;
                    } else {
                        text.push_str("\"value\",");
                    }
                }
                schema.push('}');
                text.push('}');
                reads = if matches!(case, WorkloadCase::WideFields) {
                    0
                } else {
                    size
                };
            }
            WorkloadCase::CollectionErrors | WorkloadCase::LongLineLocations => {
                schema.push_str("entry struct Root { values: [u8] @all(item > 0, \"rule\") }");
                text.push_str("{values: [");
                for _ in 0..size {
                    if matches!(case, WorkloadCase::LongLineLocations) {
                        text.push_str("/* ééééé */");
                    }
                    text.push_str("0,");
                }
                text.push_str("]}");
                error_count = size;
            }
            WorkloadCase::AssertionChain => {
                schema.push_str("struct Leaf { value: u8 } @assert(self.value == 1, \"rule\") ");
                for index in 0..size {
                    let child = if index == 0 {
                        "Leaf".to_owned()
                    } else {
                        format!("S{}", index - 1)
                    };
                    write!(
                        schema,
                        "struct S{index} {{ child: {child} }} @assert(true, \"rule\") "
                    )
                    .store_err_into(out.reborrow())?;
                }
                write!(
                    schema,
                    "entry struct Root {{ child: S{} }} @assert(true, \"rule\")",
                    size - 1
                )
                .store_err_into(out.reborrow())?;
                text.push_str(&"{child:".repeat(size + 1));
                text.push_str("{value:1}");
                text.push_str(&"}".repeat(size + 1));
            }
            WorkloadCase::DefaultMap => {
                schema.push_str("entry struct Root { values: map<string, string> = {");
                for index in 0..size {
                    write!(schema, "\"key{index:05}\": \"value\",")
                        .store_err_into(out.reborrow())?;
                }
                schema.push_str("} }");
                text.push_str("{}");
            }
            WorkloadCase::UniqueCaseVariants => {
                schema.push_str("entry struct Root { values: [string] @unique }");
                text.push_str("{values: [");
                // @unique distinguishes strings with different original casing.
                for index in 0..size {
                    text.push('\"');
                    for bit in 0..16 {
                        text.push(if index & (1 << bit) == 0 { 'a' } else { 'A' });
                    }
                    text.push_str("\",");
                }
                text.push_str("]}");
            }
        }
        let mut limits = SchemaLimits::default();
        limits.set_max_nesting_depth(8192);
        limits.set_max_value_nodes(1_000_000);
        limits.set_max_schema_ast_nodes(1_000_000);
        let mut errors = ErrSlot::new();
        let result = errors.try_call(|mut out| {
            let compiler = SchemaCompiler::new(SchemaCompileOptions::new(limits), out.reborrow())?;
            compiler.compile(
                SchemaSource::from_str(b"profile", &schema),
                &mut NoImports,
                None,
                out.reborrow(),
            )
        });
        let schema = match result {
            Ok(schema) => Ok(schema),
            Err(caught) => Err(WorkloadBuildError::Schema(format!("{:?}", caught.take()))),
        }
        .store_err_into(out.reborrow())?;
        Ok(Self {
            name,
            schema,
            text,
            size,
            error_count,
            reads,
        })
    }

    pub fn run<'slot, 'borrow>(
        &self,
        options: ConfigParseOptions,
        provider: &mut Provider,
        out: Out<'slot, 'borrow, ConfigParseErrors>,
    ) -> OResult<'slot, ConfigValue> {
        if self.name == WorkloadCase::LargeEnvironmentValues.name() {
            provider.bytes = &[b'x'; 32768];
        }
        ConfigParser::new(&self.schema)
            .parse(
                ConfigParseRequest::new(
                    &mut SliceSource::new(self.text.as_bytes()),
                    "profile",
                    options,
                ),
                provider,
                out,
            )
            .map(|value| value.into_config_value())
    }

    pub fn options(&self, execution: ExecutionLimits) -> ConfigParseOptions {
        let limits = ConfigLimits::default()
            .with_max_nesting_depth(8192)
            .with_max_diagnostics(self.size + 1);
        ConfigParseOptions::new(ResourceLimits::default(), limits, execution)
    }

    pub const fn name(&self) -> &'static str {
        self.name
    }

    pub fn source_bytes(&self) -> usize {
        self.text.len()
    }

    pub const fn errors(&self) -> usize {
        self.error_count
    }

    pub const fn reads(&self) -> usize {
        self.reads
    }
}
