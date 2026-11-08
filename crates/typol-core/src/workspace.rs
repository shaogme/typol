use crate::{
    BudgetError, ExecutionBudget, RegexExecutionBudget, budget_sort::FallibleSort,
    storage::WorkspaceCharge,
};
use alloc::{
    alloc::Layout,
    vec::{Drain, IntoIter, Vec},
};
use core::{
    cmp::Ordering,
    mem::{replace, size_of, take},
    ops::{Deref, DerefMut, RangeFrom},
    slice,
};
use opeo::{OResult, Out, ResultOutExt};

/// Workspace growth reserves both old and new storage; the charge is returned after release, while `pop` and `clear` do not return capacity.
pub struct WorkspaceVec<T> {
    values: Vec<T>,
    charge: WorkspaceCharge,
}

impl<T> WorkspaceVec<T> {
    pub const fn unmetered() -> Self {
        Self {
            values: Vec::new(),
            charge: WorkspaceCharge::unmetered(),
        }
    }

    pub fn from_vec(values: Vec<T>) -> Self {
        let bytes = values.capacity().saturating_mul(size_of::<T>());
        Self {
            values,
            charge: WorkspaceCharge::unmetered_with_bytes(bytes),
        }
    }

    /// Takes ownership of an existing vector allocation and charges its actual capacity to the execution budget.
    pub fn try_from_vec_with_budget<'slot, E>(
        values: Vec<T>,
        budget: &ExecutionBudget,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, Self>
    where
        E: From<BudgetError>,
    {
        let layout = Layout::array::<T>(values.capacity())
            .map_err(|_| BudgetError::CapacityExceeded)
            .store_err_into(out.reborrow())?;
        let mut charge = WorkspaceCharge::new(Some(budget), out.reborrow())?;
        charge.add(layout.size(), out.reborrow())?;
        Ok(Self { values, charge })
    }

    pub fn new<'slot, E>(
        budget: Option<&ExecutionBudget>,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, Self>
    where
        E: From<BudgetError>,
    {
        let charge = WorkspaceCharge::new(budget, out.reborrow())?;
        Ok(Self {
            values: Vec::new(),
            charge,
        })
    }

    /// Creates storage charged to a dedicated Regex execution session.
    pub fn new_regex<'slot, E>(
        budget: Option<&RegexExecutionBudget>,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, Self>
    where
        E: From<BudgetError>,
    {
        let charge = WorkspaceCharge::new_regex(budget, out.reborrow())?;
        Ok(Self {
            values: Vec::new(),
            charge,
        })
    }

    pub(crate) fn new_optional(budget: Option<&ExecutionBudget>) -> Option<Self> {
        let charge = WorkspaceCharge::new_optional(budget)?;
        Some(Self {
            values: Vec::new(),
            charge,
        })
    }

    pub(crate) fn reserve_optional(&mut self, additional: usize, max_bytes: usize) -> bool {
        let Some(required) = self.values.len().checked_add(additional) else {
            return false;
        };
        if required <= self.values.capacity() {
            return true;
        }
        let Some(doubled) = self.values.capacity().checked_mul(2) else {
            return false;
        };
        self.grow_optional(required.max(doubled), max_bytes)
    }

    fn grow_optional(&mut self, capacity: usize, max_bytes: usize) -> bool {
        let Ok(layout) = Layout::array::<T>(capacity) else {
            return false;
        };
        if layout.size() > max_bytes {
            return false;
        }
        let Some(old_capacity_bytes) = self.values.capacity().checked_mul(size_of::<T>()) else {
            return false;
        };
        let Some(mut replacement_charge) = self.charge.try_clone_optional() else {
            return false;
        };
        if !replacement_charge.add_optional(layout.size()) {
            return false;
        }
        let mut replacement = Vec::new();
        if replacement.try_reserve_exact(capacity).is_err() {
            return false;
        }
        let Some(actual_bytes) = replacement.capacity().checked_mul(size_of::<T>()) else {
            return false;
        };
        if actual_bytes > max_bytes {
            return false;
        }
        if actual_bytes > layout.size()
            && !replacement_charge.add_optional(actual_bytes - layout.size())
        {
            return false;
        }
        if !replacement_charge.consume_optional(self.values.len()) {
            return false;
        }
        if !replacement_charge.transfer_optional_into(&mut self.charge) {
            return false;
        }
        replacement.append(&mut self.values);
        let old_values = replace(&mut self.values, replacement);
        drop(old_values);
        self.charge.release_optional(old_capacity_bytes)
    }

