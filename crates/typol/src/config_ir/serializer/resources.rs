use crate::{
    config_ir::{ConfigIrError, ConfigIrErrorKind},
    value_walk::ValueDepth,
};
use alloc::vec::Vec;
use core::{cell::Cell, fmt, fmt::Write as FmtWrite, str};
use serde::{Serialize, Serializer, ser::Error as SerError};
use typol_model::BuiltinType;

const INITIAL_BUFFER_CAPACITY: usize = 8;
const SIGNED_INTEGER_TEXT_MAX_BYTES: usize = 40;
const F64_SIGNIFICANT_DIGITS_MAX: usize = 17;
const F64_MIN_DECIMAL_EXPONENT_MAGNITUDE: usize = 324;
// The longest fixed-point form contains a sign, `0.`, 323 leading zeros, and 17 significant digits.
// The longest fixed form is a sign, `0.`, 323 leading zeros, and 17 significant digits.
const FLOAT_TEXT_MAX_BYTES: usize =
    1 + 2 + (F64_MIN_DECIMAL_EXPONENT_MAGNITUDE - 1) + F64_SIGNIFICANT_DIGITS_MAX;
const NUMBER_TEXT_CAPACITY: usize = if FLOAT_TEXT_MAX_BYTES > SIGNED_INTEGER_TEXT_MAX_BYTES {
    FLOAT_TEXT_MAX_BYTES
} else {
    SIGNED_INTEGER_TEXT_MAX_BYTES
};

pub(super) struct SerializationState {
    failure: Cell<Option<ConfigIrError>>,
    value_depth: Cell<usize>,
    type_depth: Cell<usize>,
    max_nesting_depth: usize,
    reject_secrets: bool,
}

pub(super) struct DepthReservation<'a> {
    depth: &'a Cell<usize>,
}

pub(super) struct WorkspaceBudget {
    used: Cell<usize>,
    limit: usize,
}

pub(super) struct WorkspaceReservation<'a> {
    budget: &'a WorkspaceBudget,
    bytes: usize,
}

pub(super) struct BoundedBuffer {
    bytes: Vec<u8>,
    limit: usize,
    failure: Option<ConfigIrError>,
}

struct NumberText {
    bytes: [u8; NUMBER_TEXT_CAPACITY],
    length: usize,
}

pub(super) struct Decimal<'a, T> {
    value: T,
    state: &'a SerializationState,
}

impl SerializationState {
    pub(super) const fn new(max_nesting_depth: usize, reject_secrets: bool) -> Self {
        Self {
            failure: Cell::new(None),
            value_depth: Cell::new(0),
            type_depth: Cell::new(0),
            max_nesting_depth,
            reject_secrets,
        }
    }

    pub(super) fn enter_value(&self) -> Result<DepthReservation<'_>, ConfigIrError> {
        reserve_depth(&self.value_depth, self.max_nesting_depth)
    }

    pub(super) fn enter_type(&self) -> Result<DepthReservation<'_>, ConfigIrError> {
        reserve_depth(&self.type_depth, self.max_nesting_depth)
    }

    pub(super) const fn rejects_secrets(&self) -> bool {
        self.reject_secrets
    }

    pub(super) fn take_failure(&self) -> Option<ConfigIrError> {
        self.failure.take()
    }

    pub(super) fn record(&self, error: ConfigIrError) {
        let failure = self.failure.take();
        if let Some(failure) = failure {
            self.failure.set(Some(failure));
        } else {
            self.failure.set(Some(error));
        }
    }
}

impl Drop for DepthReservation<'_> {
    fn drop(&mut self) {
        self.depth.set(self.depth.get().saturating_sub(1));
    }
}

fn reserve_depth(
    depth: &Cell<usize>,
    max_nesting_depth: usize,
) -> Result<DepthReservation<'_>, ConfigIrError> {
    let next_depth = ValueDepth::from_level(depth.get())
        .child()
        .map(ValueDepth::level)
        .ok_or(ConfigIrError::NestingLimitExceeded)?;
    if next_depth > max_nesting_depth {
        return Err(ConfigIrError::NestingLimitExceeded);
    }
    depth.set(next_depth);
    Ok(DepthReservation { depth })
}

