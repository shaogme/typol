use crate::{
    EvalError, EvalKind, EvalValue, Evaluator, FieldId, StoredPayloadInfo, StoredTypeKind,
    TextValidation, TypeId, engine::model::EvaluationStack,
};
use core::cmp::Ordering;
use opeo::{OResult, Out, ResultOutExt};
use typol_core::{
    ChargedVec, DiagnosticContext, ValueEntries, ValueKind, ValuePayloadView, ValueView,
    diagnostic_context,
};

static COMPARISON_INTERNAL: DiagnosticContext = diagnostic_context!(
    "typol-semantic",
    "value-comparison",
    "typed-value-shape-mismatch",
);

fn optional(value: EvalValue<'_>) -> Result<Option<ValueView<'_>>, EvalError> {
    match value.kind() {
        EvalKind::None => Ok(None),
        EvalKind::Value(value) => match value.as_ref() {
            ValueKind::Optional(value) => Ok(value),
            _ => Err(EvalError::internal(COMPARISON_INTERNAL)),
        },
        _ => Err(EvalError::internal(COMPARISON_INTERNAL)),
    }
}

struct Pair<'a> {
    left: EvalValue<'a>,
    right: EvalValue<'a>,
    equality_only: bool,
    depth: usize,
}

struct FieldComparisonRequest<'values, 'ids> {
    left: ValueView<'values>,
    right: ValueView<'values>,
    ids: &'ids [FieldId],
    depth: usize,
}

struct ArrayComparisonRequest<'values> {
    left: ValueEntries<'values>,
    right: ValueEntries<'values>,
    left_element: TypeId,
    right_element: TypeId,
    depth: usize,
}

struct MapComparisonRequest<'values> {
    left: ValueEntries<'values>,
    right: ValueEntries<'values>,
    left_value: TypeId,
    right_value: TypeId,
    depth: usize,
}

struct EnumComparisonRequest<'values> {
    left: EvalValue<'values>,
    right: EvalValue<'values>,
    left_value: ValueView<'values>,
    right_value: ValueView<'values>,
    left_variant: &'values str,
    right_variant: &'values str,
    left_payload: ValuePayloadView<'values>,
    right_payload: ValuePayloadView<'values>,
    depth: usize,
}

fn field_value<'a, 'slot>(
    evaluator: &Evaluator<'_>,
    parent: ValueView<'a>,
    name: &str,
    mut out: Out<'slot, '_, EvalError>,
) -> OResult<'slot, ValueView<'a>> {
    let entries = match parent.as_ref() {
        ValueKind::Structure(entries) => entries,
        ValueKind::Enumeration {
            payload: ValuePayloadView::Structure(entries),
            ..
        } => entries,
        _ => return Err(out.fail(EvalError::internal(COMPARISON_INTERNAL))),
    };
    for entry in entries {
        evaluator.step(out.reborrow())?;
        let candidate = entry
            .name()
            .ok_or(EvalError::internal(COMPARISON_INTERNAL))
            .store_err_into(out.reborrow())?;
        if evaluator.compare_text(candidate, name, out.reborrow())? == Ordering::Equal {
            return Ok(entry.value());
        }
    }
    Err(out.fail(EvalError::internal(COMPARISON_INTERNAL)))
}

fn queue_fields<'a, 'slot>(
    evaluator: &Evaluator<'_>,
    tasks: &mut ChargedVec<Pair<'a>>,
    request: FieldComparisonRequest<'a, '_>,
    mut out: Out<'slot, '_, EvalError>,
) -> OResult<'slot, ()> {
    let FieldComparisonRequest {
        left,
        right,
        ids,
        depth,
    } = request;
    let left_len = match left.as_ref() {
        ValueKind::Structure(entries) => entries.len(),
        ValueKind::Enumeration {
            payload: ValuePayloadView::Structure(entries),
            ..
        } => entries.len(),
        _ => return Err(out.fail(EvalError::internal(COMPARISON_INTERNAL))),
    };
    let right_len = match right.as_ref() {
        ValueKind::Structure(entries) => entries.len(),
        ValueKind::Enumeration {
            payload: ValuePayloadView::Structure(entries),
            ..
        } => entries.len(),
        _ => return Err(out.fail(EvalError::internal(COMPARISON_INTERNAL))),
    };
    if left_len != ids.len() || right_len != ids.len() {
        return Err(out.fail(EvalError::internal(COMPARISON_INTERNAL)));
    }
    for id in ids.iter().rev() {
        let field = evaluator.schema().field_info(*id, out.reborrow())?;
        let field = field
            .ok_or(EvalError::internal(COMPARISON_INTERNAL))
            .store_err_into(out.reborrow())?;
        evaluator.step(out.reborrow())?;
        let left_value = field_value(evaluator, left, field.name(), out.reborrow())?;
        let right_value = field_value(evaluator, right, field.name(), out.reborrow())?;
        EvaluationStack::push(
            tasks,
            Pair {
                left: EvalValue::config(left_value, field.ty(), false),
                right: EvalValue::config(right_value, field.ty(), false),
                equality_only: true,
                depth,
            },
            out.reborrow(),
        )?;
    }
    Ok(())
}

