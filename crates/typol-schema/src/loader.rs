mod input;
mod support;

use core::cell::RefCell;
use opeo::{ErrSlot, OResult, Out};
use typol_core::{
    BudgetError, ChargedVec, Diagnostic, DiagnosticMessage, ExecutionBudget, Read, SourceCapture,
    SourceId, SourceReadError, SourceSpan,
};
use typol_i18n::diagnostic_message;
use typol_syntax::{
    AstAccessError, SchemaAst, SchemaParseOptions, SchemaParseOutput, SchemaParser,
    SourcePositionIndex, SyntaxErrors,
};

use crate::{
    ResolverError, SchemaCompileContext, SchemaLimits, SchemaResolveRequest, SchemaResolver,
    SchemaSource, SourceLoadedRequest, SourceObserver, SourceObserverError,
    diagnostics::{BudgetDiagnostics, DiagnosticCollector, EmergencyDiagnostics},
    session::{CompileBudget, ObjectState, SourceCatalog},
};
use input::BoundedSource;
use support::{ExternFailure, ImportRequest, LoaderSupport};

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
}

struct Frame {
    file_index: usize,
    next_item_index: usize,
}

struct LoadContext<'a, 'observer> {
    limits: &'a SchemaLimits,
    execution: &'a ExecutionBudget,
    session: SchemaCompileContext,
    diagnostics: &'a mut DiagnosticCollector,
    observer: Option<&'observer mut dyn SourceObserver>,
    budget_errors: ErrSlot<BudgetError>,
    ast_errors: ErrSlot<AstAccessError>,
    syntax_errors: ErrSlot<SyntaxErrors>,
    resolver_errors: ErrSlot<ResolverError>,
    observer_errors: ErrSlot<SourceObserverError>,
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

fn budget_call<T>(
    errors: &mut ErrSlot<BudgetError>,
    operation: impl for<'slot> FnOnce(Out<'slot, 'slot, BudgetError>) -> OResult<'slot, T>,
) -> Result<T, BudgetError> {
    errors.try_call(operation).map_err(|caught| caught.take())
}

fn copy_diagnostic(
    diagnostic: &Diagnostic,
    execution: &ExecutionBudget,
    errors: &mut ErrSlot<BudgetError>,
) -> Result<Diagnostic, BudgetError> {
    errors
        .try_call(|mut out| {
            let mut copied = match diagnostic.message_key() {
                Some(key) => Diagnostic::try_new(
                    diagnostic.code(),
                    DiagnosticMessage::localized(diagnostic.message(), key),
                    Some(execution),
                    out.reborrow(),
                )?,
                None => Diagnostic::try_new(
                    diagnostic.code(),
                    diagnostic.message(),
                    Some(execution),
                    out.reborrow(),
                )?,
            };
            copied.set_located_primary(diagnostic.located_primary());
            copied.set_context(diagnostic.context());
            copied.set_value_source(diagnostic.value_source());
            for label in diagnostic.labels() {
                copied.try_add_label_with_text(
                    label.located_span(),
                    match label.key() {
                        Some(key) => DiagnosticMessage::localized(label.message(), key),
                        None => DiagnosticMessage::custom(label.message()),
                    },
                    Some(execution),
                    out.reborrow(),
                )?;
            }
            Ok(copied)
        })
        .map_err(|caught| caught.take())
}

fn resolver_call<'src, R: SchemaResolver + ?Sized>(
    errors: &mut ErrSlot<ResolverError>,
    resolver: &'src mut R,
    request: SchemaResolveRequest<'_>,
) -> Result<SchemaSource<'src>, ResolverError> {
    errors
        .try_call(|out| resolver.resolve(request, out))
        .map_err(|caught| caught.take())
}

pub(crate) struct SourceLoader;