pub(super) struct BuiltinNames;

impl BuiltinNames {
    pub(super) const fn name(builtin: BuiltinType) -> &'static str {
        match builtin {
            BuiltinType::I8 => "i8",
            BuiltinType::I16 => "i16",
            BuiltinType::I32 => "i32",
            BuiltinType::I64 => "i64",
            BuiltinType::I128 => "i128",
            BuiltinType::U8 => "u8",
            BuiltinType::U16 => "u16",
            BuiltinType::U32 => "u32",
            BuiltinType::U64 => "u64",
            BuiltinType::U128 => "u128",
            BuiltinType::F32 => "f32",
            BuiltinType::F64 => "f64",
            BuiltinType::String => "string",
            BuiltinType::Bool => "bool",
        }
    }
}

impl<'a, T> Decimal<'a, T> {
    pub(super) const fn new(value: T, state: &'a SerializationState) -> Self {
        Self { value, state }
    }
}

impl WorkspaceBudget {
    pub(super) const fn new(limit: usize) -> Self {
        Self {
            used: Cell::new(0),
            limit,
        }
    }

    pub(super) fn reserve(&self, bytes: usize) -> Result<WorkspaceReservation<'_>, ConfigIrError> {
        let next = self
            .used
            .get()
            .checked_add(bytes)
            .ok_or(ConfigIrError::WorkspaceLimitExceeded)?;
        if next > self.limit {
            return Err(ConfigIrError::WorkspaceLimitExceeded);
        }
        self.used.set(next);
        Ok(WorkspaceReservation {
            budget: self,
            bytes,
        })
    }

    fn set_used(&self, bytes: usize) {
        self.used.set(self.used.get().saturating_sub(bytes));
    }
}

impl Drop for WorkspaceReservation<'_> {
    fn drop(&mut self) {
        self.budget.set_used(self.bytes);
    }
}

impl BoundedBuffer {
    pub(super) fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
            failure: None,
        }
    }

    pub(super) fn take_failure(&mut self) -> Option<ConfigIrError> {
        self.failure.take()
    }

    pub(super) fn finish(&mut self) -> Result<(), ConfigIrErrorKind> {
        self.write_bytes(b"\n")
    }

    pub(super) fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    pub(super) fn write_bytes(&mut self, bytes: &[u8]) -> Result<(), ConfigIrErrorKind> {
        let Some(next_length) = self.bytes.len().checked_add(bytes.len()) else {
            return self.record_failure(ConfigIrError::OutputLimitExceeded);
        };
        if next_length > self.limit {
            return self.record_failure(ConfigIrError::OutputLimitExceeded);
        }
        if self.bytes.capacity() < next_length {
            let target_capacity = self
                .bytes
                .capacity()
                .saturating_mul(2)
                .max(INITIAL_BUFFER_CAPACITY)
                .min(self.limit)
                .max(next_length);
            let additional = target_capacity - self.bytes.len();
            if self.bytes.try_reserve_exact(additional).is_err() {
                return self.record_failure(ConfigIrError::AllocationFailed);
            }
        }
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }

    fn record_failure(&mut self, error: ConfigIrError) -> Result<(), ConfigIrErrorKind> {
        let kind = error.kind();
        if self.failure.is_none() {
            self.failure = Some(error);
        }
        Err(kind)
    }
}

impl NumberText {
    fn new() -> Self {
        Self {
            bytes: [0; NUMBER_TEXT_CAPACITY],
            length: 0,
        }
    }

    fn formatted<T: fmt::Display>(number: T) -> Option<Self> {
        let mut text = Self::new();
        write!(&mut text, "{number}").ok()?;
        Some(text)
    }

    fn as_str(&self) -> Option<&str> {
        str::from_utf8(self.bytes.get(..self.length)?).ok()
    }
}

impl FmtWrite for NumberText {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        let end = self.length.checked_add(value.len()).ok_or(fmt::Error)?;
        let Some(bytes) = self.bytes.get_mut(self.length..end) else {
            return Err(fmt::Error);
        };
        bytes.copy_from_slice(value.as_bytes());
        self.length = end;
        Ok(())
    }
}

