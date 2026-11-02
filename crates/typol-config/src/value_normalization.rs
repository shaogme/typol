mod builder;
mod model;
mod planner;

use crate::{
    diagnostics::BudgetDiagnostic,
    secret::SecretPolicy,
    value_normalization::{
        builder::NormalizeBuilder,
        model::{Input, NormalizeErrors, NormalizeOps, NormalizeState, Task},
        planner::NormalizePlanner,
    },
};
use opeo::{ErrSlot, OResult, Out};
use typol_core::{
    BudgetError, ChargedVec, ConfigValue, Diagnostic, ExecutionBudget, InstancePath, ValueKind,
    ValuePayloadView, ValueView,
};
use typol_policy::ValueLimits;

pub(crate) use model::NormalizeRequest;

/// Holds a normalizer-rebuilt value as an internal proof that its full tree shape was checked.
pub(crate) struct NormalizedValue {
    value: ConfigValue,
}

impl NormalizedValue {
    pub(crate) fn into_config_value(self) -> ConfigValue {
        self.value
    }
}

pub(crate) struct ValueNormalizer;

impl ValueNormalizer {
    /// Borrows the external tree and rebuilds it against the actual Schema without changing the original tree or the source and order of defaults.
    pub(crate) fn copy<'slot, 'borrow>(
        request: NormalizeRequest<'_>,
        nodes: &mut usize,
        limits: &ValueLimits,
        budget: &ExecutionBudget,
        out: Out<'slot, 'borrow, Diagnostic>,
    ) -> OResult<'slot, NormalizedValue> {
        let primary = request.primary;
        let mut errors = ErrSlot::new();
        match errors
            .try_call(|inner_out| Self::copy_inner(request, nodes, limits, budget, inner_out))
        {
            Ok(value) => Ok(NormalizedValue { value }),
            Err(caught) => {
                let mut diagnostic = caught.take();
                if diagnostic.located_primary().is_none() {
                    diagnostic.set_located_primary(primary);
                }
                Err(out.fail(diagnostic))
            }
        }
    }

    fn copy_inner<'slot>(
        request: NormalizeRequest<'_>,
        nodes: &mut usize,
        limits: &ValueLimits,
        budget: &ExecutionBudget,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ConfigValue> {
        let mut state = NormalizeState {
            request,
            nodes,
            limits,
            budget,
            errors: NormalizeErrors {
                budget: ErrSlot::new(),
                schema: ErrSlot::new(),
                value: ErrSlot::new(),
            },
        };
        let mut tasks = NormalizeOps::budget_call(
            &mut state.errors.budget,
            |budget_out| ChargedVec::new(Some(budget), budget_out),
            out.reborrow(),
        )?;
        let mut values = NormalizeOps::budget_call(
            &mut state.errors.budget,
            |budget_out| ChargedVec::new(Some(budget), budget_out),
            out.reborrow(),
        )?;
        let mut path = NormalizeOps::budget_call(
            &mut state.errors.budget,
            |budget_out| InstancePath::new(Some(budget), budget_out),
            out.reborrow(),
        )?;
        NormalizeOps::reserve(&mut tasks, &mut state.errors, 1, out.reborrow())?;
        NormalizeOps::push(
            &mut tasks,
            &mut state.errors.budget,
            Task::Enter(
                Input {
                    ty: state.request.expected,
                    value: Some(state.request.value),
                    field: None,
                    hidden: false,
                    schema_default: state.request.schema_default,
                },
                state.request.depth,
            ),
            out.reborrow(),
        )?;
        while let Some(task) = tasks.pop() {
            NormalizeOps::budget_call(
                &mut state.errors.budget,
                |budget_out| budget.consume(1, budget_out),
                out.reborrow(),
            )?;
            match task {
                Task::Child(child, depth) => {
                    let restore = path.len();
                    if !child.input.hidden {
                        NormalizePlanner::append_role(
                            child.role,
                            &mut path,
                            &mut state.errors,
                            out.reborrow(),
                        )?;
                    }
                    NormalizeOps::reserve(&mut tasks, &mut state.errors, 2, out.reborrow())?;
                    NormalizeOps::push(
                        &mut tasks,
                        &mut state.errors.budget,
                        Task::Restore(restore),
                        out.reborrow(),
                    )?;
                    NormalizeOps::push(
                        &mut tasks,
                        &mut state.errors.budget,
                        Task::Enter(child.input, depth),
                        out.reborrow(),
                    )?;
                }
                Task::Enter(mut input, depth) => {
                    if !limits.allows_depth(depth) {
                        return Err(out.fail(BudgetDiagnostic::from_error(BudgetError::Resource)));
                    }
                    NormalizePlanner::node(&mut state, out.reborrow())?;
                    let field_hidden = if let Some(field) = input.field {
                        NormalizeOps::schema_call(
                            &mut state.errors.schema,
                            |schema_out| field.is_secret(schema_out),
                            out.reborrow(),
                        )?
                    } else {
                        false
                    };
                    let type_hidden = SecretPolicy::secret_type(
                        input.ty,
                        Some(budget),
                        &mut state.errors.schema,
                        &mut state.errors.budget,
                        out.reborrow(),
                    )?;
                    input.hidden |= field_hidden || type_hidden;
                    let plan = NormalizePlanner::plan(&mut state, input, &path, out.reborrow())?;
                    let Some(count) = plan.children.len().checked_add(1) else {
                        return Err(out.fail(BudgetDiagnostic::from_error(BudgetError::Resource)));
                    };
                    NormalizeOps::reserve(&mut tasks, &mut state.errors, count, out.reborrow())?;
                    let Some(next) = depth.checked_add(1) else {
                        return Err(out.fail(BudgetDiagnostic::from_error(BudgetError::Resource)));
                    };
                    let extra = usize::from(matches!(
                        plan.input.value.map(ValueView::as_ref),
                        Some(ValueKind::Enumeration {
                            payload: ValuePayloadView::Structure(_),
                            ..
                        })
                    ));
                    if extra != 0 && !limits.allows_depth(next) {
                        return Err(out.fail(BudgetDiagnostic::from_error(BudgetError::Resource)));
                    }
                    let Some(next) = next.checked_add(extra) else {
                        return Err(out.fail(BudgetDiagnostic::from_error(BudgetError::Resource)));
                    };
                    let start = values.len();
                    let mut children = NormalizeOps::budget_call(
                        &mut state.errors.budget,
                        |budget_out| ChargedVec::new(Some(budget), budget_out),
                        out.reborrow(),
                    )?;
                    NormalizeOps::reserve(
                        &mut children,
                        &mut state.errors,
                        plan.children.len(),
                        out.reborrow(),
                    )?;
                    NormalizeOps::budget_call(
                        &mut state.errors.budget,
                        |budget_out| children.extend(plan.children.iter().copied(), budget_out),
                        out.reborrow(),
                    )?;
                    NormalizeOps::push(
                        &mut tasks,
                        &mut state.errors.budget,
                        Task::Finish(plan, start),
                        out.reborrow(),
                    )?;
                    for child in children.into_iter().rev() {
                        NormalizeOps::budget_call(
                            &mut state.errors.budget,
                            |budget_out| budget.consume(1, budget_out),
                            out.reborrow(),
                        )?;
                        NormalizeOps::push(
                            &mut tasks,
                            &mut state.errors.budget,
                            Task::Child(child, next),
                            out.reborrow(),
                        )?;
                    }
                }
                Task::Finish(plan, start) => {
                    let mut children = NormalizeOps::budget_call(
                        &mut state.errors.budget,
                        |budget_out| ChargedVec::new(Some(budget), budget_out),
                        out.reborrow(),
                    )?;
                    NormalizeOps::reserve(
                        &mut children,
                        &mut state.errors,
                        values.len() - start,
                        out.reborrow(),
                    )?;
                    let drained = values.drain(start..);
                    NormalizeOps::budget_call(
                        &mut state.errors.budget,
                        |budget_out| children.extend(drained, budget_out),
                        out.reborrow(),
                    )?;
                    let value =
                        NormalizeBuilder::finish(&mut state, plan, children, out.reborrow())?;
                    NormalizeOps::budget_call(
                        &mut state.errors.budget,
                        |budget_out| values.try_push(value, budget_out),
                        out.reborrow(),
                    )?;
                }
                Task::Restore(depth) => path.truncate(depth),
            }
        }
        match values.pop() {
            Some(value) => Ok(value),
            None => Err(out.fail(BudgetDiagnostic::from_error(BudgetError::Resource))),
        }
    }
}
