use crate::{
    Bindings, ComparisonOptions, EvalError, EvalKind, EvalValue, Evaluator, FieldId, NumberValue,
    Numeric, StoredPayloadInfo, StoredRuleView, StoredTypeKind, TextChecks, TypeId,
    TypedExpression, ValueComparison, engine::model::EvaluationStack,
};
use core::cmp::Ordering;
use opeo::{OResult, Out, ResultOutExt};
use typol_core::{
    ChargedVec, DiagnosticContext, ExecutionBudget, ValueEntries, ValueKind, ValuePayloadView,
    ValueView, diagnostic_context,
};

static RULE_EVALUATION_INTERNAL: DiagnosticContext = diagnostic_context!(
    "typol-semantic",
    "constraint-evaluation",
    "rule-value-shape-mismatch",
);

pub struct SumBoundsRequest<'a, Element> {
    pub element: Element,
    pub values: ValueView<'a>,
    pub min: Option<NumberValue>,
    pub max: Option<NumberValue>,
}

#[derive(Clone, Copy)]
pub enum AllItemValue<'a> {
    ArrayItem(ValueView<'a>),
    MapEntry { key: &'a str, value: ValueView<'a> },
}

pub struct AllItemRequest<'a, Expression, Element> {
    pub expression: Expression,
    pub element: Element,
    pub item: AllItemValue<'a>,
}

pub(crate) struct UniqueRequest<'e, 'schema, 'values, 'path, Hash> {
    pub(crate) evaluator: &'e Evaluator<'schema>,
    pub(crate) values: ValueView<'values>,
    pub(crate) element: TypeId,
    pub(crate) by: Option<&'path [FieldId]>,
    pub(crate) hash: Hash,
}

struct UniqueItem<'a> {
    hash: (u64, u64),
    index: usize,
    value: EvalValue<'a>,
}

fn unique_with<'e, 'schema, 'values, 'path, 'result_slot, Hash>(
    request: UniqueRequest<'e, 'schema, 'values, 'path, Hash>,
    mut out: Out<'result_slot, '_, EvalError>,
) -> OResult<'result_slot, ChargedVec<bool>>
where
    Hash: for<'slot, 'call> Fn(
        &Evaluator<'schema>,
        EvalValue<'values>,
        Out<'slot, 'call, EvalError>,
    ) -> OResult<'slot, (u64, u64)>,
{
    let UniqueRequest {
        evaluator,
        values,
        element,
        by,
        hash,
    } = request;
    let ValueKind::Array(values) = values.as_ref() else {
        return Err(out.fail(EvalError::internal(RULE_EVALUATION_INTERNAL)));
    };
    let value_count = values.len();
    let mut items = ChargedVec::new(evaluator.budget(), out.reborrow())?;
    let mut duplicates = ChargedVec::new(evaluator.budget(), out.reborrow())?;
    items.try_reserve_exact(value_count, out.reborrow())?;
    duplicates.try_reserve_exact(value_count, out.reborrow())?;
    duplicates.resize(value_count, false, out.reborrow())?;
    for (index, value) in values.enumerate() {
        evaluator.step(out.reborrow())?;
        let value =
            ValueRules::unique_value(evaluator, value.value(), element, by, out.reborrow())?;
        let hash = hash(evaluator, value, out.reborrow())?;
        items.try_push(UniqueItem { hash, index, value }, out.reborrow())?;
    }
    items.sort_by(
        |left, right, mut sort_out| {
            evaluator.step(sort_out.reborrow())?;
            Ok(left
                .hash
                .cmp(&right.hash)
                .then(left.index.cmp(&right.index)))
        },
        out.reborrow(),
    )?;
    let mut start = 0;
    while start < items.len() {
        let mut end = start + 1;
        let first_hash = items
            .get(start)
            .ok_or(EvalError::internal(RULE_EVALUATION_INTERNAL))
            .store_err_into(out.reborrow())?
            .hash;
        while end < items.len() {
            let next_hash = items
                .get(end)
                .ok_or(EvalError::internal(RULE_EVALUATION_INTERNAL))
                .store_err_into(out.reborrow())?
                .hash;
            if next_hash != first_hash {
                break;
            }
            evaluator.step(out.reborrow())?;
            end += 1;
        }
        for current in start..end {
            let current_item = items
                .get(current)
                .ok_or(EvalError::internal(RULE_EVALUATION_INTERNAL))
                .store_err_into(out.reborrow())?;
            let mut duplicate = false;
            for previous in start..current {
                evaluator.step(out.reborrow())?;
                let previous_item = items
                    .get(previous)
                    .ok_or(EvalError::internal(RULE_EVALUATION_INTERNAL))
                    .store_err_into(out.reborrow())?;
                let was_duplicate = duplicates
                    .get(previous_item.index)
                    .copied()
                    .ok_or(EvalError::internal(RULE_EVALUATION_INTERNAL))
                    .store_err_into(out.reborrow())?;
                if !was_duplicate
                    && ValueComparison::compare(
                        evaluator,
                        previous_item.value,
                        current_item.value,
                        ComparisonOptions {
                            equality_only: true,
                            depth: 0,
                        },
                        out.reborrow(),
                    )? == Ordering::Equal
                {
                    duplicate = true;
                    break;
                }
            }
            if duplicate {
                *duplicates
                    .get_mut(current_item.index)
                    .ok_or(EvalError::internal(RULE_EVALUATION_INTERNAL))
                    .store_err_into(out.reborrow())? = true;
            }
        }
        start = end;
    }
    Ok(duplicates)
}

