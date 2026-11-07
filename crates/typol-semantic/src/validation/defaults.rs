use crate::{
    AllItemRequest, AllItemValue, BuildError, CompiledSchema, ConstraintList, EffectiveRules,
    EvalError, EvalFailure, FieldId, RegexExecutionError, RegexExecutionFailure, RegexExecutor,
    RegexMatchOptions, StoredPayloadInfo, StoredRuleView, StoredTypeKind, SumBoundsRequest, TypeId,
    TypedExpression, ValueRules, VariantId, engine::Evaluator,
};
use core::cell::RefCell;
use opeo::{ErrSlot, OResult, Out, ResultOutExt};
use typol_core::{
    ChargedVec, ConfigValue, DiagnosticContext, ExecutionBudget, RegexExecutionBudget, SourceSpan,
    ValueEntries, ValueKind, ValuePayloadView, ValueView, diagnostic_context,
};
use typol_model::BuiltinType;
use typol_policy::{RegexLimits, ValueLimits};
use typol_regex::{RegexMatchError, RegexScratchError};

static DEFAULT_VALIDATION_INTERNAL: DiagnosticContext = diagnostic_context!(
    "typol-semantic",
    "default-value-validation",
    "default-shape-or-field-mismatch",
);
static DEFAULT_RULE_OWNER_INTERNAL: DiagnosticContext = diagnostic_context!(
    "typol-semantic",
    "default-value-validation",
    "constraint-list-owner-mismatch",
);

pub(crate) struct DefaultValidationInput<'a> {
    pub(crate) schema: &'a CompiledSchema,
    pub(crate) value: &'a ConfigValue,
    pub(crate) ty: TypeId,
    pub(crate) constraints: ConstraintList<'a>,
    pub(crate) execution: &'a ExecutionBudget,
    pub(crate) regex_execution: &'a RegexExecutionBudget,
    pub(crate) value_limits: &'a ValueLimits,
    pub(crate) regex_limits: &'a RegexLimits,
}

struct PendingValue<'a> {
    value: ValueView<'a>,
    ty: TypeId,
    depth: usize,
    inherited: Option<ConstraintList<'a>>,
    constraints: Option<ConstraintList<'a>>,
}

struct PendingValueRequest<'a> {
    value: ValueView<'a>,
    ty: TypeId,
    depth: usize,
    inherited: Option<ConstraintList<'a>>,
    constraints: Option<ConstraintList<'a>>,
}

struct PendingFieldsRequest<'value, 'pending> {
    pending: &'pending mut ChargedVec<PendingValue<'value>>,
    schema: &'value CompiledSchema,
    fields: &'value [FieldId],
    values: ValueEntries<'value>,
    depth: usize,
    execution: &'value ExecutionBudget,
}

struct RuleValidationRequest<'value, 'schema> {
    value: ValueView<'value>,
    ty: TypeId,
    inherited: ConstraintList<'schema>,
    constraints: ConstraintList<'schema>,
}

struct AllValidationRequest<'value, 'schema> {
    value: ValueView<'value>,
    ty: TypeId,
    expression: &'schema TypedExpression,
    span: SourceSpan,
}

struct RuleExecution<'a> {
    schema: &'a CompiledSchema,
    execution: &'a ExecutionBudget,
    regex_execution: &'a RegexExecutionBudget,
    evaluator: Evaluator<'a>,
    regex_limits: &'a RegexLimits,
    regex_executor: RegexExecutor,
    evaluation_errors: RefCell<ErrSlot<EvalError>>,
    regex_errors: ErrSlot<RegexExecutionError>,
}

pub(crate) struct DefaultValidation;

