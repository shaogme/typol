use opeo::{OResult, Out, ResultOutExt};
use typol_semantic::SourceLookup;

use typol_core::{
    BudgetError, ChargedVec, Diagnostic, DiagnosticCode, ExecutionBudget, Read, SourceId,
    SourceReadError, SourceSpan, Span,
};

use crate::{
    SchemaCompileContext, SchemaCompileStage, SchemaLimits,
    compiled::{SchemaCompileContextFailure, SchemaCompileErrors},
    diagnostics::DiagnosticCollector,
    loader::{LoadedFile, SourceLoader},
    resolver::{SchemaResolver, SchemaSource, SourceObserver},
};

pub(crate) struct SourceCatalog {
    files: ChargedVec<LoadedFile>,
}

impl SourceLookup for SourceCatalog {
    fn source_count(&self) -> usize {
        self.files.len()
    }

    fn is_valid_span(&self, span: SourceSpan) -> bool {
        self.files
            .get(span.source_id().get() as usize)
            .filter(|file| file.source_id() == span.source_id())
            .and_then(LoadedFile::positions)
            .is_some_and(|positions| {
                positions.is_valid_span(span.span().start(), span.span().end())
            })
    }
}

impl SourceCatalog {
    pub(crate) fn new<'slot>(
        execution: &ExecutionBudget,
        mut out: Out<'slot, '_, BudgetError>,
    ) -> OResult<'slot, Self> {
        let files = ChargedVec::new(Some(execution), out.reborrow())?;
        Ok(Self { files })
    }

    pub(crate) fn files_mut(&mut self) -> &mut ChargedVec<LoadedFile> {
        &mut self.files
    }

    pub(crate) fn files(&self) -> &[LoadedFile] {
        &self.files
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ObjectState {
    Pending,
    Valid,
    Invalid,
    Ambiguous,
}

/// Tracks session resources and charges successful and syntactically invalid sources alike.
pub(crate) struct CompileBudget {
    limits: SchemaLimits,
    source_bytes: usize,
    source_files: usize,
    execution: ExecutionBudget,
}

/// Loading and all semantic passes share the source directory, diagnostic budget, and terminal state.
pub(crate) struct CompileSession {
    sources: SourceCatalog,
    budget: CompileBudget,
    diagnostics: DiagnosticCollector,
}

pub(crate) struct SessionParts {
    pub(crate) sources: SourceCatalog,
    pub(crate) budget: CompileBudget,
    pub(crate) diagnostics: DiagnosticCollector,
}

impl CompileSession {
    pub(crate) fn into_parts(self) -> SessionParts {
        SessionParts {
            sources: self.sources,
            budget: self.budget,
            diagnostics: self.diagnostics,
        }
    }

    pub(crate) fn load<'slot, R: SchemaResolver + ?Sized, S: Read<SourceReadError>>(
        root: SchemaSource<'_, S>,
        resolver: &mut R,
        context: &SchemaCompileContext,
        observer: Option<&mut dyn SourceObserver>,
        mut out: Out<'slot, '_, SchemaCompileErrors>,
    ) -> OResult<'slot, Self> {
        let span = root_span(root.known_len().unwrap_or(0));
        let failure = if !root.matches_context(context) {
            Some(SchemaCompileContextFailure::RootContextMismatch)
        } else if context.execution().finished() {
            Some(SchemaCompileContextFailure::SessionFinished)
        } else {
            None
        };
        if let Some(failure) = failure {
            let code = match failure {
                SchemaCompileContextFailure::RootContextMismatch => {
                    DiagnosticCode::SCHEMA_CONTEXT_MISMATCH
                }
                SchemaCompileContextFailure::SessionFinished => {
                    DiagnosticCode::SCHEMA_SESSION_FINISHED
                }
            };
            let mut diagnostic = Diagnostic::emergency(code);
            diagnostic.set_primary(Some(span));
            let context = match failure {
                SchemaCompileContextFailure::RootContextMismatch => {
                    schema_context!("source-loading", "root-context-mismatch")
                }
                SchemaCompileContextFailure::SessionFinished => {
                    schema_context!("source-loading", "execution-context-finished")
                }
            };
            diagnostic.set_context(Some(context));
            return Err(out.fail(SchemaCompileErrors::with_context_failure(
                diagnostic, failure,
            )));
        }
        let execution = context.execution();
        execution.consume(0, out.reborrow())?;
        context.set_stage(SchemaCompileStage::SourceLoading);
        let limits = context.resources();
        let mut diagnostics =
            DiagnosticCollector::new(limits.max_diagnostics(), span, execution, out.reborrow())?;
        let mut budget = CompileBudget::new(*limits, execution, out.reborrow())?;
        let sources = SourceLoader::load(root, resolver, &mut budget, &mut diagnostics, observer)
            .map_err(SchemaCompileErrors::inline)
            .store_err_into(out.reborrow())?;
        Ok(Self {
            sources,
            diagnostics,
            budget,
        })
    }
}

impl CompileBudget {
    pub(crate) fn new<'slot, E>(
        limits: SchemaLimits,
        execution: &ExecutionBudget,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, Self>
    where
        E: From<BudgetError>,
    {
        let execution = execution.try_clone(out.reborrow())?;
        Ok(Self {
            limits,
            source_bytes: 0,
            source_files: 0,
            execution,
        })
    }

    pub(crate) fn execution(&self) -> &ExecutionBudget {
        &self.execution
    }

    pub(crate) const fn limits(&self) -> &SchemaLimits {
        &self.limits
    }

    pub(crate) fn request_limit(&self, depth: usize) -> Option<usize> {
        if depth > self.limits.max_nesting_depth()
            || self.source_files >= self.limits.max_schema_files()
        {
            return None;
        }
        Some(
            self.limits.max_source_bytes().min(
                self.limits
                    .max_schema_total_bytes()
                    .checked_sub(self.source_bytes)?,
            ),
        )
    }

    pub(crate) fn register_source(&mut self) -> bool {
        let Some(files) = self.source_files.checked_add(1) else {
            return false;
        };
        if files > self.limits.max_schema_files() {
            return false;
        }
        self.source_files = files;
        true
    }

    pub(crate) fn charge_source_bytes(&mut self, bytes: usize) -> Option<bool> {
        let total = self.source_bytes.checked_add(bytes)?;
        self.source_bytes = total;
        Some(total <= self.limits.max_schema_total_bytes())
    }

    pub(crate) fn source_bytes_remaining(&self) -> usize {
        self.limits
            .max_schema_total_bytes()
            .saturating_sub(self.source_bytes)
    }
}

fn root_span(source_len: usize) -> SourceSpan {
    SourceSpan::new(SourceId::new(0), Span::from_zero_to(source_len))
}