enum Work<'a> {
    Value(EvalValue<'a>, usize),
    Bytes(&'a str),
}

struct HashFields<'values, 'ids> {
    values: ValueEntries<'values>,
    ids: &'ids [FieldId],
}

fn hash_fields<'a, 'slot>(
    evaluator: &Evaluator<'_>,
    pending: &mut ChargedVec<Work<'a>>,
    input: HashFields<'a, '_>,
    depth: usize,
    mut out: Out<'slot, '_, EvalError>,
) -> OResult<'slot, ()> {
    let HashFields { values, ids } = input;
    if values.len() != ids.len() {
        return Err(out.fail(EvalError::internal(RULE_EVALUATION_INTERNAL)));
    }
    for id in ids.iter().rev() {
        evaluator.step(out.reborrow())?;
        let field_info = evaluator.schema().field_info(*id, out.reborrow())?;
        let field = field_info
            .ok_or(EvalError::internal(RULE_EVALUATION_INTERNAL))
            .store_err_into(out.reborrow())?;
        let value = values
            .get(field.slot())
            .filter(|value| value.name() == Some(field.name()))
            .ok_or(EvalError::internal(RULE_EVALUATION_INTERNAL))
            .store_err_into(out.reborrow())?;
        let name = value
            .name()
            .ok_or(EvalError::internal(RULE_EVALUATION_INTERNAL))
            .store_err_into(out.reborrow())?;
        EvaluationStack::push(
            pending,
            Work::Value(EvalValue::config(value.value(), field.ty(), false), depth),
            out.reborrow(),
        )?;
        EvaluationStack::push(pending, Work::Bytes(name), out.reborrow())?;
    }
    Ok(())
}

struct ValueFingerprintHasher<'a> {
    budget: Option<&'a ExecutionBudget>,
    first: u64,
    second: u64,
}

impl<'a> ValueFingerprintHasher<'a> {
    const fn new(budget: Option<&'a ExecutionBudget>) -> Self {
        Self {
            budget,
            first: 0xcbf2_9ce4_8422_2325,
            second: 0x9e37_79b9_7f4a_7c15,
        }
    }

    fn write_byte(&mut self, value: u8) {
        self.first ^= u64::from(value);
        self.first = self.first.wrapping_mul(0x0000_0100_0000_01b3);
        self.second ^= u64::from(value).wrapping_add(0x517c_c1b7_2722_0a95);
        self.second = self
            .second
            .rotate_left(13)
            .wrapping_mul(0x9ddf_ea08_eb38_2d69);
    }

    fn write_bool(&mut self, value: bool) {
        self.write_byte(u8::from(value));
    }

    fn write_bytes<'slot>(
        &mut self,
        bytes: &[u8],
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, ()> {
        self.write_u64(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
        for byte in bytes {
            if let Some(budget) = self.budget {
                budget.consume(1, out.reborrow())?;
            }
            self.write_byte(*byte);
        }
        Ok(())
    }

    fn write_wide(&mut self, bytes: [u8; 16]) {
        self.write_u64(16);
        for byte in bytes {
            self.write_byte(byte);
        }
    }

    fn write_u64(&mut self, value: u64) {
        for byte in value.to_le_bytes() {
            self.write_byte(byte);
        }
    }

    const fn finish(self) -> (u64, u64) {
        (self.first, self.second)
    }
}

pub struct ValueRules;

impl ValueRules {
    pub fn compare_number(left: NumberValue, right: NumberValue) -> Option<Ordering> {
        match (left, right) {
            (NumberValue::Signed(a), NumberValue::Signed(b)) => Some(a.cmp(&b)),
            (NumberValue::Unsigned(a), NumberValue::Unsigned(b)) => Some(a.cmp(&b)),
            (NumberValue::Float(a), NumberValue::Float(b)) => a.partial_cmp(&b),
            _ => None,
        }
    }

    pub fn number_bounds(
        number: NumberValue,
        min: Option<NumberValue>,
        max: Option<NumberValue>,
    ) -> bool {
        min.is_none_or(|min| {
            ValueRules::compare_number(number, min).is_some_and(|order| !order.is_lt())
        }) && max.is_none_or(|max| {
            ValueRules::compare_number(number, max).is_some_and(|order| !order.is_gt())
        })
    }