impl SourceLoader {
    pub(crate) fn load<R: SchemaResolver + ?Sized, S: Read<SourceReadError>>(
        root: SchemaSource<'_, S>,
        resolver: &mut R,
        budget: &mut CompileBudget,
        diagnostics: &mut DiagnosticCollector,
        observer: Option<&mut dyn SourceObserver>,
    ) -> Result<SourceCatalog, Diagnostic> {
        let limits = *budget.limits();
        let limits = &limits;
        let root_span = LoaderSupport::source_span(SourceId::new(0), 0);
        let mut budget_errors = ErrSlot::<BudgetError>::new();
        let execution = match budget_errors.try_call(|out| budget.execution().try_clone(out)) {
            Ok(execution) => execution,
            Err(caught) => return Err(BudgetDiagnostics::make(caught.take(), root_span)),
        };
        let mut loaded = match budget_errors.try_call(|out| SourceCatalog::new(&execution, out)) {
            Ok(loaded) => loaded,
            Err(caught) => return Err(BudgetDiagnostics::make(caught.take(), root_span)),
        };
        let session = match budget_errors
            .try_call(|out| SchemaCompileContext::new(*limits, &execution, out))
        {
            Ok(session) => session,
            Err(caught) => return Err(BudgetDiagnostics::make(caught.take(), root_span)),
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
            diagnostics.push(EmergencyDiagnostics::resource(root_span));
            return Ok(loaded);
        }
        if !append_source(loaded.files_mut(), root, root_span, budget, &mut context) {
            return Ok(loaded);
        }
        load_imports(&mut loaded, resolver, budget, &mut context, root_span);
        Ok(loaded)
    }
}

