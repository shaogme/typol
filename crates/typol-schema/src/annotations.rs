mod compilation;
mod compiler;
mod tasks;
mod values;

pub(crate) use compilation::{
    AnnotationCompilation, AnnotationParts, AnnotationSlot, AnnotationSlotParts,
};
use compiler::{AnnotationRuleCompiler, CompiledAnnotation};
use tasks::{AnnotationTaskCollector, MakeTasksInput};
use typol_i18n::diagnostic_message;

use crate::{
    SchemaLimits,
    annotations::values::{AnnotationError, AnnotationValues, ConstraintValidationInput},
    context::CompileContext,
    diagnostics::{BuildFailure, DiagnosticCollector, EmergencyDiagnostics},
    module_tree::ModuleTree,
    regex::{CompiledRegexConstraints, RegexConstraintCompiler},
    symbols::{FieldSymbolId, SymbolTable, TypeSymbolId, VariantSymbolId},
    types::{ResolvedTypeGraph, TypeUseId},
};
pub(crate) use typol_semantic::{
    ConstraintKind, ConstraintLayer, ConstraintRecord as CompiledConstraint, EnvironmentRecord,
    NumberValue as TypedNumber, RegexId, StoredConstraintData as ConstraintData,
};

use opeo::{ErrSlot, OResult, Out};
use typol_core::{ChargedVec, SourceSpan};
use typol_syntax::{AnnotationKind, AnnotationList, BuiltinType};

pub(crate) struct AnnotationCompiler;

pub(crate) struct AnnotationCompileInput<'tree, 'symbol, 'types, 'context, 'diagnostics> {
    pub(crate) tree: &'tree ModuleTree<'tree>,
    pub(crate) symbols: &'symbol SymbolTable<'symbol>,
    pub(crate) types: &'types ResolvedTypeGraph,
    pub(crate) context: &'context CompileContext,
    pub(crate) diagnostics: &'diagnostics mut DiagnosticCollector,
}

#[derive(Clone, Copy)]
pub(crate) enum AnnotationOwner {
    Alias(TypeSymbolId),
    Field(FieldSymbolId),
    VariantPayload(VariantSymbolId),
}

pub(crate) struct AnnotationTask<'a> {
    pub(crate) annotations: AnnotationList<'a>,
    pub(crate) target: TypeUseId,
    pub(crate) owner: AnnotationOwner,
    pub(crate) span: SourceSpan,
}

pub(crate) struct AnnotationContext<'a, 'src> {
    pub(crate) symbols: &'a SymbolTable<'src>,
    pub(crate) types: &'a ResolvedTypeGraph,
    pub(crate) limits: &'a SchemaLimits,
    pub(crate) budget: &'a CompileContext,
}

struct AnnotationListInput<'task, 'source, 'context, 'schema, 'regex, 'diagnostics, 'errors> {
    task: &'task AnnotationTask<'source>,
    layer: ConstraintLayer,
    context: &'context AnnotationContext<'schema, 'source>,
    regex_compiler: &'regex mut RegexConstraintCompiler<'schema>,
    diagnostics: &'diagnostics mut DiagnosticCollector,
    errors: &'errors mut ErrSlot<AnnotationError>,
}

impl AnnotationCompiler {
    pub(crate) fn compile<'slot>(
        input: AnnotationCompileInput<'_, '_, '_, '_, '_>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, AnnotationCompilation> {
        let AnnotationCompileInput {
            tree,
            symbols,
            types,
            context,
            diagnostics,
        } = input;
        let mut annotation_errors = ErrSlot::new();
        let fallback_span = tree.root_span();
        let limits = context.limits();
        let mut output = AnnotationCompilation::new(
            empty_constraints(
                symbols.types().len(),
                fallback_span,
                context,
                diagnostics,
                out.reborrow(),
            )?,
            empty_constraints(
                symbols.fields().len(),
                fallback_span,
                context,
                diagnostics,
                out.reborrow(),
            )?,
            empty_constraints(
                symbols.variants().len(),
                fallback_span,
                context,
                diagnostics,
                out.reborrow(),
            )?,
            CompiledRegexConstraints::empty(),
        );
        let mut tasks = AnnotationTaskCollector::collect(
            MakeTasksInput {
                symbols,
                types,
                span: fallback_span,
                context,
                diagnostics,
            },
            out.reborrow(),
        )?;
        context.sort(
            &mut tasks,
            |left, right, mut sort_out| {
                context.execution().consume(1, sort_out.reborrow())?;
                Ok(AnnotationValues::span_key(left.span)
                    .cmp(&AnnotationValues::span_key(right.span)))
            },
            out.reborrow(),
        )?;
        let mut regex_compiler = RegexConstraintCompiler::new(context, out.reborrow())?;
        let annotation_context = AnnotationContext {
            symbols,
            types,
            limits,
            budget: context,
        };