    /// The caller continues dispatching collection and regex rules that do not apply at this phase.
    pub fn scalar<'slot>(
        value: ValueView<'_>,
        rule: StoredRuleView<'_>,
        budget: Option<&ExecutionBudget>,
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, Option<bool>> {
        let string = || match value.as_ref() {
            ValueKind::String(value) => Ok(value),
            _ => Err(EvalError::internal(RULE_EVALUATION_INTERNAL)),
        };
        let text = TextChecks::new(budget);
        let result = match rule {
            StoredRuleView::NumberBounds { min, max } => {
                let number = match value.as_ref() {
                    ValueKind::Signed(v) => NumberValue::Signed(v),
                    ValueKind::Unsigned(v) => NumberValue::Unsigned(v),
                    ValueKind::Float32(v) => NumberValue::Float(f64::from(v)),
                    ValueKind::Float64(v) => NumberValue::Float(v),
                    _ => {
                        return Err(out.fail(EvalError::internal(RULE_EVALUATION_INTERNAL)));
                    }
                };
                ValueRules::number_bounds(number, min, max)
            }
            StoredRuleView::LengthBounds { min, max } => {
                let value = string().store_err_into(out.reborrow())?;
                let mut length = 0u64;
                for byte in value.bytes() {
                    if let Some(budget) = budget {
                        budget.consume(1, out.reborrow())?;
                    }
                    if byte & 0xc0 != 0x80 {
                        length = length
                            .checked_add(1)
                            .ok_or(EvalError::Resource)
                            .store_err_into(out.reborrow())?;
                    }
                }
                (min..=max).contains(&length)
            }
            StoredRuleView::Host => {
                let value = string().store_err_into(out.reborrow())?;
                text.host(value, out.reborrow())?
            }
            StoredRuleView::Email => {
                let value = string().store_err_into(out.reborrow())?;
                text.email(value, out.reborrow())?
            }
            StoredRuleView::Cidr { v4_only } => {
                let value = string().store_err_into(out.reborrow())?;
                text.cidr(value, v4_only, out.reborrow())?
            }
            StoredRuleView::SizeBounds { min, max } => {
                let length = match value.as_ref() {
                    ValueKind::Array(v) => v.len(),
                    ValueKind::Map(v) => v.len(),
                    _ => {
                        return Err(out.fail(EvalError::internal(RULE_EVALUATION_INTERNAL)));
                    }
                };
                let length = u64::try_from(length)
                    .map_err(|_| EvalError::internal(RULE_EVALUATION_INTERNAL))
                    .store_err_into(out.reborrow())?;
                min.is_none_or(|min| length >= min) && max.is_none_or(|max| length <= max)
            }
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    pub fn sum_bounds<'slot>(
        evaluator: &Evaluator<'_>,
        request: SumBoundsRequest<'_, TypeId>,
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, bool> {
        let builtin = evaluator.builtin(request.element, out.reborrow())?;
        let ValueKind::Array(values) = request.values.as_ref() else {
            return Err(out.fail(EvalError::internal(RULE_EVALUATION_INTERNAL)));
        };
        let sum = Numeric::sum(values, builtin, evaluator.budget(), out.reborrow())?;
        let number = match sum {
            EvalKind::Signed(v) => NumberValue::Signed(v),
            EvalKind::Unsigned(v) => NumberValue::Unsigned(v),
            EvalKind::Float(v) => NumberValue::Float(v),
            _ => return Err(out.fail(EvalError::internal(RULE_EVALUATION_INTERNAL))),
        };
        Ok(ValueRules::number_bounds(number, request.min, request.max))
    }

    pub fn unique_value<'a, 'slot>(
        evaluator: &Evaluator<'_>,
        mut value: ValueView<'a>,
        mut ty: TypeId,
        by: Option<&[FieldId]>,
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, EvalValue<'a>> {
        if let Some(path) = by {
            for (depth, id) in path.iter().enumerate() {
                if depth >= evaluator.limits().max_nesting_depth() {
                    return Err(out.fail(EvalError::Resource));
                }
                evaluator.step(out.reborrow())?;
                let type_info = evaluator.schema().type_info(ty, out.reborrow())?;
                let type_info = type_info
                    .ok_or(EvalError::internal(RULE_EVALUATION_INTERNAL))
                    .store_err_into(out.reborrow())?;
                let field_info = evaluator.schema().field_info(*id, out.reborrow())?;
                let field = field_info
                    .ok_or(EvalError::internal(RULE_EVALUATION_INTERNAL))
                    .store_err_into(out.reborrow())?;
                if type_info.fields().get(field.slot()) != Some(id) {
                    return Err(out.fail(EvalError::internal(RULE_EVALUATION_INTERNAL)));
                }
                let ValueKind::Structure(fields) = value.as_ref() else {
                    return Err(out.fail(EvalError::internal(RULE_EVALUATION_INTERNAL)));
                };
                if fields.len() != type_info.fields().len() {
                    return Err(out.fail(EvalError::internal(RULE_EVALUATION_INTERNAL)));
                }
                evaluator.step(out.reborrow())?;
                let field_value = fields
                    .get(field.slot())
                    .filter(|entry| entry.name() == Some(field.name()))
                    .ok_or(EvalError::internal(RULE_EVALUATION_INTERNAL))
                    .store_err_into(out.reborrow())?;
                value = field_value.value();
                ty = field.ty();
            }
        }
        Ok(EvalValue::config(value, ty, false))
    }

    /// Hashing filters candidates only; values in the same fingerprint group are always checked using full value semantics.
    pub fn unique<'slot>(
        evaluator: &Evaluator<'_>,
        values: ValueView<'_>,
        element: TypeId,
        by: Option<&[FieldId]>,
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, ChargedVec<bool>> {
        unique_with(
            UniqueRequest {
                evaluator,
                values,
                element,
                by,
                hash: Self::fingerprint,
            },
            out.reborrow(),
        )
    }

    #[cfg(test)]
    pub(crate) fn unique_with_hash_for_test<'result_slot, 'e, 'schema, 'values, 'path, Hash>(
        request: UniqueRequest<'e, 'schema, 'values, 'path, Hash>,
        out: Out<'result_slot, '_, EvalError>,
    ) -> OResult<'result_slot, ChargedVec<bool>>
    where
        Hash: for<'slot, 'call> Fn(
            &Evaluator<'schema>,
            EvalValue<'values>,
            Out<'slot, 'call, EvalError>,
        ) -> OResult<'slot, (u64, u64)>,
    {
        unique_with(request, out)
    }

    pub fn fingerprint<'slot>(
        evaluator: &Evaluator<'_>,
        value: EvalValue<'_>,
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, (u64, u64)> {
        let mut pending = ChargedVec::new(evaluator.budget(), out.reborrow())?;
        EvaluationStack::push(&mut pending, Work::Value(value, 0), out.reborrow())?;
        let mut hash = ValueFingerprintHasher::new(evaluator.budget());
        while let Some(work) = pending.pop() {
            evaluator.step(out.reborrow())?;
            let (value, depth) = match work {
                Work::Bytes(text) => {
                    hash.write_bytes(text.as_bytes(), out.reborrow())?;
                    continue;
                }
                Work::Value(value, depth) => (value, depth),
            };
            if depth >= evaluator.limits().max_nesting_depth() {
                return Err(out.fail(EvalError::Resource));
            }
            evaluator.checked(value, false, out.reborrow())?;
            let next = depth
                .checked_add(1)
                .ok_or(EvalError::Resource)
                .store_err_into(out.reborrow())?;
            match value.kind() {
                EvalKind::Signed(v) => {
                    hash.write_byte(1);
                    hash.write_wide(v.to_le_bytes());
                }
                EvalKind::Unsigned(v) => {
                    hash.write_byte(2);
                    hash.write_wide(v.to_le_bytes());
                }
                EvalKind::Float(v) => {
                    hash.write_byte(3);
                    hash.write_u64(if v == 0.0 { 0 } else { v.to_bits() });
                }
                EvalKind::Boolean(v) => {
                    hash.write_byte(4);
                    hash.write_bool(v);
                }
                EvalKind::String(v) => {
                    hash.write_byte(5);
                    hash.write_bytes(v.as_bytes(), out.reborrow())?;
                }
                EvalKind::None => {
                    hash.write_byte(6);
                    hash.write_bool(false);
                }
                EvalKind::Structure(fields) => {
                    hash.write_byte(7);
                    let type_info = evaluator.schema().type_info(value.ty(), out.reborrow())?;
                    let type_info = type_info
                        .ok_or(EvalError::internal(RULE_EVALUATION_INTERNAL))
                        .store_err_into(out.reborrow())?;
                    let StoredTypeKind::Struct { fields: ids } = type_info.kind() else {
                        return Err(out.fail(EvalError::internal(RULE_EVALUATION_INTERNAL)));
                    };
                    let ValueKind::Structure(fields) = fields.as_ref() else {
                        return Err(out.fail(EvalError::internal(RULE_EVALUATION_INTERNAL)));
                    };
                    hash_fields(
                        evaluator,
                        &mut pending,
                        HashFields {
                            values: fields,
                            ids,
                        },
                        next,
                        out.reborrow(),
                    )?;
                }
                EvalKind::Value(config) => {
                    let type_info = evaluator.schema().type_info(value.ty(), out.reborrow())?;
                    let type_info = type_info
                        .ok_or(EvalError::internal(RULE_EVALUATION_INTERNAL))
                        .store_err_into(out.reborrow())?;
                    match (type_info.kind(), config.as_ref()) {
                        (StoredTypeKind::Option { inner }, ValueKind::Optional(v)) => {
                            hash.write_byte(6);
                            hash.write_bool(v.is_some());
                            if let Some(v) = v {
                                EvaluationStack::push(
                                    &mut pending,
                                    Work::Value(EvalValue::config(v, inner, false), next),
                                    out.reborrow(),
                                )?;
                            }
                        }
                        (StoredTypeKind::Array { element, .. }, ValueKind::Array(values)) => {
                            hash.write_byte(8);
                            let value_count = values.len();
                            hash.write_u64(value_count as u64);
                            let mut ordered = ChargedVec::new(evaluator.budget(), out.reborrow())?;
                            ordered.try_reserve_exact(value_count, out.reborrow())?;
                            for entry in values {
                                evaluator.step(out.reborrow())?;
                                ordered.try_push(entry.value(), out.reborrow())?;
                            }
                            for v in ordered.iter().rev() {
                                evaluator.step(out.reborrow())?;
                                EvaluationStack::push(
                                    &mut pending,
                                    Work::Value(EvalValue::config(*v, element, false), next),
                                    out.reborrow(),
                                )?;
                            }
                        }
                        (StoredTypeKind::Map { value: ty }, ValueKind::Map(entries)) => {
                            hash.write_byte(9);
                            let entry_count = entries.len();
                            hash.write_u64(entry_count as u64);
                            let mut sorted = ChargedVec::new(evaluator.budget(), out.reborrow())?;
                            sorted.try_reserve_exact(entry_count, out.reborrow())?;
                            for entry in entries {
                                evaluator.step(out.reborrow())?;
                                sorted.try_push(entry, out.reborrow())?;
                            }
                            sorted.sort_by(
                                |a, b, mut sort_out| {
                                    evaluator.step(sort_out.reborrow())?;
                                    let a_name = a
                                        .name()
                                        .ok_or(EvalError::internal(RULE_EVALUATION_INTERNAL))
                                        .store_err_into(sort_out.reborrow())?;
                                    let b_name = b
                                        .name()
                                        .ok_or(EvalError::internal(RULE_EVALUATION_INTERNAL))
                                        .store_err_into(sort_out.reborrow())?;
                                    evaluator.compare_text(a_name, b_name, sort_out.reborrow())
                                },
                                out.reborrow(),
                            )?;
                            for entry in sorted.iter().rev() {
                                let name = entry
                                    .name()
                                    .ok_or(EvalError::internal(RULE_EVALUATION_INTERNAL))
                                    .store_err_into(out.reborrow())?;
                                EvaluationStack::push(
                                    &mut pending,
                                    Work::Value(EvalValue::config(entry.value(), ty, false), next),
                                    out.reborrow(),
                                )?;
                                EvaluationStack::push(
                                    &mut pending,
                                    Work::Bytes(name),
                                    out.reborrow(),
                                )?;
                            }
                        }
                        (
                            StoredTypeKind::Enum { .. },
                            ValueKind::Enumeration { variant, payload },
                        ) => {
                            hash.write_byte(10);
                            hash.write_bytes(variant.as_bytes(), out.reborrow())?;
                            let variant_id =
                                evaluator.variant_id(value.ty(), variant, out.reborrow())?;
                            let variant_info = evaluator
                                .schema()
                                .variant_info(variant_id, out.reborrow())?;
                            let variant = variant_info
                                .ok_or(EvalError::internal(RULE_EVALUATION_INTERNAL))
                                .store_err_into(out.reborrow())?;
                            match (variant.payload(), payload) {
                                (StoredPayloadInfo::Unit, ValuePayloadView::Unit) => {
                                    hash.write_byte(0)
                                }
                                (StoredPayloadInfo::Single { ty }, ValuePayloadView::Single(v)) => {
                                    hash.write_byte(1);
                                    EvaluationStack::push(
                                        &mut pending,
                                        Work::Value(EvalValue::config(v, ty, false), next),
                                        out.reborrow(),
                                    )?;
                                }
                                (
                                    StoredPayloadInfo::Structure { fields: ids },
                                    ValuePayloadView::Structure(fields),
                                ) => {
                                    hash.write_byte(2);
                                    hash_fields(
                                        evaluator,
                                        &mut pending,
                                        HashFields {
                                            values: fields,
                                            ids,
                                        },
                                        next,
                                        out.reborrow(),
                                    )?;
                                }
                                _ => {
                                    return Err(
                                        out.fail(EvalError::internal(RULE_EVALUATION_INTERNAL))
                                    );
                                }
                            }
                        }
                        _ => {
                            return Err(out.fail(EvalError::internal(RULE_EVALUATION_INTERNAL)));
                        }
                    }
                }
                EvalKind::Magnitude(_) => {
                    return Err(out.fail(EvalError::internal(RULE_EVALUATION_INTERNAL)));
                }
            }
        }
        Ok(hash.finish())
    }

    /// Each call returns only the current element’s structured execution result; the caller chooses the report location and message.
    pub fn all_item<'slot>(
        evaluator: &Evaluator<'_>,
        request: AllItemRequest<'_, &'_ TypedExpression, TypeId>,
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, bool> {
        evaluator.step(out.reborrow())?;
        let mut bindings = Bindings::default();
        match request.item {
            AllItemValue::ArrayItem(item) => {
                bindings.item = Some(EvalValue::config(item, request.element, false));
            }
            AllItemValue::MapEntry { key, value } => {
                bindings.key = Some(key);
                bindings.value = Some(EvalValue::config(value, request.element, false));
            }
        }
        let value = evaluator.evaluate(request.expression, bindings, out.reborrow())?;
        value.boolean(out.reborrow())
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "compiler")]
    mod compiler {
        use crate::{
            ClosureLocalId, ExpressionBinding, ExpressionNodeId, FieldId, SchemaAccessError,
            StoredTypeKind, TypeId, TypedExpression, TypedExpressionKind, TypedExpressionNode,
            VariantId,
            engine::{
                Bindings, EvalError, EvalValue, Evaluator,
                model::{ExecutionField, ExecutionModel, ExecutionType, ExecutionVariant},
                rules::{UniqueRequest, ValueRules},
            },
        };
        use alloc::{format, string::String, vec::Vec};
        use core::fmt::{Debug, Formatter, Result as FmtResult};
        use opeo::{ErrSlot, OResult, Out};
        use typol_core::{
            BudgetError, ChargedVec, ConfigValue, ConfigValueBuilder, ExecutionBudget,
            ExecutionLimits, SourceId, SourceSpan, Span, SpanError, ValueError, ValueId,
            ValueSource,
        };
        use typol_model::BuiltinType;
        use typol_policy::{RegexLimits, ValueLimits};

