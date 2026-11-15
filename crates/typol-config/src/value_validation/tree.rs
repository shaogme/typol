mod environment;
mod tasks;

use self::{
    environment::{EnvironmentReferenceValidator, EnvironmentValidation},
    tasks::{Children, Task, Visit},
};
use crate::{
    context::{AssignmentContext, CompletionState, RuleContext, SemanticStatus},
    diagnostics::{DetailedDiagnosticRequest, DiagnosticFactory},
    instance::{LocationNode, Member, NodeId},
    validation::Validator,
    value_validation::{TreeContext, TreeTarget, support::TreeSupport},
};
use core::mem::replace;
use opeo::{OResult, Out, ResultOutExt};
use typol_core::{
    ChargedVec, ConfigValue, Diagnostic, DiagnosticCode, DiagnosticContext, LocatedSpan, ValueKind,
    ValuePayloadView, ValueSource, diagnostic_context,
};
use typol_i18n::{Translator, t};
use typol_semantic::{ConstraintList, TypeKind, TypeView, VariantPayloadInfo};

static VALUE_TREE_SHAPE_INVARIANT: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "value-tree-validation",
    "preconstructed-tree-shape-mismatch",
);

pub(super) struct TreeTraversal;

impl TreeTraversal {
    pub(super) fn subtree<'schema, 'slot>(
        context: &mut TreeContext<'schema, '_, '_>,
        target: TreeTarget<'schema>,
        value: &ConfigValue,
        completion: CompletionState,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, SemanticStatus> {
        let parent = context.current_node;
        let path_depth = context.path.len();
        let nesting = context.nesting_depth;
        let inherited_depth = context.inherited_constraints.len();
        let mut tasks = TreeSupport::budget_call(
            &mut context.resources.budget_errors,
            context.resources.translator,
            |budget_out| ChargedVec::new(Some(&context.resources.budget), budget_out),
            out.reborrow(),
        )?;
        TreeSupport::reserve_tasks(
            &mut tasks,
            &mut context.resources.budget_errors,
            context.resources.translator,
            out.reborrow(),
        )?;
        TreeSupport::push_task(
            &mut tasks,
            &mut context.resources.budget_errors,
            context.resources.translator,
            Task::Enter(Visit {
                target,
                value: value.root(),
                member: context.next_member,
                fallback_validated: false,
                schema_default_parent: false,
            }),
            out.reborrow(),
        )?;
        let mut reported = false;
        let result = run(
            context,
            &mut tasks,
            completion,
            &mut reported,
            out.reborrow(),
        );
        while let Some(task) = tasks.pop() {
            match task {
                Task::Restore { inherited, .. } => *context.inherited_constraints = inherited,
                Task::PopInherited => {
                    context.inherited_constraints.pop();
                }
                _ => {}
            }
        }
        context.inherited_constraints.truncate(inherited_depth);
        context.path.truncate(path_depth);
        context.nesting_depth = nesting;
        context.current_node = parent;
        result?;
        Ok(if context.diagnostics.stopped() {
            SemanticStatus::Stopped
        } else if reported {
            SemanticStatus::Reported
        } else {
            SemanticStatus::Success
        })
    }
}

struct TreeControl;

impl TreeControl {
    fn begin_location<'schema, 'slot>(
        context: &mut TreeContext<'schema, '_, '_>,
        expected: TypeView<'schema>,
        span: Option<LocatedSpan>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, (NodeId, Option<LocatedSpan>)> {
        let existing = match context.current_node {
            Some(parent) => {
                context
                    .locations
                    .existing_child(parent, context.next_member, out.reborrow())?
            }
            None if matches!(context.next_member, Member::Root) => context.locations.root_node(),
            None => None,
        };
        if let Some(id) = existing {
            return Ok((id, context.locations.get(id).and_then(|node| node.span())));
        }
        let id = context.locations.begin(
            LocationNode::new(
                context.current_node,
                context.next_member,
                span,
                expected,
                context.path.len(),
            ),
            context.value_nodes,
            out.reborrow(),
        )?;
        Ok((id, None))
    }

    fn take_inherited<'schema, 'slot>(
        context: &mut TreeContext<'schema, '_, '_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ChargedVec<ConstraintList<'schema>>> {
        let empty = TreeSupport::budget_call(
            &mut context.resources.budget_errors,
            context.resources.translator,
            |budget_out| ChargedVec::new(Some(&context.resources.budget), budget_out),
            out.reborrow(),
        )?;
        Ok(replace(context.inherited_constraints, empty))
    }
}

fn run<'schema, 'slot>(
    context: &mut TreeContext<'schema, '_, '_>,
    tasks: &mut ChargedVec<Task<'_, 'schema>>,
    completion: CompletionState,
    reported: &mut bool,
    mut out: Out<'slot, '_, Diagnostic>,
) -> OResult<'slot, ()> {
    while let Some(task) = tasks.pop() {
        TreeSupport::budget_call(
            &mut context.resources.budget_errors,
            context.resources.translator,
            |budget_out| context.resources.budget.consume(1, budget_out),
            out.reborrow(),
        )?;
        #[cfg(test)]
        {
            context.work_peak = context.work_peak.max(tasks.len() + 1);
        }
        match task {
            Task::Enter(mut visit) => {
                match EnvironmentReferenceValidator::validate(
                    context,
                    visit.target,
                    visit.value,
                    visit.fallback_validated,
                    out.reborrow(),
                )? {
                    EnvironmentValidation::Valid => {}
                    EnvironmentValidation::Reported => *reported = true,
                    EnvironmentValidation::Stopped => return Ok(()),
                }
                if visit.value.source() == ValueSource::SchemaDefault
                    && !visit.schema_default_parent
                    && let Some(field) = visit.target.field
                {
                    visit.target.span = Some(
                        TreeSupport::schema_call(
                            &mut context.resources.schema_access_errors,
                            context.resources.translator,
                            |schema_out| field.span(schema_out),
                            out.reborrow(),
                        )?
                        .into(),
                    );
                }
                let span = visit.target.span;
                TreeSupport::reserve_tasks(
                    tasks,
                    &mut context.resources.budget_errors,
                    context.resources.translator,
                    out.reborrow(),
                )?;
                let parent = context.current_node;
                context.next_member = visit.member;
                let (node, existing_span) = TreeControl::begin_location(
                    context,
                    visit.target.expected,
                    span,
                    out.reborrow(),
                )?;
                if let Some(existing_span) = existing_span {
                    visit.target.span = Some(existing_span);
                }
                context.current_node = Some(node);
                TreeSupport::push_task(
                    tasks,
                    &mut context.resources.budget_errors,
                    context.resources.translator,
                    Task::Finish {
                        visit,
                        parent,
                        errors: context.diagnostics.len(),
                    },
                    out.reborrow(),
                )?;
                schedule_children(context, tasks, &visit, out.reborrow())?;
            }
            Task::Finish {
                visit,
                parent,
                errors,
            } => {
                let already_validated = context
                    .current_node
                    .and_then(|id| context.locations.get(id))
                    .is_some_and(|node| node.validated());
                if context.diagnostics.len() == errors
                    && !matches!(completion, CompletionState::Validated(_))
                    && !already_validated
                {
                    #[cfg(test)]
                    if context.shape_validated && !matches!(visit.member, Member::Root) {
                        context.shape_skips += 1;
                    }
                    let source = Some(visit.value.source());
                    if !context
                        .resources
                        .value_limits
                        .allows_depth(context.nesting_depth)
                    {
                        return Err(out.fail(DiagnosticFactory::make_diagnostic(DetailedDiagnosticRequest {
                            code: DiagnosticCode::CONFIG_VALUE_DEPTH_LIMIT_EXCEEDED,
                            message: t! { name: "diagnostic.config.value_tree_nesting_limit", locale: context.resources.translator }.into(),
                            primary: visit.target.span,
                            path: None,
                            value_source: source,
                            context: None,
                            translator: context.resources.translator,
                        })));
                    }
                    let span = if visit.value.source() == ValueSource::SchemaDefault {
                        if let Some(field) = visit.target.field {
                            Some(
                                TreeSupport::schema_call(
                                    &mut context.resources.schema_access_errors,
                                    context.resources.translator,
                                    |schema_out| field.span(schema_out),
                                    out.reborrow(),
                                )?
                                .into(),
                            )
                            .or(visit.target.span)
                        } else {
                            visit.target.span
                        }
                    } else {
                        visit.target.span
                    };
                    let result = context.diagnostic_errors.try_call(|diagnostic_out| {
                        Validator::completed(
                            AssignmentContext {
                                translator: context.resources.translator,
                                schema: context.schema,
                                node: context.current_node,
                                locations: context.locations,
                                inherited_constraints: context.inherited_constraints,
                                expected: visit.target.expected,
                                field: visit.target.field,
                                variant: visit.target.variant,
                                span,
                                path: context.path,
                                source,
                                shape_validated: context.shape_validated,
                            },
                            visit.value,
                            context.resources,
                            context.diagnostics,
                            diagnostic_out,
                        )
                    });
                    match result {
                        Err(caught) => {
                            if context.diagnostics.push(caught.take()) {
                                *reported = true;
                            } else {
                                return Ok(());
                            }
                        }
                        Ok(SemanticStatus::Reported) => *reported = true,
                        Ok(SemanticStatus::Stopped) => return Ok(()),
                        Ok(SemanticStatus::Success) => {}
                    }
                }
                if let Some(id) = context.current_node {
                    let completed_span = context
                        .locations
                        .get(id)
                        .and_then(|node| node.span())
                        .or(visit.target.span);
                    context
                        .locations
                        .complete(id, completed_span, out.reborrow())?;
                }
                context.current_node = parent;
            }
            Task::Children(mut children) => {
                if let Some(visit) = children.next(context.resources, out.reborrow())? {
                    let span = visit.target.span;
                    TreeSupport::reserve_tasks(
                        tasks,
                        &mut context.resources.budget_errors,
                        context.resources.translator,
                        out.reborrow(),
                    )?;
                    let depth = context.path.len();
                    append_member(context, &children, visit.member, span, out.reborrow())?;
                    let inherited = TreeControl::take_inherited(context, out.reborrow())?;
                    TreeSupport::push_task(
                        tasks,
                        &mut context.resources.budget_errors,
                        context.resources.translator,
                        Task::Children(children),
                        out.reborrow(),
                    )?;
                    TreeSupport::push_task(
                        tasks,
                        &mut context.resources.budget_errors,
                        context.resources.translator,
                        Task::Restore { inherited, depth },
                        out.reborrow(),
                    )?;
                    TreeSupport::push_task(
                        tasks,
                        &mut context.resources.budget_errors,
                        context.resources.translator,
                        Task::Enter(visit),
                        out.reborrow(),
                    )?;
                }
            }
            Task::Restore { inherited, depth } => {
                *context.inherited_constraints = inherited;
                context.path.truncate(depth);
            }
            Task::PopInherited => {
                context.inherited_constraints.pop();
            }
            Task::LeaveNesting => TreeSupport::leave_nesting(context),
        }
    }
    Ok(())
}

fn schedule_children<'value, 'schema, 'slot>(
    context: &mut TreeContext<'schema, '_, '_>,
    tasks: &mut ChargedVec<Task<'value, 'schema>>,
    visit: &Visit<'value, 'schema>,
    mut out: Out<'slot, '_, Diagnostic>,
) -> OResult<'slot, ()> {
    let target = visit.target;
    let span = target.span;
    let kind = TreeSupport::schema_call(
        &mut context.resources.schema_access_errors,
        context.resources.translator,
        |schema_out| target.expected.kind(schema_out),
        out.reborrow(),
    )?;
    let children = match (kind, visit.value.as_ref()) {
        (TypeKind::Option { inner }, ValueKind::Optional(Some(value))) => {
            TreeSupport::enter_nesting(
                context,
                span,
                TreeSupport::source(context, value),
                out.reborrow(),
            )?;
            let constraints = TreeSupport::schema_call(
                &mut context.resources.schema_access_errors,
                context.resources.translator,
                |schema_out| {
                    RuleContext::constraints(
                        target.expected,
                        target.field,
                        target.variant,
                        schema_out,
                    )
                },
                out.reborrow(),
            )?;
            TreeSupport::budget_call(
                &mut context.resources.budget_errors,
                context.resources.translator,
                |budget_out| context.inherited_constraints.try_reserve(1, budget_out),
                out.reborrow(),
            )?;
            TreeSupport::budget_call(
                &mut context.resources.budget_errors,
                context.resources.translator,
                |budget_out| {
                    context
                        .inherited_constraints
                        .try_push(constraints, budget_out)
                },
                out.reborrow(),
            )?;
            TreeSupport::push_task(
                tasks,
                &mut context.resources.budget_errors,
                context.resources.translator,
                Task::LeaveNesting,
                out.reborrow(),
            )?;
            TreeSupport::push_task(
                tasks,
                &mut context.resources.budget_errors,
                context.resources.translator,
                Task::PopInherited,
                out.reborrow(),
            )?;
            TreeSupport::push_task(
                tasks,
                &mut context.resources.budget_errors,
                context.resources.translator,
                Task::Enter(Visit {
                    target: TreeTarget {
                        expected: inner,
                        field: None,
                        environment_field: target.environment_field,
                        variant: None,
                        span,
                    },
                    value,
                    member: Member::Optional,
                    fallback_validated: visit.fallback_validated
                        || visit.value.shares_environment_reference(value),
                    schema_default_parent: visit.value.source() == ValueSource::SchemaDefault,
                }),
                out.reborrow(),
            )?;
            return Ok(());
        }
        (TypeKind::Array { element, .. }, ValueKind::Array(_)) => Children::Array {
            ty: element,
            value: visit.value,
            next: 0,
            span,
        },
        (TypeKind::Map { value: ty }, ValueKind::Map(_)) => Children::Map {
            ty,
            value: visit.value,
            next: 0,
            span,
        },
        (TypeKind::Struct { fields }, ValueKind::Structure(_)) => Children::Fields {
            fields,
            value: visit.value,
            variant: None,
            next: 0,
            span,
        },
        (
            TypeKind::Enum { .. },
            ValueKind::Enumeration {
                variant: name,
                payload,
            },
        ) => {
            let variant = TreeSupport::budget_call(
                &mut context.resources.budget_errors,
                context.resources.translator,
                |budget_out| {
                    target.expected.variant_named_budgeted(
                        name,
                        &context.resources.budget,
                        budget_out,
                    )
                },
                out.reborrow(),
            )?;
            let variant = variant
                .ok_or_else(|| internal(span, context.resources.translator))
                .store_err_into(out.reborrow())?;
            let current_node = context
                .current_node
                .ok_or_else(|| internal(span, context.resources.translator))
                .store_err_into(out.reborrow())?;
            context
                .locations
                .set_variant(current_node, variant, span, out.reborrow())?;
            TreeSupport::enter_nesting(
                context,
                span,
                TreeSupport::source(context, visit.value),
                out.reborrow(),
            )?;
            TreeSupport::push_task(
                tasks,
                &mut context.resources.budget_errors,
                context.resources.translator,
                Task::LeaveNesting,
                out.reborrow(),
            )?;
            let payload_kind = TreeSupport::schema_call(
                &mut context.resources.schema_access_errors,
                context.resources.translator,
                |schema_out| variant.payload(schema_out),
                out.reborrow(),
            )?;
            match (payload_kind, payload) {
                (VariantPayloadInfo::Unit, ValuePayloadView::Unit) => {
                    return Ok(());
                }
                (VariantPayloadInfo::Single { ty }, ValuePayloadView::Single(value)) => {
                    let depth = context.path.len();
                    TreeSupport::budget_call(
                        &mut context.resources.budget_errors,
                        context.resources.translator,
                        |budget_out| context.path.push_payload(name, budget_out),
                        out.reborrow(),
                    )?;
                    let inherited = TreeControl::take_inherited(context, out.reborrow())?;
                    TreeSupport::push_task(
                        tasks,
                        &mut context.resources.budget_errors,
                        context.resources.translator,
                        Task::Restore { inherited, depth },
                        out.reborrow(),
                    )?;
                    TreeSupport::push_task(
                        tasks,
                        &mut context.resources.budget_errors,
                        context.resources.translator,
                        Task::Enter(Visit {
                            target: TreeTarget {
                                expected: ty,
                                field: None,
                                environment_field: None,
                                variant: Some(variant),
                                span,
                            },
                            value,
                            member: Member::Payload(variant),
                            fallback_validated: false,
                            schema_default_parent: visit.value.source()
                                == ValueSource::SchemaDefault,
                        }),
                        out.reborrow(),
                    )?;
                    return Ok(());
                }
                (VariantPayloadInfo::Structure { fields }, ValuePayloadView::Structure(_)) => {
                    TreeSupport::check_depth(
                        context,
                        context.nesting_depth,
                        span,
                        TreeSupport::source(context, visit.value),
                        out.reborrow(),
                    )?;
                    Children::Fields {
                        fields,
                        value: visit.value,
                        variant: Some(variant),
                        next: 0,
                        span,
                    }
                }
                _ => return Err(out.fail(internal(span, context.resources.translator))),
            }
        }
        (TypeKind::Builtin(_), _) | (TypeKind::Option { .. }, ValueKind::Optional(None)) => {
            return Ok(());
        }
        _ => return Err(out.fail(internal(span, context.resources.translator))),
    };
    TreeSupport::enter_nesting(
        context,
        span,
        TreeSupport::source(context, visit.value),
        out.reborrow(),
    )?;
    TreeSupport::push_task(
        tasks,
        &mut context.resources.budget_errors,
        context.resources.translator,
        Task::LeaveNesting,
        out.reborrow(),
    )?;
    TreeSupport::push_task(
        tasks,
        &mut context.resources.budget_errors,
        context.resources.translator,
        Task::Children(children),
        out.reborrow(),
    )?;
    Ok(())
}

