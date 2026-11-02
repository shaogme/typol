use crate::{EvalError, EvalKind};
use core::cmp::Ordering;
use opeo::{OResult, Out, ResultOutExt};
use typol_core::{DiagnosticContext, ExecutionBudget, ValueEntries, ValueKind, diagnostic_context};
use typol_model::{BinaryOperator, BuiltinType};

static NUMERIC_INTERNAL: DiagnosticContext = diagnostic_context!(
    "typol-semantic",
    "numeric-evaluation",
    "numeric-kind-or-width-mismatch",
);

struct WideUnsigned([u64; 3]);

impl WideUnsigned {
    const fn zero() -> Self {
        Self([0; 3])
    }

    fn add(&mut self, value: u128) {
        let words = [value as u64, (value >> 64) as u64, 0];
        add_words(&mut self.0, words);
    }

    fn fits_unsigned(&self, builtin: BuiltinType) -> bool {
        let Some(maximum) = Numeric::unsigned_max(builtin) else {
            return false;
        };
        let [low, high, overflow] = self.0;
        overflow == 0 && (u128::from(high) << 64 | u128::from(low)) <= maximum
    }

    fn magnitude_u128(&self) -> Option<u128> {
        let [low, high, overflow] = self.0;
        (overflow == 0).then_some(u128::from(high) << 64 | u128::from(low))
    }
}

struct WideSigned {
    negative: bool,
    magnitude: [u64; 3],
}

impl WideSigned {
    const fn zero() -> Self {
        Self {
            negative: false,
            magnitude: [0; 3],
        }
    }

    fn add(&mut self, value: i128) {
        let negative = value < 0;
        let magnitude = value.unsigned_abs();
        let words = [magnitude as u64, (magnitude >> 64) as u64, 0];
        if self.magnitude == [0; 3] {
            self.magnitude = words;
            self.negative = negative && magnitude != 0;
            return;
        }
        if self.negative == negative {
            add_words(&mut self.magnitude, words);
        } else {
            match cmp_words(&self.magnitude, &words) {
                Ordering::Greater | Ordering::Equal => {
                    sub_words(&mut self.magnitude, words);
                    if self.magnitude == [0; 3] {
                        self.negative = false;
                    }
                }
                Ordering::Less => {
                    let mut result = words;
                    sub_words(&mut result, self.magnitude);
                    self.magnitude = result;
                    self.negative = negative;
                }
            }
        }
    }

    fn fits_signed(&self, builtin: BuiltinType) -> bool {
        let Some((minimum, maximum)) = Numeric::signed_bounds(builtin) else {
            return false;
        };
        let Some(magnitude) = self.magnitude_u128() else {
            return false;
        };
        if self.negative {
            magnitude <= minimum.unsigned_abs()
        } else {
            magnitude <= maximum as u128
        }
    }

    fn magnitude_u128(&self) -> Option<u128> {
        let [low, high, overflow] = self.magnitude;
        (overflow == 0).then_some(u128::from(high) << 64 | u128::from(low))
    }
}

fn add_words(target: &mut [u64; 3], value: [u64; 3]) {
    let mut carry = 0_u128;
    for (target, value) in target.iter_mut().zip(value) {
        let sum = u128::from(*target) + u128::from(value) + carry;
        *target = sum as u64;
        carry = sum >> 64;
    }
}

fn sub_words(target: &mut [u64; 3], value: [u64; 3]) {
    let mut borrow = 0_u128;
    for (target, value) in target.iter_mut().zip(value) {
        let left = u128::from(*target);
        let right = u128::from(value) + borrow;
        *target = left.wrapping_sub(right) as u64;
        borrow = u128::from(left < right);
    }
}

fn cmp_words(left: &[u64; 3], right: &[u64; 3]) -> Ordering {
    for (left, right) in left.iter().rev().zip(right.iter().rev()) {
        match left.cmp(right) {
            Ordering::Equal => {}
            ordering => return ordering,
        }
    }
    Ordering::Equal
}

pub struct Numeric;

