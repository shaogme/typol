mod builder;
mod model;
mod planner;

pub(crate) use model::NormalizeRequest;

use crate::{
    diagnostics::BudgetDiagnostic,
    secret::{SecretPolicy, SecretTypeContext},
    value_normalization::{
        builder::NormalizeBuilder,
        model::{
            Input, NormalizeBudgetErrors, NormalizeErrors, NormalizeOps, NormalizeSchemaErrors,
            NormalizeState, NormalizeValueErrors, Plan, PlanChild, Task,
        },
        planner::NormalizePlanner,
    },
};
use opeo::{ErrSlot, OResult, Out};
use typol_core::{
    BudgetError, ChargedVec, ConfigValue, Diagnostic, ExecutionBudget, InstancePath, ValueKind,
    ValuePayloadView, ValueView,
};
use typol_policy::ValueLimits;
use typol_semantic::TypeKind;

pub(crate) enum MissingFieldDecision<'value> {
    Provided,
    SchemaDefault(&'value ConfigValue),
    OptionalNone,
    Required,
}

pub(crate) struct MissingFieldValue;

impl MissingFieldValue {
    pub(crate) fn decide<'value>(
        provided: bool,
        default: Option<&'value ConfigValue>,
        kind: TypeKind<'_>,
    ) -> MissingFieldDecision<'value> {
        if provided {
            MissingFieldDecision::Provided
        } else if let Some(default) = default {
            MissingFieldDecision::SchemaDefault(default)
        } else if matches!(kind, TypeKind::Option { .. }) {
            MissingFieldDecision::OptionalNone
        } else {
            MissingFieldDecision::Required
        }
    }
}

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

struct NormalizeWork<'schema> {
    tasks: ChargedVec<Task<'schema>>,
    values: ChargedVec<ConfigValue>,
    path: InstancePath,
}

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
        let translator = request.translator;
        let mut state = NormalizeState {
            request,
            nodes,
            limits,
            budget,
            errors: NormalizeErrors {
                budget: NormalizeBudgetErrors {
                    slot: ErrSlot::new(),
                    translator,
                },
                schema: NormalizeSchemaErrors {
                    slot: ErrSlot::new(),
                    translator,
                },
                value: NormalizeValueErrors {
                    slot: ErrSlot::new(),
                    translator,
                },
            },
        };
        let mut work = NormalizeWork::new(&mut state, out.reborrow())?;
        work.run(&mut state, out)
    }
}

impl<'schema> NormalizeWork<'schema> {
    fn new<'slot>(
        state: &mut NormalizeState<'schema, '_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Self> {
        let budget = Some(state.budget);
        let tasks = NormalizeOps::budget_call(
            &mut state.errors.budget,
            |budget_out| ChargedVec::new(budget, budget_out),
            out.reborrow(),
        )?;
        let values = NormalizeOps::budget_call(
            &mut state.errors.budget,
            |budget_out| ChargedVec::new(budget, budget_out),
            out.reborrow(),
        )?;
        let path = NormalizeOps::budget_call(
            &mut state.errors.budget,
            |budget_out| InstancePath::new(budget, budget_out),
            out.reborrow(),
        )?;
        Ok(Self {
            tasks,
            values,
            path,
        })
    }

