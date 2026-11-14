mod imports;
mod input;
mod support;

use core::cell::RefCell;
use opeo::{ErrSlot, OResult, Out};
use typol_core::{
    BudgetError, ChargedVec, Diagnostic, DiagnosticLabelInput, DiagnosticMessage, ExecutionBudget,
    Read, SourceCapture, SourceId, SourceReadError, SourceSpan,
};
use typol_syntax::{
    AstAccessError, SchemaAst, SchemaParseOptions, SchemaParseOutput, SchemaParser,
    SourcePositionIndex, SyntaxErrors,
};

use crate::{
    ResolverError, SchemaCompileContext, SchemaCompileOptions, SchemaLimits, SchemaResolver,
    SchemaSource, SourceLoadedRequest, SourceObserver, SourceObserverError,
    diagnostics::{BudgetDiagnostics, DiagnosticCollector, EmergencyDiagnostics},
    session::{CompileBudget, ObjectState, SourceCatalog},
};
use imports::ImportLoader;
use input::BoundedSource;
use support::LoaderSupport;

#[derive(Clone, Copy, Eq, PartialEq)]
enum VisitState {
    Loading,
    Loaded,
    Failed,
}

#[derive(Clone, Copy)]
pub(crate) enum ImportEdge {
    Pending,
    Resolved { target: usize },
    Failed { source: Option<usize> },
}

pub(crate) struct LoadedFile {
    identity: ChargedVec<u8>,
    source_id: SourceId,
    ast: Option<SchemaAst>,
    source_len: usize,
    positions: Option<SourcePositionIndex>,
    capture: RefCell<SourceCapture>,
    visit: VisitState,
    state: ObjectState,
    import_targets: ChargedVec<ImportEdge>,
}

pub(crate) struct SourceLoadRequest<
    'source,
    'resolver,
    'budget,
    'diagnostics,
    'observer,
    R: ?Sized,
    S: Read<SourceReadError>,
> {
    pub(crate) root: SchemaSource<'source, S>,
    pub(crate) resolver: &'resolver mut R,
    pub(crate) budget: &'budget mut CompileBudget,
    pub(crate) diagnostics: &'diagnostics mut DiagnosticCollector,
    pub(crate) observer: Option<&'observer mut dyn SourceObserver>,
}

impl LoadedFile {
    pub(crate) const fn source_id(&self) -> SourceId {
        self.source_id
    }

    pub(crate) fn ast(&self) -> Option<&SchemaAst> {
        self.ast.as_ref()
    }

    pub(crate) fn parsed_ast(&self) -> Option<&SchemaAst> {
        self.ast
            .as_ref()
            .filter(|_| self.state == ObjectState::Valid)
    }

    pub(crate) const fn source_len(&self) -> usize {
        self.source_len
    }

    pub(crate) fn positions(&self) -> Option<&SourcePositionIndex> {
        self.positions.as_ref()
    }

    pub(crate) fn capture_mut(&mut self) -> &mut SourceCapture {
        self.capture.get_mut()
    }

    pub(crate) fn capture_cell(&self) -> &RefCell<SourceCapture> {
        &self.capture
    }

    /// Each import edge has an explicit variant; failed edges only retain an optional source-file index.
    pub(crate) fn import_targets(&self) -> &[ImportEdge] {
        &self.import_targets
    }

    pub(crate) fn identity(&self) -> &[u8] {
        self.identity.as_slice()
    }

    pub(crate) fn identity_len(&self) -> usize {
        self.identity.len()
    }

    pub(crate) fn is_loading(&self) -> bool {
        self.visit == VisitState::Loading
    }

    pub(crate) fn mark_loaded(&mut self) {
        self.visit = VisitState::Loaded;
    }

    pub(crate) fn push_pending_import(
        &mut self,
        errors: &mut ErrSlot<BudgetError>,
    ) -> Result<(), BudgetError> {
        errors
            .try_call(|out| self.import_targets.try_reserve(1, out))
            .map_err(|caught| caught.take())?;
        errors
            .try_call(|out| self.import_targets.try_push(ImportEdge::Pending, out))
            .map_err(|caught| caught.take())
    }

    pub(crate) fn set_last_import_edge(&mut self, edge: ImportEdge) -> bool {
        let Some(last_edge) = self.import_targets.last_mut() else {
            return false;
        };
        *last_edge = edge;
        true
    }
}