impl<T> Serialize for Decimal<'_, T>
where
    T: fmt::Display + Copy,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let text = NumberText::formatted(self.value).ok_or_else(|| {
            self.state.record(ConfigIrError::SerializationFailed);
            S::Error::custom("config IR serialization failed")
        })?;
        let text = text.as_str().ok_or_else(|| {
            self.state.record(ConfigIrError::SerializationFailed);
            S::Error::custom("config IR serialization failed")
        })?;
        serializer.serialize_str(text)
    }
}

#[cfg(test)]
#[test]
fn number_text_capacity_covers_integer_and_float_display_extremes() {
    let signed_min = i128::MIN.to_string();
    let unsigned_max = u128::MAX.to_string();
    let f32_max = f32::MAX.to_string();
    let f32_subnormal = f32::from_bits(1).to_string();
    let f64_max = f64::MAX.to_string();
    let f64_subnormal = f64::from_bits(1).to_string();
    let f64_negative_subnormal = (-f64::from_bits(1)).to_string();

    assert!(NUMBER_TEXT_CAPACITY >= signed_min.len());
    assert!(NUMBER_TEXT_CAPACITY >= unsigned_max.len());
    assert!(NUMBER_TEXT_CAPACITY >= f32_max.len());
    assert!(NUMBER_TEXT_CAPACITY >= f32_subnormal.len());
    assert!(NUMBER_TEXT_CAPACITY >= f64_max.len());
    assert!(NUMBER_TEXT_CAPACITY >= f64_subnormal.len());
    assert!(NUMBER_TEXT_CAPACITY >= f64_negative_subnormal.len());
    assert_eq!(
        NumberText::formatted(i128::MIN).and_then(|text| text.as_str().map(str::to_owned)),
        Some(signed_min)
    );
    assert_eq!(
        NumberText::formatted(u128::MAX).and_then(|text| text.as_str().map(str::to_owned)),
        Some(unsigned_max)
    );
    assert_eq!(
        NumberText::formatted(f32::MAX).and_then(|text| text.as_str().map(str::to_owned)),
        Some(f32_max)
    );
    assert_eq!(
        NumberText::formatted(f32::from_bits(1)).and_then(|text| text.as_str().map(str::to_owned)),
        Some(f32_subnormal)
    );
    assert_eq!(
        NumberText::formatted(f64::MAX).and_then(|text| text.as_str().map(str::to_owned)),
        Some(f64_max)
    );
    assert_eq!(
        NumberText::formatted(f64::from_bits(1)).and_then(|text| text.as_str().map(str::to_owned)),
        Some(f64_subnormal)
    );
    assert_eq!(
        NumberText::formatted(-f64::from_bits(1)).and_then(|text| text.as_str().map(str::to_owned)),
        Some(f64_negative_subnormal)
    );
}

#[cfg(test)]
#[test]
fn short_writes_grow_geometrically_with_bounded_capacity() {
    let limit = 4_096;
    let mut buffer = BoundedBuffer::new(limit);
    let mut previous_capacity = buffer.bytes.capacity();
    let mut capacity_growths = 0;
    let mut moved_bytes_upper_bound = 0;

    for _ in 0..limit {
        let previous_length = buffer.bytes.len();
        assert!(buffer.write_bytes(b"x").is_ok());
        let capacity = buffer.bytes.capacity();
        assert!(capacity <= limit);
        if capacity > previous_capacity {
            capacity_growths += 1;
            moved_bytes_upper_bound += previous_length;
            previous_capacity = capacity;
        }
    }

    assert_eq!(buffer.bytes.len(), limit);
    assert!(capacity_growths <= 10);
    assert!(moved_bytes_upper_bound < limit);
}

#[cfg(test)]
#[test]
fn output_limit_caps_growth_and_rejects_the_next_byte() {
    let mut buffer = BoundedBuffer::new(10);
    assert!(buffer.write_bytes(b"12345678").is_ok());
    assert!(buffer.write_bytes(b"90").is_ok());
    assert!(buffer.bytes.capacity() <= 10);
    assert!(buffer.write_bytes(b"x").is_err());
    assert!(matches!(
        buffer.take_failure(),
        Some(ConfigIrError::OutputLimitExceeded)
    ));
    assert_eq!(buffer.bytes.as_slice(), b"1234567890");
}