        enum TestError {
            Evaluation(EvalError),
            Budget(BudgetError),
            Value(ValueError),
            Span(SpanError),
            Assertion(String),
        }

        impl Debug for TestError {
            fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
                match self {
                    Self::Evaluation(error) => write!(formatter, "Evaluation({error:?})"),
                    Self::Budget(error) => write!(formatter, "Budget({error:?})"),
                    Self::Value(error) => write!(formatter, "Value({error:?})"),
                    Self::Span(error) => write!(formatter, "Span({error:?})"),
                    Self::Assertion(message) => write!(formatter, "Assertion({message})"),
                }
            }
        }

        impl From<EvalError> for TestError {
            fn from(error: EvalError) -> Self {
                Self::Evaluation(error)
            }
        }

        impl From<BudgetError> for TestError {
            fn from(error: BudgetError) -> Self {
                Self::Budget(error)
            }
        }

        impl From<ValueError> for TestError {
            fn from(error: ValueError) -> Self {
                Self::Value(error)
            }
        }

        impl From<SpanError> for TestError {
            fn from(error: SpanError) -> Self {
                Self::Span(error)
            }
        }

        type TestResult<T> = Result<T, TestError>;

        struct TestSlots {
            evaluation: ErrSlot<EvalError>,
            budget: ErrSlot<BudgetError>,
            value: ErrSlot<ValueError>,
            span: ErrSlot<SpanError>,
        }

