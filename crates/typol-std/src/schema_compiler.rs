use std::{error::Error, fmt, path::Path, slice::from_ref};

use opeo::{ErrSlot, OResult, Out};

use typol_core::{
    BudgetError, Diagnostic, DiagnosticCode, DiagnosticContext, FilledTerminalSlot, TerminalSlot,
    diagnostic_context,
};
use typol_report::{DiagnosticReport, ReportContext};
use typol_schema::{
    DiagnosticList, SchemaCompileBudgetDomain, SchemaCompileContext, SchemaCompileErrors,
    SchemaCompileOptions, SchemaCompileStage, SchemaCompileUsage, SchemaCompiler,
};
use typol_semantic::CompiledSchema;

use crate::{
    FileSchemaLimits, FileSchemaResolver, HostError, HostErrorKind, SourceNameLimits, SourceNames,
};

static FILE_SCHEMA_INITIALIZATION_CONTEXT: DiagnosticContext = diagnostic_context!(
    "typol-std",
    "file-schema-compilation",
    "initialization-failure",
);

#[derive(Default)]
pub struct FileSchemaCompileOptions {
    schema: SchemaCompileOptions,
    files: FileSchemaLimits,
    names: SourceNameLimits,
}

impl FileSchemaCompileOptions {
    pub const fn new(
        schema: SchemaCompileOptions,
        files: FileSchemaLimits,
        names: SourceNameLimits,
    ) -> Self {
        Self {
            schema,
            files,
            names,
        }
    }
}

pub struct FileSchemaCompilation {
    schema: CompiledSchema,
    names: SourceNames,
    usage: SchemaCompileUsage,
}

impl FileSchemaCompilation {
    pub fn schema(&self) -> &CompiledSchema {
        &self.schema
    }

    pub fn names(&self) -> &SourceNames {
        &self.names
    }

    pub fn names_mut(&mut self) -> &mut SourceNames {
        &mut self.names
    }

    pub const fn usage(&self) -> SchemaCompileUsage {
        self.usage
    }

    pub fn into_parts(self) -> (CompiledSchema, SourceNames) {
        (self.schema, self.names)
    }
}