pub(super) struct LoadContext<'a, 'observer> {
    pub(super) limits: &'a SchemaLimits,
    pub(super) execution: &'a ExecutionBudget,
    pub(super) session: SchemaCompileContext,
    pub(super) diagnostics: &'a mut DiagnosticCollector,
    pub(super) observer: Option<&'observer mut dyn SourceObserver>,
    pub(super) budget_errors: ErrSlot<BudgetError>,
    pub(super) ast_errors: ErrSlot<AstAccessError>,
    pub(super) syntax_errors: ErrSlot<SyntaxErrors>,
    pub(super) resolver_errors: ErrSlot<ResolverError>,
    pub(super) observer_errors: ErrSlot<SourceObserverError>,
}

struct LoadedSourceInput<'src> {
    input: &'src mut dyn Read<SourceReadError>,
    source_id: SourceId,
    span: SourceSpan,
}

struct SourceParseOutcome {
    parsed: Result<SchemaParseOutput, SyntaxErrors>,
    source_len: usize,
    reached_eof: bool,
}

fn copy_diagnostic(
    diagnostic: &Diagnostic,
    execution: &ExecutionBudget,
    errors: &mut ErrSlot<BudgetError>,
) -> Result<Diagnostic, BudgetError> {
    errors
        .try_call(|mut out| {
            let mut copied = match diagnostic.message() {
                Ok(message) => Diagnostic::try_new(
                    diagnostic.code(),
                    DiagnosticMessage::custom(message),
                    Some(execution),
                    out.reborrow(),
                )?,
                Err(error) => Diagnostic::failure(diagnostic.code(), error),
            };
            copied.set_located_primary(diagnostic.located_primary());
            copied.set_context(diagnostic.context());
            copied.set_value_source(diagnostic.value_source());
            for label in diagnostic.labels() {
                copied.try_add_label_with_text(
                    DiagnosticLabelInput {
                        span: label.located_span(),
                        text: label.message(),
                        role: label.role(),
                    },
                    Some(execution),
                    out.reborrow(),
                )?;
            }
            Ok(copied)
        })
        .map_err(|caught| caught.take())
}

pub(crate) struct SourceLoader;

impl SourceLoader {
    pub(crate) fn load<'slot, E, R: SchemaResolver + ?Sized, S: Read<SourceReadError>>(
        request: SourceLoadRequest<'_, '_, '_, '_, '_, R, S>,
        mut out: Out<'slot, '_, E>,
        map_error: impl Fn(Diagnostic) -> E,
    ) -> OResult<'slot, SourceCatalog> {
        let SourceLoadRequest {
            root,
            resolver,
            budget,
            diagnostics,
            observer,
        } = request;
        let limits = *budget.limits();
        let limits = &limits;
        let root_span = LoaderSupport::source_span(SourceId::new(0), 0);
        let mut budget_errors = ErrSlot::<BudgetError>::new();
        let execution = match budget_errors.try_call(|out| budget.execution().try_clone(out)) {
            Ok(execution) => execution,
            Err(caught) => {
                return Err(out.reborrow().fail(map_error(BudgetDiagnostics::make(
                    caught.take(),
                    root_span,
                    diagnostics.translator(),
                ))));
            }
        };
        let mut loaded = match budget_errors.try_call(|out| SourceCatalog::new(&execution, out)) {
            Ok(loaded) => loaded,
            Err(caught) => {
                return Err(out.reborrow().fail(map_error(BudgetDiagnostics::make(
                    caught.take(),
                    root_span,
                    diagnostics.translator(),
                ))));
            }
        };
        let session_options = SchemaCompileOptions::new(*limits)
            .with_regex(*budget.regex_options())
            .with_translator(diagnostics.translator());
        let session = match budget_errors.try_call(|out| {
            SchemaCompileContext::new(session_options, &execution, budget.regex_execution(), out)
        }) {
            Ok(session) => session,
            Err(caught) => {
                return Err(out.reborrow().fail(map_error(BudgetDiagnostics::make(
                    caught.take(),
                    root_span,
                    diagnostics.translator(),
                ))));
            }
        };
        let mut context = LoadContext {
            limits,
            execution: &execution,
            session,
            diagnostics,
            observer,
            budget_errors,
            ast_errors: ErrSlot::new(),
            syntax_errors: ErrSlot::new(),
            resolver_errors: ErrSlot::new(),
            observer_errors: ErrSlot::new(),
        };
        let diagnostics = &mut context.diagnostics;
        if !budget.register_source() {
            diagnostics.push(EmergencyDiagnostics::resource(
                root_span,
                diagnostics.translator(),
            ));
            return Ok(loaded);
        }
        if !Self::append(loaded.files_mut(), root, root_span, budget, &mut context) {
            return Ok(loaded);
        }
        ImportLoader::load(&mut loaded, resolver, budget, &mut context, root_span);
        Ok(loaded)
    }

    pub(super) fn append<S: Read<SourceReadError>>(
        files: &mut ChargedVec<LoadedFile>,
        source: SchemaSource<'_, S>,
        span: SourceSpan,
        budget: &mut CompileBudget,
        context: &mut LoadContext<'_, '_>,
    ) -> bool {
        append_source(files, source, span, budget, context)
    }
}

