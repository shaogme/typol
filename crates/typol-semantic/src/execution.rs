use crate::{
    AllItemRequest, ComparisonOptions, CompiledSchema, ConstraintData, EvalError, EvalKind,
    ExpressionView, FieldList, StoredRuleView, SumBoundsRequest, TypeView, ValueComparison,
    ValueRules,
    engine::{Bindings as RawBindings, EvalValue as RawValue, Evaluator as RawEvaluator},
};
use core::{cmp::Ordering, ptr};
use opeo::{OResult, Out, ResultOutExt};
use typol_core::{
    ChargedVec, ConfigValue, DiagnosticContext, ExecutionBudget, ValueView, diagnostic_context,
};
use typol_model::BuiltinType;
use typol_policy::{RegexLimits, ValueLimits};

static SCHEMA_EVALUATION_INTERNAL: DiagnosticContext = diagnostic_context!(
    "typol-semantic",
    "schema-evaluation",
    "cross-schema-expression-or-view",
);

#[derive(Clone, Copy)]
pub struct SchemaValue<'a> {
    kind: EvalKind<'a>,
    ty: TypeView<'a>,
    host: bool,
}

impl<'a> SchemaValue<'a> {
    pub const fn new(kind: EvalKind<'a>, ty: TypeView<'a>, host: bool) -> Self {
        Self { kind, ty, host }
    }

    pub const fn kind(self) -> EvalKind<'a> {
        self.kind
    }

    pub const fn ty(self) -> TypeView<'a> {
        self.ty
    }

    pub const fn host(self) -> bool {
        self.host
    }

    pub fn config(value: ValueView<'a>, ty: TypeView<'a>, host: bool) -> Self {
        Self {
            kind: RawValue::config(value, ty.id(), host).kind(),
            ty,
            host,
        }
    }

    pub fn from_config(value: &'a ConfigValue, ty: TypeView<'a>, host: bool) -> Self {
        Self::config(value.root(), ty, host)
    }

    fn raw(self) -> RawValue<'a> {
        RawValue::new(self.kind, self.ty.id(), self.host)
    }

    pub fn boolean<'slot, 'borrow>(
        self,
        out: Out<'slot, 'borrow, EvalError>,
    ) -> OResult<'slot, bool> {
        self.raw().boolean(out)
    }

    pub fn string<'slot, 'borrow>(
        self,
        out: Out<'slot, 'borrow, EvalError>,
    ) -> OResult<'slot, &'a str> {
        self.raw().string(out)
    }

    pub fn value<'slot, 'borrow>(
        self,
        out: Out<'slot, 'borrow, EvalError>,
    ) -> OResult<'slot, ValueView<'a>> {
        self.raw().value(out)
    }

    pub fn fields<'slot, 'borrow>(
        self,
        out: Out<'slot, 'borrow, EvalError>,
    ) -> OResult<'slot, ValueView<'a>> {
        self.raw().fields(out)
    }
}

#[derive(Clone, Copy, Default)]
pub struct SchemaBindings<'a> {
    pub self_value: Option<SchemaValue<'a>>,
    pub root: Option<&'a ConfigValue>,
    pub item: Option<SchemaValue<'a>>,
    pub key: Option<&'a str>,
    pub value: Option<SchemaValue<'a>>,
}

/// Evaluates expressions through an owner view of the frozen model; the execution service is exported from the crate root.
pub struct SchemaEvaluator<'a> {
    schema: &'a CompiledSchema,
    limits: &'a ValueLimits,
    regex_limits: &'a RegexLimits,
    budget: Option<&'a ExecutionBudget>,
}

impl<'model> SchemaEvaluator<'model> {
    pub const fn new(
        schema: &'model CompiledSchema,
        limits: &'model ValueLimits,
        regex_limits: &'model RegexLimits,
        budget: Option<&'model ExecutionBudget>,
    ) -> Self {
        Self {
            schema,
            limits,
            regex_limits,
            budget,
        }
    }

    pub const fn budget(&self) -> Option<&ExecutionBudget> {
        self.budget
    }

