use crate::{ExecutionBudget, workspace::WorkspaceVec};
use core::{
    mem::size_of,
    ops::{Deref, DerefMut},
    slice,
};

/// Best-effort report storage does not poison the workload budget, and its charge follows its owner.
pub struct OptionalVec<T> {
    values: WorkspaceVec<T>,
    max_bytes: usize,
}

impl<T> OptionalVec<T> {
    pub(crate) fn checked_storage_bytes(parts: impl IntoIterator<Item = usize>) -> Option<usize> {
        parts.into_iter().try_fold(0usize, usize::checked_add)
    }

    pub(crate) const fn empty() -> Self {
        Self {
            values: WorkspaceVec::unmetered(),
            max_bytes: usize::MAX,
        }
    }

    pub(crate) fn new(budget: Option<&ExecutionBudget>) -> Option<Self> {
        Some(Self {
            values: WorkspaceVec::new_optional(budget)?,
            max_bytes: usize::MAX,
        })
    }

    pub fn bounded(budget: Option<&ExecutionBudget>, max_bytes: usize) -> Option<Self> {
        let mut values = Self::new(budget)?;
        values.max_bytes = max_bytes;
        Some(values)
    }

    pub fn budget(&self) -> Option<&ExecutionBudget> {
        self.values.budget()
    }

    pub fn capacity_bytes(&self) -> usize {
        self.values.capacity().saturating_mul(size_of::<T>())
    }

    pub(crate) fn set_max_bytes(&mut self, max_bytes: usize) {
        self.max_bytes = max_bytes;
    }

    pub(crate) fn reserve(&mut self, capacity: usize) -> bool {
        let Some(additional) = capacity.checked_sub(self.values.len()) else {
            return true;
        };
        self.values.reserve_optional(additional, self.max_bytes)
    }

    pub fn push(&mut self, value: T) -> bool {
        if self.values.len() == self.values.capacity()
            && !self.values.reserve_optional(1, self.max_bytes)
        {
            return false;
        }
        self.values.push_reserved(value)
    }

    pub fn extend(&mut self, values: &[T]) -> bool
    where
        T: Copy,
    {
        let Some(end) = self.values.len().checked_add(values.len()) else {
            return false;
        };
        if !self.reserve(end) {
            return false;
        }
        for value in values {
            if !self.values.push_reserved(*value) {
                return false;
            }
        }
        true
    }

    pub(crate) fn clear(&mut self) {
        self.values.clear();
    }

    pub fn remove(&mut self, index: usize) -> T {
        self.values.remove(index)
    }
}

impl<T> Deref for OptionalVec<T> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        &self.values
    }
}

impl<T> DerefMut for OptionalVec<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.values
    }
}

impl<'a, T> IntoIterator for &'a OptionalVec<T> {
    type Item = &'a T;
    type IntoIter = slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.values.iter()
    }
}