fn append_source<S: Read<SourceReadError>>(
    files: &mut ChargedVec<LoadedFile>,
    source: SchemaSource<'_, S>,
    span: SourceSpan,
    budget: &mut CompileBudget,
    context: &mut LoadContext<'_, '_>,
) -> bool {
    let Ok(id) = u32::try_from(files.len()) else {
        context.diagnostics.push(EmergencyDiagnostics::resource(
            span,
            context.diagnostics.translator(),
        ));
        return false;
    };
    let source_id = SourceId::new(id);
    let (source_identity, mut input) = source.into_parts();
    if !register_source(files, source_identity, source_id, span, context) {
        return false;
    }
    parse_source(
        files,
        budget,
        context,
        LoadedSourceInput {
            input: &mut input,
            source_id,
            span,
        },
    )
}

fn register_source(
    files: &mut ChargedVec<LoadedFile>,
    source_identity: &[u8],
    source_id: SourceId,
    span: SourceSpan,
    context: &mut LoadContext<'_, '_>,
) -> bool {
    let execution = context.execution;
    let identity = LoaderSupport::budget_call(&mut context.budget_errors, |mut out| {
        let mut identity = ChargedVec::new(Some(execution), out.reborrow())?;
        identity.try_reserve_exact(source_identity.len(), out.reborrow())?;
        identity.extend_from_slice(source_identity, out.reborrow())?;
        Ok(identity)
    });
    let identity = match identity {
        Ok(identity) => identity,
        Err(error) => {
            context.diagnostics.push(BudgetDiagnostics::make(
                error,
                span,
                context.diagnostics.translator(),
            ));
            return false;
        }
    };
    if let Err(error) =
        LoaderSupport::budget_call(&mut context.budget_errors, |out| files.try_reserve(1, out))
    {
        context.diagnostics.push(BudgetDiagnostics::make(
            error,
            span,
            context.diagnostics.translator(),
        ));
        return false;
    }
    let import_targets = match LoaderSupport::budget_call(&mut context.budget_errors, |out| {
        ChargedVec::new(Some(execution), out)
    }) {
        Ok(import_targets) => import_targets,
        Err(error) => {
            context.diagnostics.push(BudgetDiagnostics::make(
                error,
                span,
                context.diagnostics.translator(),
            ));
            return false;
        }
    };
    let capture = RefCell::new(SourceCapture::new(
        source_id,
        LoaderSupport::source_excerpt_limits(context.limits),
        Some(execution),
    ));
    if let Err(error) = LoaderSupport::budget_call(&mut context.budget_errors, |out| {
        files.try_push(
            LoadedFile {
                identity,
                source_id,
                ast: None,
                source_len: 0,
                positions: None,
                capture,
                visit: VisitState::Failed,
                state: ObjectState::Pending,
                import_targets,
            },
            out,
        )
    }) {
        context.diagnostics.push(BudgetDiagnostics::make(
            error,
            span,
            context.diagnostics.translator(),
        ));
        return false;
    }
    if let Some(observer) = &mut context.observer {
        let Some(file) = files.last() else {
            context
                .diagnostics
                .abort(schema_internal!(context.diagnostics.translator(), span));
            return false;
        };
        let request = SourceLoadedRequest {
            source_id,
            identity: file.identity.as_slice(),
            context: &context.session,
        };
        let observed = context
            .observer_errors
            .try_call(|out| observer.source_loaded(request, out));
        if let Err(caught) = observed {
            let error = caught.take();
            let Some(file) = files.last_mut() else {
                context
                    .diagnostics
                    .abort(schema_internal!(context.diagnostics.translator(), span));
                return false;
            };
            file.state = ObjectState::Invalid;
            file.capture.get_mut().invalidate();
            context
                .diagnostics
                .abort(LoaderSupport::observer_diagnostic(
                    error,
                    LoaderSupport::source_span(source_id, 0),
                    context.diagnostics,
                ));
            return false;
        }
    }
    true
}

