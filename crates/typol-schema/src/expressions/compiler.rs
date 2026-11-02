use crate::{
    annotations::AnnotationCompilation,
    context::CompileContext,
    diagnostics::{BuildFailure, DiagnosticCollector, EmergencyDiagnostics},
    expressions::{
        CompiledAll, CompiledAssertion, ExpressionCompilation, ExpressionType,
        checker::{ExpressionChecker, ExpressionCompileInput as CheckerCompileInput},
        compile_support::{AllBindings, Context, ExpressionCompileSupport},
    },
    module_tree::{ModuleTree, TypeDeclarationKind},
    symbols::{SymbolState, SymbolTable},
    types::{ResolvedTypeGraph, ResolvedTypeKind, TypeUseId},
};

use opeo::{ErrSlot, OResult, Out};
use typol_core::SourceSpan;
use typol_i18n::diagnostic_message;
use typol_syntax::{Annotation, Assertion, AstAccessError, VariantPayloadKind};

pub(crate) struct ExpressionCompiler;

pub(crate) struct ExpressionCompileInput<'tree, 'src, 'annotations, 'diagnostics> {
    pub(crate) tree: &'tree ModuleTree<'src>,
    pub(crate) symbols: &'tree SymbolTable<'src>,
    pub(crate) types: &'tree ResolvedTypeGraph,
    pub(crate) annotations: &'annotations AnnotationCompilation,
    pub(crate) diagnostics: &'diagnostics mut DiagnosticCollector,
}

struct AllTask<'a> {
    annotation: Annotation<'a>,
    target: TypeUseId,
}

struct AssertionCompileInput<'a, 'src, 'diag> {
    assertion: Assertion<'a>,
    self_type: Option<ExpressionType>,
    root_type: Option<ExpressionType>,
    all_bindings: Option<AllBindings>,
    symbols: &'a SymbolTable<'src>,
    types: &'a ResolvedTypeGraph,
    context: &'a CompileContext,
    diagnostics: &'diag mut DiagnosticCollector,
}

fn ast_access<'slot, 'borrow, T>(
    context: &CompileContext,
    out: Out<'slot, 'borrow, BuildFailure>,
    operation: impl for<'call> FnOnce(Out<'call, 'call, AstAccessError>) -> OResult<'call, T>,
) -> OResult<'slot, T> {
    context.ast_access(out, operation, BuildFailure::from)
}

