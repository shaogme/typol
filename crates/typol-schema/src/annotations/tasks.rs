use crate::{
    annotations::{AnnotationOwner, AnnotationTask},
    context::CompileContext,
    diagnostics::{BuildFailure, DiagnosticCollector, EmergencyDiagnostics},
    module_tree::TypeDeclarationKind,
    session::ObjectState,
    symbols::{SymbolState, SymbolTable},
    types::{ResolvedTypeGraph, ResolvedTypeKind, TypeUseId},
};

use opeo::{OResult, Out, ResultOutExt};
use typol_core::{ChargedVec, SourceSpan};
use typol_syntax::{AnnotationList, TypeExpr, TypeExprKind, VariantPayloadKind};

pub(super) struct MakeTasksInput<'symbols, 'source, 'diagnostics> {
    pub(super) symbols: &'symbols SymbolTable<'source>,
    pub(super) types: &'symbols ResolvedTypeGraph,
    pub(super) span: SourceSpan,
    pub(super) context: &'symbols CompileContext,
    pub(super) diagnostics: &'diagnostics mut DiagnosticCollector,
}

struct TypeTaskFrame<'a> {
    expression: TypeExpr<'a>,
    target: TypeUseId,
    owner: AnnotationOwner,
}

struct TypeTaskCollection<'a, 'source> {
    expression: TypeExpr<'source>,
    target: TypeUseId,
    root_owner: AnnotationOwner,
    types: &'a ResolvedTypeGraph,
    context: &'a CompileContext,
    diagnostics: &'a mut DiagnosticCollector,
    tasks: &'a mut ChargedVec<AnnotationTask<'source>>,
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
        let mut tasks = diagnostics.new_vec(context.execution(), out.reborrow())?;
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
        tasks.try_reserve_exact(capacity, out.reborrow())?;

        for symbol in symbols.types() {
            context.consume(1, span, diagnostics, out.reborrow())?;
            if symbol.state() != SymbolState::Valid {
                continue;
            }
            let TypeDeclarationKind::Alias(alias) = symbol.declaration() else {
                continue;
            };
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
            let expression = context.ast_access(
                out.reborrow(),
                |ast_out| alias.target(ast_out),
                BuildFailure::from,
            )?;
            Self::collect_type_tree(
                TypeTaskCollection {
                    expression,
                    target,
                    root_owner: AnnotationOwner::Alias(symbol.id()),
                    types,
                    context,
                    diagnostics,
                    tasks: &mut tasks,
                },
                out.reborrow(),
            )?;
        }

        for field in symbols.fields() {
            context.consume(1, span, diagnostics, out.reborrow())?;
            if field.state() != SymbolState::Valid {
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
            let expression = context.ast_access(
                out.reborrow(),
                |ast_out| field.declaration().ty(ast_out),
                BuildFailure::from,
            )?;
            Self::collect_type_tree(
                TypeTaskCollection {
                    expression,
                    target,
                    root_owner: AnnotationOwner::Field(field.id()),
                    types,
                    context,
                    diagnostics,
                    tasks: &mut tasks,
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
            let expression = context.ast_access(
                out.reborrow(),
                |ast_out| payload.ty(ast_out),
                BuildFailure::from,
            )?;
            Self::collect_type_tree(
                TypeTaskCollection {
                    expression,
                    target,
                    root_owner: AnnotationOwner::VariantPayload(variant.id()),
                    types,
                    context,
                    diagnostics,
                    tasks: &mut tasks,
                },
                out.reborrow(),
            )?;
        }
        Ok(tasks)
    }

    fn collect_type_tree<'slot, 'source>(
        collection: TypeTaskCollection<'_, 'source>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ()> {
        let TypeTaskCollection {
            expression,
            target,
            root_owner,
            types,
            context,
            diagnostics,
            tasks,
        } = collection;
        let mut frames = diagnostics.new_vec(context.execution(), out.reborrow())?;
        frames.try_push(
            TypeTaskFrame {
                expression,
                target,
                owner: root_owner,
            },
            out.reborrow(),
        )?;
        while let Some(frame) = frames.pop() {
            context.consume(1, frame.expression.span(), diagnostics, out.reborrow())?;
            let annotations = context.ast_access(
                out.reborrow(),
                |ast_out| frame.expression.annotations(ast_out),
                BuildFailure::from,
            )?;
            if !annotations.is_empty() {
                let task_span = primary_annotation_span(
                    annotations,
                    frame.expression.span(),
                    context,
                    diagnostics,
                    out.reborrow(),
                )?;
                tasks.try_push(
                    AnnotationTask {
                        annotations,
                        target: frame.target,
                        owner: frame.owner,
                        span: task_span,
                    },
                    out.reborrow(),
                )?;
            }
            let ast_kind = context.ast_access(
                out.reborrow(),
                |ast_out| frame.expression.kind(ast_out),
                BuildFailure::from,
            )?;
            let use_kind = types.type_use(frame.target).map(|use_| use_.kind());
            let child = match (ast_kind, use_kind) {
                (TypeExprKind::Option(expression), Some(ResolvedTypeKind::Option(target)))
                | (TypeExprKind::Map { value: expression }, Some(ResolvedTypeKind::Map(target)))
                | (
                    TypeExprKind::Array {
                        element: expression,
                        ..
                    },
                    Some(ResolvedTypeKind::Array {
                        element: target, ..
                    }),
                ) => Some((expression, target)),
                (TypeExprKind::Builtin(_), Some(ResolvedTypeKind::Builtin(_)))
                | (TypeExprKind::Path(_), Some(_)) => None,
                _ => {
                    diagnostics.push(schema_internal!(frame.expression.span()));
                    return Err(out.fail(BuildFailure::empty()));
                }
            };
            if let Some((expression, target)) = child {
                frames.try_push(
                    TypeTaskFrame {
                        expression,
                        target,
                        owner: AnnotationOwner::TypeUse(target),
                    },
                    out.reborrow(),
                )?;
            }
        }
        Ok(())
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