impl Numeric {
    pub fn signed_bounds(builtin: BuiltinType) -> Option<(i128, i128)> {
        Some(match builtin {
            BuiltinType::I8 => (i8::MIN as i128, i8::MAX as i128),
            BuiltinType::I16 => (i16::MIN as i128, i16::MAX as i128),
            BuiltinType::I32 => (i32::MIN as i128, i32::MAX as i128),
            BuiltinType::I64 => (i64::MIN as i128, i64::MAX as i128),
            BuiltinType::I128 => (i128::MIN, i128::MAX),
            _ => return None,
        })
    }

    pub fn unsigned_max(builtin: BuiltinType) -> Option<u128> {
        Some(match builtin {
            BuiltinType::U8 => u8::MAX as u128,
            BuiltinType::U16 => u16::MAX as u128,
            BuiltinType::U32 => u32::MAX as u128,
            BuiltinType::U64 => u64::MAX as u128,
            BuiltinType::U128 => u128::MAX,
            _ => return None,
        })
    }

    pub fn narrow_signed(value: i128, builtin: BuiltinType) -> Option<i128> {
        let (min, max) = Numeric::signed_bounds(builtin)?;
        (value >= min && value <= max).then_some(value)
    }

    pub fn narrow_unsigned(value: u128, builtin: BuiltinType) -> Option<u128> {
        (value <= Numeric::unsigned_max(builtin)?).then_some(value)
    }

    pub fn finite_float(value: f64, builtin: BuiltinType) -> Option<f64> {
        if !value.is_finite() {
            return None;
        }
        if builtin == BuiltinType::F32 {
            let value = value as f32;
            value.is_finite().then_some(f64::from(value))
        } else if builtin == BuiltinType::F64 {
            Some(value)
        } else {
            None
        }
    }

