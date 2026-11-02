use crate::{
    context::{AssignmentContext, CompletionState, RuleContext, SemanticStatus},
    diagnostics::{DiagnosticFactory, NAMED_ENTRY_NAME_MISSING},
    instance::{LocationNode, Member, NodeId},
    session::ValidationResources,
    validation::Validator,
    value_validation::{TreeContext, TreeTarget, support::TreeSupport},
};
use core::mem::replace;
use opeo::{OResult, Out, ResultOutExt};
use typol_core::{
    ChargedVec, ConfigValue, Diagnostic, DiagnosticCode, DiagnosticContext, LocatedSpan, ValueKind,
    ValuePayloadView, ValueSource, ValueView, diagnostic_context,
};
use typol_semantic::{
    ConstraintList, FieldList, TypeKind, TypeView, VariantPayloadInfo, VariantView,
};

static VALUE_TREE_SHAPE_INVARIANT: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "value-tree-validation",
    "preconstructed-tree-shape-mismatch",
);

#[derive(Clone, Copy)]
struct Visit<'value, 'schema> {
    target: TreeTarget<'schema>,
    value: ValueView<'value>,
    member: Member<'schema>,
}

enum Children<'value, 'schema> {
    Array {
        ty: TypeView<'schema>,
        value: ValueView<'value>,
        next: usize,
        span: Option<LocatedSpan>,
    },
    Map {
        ty: TypeView<'schema>,
        value: ValueView<'value>,
        next: usize,
        span: Option<LocatedSpan>,
    },
    Fields {
        fields: FieldList<'schema>,
        value: ValueView<'value>,
        variant: Option<VariantView<'schema>>,
        next: usize,
        span: Option<LocatedSpan>,
    },
}

