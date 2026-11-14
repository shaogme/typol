use opeo::{ErrSlot, OResult, Out};
use typol_semantic::SourceLookup;

use typol_core::{
    BudgetError, ChargedVec, DiagnosticCode, ExecutionBudget, Read, RegexExecutionBudget, SourceId,
    SourceReadError, SourceSpan, Span,
};
use typol_i18n::{Translator, t};
use typol_policy::{RegexLimits, RegexOptions};

use crate::{
    SchemaCompileContext, SchemaCompileStage, SchemaLimits,
    compiled::{SchemaCompileContextFailure, SchemaCompileErrors},
    diagnostics::{DiagnosticCollector, EmergencyDiagnostics},
    loader::{LoadedFile, SourceLoadRequest, SourceLoader},
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
    regex_options: RegexOptions,
    translator: Translator,
    source_bytes: usize,
    source_files: usize,
    execution: ExecutionBudget,
    regex_execution: RegexExecutionBudget,
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
        let translator = context.translator();
        let failure = if !root.matches_context(context) {
            Some(SchemaCompileContextFailure::RootContextMismatch)
        } else if context.execution().finished() {
            Some(SchemaCompileContextFailure::SessionFinished)
        } else {
            None
        };
        if let Some(failure) = failure {
            let (code, message) = match failure {
                SchemaCompileContextFailure::RootContextMismatch => (
                    DiagnosticCode::SCHEMA_CONTEXT_MISMATCH,
                    t! {
                        name: "diagnostic.schema.root_context_mismatch",
                        locale: translator,
                    },
                ),
                SchemaCompileContextFailure::SessionFinished => (
                    DiagnosticCode::SCHEMA_SESSION_FINISHED,
                    t! {
                        name: "diagnostic.schema.session_finished",
                        locale: translator,
                    },
                ),
            };
            let context = match failure {
                SchemaCompileContextFailure::RootContextMismatch => {
                    schema_context!("source-loading", "root-context-mismatch")
                }
                SchemaCompileContextFailure::SessionFinished => {
                    schema_context!("source-loading", "execution-context-finished")
                }
            };
            let mut diagnostic = EmergencyDiagnostics::translated(code, message, Some(span));
            diagnostic.set_context(Some(context));
            return Err(out.fail(SchemaCompileErrors::with_context_failure(
                diagnostic, failure, translator,
            )));
        }
        let execution = context.execution();
        let mut execution_errors = ErrSlot::new();
        if let Err(caught) =
            execution_errors.try_call(|execution_out| execution.consume(0, execution_out))
        {
            return Err(out.fail(SchemaCompileErrors::budget_failure(
                caught.take(),
                translator,
            )));
        }
        context.set_stage(SchemaCompileStage::SourceLoading);
        let limits = context.resources();
        let mut diagnostic_errors = ErrSlot::new();
        let mut diagnostics = match diagnostic_errors.try_call(|diagnostic_out| {
            DiagnosticCollector::new(
                limits.max_diagnostics(),
                span,
                execution,
                translator,
                diagnostic_out,
            )
        }) {
            Ok(diagnostics) => diagnostics,
            Err(caught) => {
                return Err(out.fail(SchemaCompileErrors::budget_failure(
                    caught.take(),
                    translator,
                )));
            }
        };
        let mut budget_errors = ErrSlot::new();
        let mut budget =
            match budget_errors.try_call(|budget_out| CompileBudget::new(context, budget_out)) {
                Ok(budget) => budget,
                Err(caught) => {
                    return Err(out.fail(SchemaCompileErrors::budget_failure(
                        caught.take(),
                        translator,
                    )));
                }
            };
        let sources = SourceLoader::load(
            SourceLoadRequest {
                root,
                resolver,
                budget: &mut budget,
                diagnostics: &mut diagnostics,
                observer,
            },
            out.reborrow(),
            |diagnostic| SchemaCompileErrors::inline(diagnostic, translator),
        )?;
        Ok(Self {
            sources,
            diagnostics,
            budget,
        })
    }
}

impl CompileBudget {
    pub(crate) fn new<'slot, E>(
        context: &SchemaCompileContext,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, Self>
    where
        E: From<BudgetError>,
    {
        let execution = context.execution().try_clone(out.reborrow())?;
        let regex_execution = context.regex_execution().try_clone(out.reborrow())?;
        Ok(Self {
            limits: *context.resources(),
            regex_options: *context.regex(),
            translator: context.translator(),
            source_bytes: 0,
            source_files: 0,
            execution,
            regex_execution,
        })
    }

    pub(crate) fn execution(&self) -> &ExecutionBudget {
        &self.execution
    }

    pub(crate) fn regex_limits(&self) -> &RegexLimits {
        self.regex_options.limits()
    }

    pub(crate) fn regex_options(&self) -> &RegexOptions {
        &self.regex_options
    }

    pub(crate) fn regex_execution(&self) -> &RegexExecutionBudget {
        &self.regex_execution
    }

    pub(crate) const fn translator(&self) -> Translator {
        self.translator
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