        impl TestSlots {
            fn new() -> Self {
                Self {
                    evaluation: ErrSlot::new(),
                    budget: ErrSlot::new(),
                    value: ErrSlot::new(),
                    span: ErrSlot::new(),
                }
            }

            fn evaluate<T>(
                &mut self,
                operation: impl for<'slot> FnOnce(Out<'slot, 'slot, EvalError>) -> OResult<'slot, T>,
            ) -> Result<T, EvalError> {
                self.evaluation.call(operation)
            }

            fn allocate<T>(
                &mut self,
                operation: impl for<'slot> FnOnce(Out<'slot, 'slot, BudgetError>) -> OResult<'slot, T>,
            ) -> Result<T, BudgetError> {
                self.budget.call(operation)
            }

            fn span<T>(
                &mut self,
                operation: impl for<'slot> FnOnce(Out<'slot, 'slot, SpanError>) -> OResult<'slot, T>,
            ) -> Result<T, SpanError> {
                self.span.call(operation)
            }

            fn make_value<T>(
                &mut self,
                operation: impl for<'slot> FnOnce(Out<'slot, 'slot, ValueError>) -> OResult<'slot, T>,
            ) -> Result<T, ValueError> {
                self.value.call(operation)
            }
        }

        fn unsigned_value(slots: &mut TestSlots, value: u128) -> TestResult<ConfigValue> {
            let value = slots.make_value(|mut out| {
                let mut builder = ConfigValueBuilder::new(None, out.reborrow())?;
                let root = builder.add_unsigned(value, ValueSource::RustValue, out.reborrow())?;
                builder.finish(root, out)
            })?;
            Ok(value)
        }

