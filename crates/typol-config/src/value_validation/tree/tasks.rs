use crate::{
    diagnostics::{DiagnosticFactory, NAMED_ENTRY_NAME_MISSING},
    instance::{Member, NodeId},
    session::ValidationResources,
    value_validation::{TreeTarget, support::TreeSupport},
};
use opeo::{OResult, Out};
use typol_core::{
    ChargedVec, Diagnostic, DiagnosticContext, LocatedSpan, ValueKind, ValuePayloadView, ValueView,
    diagnostic_context,
};
use typol_i18n::Translator;
use typol_semantic::{ConstraintList, FieldList, TypeView, VariantView};

static VALUE_TREE_SHAPE_INVARIANT: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "value-tree-validation",
    "preconstructed-tree-shape-mismatch",
);

#[derive(Clone, Copy)]
pub(super) struct Visit<'value, 'schema> {
    pub(super) target: TreeTarget<'schema>,
    pub(super) value: ValueView<'value>,
    pub(super) member: Member<'schema>,
    pub(super) fallback_validated: bool,
}

pub(super) enum Children<'value, 'schema> {
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

pub(super) enum Task<'value, 'schema> {
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

impl<'value, 'schema> Children<'value, 'schema> {
    pub(super) fn next<'slot>(
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
                    return Err(out.fail(internal(*span, resources.translator)));
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
                        environment_field: None,
                        variant: None,
                        span: *span,
                    },
                    value: value.value(),
                    member,
                    fallback_validated: false,
                }
            }
            Self::Map {
                ty,
                value,
                next,
                span,
            } => {
                let ValueKind::Map(entries) = value.as_ref() else {
                    return Err(out.fail(internal(*span, resources.translator)));
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
                        environment_field: None,
                        variant: None,
                        span: *span,
                    },
                    value: entry.value(),
                    member,
                    fallback_validated: false,
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
                    return Err(out.fail(internal(*span, resources.translator)));
                };
                let Some(value_entry) = value_entry.as_named() else {
                    return Err(out.fail(DiagnosticFactory::internal(
                        NAMED_ENTRY_NAME_MISSING,
                        *span,
                        None,
                        None,
                        resources.translator,
                    )));
                };
                let field_name = TreeSupport::schema_call(
                    &mut resources.schema_access_errors,
                    resources.translator,
                    |schema_out| field.name(schema_out),
                    out.reborrow(),
                )?;
                let equal = TreeSupport::budget_call(
                    &mut resources.budget_errors,
                    resources.translator,
                    |budget_out| {
                        resources
                            .budget
                            .compare_text(value_entry.name(), field_name, budget_out)
                    },
                    out.reborrow(),
                )?
                .is_eq();
                if !equal {
                    return Err(out.fail(internal(*span, resources.translator)));
                }
                let value = value_entry.value();
                *next += 1;
                let expected = TreeSupport::schema_call(
                    &mut resources.schema_access_errors,
                    resources.translator,
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
                        environment_field: Some(field),
                        variant: None,
                        span: *span,
                    },
                    value,
                    member,
                    fallback_validated: false,
                }
            }
        };
        Ok(Some(visit))
    }
}

fn internal(span: Option<LocatedSpan>, translator: Translator) -> Diagnostic {
    DiagnosticFactory::internal(VALUE_TREE_SHAPE_INVARIANT, span, None, None, translator)
}
