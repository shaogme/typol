use crate::{
    SchemaCompileStage,
    annotations::AnnotationCompilation,
    annotations::{AnnotationParts, AnnotationSlot, AnnotationSlotParts},
    context::CompileContext,
    defaults::DefaultCompilation,
    diagnostics::{BuildFailure, BuildResult, DiagnosticCollector},
    expressions::ExpressionCompilation,
    expressions::{ExpressionParts, ExpressionType},
    locations::SourceLocations,
    module_tree::{ModuleId, ModuleTree},
    session::SourceCatalog,
    symbols::SymbolTable,
    types::{ResolvedTypeGraph, TypeUseId},
};
use core::mem;
use opeo::{ErrSlot, OResult, Out, ResultOutExt};
use typol_core::{ChargedString, ChargedVec, ExecutionBudget, SourceSpan, SpanLocation};
use typol_semantic::{
    AssertionInfo, Builder, ConstraintId, EnvironmentRecord, FieldRecord, ProgramId, RefinementId,
    RefinementInput, SchemaDefaultValidation, StoredVariantPayload, TypeId, VariantRecord,
};
use typol_syntax::{AstAccessError, VariantPayloadKind};

pub(super) struct RecordSupport;

pub(crate) struct RecordCompilation<'a, 'src> {
    pub(crate) loaded: &'a SourceCatalog,
    pub(crate) tree: &'a ModuleTree<'src>,
    pub(crate) symbols: &'a SymbolTable<'src>,
    pub(crate) types: &'a ResolvedTypeGraph,
    pub(crate) annotations: AnnotationCompilation,
    pub(crate) expressions: ExpressionCompilation,
    pub(crate) defaults: DefaultCompilation,
    pub(crate) context: &'a CompileContext,
}

pub(crate) struct PreparedRecordCompilation {
    pub(crate) model: Builder,
    pub(crate) entry: TypeId,
    pub(crate) locations: ChargedVec<(SourceSpan, SpanLocation)>,
    pub(crate) fallback: SourceSpan,
    pub(crate) default_validation: SchemaDefaultValidation,
}

pub(super) struct Rules {
    pub(super) ids: ChargedVec<ConstraintId>,
    pub(super) secret: bool,
    pub(super) environment: Option<EnvironmentRecord>,
}

pub(super) struct CompileRulesInput<'model, 'context, 'diagnostics> {
    pub(super) slot: AnnotationSlot,
    pub(super) model: &'model mut Builder,
    pub(super) context: &'context CompileContext,
    pub(super) diagnostics: &'diagnostics mut DiagnosticCollector,
}

pub(super) struct AddAssertionsInput<'model, 'context, 'diagnostics> {
    pub(super) model: &'model mut Builder,
    pub(super) assertions: ChargedVec<AssertionInfo>,
    pub(super) context: &'context CompileContext,
    pub(super) span: SourceSpan,
    pub(super) diagnostics: &'diagnostics mut DiagnosticCollector,
}

pub(super) struct TypeIdRequest<'types, 'diagnostics, 'budget> {
    pub(super) types: &'types ResolvedTypeGraph,
    pub(super) ty: ExpressionType,
    pub(super) span: SourceSpan,
    pub(super) diagnostics: &'diagnostics mut DiagnosticCollector,
    pub(super) execution: &'budget ExecutionBudget,
}

pub(super) struct QualifiedNameInput<'tree, 'input> {
    pub(super) module_id: ModuleId,
    pub(super) name: &'input str,
    pub(super) tree: &'input ModuleTree<'tree>,
    pub(super) execution: &'input ExecutionBudget,
    pub(super) diagnostics: &'input mut DiagnosticCollector,
    pub(super) fallback: SourceSpan,
}

