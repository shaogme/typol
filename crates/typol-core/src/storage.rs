use crate::{
    BudgetError, DiagnosticContext, ExecutionBudget, RegexExecutionBudget, diagnostic_context,
    workspace::WorkspaceVec,
};
use alloc::{
    alloc::{Layout, alloc},
    boxed::Box,
};
use core::{
    ops::{Deref, DerefMut},
    str::{Utf8Error, from_utf8, from_utf8_unchecked},
};
use opeo::{OResult, Out, ResultOutExt};

static CHARGE_RELEASE_UNDERFLOW: DiagnosticContext =
    diagnostic_context!("typol-core", "workspace-charge", "release-underflow");
static CHARGE_BUDGET_RELEASE: DiagnosticContext =
    diagnostic_context!("typol-core", "workspace-charge", "budget-release-failure");
static CHARGE_BUDGET_MISMATCH: DiagnosticContext =
    diagnostic_context!("typol-core", "workspace-charge", "budget-identity-mismatch");
static CHARGE_DROP_RELEASE: DiagnosticContext =
    diagnostic_context!("typol-core", "workspace-charge", "drop-release-failure");

/// Independent capacity charges carried by owned objects such as lexical tokens, display names, and diagnostics.
pub struct WorkspaceCharge {
    budget: Option<ExecutionBudget>,
    bytes: usize,
}

impl WorkspaceCharge {
    pub const fn unmetered() -> Self {
        Self {
            budget: None,
            bytes: 0,
        }
    }

    pub(crate) const fn unmetered_with_bytes(bytes: usize) -> Self {
        Self {
            budget: None,
            bytes,
        }
    }

    pub fn new<'slot, E>(
        budget: Option<&ExecutionBudget>,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, Self>
    where
        E: From<BudgetError>,
    {
        let budget = match budget {
            Some(budget) => Some(budget.try_clone(out.reborrow())?),
            None => None,
        };
        Ok(Self { budget, bytes: 0 })
    }

    pub(crate) fn new_regex<'slot, E>(
        budget: Option<&RegexExecutionBudget>,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, Self>
    where
        E: From<BudgetError>,
    {
        let budget = match budget {
            Some(budget) => Some(budget.execution().try_clone(out.reborrow())?),
            None => None,
        };
        Ok(Self { budget, bytes: 0 })
    }

    pub(crate) fn new_optional(budget: Option<&ExecutionBudget>) -> Option<Self> {
        let budget = match budget {
            Some(budget) => Some(budget.try_clone_optional()?),
            None => None,
        };
        Some(Self { budget, bytes: 0 })
    }

    pub fn add<'slot, E>(&mut self, bytes: usize, mut out: Out<'slot, '_, E>) -> OResult<'slot, ()>
    where
        E: From<BudgetError>,
    {
        let next = self
            .bytes
            .checked_add(bytes)
            .ok_or(BudgetError::CapacityExceeded)
            .store_err_into(out.reborrow())?;
        if let Some(budget) = &self.budget {
            budget.reserve_workspace(bytes, out.reborrow())?;
        }
        self.bytes = next;
        Ok(())
    }

    pub(crate) fn add_optional(&mut self, bytes: usize) -> bool {
        let Some(next) = self.bytes.checked_add(bytes) else {
            return false;
        };
        if self
            .budget
            .as_ref()
            .is_some_and(|budget| !budget.reserve_optional_workspace(bytes))
        {
            return false;
        }
        self.bytes = next;
        true
    }

    pub(crate) fn release_optional(&mut self, bytes: usize) -> bool {
        let Some(next) = self.bytes.checked_sub(bytes) else {
            if let Some(budget) = &self.budget {
                budget.record_failure(BudgetError::invariant(&CHARGE_RELEASE_UNDERFLOW));
            }
            return false;
        };
        self.bytes = next;
        if let Some(budget) = &self.budget {
            if !budget.return_workspace_optional(bytes) {
                budget.record_failure(BudgetError::invariant(&CHARGE_BUDGET_RELEASE));
                return false;
            }
        }
        true
    }

