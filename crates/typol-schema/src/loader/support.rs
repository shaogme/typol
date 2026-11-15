use opeo::{ErrSlot, OResult, Out};
use typol_core::{
    BudgetError, Diagnostic, DiagnosticCode, ExcerptLimits, ExecutionBudget, SourceId, SourceSpan,
    Span,
};
use typol_i18n::t;
use typol_syntax::{AstAccessError, SchemaAst};

use crate::{
    ResolverError, SchemaLimits, SchemaResolveRequest, SchemaResolver, SchemaSource,
    SourceObserverError,
    diagnostics::{DiagnosticCollector, EmergencyDiagnostics},
};

pub(super) enum ExternFailure {
    Budget(BudgetError),
    AstAccess(AstAccessError),
}

pub(super) struct ImportRequest {
    pub(super) item_index: usize,
    pub(super) span: SourceSpan,
}

pub(super) struct LoaderSupport;

impl LoaderSupport {
    pub(super) fn budget_call<T>(
        errors: &mut ErrSlot<BudgetError>,
        operation: impl for<'slot> FnOnce(Out<'slot, 'slot, BudgetError>) -> OResult<'slot, T>,
    ) -> Result<T, BudgetError> {
        errors.try_call(operation).map_err(|caught| caught.take())
    }

    pub(super) fn resolver_call<'src, R: SchemaResolver + ?Sized>(
        errors: &mut ErrSlot<ResolverError>,
        resolver: &'src mut R,
        request: SchemaResolveRequest<'_>,
    ) -> Result<SchemaSource<'src>, ResolverError> {
        errors
            .try_call(|out| resolver.resolve(request, out))
            .map_err(|caught| caught.take())
    }

    pub(super) fn source_excerpt_limits(limits: &SchemaLimits) -> ExcerptLimits {
        let excerpts = limits.excerpts();
        let files = limits.max_schema_files().max(1);
        let max_bytes = excerpts.max_bytes() / files;
        ExcerptLimits::new(
            max_bytes,
            excerpts.max_line_bytes().min(max_bytes),
            excerpts.max_records() / files,
            excerpts.context_lines(),
        )
    }

    pub(super) fn next_extern(
        ast: &SchemaAst,
        start: usize,
        execution: &ExecutionBudget,
        ast_errors: &mut ErrSlot<AstAccessError>,
        budget_errors: &mut ErrSlot<BudgetError>,
    ) -> Result<Option<ImportRequest>, ExternFailure> {
        let items = ast.items();
        for index in start..items.len() {
            budget_errors
                .try_call(|out| execution.consume(1, out))
                .map_err(|caught| ExternFailure::Budget(caught.take()))?;
            let item = ast_errors
                .try_call(|out| items.get(index, out))
                .map_err(|caught| ExternFailure::AstAccess(caught.take()))?;
            let Some(item) = item else {
                continue;
            };
            if let Some(declaration) = item.extern_mod() {
                return Ok(Some(ImportRequest {
                    item_index: index,
                    span: declaration.span(),
                }));
            }
        }
        Ok(None)
    }

    pub(super) fn extern_name<'a>(
        ast: &'a SchemaAst,
        index: usize,
        errors: &mut ErrSlot<AstAccessError>,
    ) -> Result<Option<&'a str>, AstAccessError> {
        errors
            .try_call(|mut out| {
                let Some(item) = ast.items().get(index, out.reborrow())? else {
                    return Ok(None);
                };
                let Some(declaration) = item.extern_mod() else {
                    return Ok(None);
                };
                let name = declaration.name(out.reborrow())?;
                Ok(Some(name))
            })
            .map_err(|caught| caught.take())
    }

    pub(super) fn observer_diagnostic(
        error: SourceObserverError,
        span: SourceSpan,
        diagnostics: &DiagnosticCollector,
    ) -> Diagnostic {
        match error {
            SourceObserverError::ResourceLimit(limit) => {
                EmergencyDiagnostics::resource(limit, span, diagnostics.translator())
            }
            SourceObserverError::AllocationFailed => {
                EmergencyDiagnostics::allocation(span, diagnostics.translator())
            }
            SourceObserverError::CapacityExceeded(capacity) => {
                EmergencyDiagnostics::capacity_error(span, capacity, diagnostics.translator())
            }
            SourceObserverError::Internal(error) => {
                EmergencyDiagnostics::internal_error(span, error, diagnostics.translator())
            }
            SourceObserverError::HostFailure => {
                let mut diagnostic = diagnostics.make(
                    DiagnosticCode::SCHEMA_RESOLVER_ERROR,
                    "host could not register schema source",
                    Some(span),
                );
                diagnostic.set_context(Some(
                    schema_context!("source-observer", "source-loaded-callback-failed")
                        .with_object_id(span.source_id().get() as u64),
                ));
                diagnostic
            }
        }
    }

    pub(super) fn resolver_diagnostic(
        error: ResolverError,
        span: SourceSpan,
        diagnostics: &DiagnosticCollector,
    ) -> Diagnostic {
        match error {
            ResolverError::NotFound => diagnostics.make_semantic(
                t! {
                    name: "diagnostic.schema.imported_file_not_found",
                    locale: diagnostics.translator(),
                },
                span,
            ),
            ResolverError::ImportsNotSupported => diagnostics.make_semantic(
                t! {
                    name: "diagnostic.schema.imports_require_resolver",
                    locale: diagnostics.translator(),
                },
                span,
            ),
            ResolverError::ResourceLimit(limit) => {
                EmergencyDiagnostics::resource(limit, span, diagnostics.translator())
            }
            ResolverError::AllocationFailed => {
                EmergencyDiagnostics::allocation(span, diagnostics.translator())
            }
            ResolverError::CapacityExceeded(capacity) => {
                EmergencyDiagnostics::capacity_error(span, capacity, diagnostics.translator())
            }
            ResolverError::Internal(error) => {
                EmergencyDiagnostics::internal_error(span, error, diagnostics.translator())
            }
            ResolverError::HostFailure | ResolverError::ContextMismatch => {
                let context = if error == ResolverError::ContextMismatch {
                    schema_context!("resolver-callback", "resolved-source-context-mismatch")
                } else {
                    schema_context!("resolver-callback", "resolve-callback-failed")
                };
                let mut diagnostic = diagnostics.make(
                    DiagnosticCode::SCHEMA_RESOLVER_ERROR,
                    "host could not provide the imported schema file",
                    Some(span),
                );
                diagnostic.set_context(Some(context.with_object_id(span.source_id().get() as u64)));
                diagnostic
            }
        }
    }

    pub(super) fn source_span(source_id: SourceId, source_len: usize) -> SourceSpan {
        SourceSpan::new(source_id, Span::from_zero_to(source_len))
    }
}
