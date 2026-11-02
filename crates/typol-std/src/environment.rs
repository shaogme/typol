use std::{env::var_os, ffi::OsString, fmt};

use opeo::{ErrSlot, OResult, Out};
use typol_config::{Environment, EnvironmentError, EnvironmentReadContext};
use typol_core::{BudgetError, ChargedVec, ExecutionBudget};

use crate::ProcessEnvironmentLimits;

#[cfg(unix)]
use std::os::unix::ffi::OsStringExt;

#[cfg(windows)]
use opeo::ResultOutExt;

#[cfg(windows)]
use std::{char::from_u32, os::windows::ffi::OsStrExt};
#[derive(Clone, Copy)]
pub(crate) enum EnvironmentFailure {
    Budget(BudgetError),
    Host,
}

pub(crate) struct EnvironmentBytes {
    bytes: ChargedVec<u8>,
}

/// Looks up process-environment values by name without enumerating, modifying, or caching the environment.
///
/// Unix preserves raw bytes for configuration-layer UTF-8 checks; Windows rejects unpaired surrogates.
/// Missing and empty values remain distinct, and values are neither trimmed nor parsed as scalars.
/// Name processing, platform conversion, and retained values share the config-call budget; Unix
/// adopts and charges the buffer returned by `var_os`, while Windows charges its converted output.
/// `var_os` returns an owned `OsString` before its length is checked, so its allocation is outside
/// this budget and recoverable OOM control. Separate reads need not share a snapshot.
pub struct ProcessEnvironment {
    limits: ProcessEnvironmentLimits,
    current: Option<EnvironmentBytes>,
    failures: ErrSlot<EnvironmentFailure>,
}

impl EnvironmentFailure {
    const fn public_error(self) -> EnvironmentError {
        match self {
            Self::Budget(BudgetError::Resource) => EnvironmentError::ResourceLimitExceeded,
            Self::Budget(BudgetError::Allocation) => EnvironmentError::AllocationFailed,
            Self::Budget(BudgetError::CapacityExceeded) => EnvironmentError::CapacityExceeded,
            Self::Budget(BudgetError::InvariantViolation(_)) => EnvironmentError::InternalError,
            Self::Host => EnvironmentError::HostFailure,
        }
    }
}

impl From<BudgetError> for EnvironmentFailure {
    fn from(error: BudgetError) -> Self {
        Self::Budget(error)
    }
}

impl ProcessEnvironment {
    pub const fn new(limits: ProcessEnvironmentLimits) -> Self {
        Self {
            limits,
            current: None,
            failures: ErrSlot::new(),
        }
    }

    fn validate_name<'slot>(
        limits: ProcessEnvironmentLimits,
        name: &str,
        budget: &ExecutionBudget,
        mut out: Out<'slot, '_, EnvironmentFailure>,
    ) -> OResult<'slot, ()> {
        if name.len() > limits.max_environment_name_bytes() {
            return Err(out.fail(EnvironmentFailure::Budget(BudgetError::Resource)));
        }
        if name.is_empty() {
            return Err(out.fail(EnvironmentFailure::Host));
        }
        for byte in name.bytes() {
            budget.consume(1, out.reborrow())?;
            if byte == 0 || byte == b'=' {
                return Err(out.fail(EnvironmentFailure::Host));
            }
        }
        Ok(())
    }
}

impl Environment for ProcessEnvironment {
    fn read<'env, 'slot, 'borrow>(
        &'env mut self,
        name: &str,
        context: EnvironmentReadContext<'_>,
        out: Out<'slot, 'borrow, EnvironmentError>,
    ) -> OResult<'slot, Option<&'env [u8]>> {
        self.current = None;
        let budget = context.execution();
        let limits = self.limits;
        if let Err(caught) = self
            .failures
            .try_call(|inner| Self::validate_name(limits, name, budget, inner))
        {
            return Self::fail_environment(out, *caught.get());
        }
        if let Err(caught) = self.failures.try_call(|inner| budget.consume(1, inner)) {
            return Self::fail_environment(out, *caught.get());
        }
        if let Some(value) = var_os(name) {
            let max_value_bytes = limits
                .max_environment_value_bytes()
                .min(context.max_value_bytes());
            match self.failures.try_call(|inner| {
                EnvironmentBytes::from_native(value, max_value_bytes, budget, inner)
            }) {
                Ok(value) => self.current = Some(value),
                Err(caught) => return Self::fail_environment(out, *caught.get()),
            }
        }
        Ok(self.current.as_ref().map(|value| value.bytes.as_slice()))
    }
}

impl ProcessEnvironment {
    fn fail_environment<'env, 'slot, 'borrow>(
        out: Out<'slot, 'borrow, EnvironmentError>,
        error: EnvironmentFailure,
    ) -> OResult<'slot, Option<&'env [u8]>> {
        Err(out.fail(error.public_error()))
    }
}

