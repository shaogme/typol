mod comparison;
mod copy;
mod error;
mod expression;
mod model;
mod numeric;
mod regex;
mod rules;
mod values;

use crate::{
    ExpressionLiteral, ExpressionMethodInfo, FieldId, StoredBindingInfo, StoredExpressionArgument,
    StoredExpressionKind, StoredTypeKind, TypeId, TypedExpression, VariantId,
};
use core::{cell::RefCell, cmp::Ordering};
use opeo::{ErrSlot, OResult, Out, ResultOutExt};
use typol_core::{DiagnosticContext, ExecutionBudget, ValueKind, diagnostic_context};
use typol_model::{BinaryOperator, BuiltinType, UnaryOperator};
use typol_policy::{RegexLimits, ValueLimits};

pub use comparison::{ComparisonOptions, ValueComparison};
pub use copy::{CopyError, CopyOptions, CopyRequest, ValueCopy};
pub use error::{EvalError, EvalFailure};
use expression::ExpressionExecutor;
pub use model::{ExecutionField, ExecutionModel, ExecutionType, ExecutionVariant};
use model::{FinishInputs, Local, MethodCall};
pub use numeric::Numeric;
pub use regex::{RegexExecutionError, RegexExecutionFailure, RegexExecutor, RegexMatchOptions};
pub use rules::{AllItemRequest, AllItemValue, SumBoundsRequest, ValueRules};
pub use values::{Bindings, EvalKind, EvalValue};

pub(super) static EVALUATOR_INTERNAL: DiagnosticContext = diagnostic_context!(
    "typol-semantic",
    "expression-evaluation",
    "evaluator-invariant-failed",
);

struct BindingInputs<'a, 'locals> {
    binding: StoredBindingInfo,
    ty: TypeId,
    bindings: Bindings<'a>,
    locals: &'locals [Local<'a>],
}

pub struct Evaluator<'a> {
    schema: &'a dyn ExecutionModel,
    limits: &'a ValueLimits,
    regex_limits: &'a RegexLimits,
    budget: Option<&'a ExecutionBudget>,
    evaluation_errors: RefCell<ErrSlot<EvalError>>,
}

impl<'model> Evaluator<'model> {
    pub const fn new(
        schema: &'model dyn ExecutionModel,
        limits: &'model ValueLimits,
        regex_limits: &'model RegexLimits,
        budget: Option<&'model ExecutionBudget>,
    ) -> Self {
        Self {
            schema,
            limits,
            regex_limits,
            budget,
            evaluation_errors: RefCell::new(ErrSlot::new()),
        }
    }

    pub fn schema(&self) -> &dyn ExecutionModel {
        self.schema
    }

    pub const fn limits(&self) -> &ValueLimits {
        self.limits
    }

    pub const fn regex_limits(&self) -> &RegexLimits {
        self.regex_limits
    }

    pub const fn budget(&self) -> Option<&ExecutionBudget> {
        self.budget
    }

    pub(crate) fn variant_id<'slot>(
        &self,
        ty: TypeId,
        name: &str,
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, VariantId> {
        if let Some(budget) = self.budget {
            return self
                .schema
                .variant_named(ty, name, budget, out.reborrow())?
                .ok_or(EvalError::internal(EVALUATOR_INTERNAL))
                .store_err_into(out.reborrow());
        }
        let info = self.schema.type_info(ty, out.reborrow())?;
        let info = info
            .ok_or(EvalError::internal(EVALUATOR_INTERNAL))
            .store_err_into(out.reborrow())?;
        let StoredTypeKind::Enum { variants } = info.kind() else {
            return Err(out.fail(EvalError::internal(EVALUATOR_INTERNAL)));
        };
        for id in variants {
            self.step(out.reborrow())?;
            let variant = self.schema.variant_info(*id, out.reborrow())?;
            let variant = variant
                .ok_or(EvalError::internal(EVALUATOR_INTERNAL))
                .store_err_into(out.reborrow())?;
            if self
                .compare_text(variant.name(), name, out.reborrow())?
                .is_eq()
            {
                return Ok(*id);
            }
        }
        Err(out.fail(EvalError::internal(EVALUATOR_INTERNAL)))
    }