        fn unsigned_array(slots: &mut TestSlots, values: &[u128]) -> TestResult<ConfigValue> {
            let value = slots.make_value(|mut out| {
                let mut builder = ConfigValueBuilder::new(None, out.reborrow())?;
                let mut ids = Vec::<ValueId>::with_capacity(values.len());
                for value in values {
                    ids.push(builder.add_unsigned(
                        *value,
                        ValueSource::RustValue,
                        out.reborrow(),
                    )?);
                }
                let root = builder.add_array(&ids, ValueSource::RustValue, out.reborrow())?;
                builder.finish(root, out)
            })?;
            Ok(value)
        }

        macro_rules! check_eq {
            ($actual:expr, $expected:expr $(,)?) => {{
                let actual = &$actual;
                let expected = &$expected;
                if actual != expected {
                    return Err(TestError::Assertion(format!(
                        "assertion failed: {}; actual={actual:?}, expected={expected:?}",
                        stringify!($actual)
                    )));
                }
            }};
        }

        macro_rules! check {
            ($condition:expr $(,)?) => {{
                if !$condition {
                    return Err(TestError::Assertion(format!(
                        "assertion failed: {}",
                        stringify!($condition)
                    )));
                }
            }};
        }
        struct Numbers;

        fn constant_hash<'schema, 'values, 'slot, 'call>(
            _: &Evaluator<'schema>,
            _: EvalValue<'values>,
            _: Out<'slot, 'call, EvalError>,
        ) -> OResult<'slot, (u64, u64)> {
            Ok((0, 0))
        }