fn load_imports<R: SchemaResolver + ?Sized>(
    loaded: &mut SourceCatalog,
    resolver: &mut R,
    budget: &mut CompileBudget,
    context: &mut LoadContext<'_, '_>,
    root_span: SourceSpan,
) {
    let execution = context.execution;
    let diagnostics = &mut context.diagnostics;
    let mut stack = match budget_call(&mut context.budget_errors, |out| {
        ChargedVec::new(Some(execution), out)
    }) {
        Ok(stack) => stack,
        Err(error) => {
            diagnostics.push(BudgetDiagnostics::make(error, root_span));
            return;
        }
    };
    if let Err(error) = budget_call(&mut context.budget_errors, |out| stack.try_reserve(1, out)) {
        diagnostics.push(BudgetDiagnostics::make(error, root_span));
        return;
    }
    if let Err(error) = budget_call(&mut context.budget_errors, |out| {
        stack.try_push(
            Frame {
                file_index: 0,
                next_item_index: 0,
            },
            out,
        )
    }) {
        diagnostics.push(BudgetDiagnostics::make(error, root_span));
        return;
    }
    while !stack.is_empty() && !context.diagnostics.is_stopped() {
        let diagnostics = &mut context.diagnostics;
        if let Err(error) = budget_call(&mut context.budget_errors, |out| execution.consume(1, out))
        {
            diagnostics.abort(BudgetDiagnostics::make(error, root_span));
            break;
        }
        let Some(frame) = stack.last() else {
            diagnostics.abort(schema_internal!(root_span));
            break;
        };
        let file_index = frame.file_index;
        let Some(file) = loaded.files().get(file_index) else {
            diagnostics.abort(schema_internal!(root_span));
            break;
        };
        let file_span = LoaderSupport::source_span(file.source_id(), file.source_len());
        let Some(ast) = file.parsed_ast() else {
            diagnostics.abort(schema_internal!(file_span));
            break;
        };
        let next = match LoaderSupport::next_extern(
            ast,
            frame.next_item_index,
            execution,
            &mut context.ast_errors,
            &mut context.budget_errors,
        ) {
            Ok(next) => next,
            Err(ExternFailure::Budget(error)) => {
                diagnostics.push(BudgetDiagnostics::make(error, file_span));
                break;
            }
            Err(ExternFailure::AstAccess(error)) => {
                let context = EmergencyDiagnostics::ast_access_context(error);
                diagnostics.abort(EmergencyDiagnostics::internal(file_span, context));
                break;
            }
        };
        let Some(ImportRequest {
            item_index,
            span: import_span,
        }) = next
        else {
            let Some(file) = loaded.files_mut().get_mut(file_index) else {
                diagnostics.abort(schema_internal!(file_span));
                break;
            };
            file.visit = VisitState::Loaded;
            stack.pop();
            continue;
        };
        let Some(frame) = stack.last_mut() else {
            diagnostics.abort(schema_internal!(import_span));
            break;
        };
        let Some(next_item_index) = item_index.checked_add(1) else {
            diagnostics.push(EmergencyDiagnostics::capacity(import_span));
            break;
        };
        frame.next_item_index = next_item_index;
        // Depth and file count are checked before calling the host; no further reads occur after either limit fails.
        let Some(byte_limit) = budget.request_limit(stack.len()) else {
            diagnostics.push(EmergencyDiagnostics::resource(import_span));
            break;
        };
        let Some(file) = loaded.files_mut().get_mut(file_index) else {
            diagnostics.abort(schema_internal!(import_span));
            break;
        };
        let targets = &mut file.import_targets;
        if let Err(error) = budget_call(&mut context.budget_errors, |out| {
            targets.try_reserve(1, out)
        }) {
            diagnostics.push(BudgetDiagnostics::make(error, import_span));
            break;
        }
        if let Err(error) = budget_call(&mut context.budget_errors, |out| {
            targets.try_push(ImportEdge::Pending, out)
        }) {
            diagnostics.push(BudgetDiagnostics::make(error, import_span));
            break;
        }
        let name = match loaded
            .files()
            .get(file_index)
            .and_then(LoadedFile::parsed_ast)
        {
            Some(ast) => match LoaderSupport::extern_name(ast, item_index, &mut context.ast_errors)
            {
                Ok(name) => name,
                Err(error) => {
                    let context = EmergencyDiagnostics::ast_access_context(error);
                    diagnostics.abort(EmergencyDiagnostics::internal(import_span, context));
                    break;
                }
            },
            None => None,
        };
        let Some(name) = name else {
            diagnostics.abort(EmergencyDiagnostics::internal(
                import_span,
                schema_context!("source-loader", "extern-declaration-missing")
                    .with_object_id(item_index as u64),
            ));
            break;
        };
        let Some(from_identity) = loaded
            .files()
            .get(file_index)
            .map(|file| file.identity.as_slice())
        else {
            diagnostics.abort(EmergencyDiagnostics::internal(
                import_span,
                schema_context!("source-loader", "current-source-identity-missing")
                    .with_object_id(file_index as u64),
            ));
            break;
        };
        let request = SchemaResolveRequest {
            from_identity,
            module_name: name,
            source_byte_limit: byte_limit,
            context: &context.session,
        };
        let resolved = resolver_call(&mut context.resolver_errors, resolver, request);
        let source = match resolved {
            Ok(source) => source,
            Err(error) => {
                if !set_last_edge(
                    loaded.files_mut(),
                    file_index,
                    ImportEdge::Failed { source: None },
                    import_span,
                    diagnostics,
                ) {
                    break;
                }
                if error == ResolverError::ContextMismatch {
                    diagnostics.abort(LoaderSupport::resolver_diagnostic(
                        error,
                        import_span,
                        diagnostics,
                    ));
                    break;
                }
                diagnostics.push(LoaderSupport::resolver_diagnostic(
                    error,
                    import_span,
                    diagnostics,
                ));
                continue;
            }
        };
        if !source.matches_context(&context.session) {
            if !set_last_edge(
                loaded.files_mut(),
                file_index,
                ImportEdge::Failed { source: None },
                import_span,
                diagnostics,
            ) {
                break;
            }
            diagnostics.abort(LoaderSupport::resolver_diagnostic(
                ResolverError::ContextMismatch,
                import_span,
                diagnostics,
            ));
            break;
        }
        let mut existing = None;
        for (index, file) in loaded.files().iter().enumerate() {
            if let Err(error) =
                budget_call(&mut context.budget_errors, |out| execution.consume(1, out))
            {
                diagnostics.abort(BudgetDiagnostics::make(error, import_span));
                return;
            }
            if file.identity.len() != source.identity().len() {
                continue;
            }
            if let Err(error) = budget_call(&mut context.budget_errors, |out| {
                execution.consume(file.identity.len(), out)
            }) {
                diagnostics.abort(BudgetDiagnostics::make(error, import_span));
                return;
            }
            if file.identity.as_slice() == source.identity() {
                existing = Some(index);
                break;
            }
        }
        if let Some(existing) = existing {
            if !set_last_edge(
                loaded.files_mut(),
                file_index,
                ImportEdge::Failed { source: None },
                import_span,
                diagnostics,
            ) {
                break;
            }
            let Some(existing_file) = loaded.files().get(existing) else {
                diagnostics.abort(schema_internal!(import_span));
                break;
            };
            diagnostics.push(diagnostics.make_semantic(
                if existing_file.visit == VisitState::Loading {
                    diagnostic_message!(
                        "diagnostic.schema.import_cycle",
                        "schema import cycle detected"
                    )
                } else {
                    diagnostic_message!(
                        "diagnostic.schema.repeated_source_import",
                        "the same schema source identity was imported more than once"
                    )
                },
                import_span,
            ));
            continue;
        }
        let child_index = loaded.files().len();
        if !budget.register_source() {
            if !set_last_edge(
                loaded.files_mut(),
                file_index,
                ImportEdge::Failed { source: None },
                import_span,
                diagnostics,
            ) {
                break;
            }
            diagnostics.push(EmergencyDiagnostics::resource(import_span));
            break;
        }
        let valid = append_source(loaded.files_mut(), source, import_span, budget, context);
        let diagnostics = &mut context.diagnostics;
        let source_index = (loaded.files().len() > child_index).then_some(child_index);
        let edge_state = match (valid, source_index) {
            (true, Some(target)) => ImportEdge::Resolved { target },
            (true, None) => {
                diagnostics.abort(schema_internal!(import_span));
                break;
            }
            (false, source) => ImportEdge::Failed { source },
        };
        if !set_last_edge(
            loaded.files_mut(),
            file_index,
            edge_state,
            import_span,
            diagnostics,
        ) {
            break;
        }
        if !valid {
            continue;
        }
        if let Err(error) = budget_call(&mut context.budget_errors, |out| stack.try_reserve(1, out))
        {
            diagnostics.push(BudgetDiagnostics::make(error, import_span));
            break;
        }
        if let Err(error) = budget_call(&mut context.budget_errors, |out| {
            stack.try_push(
                Frame {
                    file_index: child_index,
                    next_item_index: 0,
                },
                out,
            )
        }) {
            diagnostics.push(BudgetDiagnostics::make(error, import_span));
            break;
        }
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
        context
            .diagnostics
            .push(EmergencyDiagnostics::resource(span));
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
    let identity = budget_call(&mut context.budget_errors, |mut out| {
        let mut identity = ChargedVec::new(Some(execution), out.reborrow())?;
        identity.try_reserve_exact(source_identity.len(), out.reborrow())?;
        identity.extend_from_slice(source_identity, out.reborrow())?;
        Ok(identity)
    });
    let identity = match identity {
        Ok(identity) => identity,
        Err(error) => {
            context
                .diagnostics
                .push(BudgetDiagnostics::make(error, span));
            return false;
        }
    };
    if let Err(error) = budget_call(&mut context.budget_errors, |out| files.try_reserve(1, out)) {
        context
            .diagnostics
            .push(BudgetDiagnostics::make(error, span));
        return false;
    }
    let import_targets = match budget_call(&mut context.budget_errors, |out| {
        ChargedVec::new(Some(execution), out)
    }) {
        Ok(import_targets) => import_targets,
        Err(error) => {
            context
                .diagnostics
                .push(BudgetDiagnostics::make(error, span));
            return false;
        }
    };
    let capture = RefCell::new(SourceCapture::new(
        source_id,
        LoaderSupport::source_excerpt_limits(context.limits),
        Some(execution),
    ));
    if let Err(error) = budget_call(&mut context.budget_errors, |out| {
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
        context
            .diagnostics
            .push(BudgetDiagnostics::make(error, span));
        return false;
    }
    if let Some(observer) = &mut context.observer {
        let Some(file) = files.last() else {
            context.diagnostics.abort(schema_internal!(span));
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
                context.diagnostics.abort(schema_internal!(span));
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
    let parser_options = SchemaParseOptions::new(parser_limits, context.limits.stream_limits());
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
        context.diagnostics.abort(schema_internal!(span));
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
                    context.diagnostics.abort(schema_internal!(span));
                    return false;
                };
                file.capture.get_mut().invalidate();
                file.state = ObjectState::Invalid;
                file.source_len = source_len;
                context.diagnostics.abort(EmergencyDiagnostics::internal(
                    LoaderSupport::source_span(source_id, source_len),
                    schema_context!("source-input", "successful-parse-without-eof"),
                ));
                return false;
            }
            let Some(file) = files.last_mut() else {
                context.diagnostics.abort(schema_internal!(span));
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
                context.diagnostics.abort(schema_internal!(span));
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
                        ));
                        return false;
                    }
                }
            }
            false
        }
    }
}

fn set_last_edge(
    files: &mut ChargedVec<LoadedFile>,
    file_index: usize,
    edge_state: ImportEdge,
    span: SourceSpan,
    diagnostics: &mut DiagnosticCollector,
) -> bool {
    let Some(edge) = files
        .get_mut(file_index)
        .and_then(|file| file.import_targets.last_mut())
    else {
        diagnostics.abort(schema_internal!(span));
        return false;
    };
    *edge = edge_state;
    true
}