    pub fn step<'slot, 'borrow>(&self, out: Out<'slot, 'borrow, EvalError>) -> OResult<'slot, ()> {
        self.consume(1, out)
    }

    pub fn consume<'slot>(
        &self,
        steps: usize,
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, ()> {
        if let Some(budget) = self.budget {
            budget.consume(steps, out.reborrow())?;
        }
        Ok(())
    }

    pub fn compare_text<'slot, 'borrow>(
        &self,
        left: &str,
        right: &str,
        out: Out<'slot, 'borrow, EvalError>,
    ) -> OResult<'slot, Ordering> {
        match self.budget {
            Some(budget) => budget.compare_text(left, right, out),
            None => Ok(left.cmp(right)),
        }
    }

    pub fn host<'slot>(
        &self,
        ty: TypeId,
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, bool> {
        let host = self.schema.host(ty, out.reborrow())?;
        host.ok_or(EvalError::internal(EVALUATOR_INTERNAL))
            .store_err_into(out.reborrow())
    }

    pub fn builtin<'slot>(
        &self,
        ty: TypeId,
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, BuiltinType> {
        let info = self.schema.type_info(ty, out.reborrow())?;
        let info = info
            .ok_or(EvalError::internal(EVALUATOR_INTERNAL))
            .store_err_into(out.reborrow())?;
        match info.kind() {
            StoredTypeKind::Builtin(builtin) => Ok(builtin),
            _ => Err(out.fail(EvalError::internal(EVALUATOR_INTERNAL))),
        }
    }

    pub fn checked<'a, 'slot>(
        &self,
        value: EvalValue<'a>,
        allow_magnitude: bool,
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, EvalValue<'a>> {
        let info = self.schema.type_info(value.ty(), out.reborrow())?;
        let info = info
            .ok_or(EvalError::internal(EVALUATOR_INTERNAL))
            .store_err_into(out.reborrow())?;
        let valid = match (info.kind(), value.kind()) {
            (StoredTypeKind::Builtin(builtin), EvalKind::Signed(value)) => {
                Numeric::narrow_signed(value, builtin).is_some()
            }
            (StoredTypeKind::Builtin(builtin), EvalKind::Unsigned(value)) => {
                Numeric::narrow_unsigned(value, builtin).is_some()
            }
            (StoredTypeKind::Builtin(builtin), EvalKind::Magnitude(value)) if allow_magnitude => {
                Numeric::signed_bounds(builtin).is_some_and(|(min, _)| value <= min.unsigned_abs())
            }
            (StoredTypeKind::Builtin(builtin), EvalKind::Float(value)) => {
                Numeric::finite_float(value, builtin) == Some(value)
            }
            (StoredTypeKind::Builtin(BuiltinType::Bool), EvalKind::Boolean(_))
            | (StoredTypeKind::Builtin(BuiltinType::String), EvalKind::String(_))
            | (StoredTypeKind::Option { .. }, EvalKind::None)
            | (StoredTypeKind::Struct { .. }, EvalKind::Structure(_)) => true,
            (kind, EvalKind::Value(value)) => matches!(
                (kind, value.as_ref()),
                (StoredTypeKind::Option { .. }, ValueKind::Optional(_))
                    | (StoredTypeKind::Array { .. }, ValueKind::Array(_))
                    | (StoredTypeKind::Map { .. }, ValueKind::Map(_))
                    | (StoredTypeKind::Enum { .. }, ValueKind::Enumeration { .. })
            ),
            _ => false,
        };
        if valid {
            Ok(value)
        } else {
            Err(out.fail(EvalError::internal(EVALUATOR_INTERNAL)))
        }
    }

    pub fn evaluate<'a, 'slot, 'borrow>(
        &self,
        expression: &'a TypedExpression,
        bindings: Bindings<'a>,
        out: Out<'slot, 'borrow, EvalError>,
    ) -> OResult<'slot, EvalValue<'a>> {
        match self
            .evaluation_errors
            .borrow_mut()
            .try_call(|evaluation_out| {
                ExpressionExecutor::new(self).evaluate_inner(expression, bindings, evaluation_out)
            }) {
            Ok(value) => Ok(value),
            Err(caught) => Err(out.fail(caught.take().with_source_span(expression.span()))),
        }
    }

    pub(super) fn finish<'a, 'slot>(
        &self,
        expression: &'a TypedExpression,
        inputs: FinishInputs<'a, '_>,
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, EvalValue<'a>> {
        let FinishInputs {
            index,
            bindings,
            locals,
            first,
            second,
        } = inputs;
        let node = expression
            .node(index)
            .ok_or(EvalError::internal(EVALUATOR_INTERNAL))
            .store_err_into(out.reborrow())?;
        let ty = node.ty();
        let info = self.schema.type_info(ty, out.reborrow())?;
        info.ok_or(EvalError::internal(EVALUATOR_INTERNAL))
            .store_err_into(out.reborrow())?;
        match node.kind() {
            StoredExpressionKind::Literal(literal) => {
                self.finish_literal(literal, ty, out.reborrow())
            }
            StoredExpressionKind::Binding(binding) => self.finish_binding(
                BindingInputs {
                    binding,
                    ty,
                    bindings,
                    locals,
                },
                out.reborrow(),
            ),
            StoredExpressionKind::Field { field, .. } => {
                self.finish_field(field, ty, first, out.reborrow())
            }
            StoredExpressionKind::Unary { operator, .. } => {
                self.finish_unary(operator, ty, first, out.reborrow())
            }
            StoredExpressionKind::Binary { operator, .. } => {
                self.finish_binary(operator, ty, (first, second), out.reborrow())
            }
            StoredExpressionKind::Method { .. } => {
                Err(out.fail(EvalError::internal(EVALUATOR_INTERNAL)))
            }
        }
    }

    fn finish_literal<'a, 'slot>(
        &self,
        literal: ExpressionLiteral<'a>,
        ty: TypeId,
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, EvalValue<'a>> {
        let kind = match literal {
            ExpressionLiteral::String(value) => EvalKind::String(value),
            ExpressionLiteral::Boolean(value) => EvalKind::Boolean(value),
            ExpressionLiteral::None => EvalKind::None,
            ExpressionLiteral::Float(value) => {
                let builtin = self.builtin(ty, out.reborrow())?;
                EvalKind::Float(
                    Numeric::finite_float(value, builtin)
                        .ok_or(EvalError::Arithmetic)
                        .store_err_into(out.reborrow())?,
                )
            }
            ExpressionLiteral::Integer(value) => {
                let builtin = self.builtin(ty, out.reborrow())?;
                if Numeric::signed_bounds(builtin).is_some() {
                    match i128::try_from(value) {
                        Ok(value) if Numeric::narrow_signed(value, builtin).is_some() => {
                            EvalKind::Signed(value)
                        }
                        Ok(_) => EvalKind::Magnitude(value),
                        Err(_) => EvalKind::Magnitude(value),
                    }
                } else if Numeric::unsigned_max(builtin).is_some() {
                    EvalKind::Unsigned(
                        Numeric::narrow_unsigned(value, builtin)
                            .ok_or(EvalError::internal(EVALUATOR_INTERNAL))
                            .store_err_into(out.reborrow())?,
                    )
                } else {
                    return Err(out.fail(EvalError::internal(EVALUATOR_INTERNAL)));
                }
            }
        };
        self.checked(EvalValue::new(kind, ty, false), true, out)
    }

    fn finish_binding<'a, 'locals, 'slot>(
        &self,
        inputs: BindingInputs<'a, 'locals>,
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, EvalValue<'a>> {
        let BindingInputs {
            binding,
            ty,
            bindings,
            locals,
        } = inputs;
        let value = match binding {
            StoredBindingInfo::SelfValue => bindings
                .self_value
                .ok_or(EvalError::internal(EVALUATOR_INTERNAL))
                .store_err_into(out.reborrow())?,
            StoredBindingInfo::Root => {
                let root = bindings
                    .root
                    .ok_or(EvalError::internal(EVALUATOR_INTERNAL))
                    .store_err_into(out.reborrow())?;
                let entry = self.schema.entry_type(out.reborrow())?;
                let entry = entry
                    .ok_or(EvalError::internal(EVALUATOR_INTERNAL))
                    .store_err_into(out.reborrow())?;
                let host = self.host(ty, out.reborrow())?;
                EvalValue::from_config(root, entry, host)
            }
            StoredBindingInfo::Item => bindings
                .item
                .ok_or(EvalError::internal(EVALUATOR_INTERNAL))
                .store_err_into(out.reborrow())?,
            StoredBindingInfo::Key => {
                let key = bindings
                    .key
                    .ok_or(EvalError::internal(EVALUATOR_INTERNAL))
                    .store_err_into(out.reborrow())?;
                EvalValue::new(EvalKind::String(key), ty, false)
            }
            StoredBindingInfo::Value => bindings
                .value
                .ok_or(EvalError::internal(EVALUATOR_INTERNAL))
                .store_err_into(out.reborrow())?,
            StoredBindingInfo::Closure(parameter) => {
                let mut found = None;
                for local in locals.iter().rev() {
                    self.step(out.reborrow())?;
                    if local.parameter == parameter {
                        found = Some(local.value);
                        break;
                    }
                }
                found
                    .ok_or(EvalError::internal(EVALUATOR_INTERNAL))
                    .store_err_into(out.reborrow())?
            }
        };
        if !self.schema.same_type(value.ty(), ty, out.reborrow())? {
            return Err(out.fail(EvalError::internal(EVALUATOR_INTERNAL)));
        }
        self.checked(value, false, out)
    }

    fn finish_field<'a, 'slot>(
        &self,
        field_id: FieldId,
        ty: TypeId,
        first: Option<EvalValue<'a>>,
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, EvalValue<'a>> {
        let receiver = first
            .ok_or(EvalError::internal(EVALUATOR_INTERNAL))
            .store_err_into(out.reborrow())?;
        let owner = self.schema.type_info(receiver.ty(), out.reborrow())?;
        let owner = owner
            .ok_or(EvalError::internal(EVALUATOR_INTERNAL))
            .store_err_into(out.reborrow())?;
        let field = self.schema.field_info(field_id, out.reborrow())?;
        let field = field
            .ok_or(EvalError::internal(EVALUATOR_INTERNAL))
            .store_err_into(out.reborrow())?;
        if owner.fields().get(field.slot()) != Some(&field_id) {
            return Err(out.fail(EvalError::internal(EVALUATOR_INTERNAL)));
        }
        if !self.schema.same_type(field.ty(), ty, out.reborrow())? {
            return Err(out.fail(EvalError::internal(EVALUATOR_INTERNAL)));
        }
        let fields = receiver.fields(out.reborrow())?;
        let ValueKind::Structure(values) = fields.as_ref() else {
            return Err(out.fail(EvalError::internal(EVALUATOR_INTERNAL)));
        };
        if values.len() != owner.fields().len() {
            return Err(out.fail(EvalError::internal(EVALUATOR_INTERNAL)));
        }
        self.step(out.reborrow())?;
        let value = values
            .get(field.slot())
            .filter(|entry| entry.name() == Some(field.name()))
            .map(|entry| entry.value())
            .ok_or(EvalError::internal(EVALUATOR_INTERNAL))
            .store_err_into(out.reborrow())?;
        let host = if field.host() {
            true
        } else {
            self.host(ty, out.reborrow())?
        };
        self.checked(EvalValue::config(value, ty, host), false, out)
    }

    fn finish_unary<'a, 'slot>(
        &self,
        operator: UnaryOperator,
        ty: TypeId,
        first: Option<EvalValue<'a>>,
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, EvalValue<'a>> {
        let operand = first
            .ok_or(EvalError::internal(EVALUATOR_INTERNAL))
            .store_err_into(out.reborrow())?;
        let kind = match operator {
            UnaryOperator::Not => EvalKind::Boolean(!operand.boolean(out.reborrow())?),
            UnaryOperator::Negate => {
                let builtin = self.builtin(ty, out.reborrow())?;
                match operand.kind() {
                    EvalKind::Signed(value) => EvalKind::Signed(
                        value
                            .checked_neg()
                            .and_then(|value| Numeric::narrow_signed(value, builtin))
                            .ok_or(EvalError::Arithmetic)
                            .store_err_into(out.reborrow())?,
                    ),
                    EvalKind::Magnitude(value) => {
                        let value = if value == 1_u128 << 127 {
                            i128::MIN
                        } else {
                            -i128::try_from(value)
                                .map_err(|_| EvalError::internal(EVALUATOR_INTERNAL))
                                .store_err_into(out.reborrow())?
                        };
                        EvalKind::Signed(
                            Numeric::narrow_signed(value, builtin)
                                .ok_or(EvalError::Arithmetic)
                                .store_err_into(out.reborrow())?,
                        )
                    }
                    EvalKind::Float(value) => EvalKind::Float(
                        Numeric::finite_float(-value, builtin)
                            .ok_or(EvalError::Arithmetic)
                            .store_err_into(out.reborrow())?,
                    ),
                    _ => {
                        return Err(out.fail(EvalError::internal(EVALUATOR_INTERNAL)));
                    }
                }
            }
        };
        self.checked(EvalValue::new(kind, ty, false), true, out)
    }

    fn finish_binary<'a, 'slot>(
        &self,
        operator: BinaryOperator,
        ty: TypeId,
        values: (Option<EvalValue<'a>>, Option<EvalValue<'a>>),
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, EvalValue<'a>> {
        let (first, second) = values;
        let left = first
            .ok_or(EvalError::internal(EVALUATOR_INTERNAL))
            .store_err_into(out.reborrow())?;
        let kind = match operator {
            BinaryOperator::And => {
                let left = left.boolean(out.reborrow())?;
                let value = if left {
                    let right = second
                        .ok_or(EvalError::internal(EVALUATOR_INTERNAL))
                        .store_err_into(out.reborrow())?;
                    right.boolean(out.reborrow())?
                } else {
                    false
                };
                EvalKind::Boolean(value)
            }
            BinaryOperator::Or => {
                let left = left.boolean(out.reborrow())?;
                let value = if left {
                    true
                } else {
                    let right = second
                        .ok_or(EvalError::internal(EVALUATOR_INTERNAL))
                        .store_err_into(out.reborrow())?;
                    right.boolean(out.reborrow())?
                };
                EvalKind::Boolean(value)
            }
            BinaryOperator::Add
            | BinaryOperator::Subtract
            | BinaryOperator::Multiply
            | BinaryOperator::Divide => {
                let right = second
                    .ok_or(EvalError::internal(EVALUATOR_INTERNAL))
                    .store_err_into(out.reborrow())?;
                let builtin = self.builtin(ty, out.reborrow())?;
                if self.builtin(left.ty(), out.reborrow())? != builtin
                    || self.builtin(right.ty(), out.reborrow())? != builtin
                {
                    return Err(out.fail(EvalError::internal(EVALUATOR_INTERNAL)));
                }
                Numeric::arithmetic(operator, left.kind(), right.kind(), builtin, out.reborrow())?
            }
            _ => {
                let right = second
                    .ok_or(EvalError::internal(EVALUATOR_INTERNAL))
                    .store_err_into(out.reborrow())?;
                let equality = matches!(operator, BinaryOperator::Equal | BinaryOperator::NotEqual);
                let order = ValueComparison::compare(
                    self,
                    left,
                    right,
                    ComparisonOptions {
                        equality_only: equality,
                        depth: 0,
                    },
                    out.reborrow(),
                )?;
                EvalKind::Boolean(match operator {
                    BinaryOperator::Equal => order == Ordering::Equal,
                    BinaryOperator::NotEqual => order != Ordering::Equal,
                    BinaryOperator::Less => order == Ordering::Less,
                    BinaryOperator::LessEqual => order != Ordering::Greater,
                    BinaryOperator::Greater => order == Ordering::Greater,
                    BinaryOperator::GreaterEqual => order != Ordering::Less,
                    _ => {
                        return Err(out.fail(EvalError::internal(EVALUATOR_INTERNAL)));
                    }
                })
            }
        };
        self.checked(EvalValue::new(kind, ty, false), true, out)
    }

    pub(super) fn method<'a, 'slot>(
        &self,
        receiver: EvalValue<'a>,
        call: MethodCall,
        child: Option<EvalValue<'a>>,
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, EvalValue<'a>> {
        let MethodCall {
            method,
            argument,
            ty,
        } = call;
        let kind = match method {
            ExpressionMethodInfo::Length | ExpressionMethodInfo::IsEmpty => {
                if argument.is_some() {
                    return Err(out.fail(EvalError::internal(EVALUATOR_INTERNAL)));
                }
                let value = receiver.value(out.reborrow())?;
                let length = match value.as_ref() {
                    ValueKind::Array(values) => values.len(),
                    ValueKind::Map(values) => values.len(),
                    _ => {
                        return Err(out.fail(EvalError::internal(EVALUATOR_INTERNAL)));
                    }
                };
                if method == ExpressionMethodInfo::Length {
                    let length = u64::try_from(length)
                        .map_err(|_| EvalError::internal(EVALUATOR_INTERNAL))
                        .store_err_into(out.reborrow())?;
                    EvalKind::Unsigned(u128::from(length))
                } else {
                    EvalKind::Boolean(length == 0)
                }
            }
            ExpressionMethodInfo::IsSome | ExpressionMethodInfo::IsNone => {
                if argument.is_some() {
                    return Err(out.fail(EvalError::internal(EVALUATOR_INTERNAL)));
                }
                let some = match receiver.kind() {
                    EvalKind::None => false,
                    EvalKind::Value(value) => match value.as_ref() {
                        ValueKind::Optional(value) => value.is_some(),
                        _ => {
                            return Err(out.fail(EvalError::internal(EVALUATOR_INTERNAL)));
                        }
                    },
                    _ => {
                        return Err(out.fail(EvalError::internal(EVALUATOR_INTERNAL)));
                    }
                };
                EvalKind::Boolean(if method == ExpressionMethodInfo::IsSome {
                    some
                } else {
                    !some
                })
            }
            ExpressionMethodInfo::StartsWith | ExpressionMethodInfo::IsVariant => {
                let Some(StoredExpressionArgument::Expression(_)) = argument else {
                    return Err(out.fail(EvalError::internal(EVALUATOR_INTERNAL)));
                };
                let child = child
                    .ok_or(EvalError::internal(EVALUATOR_INTERNAL))
                    .store_err_into(out.reborrow())?;
                let argument = child.string(out.reborrow())?;
                let matches = if method == ExpressionMethodInfo::StartsWith {
                    let text = receiver.string(out.reborrow())?;
                    let mut matches = true;
                    for (index, byte) in argument.bytes().enumerate() {
                        self.consume(1, out.reborrow())?;
                        let Some(actual) = text.as_bytes().get(index) else {
                            matches = false;
                            break;
                        };
                        self.consume(1, out.reborrow())?;
                        if *actual != byte {
                            matches = false;
                            break;
                        }
                    }
                    matches
                } else {
                    let value = receiver.value(out.reborrow())?;
                    let ValueKind::Enumeration { variant, .. } = value.as_ref() else {
                        return Err(out.fail(EvalError::internal(EVALUATOR_INTERNAL)));
                    };
                    self.compare_text(variant, argument, out.reborrow())? == Ordering::Equal
                };
                EvalKind::Boolean(matches)
            }
            ExpressionMethodInfo::Any | ExpressionMethodInfo::All | ExpressionMethodInfo::Sum => {
                let info = self.schema.type_info(receiver.ty(), out.reborrow())?;
                let info = info
                    .ok_or(EvalError::internal(EVALUATOR_INTERNAL))
                    .store_err_into(out.reborrow())?;
                let StoredTypeKind::Array { element, .. } = info.kind() else {
                    return Err(out.fail(EvalError::internal(EVALUATOR_INTERNAL)));
                };
                let value = receiver.value(out.reborrow())?;
                let ValueKind::Array(values) = value.as_ref() else {
                    return Err(out.fail(EvalError::internal(EVALUATOR_INTERNAL)));
                };
                if method == ExpressionMethodInfo::Sum {
                    if argument.is_some() || !self.schema.same_type(ty, element, out.reborrow())? {
                        return Err(out.fail(EvalError::internal(EVALUATOR_INTERNAL)));
                    }
                    Numeric::sum(
                        values,
                        self.builtin(ty, out.reborrow())?,
                        self.budget,
                        out.reborrow(),
                    )?
                } else {
                    return Err(out.fail(EvalError::internal(EVALUATOR_INTERNAL)));
                }
            }
        };
        self.checked(EvalValue::new(kind, ty, false), false, out)
    }
}