impl fmt::Debug for ProcessEnvironment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProcessEnvironment")
            .finish_non_exhaustive()
    }
}

impl EnvironmentBytes {
    #[cfg(unix)]
    pub(crate) fn from_native<'slot>(
        value: OsString,
        limit: usize,
        budget: &ExecutionBudget,
        mut out: Out<'slot, '_, EnvironmentFailure>,
    ) -> OResult<'slot, Self> {
        let native = value.into_vec();
        budget.consume(1, out.reborrow())?;
        if native.len() > limit {
            return Err(out.fail(EnvironmentFailure::Budget(BudgetError::Resource)));
        }
        let bytes = ChargedVec::try_from_vec_with_budget(native, budget, out.reborrow())?;
        Ok(Self { bytes })
    }

    #[cfg(windows)]
    pub(crate) fn from_native<'slot>(
        value: OsString,
        limit: usize,
        budget: &ExecutionBudget,
        mut out: Out<'slot, '_, EnvironmentFailure>,
    ) -> OResult<'slot, Self> {
        let mut length = 0usize;
        let mut units = value.encode_wide();
        while let Some(scalar) =
            Self::scalar(&mut units, budget, out.reborrow()).store_err_into(out.reborrow())?
        {
            length = length
                .checked_add(scalar.len_utf8())
                .filter(|length| *length <= limit)
                .ok_or(EnvironmentFailure::Budget(BudgetError::Resource))
                .store_err_into(out.reborrow())?;
        }
        let mut output =
            ChargedVec::new(Some(budget), out.reborrow()).store_err_into(out.reborrow())?;
        output
            .try_reserve_exact(length, out.reborrow())
            .store_err_into(out.reborrow())?;
        let mut units = value.encode_wide();
        while let Some(scalar) =
            Self::scalar(&mut units, budget, out.reborrow()).store_err_into(out.reborrow())?
        {
            let mut encoded = [0; 4];
            output
                .extend_from_slice(scalar.encode_utf8(&mut encoded).as_bytes(), out.reborrow())
                .store_err_into(out.reborrow())?;
        }
        Ok(Self { bytes: output })
    }

    #[cfg(windows)]
    fn scalar<'slot>(
        units: &mut impl Iterator<Item = u16>,
        budget: &ExecutionBudget,
        mut out: Out<'slot, '_, EnvironmentFailure>,
    ) -> OResult<'slot, Option<char>> {
        let Some(first) = units.next() else {
            return Ok(None);
        };
        budget
            .consume(1, out.reborrow())
            .store_err_into(out.reborrow())?;
        let point = match first {
            0xd800..=0xdbff => {
                let second = units
                    .next()
                    .ok_or(EnvironmentFailure::Host)
                    .store_err_into(out.reborrow())?;
                budget
                    .consume(1, out.reborrow())
                    .store_err_into(out.reborrow())?;
                if !(0xdc00..=0xdfff).contains(&second) {
                    return Err(out.fail(EnvironmentFailure::Host));
                }
                0x10000 + ((u32::from(first) - 0xd800) << 10) + u32::from(second) - 0xdc00
            }
            0xdc00..=0xdfff => return Err(out.fail(EnvironmentFailure::Host)),
            _ => u32::from(first),
        };
        let scalar = from_u32(point)
            .map(Some)
            .ok_or(EnvironmentFailure::Host)
            .store_err_into(out.reborrow())?;
        Ok(scalar)
    }

    #[cfg(not(any(unix, windows)))]
    pub(crate) fn from_native<'slot, 'borrow>(
        _: OsString,
        _: usize,
        _: &ExecutionBudget,
        out: Out<'slot, 'borrow, EnvironmentFailure>,
    ) -> OResult<'slot, Self> {
        Err(out.fail(EnvironmentFailure::Host))
    }
}

#[cfg(test)]
mod tests {
    use std::{error::Error, ffi::OsString, io::Error as IoError};

    use opeo::{ErrSlot, OResult, Out};
    use typol_core::{BudgetError, ExecutionBudget, ExecutionLimits};

    use crate::environment::{EnvironmentBytes, EnvironmentFailure};

    #[cfg(unix)]
    use std::os::unix::ffi::OsStringExt;

    #[cfg(windows)]
    use std::os::windows::ffi::OsStringExt;

    fn call_opeo<T, E>(
        operation: impl for<'slot> FnOnce(Out<'slot, 'slot, E>) -> OResult<'slot, T>,
    ) -> Result<T, E> {
        let mut errors = ErrSlot::<E>::new();
        match errors.try_call(operation) {
            Ok(value) => Ok(value),
            Err(caught) => Err(caught.take()),
        }
    }