    pub fn try_reserve<'slot, E>(
        &mut self,
        additional: usize,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, ()>
    where
        E: From<BudgetError>,
    {
        let required = self
            .values
            .len()
            .checked_add(additional)
            .ok_or(BudgetError::CapacityExceeded)
            .store_err_into(out.reborrow())?;
        if required <= self.values.capacity() {
            return Ok(());
        }
        let doubled = self
            .values
            .capacity()
            .checked_mul(2)
            .ok_or(BudgetError::CapacityExceeded)
            .store_err_into(out.reborrow())?;
        self.grow(required.max(doubled), out.reborrow())
    }

    pub fn try_reserve_exact<'slot, E>(
        &mut self,
        additional: usize,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, ()>
    where
        E: From<BudgetError>,
    {
        let required = self
            .values
            .len()
            .checked_add(additional)
            .ok_or(BudgetError::CapacityExceeded)
            .store_err_into(out.reborrow())?;
        if required <= self.values.capacity() {
            return Ok(());
        }
        self.grow(required, out.reborrow())
    }

    fn grow<'slot, E>(&mut self, capacity: usize, mut out: Out<'slot, '_, E>) -> OResult<'slot, ()>
    where
        E: From<BudgetError>,
    {
        let layout = Layout::array::<T>(capacity)
            .map_err(|_| BudgetError::CapacityExceeded)
            .store_err_into(out.reborrow())?;
        let old_capacity_bytes = self
            .values
            .capacity()
            .checked_mul(size_of::<T>())
            .ok_or(BudgetError::CapacityExceeded)
            .store_err_into(out.reborrow())?;
        let mut replacement_charge = self.charge.try_clone(out.reborrow())?;
        replacement_charge.add(layout.size(), out.reborrow())?;
        let mut replacement = Vec::new();
        if replacement.try_reserve_exact(capacity).is_err() {
            replacement_charge.record_failure(BudgetError::Allocation);
            return Err(out.fail(E::from(BudgetError::Allocation)));
        }
        let actual = replacement
            .capacity()
            .checked_mul(size_of::<T>())
            .ok_or(BudgetError::CapacityExceeded)
            .store_err_into(out.reborrow())?;
        if actual > layout.size() {
            replacement_charge.add(actual - layout.size(), out.reborrow())?;
        }
        replacement_charge.consume(self.values.len(), out.reborrow())?;

        replacement_charge.transfer_all_into(&mut self.charge, out.reborrow())?;
        replacement.append(&mut self.values);
        let old_values = replace(&mut self.values, replacement);
        drop(old_values);
        self.charge.release(old_capacity_bytes, out.reborrow())?;
        Ok(())
    }

    /// Appends and grows as needed; capacity and allocation failures use the error slot.
    pub fn try_push<'slot, E>(&mut self, value: T, mut out: Out<'slot, '_, E>) -> OResult<'slot, ()>
    where
        E: From<BudgetError>,
    {
        self.charge.consume(1, out.reborrow())?;
        if self.values.len() == self.values.capacity() {
            self.try_reserve(1, out.reborrow())?;
        }
        if !self.push_reserved(value) {
            return Err(out.fail(E::from(BudgetError::CapacityExceeded)));
        }
        Ok(())
    }

    pub(crate) fn push_reserved(&mut self, value: T) -> bool {
        if self.values.len() >= self.values.capacity() {
            return false;
        }
        self.values.push(value);
        true
    }

    /// Appends and grows as needed; a failure preserves the successfully appended prefix.
    pub fn extend<'slot, E>(
        &mut self,
        values: impl IntoIterator<Item = T>,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, ()>
    where
        E: From<BudgetError>,
    {
        for value in values {
            self.charge.consume(1, out.reborrow())?;
            if self.values.len() == self.values.capacity() {
                self.try_reserve(1, out.reborrow())?;
            }
            if !self.push_reserved(value) {
                return Err(out.fail(E::from(BudgetError::CapacityExceeded)));
            }
        }
        Ok(())
    }

    pub fn charge_storage<'slot, 'borrow, E>(
        &mut self,
        bytes: usize,
        out: Out<'slot, 'borrow, E>,
    ) -> OResult<'slot, ()>
    where
        E: From<BudgetError>,
    {
        self.charge.add(bytes, out)
    }

    pub fn capacity(&self) -> usize {
        self.values.capacity()
    }

    pub fn as_slice(&self) -> &[T] {
        &self.values
    }

    pub fn as_mut_slice(&mut self) -> &mut [T] {
        &mut self.values
    }

    pub fn pop(&mut self) -> Option<T> {
        self.values.pop()
    }