    fn raw(&self) -> RawEvaluator<'_> {
        RawEvaluator::new(self.schema, self.limits, self.regex_limits, self.budget)
    }

    fn check(&self, ty: TypeView<'_>) -> Result<(), EvalError> {
        if ptr::eq(self.schema, ty.schema()) {
            Ok(())
        } else {
            Err(EvalError::internal(SCHEMA_EVALUATION_INTERNAL))
        }
    }

    fn value<'a>(&self, value: SchemaValue<'a>) -> Result<RawValue<'a>, EvalError> {
        self.check(value.ty)?;
        Ok(value.raw())
    }

    pub fn host<'slot>(
        &self,
        ty: TypeView<'_>,
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, bool> {
        self.check(ty).store_err_into(out.reborrow())?;
        self.raw().host(ty.id(), out.reborrow())
    }

    pub fn builtin<'slot>(
        &self,
        ty: TypeView<'_>,
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, BuiltinType> {
        self.check(ty).store_err_into(out.reborrow())?;
        self.raw().builtin(ty.id(), out.reborrow())
    }

    pub fn checked<'a, 'slot>(
        &self,
        value: SchemaValue<'a>,
        allow_magnitude: bool,
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, SchemaValue<'a>> {
        let raw_value = self.value(value).store_err_into(out.reborrow())?;
        self.raw()
            .checked(raw_value, allow_magnitude, out.reborrow())?;
        Ok(value)
    }

    pub fn evaluate<'a, 'slot>(
        &self,
        expression: ExpressionView<'a>,
        bindings: SchemaBindings<'a>,
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, SchemaValue<'a>> {
        if !ptr::eq(self.schema, expression.schema()) {
            return Err(out.fail(
                EvalError::internal(SCHEMA_EVALUATION_INTERNAL).with_source_span(expression.span()),
            ));
        }
        let self_value = match bindings.self_value {
            Some(value) => Some(
                self.value(value)
                    .map_err(|error| error.with_source_span(expression.span()))
                    .store_err_into(out.reborrow())?,
            ),
            None => None,
        };
        let item = match bindings.item {
            Some(value) => Some(
                self.value(value)
                    .map_err(|error| error.with_source_span(expression.span()))
                    .store_err_into(out.reborrow())?,
            ),
            None => None,
        };
        let value = match bindings.value {
            Some(value) => Some(
                self.value(value)
                    .map_err(|error| error.with_source_span(expression.span()))
                    .store_err_into(out.reborrow())?,
            ),
            None => None,
        };
        let raw = RawBindings {
            self_value,
            root: bindings.root,
            item,
            key: bindings.key,
            value,
        };
        let result = self.raw().evaluate(expression.raw(), raw, out.reborrow())?;
        Ok(SchemaValue {
            kind: result.kind(),
            ty: expression.schema().view(result.ty()),
            host: result.host(),
        })
    }
}

pub struct SchemaComparison;

impl SchemaComparison {
    pub fn compare<'slot>(
        evaluator: &SchemaEvaluator<'_>,
        left: SchemaValue<'_>,
        right: SchemaValue<'_>,
        options: ComparisonOptions,
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, Ordering> {
        ValueComparison::compare(
            &evaluator.raw(),
            evaluator.value(left).store_err_into(out.reborrow())?,
            evaluator.value(right).store_err_into(out.reborrow())?,
            options,
            out,
        )
    }
}

pub struct SchemaRules;

impl SchemaRules {
    pub fn scalar<'slot, 'borrow>(
        value: ValueView<'_>,
        rule: ConstraintData<'_>,
        budget: Option<&ExecutionBudget>,
        out: Out<'slot, 'borrow, EvalError>,
    ) -> OResult<'slot, Option<bool>> {
        let raw = match rule {
            ConstraintData::NumberBounds { min, max } => StoredRuleView::NumberBounds { min, max },
            ConstraintData::LengthBounds { min, max } => StoredRuleView::LengthBounds { min, max },
            ConstraintData::Host => StoredRuleView::Host,
            ConstraintData::Email => StoredRuleView::Email,
            ConstraintData::Cidr { v4_only } => StoredRuleView::Cidr { v4_only },
            ConstraintData::SizeBounds { min, max } => StoredRuleView::SizeBounds { min, max },
            _ => return Ok(None),
        };
        ValueRules::scalar(value, raw, budget, out)
    }

    pub fn sum_bounds<'slot>(
        evaluator: &SchemaEvaluator<'_>,
        request: SumBoundsRequest<'_, TypeView<'_>>,
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, bool> {
        evaluator
            .check(request.element)
            .store_err_into(out.reborrow())?;
        ValueRules::sum_bounds(
            &evaluator.raw(),
            SumBoundsRequest {
                element: request.element.id(),
                values: request.values,
                min: request.min,
                max: request.max,
            },
            out,
        )
    }

    pub fn unique<'slot>(
        evaluator: &SchemaEvaluator<'_>,
        values: ValueView<'_>,
        element: TypeView<'_>,
        by: Option<FieldList<'_>>,
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, ChargedVec<bool>> {
        evaluator.check(element).store_err_into(out.reborrow())?;
        if by.is_some_and(|path| !ptr::eq(path.schema(), evaluator.schema)) {
            return Err(out.fail(EvalError::internal(SCHEMA_EVALUATION_INTERNAL)));
        }
        ValueRules::unique(
            &evaluator.raw(),
            values,
            element.id(),
            by.map(|v| v.ids()),
            out,
        )
    }

    pub fn all_item<'slot>(
        evaluator: &SchemaEvaluator<'_>,
        request: AllItemRequest<'_, ExpressionView<'_>, TypeView<'_>>,
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, bool> {
        let AllItemRequest {
            expression,
            element,
            item,
        } = request;
        evaluator.check(element).store_err_into(out.reborrow())?;
        if !ptr::eq(expression.schema(), evaluator.schema) {
            return Err(out.fail(EvalError::internal(SCHEMA_EVALUATION_INTERNAL)));
        }
        ValueRules::all_item(
            &evaluator.raw(),
            AllItemRequest {
                expression: expression.raw(),
                element: element.id(),
                item,
            },
            out,
        )
    }

    pub fn fingerprint<'slot>(
        evaluator: &SchemaEvaluator<'_>,
        value: SchemaValue<'_>,
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, (u64, u64)> {
        let raw = evaluator.value(value).store_err_into(out.reborrow())?;
        ValueRules::fingerprint(&evaluator.raw(), raw, out)
    }
}