impl DefaultValidation {
    pub(crate) fn validate<'slot>(
        input: DefaultValidationInput<'_>,
        mut out: Out<'slot, '_, BuildError>,
    ) -> OResult<'slot, ()> {
        let DefaultValidationInput {
            schema,
            value,
            ty,
            constraints,
            execution,
            regex_execution,
            value_limits,
            regex_limits,
        } = input;
        let evaluator = Evaluator::new(schema, value_limits, regex_limits, Some(execution));
        let mut rules = RuleExecution {
            schema,
            execution,
            regex_execution,
            evaluator,
            regex_limits,
            regex_executor: RegexExecutor::new(),
            evaluation_errors: RefCell::new(ErrSlot::new()),
            regex_errors: ErrSlot::new(),
        };
        let mut pending = ChargedVec::new(Some(execution), out.reborrow())?;
        pending.try_reserve(1, out.reborrow())?;
        pending.try_push(
            PendingValue {
                value: value.root(),
                ty,
                depth: 0,
                inherited: None,
                constraints: Some(constraints),
            },
            out.reborrow(),
        )?;
        let mut value_nodes = 0usize;
        while let Some(current) = pending.pop() {
            execution.consume(1, out.reborrow())?;
            value_nodes = value_nodes
                .checked_add(1)
                .ok_or(BuildError::Resource)
                .store_err_into(out.reborrow())?;
            if !value_limits.allows_depth(current.depth)
                || value_nodes > value_limits.max_value_nodes()
            {
                return Err(out.fail(BuildError::Resource));
            }
            if let ValueKind::String(text) = current.value.as_ref() {
                if text.len() > value_limits.max_string_bytes() {
                    return Err(out.fail(BuildError::Resource));
                }
            }
            let type_info = schema
                .type_info(current.ty)
                .store_err_into(out.reborrow())?;
            let info = type_info
                .ok_or(BuildError::internal(DEFAULT_VALIDATION_INTERNAL))
                .store_err_into(out.reborrow())?;
            let inherited = current.inherited.unwrap_or_else(|| info.constraints());
            let constraints = current
                .constraints
                .unwrap_or_else(|| ConstraintList::new(schema, None));
            rules.validate_rules(
                RuleValidationRequest {
                    value: current.value,
                    ty: current.ty,
                    inherited,
                    constraints,
                },
                out.reborrow(),
            )?;
            let next_depth = current
                .depth
                .checked_add(1)
                .ok_or(BuildError::Resource)
                .store_err_into(out.reborrow())?;
            match (info.kind(), current.value.as_ref()) {
                (StoredTypeKind::Builtin(BuiltinType::Bool), ValueKind::Bool(_))
                | (StoredTypeKind::Builtin(BuiltinType::String), ValueKind::String(_)) => {}
                (StoredTypeKind::Builtin(BuiltinType::F32), ValueKind::Float32(number))
                    if number.is_finite() => {}
                (StoredTypeKind::Builtin(BuiltinType::F64), ValueKind::Float64(number))
                    if number.is_finite() => {}
                (StoredTypeKind::Builtin(builtin), ValueKind::Signed(number))
                    if Self::signed_fits(builtin, number) => {}
                (StoredTypeKind::Builtin(builtin), ValueKind::Unsigned(number))
                    if Self::unsigned_fits(builtin, number) => {}
                (StoredTypeKind::Option { inner }, ValueKind::Optional(value)) => {
                    if let Some(value) = value {
                        Self::push_value(
                            &mut pending,
                            PendingValueRequest {
                                value,
                                ty: inner,
                                depth: next_depth,
                                inherited: None,
                                constraints: None,
                            },
                            out.reborrow(),
                        )?;
                    }
                }
                (StoredTypeKind::Array { element, length }, ValueKind::Array(values)) => {
                    if length.is_some_and(|expected| values.len() != expected as usize) {
                        return Err(out.fail(BuildError::internal(DEFAULT_VALIDATION_INTERNAL)));
                    }
                    for entry in values {
                        if entry.name().is_some() {
                            return Err(out.fail(BuildError::internal(DEFAULT_VALIDATION_INTERNAL)));
                        }
                        Self::push_value(
                            &mut pending,
                            PendingValueRequest {
                                value: entry.value(),
                                ty: element,
                                depth: next_depth,
                                inherited: None,
                                constraints: None,
                            },
                            out.reborrow(),
                        )?;
                    }
                }
                (StoredTypeKind::Map { value: inner }, ValueKind::Map(values)) => {
                    for entry in values {
                        if entry.name().is_none() {
                            return Err(out.fail(BuildError::internal(DEFAULT_VALIDATION_INTERNAL)));
                        }
                        Self::push_value(
                            &mut pending,
                            PendingValueRequest {
                                value: entry.value(),
                                ty: inner,
                                depth: next_depth,
                                inherited: None,
                                constraints: None,
                            },
                            out.reborrow(),
                        )?;
                    }
                }
                (StoredTypeKind::Struct { fields }, ValueKind::Structure(values)) => {
                    Self::push_fields(
                        PendingFieldsRequest {
                            schema,
                            fields,
                            values,
                            depth: next_depth,
                            pending: &mut pending,
                            execution,
                        },
                        out.reborrow(),
                    )?;
                }
                (
                    StoredTypeKind::Enum { variants },
                    ValueKind::Enumeration { variant, payload },
                ) => {
                    let variant_id =
                        Self::find_variant(schema, variants, variant, execution, out.reborrow())?;
                    let variant_info = schema
                        .variant_info(variant_id)
                        .store_err_into(out.reborrow())?;
                    let variant_info = variant_info
                        .ok_or(BuildError::internal(DEFAULT_VALIDATION_INTERNAL))
                        .store_err_into(out.reborrow())?;
                    match (variant_info.payload(), payload) {
                        (StoredPayloadInfo::Unit, ValuePayloadView::Unit) => {}
                        (StoredPayloadInfo::Single { ty }, ValuePayloadView::Single(value)) => {
                            Self::push_value(
                                &mut pending,
                                PendingValueRequest {
                                    value,
                                    ty,
                                    depth: next_depth,
                                    inherited: None,
                                    constraints: Some(variant_info.constraints()),
                                },
                                out.reborrow(),
                            )?;
                        }
                        (
                            StoredPayloadInfo::Structure { fields },
                            ValuePayloadView::Structure(values),
                        ) => {
                            Self::push_fields(
                                PendingFieldsRequest {
                                    schema,
                                    fields,
                                    values,
                                    depth: next_depth,
                                    pending: &mut pending,
                                    execution,
                                },
                                out.reborrow(),
                            )?;
                        }
                        _ => {
                            return Err(out.fail(BuildError::internal(DEFAULT_VALIDATION_INTERNAL)));
                        }
                    }
                }
                _ => {
                    return Err(out.fail(BuildError::internal(DEFAULT_VALIDATION_INTERNAL)));
                }
            }
        }
        Ok(())
    }

    fn push_value<'a, 'slot>(
        pending: &mut ChargedVec<PendingValue<'a>>,
        request: PendingValueRequest<'a>,
        out: Out<'slot, '_, BuildError>,
    ) -> OResult<'slot, ()> {
        pending.try_push(
            PendingValue {
                value: request.value,
                ty: request.ty,
                depth: request.depth,
                inherited: request.inherited,
                constraints: request.constraints,
            },
            out,
        )
    }

    fn push_fields<'a, 'pending, 'slot>(
        request: PendingFieldsRequest<'a, 'pending>,
        mut out: Out<'slot, '_, BuildError>,
    ) -> OResult<'slot, ()> {
        let PendingFieldsRequest {
            schema,
            fields,
            values,
            depth,
            pending,
            execution,
        } = request;
        if fields.len() != values.len() {
            return Err(out.fail(BuildError::internal(DEFAULT_VALIDATION_INTERNAL)));
        }
        for (slot, id) in fields.iter().enumerate() {
            execution.consume(1, out.reborrow())?;
            let field_info = schema.field_info(*id).store_err_into(out.reborrow())?;
            let field = field_info
                .ok_or(BuildError::internal(DEFAULT_VALIDATION_INTERNAL))
                .store_err_into(out.reborrow())?;
            let entry = values
                .get(slot)
                .filter(|entry| entry.name() == Some(field.name()))
                .ok_or(BuildError::internal(DEFAULT_VALIDATION_INTERNAL))
                .store_err_into(out.reborrow())?;
            Self::push_value(
                pending,
                PendingValueRequest {
                    value: entry.value(),
                    ty: field.ty(),
                    depth,
                    inherited: None,
                    constraints: Some(field.constraints()),
                },
                out.reborrow(),
            )?;
        }
        Ok(())
    }

    fn find_variant<'slot>(
        schema: &CompiledSchema,
        variants: &[VariantId],
        name: &str,
        execution: &ExecutionBudget,
        mut out: Out<'slot, '_, BuildError>,
    ) -> OResult<'slot, VariantId> {
        for id in variants {
            execution.consume(1, out.reborrow())?;
            let variant_info = schema.variant_info(*id).store_err_into(out.reborrow())?;
            let candidate = variant_info
                .ok_or(BuildError::internal(DEFAULT_VALIDATION_INTERNAL))
                .store_err_into(out.reborrow())?;
            if execution
                .compare_text(candidate.name(), name, out.reborrow())?
                .is_eq()
            {
                return Ok(*id);
            }
        }
        Err(out.fail(BuildError::internal(DEFAULT_VALIDATION_INTERNAL)))
    }

    pub(crate) fn signed_fits(builtin: BuiltinType, number: i128) -> bool {
        match builtin {
            BuiltinType::I8 => i8::try_from(number).is_ok(),
            BuiltinType::I16 => i16::try_from(number).is_ok(),
            BuiltinType::I32 => i32::try_from(number).is_ok(),
            BuiltinType::I64 => i64::try_from(number).is_ok(),
            BuiltinType::I128 => true,
            _ => false,
        }
    }

    pub(crate) fn unsigned_fits(builtin: BuiltinType, number: u128) -> bool {
        match builtin {
            BuiltinType::U8 => u8::try_from(number).is_ok(),
            BuiltinType::U16 => u16::try_from(number).is_ok(),
            BuiltinType::U32 => u32::try_from(number).is_ok(),
            BuiltinType::U64 => u64::try_from(number).is_ok(),
            BuiltinType::U128 => true,
            _ => false,
        }
    }
}