enum Task<'value, 'schema> {
    Enter(Visit<'value, 'schema>),
    Finish {
        visit: Visit<'value, 'schema>,
        parent: Option<NodeId>,
        errors: usize,
    },
    Children(Children<'value, 'schema>),
    Restore {
        inherited: ChargedVec<ConstraintList<'schema>>,
        depth: usize,
    },
    PopInherited,
    LeaveNesting,
}

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
            |budget_out| ChargedVec::new(Some(&context.resources.budget), budget_out),
            out.reborrow(),
        )?;
        TreeSupport::reserve_tasks(
            &mut tasks,
            &mut context.resources.budget_errors,
            out.reborrow(),
        )?;
        TreeSupport::push_task(
            &mut tasks,
            &mut context.resources.budget_errors,
            Task::Enter(Visit {
                target,
                value: value.root(),
                member: context.next_member,
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
    ) -> OResult<'slot, NodeId> {
        context.locations.begin(
            LocationNode::new(
                context.current_node,
                context.next_member,
                span,
                expected,
                context.path.len(),
            ),
            context.value_nodes,
            out.reborrow(),
        )
    }

    fn take_inherited<'schema, 'slot>(
        context: &mut TreeContext<'schema, '_, '_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ChargedVec<ConstraintList<'schema>>> {
        let empty = TreeSupport::budget_call(
            &mut context.resources.budget_errors,
            |budget_out| ChargedVec::new(Some(&context.resources.budget), budget_out),
            out.reborrow(),
        )?;
        Ok(replace(context.inherited_constraints, empty))
    }
}

impl<'value, 'schema> Children<'value, 'schema> {
    fn next<'slot>(
        &mut self,
        resources: &mut ValidationResources<'_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Option<Visit<'value, 'schema>>> {
        let visit = match self {
            Self::Array {
                ty,
                value,
                next,
                span,
            } => {
                let ValueKind::Array(values) = value.as_ref() else {
                    return Err(out.fail(internal(*span)));
                };
                let Some(value) = values.get(*next) else {
                    return Ok(None);
                };
                let member = Member::Index(*next);
                *next += 1;
                Visit {
                    target: TreeTarget {
                        expected: *ty,
                        field: None,
                        variant: None,
                        span: *span,
                    },
                    value: value.value(),
                    member,
                }
            }
            Self::Map {
                ty,
                value,
                next,
                span,
            } => {
                let ValueKind::Map(entries) = value.as_ref() else {
                    return Err(out.fail(internal(*span)));
                };
                let Some(entry) = entries.get(*next) else {
                    return Ok(None);
                };
                let member = Member::MapItem(*next);
                *next += 1;
                Visit {
                    target: TreeTarget {
                        expected: *ty,
                        field: None,
                        variant: None,
                        span: *span,
                    },
                    value: entry.value(),
                    member,
                }
            }
            Self::Fields {
                fields,
                value,
                variant,
                next,
                span,
            } => {
                let Some(field) = fields.get(*next) else {
                    return Ok(None);
                };
                let value_entry = match (*variant, value.as_ref()) {
                    (None, ValueKind::Structure(values)) => values.get(*next),
                    (
                        Some(_),
                        ValueKind::Enumeration {
                            payload: ValuePayloadView::Structure(values),
                            ..
                        },
                    ) => values.get(*next),
                    _ => None,
                };
                let Some(value_entry) = value_entry else {
                    return Err(out.fail(internal(*span)));
                };
                let Some(value_entry) = value_entry.as_named() else {
                    return Err(out.fail(DiagnosticFactory::internal(
                        NAMED_ENTRY_NAME_MISSING,
                        *span,
                        None,
                        None,
                    )));
                };
                let field_name = TreeSupport::schema_call(
                    &mut resources.schema_access_errors,
                    |schema_out| field.name(schema_out),
                    out.reborrow(),
                )?;
                let equal = TreeSupport::budget_call(
                    &mut resources.budget_errors,
                    |budget_out| {
                        resources
                            .budget
                            .compare_text(value_entry.name(), field_name, budget_out)
                    },
                    out.reborrow(),
                )?
                .is_eq();
                if !equal {
                    return Err(out.fail(internal(*span)));
                }
                let value = value_entry.value();
                *next += 1;
                let expected = TreeSupport::schema_call(
                    &mut resources.schema_access_errors,
                    |schema_out| field.ty(schema_out),
                    out.reborrow(),
                )?;
                let member = match *variant {
                    Some(variant) => Member::VariantField { variant, field },
                    None => Member::Field(field),
                };
                Visit {
                    target: TreeTarget {
                        expected,
                        field: Some(field),
                        variant: None,
                        span: *span,
                    },
                    value,
                    member,
                }
            }
        };
        Ok(Some(visit))
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
            |budget_out| context.resources.budget.consume(1, budget_out),
            out.reborrow(),
        )?;
        #[cfg(test)]
        {
            context.work_peak = context.work_peak.max(tasks.len() + 1);
        }
        match task {
            Task::Enter(mut visit) => {
                if visit.value.source() == ValueSource::SchemaDefault
                    && visit.target.span.is_none()
                    && let Some(field) = visit.target.field
                {
                    visit.target.span = Some(
                        TreeSupport::schema_call(
                            &mut context.resources.schema_access_errors,
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
                    out.reborrow(),
                )?;
                let parent = context.current_node;
                context.next_member = visit.member;
                context.current_node = Some(TreeControl::begin_location(
                    context,
                    visit.target.expected,
                    span,
                    out.reborrow(),
                )?);
                TreeSupport::push_task(
                    tasks,
                    &mut context.resources.budget_errors,
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
                if context.diagnostics.len() == errors
                    && !matches!(completion, CompletionState::Validated(_))
                {
                    #[cfg(test)]
                    if context.shape_validated {
                        context.shape_skips += 1;
                    }
                    let source = Some(visit.value.source());
                    if !context
                        .resources
                        .value_limits
                        .allows_depth(context.nesting_depth)
                    {
                        return Err(out.fail(DiagnosticFactory::make_diagnostic(
                            DiagnosticCode::RESOURCE_LIMIT_EXCEEDED,
                            "value tree nesting depth exceeded the limit",
                            visit.target.span,
                            None,
                            source,
                        )));
                    }
                    let span = if visit.value.source() == ValueSource::SchemaDefault {
                        if let Some(field) = visit.target.field {
                            Some(
                                TreeSupport::schema_call(
                                    &mut context.resources.schema_access_errors,
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
                    context
                        .locations
                        .complete(id, visit.target.span, out.reborrow())?;
                }
                context.current_node = parent;
            }
            Task::Children(mut children) => {
                if let Some(visit) = children.next(context.resources, out.reborrow())? {
                    let span = visit.target.span;
                    TreeSupport::reserve_tasks(
                        tasks,
                        &mut context.resources.budget_errors,
                        out.reborrow(),
                    )?;
                    let depth = context.path.len();
                    append_member(context, &children, visit.member, span, out.reborrow())?;
                    let inherited = TreeControl::take_inherited(context, out.reborrow())?;
                    TreeSupport::push_task(
                        tasks,
                        &mut context.resources.budget_errors,
                        Task::Children(children),
                        out.reborrow(),
                    )?;
                    TreeSupport::push_task(
                        tasks,
                        &mut context.resources.budget_errors,
                        Task::Restore { inherited, depth },
                        out.reborrow(),
                    )?;
                    TreeSupport::push_task(
                        tasks,
                        &mut context.resources.budget_errors,
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
                |budget_out| context.inherited_constraints.try_reserve(1, budget_out),
                out.reborrow(),
            )?;
            TreeSupport::budget_call(
                &mut context.resources.budget_errors,
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
                Task::LeaveNesting,
                out.reborrow(),
            )?;
            TreeSupport::push_task(
                tasks,
                &mut context.resources.budget_errors,
                Task::PopInherited,
                out.reborrow(),
            )?;
            TreeSupport::push_task(
                tasks,
                &mut context.resources.budget_errors,
                Task::Enter(Visit {
                    target: TreeTarget {
                        expected: inner,
                        field: None,
                        variant: None,
                        span,
                    },
                    value,
                    member: Member::Optional,
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
                .ok_or_else(|| internal(span))
                .store_err_into(out.reborrow())?;
            let current_node = context
                .current_node
                .ok_or_else(|| internal(span))
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
                Task::LeaveNesting,
                out.reborrow(),
            )?;
            let payload_kind = TreeSupport::schema_call(
                &mut context.resources.schema_access_errors,
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
                        |budget_out| context.path.push_payload(name, budget_out),
                        out.reborrow(),
                    )?;
                    let inherited = TreeControl::take_inherited(context, out.reborrow())?;
                    TreeSupport::push_task(
                        tasks,
                        &mut context.resources.budget_errors,
                        Task::Restore { inherited, depth },
                        out.reborrow(),
                    )?;
                    TreeSupport::push_task(
                        tasks,
                        &mut context.resources.budget_errors,
                        Task::Enter(Visit {
                            target: TreeTarget {
                                expected: ty,
                                field: None,
                                variant: Some(variant),
                                span,
                            },
                            value,
                            member: Member::Payload(variant),
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
                _ => return Err(out.fail(internal(span))),
            }
        }
        (TypeKind::Builtin(_), _) | (TypeKind::Option { .. }, ValueKind::Optional(None)) => {
            return Ok(());
        }
        _ => return Err(out.fail(internal(span))),
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
        Task::LeaveNesting,
        out.reborrow(),
    )?;
    TreeSupport::push_task(
        tasks,
        &mut context.resources.budget_errors,
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
            |budget_out| context.path.push_index(index, budget_out),
            out.reborrow(),
        )?,
        Member::MapItem(index) => {
            let Children::Map { value, .. } = children else {
                return Err(out.fail(internal(span)));
            };
            let ValueKind::Map(entries) = value.as_ref() else {
                return Err(out.fail(internal(span)));
            };
            let Some(entry) = entries.get(index) else {
                return Err(out.fail(internal(span)));
            };
            let Some(key) = entry.name() else {
                return Err(out.fail(internal(span)));
            };
            TreeSupport::budget_call(
                &mut context.resources.budget_errors,
                |budget_out| context.path.push_map_key(key, budget_out),
                out.reborrow(),
            )?
        }
        Member::Field(field) => {
            let name = TreeSupport::schema_call(
                &mut context.resources.schema_access_errors,
                |schema_out| field.name(schema_out),
                out.reborrow(),
            )?;
            TreeSupport::budget_call(
                &mut context.resources.budget_errors,
                |budget_out| context.path.push_field(name, budget_out),
                out.reborrow(),
            )?
        }
        Member::VariantField { variant, field } => {
            let variant_name = TreeSupport::schema_call(
                &mut context.resources.schema_access_errors,
                |schema_out| variant.name(schema_out),
                out.reborrow(),
            )?;
            TreeSupport::budget_call(
                &mut context.resources.budget_errors,
                |budget_out| context.path.push_payload(variant_name, budget_out),
                out.reborrow(),
            )?;
            let field_name = TreeSupport::schema_call(
                &mut context.resources.schema_access_errors,
                |schema_out| field.name(schema_out),
                out.reborrow(),
            )?;
            TreeSupport::budget_call(
                &mut context.resources.budget_errors,
                |budget_out| context.path.push_field(field_name, budget_out),
                out.reborrow(),
            )?
        }
        _ => return Err(out.fail(internal(span))),
    }
    Ok(())
}

fn internal(span: Option<LocatedSpan>) -> Diagnostic {
    DiagnosticFactory::internal(VALUE_TREE_SHAPE_INVARIANT, span, None, None)
}
