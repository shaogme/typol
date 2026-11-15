use crate::{
    diagnostics::{
        BudgetDiagnostic, ConfigParseErrors, ConfigValueErrors, DiagnosticBuffer, DiagnosticFactory,
    },
    options::ConfigValidationRequest,
    value::{NormalizedConfigValue, ValidatedConfigValue},
    value_normalization::{NormalizeRequest, ValueNormalizer},
    value_validation::{ValueValidation, ValueValidationRequest},
};
use ::opeo::{ErrSlot, OResult, Out};
use typol_core::{
    ConfigValue, DiagnosticContext, ExecutionBudget, ValueSource, diagnostic_context,
};
use typol_semantic::CompiledSchema;

static CONFIG_VALUE_BUDGET_MISMATCH: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "config-value-validation-entry",
    "execution-budget-does-not-match-options",
);
static CONFIG_VALUE_NORMALIZATION_BUDGET_MISMATCH: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "config-value-normalization-entry",
    "execution-budget-does-not-match-options",
);

/// Normalizes and validates external values against one compiled Schema.
pub struct ConfigValueProcessor<'schema> {
    schema: &'schema CompiledSchema,
}

impl<'schema> ConfigValueProcessor<'schema> {
    pub const fn new(schema: &'schema CompiledSchema) -> Self {
        Self { schema }
    }