pub enum FileSchemaCompileFailure<'a> {
    Root(&'a HostError),
    Compilation(&'a SchemaCompileErrors),
}

enum StoredFileFailure {
    Root(HostError),
    Compilation(SchemaCompileErrors),
}

struct FileErrorPayload {
    failure: StoredFileFailure,
    names: SourceNames,
    host_failure: Option<HostError>,
    usage: SchemaCompileUsage,
}

enum FileErrorStorage {
    Initialization {
        errors: SchemaCompileErrors,
        usage: SchemaCompileUsage,
    },
    Retained {
        slot: FilledTerminalSlot<FileErrorPayload>,
        fallback: SchemaCompileErrors,
        fallback_usage: SchemaCompileUsage,
    },
}

pub struct FileSchemaCompileErrors {
    storage: FileErrorStorage,
}

impl FileSchemaCompileErrors {
    fn initialization(error: SchemaCompileErrors) -> Self {
        let usage = Self::initialization_usage(&error, 0);
        Self::initialization_with_usage(error, usage)
    }

    fn initialization_with_usage(errors: SchemaCompileErrors, usage: SchemaCompileUsage) -> Self {
        let usage = Self::usage_with_failure(&errors, usage);
        Self {
            storage: FileErrorStorage::Initialization { errors, usage },
        }
    }

    fn initialization_usage(errors: &SchemaCompileErrors, peak: usize) -> SchemaCompileUsage {
        let failure = Self::failure_from_errors(errors);
        let (failure, regex_failure) = match errors.budget_domain() {
            Some(SchemaCompileBudgetDomain::Regex) => (None, Some(failure)),
            Some(SchemaCompileBudgetDomain::Execution) | None => (Some(failure), None),
        };
        SchemaCompileUsage {
            steps: 0,
            peak_workspace_bytes: peak,
            workspace_bytes: 0,
            last_stage: SchemaCompileStage::Initialization,
            failure,
            regex_steps: 0,
            regex_workspace_bytes: 0,
            regex_peak_workspace_bytes: 0,
            regex_failure,
        }
    }

    fn usage_with_failure(
        errors: &SchemaCompileErrors,
        mut usage: SchemaCompileUsage,
    ) -> SchemaCompileUsage {
        let failure = Self::failure_from_errors(errors);
        match errors.budget_domain() {
            Some(SchemaCompileBudgetDomain::Regex) => {
                if usage.regex_failure.is_none() {
                    usage.regex_failure = Some(failure);
                }
            }
            Some(SchemaCompileBudgetDomain::Execution) | None => {
                if usage.failure.is_none() {
                    usage.failure = Some(failure);
                }
            }
        }
        usage
    }

    fn failure_from_errors(errors: &SchemaCompileErrors) -> BudgetError {
        errors
            .diagnostics()
            .first()
            .map(Diagnostic::code)
            .map_or_else(
                || BudgetError::invariant(&FILE_SCHEMA_INITIALIZATION_CONTEXT),
                |code| match code {
                    DiagnosticCode::RESOURCE_LIMIT_EXCEEDED => BudgetError::Resource,
                    DiagnosticCode::ALLOCATION_FAILED => BudgetError::Allocation,
                    DiagnosticCode::CAPACITY_EXCEEDED => BudgetError::CapacityExceeded,
                    _ => BudgetError::invariant(&FILE_SCHEMA_INITIALIZATION_CONTEXT),
                },
            )
    }

    fn retained(
        slot: TerminalSlot<FileErrorPayload>,
        failure: StoredFileFailure,
        names: SourceNames,
        host_failure: Option<HostError>,
        context: &SchemaCompileContext,
    ) -> Self {
        let usage = context.usage();
        let Some(slot) = slot.store(FileErrorPayload {
            failure,
            names,
            host_failure,
            usage,
        }) else {
            return Self::initialization_with_usage(
                SchemaCompileErrors::budget_failure(BudgetError::CapacityExceeded),
                usage,
            );
        };
        let fallback = SchemaCompileErrors::budget_failure(BudgetError::CapacityExceeded);
        let fallback_usage = Self::usage_with_failure(&fallback, usage);
        Self {
            storage: FileErrorStorage::Retained {
                slot,
                fallback,
                fallback_usage,
            },
        }
    }

    pub fn failure(&self) -> FileSchemaCompileFailure<'_> {
        match &self.storage {
            FileErrorStorage::Initialization { errors, .. } => {
                FileSchemaCompileFailure::Compilation(errors)
            }
            FileErrorStorage::Retained { slot, fallback, .. } => match slot.get() {
                Some(payload) => match &payload.failure {
                    StoredFileFailure::Root(error) => FileSchemaCompileFailure::Root(error),
                    StoredFileFailure::Compilation(error) => {
                        FileSchemaCompileFailure::Compilation(error)
                    }
                },
                None => FileSchemaCompileFailure::Compilation(fallback),
            },
        }
    }

    pub fn diagnostics(&self) -> Option<DiagnosticList<'_>> {
        match self.failure() {
            FileSchemaCompileFailure::Root(_) => None,
            FileSchemaCompileFailure::Compilation(errors) => Some(errors.diagnostics()),
        }
    }

    pub fn internal_context(&self) -> Option<DiagnosticContext> {
        match self.failure() {
            FileSchemaCompileFailure::Root(_) => None,
            FileSchemaCompileFailure::Compilation(errors) => errors.internal_context(),
        }
    }

    pub fn host_failure(&self) -> Option<HostError> {
        match &self.storage {
            FileErrorStorage::Initialization { .. } => None,
            FileErrorStorage::Retained { slot, .. } => {
                slot.get().and_then(|payload| match &payload.failure {
                    StoredFileFailure::Root(error) => Some(*error),
                    StoredFileFailure::Compilation(_) => payload
                        .host_failure
                        .or_else(|| payload.names.failure_context()),
                })
            }
        }
    }

    pub fn names(&self) -> Option<&SourceNames> {
        match &self.storage {
            FileErrorStorage::Initialization { .. } => None,
            FileErrorStorage::Retained { slot, .. } => slot.get().map(|payload| &payload.names),
        }
    }

    pub fn usage(&self) -> SchemaCompileUsage {
        match &self.storage {
            FileErrorStorage::Initialization { usage, .. } => *usage,
            FileErrorStorage::Retained {
                slot,
                fallback_usage,
                ..
            } => slot.get().map_or(*fallback_usage, |payload| payload.usage),
        }
    }
}

pub struct FileSchemaCompiler;

impl FileSchemaCompiler {
    pub fn compile<'slot, 'borrow>(
        path: &Path,
        options: FileSchemaCompileOptions,
        out: Out<'slot, 'borrow, FileSchemaCompileErrors>,
    ) -> OResult<'slot, FileSchemaCompilation> {
        let mut schema_errors = ErrSlot::<SchemaCompileErrors>::new();
        let compiler = match schema_errors
            .try_call(|schema_out| SchemaCompiler::new(options.schema, schema_out))
        {
            Ok(compiler) => compiler,
            Err(caught) => {
                return Err(out.fail(FileSchemaCompileErrors::initialization(caught.take())));
            }
        };

