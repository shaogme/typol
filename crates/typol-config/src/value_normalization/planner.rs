use crate::{
    diagnostics::{BudgetDiagnostic, DiagnosticFactory, NAMED_ENTRY_NAME_MISSING},
    secret::SecretPolicy,
    validation::Validator,
    value_normalization::model::{
        ChildRole, FieldsRequest, Input, NORMALIZE_FIELD_SLOT, NORMALIZE_MAP_CHILD_ROLE,
        NormalizeErrors, NormalizeOps, NormalizeState, Plan, PlanChild, PlanKind,
    },
    value_normalization::{MissingFieldDecision, MissingFieldValue},
};
use opeo::{OResult, Out, ResultOutExt};
use typol_core::{
    BudgetError, ChargedVec, Diagnostic, DiagnosticCode, DiagnosticMessage, InstancePath,
    ValueKind, ValuePayloadView, ValueSource, ValueView,
};
use typol_i18n::diagnostic_message;
use typol_semantic::{TypeKind, VariantPayloadInfo};

pub(super) struct NormalizePlanner;

impl NormalizePlanner {
    pub(super) fn append_role<'slot>(
        role: ChildRole<'_>,
        path: &mut InstancePath,
        errors: &mut NormalizeErrors,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ()> {
        match role {
            ChildRole::Field(field) => {
                let name = NormalizeOps::schema_call(
                    &mut errors.schema,
                    |schema_out| field.name(schema_out),
                    out.reborrow(),
                )?;
                NormalizeOps::budget_call(
                    &mut errors.budget,
                    |budget_out| path.push_field(name, budget_out),
                    out.reborrow(),
                )?
            }
            ChildRole::VariantField { variant, field } => {
                NormalizeOps::budget_call(
                    &mut errors.budget,
                    |budget_out| path.push_payload(variant, budget_out),
                    out.reborrow(),
                )?;
                let name = NormalizeOps::schema_call(
                    &mut errors.schema,
                    |schema_out| field.name(schema_out),
                    out.reborrow(),
                )?;
                NormalizeOps::budget_call(
                    &mut errors.budget,
                    |budget_out| path.push_field(name, budget_out),
                    out.reborrow(),
                )?
            }
            ChildRole::ArrayItem(index) => NormalizeOps::budget_call(
                &mut errors.budget,
                |budget_out| path.push_index(index, budget_out),
                out.reborrow(),
            )?,
            ChildRole::MapItem(key) => NormalizeOps::budget_call(
                &mut errors.budget,
                |budget_out| path.push_map_key(key, budget_out),
                out.reborrow(),
            )?,
            ChildRole::OptionalContent => return Ok(()),
            ChildRole::EnumPayload(name) => NormalizeOps::budget_call(
                &mut errors.budget,
                |budget_out| path.push_payload(name, budget_out),
                out.reborrow(),
            )?,
        }
        Ok(())
    }

    pub(super) fn node<'slot>(
        state: &mut NormalizeState<'_, '_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ()> {
        *state.nodes = state
            .nodes
            .checked_add(1)
            .filter(|nodes| *nodes <= state.limits.max_value_nodes())
            .ok_or_else(|| BudgetDiagnostic::from_error(BudgetError::Resource))
            .store_err_into(out.reborrow())?;
        Ok(())
    }