    /// When `usize` is at most 64 bits, the element count times the maximum `u128` magnitude is strictly less than 2^192.
    /// Three `u64` words hold the complete intermediate sum without allocation or truncation to the target width during traversal.
    pub fn sum<'slot>(
        values: ValueEntries<'_>,
        builtin: BuiltinType,
        budget: Option<&ExecutionBudget>,
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, EvalKind<'static>> {
        if Numeric::signed_bounds(builtin).is_some() {
            let mut sum = WideSigned::zero();
            for entry in values {
                if let Some(budget) = budget {
                    budget.consume(1, out.reborrow())?;
                }
                let value = entry.value();
                let ValueKind::Signed(value) = value.as_ref() else {
                    return Err(out.fail(EvalError::internal(NUMERIC_INTERNAL)));
                };
                Numeric::narrow_signed(value, builtin)
                    .ok_or(EvalError::internal(NUMERIC_INTERNAL))
                    .store_err_into(out.reborrow())?;
                sum.add(value);
            }
            if !sum.fits_signed(builtin) {
                return Err(out.fail(EvalError::Arithmetic));
            }
            let magnitude = sum
                .magnitude_u128()
                .ok_or(EvalError::Arithmetic)
                .store_err_into(out.reborrow())?;
            let value = if sum.negative {
                if magnitude == 1_u128 << 127 {
                    i128::MIN
                } else {
                    -i128::try_from(magnitude)
                        .map_err(|_| EvalError::Arithmetic)
                        .store_err_into(out.reborrow())?
                }
            } else {
                i128::try_from(magnitude)
                    .map_err(|_| EvalError::Arithmetic)
                    .store_err_into(out.reborrow())?
            };
            Ok(EvalKind::Signed(value))
        } else if Numeric::unsigned_max(builtin).is_some() {
            let mut sum = WideUnsigned::zero();
            for entry in values {
                if let Some(budget) = budget {
                    budget.consume(1, out.reborrow())?;
                }
                let value = entry.value();
                let ValueKind::Unsigned(value) = value.as_ref() else {
                    return Err(out.fail(EvalError::internal(NUMERIC_INTERNAL)));
                };
                Numeric::narrow_unsigned(value, builtin)
                    .ok_or(EvalError::internal(NUMERIC_INTERNAL))
                    .store_err_into(out.reborrow())?;
                sum.add(value);
            }
            if !sum.fits_unsigned(builtin) {
                return Err(out.fail(EvalError::Arithmetic));
            }
            Ok(EvalKind::Unsigned(
                sum.magnitude_u128()
                    .ok_or(EvalError::Arithmetic)
                    .store_err_into(out.reborrow())?,
            ))
        } else if matches!(builtin, BuiltinType::F32 | BuiltinType::F64) {
            let mut sum = 0.0;
            for entry in values {
                if let Some(budget) = budget {
                    budget.consume(1, out.reborrow())?;
                }
                let value = entry.value();
                let value = match (builtin, value.as_ref()) {
                    (BuiltinType::F32, ValueKind::Float32(value)) => f64::from(value),
                    (BuiltinType::F64, ValueKind::Float64(value)) => value,
                    _ => {
                        return Err(out.fail(EvalError::internal(NUMERIC_INTERNAL)));
                    }
                };
                sum = Numeric::finite_float(sum + value, builtin)
                    .ok_or(EvalError::Arithmetic)
                    .store_err_into(out.reborrow())?;
            }
            Ok(EvalKind::Float(sum))
        } else {
            Err(out.fail(EvalError::internal(NUMERIC_INTERNAL)))
        }
    }

    pub fn arithmetic<'slot>(
        operator: BinaryOperator,
        left: EvalKind<'_>,
        right: EvalKind<'_>,
        builtin: BuiltinType,
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, EvalKind<'static>> {
        let result = match (left, right) {
            (EvalKind::Signed(left), EvalKind::Signed(right))
                if Numeric::signed_bounds(builtin).is_some() =>
            {
                let value = match operator {
                    BinaryOperator::Add => left.checked_add(right),
                    BinaryOperator::Subtract => left.checked_sub(right),
                    BinaryOperator::Multiply => left.checked_mul(right),
                    BinaryOperator::Divide => left.checked_div(right),
                    _ => {
                        return Err(out.fail(EvalError::internal(NUMERIC_INTERNAL)));
                    }
                }
                .and_then(|value| Numeric::narrow_signed(value, builtin))
                .ok_or(EvalError::Arithmetic);
                let value = value.store_err_into(out.reborrow())?;
                EvalKind::Signed(value)
            }
            (EvalKind::Unsigned(left), EvalKind::Unsigned(right))
                if Numeric::unsigned_max(builtin).is_some() =>
            {
                let value = match operator {
                    BinaryOperator::Add => left.checked_add(right),
                    BinaryOperator::Subtract => left.checked_sub(right),
                    BinaryOperator::Multiply => left.checked_mul(right),
                    BinaryOperator::Divide => left.checked_div(right),
                    _ => {
                        return Err(out.fail(EvalError::internal(NUMERIC_INTERNAL)));
                    }
                }
                .and_then(|value| Numeric::narrow_unsigned(value, builtin))
                .ok_or(EvalError::Arithmetic);
                let value = value.store_err_into(out.reborrow())?;
                EvalKind::Unsigned(value)
            }
            (EvalKind::Float(left), EvalKind::Float(right))
                if matches!(builtin, BuiltinType::F32 | BuiltinType::F64) =>
            {
                let value = if builtin == BuiltinType::F32 {
                    let left = left as f32;
                    let right = right as f32;
                    f64::from(match operator {
                        BinaryOperator::Add => left + right,
                        BinaryOperator::Subtract => left - right,
                        BinaryOperator::Multiply => left * right,
                        BinaryOperator::Divide if right != 0.0 => left / right,
                        BinaryOperator::Divide => {
                            return Err(out.fail(EvalError::Arithmetic));
                        }
                        _ => {
                            return Err(out.fail(EvalError::internal(NUMERIC_INTERNAL)));
                        }
                    })
                } else {
                    match operator {
                        BinaryOperator::Add => left + right,
                        BinaryOperator::Subtract => left - right,
                        BinaryOperator::Multiply => left * right,
                        BinaryOperator::Divide if right != 0.0 => left / right,
                        BinaryOperator::Divide => {
                            return Err(out.fail(EvalError::Arithmetic));
                        }
                        _ => {
                            return Err(out.fail(EvalError::internal(NUMERIC_INTERNAL)));
                        }
                    }
                };
                EvalKind::Float(
                    Numeric::finite_float(value, builtin)
                        .ok_or(EvalError::Arithmetic)
                        .store_err_into(out.reborrow())?,
                )
            }
            _ => return Err(out.fail(EvalError::internal(NUMERIC_INTERNAL))),
        };
        Ok(result)
    }
}