        for task in &tasks {
            context.consume(1, fallback_span, diagnostics, out.reborrow())?;
            if diagnostics.is_stopped() {
                return Err(out.fail(BuildFailure::empty()));
            }
            let layer = match task.owner {
                AnnotationOwner::Alias(_) => ConstraintLayer::Alias,
                AnnotationOwner::Field(_) => ConstraintLayer::Field,
                AnnotationOwner::VariantPayload(_) => ConstraintLayer::VariantPayload,
            };
            let constraints = compile_annotation_list(
                AnnotationListInput {
                    task,
                    layer,
                    context: &annotation_context,
                    regex_compiler: &mut regex_compiler,
                    diagnostics,
                    errors: &mut annotation_errors,
                },
                out.reborrow(),
            )?;
            let Some(target) = task_output_mut(&mut output, task.owner, task.span) else {
                diagnostics.push(schema_internal!(task.span));
                return Err(out.fail(BuildFailure::empty()));
            };
            *target = constraints;
        }
        output.set_regexes(regex_compiler.finish());

        for task in &tasks {
            context.consume(1, fallback_span, diagnostics, out.reborrow())?;
            if diagnostics.is_stopped() {
                return Err(out.fail(BuildFailure::empty()));
            }
            AnnotationValues::validate_combined_constraints(
                ConstraintValidationInput {
                    task,
                    output: &output,
                    types,
                    context,
                    diagnostics,
                },
                out.reborrow(),
            )?;
        }

        Ok(output)
    }
}

fn compile_annotation_list<'slot>(
    input: AnnotationListInput<'_, '_, '_, '_, '_, '_, '_>,
    mut out: Out<'slot, '_, BuildFailure>,
) -> OResult<'slot, AnnotationSlot> {
    let AnnotationListInput {
        task,
        layer,
        context,
        regex_compiler,
        diagnostics,
        errors,
    } = input;
    let annotations = task.annotations;
    let span = task.span;
    let mut output = AnnotationSlot::new(context.budget.execution(), diagnostics, out.reborrow())?;
    output
        .rules_mut()
        .try_reserve_exact(annotations.len(), out.reborrow())?;
    let mut seen = diagnostics.new_vec(context.budget.execution(), out.reborrow())?;
    seen.try_reserve_exact(annotations.len(), out.reborrow())?;

    for index in 0..annotations.len() {
        let annotation = context.budget.ast_access(
            out.reborrow(),
            |ast_out| annotations.get(index, ast_out),
            BuildFailure::from,
        )?;
        let Some(annotation) = annotation else {
            diagnostics.push(schema_internal!(span));
            return Err(out.fail(BuildFailure::empty()));
        };
        let annotation_kind = context.budget.ast_access(
            out.reborrow(),
            |ast_out| annotation.kind(ast_out),
            BuildFailure::from,
        )?;
        context
            .budget
            .consume(1, annotation.span(), diagnostics, out.reborrow())?;
        context
            .budget
            .consume(seen.len(), annotation.span(), diagnostics, out.reborrow())?;
        if diagnostics.is_stopped() {
            return Err(out.fail(BuildFailure::empty()));
        }
        if let Some((_, earlier)) = seen.iter().find(|(kind, _)| *kind == annotation_kind) {
            diagnostics.push(diagnostics.make_duplicate(
                diagnostic_message!(
                    "diagnostic.schema.annotation_repeated",
                    "the same annotation cannot be repeated in one declaration"
                ),
                annotation.span(),
                *earlier,
                diagnostic_message!(
                    "diagnostic.label.annotation_first_appears_here",
                    "the duplicate annotation first appears here"
                ),
            ));
            if diagnostics.is_stopped() {
                return Err(out.fail(BuildFailure::empty()));
            }
            continue;
        }
        seen.try_push((annotation_kind, annotation.span()), out.reborrow())?;

        if annotation_kind == AnnotationKind::Regex {
            let valid_target = match AnnotationValues::effective_builtin(context.types, task.target)
            {
                Ok(target) => matches!(target, Some((BuiltinType::String, _))),
                Err(error) => {
                    report_annotation_error(error, annotation.span(), diagnostics);
                    if diagnostics.is_stopped() {
                        return Err(out.fail(BuildFailure::empty()));
                    }
                    false
                }
            };
            let previous_count = regex_compiler.constraint_count();
            regex_compiler.compile_resolved_annotation(
                annotation,
                valid_target,
                diagnostics,
                out.reborrow(),
            )?;
            if regex_compiler.constraint_count() > previous_count {
                output.rules_mut().try_push(
                    CompiledConstraint::new(
                        ConstraintKind::Regex,
                        layer,
                        annotation.span(),
                        ConstraintData::Regex {
                            regex: RegexId::new(regex_compiler.constraint_count() - 1),
                        },
                    ),
                    out.reborrow(),
                )?;
            }
            if diagnostics.is_stopped() {
                return Err(out.fail(BuildFailure::empty()));
            }
            continue;
        }

        match errors.try_call(|annotation_out| {
            AnnotationRuleCompiler::compile_one_annotation(
                annotation,
                task.target,
                layer,
                context,
                annotation_out,
            )
        }) {
            Ok(CompiledAnnotation::Constraint(data)) => {
                let Some(kind) = AnnotationRuleCompiler::constraint_kind(annotation_kind) else {
                    diagnostics.abort(schema_internal!(annotation.span()));
                    return Err(out.fail(BuildFailure::empty()));
                };
                output.rules_mut().try_push(
                    CompiledConstraint::new(kind, layer, annotation.span(), data),
                    out.reborrow(),
                )?;
            }
            Ok(CompiledAnnotation::Environment(environment)) => {
                output.set_environment(environment);
            }
            Ok(CompiledAnnotation::Secret) => output.set_secret(),
            Err(caught) => report_annotation_error(caught.take(), annotation.span(), diagnostics),
        }
        if diagnostics.is_stopped() {
            return Err(out.fail(BuildFailure::empty()));
        }
    }
    Ok(output)
}