    pub(crate) fn remove(&mut self, index: usize) -> T {
        self.values.remove(index)
    }

    pub fn clear(&mut self) {
        self.values.clear();
    }

    pub fn truncate(&mut self, length: usize) {
        self.values.truncate(length);
    }

    pub(crate) fn rollback(&mut self, length: usize) {
        self.values.truncate(length);
    }

    pub fn drain(&mut self, range: RangeFrom<usize>) -> Drain<'_, T> {
        self.values.drain(range)
    }

    pub fn dedup(&mut self)
    where
        T: PartialEq,
    {
        self.values.dedup();
    }

    pub fn resize_with<'slot, E>(
        &mut self,
        length: usize,
        make: impl FnMut() -> T,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, ()>
    where
        E: From<BudgetError>,
    {
        if length > self.values.len() {
            let additional = length - self.values.len();
            self.charge.consume(additional, out.reborrow())?;
            self.try_reserve(additional, out.reborrow())?;
        }
        self.values.resize_with(length, make);
        Ok(())
    }

    pub fn resize<'slot, E>(
        &mut self,
        length: usize,
        value: T,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, ()>
    where
        T: Clone,
        E: From<BudgetError>,
    {
        if length > self.values.len() {
            let additional = length - self.values.len();
            self.charge.consume(additional, out.reborrow())?;
            self.try_reserve(additional, out.reborrow())?;
        }
        self.values.resize(length, value);
        Ok(())
    }

    pub fn extend_from_slice<'slot, E>(
        &mut self,
        values: &[T],
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, ()>
    where
        T: Clone,
        E: From<BudgetError>,
    {
        self.charge.consume(values.len(), out.reborrow())?;
        self.try_reserve(values.len(), out.reborrow())?;
        self.values.extend_from_slice(values);
        Ok(())
    }

    /// Explicitly transfers storage responsibility to an unmetered caller.
    pub fn into_unmetered_vec(mut self) -> Vec<T> {
        let values = take(&mut self.values);
        self.charge = WorkspaceCharge::unmetered();
        values
    }

    pub fn budget(&self) -> Option<&ExecutionBudget> {
        self.charge.budget()
    }

    pub fn regex_budget<'slot, E>(
        &self,
        out: Out<'slot, '_, E>,
    ) -> OResult<'slot, Option<RegexExecutionBudget>>
    where
        E: From<BudgetError>,
    {
        self.charge.regex_budget(out)
    }

    /// Abortable heapsort that avoids a standard-library comparison closure, which cannot propagate resource errors outward.
    pub fn sort_by<'slot, E>(
        &mut self,
        compare: impl for<'call> FnMut(&T, &T, Out<'slot, 'call, E>) -> OResult<'slot, Ordering>,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, ()>
    where
        E: From<BudgetError>,
    {
        let sorter = FallibleSort::new(self.charge.try_clone(out.reborrow())?);
        sorter.sort(&mut self.values, compare, out.reborrow())
    }
}

impl<T> Deref for WorkspaceVec<T> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        &self.values
    }
}

impl<T> DerefMut for WorkspaceVec<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.values
    }
}

impl<T> Drop for WorkspaceVec<T> {
    fn drop(&mut self) {
        let values = take(&mut self.values);
        drop(values);
        drop(replace(&mut self.charge, WorkspaceCharge::unmetered()));
    }
}

impl<'a, T> IntoIterator for &'a WorkspaceVec<T> {
    type Item = &'a T;
    type IntoIter = slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.values.iter()
    }
}

pub struct WorkspaceIntoIter<T> {
    values: IntoIter<T>,
    charge: WorkspaceCharge,
}

impl<T> IntoIterator for WorkspaceVec<T> {
    type Item = T;
    type IntoIter = WorkspaceIntoIter<T>;

    fn into_iter(mut self) -> Self::IntoIter {
        let values = take(&mut self.values).into_iter();
        WorkspaceIntoIter {
            values,
            charge: replace(&mut self.charge, WorkspaceCharge::unmetered()),
        }
    }
}

impl<T> Iterator for WorkspaceIntoIter<T> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        self.values.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.values.size_hint()
    }
}

impl<T> DoubleEndedIterator for WorkspaceIntoIter<T> {
    fn next_back(&mut self) -> Option<T> {
        self.values.next_back()
    }
}

impl<T> ExactSizeIterator for WorkspaceIntoIter<T> {}

impl<T> Drop for WorkspaceIntoIter<T> {
    fn drop(&mut self) {
        drop(replace(&mut self.values, Vec::new().into_iter()));
        drop(replace(&mut self.charge, WorkspaceCharge::unmetered()));
    }
}