    macro_rules! ensure {
        ($condition:expr $(,)?) => {
            if !$condition {
                return Err(IoError::other(concat!("assertion failed: ", stringify!($condition))).into());
            }
        };
        ($condition:expr, $($argument:tt)+) => {
            if !$condition {
                return Err(IoError::other(format!($($argument)+)).into());
            }
        };
    }

    macro_rules! ensure_eq {
        ($left:expr, $right:expr $(,)?) => {{
            let left = &$left;
            let right = &$right;
            if left != right {
                return Err(IoError::other(format!(
                    "assertion failed: {} and {} differ (actual {:?}, expected {:?})",
                    stringify!($left),
                    stringify!($right),
                    left,
                    right,
                ))
                .into());
            }
        }};
    }

    #[cfg(unix)]
    #[test]
    fn transferred_native_capacity_and_steps_are_bounded() -> Result<(), Box<dyn Error>> {
        let baseline = call_opeo(|out| ExecutionBudget::new(ExecutionLimits::new(100, 4096), out))?;
        let overhead = baseline.workspace_bytes();
        let mut raw = Vec::with_capacity(128);
        raw.extend_from_slice(b"x");
        let capacity = raw.capacity();
        let budget = call_opeo(|out| {
            ExecutionBudget::new(ExecutionLimits::new(2, overhead + capacity - 1), out)
        })?;
        ensure!(matches!(
            call_opeo(|out| EnvironmentBytes::from_native(
                OsString::from_vec(raw),
                1,
                &budget,
                out,
            )),
            Err(EnvironmentFailure::Budget(BudgetError::Resource))
        ));
        ensure_eq!(budget.workspace_bytes(), overhead);

        let mut raw = Vec::with_capacity(128);
        raw.extend_from_slice(b"x");
        let capacity = raw.capacity();
        let budget = call_opeo(|out| {
            ExecutionBudget::new(ExecutionLimits::new(2, overhead + capacity), out)
        })?;
        let result = call_opeo(|out| {
            EnvironmentBytes::from_native(OsString::from_vec(raw), 1, &budget, out)
        });
        let value = result
            .as_ref()
            .map_err(|_| IoError::other("environment variable read failed"))?;
        ensure_eq!(value.bytes.capacity(), capacity);
        ensure_eq!(budget.workspace_bytes(), overhead + value.bytes.capacity());
        ensure_eq!(budget.steps(), 1);
        ensure_eq!(value.bytes.as_slice(), b"x");
        drop(result);
        ensure_eq!(budget.workspace_bytes(), overhead);

        let budget = call_opeo(|out| ExecutionBudget::new(ExecutionLimits::new(100, 4096), out))?;
        let overhead = budget.workspace_bytes();
        let mut raw = Vec::with_capacity(128);
        raw.extend_from_slice(b"xy");
        ensure!(matches!(
            call_opeo(|out| EnvironmentBytes::from_native(
                OsString::from_vec(raw),
                1,
                &budget,
                out,
            )),
            Err(EnvironmentFailure::Budget(BudgetError::Resource))
        ));
        ensure_eq!(budget.workspace_bytes(), overhead);
        let budget = call_opeo(|out| ExecutionBudget::new(ExecutionLimits::new(0, 4096), out))?;
        ensure!(matches!(
            call_opeo(|out| EnvironmentBytes::from_native(OsString::from("x"), 1, &budget, out,)),
            Err(EnvironmentFailure::Budget(BudgetError::Resource))
        ));
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn wide_scalars_reject_unpaired_surrogates_without_partial_output() -> Result<(), Box<dyn Error>>
    {
        for units in [
            &[0xd800][..],
            &[0xdc00][..],
            &[b'a' as u16, 0xd800, b'b' as u16][..],
        ] {
            let budget =
                call_opeo(|out| ExecutionBudget::new(ExecutionLimits::new(100, 4096), out))?;
            let overhead = budget.workspace_bytes();
            let result = call_opeo(|out| {
                EnvironmentBytes::from_native(OsString::from_wide(units), 100, &budget, out)
            });
            ensure!(matches!(result, Err(EnvironmentFailure::Host)));
            ensure_eq!(budget.workspace_bytes(), overhead);
        }
        let budget = call_opeo(|out| ExecutionBudget::new(ExecutionLimits::new(100, 4096), out))?;
        let value = call_opeo(|out| {
            EnvironmentBytes::from_native(
                OsString::from_wide(&[0x754c, 0xd83d, 0xde00]),
                7,
                &budget,
                out,
            )
        })?;
        ensure_eq!(value.bytes.as_slice(), "Ḁ😀".as_bytes());
        Ok(())
    }
}