    pub(crate) fn release<'slot, E>(
        &mut self,
        bytes: usize,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, ()>
    where
        E: From<BudgetError>,
    {
        let Some(next) = self.bytes.checked_sub(bytes) else {
            return Err(out.fail(E::from(BudgetError::invariant(&CHARGE_RELEASE_UNDERFLOW))));
        };
        self.bytes = next;
        if let Some(budget) = &self.budget {
            let result = budget.return_workspace(bytes, out.reborrow(), |_| {
                let error = BudgetError::invariant(&CHARGE_BUDGET_RELEASE);
                budget.record_failure(error);
                E::from(error)
            });
            result?;
        }
        Ok(())
    }

    pub(crate) fn transfer_all_into<'slot, E>(
        &mut self,
        destination: &mut Self,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, ()>
    where
        E: From<BudgetError>,
    {
        let compatible = match (&self.budget, &destination.budget) {
            (Some(source), Some(target)) => source.same_call(target),
            (None, None) => true,
            _ => false,
        };
        if !compatible {
            return Err(out.fail(E::from(BudgetError::invariant(&CHARGE_BUDGET_MISMATCH))));
        }
        let total = destination
            .bytes
            .checked_add(self.bytes)
            .ok_or(BudgetError::CapacityExceeded)
            .store_err_into(out.reborrow())?;
        destination.bytes = total;
        self.bytes = 0;
        Ok(())
    }

    pub(crate) fn transfer_optional_into(&mut self, destination: &mut Self) -> bool {
        let compatible = match (&self.budget, &destination.budget) {
            (Some(source), Some(target)) => source.same_call(target),
            (None, None) => true,
            _ => false,
        };
        if !compatible {
            if let Some(budget) = &destination.budget {
                budget.record_failure(BudgetError::invariant(&CHARGE_BUDGET_MISMATCH));
            }
            return false;
        }
        let Some(total) = destination.bytes.checked_add(self.bytes) else {
            return false;
        };
        destination.bytes = total;
        self.bytes = 0;
        true
    }

    pub fn budget(&self) -> Option<&ExecutionBudget> {
        self.budget.as_ref().filter(|budget| !budget.is_regex())
    }

    pub(crate) fn regex_budget<'slot, E>(
        &self,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, Option<RegexExecutionBudget>>
    where
        E: From<BudgetError>,
    {
        match &self.budget {
            Some(budget) if budget.is_regex() => Ok(Some(budget.try_regex_clone(out.reborrow())?)),
            _ => Ok(None),
        }
    }

    pub(crate) fn try_clone<'slot, E>(&self, out: Out<'slot, '_, E>) -> OResult<'slot, Self>
    where
        E: From<BudgetError>,
    {
        let budget = self
            .budget
            .as_ref()
            .map(|budget| budget.try_clone(out))
            .transpose()?;
        Ok(Self { budget, bytes: 0 })
    }

    pub(crate) fn try_clone_optional(&self) -> Option<Self> {
        let budget = match &self.budget {
            Some(budget) => Some(budget.try_clone_optional()?),
            None => None,
        };
        Some(Self { budget, bytes: 0 })
    }

    pub(crate) fn consume<'slot, E>(
        &self,
        steps: usize,
        out: Out<'slot, '_, E>,
    ) -> OResult<'slot, ()>
    where
        E: From<BudgetError>,
    {
        if let Some(budget) = &self.budget {
            budget.consume(steps, out)?;
        }
        Ok(())
    }

    pub(crate) fn consume_optional(&self, steps: usize) -> bool {
        self.budget
            .as_ref()
            .is_none_or(|budget| budget.consume_optional(steps))
    }

    pub(crate) fn record_failure(&self, error: BudgetError) {
        if let Some(budget) = &self.budget {
            budget.record_failure(error);
        }
    }

    pub(crate) const fn bytes(&self) -> usize {
        self.bytes
    }
}

