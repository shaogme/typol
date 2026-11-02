use crate::{
    annotations::{AnnotationOwner, AnnotationTask},
    context::CompileContext,
    diagnostics::{BuildFailure, DiagnosticCollector, EmergencyDiagnostics},
    module_tree::TypeDeclarationKind,
    session::ObjectState,
    symbols::{SymbolState, SymbolTable},
    types::ResolvedTypeGraph,
};

use opeo::{OResult, Out, ResultOutExt};
use typol_core::{ChargedVec, SourceSpan};
use typol_syntax::{AnnotationList, VariantPayloadKind};

pub(super) struct MakeTasksInput<'symbols, 'source, 'diagnostics> {
    pub(super) symbols: &'symbols SymbolTable<'source>,
    pub(super) types: &'symbols ResolvedTypeGraph,
    pub(super) span: SourceSpan,
    pub(super) context: &'symbols CompileContext,
    pub(super) diagnostics: &'diagnostics mut DiagnosticCollector,
}

pub(super) struct AnnotationTaskCollector;

impl AnnotationTaskCollector {
    pub(super) fn collect<'slot, 'symbols, 'source>(
        input: MakeTasksInput<'symbols, 'source, '_>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ChargedVec<AnnotationTask<'source>>> {
        let MakeTasksInput {
            symbols,
            types,
            span,
            context,
            diagnostics,
        } = input;
        let capacity = symbols
            .types()
            .len()
            .checked_add(symbols.fields().len())
            .and_then(|value| value.checked_add(symbols.variants().len()))
            .ok_or_else(|| {
                diagnostics.push(EmergencyDiagnostics::resource(span));
                BuildFailure::empty()
            })
            .store_err_into(out.reborrow())?;
        let mut tasks = diagnostics.new_vec(context.execution(), out.reborrow())?;
        tasks.try_reserve_exact(capacity, out.reborrow())?;

        for symbol in symbols.types() {
            context.consume(1, span, diagnostics, out.reborrow())?;
            if symbol.state() != SymbolState::Valid {
                continue;
            }
            if let TypeDeclarationKind::Alias(alias) = symbol.declaration() {
                let annotations = context.ast_access(
                    out.reborrow(),
                    |ast_out| alias.annotations(ast_out),
                    BuildFailure::from,
                )?;
                if !annotations.is_empty() {
                    let state = diagnostics.require(
                        types.alias_state(symbol.id()),
                        alias.span(),
                        out.reborrow(),
                    )?;
                    let target = match (state, types.alias_target(symbol.id())) {
                        (ObjectState::Valid, Some(target)) => target,
                        (ObjectState::Invalid | ObjectState::Ambiguous, None) => continue,
                        _ => {
                            diagnostics.abort(schema_internal!(alias.span()));
                            return Err(out.fail(BuildFailure::empty()));
                        }
                    };
                    let task_span = primary_annotation_span(
                        annotations,
                        alias.span(),
                        context,
                        diagnostics,
                        out.reborrow(),
                    )?;
                    tasks.try_push(
                        AnnotationTask {
                            annotations,
                            target,
                            owner: AnnotationOwner::Alias(symbol.id()),
                            span: task_span,
                        },
                        out.reborrow(),
                    )?;
                }
            }
        }

        for field in symbols.fields() {
            context.consume(1, span, diagnostics, out.reborrow())?;
            let annotations = context.ast_access(
                out.reborrow(),
                |ast_out| field.declaration().annotations(ast_out),
                BuildFailure::from,
            )?;
            if field.state() != SymbolState::Valid || annotations.is_empty() {
                continue;
            }
            let field_span = field.declaration().span();
            let state =
                diagnostics.require(types.field_state(field.id()), field_span, out.reborrow())?;
            let target = match (state, types.field_type(field.id())) {
                (ObjectState::Valid, Some(target)) => target,
                (ObjectState::Invalid | ObjectState::Ambiguous, None) => continue,
                _ => {
                    diagnostics.abort(schema_internal!(field_span));
                    return Err(out.fail(BuildFailure::empty()));
                }
            };
            let task_span = primary_annotation_span(
                annotations,
                field_span,
                context,
                diagnostics,
                out.reborrow(),
            )?;
            tasks.try_push(
                AnnotationTask {
                    annotations,
                    target,
                    owner: AnnotationOwner::Field(field.id()),
                    span: task_span,
                },
                out.reborrow(),
            )?;
        }

        for variant in symbols.variants() {
            context.consume(1, span, diagnostics, out.reborrow())?;
            if variant.state() != SymbolState::Valid {
                continue;
            }
            let payload = context.ast_access(
                out.reborrow(),
                |ast_out| variant.declaration().payload(ast_out),
                BuildFailure::from,
            )?;
            let payload_kind = context.ast_access(
                out.reborrow(),
                |ast_out| payload.kind(ast_out),
                BuildFailure::from,
            )?;
            let VariantPayloadKind::Single(payload) = payload_kind else {
                continue;
            };
            let annotations = context.ast_access(
                out.reborrow(),
                |ast_out| payload.annotations(ast_out),
                BuildFailure::from,
            )?;
            if !annotations.is_empty() {
                let variant_span = variant.declaration().span();
                let state = diagnostics.require(
                    types.variant_payload_state(variant.id()),
                    variant_span,
                    out.reborrow(),
                )?;
                let target = match (state, types.variant_payload_type(variant.id())) {
                    (ObjectState::Valid, Some(target)) => target,
                    (ObjectState::Invalid | ObjectState::Ambiguous, None) => continue,
                    _ => {
                        diagnostics.abort(schema_internal!(variant_span));
                        return Err(out.fail(BuildFailure::empty()));
                    }
                };
                let task_span = primary_annotation_span(
                    annotations,
                    variant_span,
                    context,
                    diagnostics,
                    out.reborrow(),
                )?;
                tasks.try_push(
                    AnnotationTask {
                        annotations,
                        target,
                        owner: AnnotationOwner::VariantPayload(variant.id()),
                        span: task_span,
                    },
                    out.reborrow(),
                )?;
            }
        }
        Ok(tasks)
    }
}

fn primary_annotation_span<'slot>(
    annotations: AnnotationList<'_>,
    fallback: SourceSpan,
    context: &CompileContext,
    diagnostics: &mut DiagnosticCollector,
    mut out: Out<'slot, '_, BuildFailure>,
) -> OResult<'slot, SourceSpan> {
    match context.ast_access(
        out.reborrow(),
        |ast_out| annotations.first(ast_out),
        BuildFailure::from,
    )? {
        Some(annotation) => Ok(annotation.span()),
        None => {
            diagnostics.abort(schema_internal!(fallback));
            Err(out.fail(BuildFailure::empty()))
        }
    }
}