        impl ExecutionModel for Numbers {
            fn type_info<'slot, 'borrow>(
                &self,
                _: TypeId,
                _: Out<'slot, 'borrow, EvalError>,
            ) -> OResult<'slot, Option<ExecutionType<'_>>> {
                Ok(Some(ExecutionType::new(StoredTypeKind::Builtin(
                    BuiltinType::U8,
                ))))
            }

            fn field_info<'slot, 'borrow>(
                &self,
                _: FieldId,
                _: Out<'slot, 'borrow, EvalError>,
            ) -> OResult<'slot, Option<ExecutionField<'_>>> {
                Ok(None)
            }

            fn variant_info<'slot, 'borrow>(
                &self,
                _: VariantId,
                _: Out<'slot, 'borrow, EvalError>,
            ) -> OResult<'slot, Option<ExecutionVariant<'_>>> {
                Ok(None)
            }

            fn same_type<'slot, 'borrow>(
                &self,
                a: TypeId,
                b: TypeId,
                _: Out<'slot, 'borrow, EvalError>,
            ) -> OResult<'slot, bool> {
                Ok(a == b)
            }

            fn host<'slot, 'borrow>(
                &self,
                _: TypeId,
                _: Out<'slot, 'borrow, EvalError>,
            ) -> OResult<'slot, Option<bool>> {
                Ok(Some(false))
            }

            fn entry_type<'slot, 'borrow>(
                &self,
                _: Out<'slot, 'borrow, EvalError>,
            ) -> OResult<'slot, Option<TypeId>> {
                Ok(Some(TypeId::new(0)))
            }
        }

        #[test]
        fn forced_fingerprint_collisions_still_compare_values_and_keep_source_order()
        -> TestResult<()> {
            let mut slots = TestSlots::new();
            let limits = ValueLimits::default();
            let regex_limits = RegexLimits::default();
            let evaluator = Evaluator::new(&Numbers, &limits, &regex_limits, None);
            let values = unsigned_array(&mut slots, &[1, 2, 1, 3, 2])?;
            let duplicates = slots.evaluate(|out| {
                ValueRules::unique_with_hash_for_test(
                    UniqueRequest {
                        evaluator: &evaluator,
                        values: values.root(),
                        element: TypeId::new(0),
                        by: None,
                        hash: constant_hash,
                    },
                    out,
                )
            })?;
            check_eq!(duplicates.as_slice(), [false, false, true, false, true]);
            Ok(())
        }

        #[test]
        fn fingerprint_depth_and_invalid_value_are_distinct_errors() -> TestResult<()> {
            let mut slots = TestSlots::new();
            let mut limits = ValueLimits::default();
            limits.set_max_nesting_depth(0);
            let regex_limits = RegexLimits::default();
            let evaluator = Evaluator::new(&Numbers, &limits, &regex_limits, None);
            let value = unsigned_value(&mut slots, 1)?;
            check_eq!(
                slots.evaluate(|out| {
                    ValueRules::fingerprint(
                        &evaluator,
                        EvalValue::from_config(&value, TypeId::new(0), false),
                        out,
                    )
                }),
                Err(EvalError::Resource)
            );
            limits.set_max_nesting_depth(1);
            let evaluator = Evaluator::new(&Numbers, &limits, &regex_limits, None);
            let value = unsigned_value(&mut slots, 256)?;
            check!(matches!(
                slots.evaluate(|out| {
                    ValueRules::fingerprint(
                        &evaluator,
                        EvalValue::from_config(&value, TypeId::new(0), false),
                        out,
                    )
                }),
                Err(error) if error.is_internal() && error.context().is_some()
            ));
            Ok(())
        }

        #[test]
        fn malformed_expression_root_and_unbound_closure_are_internal_errors() -> TestResult<()> {
            let mut slots = TestSlots::new();
            let span = SourceSpan::new(SourceId::new(0), slots.span(|out| Span::new(0, 0, out))?);
            let limits = ValueLimits::default();
            let regex_limits = RegexLimits::default();
            let evaluator = Evaluator::new(&Numbers, &limits, &regex_limits, None);
            let budget = slots.allocate(|out| {
                ExecutionBudget::new(ExecutionLimits::new(67_108_864, 67_108_864), out)
            })?;
            let empty = TypedExpression::new(
                slots.allocate(|out| ChargedVec::new(Some(&budget), out))?,
                ExpressionNodeId::new(0),
                TypeId::new(0),
                span,
            );
            let error = slots
                .evaluate(|out| evaluator.evaluate(&empty, Bindings::default(), out))
                .err()
                .ok_or(TestError::Assertion(String::from(
                    "evaluation error is missing",
                )))?;
            check!(error.is_internal() && error.context().is_some());
            check_eq!(error.source_span(), Some(span));
            let closure = TypedExpressionNode::new(
                TypedExpressionKind::Binding(ExpressionBinding::Closure(ClosureLocalId::new(0))),
                TypeId::new(0),
                span,
            );
            let mut nodes = slots.allocate(|out| ChargedVec::new(Some(&budget), out))?;
            slots.allocate(|out| nodes.try_push(closure, out))?;
            let expression =
                TypedExpression::new(nodes, ExpressionNodeId::new(0), TypeId::new(0), span);
            let error = slots
                .evaluate(|out| evaluator.evaluate(&expression, Bindings::default(), out))
                .err()
                .ok_or(TestError::Assertion(String::from(
                    "evaluation error is missing",
                )))?;
            check!(error.is_internal() && error.context().is_some());
            check_eq!(error.source_span(), Some(span));
            Ok(())
        }

        #[test]
        fn eval_error_conversion_preserves_schema_table_and_index() -> TestResult<()> {
            let access = SchemaAccessError::new("program", 11);
            let error = EvalError::from(access);

            check!(error.is_internal());
            check_eq!(error.schema_access(), Some(access));
            check_eq!(
                error.context().map(|context| context.object_id()),
                Some(Some(11))
            );
            Ok(())
        }

        #[test]
        fn collision_verification_consumes_the_shared_balance() -> TestResult<()> {
            let mut slots = TestSlots::new();
            let limits = ValueLimits::default();
            let regex_limits = RegexLimits::default();
            let input = [1, 2, 3, 4, 5];
            let values = unsigned_array(&mut slots, &input)?;
            let budget = slots.allocate(|out| {
                ExecutionBudget::new(ExecutionLimits::new(10_000, 1_048_576), out)
            })?;
            let evaluator = Evaluator::new(&Numbers, &limits, &regex_limits, Some(&budget));
            let duplicates = slots.evaluate(|out| {
                ValueRules::unique_with_hash_for_test(
                    UniqueRequest {
                        evaluator: &evaluator,
                        values: values.root(),
                        element: TypeId::new(0),
                        by: None,
                        hash: constant_hash,
                    },
                    out,
                )
            })?;
            check!(duplicates.iter().all(|duplicate| !duplicate));
            let steps = budget.steps();
            check!(steps > input.len());
            for quota in [steps, steps - 1] {
                let shared = slots.allocate(|out| {
                    ExecutionBudget::new(ExecutionLimits::new(quota, 1_048_576), out)
                })?;
                let evaluator = Evaluator::new(&Numbers, &limits, &regex_limits, Some(&shared));
                match slots.evaluation.try_call(|out| {
                    ValueRules::unique_with_hash_for_test(
                        UniqueRequest {
                            evaluator: &evaluator,
                            values: values.root(),
                            element: TypeId::new(0),
                            by: None,
                            hash: constant_hash,
                        },
                        out,
                    )
                }) {
                    Ok(_) if quota == steps => {}
                    Err(caught) if quota < steps => {
                        check_eq!(caught.get(), &EvalError::Resource);
                    }
                    _ => {
                        return Err(TestError::Assertion(format!(
                            "assertion failed: quota={quota}"
                        )));
                    }
                }
            }
            Ok(())
        }
    }
}