impl ExpressionCompiler {
    pub(crate) fn compile<'slot>(
        input: ExpressionCompileInput<'_, '_, '_, '_>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ExpressionCompilation> {
        let ExpressionCompileInput {
            tree,
            symbols,
            types,
            annotations,
            diagnostics,
        } = input;
        let fallback = tree.root_span();
        let context = symbols.context();
        let mut recoverable_errors = ErrSlot::new();
        let mut output = ExpressionCompilation::new(
            ExpressionCompileSupport::empty_slots(
                symbols.types().len(),
                context,
                diagnostics,
                out.reborrow(),
            )?,
            ExpressionCompileSupport::empty_slots(
                symbols.variants().len(),
                context,
                diagnostics,
                out.reborrow(),
            )?,
            diagnostics.new_vec(context.execution(), out.reborrow())?,
            diagnostics.new_vec(context.execution(), out.reborrow())?,
        );
        let root_type = symbols.entry_type().map(ExpressionType::User);

        for symbol in symbols.types() {
            context.consume(1, fallback, diagnostics, out.reborrow())?;
            if diagnostics.is_stopped() {
                return Err(out.fail(BuildFailure::empty()));
            }
            let declaration_state = diagnostics.require(
                types.declaration_state(symbol.id()),
                symbol.name_span(),
                out.reborrow(),
            )?;
            if symbol.state() != SymbolState::Valid || declaration_state != SymbolState::Valid {
                continue;
            }
            match symbol.declaration() {
                TypeDeclarationKind::Struct(declaration) => {
                    let assertions = ast_access(context, out.reborrow(), |ast_out| {
                        declaration.assertions(ast_out)
                    })?;
                    for index in 0..assertions.len() {
                        let assertion = ast_access(context, out.reborrow(), |ast_out| {
                            assertions.get(index, ast_out)
                        })?;
                        let Some(assertion) = assertion else {
                            diagnostics.push(schema_internal!(declaration.span()));
                            return Err(out.fail(BuildFailure::empty()));
                        };
                        context.consume(1, assertion.span(), diagnostics, out.reborrow())?;
                        match recoverable_errors.try_call(|assertion_out| {
                            compile_assertion(
                                AssertionCompileInput {
                                    assertion,
                                    self_type: Some(ExpressionType::User(symbol.id())),
                                    root_type,
                                    all_bindings: None,
                                    symbols,
                                    types,
                                    context,
                                    diagnostics,
                                },
                                assertion_out,
                            )
                        }) {
                            Ok(compiled) => {
                                let Some(assertions) = output.type_assertions_mut(symbol.id())
                                else {
                                    diagnostics.push(schema_internal!(assertion.span()));
                                    return Err(out.fail(BuildFailure::empty()));
                                };
                                assertions.try_reserve(1, out.reborrow())?;
                                assertions.try_push(compiled, out.reborrow())?;
                            }
                            Err(caught) if diagnostics.is_stopped() => {
                                let _ = caught.take();
                                return Err(out.fail(BuildFailure::empty()));
                            }
                            Err(caught) => {
                                diagnostics.report_failure(caught.take());
                                if diagnostics.is_stopped() {
                                    return Err(out.fail(BuildFailure::empty()));
                                }
                            }
                        }
                    }
                }
                TypeDeclarationKind::Enum(declaration) => {
                    let variants = ast_access(context, out.reborrow(), |ast_out| {
                        declaration.variants(ast_out)
                    })?;
                    let Some(variant_ids) =
                        symbols.type_symbol(symbol.id()).map(|item| item.variants())
                    else {
                        diagnostics.push(schema_internal!(declaration.span()));
                        return Err(out.fail(BuildFailure::empty()));
                    };
                    if variant_ids.len() != variants.len() {
                        diagnostics.push(schema_internal!(declaration.span()));
                        return Err(out.fail(BuildFailure::empty()));
                    }
                    for (variant_index, variant_id) in variant_ids.iter().copied().enumerate() {
                        let variant = ast_access(context, out.reborrow(), |ast_out| {
                            variants.get(variant_index, ast_out)
                        })?;
                        let Some(variant) = variant else {
                            diagnostics.push(schema_internal!(declaration.span()));
                            return Err(out.fail(BuildFailure::empty()));
                        };
                        context.consume(1, variant.span(), diagnostics, out.reborrow())?;
                        if diagnostics.is_stopped() {
                            return Err(out.fail(BuildFailure::empty()));
                        }
                        if symbols
                            .variant_symbol(variant_id)
                            .is_none_or(|entry| entry.state() != SymbolState::Valid)
                        {
                            continue;
                        }
                        let payload = ast_access(context, out.reborrow(), |ast_out| {
                            variant.payload(ast_out)
                        })?;
                        let payload_kind =
                            ast_access(context, out.reborrow(), |ast_out| payload.kind(ast_out))?;
                        let self_type = match payload_kind {
                            VariantPayloadKind::Unit => None,
                            VariantPayloadKind::Single(_) => types
                                .variant_payload_type(variant_id)
                                .map(ExpressionType::Resolved),
                            VariantPayloadKind::Structure(_) => {
                                Some(ExpressionType::VariantStructure(variant_id))
                            }
                        };
                        if matches!(payload_kind, VariantPayloadKind::Single(_))
                            && self_type.is_none()
                        {
                            continue;
                        }
                        let assertions = ast_access(context, out.reborrow(), |ast_out| {
                            variant.assertions(ast_out)
                        })?;
                        for assertion_index in 0..assertions.len() {
                            let assertion = ast_access(context, out.reborrow(), |ast_out| {
                                assertions.get(assertion_index, ast_out)
                            })?;
                            let Some(assertion) = assertion else {
                                diagnostics.push(schema_internal!(variant.span()));
                                return Err(out.fail(BuildFailure::empty()));
                            };
                            context.consume(1, assertion.span(), diagnostics, out.reborrow())?;
                            match recoverable_errors.try_call(|assertion_out| {
                                compile_assertion(
                                    AssertionCompileInput {
                                        assertion,
                                        self_type,
                                        root_type,
                                        all_bindings: None,
                                        symbols,
                                        types,
                                        context,
                                        diagnostics,
                                    },
                                    assertion_out,
                                )
                            }) {
                                Ok(compiled) => {
                                    let Some(assertions) =
                                        output.variant_assertions_mut(variant_id)
                                    else {
                                        diagnostics.push(schema_internal!(assertion.span()));
                                        return Err(out.fail(BuildFailure::empty()));
                                    };
                                    assertions.try_reserve(1, out.reborrow())?;
                                    assertions.try_push(compiled, out.reborrow())?;
                                }
                                Err(caught) if diagnostics.is_stopped() => {
                                    let _ = caught.take();
                                    return Err(out.fail(BuildFailure::empty()));
                                }
                                Err(caught) => {
                                    diagnostics.report_failure(caught.take());
                                    if diagnostics.is_stopped() {
                                        return Err(out.fail(BuildFailure::empty()));
                                    }
                                }
                            }
                        }
                    }
                }
                TypeDeclarationKind::Alias(_) => {}
            }
        }

        let mut all_tasks = diagnostics.new_vec(context.execution(), out.reborrow())?;
        let all_task_capacity = symbols
            .types()
            .len()
            .checked_add(symbols.fields().len())
            .and_then(|count| count.checked_add(symbols.variants().len()));
        let Some(all_task_capacity) = all_task_capacity else {
            diagnostics.push(EmergencyDiagnostics::resource(fallback));
            return Err(out.fail(BuildFailure::empty()));
        };
        all_tasks.try_reserve_exact(all_task_capacity, out.reborrow())?;

        for symbol in symbols.types() {
            context.consume(1, fallback, diagnostics, out.reborrow())?;
            if diagnostics.is_stopped() {
                return Err(out.fail(BuildFailure::empty()));
            }
            let declaration_state = diagnostics.require(
                types.declaration_state(symbol.id()),
                symbol.name_span(),
                out.reborrow(),
            )?;
            if symbol.state() != SymbolState::Valid || declaration_state != SymbolState::Valid {
                continue;
            }
            let TypeDeclarationKind::Alias(alias) = symbol.declaration() else {
                continue;
            };
            let Some(constraint) = ExpressionCompileSupport::all_rule(
                annotations.alias_constraints(symbol.id()),
                context,
                alias.span(),
                diagnostics,
                out.reborrow(),
            )?
            else {
                continue;
            };
            let Some(annotation) = ExpressionCompileSupport::annotation(
                ast_access(context, out.reborrow(), |ast_out| {
                    alias.annotations(ast_out)
                })?,
                constraint.span(),
                context,
                diagnostics,
                out.reborrow(),
            )?
            else {
                diagnostics.push(schema_internal!(alias.span()));
                return Err(out.fail(BuildFailure::empty()));
            };
            let Some(target) = types.alias_target(symbol.id()) else {
                continue;
            };
            all_tasks.try_push(AllTask { annotation, target }, out.reborrow())?;
        }

        for field in symbols.fields() {
            context.consume(1, fallback, diagnostics, out.reborrow())?;
            if diagnostics.is_stopped() {
                return Err(out.fail(BuildFailure::empty()));
            }
            if field.state() != SymbolState::Valid {
                continue;
            }
            let Some(constraint) = ExpressionCompileSupport::all_rule(
                annotations.field_constraints(field.id()),
                context,
                field.declaration().span(),
                diagnostics,
                out.reborrow(),
            )?
            else {
                continue;
            };
            let Some(annotation) = ExpressionCompileSupport::annotation(
                ast_access(context, out.reborrow(), |ast_out| {
                    field.declaration().annotations(ast_out)
                })?,
                constraint.span(),
                context,
                diagnostics,
                out.reborrow(),
            )?
            else {
                diagnostics.push(schema_internal!(field.declaration().span()));
                return Err(out.fail(BuildFailure::empty()));
            };
            let Some(ty) = types.field_type(field.id()) else {
                continue;
            };
            all_tasks.try_push(
                AllTask {
                    annotation,
                    target: ty,
                },
                out.reborrow(),
            )?;
        }

        for variant in symbols.variants() {
            context.consume(1, fallback, diagnostics, out.reborrow())?;
            if diagnostics.is_stopped() {
                return Err(out.fail(BuildFailure::empty()));
            }
            if variant.state() != SymbolState::Valid {
                continue;
            }
            let payload = ast_access(context, out.reborrow(), |ast_out| {
                variant.declaration().payload(ast_out)
            })?;
            let payload_kind =
                ast_access(context, out.reborrow(), |ast_out| payload.kind(ast_out))?;
            let VariantPayloadKind::Single(payload) = payload_kind else {
                continue;
            };
            let Some(constraint) = ExpressionCompileSupport::all_rule(
                annotations.variant_payload_constraints(variant.id()),
                context,
                variant.declaration().span(),
                diagnostics,
                out.reborrow(),
            )?
            else {
                continue;
            };
            let Some(annotation) = ExpressionCompileSupport::annotation(
                ast_access(context, out.reborrow(), |ast_out| {
                    payload.annotations(ast_out)
                })?,
                constraint.span(),
                context,
                diagnostics,
                out.reborrow(),
            )?
            else {
                diagnostics.push(schema_internal!(variant.declaration().span()));
                return Err(out.fail(BuildFailure::empty()));
            };
            let Some(target) = types.variant_payload_type(variant.id()) else {
                continue;
            };
            all_tasks.try_push(AllTask { annotation, target }, out.reborrow())?;
        }

        context.sort(
            &mut all_tasks,
            |left, right, mut sort_out| {
                context.execution().consume(1, sort_out.reborrow())?;
                let left = left.annotation.span();
                let right = right.annotation.span();
                Ok((left.source_id().get(), left.span().start())
                    .cmp(&(right.source_id().get(), right.span().start())))
            },
            out.reborrow(),
        )?;
        for task in all_tasks {
            context.consume(1, task.annotation.span(), diagnostics, out.reborrow())?;
            if diagnostics.is_stopped() {
                return Err(out.fail(BuildFailure::empty()));
            }
            let Some((expression, message)) =
                ExpressionCompileSupport::all_arguments(task.annotation, context, out.reborrow())?
            else {
                diagnostics.push(diagnostics.make_semantic(
                    diagnostic_message!(
                        "diagnostic.schema.annotation_all_arguments_invalid",
                        "@all requires two positional arguments: an expression and a string message"
                    ),
                    task.annotation.span(),
                ));
                record_invalid_all(&mut output, task.annotation.span(), out.reborrow())?;
                continue;
            };
            let all_bindings = match types.type_use(task.target).map(|use_| use_.kind()) {
                Some(ResolvedTypeKind::Array { element, .. }) => {
                    AllBindings::Array(ExpressionType::Resolved(element))
                }
                Some(ResolvedTypeKind::Map(value)) => {
                    AllBindings::Map(ExpressionType::Resolved(value))
                }
                _ => {
                    diagnostics.push(diagnostics.make_semantic(
                        diagnostic_message!(
                            "diagnostic.schema.annotation_all_requires_collection",
                            "@all applies only to arrays or maps"
                        ),
                        task.annotation.span(),
                    ));
                    record_invalid_all(&mut output, task.annotation.span(), out.reborrow())?;
                    continue;
                }
            };
            match recoverable_errors.try_call(|checker_out| {
                ExpressionChecker::compile_boolean(
                    CheckerCompileInput {
                        expression,
                        context: Context::new(None, None, Some(all_bindings)),
                        symbols,
                        types,
                        diagnostics,
                    },
                    checker_out,
                )
            }) {
                Ok(compiled) => {
                    let message = ExpressionCompileSupport::copy_text(
                        ast_access(context, out.reborrow(), |ast_out| message.value(ast_out))?,
                        context,
                        out.reborrow(),
                    )?;
                    output
                        .all_constraints_mut()
                        .try_reserve(1, out.reborrow())?;
                    output.all_constraints_mut().try_push(
                        CompiledAll::new(compiled, message, task.annotation.span()),
                        out.reborrow(),
                    )?;
                }
                Err(caught) if diagnostics.is_stopped() => {
                    let _ = caught.take();
                    return Err(out.fail(BuildFailure::empty()));
                }
                Err(caught) => {
                    diagnostics.report_failure(caught.take());
                    if diagnostics.is_stopped() {
                        return Err(out.fail(BuildFailure::empty()));
                    }
                    record_invalid_all(&mut output, task.annotation.span(), out.reborrow())?;
                }
            }
        }

        Ok(output)
    }
}