impl Drop for WorkspaceCharge {
    fn drop(&mut self) {
        if let Some(budget) = &self.budget {
            if !budget.return_workspace_optional(self.bytes) {
                budget.record_failure(BudgetError::invariant(&CHARGE_DROP_RELEASE));
            }
        }
    }
}

pub struct WorkspaceString {
    bytes: WorkspaceVec<u8>,
}

impl WorkspaceString {
    pub fn from_utf8_bytes(bytes: WorkspaceVec<u8>) -> Result<Self, (WorkspaceVec<u8>, Utf8Error)> {
        match from_utf8(bytes.as_slice()) {
            Ok(_) => Ok(Self { bytes }),
            Err(error) => Err((bytes, error)),
        }
    }

    pub fn copy<'slot, E>(
        value: &str,
        budget: Option<&ExecutionBudget>,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, Self>
    where
        E: From<BudgetError>,
    {
        let mut text = Self::with_capacity(value.len(), budget, out.reborrow())?;
        text.bytes
            .extend_from_slice(value.as_bytes(), out.reborrow())?;
        Ok(text)
    }

    pub fn with_capacity<'slot, E>(
        capacity: usize,
        budget: Option<&ExecutionBudget>,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, Self>
    where
        E: From<BudgetError>,
    {
        let mut bytes = WorkspaceVec::new(budget, out.reborrow())?;
        bytes.try_reserve_exact(capacity, out.reborrow())?;
        Ok(Self { bytes })
    }

    pub fn as_str(&self) -> &str {
        // Safety: writes only come from valid UTF-8 text, and the byte slice is never mutably exposed.
        unsafe { from_utf8_unchecked(&self.bytes) }
    }

    pub fn budget(&self) -> Option<&ExecutionBudget> {
        self.bytes.budget()
    }

    pub fn capacity(&self) -> usize {
        self.bytes.capacity()
    }

    pub fn try_push_str<'slot, E>(
        &mut self,
        value: &str,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, ()>
    where
        E: From<BudgetError>,
    {
        self.bytes
            .extend_from_slice(value.as_bytes(), out.reborrow())
    }

    pub fn try_push_char<'slot, E>(
        &mut self,
        value: char,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, ()>
    where
        E: From<BudgetError>,
    {
        let mut encoded = [0; 4];
        self.try_push_str(value.encode_utf8(&mut encoded), out.reborrow())
    }

    pub fn clear(&mut self) {
        self.bytes.clear();
    }
}

impl Deref for WorkspaceString {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

/// Fixed-size wrapper storage is charged before allocation and released with its actual owner.
pub(crate) struct WorkspaceBox<T> {
    value: Box<T>,
    _charge: WorkspaceCharge,
}

impl<T> WorkspaceBox<T> {
    pub(crate) fn new<'slot, E>(
        value: T,
        budget: Option<&ExecutionBudget>,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, Self>
    where
        E: From<BudgetError>,
    {
        let layout = Layout::new::<T>();
        let mut charge = WorkspaceCharge::new(budget, out.reborrow())?;
        charge.add(layout.size(), out.reborrow())?;
        let value = if layout.size() == 0 {
            Box::new(value)
        } else {
            // SAFETY: Allocates with the layout of `T`, checks for null before initialization, and transfers unique ownership to `Box`.
            let pointer = unsafe { alloc(layout) }.cast::<T>();
            if pointer.is_null() {
                if let Some(budget) = charge.budget() {
                    budget.record_failure(BudgetError::Allocation);
                }
                return Err(out.fail(E::from(BudgetError::Allocation)));
            }
            unsafe {
                pointer.write(value);
                Box::from_raw(pointer)
            }
        };
        Ok(Self {
            value,
            _charge: charge,
        })
    }
}

impl<T> Deref for WorkspaceBox<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

impl<T> DerefMut for WorkspaceBox<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.value
    }
}