fn queue_array<'a, 'slot>(
    tasks: &mut ChargedVec<Pair<'a>>,
    request: ArrayComparisonRequest<'a>,
    mut out: Out<'slot, '_, EvalError>,
) -> OResult<'slot, bool> {
    let ArrayComparisonRequest {
        left,
        right,
        left_element,
        right_element,
        depth,
    } = request;
    if left.len() != right.len() {
        return Ok(false);
    }
    for (left_entry, right_entry) in left.zip(right).rev() {
        EvaluationStack::push(
            tasks,
            Pair {
                left: EvalValue::config(left_entry.value(), left_element, false),
                right: EvalValue::config(right_entry.value(), right_element, false),
                equality_only: true,
                depth,
            },
            out.reborrow(),
        )?;
    }
    Ok(true)
}

fn queue_map<'a, 'slot>(
    evaluator: &Evaluator<'_>,
    tasks: &mut ChargedVec<Pair<'a>>,
    request: MapComparisonRequest<'a>,
    mut out: Out<'slot, '_, EvalError>,
) -> OResult<'slot, bool> {
    let MapComparisonRequest {
        left,
        right,
        left_value,
        right_value,
        depth,
    } = request;
    if left.len() != right.len() {
        return Ok(false);
    }
    for left_entry in left.rev() {
        let left_name = left_entry
            .name()
            .ok_or(EvalError::internal(COMPARISON_INTERNAL))
            .store_err_into(out.reborrow())?;
        let mut found = None;
        let mut candidates = right;
        for candidate in &mut candidates {
            evaluator.step(out.reborrow())?;
            let candidate_name = candidate
                .name()
                .ok_or(EvalError::internal(COMPARISON_INTERNAL))
                .store_err_into(out.reborrow())?;
            if evaluator.compare_text(candidate_name, left_name, out.reborrow())? == Ordering::Equal
            {
                found = Some(candidate.value());
                break;
            }
        }
        let Some(right_entry) = found else {
            return Ok(false);
        };
        EvaluationStack::push(
            tasks,
            Pair {
                left: EvalValue::config(left_entry.value(), left_value, false),
                right: EvalValue::config(right_entry, right_value, false),
                equality_only: true,
                depth,
            },
            out.reborrow(),
        )?;
    }
    Ok(true)
}

fn queue_enum<'a, 'slot>(
    evaluator: &Evaluator<'_>,
    tasks: &mut ChargedVec<Pair<'a>>,
    request: EnumComparisonRequest<'a>,
    mut out: Out<'slot, '_, EvalError>,
) -> OResult<'slot, bool> {
    let EnumComparisonRequest {
        left,
        right,
        left_value,
        right_value,
        left_variant,
        right_variant,
        left_payload,
        right_payload,
        depth,
    } = request;
    let id = evaluator.variant_id(left.ty(), left_variant, out.reborrow())?;
    evaluator.variant_id(right.ty(), right_variant, out.reborrow())?;
    if evaluator.compare_text(left_variant, right_variant, out.reborrow())? != Ordering::Equal {
        return Ok(false);
    }
    let variant = evaluator.schema().variant_info(id, out.reborrow())?;
    let variant = variant
        .ok_or(EvalError::internal(COMPARISON_INTERNAL))
        .store_err_into(out.reborrow())?;
    match (variant.payload(), (left_payload, right_payload)) {
        (StoredPayloadInfo::Unit, (ValuePayloadView::Unit, ValuePayloadView::Unit)) => {}
        (
            StoredPayloadInfo::Single { ty },
            (ValuePayloadView::Single(left), ValuePayloadView::Single(right)),
        ) => {
            EvaluationStack::push(
                tasks,
                Pair {
                    left: EvalValue::config(left, ty, false),
                    right: EvalValue::config(right, ty, false),
                    equality_only: true,
                    depth,
                },
                out.reborrow(),
            )?;
        }
        (
            StoredPayloadInfo::Structure { fields: ids },
            (ValuePayloadView::Structure(left_entries), ValuePayloadView::Structure(right_entries)),
        ) => {
            if left_entries.len() != ids.len() || right_entries.len() != ids.len() {
                return Err(out.fail(EvalError::internal(COMPARISON_INTERNAL)));
            }
            queue_fields(
                evaluator,
                tasks,
                FieldComparisonRequest {
                    left: left_value,
                    right: right_value,
                    ids,
                    depth,
                },
                out.reborrow(),
            )?;
        }
        _ => return Err(out.fail(EvalError::internal(COMPARISON_INTERNAL))),
    }
    Ok(true)
}