fn append_member<'slot>(
    context: &mut TreeContext<'_, '_, '_>,
    children: &Children<'_, '_>,
    member: Member<'_>,
    span: Option<LocatedSpan>,
    mut out: Out<'slot, '_, Diagnostic>,
) -> OResult<'slot, ()> {
    match member {
        Member::Index(index) => TreeSupport::budget_call(
            &mut context.resources.budget_errors,
            context.resources.translator,
            |budget_out| context.path.push_index(index, budget_out),
            out.reborrow(),
        )?,
        Member::MapItem(index) => {
            let Children::Map { value, .. } = children else {
                return Err(out.fail(internal(span, context.resources.translator)));
            };
            let ValueKind::Map(entries) = value.as_ref() else {
                return Err(out.fail(internal(span, context.resources.translator)));
            };
            let Some(entry) = entries.get(index) else {
                return Err(out.fail(internal(span, context.resources.translator)));
            };
            let Some(key) = entry.name() else {
                return Err(out.fail(internal(span, context.resources.translator)));
            };
            TreeSupport::budget_call(
                &mut context.resources.budget_errors,
                context.resources.translator,
                |budget_out| context.path.push_map_key(key, budget_out),
                out.reborrow(),
            )?
        }
        Member::Field(field) => {
            let name = TreeSupport::schema_call(
                &mut context.resources.schema_access_errors,
                context.resources.translator,
                |schema_out| field.name(schema_out),
                out.reborrow(),
            )?;
            TreeSupport::budget_call(
                &mut context.resources.budget_errors,
                context.resources.translator,
                |budget_out| context.path.push_field(name, budget_out),
                out.reborrow(),
            )?
        }
        Member::VariantField { variant, field } => {
            let variant_name = TreeSupport::schema_call(
                &mut context.resources.schema_access_errors,
                context.resources.translator,
                |schema_out| variant.name(schema_out),
                out.reborrow(),
            )?;
            TreeSupport::budget_call(
                &mut context.resources.budget_errors,
                context.resources.translator,
                |budget_out| context.path.push_payload(variant_name, budget_out),
                out.reborrow(),
            )?;
            let field_name = TreeSupport::schema_call(
                &mut context.resources.schema_access_errors,
                context.resources.translator,
                |schema_out| field.name(schema_out),
                out.reborrow(),
            )?;
            TreeSupport::budget_call(
                &mut context.resources.budget_errors,
                context.resources.translator,
                |budget_out| context.path.push_field(field_name, budget_out),
                out.reborrow(),
            )?
        }
        _ => return Err(out.fail(internal(span, context.resources.translator))),
    }
    Ok(())
}

fn internal(span: Option<LocatedSpan>, translator: Translator) -> Diagnostic {
    DiagnosticFactory::internal(VALUE_TREE_SHAPE_INVARIANT, span, None, None, translator)
}