    fn error<'slot>(
        state: &mut NormalizeState<'_, '_>,
        input: Input<'_>,
        path: &InstancePath,
        message: DiagnosticMessage<'static>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Diagnostic> {
        let copied_path = NormalizeOps::budget_call(
            &mut state.errors.budget,
            |budget_out| path.copy(budget_out),
            out.reborrow(),
        )?;
        let mut error = DiagnosticFactory::make_diagnostic(
            DiagnosticCode::CONFIG_VALUE_ERROR,
            if input.hidden {
                diagnostic_message!("diagnostic.validation.value_hidden", "value is hidden")
            } else {
                message
            },
            state.request.primary,
            Some(copied_path),
            input.value.map(|_| {
                if input.schema_default {
                    ValueSource::SchemaDefault
                } else {
                    ValueSource::RustValue
                }
            }),
        );
        let span = match input.field {
            Some(field) => NormalizeOps::schema_call(
                &mut state.errors.schema,
                |schema_out| field.span(schema_out),
                out.reborrow(),
            )?,
            None => NormalizeOps::schema_call(
                &mut state.errors.schema,
                |schema_out| input.ty.span(schema_out),
                out.reborrow(),
            )?,
        };
        DiagnosticFactory::add_label_message(
            &mut error,
            span,
            diagnostic_message!(
                "diagnostic.label.type_or_field_defined_here",
                "type or field defined here"
            ),
            out.reborrow(),
        )?;
        Ok(error)
    }

    pub(super) fn plan<'a, 'slot>(
        state: &mut NormalizeState<'a, '_>,
        input: Input<'a>,
        path: &InstancePath,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Plan<'a>> {
        let mut children = NormalizeOps::budget_call(
            &mut state.errors.budget,
            |budget_out| ChargedVec::new(Some(state.budget), budget_out),
            out.reborrow(),
        )?;
        let Some(value) = input.value else {
            let kind = NormalizeOps::schema_call(
                &mut state.errors.schema,
                |schema_out| input.ty.kind(schema_out),
                out.reborrow(),
            )?;
            if matches!(kind, TypeKind::Option { .. }) {
                return Ok(Plan {
                    input,
                    kind: PlanKind::MissingOptional,
                    children,
                });
            }
            let diagnostic = Self::error(
                state,
                input,
                path,
                diagnostic_message!(
                    "diagnostic.config.required_field_missing",
                    "required field is missing"
                ),
                out.reborrow(),
            )?;
            return Err(out.fail(diagnostic));
        };
        let shape_valid = NormalizeOps::schema_call(
            &mut state.errors.schema,
            |schema_out| Validator::shape(input.ty, value, schema_out),
            out.reborrow(),
        )?;
        let kind = NormalizeOps::schema_call(
            &mut state.errors.schema,
            |schema_out| input.ty.kind(schema_out),
            out.reborrow(),
        )?;
        let kind = match (kind, value.as_ref()) {
            (TypeKind::Struct { fields }, ValueKind::Structure(values)) => {
                Self::fields(
                    state,
                    FieldsRequest {
                        input,
                        fields,
                        values,
                        output: &mut children,
                        path,
                    },
                    out.reborrow(),
                )?;
                PlanKind::Structure
            }
            (
                TypeKind::Enum { .. },
                ValueKind::Enumeration {
                    variant: name,
                    payload,
                },
            ) => {
                let Some(variant) = NormalizeOps::budget_call(
                    &mut state.errors.budget,
                    |budget_out| {
                        input
                            .ty
                            .variant_named_budgeted(name, state.budget, budget_out)
                    },
                    out.reborrow(),
                )?
                else {
                    let diagnostic = Self::error(
                        state,
                        input,
                        path,
                        diagnostic_message!(
                            "diagnostic.config.enum_variant_undeclared",
                            "enum variant is undeclared"
                        ),
                        out.reborrow(),
                    )?;
                    return Err(out.fail(diagnostic));
                };
                let variant_hidden = NormalizeOps::schema_call(
                    &mut state.errors.schema,
                    |schema_out| variant.is_secret(schema_out),
                    out.reborrow(),
                )?;
                let hidden = input.hidden || variant_hidden;
                let variant_name = NormalizeOps::schema_call(
                    &mut state.errors.schema,
                    |schema_out| variant.name(schema_out),
                    out.reborrow(),
                )?;
                let payload_kind = NormalizeOps::schema_call(
                    &mut state.errors.schema,
                    |schema_out| variant.payload(schema_out),
                    out.reborrow(),
                )?;
                match (payload_kind, payload) {
                    (VariantPayloadInfo::Unit, ValuePayloadView::Unit) => {
                        PlanKind::EnumUnit(variant_name)
                    }
                    (VariantPayloadInfo::Single { ty }, ValuePayloadView::Single(value)) => {
                        NormalizeOps::reserve(&mut children, &mut state.errors, 1, out.reborrow())?;
                        let type_hidden = SecretPolicy::secret_type(
                            ty,
                            Some(state.budget),
                            &mut state.errors.schema,
                            &mut state.errors.budget,
                            out.reborrow(),
                        )?;
                        NormalizeOps::push(
                            &mut children,
                            &mut state.errors.budget,
                            PlanChild {
                                input: Input {
                                    ty,
                                    value: Some(value),
                                    field: None,
                                    schema_default: input.schema_default,
                                    hidden: hidden || type_hidden,
                                },
                                role: ChildRole::EnumPayload(variant_name),
                            },
                            out.reborrow(),
                        )?;
                        PlanKind::EnumSingle(variant_name)
                    }
                    (
                        VariantPayloadInfo::Structure { fields },
                        ValuePayloadView::Structure(values),
                    ) => {
                        Self::fields(
                            state,
                            FieldsRequest {
                                input: Input { hidden, ..input },
                                fields,
                                values,
                                output: &mut children,
                                path,
                            },
                            out.reborrow(),
                        )?;
                        PlanKind::EnumStructure(variant_name)
                    }
                    _ => {
                        let diagnostic = Self::error(
                            state,
                            input,
                            path,
                            diagnostic_message!(
                                "diagnostic.config.enum_payload_shape_mismatch",
                                "enum payload shape does not match"
                            ),
                            out.reborrow(),
                        )?;
                        return Err(out.fail(diagnostic));
                    }
                }
            }
            (TypeKind::Array { element, .. }, ValueKind::Array(values)) if shape_valid => {
                NormalizeOps::reserve(
                    &mut children,
                    &mut state.errors,
                    values.len(),
                    out.reborrow(),
                )?;
                let type_hidden = SecretPolicy::secret_type(
                    element,
                    Some(state.budget),
                    &mut state.errors.schema,
                    &mut state.errors.budget,
                    out.reborrow(),
                )?;
                let hidden = input.hidden || type_hidden;
                for (index, entry) in values.enumerate() {
                    NormalizeOps::budget_call(
                        &mut state.errors.budget,
                        |budget_out| state.budget.consume(1, budget_out),
                        out.reborrow(),
                    )?;
                    NormalizeOps::push(
                        &mut children,
                        &mut state.errors.budget,
                        PlanChild {
                            input: Input {
                                ty: element,
                                value: Some(entry.value()),
                                field: None,
                                hidden,
                                schema_default: input.schema_default,
                            },
                            role: ChildRole::ArrayItem(index),
                        },
                        out.reborrow(),
                    )?;
                }
                PlanKind::Array
            }
            (TypeKind::Map { value: ty }, ValueKind::Map(entries)) => {
                NormalizeOps::reserve(
                    &mut children,
                    &mut state.errors,
                    entries.len(),
                    out.reborrow(),
                )?;
                let type_hidden = SecretPolicy::secret_type(
                    ty,
                    Some(state.budget),
                    &mut state.errors.schema,
                    &mut state.errors.budget,
                    out.reborrow(),
                )?;
                let hidden = input.hidden || type_hidden;
                for entry in entries {
                    NormalizeOps::budget_call(
                        &mut state.errors.budget,
                        |budget_out| state.budget.consume(1, budget_out),
                        out.reborrow(),
                    )?;
                    Self::node(state, out.reborrow())?;
                    let Some(entry) = entry.as_named() else {
                        return Err(out.fail(Self::missing_name_error()));
                    };
                    let key = entry.name();
                    Self::text(state, key, out.reborrow())?;
                    NormalizeOps::push(
                        &mut children,
                        &mut state.errors.budget,
                        PlanChild {
                            input: Input {
                                ty,
                                value: Some(entry.value()),
                                field: None,
                                hidden,
                                schema_default: input.schema_default,
                            },
                            role: ChildRole::MapItem(key),
                        },
                        out.reborrow(),
                    )?;
                }
                if !input.schema_default {
                    NormalizeOps::budget_call(
                        &mut state.errors.budget,
                        |sort_out| {
                            children.sort_by(
                                |left, right, mut compare_out| {
                                    state.budget.consume(1, compare_out.reborrow())?;
                                    let (ChildRole::MapItem(left), ChildRole::MapItem(right)) =
                                        (left.role, right.role)
                                    else {
                                        return Err(compare_out.fail(BudgetError::invariant(
                                            &NORMALIZE_MAP_CHILD_ROLE,
                                        )));
                                    };
                                    state
                                        .budget
                                        .compare_text(left, right, compare_out.reborrow())
                                },
                                sort_out,
                            )
                        },
                        out.reborrow(),
                    )?;
                }
                PlanKind::Map
            }
            (TypeKind::Option { inner }, ValueKind::Optional(Some(value))) => {
                NormalizeOps::reserve(&mut children, &mut state.errors, 1, out.reborrow())?;
                NormalizeOps::push(
                    &mut children,
                    &mut state.errors.budget,
                    PlanChild {
                        input: Input {
                            ty: inner,
                            value: Some(value),
                            field: None,
                            hidden: input.hidden,
                            schema_default: input.schema_default,
                        },
                        role: ChildRole::OptionalContent,
                    },
                    out.reborrow(),
                )?;
                PlanKind::OptionalSome
            }
            (TypeKind::Option { .. }, ValueKind::Optional(None)) => PlanKind::OptionalNone,
            _ if shape_valid => PlanKind::Scalar,
            _ => {
                let diagnostic = Self::error(
                    state,
                    input,
                    path,
                    diagnostic_message!(
                        "diagnostic.config.value_shape_mismatch",
                        "value shape does not match the schema type"
                    ),
                    out.reborrow(),
                )?;
                return Err(out.fail(diagnostic));
            }
        };
        Ok(Plan {
            input,
            kind,
            children,
        })
    }

    fn fields<'a, 'slot>(
        state: &mut NormalizeState<'a, '_>,
        request: FieldsRequest<'a, '_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ()> {
        let FieldsRequest {
            input,
            fields,
            values,
            output,
            path,
        } = request;
        let mut slots = NormalizeOps::budget_call(
            &mut state.errors.budget,
            |budget_out| ChargedVec::new(Some(state.budget), budget_out),
            out.reborrow(),
        )?;
        NormalizeOps::reserve(&mut slots, &mut state.errors, fields.len(), out.reborrow())?;
        NormalizeOps::budget_call(
            &mut state.errors.budget,
            |budget_out| slots.resize(fields.len(), None, budget_out),
            out.reborrow(),
        )?;
        for value in values {
            NormalizeOps::budget_call(
                &mut state.errors.budget,
                |budget_out| state.budget.consume(1, budget_out),
                out.reborrow(),
            )?;
            let Some(value) = value.as_named() else {
                return Err(out.fail(Self::missing_name_error()));
            };
            let Some((slot, _)) = NormalizeOps::budget_call(
                &mut state.errors.budget,
                |budget_out| fields.named_budgeted(value.name(), state.budget, budget_out),
                out.reborrow(),
            )?
            else {
                let diagnostic = Self::error(
                    state,
                    input,
                    path,
                    diagnostic_message!(
                        "diagnostic.config.undeclared_object_field",
                        "object contains an undeclared field"
                    ),
                    out.reborrow(),
                )?;
                return Err(out.fail(diagnostic));
            };
            let slot_value = slots
                .get_mut(slot)
                .ok_or_else(|| {
                    BudgetDiagnostic::from_error(BudgetError::invariant(&NORMALIZE_FIELD_SLOT))
                })
                .store_err_into(out.reborrow())?;
            if slot_value.replace(value.value()).is_some() {
                let diagnostic = Self::error(
                    state,
                    input,
                    path,
                    diagnostic_message!(
                        "diagnostic.config.duplicate_object_field_or_map_key",
                        "duplicate object field"
                    ),
                    out.reborrow(),
                )?;
                return Err(out.fail(diagnostic));
            }
        }
        NormalizeOps::reserve(output, &mut state.errors, fields.len(), out.reborrow())?;
        for field in fields {
            NormalizeOps::budget_call(
                &mut state.errors.budget,
                |budget_out| state.budget.consume(1, budget_out),
                out.reborrow(),
            )?;
            let slot = NormalizeOps::schema_call(
                &mut state.errors.schema,
                |schema_out| field.slot(schema_out),
                out.reborrow(),
            )?;
            let slot_value = slots
                .get(slot)
                .copied()
                .ok_or_else(|| {
                    BudgetDiagnostic::from_error(BudgetError::invariant(&NORMALIZE_FIELD_SLOT))
                })
                .store_err_into(out.reborrow())?;
            let default = NormalizeOps::schema_call(
                &mut state.errors.schema,
                |schema_out| field.default_value(schema_out),
                out.reborrow(),
            )?;
            let ty = NormalizeOps::schema_call(
                &mut state.errors.schema,
                |schema_out| field.ty(schema_out),
                out.reborrow(),
            )?;
            let kind = NormalizeOps::schema_call(
                &mut state.errors.schema,
                |schema_out| ty.kind(schema_out),
                out.reborrow(),
            )?;
            let decision = MissingFieldValue::decide(slot_value.is_some(), default, kind);
            let (value, is_schema_default) = match (slot_value, decision) {
                (Some(value), MissingFieldDecision::Provided) => (Some(value), false),
                (None, MissingFieldDecision::SchemaDefault(default)) => {
                    (Some(default.root()), true)
                }
                (None, MissingFieldDecision::OptionalNone | MissingFieldDecision::Required) => {
                    (None, false)
                }
                _ => return Err(out.fail(NormalizeOps::invariant_error())),
            };
            let role = match input.value.map(ValueView::as_ref) {
                Some(ValueKind::Enumeration { variant, .. }) => {
                    ChildRole::VariantField { variant, field }
                }
                _ => ChildRole::Field(field),
            };
            NormalizeOps::push(
                output,
                &mut state.errors.budget,
                PlanChild {
                    input: Input {
                        ty,
                        value,
                        field: Some(field),
                        hidden: input.hidden,
                        schema_default: input.schema_default || is_schema_default,
                    },
                    role,
                },
                out.reborrow(),
            )?;
        }
        Ok(())
    }

    pub(super) fn text<'slot>(
        state: &mut NormalizeState<'_, '_>,
        text: &str,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ()> {
        if text.len() > state.limits.max_string_bytes() {
            return Err(out.fail(BudgetDiagnostic::from_error(BudgetError::Resource)));
        }
        NormalizeOps::budget_call(
            &mut state.errors.budget,
            |budget_out| {
                state
                    .budget
                    .construct_text(text.len(), state.request.text_limit, budget_out)
            },
            out.reborrow(),
        )?;
        Ok(())
    }

    fn missing_name_error() -> Diagnostic {
        DiagnosticFactory::internal(NAMED_ENTRY_NAME_MISSING, None, None, None)
    }
}