pub struct ValueComparison;

pub struct ComparisonOptions {
    pub equality_only: bool,
    pub depth: usize,
}

impl ValueComparison {
    /// Explicit host comparison removes one trailing dot, compares IP addresses, and folds ASCII case otherwise.
    pub fn host_equal<'slot>(
        left: &str,
        right: &str,
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, bool> {
        let left = left.strip_suffix('.').unwrap_or(left);
        let right = right.strip_suffix('.').unwrap_or(right);
        match (
            TextValidation::parse_ipv4(left, out.reborrow())?,
            TextValidation::parse_ipv4(right, out.reborrow())?,
        ) {
            (Some(left), Some(right)) => return Ok(left == right),
            (Some(_), None) | (None, Some(_)) => return Ok(false),
            (None, None) => {}
        }
        match (
            TextValidation::parse_ipv6(left, out.reborrow())?,
            TextValidation::parse_ipv6(right, out.reborrow())?,
        ) {
            (Some(left), Some(right)) => Ok(left == right),
            (Some(_), None) | (None, Some(_)) => Ok(false),
            (None, None) => Ok(left.eq_ignore_ascii_case(right)),
        }
    }

    /// An explicit work stack preserves optional-value ordering and container equality semantics without using the host call stack.
    pub fn compare<'slot>(
        evaluator: &Evaluator<'_>,
        left: EvalValue<'_>,
        right: EvalValue<'_>,
        options: ComparisonOptions,
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, Ordering> {
        let mut tasks = ChargedVec::new(evaluator.budget(), out.reborrow())?;
        EvaluationStack::push(
            &mut tasks,
            Pair {
                left,
                right,
                equality_only: options.equality_only,
                depth: options.depth,
            },
            out.reborrow(),
        )?;
        while let Some(Pair {
            left,
            right,
            equality_only,
            depth,
        }) = tasks.pop()
        {
            evaluator.step(out.reborrow())?;
            if depth >= evaluator.limits().max_nesting_depth() {
                return Err(out.fail(EvalError::Resource));
            }
            evaluator.checked(left, false, out.reborrow())?;
            evaluator.checked(right, false, out.reborrow())?;
            if !evaluator
                .schema()
                .same_type(left.ty(), right.ty(), out.reborrow())?
            {
                return Err(out.fail(EvalError::internal(COMPARISON_INTERNAL)));
            }
            let info = evaluator.schema().type_info(left.ty(), out.reborrow())?;
            let info = info
                .ok_or(EvalError::internal(COMPARISON_INTERNAL))
                .store_err_into(out.reborrow())?;
            let kind = info.kind();
            let right_info = evaluator.schema().type_info(right.ty(), out.reborrow())?;
            let right_info = right_info
                .ok_or(EvalError::internal(COMPARISON_INTERNAL))
                .store_err_into(out.reborrow())?;
            let right_kind = right_info.kind();
            let next = depth
                .checked_add(1)
                .ok_or(EvalError::Resource)
                .store_err_into(out.reborrow())?;
            let order = match (left.kind(), right.kind()) {
                (EvalKind::Signed(left), EvalKind::Signed(right)) => left.cmp(&right),
                (EvalKind::Unsigned(left), EvalKind::Unsigned(right)) => left.cmp(&right),
                (EvalKind::Float(left), EvalKind::Float(right)) => left
                    .partial_cmp(&right)
                    .ok_or(EvalError::Arithmetic)
                    .store_err_into(out.reborrow())?,
                (EvalKind::Boolean(left), EvalKind::Boolean(right)) if equality_only => {
                    left.cmp(&right)
                }
                (EvalKind::String(a), EvalKind::String(b)) => {
                    evaluator.compare_text(a, b, out.reborrow())?
                }
                _ => {
                    if let StoredTypeKind::Option { inner } = kind {
                        let StoredTypeKind::Option { inner: right_inner } = right_kind else {
                            return Err(out.fail(EvalError::internal(COMPARISON_INTERNAL)));
                        };
                        match (
                            optional(left).store_err_into(out.reborrow())?,
                            optional(right).store_err_into(out.reborrow())?,
                        ) {
                            (None, None) => {}
                            (None, Some(_)) => return Ok(Ordering::Less),
                            (Some(_), None) => return Ok(Ordering::Greater),
                            (Some(a), Some(b)) => {
                                EvaluationStack::push(
                                    &mut tasks,
                                    Pair {
                                        left: EvalValue::config(a, inner, false),
                                        right: EvalValue::config(b, right_inner, false),
                                        equality_only,
                                        depth: next,
                                    },
                                    out.reborrow(),
                                )?;
                            }
                        }
                        continue;
                    }
                    if !equality_only {
                        return Err(out.fail(EvalError::internal(COMPARISON_INTERNAL)));
                    }
                    if let StoredTypeKind::Struct { fields: ids } = kind {
                        let left_fields = left.fields(out.reborrow())?;
                        let right_fields = right.fields(out.reborrow())?;
                        let (
                            ValueKind::Structure(left_entries),
                            ValueKind::Structure(right_entries),
                        ) = (left_fields.as_ref(), right_fields.as_ref())
                        else {
                            return Err(out.fail(EvalError::internal(COMPARISON_INTERNAL)));
                        };
                        if left_entries.len() != ids.len() || right_entries.len() != ids.len() {
                            return Err(out.fail(EvalError::internal(COMPARISON_INTERNAL)));
                        }
                        queue_fields(
                            evaluator,
                            &mut tasks,
                            FieldComparisonRequest {
                                left: left_fields,
                                right: right_fields,
                                ids,
                                depth: next,
                            },
                            out.reborrow(),
                        )?;
                        continue;
                    }
                    let left_value = left.value(out.reborrow())?;
                    let right_value = right.value(out.reborrow())?;
                    match (kind, (left_value.as_ref(), right_value.as_ref())) {
                        (
                            StoredTypeKind::Array { element, .. },
                            (ValueKind::Array(left_entries), ValueKind::Array(right_entries)),
                        ) => {
                            let StoredTypeKind::Array {
                                element: right_element,
                                ..
                            } = right_kind
                            else {
                                return Err(out.fail(EvalError::internal(COMPARISON_INTERNAL)));
                            };
                            if !queue_array(
                                &mut tasks,
                                ArrayComparisonRequest {
                                    left: left_entries,
                                    right: right_entries,
                                    left_element: element,
                                    right_element,
                                    depth: next,
                                },
                                out.reborrow(),
                            )? {
                                return Ok(Ordering::Greater);
                            }
                        }
                        (
                            StoredTypeKind::Map { value },
                            (ValueKind::Map(left_entries), ValueKind::Map(right_entries)),
                        ) => {
                            let StoredTypeKind::Map {
                                value: right_value_type,
                            } = right_kind
                            else {
                                return Err(out.fail(EvalError::internal(COMPARISON_INTERNAL)));
                            };
                            if !queue_map(
                                evaluator,
                                &mut tasks,
                                MapComparisonRequest {
                                    left: left_entries,
                                    right: right_entries,
                                    left_value: value,
                                    right_value: right_value_type,
                                    depth: next,
                                },
                                out.reborrow(),
                            )? {
                                return Ok(Ordering::Greater);
                            }
                        }
                        (
                            StoredTypeKind::Enum { .. },
                            (
                                ValueKind::Enumeration {
                                    variant: left_variant,
                                    payload: left_payload,
                                },
                                ValueKind::Enumeration {
                                    variant: right_variant,
                                    payload: right_payload,
                                },
                            ),
                        ) => {
                            if !queue_enum(
                                evaluator,
                                &mut tasks,
                                EnumComparisonRequest {
                                    left,
                                    right,
                                    left_value,
                                    right_value,
                                    left_variant,
                                    right_variant,
                                    left_payload,
                                    right_payload,
                                    depth: next,
                                },
                                out.reborrow(),
                            )? {
                                return Ok(Ordering::Greater);
                            }
                        }
                        _ => {
                            return Err(out.fail(EvalError::internal(COMPARISON_INTERNAL)));
                        }
                    }
                    continue;
                }
            };
            if order != Ordering::Equal {
                return Ok(order);
            }
        }
        Ok(Ordering::Equal)
    }
}