        let mut budget_errors = ErrSlot::<BudgetError>::new();
        let context = match budget_errors.try_call(|budget_out| compiler.context(budget_out)) {
            Ok(context) => context,
            Err(caught) => {
                return Err(out.fail(FileSchemaCompileErrors::initialization(
                    SchemaCompileErrors::budget_failure(caught.take()),
                )));
            }
        };
        let slot = match budget_errors
            .try_call(|budget_out| TerminalSlot::new(context.execution(), budget_out))
        {
            Ok(slot) => slot,
            Err(caught) => {
                return Err(out.fail(FileSchemaCompileErrors::initialization_with_usage(
                    SchemaCompileErrors::budget_failure(caught.take()),
                    context.usage(),
                )));
            }
        };
        let mut resolver = match budget_errors
            .try_call(|budget_out| FileSchemaResolver::new(options.files, &context, budget_out))
        {
            Ok(resolver) => resolver,
            Err(caught) => {
                return Err(out.fail(FileSchemaCompileErrors::initialization_with_usage(
                    SchemaCompileErrors::budget_failure(caught.take()),
                    context.usage(),
                )));
            }
        };
        let mut names = match budget_errors
            .try_call(|budget_out| SourceNames::new(options.names, &context, budget_out))
        {
            Ok(names) => names,
            Err(caught) => {
                return Err(out.fail(FileSchemaCompileErrors::initialization_with_usage(
                    SchemaCompileErrors::budget_failure(caught.take()),
                    context.usage(),
                )));
            }
        };

        let mut host_errors = ErrSlot::<HostError>::new();
        let mut root = match host_errors.try_call(|host_out| resolver.read_root(path, host_out)) {
            Ok(root) => root,
            Err(caught) => {
                let error = caught.take();
                let failure = match error.kind() {
                    HostErrorKind::ResourceLimitExceeded => Some(BudgetError::Resource),
                    HostErrorKind::AllocationFailed => Some(BudgetError::Allocation),
                    HostErrorKind::CapacityExceeded => Some(BudgetError::CapacityExceeded),
                    HostErrorKind::InternalError => {
                        Some(BudgetError::invariant(error.budget_context()))
                    }
                    _ => None,
                };
                if let Some(failure) = failure {
                    context.execution().record_failure(failure);
                }
                drop(resolver);
                drop(compiler);
                return Err(out.fail(FileSchemaCompileErrors::retained(
                    slot,
                    StoredFileFailure::Root(error),
                    names,
                    Some(error),
                    &context,
                )));
            }
        };

        let result = schema_errors.try_call(|schema_out| {
            compiler.compile(root.source(), &mut resolver, Some(&mut names), schema_out)
        });
        let host_failure = resolver
            .failure_context()
            .or_else(|| names.failure_context());
        drop(root);
        drop(resolver);
        match result {
            Ok(schema) => {
                drop(slot);
                Ok(FileSchemaCompilation {
                    schema,
                    names,
                    usage: context.usage(),
                })
            }
            Err(caught) => Err(out.fail(FileSchemaCompileErrors::retained(
                slot,
                StoredFileFailure::Compilation(caught.take()),
                names,
                host_failure,
                &context,
            ))),
        }
    }
}

impl fmt::Debug for FileSchemaCompilation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FileSchemaCompilation")
            .field("usage", &self.usage())
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for FileSchemaCompileErrors {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FileSchemaCompileErrors")
            .field("usage", &self.usage())
            .finish_non_exhaustive()
    }
}

struct ReportBody<'a, W: fmt::Write + ?Sized> {
    writer: &'a mut W,
    started: bool,
}

impl<W: fmt::Write + ?Sized> fmt::Write for ReportBody<'_, W> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        if self.started {
            return self.writer.write_str(text);
        }
        if let Some(separator) = text.find("\n\n") {
            self.started = true;
            let body = text.get(separator + 2..).ok_or(fmt::Error)?;
            self.writer.write_str(body)?;
        }
        Ok(())
    }
}

impl fmt::Display for FileSchemaCompileErrors {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.failure() {
            FileSchemaCompileFailure::Root(error) => error.fmt(formatter),
            FileSchemaCompileFailure::Compilation(errors) => {
                let names = self.names();
                let source_name = |source_id| names.and_then(|names| names.name(source_id));
                let span_location = |span| errors.span_location(span);
                let diagnostics = errors.diagnostics();
                let diagnostic_count = diagnostics.len();
                let diagnostic_label = if diagnostic_count == 1 {
                    "diagnostic"
                } else {
                    "diagnostics"
                };
                write!(
                    formatter,
                    "Schema compilation failed ({diagnostic_count} {diagnostic_label})"
                )?;
                for (index, diagnostic) in diagnostics.iter().enumerate() {
                    if index > 0 {
                        formatter.write_str("\n\n")?;
                    }
                    let context =
                        ReportContext::new(&source_name, &span_location, errors.excerpts());
                    let mut body = ReportBody {
                        writer: formatter,
                        started: false,
                    };
                    DiagnosticReport::new("", None, from_ref(diagnostic), context)
                        .write_to(&mut body)?;
                }
                Ok(())
            }
        }
    }
}

impl Error for FileSchemaCompileErrors {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self.failure() {
            FileSchemaCompileFailure::Root(error) => Some(error),
            FileSchemaCompileFailure::Compilation(errors) => Some(errors),
        }
    }
}
