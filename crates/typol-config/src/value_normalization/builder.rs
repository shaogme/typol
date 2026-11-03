use crate::{
    value::{EnumPayload, FieldValue, MapEntry, ValueFactory},
    value_normalization::{
        model::{ChildRole, NormalizeOps, NormalizeState, Plan, PlanChild, PlanKind},
        planner::NormalizePlanner,
    },
};
use opeo::{OResult, Out};
use typol_core::{ChargedVec, ConfigValue, Diagnostic, ValueKind, ValueSource};

pub(super) struct NormalizeBuilder;

impl NormalizeBuilder {
    pub(super) fn finish<'slot>(
        state: &mut NormalizeState<'_, '_>,
        plan: Plan<'_>,
        children: ChargedVec<ConfigValue>,
        out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ConfigValue> {
        if children.len() != plan.children.len() {
            return Err(out.fail(NormalizeOps::invariant_error()));
        }
        let source = if plan.input.schema_default {
            ValueSource::SchemaDefault
        } else {
            ValueSource::RustValue
        };
        match plan.kind {
            PlanKind::MissingOptional | PlanKind::OptionalNone => {
                Self::finish_none(state, plan, source, out)
            }
            PlanKind::Scalar => Self::finish_scalar(state, plan, out),
            PlanKind::OptionalSome => Self::finish_optional(state, plan, children, source, out),
            PlanKind::Array => Self::finish_array(state, plan, children, source, out),
            PlanKind::Map => Self::finish_map(state, plan, children, source, out),
            PlanKind::Structure => Self::finish_structure(state, plan, children, source, out),
            PlanKind::EnumUnit(_) => Self::finish_enum_unit(state, plan, source, out),
            PlanKind::EnumSingle(_) => Self::finish_enum_single(state, plan, children, source, out),
            PlanKind::EnumStructure(_) => {
                Self::finish_enum_structure(state, plan, children, source, out)
            }
        }
    }

    fn finish_none<'slot>(
        state: &mut NormalizeState<'_, '_>,
        plan: Plan<'_>,
        source: ValueSource,
        out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ConfigValue> {
        if !plan.children.is_empty() {
            return Err(out.fail(NormalizeOps::invariant_error()));
        }
        NormalizeOps::value_call(
            &mut state.errors.value,
            |value_out| ValueFactory::none(source, Some(state.budget), value_out),
            out,
        )
    }