    fn run<'slot>(
        &mut self,
        state: &mut NormalizeState<'schema, '_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ConfigValue> {
        NormalizeOps::reserve(&mut self.tasks, &mut state.errors, 1, out.reborrow())?;
        NormalizeOps::push(
            &mut self.tasks,
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
        while let Some(task) = self.tasks.pop() {
            NormalizeOps::budget_call(
                &mut state.errors.budget,
                |budget_out| state.budget.consume(1, budget_out),
                out.reborrow(),
            )?;
            match task {
                Task::Child(child, depth) => {
                    self.push_child(state, child, depth, out.reborrow())?
                }
                Task::Enter(input, depth) => self.enter(state, input, depth, out.reborrow())?,
                Task::Finish(plan, start) => self.finish(state, plan, start, out.reborrow())?,
                Task::Restore(depth) => self.path.truncate(depth),
            }
        }
        match self.values.pop() {
            Some(value) => Ok(value),
            None => Err(out.fail(BudgetDiagnostic::from_error(
                BudgetError::Resource,
                state.request.translator,
            ))),
        }
    }

    fn push_child<'slot>(
        &mut self,
        state: &mut NormalizeState<'schema, '_>,
        child: PlanChild<'schema>,
        depth: usize,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ()> {
        let restore = self.path.len();
        if !child.input.hidden {
            NormalizePlanner::append_role(
                child.role,
                &mut self.path,
                &mut state.errors,
                out.reborrow(),
            )?;
        }
        NormalizeOps::reserve(&mut self.tasks, &mut state.errors, 2, out.reborrow())?;
        NormalizeOps::push(
            &mut self.tasks,
            &mut state.errors.budget,
            Task::Restore(restore),
            out.reborrow(),
        )?;
        NormalizeOps::push(
            &mut self.tasks,
            &mut state.errors.budget,
            Task::Enter(child.input, depth),
            out,
        )
    }

    fn enter<'slot>(
        &mut self,
        state: &mut NormalizeState<'schema, '_>,
        mut input: Input<'schema>,
        depth: usize,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ()> {
        if !state.limits.allows_depth(depth) {
            return Err(out.fail(BudgetDiagnostic::from_error(
                BudgetError::Resource,
                state.request.translator,
            )));
        }
        NormalizePlanner::node(state, out.reborrow())?;
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
            SecretTypeContext {
                budget: Some(state.budget),
                schema_errors: &mut state.errors.schema.slot,
                budget_errors: &mut state.errors.budget.slot,
                translator: state.request.translator,
            },
            out.reborrow(),
        )?;
        input.hidden |= field_hidden || type_hidden;
        let plan = NormalizePlanner::plan(state, input, &self.path, out.reborrow())?;
        let Some(count) = plan.children.len().checked_add(1) else {
            return Err(out.fail(BudgetDiagnostic::from_error(
                BudgetError::Resource,
                state.request.translator,
            )));
        };
        NormalizeOps::reserve(&mut self.tasks, &mut state.errors, count, out.reborrow())?;
        let Some(next) = depth.checked_add(1) else {
            return Err(out.fail(BudgetDiagnostic::from_error(
                BudgetError::Resource,
                state.request.translator,
            )));
        };
        let extra = usize::from(matches!(
            plan.input.value.map(ValueView::as_ref),
            Some(ValueKind::Enumeration {
                payload: ValuePayloadView::Structure(_),
                ..
            })
        ));
        if extra != 0 && !state.limits.allows_depth(next) {
            return Err(out.fail(BudgetDiagnostic::from_error(
                BudgetError::Resource,
                state.request.translator,
            )));
        }
        let Some(next) = next.checked_add(extra) else {
            return Err(out.fail(BudgetDiagnostic::from_error(
                BudgetError::Resource,
                state.request.translator,
            )));
        };
        let start = self.values.len();
        let budget = Some(state.budget);
        let mut children = NormalizeOps::budget_call(
            &mut state.errors.budget,
            |budget_out| ChargedVec::new(budget, budget_out),
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
            &mut self.tasks,
            &mut state.errors.budget,
            Task::Finish(plan, start),
            out.reborrow(),
        )?;
        for child in children.into_iter().rev() {
            NormalizeOps::budget_call(
                &mut state.errors.budget,
                |budget_out| state.budget.consume(1, budget_out),
                out.reborrow(),
            )?;
            NormalizeOps::push(
                &mut self.tasks,
                &mut state.errors.budget,
                Task::Child(child, next),
                out.reborrow(),
            )?;
        }
        Ok(())
    }

    fn finish<'slot>(
        &mut self,
        state: &mut NormalizeState<'schema, '_>,
        plan: Plan<'schema>,
        start: usize,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ()> {
        let budget = Some(state.budget);
        let mut children = NormalizeOps::budget_call(
            &mut state.errors.budget,
            |budget_out| ChargedVec::new(budget, budget_out),
            out.reborrow(),
        )?;
        NormalizeOps::reserve(
            &mut children,
            &mut state.errors,
            self.values.len() - start,
            out.reborrow(),
        )?;
        let drained = self.values.drain(start..);
        NormalizeOps::budget_call(
            &mut state.errors.budget,
            |budget_out| children.extend(drained, budget_out),
            out.reborrow(),
        )?;
        let value = NormalizeBuilder::finish(state, plan, children, out.reborrow())?;
        NormalizeOps::budget_call(
            &mut state.errors.budget,
            |budget_out| self.values.try_push(value, budget_out),
            out,
        )
    }
}