fn record_invalid_all<'slot>(
    output: &mut ExpressionCompilation,
    span: SourceSpan,
    mut out: Out<'slot, '_, BuildFailure>,
) -> OResult<'slot, ()> {
    output
        .invalid_all_constraints_mut()
        .try_reserve(1, out.reborrow())?;
    output
        .invalid_all_constraints_mut()
        .try_push(span, out.reborrow())?;
    Ok(())
}

fn compile_assertion<'slot>(
    input: AssertionCompileInput<'_, '_, '_>,
    mut out: Out<'slot, '_, BuildFailure>,
) -> OResult<'slot, CompiledAssertion> {
    let AssertionCompileInput {
        assertion,
        self_type,
        root_type,
        all_bindings,
        symbols,
        types,
        context,
        diagnostics,
    } = input;
    let expression = ExpressionChecker::compile_boolean(
        CheckerCompileInput {
            expression: ast_access(context, out.reborrow(), |ast_out| {
                assertion.expression(ast_out)
            })?,
            context: Context::new(self_type, root_type, all_bindings),
            symbols,
            types,
            diagnostics,
        },
        out.reborrow(),
    )?;
    let message_literal = ast_access(context, out.reborrow(), |ast_out| {
        assertion.message(ast_out)
    })?;
    let message = ExpressionCompileSupport::copy_text(
        ast_access(context, out.reborrow(), |ast_out| {
            message_literal.value(ast_out)
        })?,
        symbols.context(),
        out.reborrow(),
    )?;
    Ok(CompiledAssertion::new(
        expression,
        message,
        assertion.span(),
    ))
}