fn parse_source(
    files: &mut ChargedVec<LoadedFile>,
    budget: &mut CompileBudget,
    context: &mut LoadContext<'_, '_>,
    input: LoadedSourceInput<'_>,
) -> bool {
    let source_id = input.source_id;
    let span = input.span;
    let mut parser_limits = *context.limits.syntax();
    parser_limits.set_max_diagnostics(context.diagnostics.remaining());
    let parser_options = SchemaParseOptions::new(parser_limits, context.limits.stream_limits())
        .with_translator(context.session.translator());
    let Some(outcome) = read_source(files, budget, context, input, &parser_options) else {
        return false;
    };
    finish_source(files, context, source_id, span, outcome)
}

fn read_source(
    files: &mut ChargedVec<LoadedFile>,
    budget: &mut CompileBudget,
    context: &mut LoadContext<'_, '_>,
    input: LoadedSourceInput<'_>,
    options: &SchemaParseOptions,
) -> Option<SourceParseOutcome> {
    let LoadedSourceInput {
        mut input,
        source_id,
        span,
    } = input;
    let Some(file) = files.last_mut() else {
        context
            .diagnostics
            .abort(schema_internal!(context.diagnostics.translator(), span));
        return None;
    };
    let mut bounded = BoundedSource::new(
        &mut input,
        budget,
        file.capture_mut(),
        source_id,
        context.limits.max_source_bytes(),
    );
    let parsed = context
        .syntax_errors
        .try_call(|out| {
            SchemaParser::parse_with_positions(
                source_id,
                &mut bounded,
                options,
                context.execution,
                out,
            )
        })
        .map_err(|caught| caught.take());
    Some(SourceParseOutcome {
        parsed,
        source_len: bounded.actual_bytes(),
        reached_eof: bounded.reached_eof(),
    })
}

fn finish_source(
    files: &mut ChargedVec<LoadedFile>,
    context: &mut LoadContext<'_, '_>,
    source_id: SourceId,
    span: SourceSpan,
    outcome: SourceParseOutcome,
) -> bool {
    let SourceParseOutcome {
        parsed,
        source_len,
        reached_eof,
    } = outcome;
    let execution = context.execution;
    match parsed {
        Ok(parsed) => {
            let (ast, positions) = parsed.into_parts();
            if !reached_eof || positions.source_len() != source_len {
                let Some(file) = files.last_mut() else {
                    context
                        .diagnostics
                        .abort(schema_internal!(context.diagnostics.translator(), span));
                    return false;
                };
                file.capture.get_mut().invalidate();
                file.state = ObjectState::Invalid;
                file.source_len = source_len;
                context.diagnostics.abort(EmergencyDiagnostics::internal(
                    LoaderSupport::source_span(source_id, source_len),
                    schema_context!("source-input", "successful-parse-without-eof"),
                    context.diagnostics.translator(),
                ));
                return false;
            }
            let Some(file) = files.last_mut() else {
                context
                    .diagnostics
                    .abort(schema_internal!(context.diagnostics.translator(), span));
                return false;
            };
            file.ast = Some(ast);
            file.source_len = source_len;
            file.positions = Some(positions);
            file.visit = VisitState::Loading;
            file.state = ObjectState::Valid;
            true
        }
        Err(errors) => {
            let Some(file) = files.last_mut() else {
                context
                    .diagnostics
                    .abort(schema_internal!(context.diagnostics.translator(), span));
                return false;
            };
            file.source_len = source_len;
            file.capture.get_mut().invalidate();
            file.state = ObjectState::Invalid;
            for source_diagnostic in errors.diagnostics() {
                let copied =
                    copy_diagnostic(source_diagnostic, execution, &mut context.budget_errors);
                match copied {
                    Ok(diagnostic) => context.diagnostics.push(diagnostic),
                    Err(error) => {
                        context.diagnostics.push(BudgetDiagnostics::make(
                            error,
                            source_diagnostic.primary().unwrap_or(span),
                            context.diagnostics.translator(),
                        ));
                        return false;
                    }
                }
            }
            false
        }
    }
}
