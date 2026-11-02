use opeo::ErrSlot;
use typol_core::{
    BudgetError, Diagnostic, DiagnosticCode, ExcerptLimits, ExecutionBudget, SourceId, SourceSpan,
    Span,
};
use typol_i18n::diagnostic_message;
use typol_syntax::{AstAccessError, SchemaAst};

use crate::{
    ResolverError, SchemaLimits, SourceObserverError,
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
            SourceObserverError::ResourceLimitExceeded => EmergencyDiagnostics::resource(span),
            SourceObserverError::AllocationFailed => EmergencyDiagnostics::allocation(span),
            SourceObserverError::CapacityExceeded => EmergencyDiagnostics::capacity(span),
            SourceObserverError::InternalInvariantFailure => EmergencyDiagnostics::internal(
                span,
                schema_context!("source-observer", "source-loaded-invariant")
                    .with_object_id(span.source_id().get() as u64),
            ),
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
                diagnostic_message!("diagnostic.schema.imported_file_not_found", "imported schema file does not exist"),
                span,
            ),
            ResolverError::ImportsNotSupported => diagnostics.make_semantic(
                diagnostic_message!("diagnostic.schema.imports_require_resolver", "this schema compile entry point does not support extern mod; provide a SchemaResolver"),
                span,
            ),
            ResolverError::ResourceLimitExceeded => EmergencyDiagnostics::resource(span),
            ResolverError::AllocationFailed => EmergencyDiagnostics::allocation(span),
            ResolverError::CapacityExceeded => EmergencyDiagnostics::capacity(span),
            ResolverError::InternalInvariantFailure => EmergencyDiagnostics::internal(
                span,
                schema_context!("resolver-callback", "resolve-invariant")
                    .with_object_id(span.source_id().get() as u64),
            ),
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