    fn finish_scalar<'slot>(
        state: &mut NormalizeState<'_, '_>,
        plan: Plan<'_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ConfigValue> {
        if !plan.children.is_empty() {
            return Err(out.fail(NormalizeOps::invariant_error()));
        }
        let Some(value) = plan.input.value else {
            return Err(out.fail(NormalizeOps::invariant_error()));
        };
        let source = if plan.input.schema_default {
            ValueSource::SchemaDefault
        } else {
            ValueSource::RustValue
        };
        let budget = Some(state.budget);
        if let ValueKind::String(text) = value.as_ref() {
            NormalizePlanner::text(state, text, out.reborrow())?;
        }
        NormalizeOps::value_call(
            &mut state.errors.value,
            |value_out| ValueFactory::scalar(value, source, budget, value_out),
            out,
        )
    }

    fn finish_optional<'slot>(
        state: &mut NormalizeState<'_, '_>,
        plan: Plan<'_>,
        children: ChargedVec<ConfigValue>,
        source: ValueSource,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ConfigValue> {
        if !matches!(
            plan.children.first().map(|child| child.role),
            Some(ChildRole::OptionalContent)
        ) {
            return Err(out.fail(NormalizeOps::invariant_error()));
        }
        let mut children = children.into_iter();
        let single = NormalizeOps::take_single(&mut children, out.reborrow())?;
        NormalizeOps::value_call(
            &mut state.errors.value,
            |value_out| ValueFactory::optional_owned(single, source, Some(state.budget), value_out),
            out,
        )
    }

    fn finish_array<'slot>(
        state: &mut NormalizeState<'_, '_>,
        plan: Plan<'_>,
        children: ChargedVec<ConfigValue>,
        source: ValueSource,
        out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ConfigValue> {
        if plan
            .children
            .iter()
            .any(|child| !matches!(child.role, ChildRole::ArrayItem(_)))
        {
            return Err(out.fail(NormalizeOps::invariant_error()));
        }
        let budget = Some(state.budget);
        NormalizeOps::value_call(
            &mut state.errors.value,
            |value_out| ValueFactory::array_owned(children, source, budget, value_out),
            out,
        )
    }

    fn finish_map<'slot>(
        state: &mut NormalizeState<'_, '_>,
        plan: Plan<'_>,
        children: ChargedVec<ConfigValue>,
        source: ValueSource,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ConfigValue> {
        let budget = Some(state.budget);
        let mut output = NormalizeOps::budget_call(
            &mut state.errors.budget,
            |budget_out| ChargedVec::new(budget, budget_out),
            out.reborrow(),
        )?;
        NormalizeOps::reserve(
            &mut output,
            &mut state.errors,
            children.len(),
            out.reborrow(),
        )?;
        for (child, value) in plan.children.iter().zip(children) {
            let ChildRole::MapItem(key) = child.role else {
                return Err(out.fail(NormalizeOps::invariant_error()));
            };
            let entry = NormalizeOps::budget_call(
                &mut state.errors.budget,
                |budget_out| MapEntry::try_new(key, value, budget, budget_out),
                out.reborrow(),
            )?;
            NormalizeOps::budget_call(
                &mut state.errors.budget,
                |budget_out| output.try_push(entry, budget_out),
                out.reborrow(),
            )?;
        }
        NormalizeOps::value_call(
            &mut state.errors.value,
            |value_out| ValueFactory::map_owned(output, source, budget, value_out),
            out,
        )
    }

    fn finish_structure<'slot>(
        state: &mut NormalizeState<'_, '_>,
        plan: Plan<'_>,
        children: ChargedVec<ConfigValue>,
        source: ValueSource,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ConfigValue> {
        let fields = Self::finish_fields(
            state,
            &plan.children,
            children.into_iter(),
            None,
            out.reborrow(),
        )?;
        NormalizeOps::value_call(
            &mut state.errors.value,
            |value_out| {
                ValueFactory::structure_owned(fields, source, Some(state.budget), value_out)
            },
            out,
        )
    }

    fn finish_enum_unit<'slot>(
        state: &mut NormalizeState<'_, '_>,
        plan: Plan<'_>,
        source: ValueSource,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ConfigValue> {
        let PlanKind::EnumUnit(variant) = plan.kind else {
            return Err(out.fail(NormalizeOps::invariant_error()));
        };
        if !plan.children.is_empty() {
            return Err(out.fail(NormalizeOps::invariant_error()));
        }
        NormalizePlanner::text(state, variant, out.reborrow())?;
        NormalizeOps::value_call(
            &mut state.errors.value,
            |value_out| {
                ValueFactory::enumeration(
                    variant,
                    EnumPayload::unit(),
                    source,
                    Some(state.budget),
                    value_out,
                )
            },
            out,
        )
    }

    fn finish_enum_single<'slot>(
        state: &mut NormalizeState<'_, '_>,
        plan: Plan<'_>,
        children: ChargedVec<ConfigValue>,
        source: ValueSource,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ConfigValue> {
        let PlanKind::EnumSingle(variant) = plan.kind else {
            return Err(out.fail(NormalizeOps::invariant_error()));
        };
        NormalizePlanner::text(state, variant, out.reborrow())?;
        if !matches!(plan.children.first().map(|child| child.role), Some(ChildRole::EnumPayload(name)) if name == variant)
        {
            return Err(out.fail(NormalizeOps::invariant_error()));
        }
        let mut children = children.into_iter();
        let single = NormalizeOps::take_single(&mut children, out.reborrow())?;
        let payload = EnumPayload::single(single);
        NormalizeOps::value_call(
            &mut state.errors.value,
            |value_out| {
                ValueFactory::enumeration(variant, payload, source, Some(state.budget), value_out)
            },
            out,
        )
    }

    fn finish_enum_structure<'slot>(
        state: &mut NormalizeState<'_, '_>,
        plan: Plan<'_>,
        children: ChargedVec<ConfigValue>,
        source: ValueSource,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ConfigValue> {
        let PlanKind::EnumStructure(variant) = plan.kind else {
            return Err(out.fail(NormalizeOps::invariant_error()));
        };
        NormalizePlanner::text(state, variant, out.reborrow())?;
        let fields = Self::finish_fields(
            state,
            &plan.children,
            children.into_iter(),
            Some(variant),
            out.reborrow(),
        )?;
        let payload = EnumPayload::structure(fields);
        NormalizeOps::value_call(
            &mut state.errors.value,
            |value_out| {
                ValueFactory::enumeration(variant, payload, source, Some(state.budget), value_out)
            },
            out,
        )
    }

    fn finish_fields<'slot>(
        state: &mut NormalizeState<'_, '_>,
        children: &[PlanChild<'_>],
        values: impl Iterator<Item = ConfigValue>,
        variant: Option<&str>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ChargedVec<FieldValue>> {
        let mut output = NormalizeOps::budget_call(
            &mut state.errors.budget,
            |budget_out| ChargedVec::new(Some(state.budget), budget_out),
            out.reborrow(),
        )?;
        NormalizeOps::reserve(
            &mut output,
            &mut state.errors,
            children.len(),
            out.reborrow(),
        )?;
        let mut values = values;
        for child in children {
            let field = match (variant, child.role) {
                (None, ChildRole::Field(field)) => field,
                (
                    Some(expected),
                    ChildRole::VariantField {
                        variant: actual,
                        field,
                    },
                ) if expected == actual => field,
                _ => return Err(out.fail(NormalizeOps::invariant_error())),
            };
            let Some(value) = values.next() else {
                return Err(out.fail(NormalizeOps::invariant_error()));
            };
            let name = NormalizeOps::schema_call(
                &mut state.errors.schema,
                |schema_out| field.name(schema_out),
                out.reborrow(),
            )?;
            NormalizePlanner::text(state, name, out.reborrow())?;
            let value = NormalizeOps::budget_call(
                &mut state.errors.budget,
                |budget_out| FieldValue::try_new(name, value, Some(state.budget), budget_out),
                out.reborrow(),
            )?;
            NormalizeOps::budget_call(
                &mut state.errors.budget,
                |budget_out| output.try_push(value, budget_out),
                out.reborrow(),
            )?;
        }
        if values.next().is_some() {
            return Err(out.fail(NormalizeOps::invariant_error()));
        }
        Ok(output)
    }
}