    /// Normalize an owned external value with Schema defaults, without evaluating constraints.
    /// Values carrying environment origins are rejected because normalization alone does not
    /// perform the complete authorization and fallback checks required for those references.
    pub fn normalize<'slot>(
        &self,
        value: ConfigValue,
        request: ConfigValidationRequest,
        out: Out<'slot, '_, ConfigValueErrors>,
    ) -> OResult<'slot, NormalizedConfigValue> {
        Self::normalize_for_schema(self.schema, value, request, out)
    }

    /// Normalize an owned external value while sharing the caller's execution budget.
    pub fn normalize_with_budget<'slot, 'borrow>(
        &self,
        value: ConfigValue,
        request: ConfigValidationRequest,
        budget: &ExecutionBudget,
        out: Out<'slot, 'borrow, ConfigValueErrors>,
    ) -> OResult<'slot, NormalizedConfigValue> {
        Self::normalize_for_schema_with_budget(self.schema, value, request, budget, out)
    }

    /// Normalize an owned external value and apply complete Schema validation.
    pub fn validate<'slot>(
        &self,
        value: ConfigValue,
        request: ConfigValidationRequest,
        out: Out<'slot, '_, ConfigValueErrors>,
    ) -> OResult<'slot, ValidatedConfigValue> {
        Self::validate_for_schema(self.schema, value, request, out)
    }

    /// Validate an owned external value while sharing the caller's execution budget.
    pub fn validate_with_budget<'slot, 'borrow>(
        &self,
        value: ConfigValue,
        request: ConfigValidationRequest,
        budget: &ExecutionBudget,
        out: Out<'slot, 'borrow, ConfigValueErrors>,
    ) -> OResult<'slot, ValidatedConfigValue> {
        Self::validate_for_schema_with_budget(self.schema, value, request, budget, out)
    }

    /// Fully validate a Schema-complete value without rebuilding its value tree.
    #[doc(hidden)]
    pub fn validate_schema_complete_with_budget<'slot, 'borrow>(
        &self,
        value: ConfigValue,
        request: ConfigValidationRequest,
        budget: &ExecutionBudget,
        out: Out<'slot, 'borrow, ConfigValueErrors>,
    ) -> OResult<'slot, ValidatedConfigValue> {
        Self::validate_complete_for_schema_with_budget(self.schema, value, request, budget, out)
    }

    fn validate_for_schema<'slot>(
        schema: &CompiledSchema,
        value: ConfigValue,
        request: ConfigValidationRequest,
        mut out: Out<'slot, '_, ConfigValueErrors>,
    ) -> OResult<'slot, ValidatedConfigValue> {
        let translator = request.options().translator();
        let mut budget_errors = ErrSlot::new();
        let budget = match budget_errors
            .try_call(|budget_out| ExecutionBudget::new(*request.options().execution(), budget_out))
        {
            Ok(budget) => budget,
            Err(caught) => {
                return Err(out.fail(ConfigValueErrors::from_errors(
                    ConfigParseErrors::emergency(
                        BudgetDiagnostic::from_error(caught.take(), translator),
                        None,
                        translator,
                    ),
                )));
            }
        };
        Self::validate_for_schema_with_budget(schema, value, request, &budget, out.reborrow())
    }

    fn normalize_for_schema<'slot>(
        schema: &CompiledSchema,
        value: ConfigValue,
        request: ConfigValidationRequest,
        mut out: Out<'slot, '_, ConfigValueErrors>,
    ) -> OResult<'slot, NormalizedConfigValue> {
        let translator = request.options().translator();
        let mut budget_errors = ErrSlot::new();
        let budget = match budget_errors
            .try_call(|budget_out| ExecutionBudget::new(*request.options().execution(), budget_out))
        {
            Ok(budget) => budget,
            Err(caught) => {
                return Err(out.fail(ConfigValueErrors::from_errors(
                    ConfigParseErrors::emergency(
                        BudgetDiagnostic::from_error(caught.take(), translator),
                        None,
                        translator,
                    ),
                )));
            }
        };
        Self::normalize_for_schema_with_budget(schema, value, request, &budget, out.reborrow())
    }

    fn normalize_for_schema_with_budget<'slot, 'borrow>(
        schema: &CompiledSchema,
        value: ConfigValue,
        request: ConfigValidationRequest,
        budget: &ExecutionBudget,
        out: Out<'slot, 'borrow, ConfigValueErrors>,
    ) -> OResult<'slot, NormalizedConfigValue> {
        let translator = request.options().translator();
        if budget.limits() != *request.options().execution() {
            return Err(out.fail(ConfigValueErrors::from_errors(
                ConfigParseErrors::emergency(
                    DiagnosticFactory::value_internal(
                        CONFIG_VALUE_NORMALIZATION_BUDGET_MISMATCH,
                        None,
                        None,
                        None,
                        translator,
                    ),
                    None,
                    translator,
                ),
            )));
        }
        let mut operation_errors = ErrSlot::new();
        let mut diagnostics = match operation_errors.try_call(|diagnostic_out| {
            DiagnosticBuffer::new_budgeted(
                schema,
                request.options().config().max_diagnostics(),
                budget,
                translator,
                diagnostic_out,
            )
        }) {
            Ok(diagnostics) => diagnostics,
            Err(caught) => {
                return Err(out.fail(ConfigValueErrors::from_errors(
                    ConfigParseErrors::emergency(caught.take(), None, translator),
                )));
            }
        };
        let mut nodes = 0;
        let result = operation_errors.try_call(|diagnostic_out| {
            ValueNormalizer::copy(
                NormalizeRequest {
                    translator: request.options().translator(),
                    expected: schema.entry(),
                    value: value.root(),
                    depth: 0,
                    primary: None,
                    text_limit: request.options().config().max_constructed_text_bytes(),
                    schema_default: false,
                    allow_unvalidated_environment_sources: false,
                    existing_nodes_precounted: false,
                    missing_value_source: ValueSource::RustValue,
                },
                &mut nodes,
                request.options().config().values(),
                budget,
                diagnostic_out,
            )
        });
        match result {
            Ok(value) => Ok(NormalizedConfigValue::from_config_value(
                value.into_config_value(),
            )),
            Err(caught) => {
                diagnostics.push(caught.take());
                let errors = ConfigParseErrors::finish(diagnostics, schema, None, None);
                let excerpts =
                    errors.schema_excerpts(schema, *request.options().excerpts(), budget);
                Err(out.fail(ConfigValueErrors::from_errors(
                    errors.with_excerpts(excerpts),
                )))
            }
        }
    }

    fn validate_for_schema_with_budget<'slot, 'borrow>(
        schema: &CompiledSchema,
        value: ConfigValue,
        request: ConfigValidationRequest,
        budget: &ExecutionBudget,
        out: Out<'slot, 'borrow, ConfigValueErrors>,
    ) -> OResult<'slot, ValidatedConfigValue> {
        let translator = request.options().translator();
        if budget.limits() != *request.options().execution() {
            return Err(out.fail(ConfigValueErrors::from_errors(
                ConfigParseErrors::emergency(
                    DiagnosticFactory::internal(
                        CONFIG_VALUE_BUDGET_MISMATCH,
                        None,
                        None,
                        None,
                        translator,
                    ),
                    None,
                    translator,
                ),
            )));
        }
        let mut validation_errors = ErrSlot::new();
        match validation_errors.try_call(|validation_out| {
            ValueValidation::value(
                ValueValidationRequest {
                    schema,
                    external_value: Some(&value),
                    schema_complete_value: None,
                    options: request.options(),
                    budget,
                },
                validation_out,
            )
        }) {
            Ok(value) => Ok(ValidatedConfigValue::from_config_value(schema, value)),
            Err(caught) => {
                let errors = caught.take();
                let excerpts =
                    errors.schema_excerpts(schema, *request.options().excerpts(), budget);
                Err(out.fail(ConfigValueErrors::from_errors(
                    errors.with_excerpts(excerpts),
                )))
            }
        }
    }

    /// Internal binding seam for schema-complete values.
    #[doc(hidden)]
    fn validate_complete_for_schema_with_budget<'slot, 'borrow>(
        schema: &CompiledSchema,
        value: ConfigValue,
        request: ConfigValidationRequest,
        budget: &ExecutionBudget,
        out: Out<'slot, 'borrow, ConfigValueErrors>,
    ) -> OResult<'slot, ValidatedConfigValue> {
        let translator = request.options().translator();
        if budget.limits() != *request.options().execution() {
            return Err(out.fail(ConfigValueErrors::from_errors(
                ConfigParseErrors::emergency(
                    DiagnosticFactory::internal(
                        CONFIG_VALUE_BUDGET_MISMATCH,
                        None,
                        None,
                        None,
                        translator,
                    ),
                    None,
                    translator,
                ),
            )));
        }
        let mut validation_errors = ErrSlot::new();
        match validation_errors.try_call(|validation_out| {
            ValueValidation::value(
                ValueValidationRequest {
                    schema,
                    external_value: None,
                    schema_complete_value: Some(value),
                    options: request.options(),
                    budget,
                },
                validation_out,
            )
        }) {
            Ok(value) => Ok(ValidatedConfigValue::from_config_value(schema, value)),
            Err(caught) => {
                let errors = caught.take();
                let excerpts =
                    errors.schema_excerpts(schema, *request.options().excerpts(), budget);
                Err(out.fail(ConfigValueErrors::from_errors(
                    errors.with_excerpts(excerpts),
                )))
            }
        }
    }
}