impl RecordSupport {
    fn ast_access<'slot, 'borrow, T>(
        context: &CompileContext,
        out: Out<'slot, 'borrow, BuildFailure>,
        operation: impl for<'call> FnOnce(Out<'call, 'call, AstAccessError>) -> OResult<'call, T>,
    ) -> OResult<'slot, T> {
        context.ast_access(out, operation, BuildFailure::from)
    }

    pub(super) fn use_rules(
        target: TypeUseId,
        types: &ResolvedTypeGraph,
        links: &[(Option<RefinementId>, bool)],
        diagnostics: &mut DiagnosticCollector,
        fallback: SourceSpan,
    ) -> BuildResult<(Option<RefinementId>, bool)> {
        let Some(type_use) = types.type_use(target) else {
            diagnostics.abort(schema_internal!(fallback));
            return Err(BuildFailure::empty());
        };
        let Some(link_id) = type_use.alias_chain() else {
            return Ok((None, false));
        };
        links.get(link_id.index()).copied().ok_or_else(|| {
            diagnostics.abort(schema_internal!(type_use.span()));
            BuildFailure::empty()
        })
    }

    pub(super) fn compile_rules<'slot>(
        input: CompileRulesInput<'_, '_, '_>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, Rules> {
        let CompileRulesInput {
            slot,
            model,
            context,
            diagnostics,
        } = input;
        let AnnotationSlotParts {
            rules,
            secret,
            environment,
        } = slot.into_parts();
        let execution = context.execution();
        let mut ids = diagnostics.new_vec(execution, out.reborrow())?;
        ids.try_reserve_exact(rules.len(), out.reborrow())?;
        for rule in rules {
            execution.consume(1, out.reborrow())?;
            let span = rule.span();
            let id = context.builder_access(out.reborrow(), span, diagnostics, |builder_out| {
                model.add_constraint(rule, builder_out)
            })?;
            ids.try_push(id, out.reborrow())?;
        }
        Ok(Rules {
            ids,
            secret,
            environment,
        })
    }

    pub(super) fn add_assertions<'slot>(
        input: AddAssertionsInput<'_, '_, '_>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ChargedVec<ProgramId>> {
        let AddAssertionsInput {
            model,
            assertions,
            context,
            span,
            diagnostics,
        } = input;
        let execution = context.execution();
        let mut ids = diagnostics.new_vec(execution, out.reborrow())?;
        ids.try_reserve_exact(assertions.len(), out.reborrow())?;
        for assertion in assertions {
            execution.consume(1, out.reborrow())?;
            let id = context.builder_access(out.reborrow(), span, diagnostics, |builder_out| {
                model.add_program(assertion, builder_out)
            })?;
            ids.try_push(id, out.reborrow())?;
        }
        Ok(ids)
    }

    pub(super) fn type_id<'slot>(
        input: TypeIdRequest<'_, '_, '_>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, TypeId> {
        let TypeIdRequest {
            types,
            ty,
            span,
            diagnostics,
            execution,
        } = input;
        execution.consume(1, out.reborrow())?;
        let id = types
            .semantic_id(ty)
            .ok_or_else(|| {
                diagnostics.push(schema_internal!(span));
                BuildFailure::empty()
            })
            .store_err_into(out.reborrow())?;
        Ok(id)
    }

    pub(super) fn copy_text<'slot>(
        text: &str,
        execution: &ExecutionBudget,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ChargedString> {
        ChargedString::copy(text, Some(execution), out.reborrow())
    }

    pub(super) fn qualified_name<'slot>(
        input: QualifiedNameInput<'_, '_>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ChargedString> {
        let QualifiedNameInput {
            module_id,
            name,
            tree,
            execution,
            diagnostics,
            fallback,
        } = input;
        let mut segments = diagnostics.new_vec(execution, out.reborrow())?;
        let mut current = Some(module_id);
        while let Some(id) = current {
            execution.consume(1, out.reborrow())?;
            let Some(module) = tree.module(id) else {
                diagnostics.abort(schema_internal!(fallback));
                return Err(out.fail(BuildFailure::empty()));
            };
            if let Some(segment) = module.name() {
                segments.try_push(segment, out.reborrow())?;
            }
            current = module.parent();
        }

        let mut qualified = ChargedString::with_capacity(0, Some(execution), out.reborrow())?;
        for segment in segments.iter().rev() {
            execution.consume(segment.len(), out.reborrow())?;
            qualified.try_push_str(segment, out.reborrow())?;
            qualified.try_push_str("::", out.reborrow())?;
        }
        execution.consume(name.len(), out.reborrow())?;
        qualified.try_push_str(name, out.reborrow())?;
        Ok(qualified)
    }

    pub(super) fn copy_slice<'slot, T: Copy>(
        items: &[T],
        diagnostics: &mut DiagnosticCollector,
        execution: &ExecutionBudget,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ChargedVec<T>> {
        let mut output = diagnostics.new_vec(execution, out.reborrow())?;
        output.extend_from_slice(items, out.reborrow())?;
        Ok(output)
    }

    pub(super) fn compile<'slot>(
        mut model: Builder,
        compilation: RecordCompilation<'_, '_>,
        diagnostics: &mut DiagnosticCollector,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, PreparedRecordCompilation> {
        let RecordCompilation {
            loaded,
            tree,
            symbols,
            types,
            annotations,
            expressions,
            defaults,
            context,
        } = compilation;
        let execution = context.execution();
        let fallback = tree.root_span();
        context
            .execution()
            .set_stage(SchemaCompileStage::Records as usize);
        let AnnotationParts {
            aliases,
            fields,
            variants,
            regexes,
        } = annotations.into_parts();
        let ExpressionParts {
            type_assertions,
            variant_assertions,
            all_programs,
        } = expressions.into_parts();

        for program in all_programs {
            execution.consume(1, out.reborrow())?;
            let span = program.span();
            context.builder_access(out.reborrow(), span, diagnostics, |builder_out| {
                model.add_program(program, builder_out)
            })?;
        }
        let mut alias_rules = diagnostics.new_vec(execution, out.reborrow())?;
        alias_rules.try_reserve_exact(aliases.len(), out.reborrow())?;
        for items in aliases {
            execution.consume(1, out.reborrow())?;
            let rules = RecordSupport::compile_rules(
                CompileRulesInput {
                    slot: items,
                    model: &mut model,
                    context,
                    diagnostics,
                },
                out.reborrow(),
            )?;
            alias_rules.try_push(rules, out.reborrow())?;
        }
        let mut links: ChargedVec<(Option<RefinementId>, bool)> =
            diagnostics.new_vec(execution, out.reborrow())?;
        links.try_reserve_exact(types.alias_links().len(), out.reborrow())?;
        for link in types.alias_links() {
            execution.consume(1, out.reborrow())?;
            let (next, inherited_secret) = match link.next() {
                Some(id) => {
                    let Some(value) = links.get(id.index()) else {
                        diagnostics.push(schema_internal!(fallback));
                        return Err(out.fail(BuildFailure::empty()));
                    };
                    *value
                }
                None => (None, false),
            };
            let Some(rules) = alias_rules.get_mut(link.symbol().index()) else {
                diagnostics.push(schema_internal!(fallback));
                return Err(out.fail(BuildFailure::empty()));
            };
            let target_type = RecordSupport::type_id(
                TypeIdRequest {
                    types,
                    ty: ExpressionType::User(link.symbol()),
                    span: fallback,
                    diagnostics,
                    execution,
                },
                out.reborrow(),
            )?;
            let current_rules = mem::replace(
                &mut rules.ids,
                diagnostics.new_vec(execution, out.reborrow())?,
            );
            let inherited_rules = diagnostics.new_vec(execution, out.reborrow())?;
            let head =
                context.builder_access(out.reborrow(), fallback, diagnostics, |builder_out| {
                    model.add_refinement(
                        RefinementInput {
                            target: target_type,
                            rules: current_rules,
                            next,
                            tail: inherited_rules,
                        },
                        builder_out,
                    )
                })?;
            links.try_push((head, inherited_secret || rules.secret), out.reborrow())?;
        }
        for (index, ty) in types.semantic_types().iter().copied().enumerate() {
            execution.consume(1, out.reborrow())?;
            let target = match ty {
                ExpressionType::Resolved(id) => Some(id),
                ExpressionType::User(id) => types.alias_target(id),
                _ => None,
            };
            if let Some(target) = target {
                let (head, secret) =
                    RecordSupport::use_rules(target, types, &links, diagnostics, fallback)
                        .store_err_into(out.reborrow())?;
                let record = context.builder_access(
                    out.reborrow(),
                    fallback,
                    diagnostics,
                    |builder_out| model.type_mut(TypeId::new(index), builder_out),
                )?;
                record.set_refinement(head);
                record.set_secret(secret);
            }
        }
        for (symbol, assertions) in symbols.types().iter().zip(type_assertions) {
            execution.consume(1, out.reborrow())?;
            let ids = RecordSupport::add_assertions(
                AddAssertionsInput {
                    model: &mut model,
                    assertions,
                    context,
                    span: fallback,
                    diagnostics,
                },
                out.reborrow(),
            )?;
            let id = RecordSupport::type_id(
                TypeIdRequest {
                    types,
                    ty: ExpressionType::User(symbol.id()),
                    span: fallback,
                    diagnostics,
                    execution,
                },
                out.reborrow(),
            )?;
            let record =
                context.builder_access(out.reborrow(), fallback, diagnostics, |builder_out| {
                    model.type_mut(id, builder_out)
                })?;
            record.set_assertions(ids);
        }
        let (mut defaults, default_validation) = defaults.into_parts();
        let Some(default_validation) = default_validation else {
            diagnostics.abort(schema_internal!(fallback));
            return Err(out.fail(BuildFailure::empty()));
        };
        if defaults.len() != symbols.fields().len()
            || fields.len() != symbols.fields().len()
            || variants.len() != symbols.variants().len()
        {
            diagnostics.push(schema_internal!(fallback));
            return Err(out.fail(BuildFailure::empty()));
        }
        for (field, items) in symbols.fields().iter().zip(fields) {
            execution.consume(1, out.reborrow())?;
            let declaration = field.declaration();
            let span = declaration.span();
            let Some(target) = types.field_type(field.id()) else {
                diagnostics.push(schema_internal!(span));
                return Err(out.fail(BuildFailure::empty()));
            };
            let (head, secret) = RecordSupport::use_rules(target, types, &links, diagnostics, span)
                .store_err_into(out.reborrow())?;
            let rules = RecordSupport::compile_rules(
                CompileRulesInput {
                    slot: items,
                    model: &mut model,
                    context,
                    diagnostics,
                },
                out.reborrow(),
            )?;
            let ty = RecordSupport::type_id(
                TypeIdRequest {
                    types,
                    ty: ExpressionType::Resolved(target),
                    span,
                    diagnostics,
                    execution,
                },
                out.reborrow(),
            )?;
            let refinement_rules = rules.ids;
            let inherited_rules = diagnostics.new_vec(execution, out.reborrow())?;
            let refinement =
                context.builder_access(out.reborrow(), span, diagnostics, |builder_out| {
                    model.add_refinement(
                        RefinementInput {
                            target: ty,
                            rules: inherited_rules,
                            next: head,
                            tail: refinement_rules,
                        },
                        builder_out,
                    )
                })?;
            let name =
                Self::ast_access(context, out.reborrow(), |ast_out| declaration.name(ast_out))?;
            let name = RecordSupport::copy_text(name, execution, out.reborrow())?;
            let mut record = FieldRecord::new(name, ty, span);
            let Some(default) = defaults.get_mut(field.id().index()) else {
                diagnostics.push(schema_internal!(span));
                return Err(out.fail(BuildFailure::empty()));
            };
            record.set_default(default.take());
            record.set_refinement(refinement);
            record.set_secret(secret || rules.secret);
            record.set_environment(rules.environment);
            let id = context.builder_access(out.reborrow(), span, diagnostics, |builder_out| {
                model.add_field(record, builder_out)
            })?;
            if id != field.id() {
                diagnostics.push(schema_internal!(span));
                return Err(out.fail(BuildFailure::empty()));
            }
        }
        for ((variant, items), assertions) in symbols
            .variants()
            .iter()
            .zip(variants)
            .zip(variant_assertions)
        {
            execution.consume(1, out.reborrow())?;
            let declaration = variant.declaration();
            let span = declaration.span();
            let target = types.variant_payload_type(variant.id());
            let payload_node = Self::ast_access(context, out.reborrow(), |ast_out| {
                declaration.payload(ast_out)
            })?;
            let payload_kind = Self::ast_access(context, out.reborrow(), |ast_out| {
                payload_node.kind(ast_out)
            })?;
            let is_structure = matches!(payload_kind, VariantPayloadKind::Structure(_));
            let payload = match payload_kind {
                VariantPayloadKind::Unit => StoredVariantPayload::Unit,
                VariantPayloadKind::Single(_) => {
                    let Some(target) = target else {
                        diagnostics.push(schema_internal!(span));
                        return Err(out.fail(BuildFailure::empty()));
                    };
                    StoredVariantPayload::Single(RecordSupport::type_id(
                        TypeIdRequest {
                            types,
                            ty: ExpressionType::Resolved(target),
                            span,
                            diagnostics,
                            execution,
                        },
                        out.reborrow(),
                    )?)
                }
                VariantPayloadKind::Structure(_) => {
                    StoredVariantPayload::Structure(RecordSupport::copy_slice(
                        variant.fields(),
                        diagnostics,
                        execution,
                        out.reborrow(),
                    )?)
                }
            };
            let (head, secret) = match target {
                Some(target) => RecordSupport::use_rules(target, types, &links, diagnostics, span)
                    .store_err_into(out.reborrow())?,
                None => (None, false),
            };
            let rules = RecordSupport::compile_rules(
                CompileRulesInput {
                    slot: items,
                    model: &mut model,
                    context,
                    diagnostics,
                },
                out.reborrow(),
            )?;
            let refinement = match target {
                Some(target) => {
                    let id = RecordSupport::type_id(
                        TypeIdRequest {
                            types,
                            ty: ExpressionType::Resolved(target),
                            span,
                            diagnostics,
                            execution,
                        },
                        out.reborrow(),
                    )?;
                    let empty_rules = diagnostics.new_vec(execution, out.reborrow())?;
                    context.builder_access(out.reborrow(), span, diagnostics, |builder_out| {
                        model.add_refinement(
                            RefinementInput {
                                target: id,
                                rules: empty_rules,
                                next: head,
                                tail: rules.ids,
                            },
                            builder_out,
                        )
                    })?
                }
                None if rules.ids.is_empty() => None,
                None => {
                    diagnostics.push(schema_internal!(span));
                    return Err(out.fail(BuildFailure::empty()));
                }
            };
            let ids = RecordSupport::add_assertions(
                AddAssertionsInput {
                    model: &mut model,
                    assertions,
                    context,
                    span,
                    diagnostics,
                },
                out.reborrow(),
            )?;
            let name =
                Self::ast_access(context, out.reborrow(), |ast_out| declaration.name(ast_out))?;
            let name = RecordSupport::copy_text(name, execution, out.reborrow())?;
            let mut record =
                context.builder_access(out.reborrow(), span, diagnostics, |builder_out| {
                    VariantRecord::new(name, payload, span, builder_out)
                })?;
            record.set_refinement(refinement);
            record.set_secret(secret || rules.secret);
            record.set_assertions(ids);
            if is_structure {
                record.set_structure_type(RecordSupport::type_id(
                    TypeIdRequest {
                        types,
                        ty: ExpressionType::VariantStructure(variant.id()),
                        span,
                        diagnostics,
                        execution,
                    },
                    out.reborrow(),
                )?);
            }
            let id = context.builder_access(out.reborrow(), span, diagnostics, |builder_out| {
                model.add_variant(record, builder_out)
            })?;
            if id != variant.id() {
                diagnostics.push(schema_internal!(span));
                return Err(out.fail(BuildFailure::empty()));
            }
        }
        model.set_regexes(regexes);
        execution.set_stage(SchemaCompileStage::Locations as usize);
        let spans =
            context.builder_access(out.reborrow(), fallback, diagnostics, |builder_out| {
                model.spans(builder_out)
            })?;
        let mut location_errors = ErrSlot::new();
        let locations = match location_errors.try_call(|location_out| {
            SourceLocations::collect_locations(spans, loaded.files(), location_out)
        }) {
            Ok(locations) => locations,
            Err(caught) => {
                diagnostics.push(SourceLocations::diagnostic(caught.take(), fallback));
                return Err(out.fail(BuildFailure::empty()));
            }
        };
        let Some(entry_type) = symbols.entry_type() else {
            diagnostics.push(schema_internal!(fallback));
            return Err(out.fail(BuildFailure::empty()));
        };
        let entry = RecordSupport::type_id(
            TypeIdRequest {
                types,
                ty: ExpressionType::User(entry_type),
                span: fallback,
                diagnostics,
                execution,
            },
            out.reborrow(),
        )?;
        Ok(PreparedRecordCompilation {
            model,
            entry,
            locations,
            fallback,
            default_validation,
        })
    }
}