fn report_annotation_error(
    error: AnnotationError,
    fallback: SourceSpan,
    diagnostics: &mut DiagnosticCollector,
) {
    match error {
        AnnotationError::Invalid(message) => {
            diagnostics.push(diagnostics.make_semantic(message, fallback));
        }
        AnnotationError::DuplicateBound {
            message,
            span,
            earlier_span,
        } => {
            diagnostics.push(diagnostics.make_duplicate(
                message,
                span,
                earlier_span,
                diagnostic_message!(
                    "diagnostic.label.bound_first_appears_here",
                    "the bound first appears here"
                ),
            ));
        }
        AnnotationError::Allocation => {
            diagnostics.push(EmergencyDiagnostics::allocation(fallback));
        }
        AnnotationError::Resource => {
            diagnostics.push(EmergencyDiagnostics::resource(fallback));
        }
        AnnotationError::Capacity => {
            diagnostics.push(EmergencyDiagnostics::capacity(fallback));
        }
        AnnotationError::Internal(context) => {
            diagnostics.abort(EmergencyDiagnostics::internal(fallback, context));
        }
    }
}

fn task_output_mut(
    output: &mut AnnotationCompilation,
    owner: AnnotationOwner,
    _span: SourceSpan,
) -> Option<&mut AnnotationSlot> {
    output.slot_mut(owner)
}

fn empty_constraints<'slot>(
    count: usize,
    span: SourceSpan,
    context: &CompileContext,
    diagnostics: &mut DiagnosticCollector,
    mut out: Out<'slot, '_, BuildFailure>,
) -> OResult<'slot, ChargedVec<AnnotationSlot>> {
    let mut slots = diagnostics.new_vec(context.execution(), out.reborrow())?;
    slots.try_reserve_exact(count, out.reborrow())?;
    for _ in 0..count {
        context.consume(1, span, diagnostics, out.reborrow())?;
        let slot = AnnotationSlot::new(context.execution(), diagnostics, out.reborrow())?;
        slots.try_push(slot, out.reborrow())?;
    }
    Ok(slots)
}
