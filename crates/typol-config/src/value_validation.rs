mod support;
mod tree;

#[cfg(test)]
mod tests;

use crate::{
    ConfigParseErrors, ConfigValidationOptions,
    context::{CompletionState, SemanticStatus},
    diagnostics::{BudgetDiagnostic, DiagnosticBuffer},
    instance::{LocationIndex, Member, NodeId},
    session::ValidationResources,
    validation::{AssertionsRequest, Validator},
    value_normalization::{NormalizeRequest, ValueNormalizer},
    value_validation::tree::TreeTraversal,
};
use opeo::{ErrSlot, OResult, Out};
use typol_core::{ChargedVec, ConfigValue, Diagnostic, ExecutionBudget, InstancePath, LocatedSpan};
use typol_semantic::{
    CompiledSchema, ConstraintList, FieldView, RegexExecutor, TypeView, VariantView,
};

#[derive(Clone, Copy)]
pub(crate) struct TreeTarget<'schema> {
    pub(crate) expected: TypeView<'schema>,
    pub(crate) field: Option<FieldView<'schema>>,
    pub(crate) variant: Option<VariantView<'schema>>,
    pub(crate) span: Option<LocatedSpan>,
}

pub(crate) struct TreeContext<'schema, 'work, 'limits> {
    pub(crate) schema: &'schema CompiledSchema,
    pub(crate) resources: &'work mut ValidationResources<'limits>,
    pub(crate) diagnostic_errors: &'work mut ErrSlot<Diagnostic>,
    pub(crate) locations: &'work mut LocationIndex<'schema>,
    pub(crate) current_node: Option<NodeId>,
    pub(crate) next_member: Member<'schema>,
    pub(crate) inherited_constraints: &'work mut ChargedVec<ConstraintList<'schema>>,
    pub(crate) path: &'work mut InstancePath,
    pub(crate) diagnostics: &'work mut DiagnosticBuffer<'schema>,
    pub(crate) nesting_depth: usize,
    pub(crate) value_nodes: usize,
    pub(crate) shape_validated: bool,
    #[cfg(test)]
    pub(crate) work_peak: usize,
    #[cfg(test)]
    pub(crate) shape_skips: usize,
}

pub(crate) struct ValueValidation;

pub(crate) enum ValueValidationInput<'value> {
    External(&'value ConfigValue),
    SchemaComplete(ConfigValue),
}

pub(crate) struct ValueValidationRequest<'schema, 'value, 'options, 'budget> {
    pub(crate) schema: &'schema CompiledSchema,
    pub(crate) value: ValueValidationInput<'value>,
    pub(crate) options: &'options ConfigValidationOptions,
    pub(crate) budget: &'budget ExecutionBudget,
}