impl RuleExecution<'_> {
    fn validate_rules<'slot>(
        &mut self,
        request: RuleValidationRequest<'_, '_>,
        mut out: Out<'slot, '_, BuildError>,
    ) -> OResult<'slot, ()> {
        let RuleValidationRequest {
            value,
            ty,
            inherited,
            constraints,
        } = request;
        let mut owner_errors = ErrSlot::new();
        let inherited_lists = [inherited];
        let effective = match owner_errors.try_call(|owner_out| {
            EffectiveRules::new(self.schema, &inherited_lists, constraints, owner_out)
        }) {
            Ok(effective) => effective,
            Err(_) => return Err(out.fail(BuildError::internal(DEFAULT_RULE_OWNER_INTERNAL))),
        };
        let rules = effective.collect_budgeted(self.execution, out.reborrow())?;
        for rule in rules {
            let span = rule.span();
            let data = rule.raw_data(out.reborrow())?;
            let valid = match data {
                data @ (StoredRuleView::NumberBounds { .. }
                | StoredRuleView::LengthBounds { .. }) => {
                    let leaf = Self::optional_leaf(value, self.execution, out.reborrow())?;
                    let Some(leaf) = leaf else {
                        continue;
                    };
                    let result = self.evaluate(span, out.reborrow(), |eval_out| {
                        ValueRules::scalar(leaf, data, Some(self.execution), eval_out)
                    })?;
                    result
                        .ok_or(BuildError::internal(DEFAULT_VALIDATION_INTERNAL))
                        .store_err_into(out.reborrow())?
                }
                data @ (StoredRuleView::Host
                | StoredRuleView::Email
                | StoredRuleView::Cidr { .. }
                | StoredRuleView::SizeBounds { .. }) => {
                    let leaf = Self::optional_leaf(value, self.execution, out.reborrow())?;
                    let Some(leaf) = leaf else {
                        continue;
                    };
                    let result = self.evaluate(span, out.reborrow(), |eval_out| {
                        ValueRules::scalar(leaf, data, Some(self.execution), eval_out)
                    })?;
                    result
                        .ok_or(BuildError::internal(DEFAULT_VALIDATION_INTERNAL))
                        .store_err_into(out.reborrow())?
                }
                StoredRuleView::Regex { index } => {
                    self.regex_matches(value, index, span, out.reborrow())?
                }
                StoredRuleView::Unique { by } => {
                    let element = match self
                        .schema
                        .type_info(ty)
                        .store_err_into(out.reborrow())?
                        .ok_or(BuildError::internal(DEFAULT_VALIDATION_INTERNAL))
                        .store_err_into(out.reborrow())?
                        .kind()
                    {
                        StoredTypeKind::Array { element, .. } => element,
                        _ => {
                            return Err(out.fail(BuildError::internal(DEFAULT_VALIDATION_INTERNAL)));
                        }
                    };
                    let duplicates = self.evaluate(span, out.reborrow(), |eval_out| {
                        ValueRules::unique(&self.evaluator, value, element, by, eval_out)
                    })?;
                    let mut valid = true;
                    for duplicate in duplicates {
                        self.execution.consume(1, out.reborrow())?;
                        valid &= !duplicate;
                    }
                    valid
                }
                StoredRuleView::All { expression, .. } => self.validate_all(
                    AllValidationRequest {
                        value,
                        ty,
                        expression,
                        span,
                    },
                    out.reborrow(),
                )?,
                StoredRuleView::SumBounds { min, max } => {
                    let element = match self
                        .schema
                        .type_info(ty)
                        .store_err_into(out.reborrow())?
                        .ok_or(BuildError::internal(DEFAULT_VALIDATION_INTERNAL))
                        .store_err_into(out.reborrow())?
                        .kind()
                    {
                        StoredTypeKind::Array { element, .. } => element,
                        _ => {
                            return Err(out.fail(BuildError::internal(DEFAULT_VALIDATION_INTERNAL)));
                        }
                    };
                    self.evaluate(span, out.reborrow(), |eval_out| {
                        ValueRules::sum_bounds(
                            &self.evaluator,
                            SumBoundsRequest {
                                element,
                                values: value,
                                min,
                                max,
                            },
                            eval_out,
                        )
                    })?
                }
            };
            if !valid {
                return Err(out.fail(BuildError::DefaultConstraintAt { source_span: span }));
            }
        }
        Ok(())
    }

    fn optional_leaf<'a, 'slot>(
        mut value: ValueView<'a>,
        execution: &ExecutionBudget,
        mut out: Out<'slot, '_, BuildError>,
    ) -> OResult<'slot, Option<ValueView<'a>>> {
        loop {
            let ValueKind::Optional(inner) = value.as_ref() else {
                return Ok(Some(value));
            };
            execution.consume(1, out.reborrow())?;
            let Some(inner) = inner else {
                return Ok(None);
            };
            value = inner;
        }
    }

    fn regex_matches<'slot>(
        &mut self,
        value: ValueView<'_>,
        index: usize,
        span: SourceSpan,
        mut out: Out<'slot, '_, BuildError>,
    ) -> OResult<'slot, bool> {
        let leaf = Self::optional_leaf(value, self.execution, out.reborrow())?;
        let Some(leaf) = leaf else {
            return Ok(true);
        };
        let ValueKind::String(text) = leaf.as_ref() else {
            return Err(out.fail(BuildError::internal(DEFAULT_VALIDATION_INTERNAL)));
        };
        self.execution.consume(1, out.reborrow())?;
        let regex = self
            .schema
            .records()
            .regexes
            .constraints()
            .get(index)
            .ok_or(BuildError::internal(DEFAULT_VALIDATION_INTERNAL))
            .store_err_into(out.reborrow())?
            .regex();
        let regex_limits = self.regex_limits;
        let regex_execution = self.regex_execution;
        let result = self.regex_errors.try_call(|regex_out| {
            self.regex_executor.is_match_budgeted(
                regex,
                RegexMatchOptions {
                    value: Some(text),
                    limits: regex_limits,
                    budget: regex_execution,
                },
                regex_out,
            )
        });
        match result {
            Ok(matched) => Ok(matched),
            Err(caught) => Err(out.fail(Self::regex_error(caught.take(), span))),
        }
    }

    fn validate_all<'slot>(
        &mut self,
        request: AllValidationRequest<'_, '_>,
        mut out: Out<'slot, '_, BuildError>,
    ) -> OResult<'slot, bool> {
        let AllValidationRequest {
            value,
            ty,
            expression,
            span,
        } = request;
        let info = self
            .schema
            .type_info(ty)
            .store_err_into(out.reborrow())?
            .ok_or(BuildError::internal(DEFAULT_VALIDATION_INTERNAL))
            .store_err_into(out.reborrow())?;
        let (element, is_map) = match info.kind() {
            StoredTypeKind::Array { element, .. } => (element, false),
            StoredTypeKind::Map { value } => (value, true),
            _ => return Err(out.fail(BuildError::internal(DEFAULT_VALIDATION_INTERNAL))),
        };
        let entries = match value.as_ref() {
            ValueKind::Array(entries) if !is_map => entries,
            ValueKind::Map(entries) if is_map => entries,
            _ => return Err(out.fail(BuildError::internal(DEFAULT_VALIDATION_INTERNAL))),
        };
        let mut valid = true;
        for entry in entries {
            let item = match (is_map, entry.name()) {
                (false, None) => AllItemValue::ArrayItem(entry.value()),
                (true, Some(key)) => AllItemValue::MapEntry {
                    key,
                    value: entry.value(),
                },
                _ => return Err(out.fail(BuildError::internal(DEFAULT_VALIDATION_INTERNAL))),
            };
            let matched = self.evaluate(span, out.reborrow(), |eval_out| {
                ValueRules::all_item(
                    &self.evaluator,
                    AllItemRequest {
                        expression,
                        element,
                        item,
                    },
                    eval_out,
                )
            })?;
            valid &= matched;
        }
        Ok(valid)
    }

    fn evaluate<'slot, T>(
        &self,
        span: SourceSpan,
        out: Out<'slot, '_, BuildError>,
        operation: impl for<'call> FnOnce(Out<'call, 'call, EvalError>) -> OResult<'call, T>,
    ) -> OResult<'slot, T> {
        match self.evaluation_errors.borrow_mut().try_call(operation) {
            Ok(value) => Ok(value),
            Err(caught) => {
                let error = caught.take();
                Err(out.fail(Self::evaluation_error(error, span)))
            }
        }
    }

    fn evaluation_error(error: EvalError, span: SourceSpan) -> BuildError {
        match error.kind() {
            EvalFailure::InternalAt(context) => BuildError::InternalAt {
                context,
                source_span: Some(error.source_span().unwrap_or(span)),
            },
            EvalFailure::SchemaAccess(schema_error) => BuildError::SchemaAccessAt {
                error: schema_error,
                source_span: error.source_span().unwrap_or(span),
            },
            EvalFailure::Resource => BuildError::Resource,
            EvalFailure::Arithmetic => BuildError::DefaultEvaluationAt {
                failure: EvalFailure::Arithmetic,
                source_span: error.source_span().unwrap_or(span),
            },
            EvalFailure::Allocation => BuildError::Allocation,
            EvalFailure::Capacity => BuildError::Capacity,
            EvalFailure::Internal => BuildError::InternalAt {
                context: DEFAULT_VALIDATION_INTERNAL,
                source_span: Some(error.source_span().unwrap_or(span)),
            },
        }
    }

    fn regex_error(error: RegexExecutionError, span: SourceSpan) -> BuildError {
        match error {
            RegexExecutionError::Scratch(error) => Self::scratch_error(error, span),
            RegexExecutionError::Match(error) => Self::match_error(error, span),
            RegexExecutionError::Budget(error) => BuildError::from(error).with_source_span(span),
            RegexExecutionError::InternalAt { context, .. } => BuildError::InternalAt {
                context,
                source_span: Some(span),
            },
            RegexExecutionError::Located { error, .. } => match error {
                RegexExecutionFailure::Scratch(error) => Self::scratch_error(error, span),
                RegexExecutionFailure::Match(error) => Self::match_error(error, span),
                RegexExecutionFailure::Budget(error) => {
                    BuildError::from(error).with_source_span(span)
                }
                RegexExecutionFailure::Internal(Some(context)) => BuildError::InternalAt {
                    context,
                    source_span: Some(span),
                },
                RegexExecutionFailure::Internal(None) => BuildError::InternalAt {
                    context: DEFAULT_VALIDATION_INTERNAL,
                    source_span: Some(span),
                },
            },
            RegexExecutionError::Internal => BuildError::InternalAt {
                context: DEFAULT_VALIDATION_INTERNAL,
                source_span: Some(span),
            },
        }
    }

    fn scratch_error(error: RegexScratchError, span: SourceSpan) -> BuildError {
        match error {
            RegexScratchError::ResourceLimitExceeded(_) => BuildError::Resource,
            RegexScratchError::AllocationFailed => BuildError::Allocation,
            RegexScratchError::CapacityExceeded => BuildError::Capacity,
            RegexScratchError::InternalInvariantFailure(context) => BuildError::InternalAt {
                context: context.diagnostic_context(),
                source_span: Some(span),
            },
        }
    }

    fn match_error(error: RegexMatchError, span: SourceSpan) -> BuildError {
        match error {
            RegexMatchError::ResourceLimitExceeded(_) => BuildError::Resource,
            RegexMatchError::AllocationFailed => BuildError::Allocation,
            RegexMatchError::CapacityExceeded => BuildError::Capacity,
            RegexMatchError::InternalInvariantFailure(context) => BuildError::InternalAt {
                context: context.diagnostic_context(),
                source_span: Some(span),
            },
        }
    }
}
