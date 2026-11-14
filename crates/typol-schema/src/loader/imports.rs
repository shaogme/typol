use typol_core::{ChargedVec, SourceSpan};
use typol_i18n::t;

use crate::{
    ResolverError, SchemaResolveRequest, SchemaResolver,
    diagnostics::{BudgetDiagnostics, DiagnosticCollector, EmergencyDiagnostics},
    loader::{ImportEdge, LoadContext, LoadedFile, SourceLoader},
    session::{CompileBudget, SourceCatalog},
};

use super::support::{ExternFailure, ImportRequest, LoaderSupport};

struct Frame {
    file_index: usize,
    next_item_index: usize,
}

pub(super) struct ImportLoader;

impl ImportLoader {
    pub(super) fn load<R: SchemaResolver + ?Sized>(
        loaded: &mut SourceCatalog,
        resolver: &mut R,
        budget: &mut CompileBudget,
        context: &mut LoadContext<'_, '_>,
        root_span: SourceSpan,
    ) {
        let execution = context.execution;
        let diagnostics = &mut context.diagnostics;
        let mut stack = match LoaderSupport::budget_call(&mut context.budget_errors, |out| {
            ChargedVec::new(Some(execution), out)
        }) {
            Ok(stack) => stack,
            Err(error) => {
                diagnostics.push(BudgetDiagnostics::make(
                    error,
                    root_span,
                    diagnostics.translator(),
                ));
                return;
            }
        };
        if let Err(error) =
            LoaderSupport::budget_call(&mut context.budget_errors, |out| stack.try_reserve(1, out))
        {
            diagnostics.push(BudgetDiagnostics::make(
                error,
                root_span,
                diagnostics.translator(),
            ));
            return;
        }
        if let Err(error) = LoaderSupport::budget_call(&mut context.budget_errors, |out| {
            stack.try_push(
                Frame {
                    file_index: 0,
                    next_item_index: 0,
                },
                out,
            )
        }) {
            diagnostics.push(BudgetDiagnostics::make(
                error,
                root_span,
                diagnostics.translator(),
            ));
            return;
        }
        while !stack.is_empty() && !context.diagnostics.is_stopped() {
            let diagnostics = &mut context.diagnostics;
            if let Err(error) = LoaderSupport::budget_call(&mut context.budget_errors, |out| {
                execution.consume(1, out)
            }) {
                diagnostics.abort(BudgetDiagnostics::make(
                    error,
                    root_span,
                    diagnostics.translator(),
                ));
                break;
            }
            let Some(frame) = stack.last() else {
                diagnostics.abort(schema_internal!(diagnostics.translator(), root_span));
                break;
            };
            let file_index = frame.file_index;
            let Some(file) = loaded.files().get(file_index) else {
                diagnostics.abort(schema_internal!(diagnostics.translator(), root_span));
                break;
            };
            let file_span = LoaderSupport::source_span(file.source_id(), file.source_len());
            let Some(ast) = file.parsed_ast() else {
                diagnostics.abort(schema_internal!(diagnostics.translator(), file_span));
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
                    diagnostics.push(BudgetDiagnostics::make(
                        error,
                        file_span,
                        diagnostics.translator(),
                    ));
                    break;
                }
                Err(ExternFailure::AstAccess(error)) => {
                    let context = EmergencyDiagnostics::ast_access_context(error);
                    diagnostics.abort(EmergencyDiagnostics::internal(
                        file_span,
                        context,
                        diagnostics.translator(),
                    ));
                    break;
                }
            };
            let Some(ImportRequest {
                item_index,
                span: import_span,
            }) = next
            else {
                let Some(file) = loaded.files_mut().get_mut(file_index) else {
                    diagnostics.abort(schema_internal!(diagnostics.translator(), file_span));
                    break;
                };
                file.mark_loaded();
                stack.pop();
                continue;
            };
            let Some(frame) = stack.last_mut() else {
                diagnostics.abort(schema_internal!(diagnostics.translator(), import_span));
                break;
            };
            let Some(next_item_index) = item_index.checked_add(1) else {
                diagnostics.push(EmergencyDiagnostics::capacity(
                    import_span,
                    diagnostics.translator(),
                ));
                break;
            };
            frame.next_item_index = next_item_index;
            // Depth and file count are checked before calling the host; no further reads occur after either limit fails.
            let Some(byte_limit) = budget.request_limit(stack.len()) else {
                diagnostics.push(EmergencyDiagnostics::resource(
                    import_span,
                    diagnostics.translator(),
                ));
                break;
            };
            let Some(file) = loaded.files_mut().get_mut(file_index) else {
                diagnostics.abort(schema_internal!(diagnostics.translator(), import_span));
                break;
            };
            if let Err(error) = file.push_pending_import(&mut context.budget_errors) {
                diagnostics.push(BudgetDiagnostics::make(
                    error,
                    import_span,
                    diagnostics.translator(),
                ));
                break;
            }
            let name = match loaded
                .files()
                .get(file_index)
                .and_then(LoadedFile::parsed_ast)
            {
                Some(ast) => {
                    match LoaderSupport::extern_name(ast, item_index, &mut context.ast_errors) {
                        Ok(name) => name,
                        Err(error) => {
                            let context = EmergencyDiagnostics::ast_access_context(error);
                            diagnostics.abort(EmergencyDiagnostics::internal(
                                import_span,
                                context,
                                diagnostics.translator(),
                            ));
                            break;
                        }
                    }
                }
                None => None,
            };
            let Some(name) = name else {
                diagnostics.abort(EmergencyDiagnostics::internal(
                    import_span,
                    schema_context!("source-loader", "extern-declaration-missing")
                        .with_object_id(item_index as u64),
                    diagnostics.translator(),
                ));
                break;
            };
            let Some(from_identity) = loaded.files().get(file_index).map(|file| file.identity())
            else {
                diagnostics.abort(EmergencyDiagnostics::internal(
                    import_span,
                    schema_context!("source-loader", "current-source-identity-missing")
                        .with_object_id(file_index as u64),
                    diagnostics.translator(),
                ));
                break;
            };
            let request = SchemaResolveRequest {
                from_identity,
                module_name: name,
                source_byte_limit: byte_limit,
                context: &context.session,
            };
            let resolved =
                LoaderSupport::resolver_call(&mut context.resolver_errors, resolver, request);
            let source = match resolved {
                Ok(source) => source,
                Err(error) => {
                    if !Self::set_last_edge(
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
                if !Self::set_last_edge(
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
                if let Err(error) = LoaderSupport::budget_call(&mut context.budget_errors, |out| {
                    execution.consume(1, out)
                }) {
                    diagnostics.abort(BudgetDiagnostics::make(
                        error,
                        import_span,
                        diagnostics.translator(),
                    ));
                    return;
                }
                if file.identity_len() != source.identity().len() {
                    continue;
                }
                if let Err(error) = LoaderSupport::budget_call(&mut context.budget_errors, |out| {
                    execution.consume(file.identity_len(), out)
                }) {
                    diagnostics.abort(BudgetDiagnostics::make(
                        error,
                        import_span,
                        diagnostics.translator(),
                    ));
                    return;
                }
                if file.identity() == source.identity() {
                    existing = Some(index);
                    break;
                }
            }
            if let Some(existing) = existing {
                if !Self::set_last_edge(
                    loaded.files_mut(),
                    file_index,
                    ImportEdge::Failed { source: None },
                    import_span,
                    diagnostics,
                ) {
                    break;
                }
                let Some(existing_file) = loaded.files().get(existing) else {
                    diagnostics.abort(schema_internal!(diagnostics.translator(), import_span));
                    break;
                };
                diagnostics.push(diagnostics.make_semantic(
                    if existing_file.is_loading() {
                        t! {
                            name: "diagnostic.schema.import_cycle",
                            locale: diagnostics.translator(),
                        }
                    } else {
                        t! {
                            name: "diagnostic.schema.repeated_source_import",
                            locale: diagnostics.translator(),
                        }
                    },
                    import_span,
                ));
                continue;
            }
            let child_index = loaded.files().len();
            if !budget.register_source() {
                if !Self::set_last_edge(
                    loaded.files_mut(),
                    file_index,
                    ImportEdge::Failed { source: None },
                    import_span,
                    diagnostics,
                ) {
                    break;
                }
                diagnostics.push(EmergencyDiagnostics::resource(
                    import_span,
                    diagnostics.translator(),
                ));
                break;
            }
            let valid =
                SourceLoader::append(loaded.files_mut(), source, import_span, budget, context);
            let diagnostics = &mut context.diagnostics;
            let source_index = (loaded.files().len() > child_index).then_some(child_index);
            let edge_state = match (valid, source_index) {
                (true, Some(target)) => ImportEdge::Resolved { target },
                (true, None) => {
                    diagnostics.abort(schema_internal!(diagnostics.translator(), import_span));
                    break;
                }
                (false, source) => ImportEdge::Failed { source },
            };
            if !Self::set_last_edge(
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
            if let Err(error) = LoaderSupport::budget_call(&mut context.budget_errors, |out| {
                stack.try_reserve(1, out)
            }) {
                diagnostics.push(BudgetDiagnostics::make(
                    error,
                    import_span,
                    diagnostics.translator(),
                ));
                break;
            }
            if let Err(error) = LoaderSupport::budget_call(&mut context.budget_errors, |out| {
                stack.try_push(
                    Frame {
                        file_index: child_index,
                        next_item_index: 0,
                    },
                    out,
                )
            }) {
                diagnostics.push(BudgetDiagnostics::make(
                    error,
                    import_span,
                    diagnostics.translator(),
                ));
                break;
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
        let Some(file) = files.get_mut(file_index) else {
            diagnostics.abort(schema_internal!(diagnostics.translator(), span));
            return false;
        };
        if file.set_last_import_edge(edge_state) {
            return true;
        }
        diagnostics.abort(schema_internal!(diagnostics.translator(), span));
        false
    }
}