impl ValueValidation {
    pub(crate) fn subtree<'schema, 'slot>(
        context: &mut TreeContext<'schema, '_, '_>,
        target: TreeTarget<'schema>,
        value: &ConfigValue,
        completion: CompletionState,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, SemanticStatus> {
        TreeTraversal::subtree(context, target, value, completion, out.reborrow())
    }

    pub(crate) fn value<'slot, 'borrow>(
        request: ValueValidationRequest<'_, '_, '_, '_>,
        out: Out<'slot, 'borrow, ConfigParseErrors>,
    ) -> OResult<'slot, ConfigValue> {
        let ValueValidationRequest {
            schema,
            value,
            options,
            budget,
        } = request;
        let mut operation_errors = ErrSlot::new();
        let mut budget_errors = ErrSlot::new();
        let mut diagnostics = match operation_errors.try_call(|diagnostic_out| {
            DiagnosticBuffer::new_budgeted(
                schema,
                options.config().max_diagnostics(),
                budget,
                diagnostic_out,
            )
        }) {
            Ok(diagnostics) => diagnostics,
            Err(caught) => {
                return Err(out.fail(ConfigParseErrors::emergency(caught.take(), None)));
            }
        };
        let mut nodes = 0;
        let (value, shape_validated) = match value {
            ValueValidationInput::External(value) => {
                let normalized = match operation_errors.try_call(|diagnostic_out| {
                    ValueNormalizer::copy(
                        NormalizeRequest {
                            expected: schema.entry(),
                            value: value.root(),
                            depth: 0,
                            primary: None,
                            text_limit: options.config().max_constructed_text_bytes(),
                            schema_default: false,
                        },
                        &mut nodes,
                        options.config().values(),
                        budget,
                        diagnostic_out,
                    )
                }) {
                    Ok(value) => value,
                    Err(caught) => {
                        diagnostics.push(caught.take());
                        return Err(out.fail(ConfigParseErrors::finish(
                            diagnostics,
                            schema,
                            None,
                            None,
                        )));
                    }
                };
                (normalized.into_config_value(), true)
            }
            ValueValidationInput::SchemaComplete(value) => (value, false),
        };
        let value_nodes = if shape_validated {
            nodes
        } else {
            options.config().values().max_value_nodes()
        };
        let validation_budget = match budget_errors
            .try_call(|budget_out| budget.try_clone(budget_out))
        {
            Ok(budget) => budget,
            Err(caught) => {
                diagnostics.push(BudgetDiagnostic::from_error(caught.take()));
                return Err(out.fail(ConfigParseErrors::finish(diagnostics, schema, None, None)));
            }
        };
        let mut locations = match operation_errors
            .try_call(|diagnostic_out| LocationIndex::new_budgeted(budget, diagnostic_out))
        {
            Ok(locations) => locations,
            Err(caught) => {
                diagnostics.push(caught.take());
                return Err(out.fail(ConfigParseErrors::finish(diagnostics, schema, None, None)));
            }
        };
        let mut path = match budget_errors
            .try_call(|budget_out| InstancePath::new(Some(budget), budget_out))
        {
            Ok(path) => path,
            Err(caught) => {
                diagnostics.push(BudgetDiagnostic::from_error(caught.take()));
                return Err(out.fail(ConfigParseErrors::finish(diagnostics, schema, None, None)));
            }
        };
        let mut inherited = match budget_errors
            .try_call(|budget_out| BudgetDiagnostic::workspace_vec(Some(budget), budget_out))
        {
            Ok(inherited) => inherited,
            Err(caught) => {
                diagnostics.push(BudgetDiagnostic::from_error(caught.take()));
                return Err(out.fail(ConfigParseErrors::finish(diagnostics, schema, None, None)));
            }
        };
        let mut resources = ValidationResources {
            scratch: RegexExecutor::new(),
            value_limits: options.config().values(),
            regex_limits: options.resources().regex(),
            budget: validation_budget,
            budget_errors: ErrSlot::new(),
            schema_access_errors: ErrSlot::new(),
            schema_iteration_errors: ErrSlot::new(),
            environment_text_errors: ErrSlot::new(),
            rule_owner_errors: ErrSlot::new(),
            eval_errors: ErrSlot::new(),
            regex_errors: ErrSlot::new(),
        };
        let mut tree_diagnostic_errors = ErrSlot::new();
        let subtree_error = {
            let mut context = TreeContext {
                schema,
                resources: &mut resources,
                diagnostic_errors: &mut tree_diagnostic_errors,
                locations: &mut locations,
                current_node: None,
                next_member: Member::Root,
                inherited_constraints: &mut inherited,
                path: &mut path,
                diagnostics: &mut diagnostics,
                nesting_depth: 0,
                value_nodes,
                shape_validated,
                #[cfg(test)]
                work_peak: 0,
                #[cfg(test)]
                shape_skips: 0,
            };
            match operation_errors.try_call(|diagnostic_out| {
                Self::subtree(
                    &mut context,
                    TreeTarget {
                        expected: schema.entry(),
                        field: None,
                        variant: None,
                        span: None,
                    },
                    &value,
                    CompletionState::NeedsTraversal,
                    diagnostic_out,
                )
            }) {
                Ok(_) => None,
                Err(caught) => Some(caught.take()),
            }
        };
        if let Some(error) = subtree_error {
            diagnostics.push(error);
        }
        if diagnostics.is_empty() {
            let result = operation_errors.try_call(|diagnostic_out| {
                Validator::assertions(
                    AssertionsRequest {
                        schema,
                        root: &value,
                        locations: &locations,
                        resources: &mut resources,
                        diagnostics: &mut diagnostics,
                    },
                    diagnostic_out,
                )
            });
            if let Err(caught) = result {
                diagnostics.push(caught.take());
            }
        }
        if diagnostics.is_empty() {
            Ok(value)
        } else {
            Err(out.fail(ConfigParseErrors::finish(diagnostics, schema, None, None)))
        }
    }
}
