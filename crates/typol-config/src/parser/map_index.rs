use crate::{diagnostics::DiagnosticFactory, value::MapEntry};
use core::cmp::Ordering;
use opeo::{OResult, Out};
use typol_core::{
    BudgetError, ChargedString, ChargedVec, ConfigValue, Diagnostic, DiagnosticContext,
    ExecutionBudget, diagnostic_context,
};

#[cfg(test)]
use alloc::string::String;

static MAP_INDEX_ENTRY_UNAVAILABLE: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "parser-map-index",
    "entry-reference-out-of-bounds",
);
static MAP_INDEX_LEFT_CHILD_MISSING: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "parser-map-index",
    "left-rotation-child-missing",
);
static MAP_INDEX_RIGHT_CHILD_MISSING: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "parser-map-index",
    "right-rotation-child-missing",
);
static MAP_INDEX_ROTATION_CHILD_MISSING: DiagnosticContext =
    diagnostic_context!("typol-config", "parser-map-index", "rotation-pivot-missing",);
static MAP_INDEX_VALUE_MISSING: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "parser-map-index",
    "completed-entry-value-missing",
);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct EntryId(usize);

struct Entry {
    key: ChargedString,
    value: Option<ConfigValue>,
    parent: Option<EntryId>,
    left: Option<EntryId>,
    right: Option<EntryId>,
    height: usize,
}

impl EntryId {
    pub(super) const fn slot(self) -> usize {
        self.0
    }
}

/// The AVL index refers to stable source slots and retains keys for failed items so duplicates remain detectable after recovery.
pub(super) struct MapIndex {
    entries: ChargedVec<Entry>,
    root: Option<EntryId>,
    #[cfg(test)]
    comparisons: usize,
}

impl MapIndex {
    pub(super) fn new() -> Self {
        Self {
            entries: ChargedVec::unmetered(),
            root: None,
            #[cfg(test)]
            comparisons: 0,
        }
    }

    pub(super) fn new_budgeted<'slot, E>(
        budget: &ExecutionBudget,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, Self>
    where
        E: From<BudgetError>,
    {
        let mut index = Self::new();
        index.entries = ChargedVec::new(Some(budget), out.reborrow())?;
        Ok(index)
    }

    #[cfg(test)]
    pub(super) fn insert<'slot>(
        &mut self,
        key: String,
        mut out: Out<'slot, '_, BudgetError>,
    ) -> OResult<'slot, Option<EntryId>> {
        let key = ChargedString::copy(&key, None, out.reborrow())?;
        self.insert_budgeted(key, out.reborrow())
    }

    pub(super) fn insert_budgeted<'slot, E>(
        &mut self,
        key: ChargedString,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, Option<EntryId>>
    where
        E: From<BudgetError>,
    {
        let mut current = self.root;
        let mut parent = None;
        let mut left = false;
        while let Some(index) = current {
            #[cfg(test)]
            {
                self.comparisons += 1;
            }
            let entry = self.entry(index, out.reborrow())?;
            let order = if let Some(budget) = self.entries.budget() {
                budget.consume(1, out.reborrow())?;
                budget.compare_text(&key, &entry.key, out.reborrow())?
            } else {
                key.cmp(&entry.key)
            };
            match order {
                Ordering::Equal => return Ok(None),
                Ordering::Less => {
                    left = true;
                    current = entry.left;
                }
                Ordering::Greater => {
                    left = false;
                    current = entry.right;
                }
            }
            parent = Some(index);
        }
        let index = EntryId(self.entries.len());
        self.entries.try_push(
            Entry {
                key,
                value: None,
                parent,
                left: None,
                right: None,
                height: 1,
            },
            out.reborrow(),
        )?;
        if let Some(parent) = parent {
            let entry = self.entry_mut(parent, out.reborrow())?;
            if left {
                entry.left = Some(index);
            } else {
                entry.right = Some(index);
            }
        } else {
            self.root = Some(index);
        }
        while let Some(index) = parent {
            if let Some(budget) = self.entries.budget() {
                budget.consume(1, out.reborrow())?;
            }
            let top = self.balance(index, out.reborrow())?;
            parent = self.entry(top, out.reborrow())?.parent;
        }
        Ok(Some(index))
    }

    pub(super) fn set_value<'slot, E>(
        &mut self,
        index: EntryId,
        value: ConfigValue,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, ()>
    where
        E: From<BudgetError>,
    {
        self.entry_mut(index, out.reborrow())?.value = Some(value);
        Ok(())
    }

    pub(super) fn into_entries<'slot, E>(
        self,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, ChargedVec<MapEntry>>
    where
        E: From<BudgetError> + From<Diagnostic>,
    {
        let budget = match self.entries.budget() {
            Some(budget) => Some(budget.try_clone(out.reborrow())?),
            None => None,
        };
        let mut entries = ChargedVec::new(budget.as_ref(), out.reborrow())?;
        entries.try_reserve_exact(self.entries.len(), out.reborrow())?;
        for (index, entry) in self.entries.into_iter().enumerate() {
            if let Some(budget) = budget.as_ref() {
                budget.consume(1, out.reborrow())?;
            }
            let Some(value) = entry.value else {
                return Err(out.fail(E::from(DiagnosticFactory::internal(
                    MAP_INDEX_VALUE_MISSING.with_object_id(index as u64),
                    None,
                    None,
                    None,
                ))));
            };
            let entry = MapEntry::try_new(&entry.key, value, budget.as_ref(), out.reborrow())?;
            entries.try_push(entry, out.reborrow())?;
        }
        Ok(entries)
    }

    fn entry<'index, 'slot, 'borrow, E>(
        &'index self,
        index: EntryId,
        out: Out<'slot, 'borrow, E>,
    ) -> OResult<'slot, &'index Entry>
    where
        E: From<BudgetError>,
    {
        match self.entries.get(index.0) {
            Some(entry) => Ok(entry),
            None => Err(out.fail(E::from(BudgetError::invariant(
                &MAP_INDEX_ENTRY_UNAVAILABLE,
            )))),
        }
    }

    fn entry_mut<'index, 'slot, 'borrow, E>(
        &'index mut self,
        index: EntryId,
        out: Out<'slot, 'borrow, E>,
    ) -> OResult<'slot, &'index mut Entry>
    where
        E: From<BudgetError>,
    {
        match self.entries.get_mut(index.0) {
            Some(entry) => Ok(entry),
            None => Err(out.fail(E::from(BudgetError::invariant(
                &MAP_INDEX_ENTRY_UNAVAILABLE,
            )))),
        }
    }

    fn height<'slot, E>(
        &self,
        index: Option<EntryId>,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, usize>
    where
        E: From<BudgetError>,
    {
        match index {
            Some(index) => Ok(self.entry(index, out.reborrow())?.height),
            None => Ok(0),
        }
    }

    fn update<'slot, E>(&mut self, index: EntryId, mut out: Out<'slot, '_, E>) -> OResult<'slot, ()>
    where
        E: From<BudgetError>,
    {
        let (left, right) = {
            let entry = self.entry(index, out.reborrow())?;
            (entry.left, entry.right)
        };
        let left_height = self.height(left, out.reborrow())?;
        let right_height = self.height(right, out.reborrow())?;
        let height = 1 + left_height.max(right_height);
        self.entry_mut(index, out.reborrow())?.height = height;
        Ok(())
    }

    fn balance<'slot, E>(
        &mut self,
        index: EntryId,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, EntryId>
    where
        E: From<BudgetError>,
    {
        self.update(index, out.reborrow())?;
        let (left, right) = {
            let entry = self.entry(index, out.reborrow())?;
            (entry.left, entry.right)
        };
        let left_height = self.height(left, out.reborrow())?;
        let right_height = self.height(right, out.reborrow())?;
        if left_height > right_height + 1 {
            let Some(left) = left else {
                return Err(out.fail(E::from(BudgetError::invariant(
                    &MAP_INDEX_LEFT_CHILD_MISSING,
                ))));
            };
            let (left_child, right_child) = {
                let entry = self.entry(left, out.reborrow())?;
                (entry.left, entry.right)
            };
            if (self.height(right_child, out.reborrow()))?
                > (self.height(left_child, out.reborrow()))?
            {
                self.rotate(left, false, out.reborrow())?;
            }
            self.rotate(index, true, out.reborrow())
        } else if right_height > left_height + 1 {
            let Some(right) = right else {
                return Err(out.fail(E::from(BudgetError::invariant(
                    &MAP_INDEX_RIGHT_CHILD_MISSING,
                ))));
            };
            let (left_child, right_child) = {
                let entry = self.entry(right, out.reborrow())?;
                (entry.left, entry.right)
            };
            if (self.height(left_child, out.reborrow()))?
                > (self.height(right_child, out.reborrow()))?
            {
                self.rotate(right, true, out.reborrow())?;
            }
            self.rotate(index, false, out.reborrow())
        } else {
            Ok(index)
        }
    }

    fn rotate<'slot, E>(
        &mut self,
        index: EntryId,
        right: bool,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, EntryId>
    where
        E: From<BudgetError>,
    {
        let (parent, child) = {
            let entry = self.entry(index, out.reborrow())?;
            (entry.parent, if right { entry.left } else { entry.right })
        };
        let Some(top) = child else {
            return Err(out.fail(E::from(BudgetError::invariant(
                &MAP_INDEX_ROTATION_CHILD_MISSING,
            ))));
        };
        let middle = {
            let entry = self.entry(top, out.reborrow())?;
            if right { entry.right } else { entry.left }
        };
        if right {
            self.entry_mut(index, out.reborrow())?.left = middle;
            self.entry_mut(top, out.reborrow())?.right = Some(index);
        } else {
            self.entry_mut(index, out.reborrow())?.right = middle;
            self.entry_mut(top, out.reborrow())?.left = Some(index);
        }
        if let Some(middle) = middle {
            self.entry_mut(middle, out.reborrow())?.parent = Some(index);
        }
        self.entry_mut(index, out.reborrow())?.parent = Some(top);
        self.entry_mut(top, out.reborrow())?.parent = parent;
        if let Some(parent) = parent {
            let is_left = self.entry(parent, out.reborrow())?.left == Some(index);
            if is_left {
                self.entry_mut(parent, out.reborrow())?.left = Some(top);
            } else {
                self.entry_mut(parent, out.reborrow())?.right = Some(top);
            }
        } else {
            self.root = Some(top);
        }
        self.update(index, out.reborrow())?;
        self.update(top, out.reborrow())?;
        Ok(top)
    }
}
#[cfg(test)]
mod tests {
    use crate::parser::map_index::{EntryId, MapIndex};
    use typol_test_support::*;

    use alloc::{format, vec::Vec};
    use typol_core::{BudgetError, Diagnostic, ValueKind};

    #[test]
    fn ordered_reverse_and_shuffled_keys_keep_avl_bounds_and_source_slots() -> TestResult<()> {
        let mut budget_errors = TestSlot::<BudgetError>::default();
        let mut diagnostics = TestSlot::<Diagnostic>::default();
        for size in [256_usize, 512] {
            for order in [0, 1, 2] {
                let mut index = MapIndex::new();
                let mut keys = Vec::new();
                for slot in 0..size {
                    let number = match order {
                        0 => slot,
                        1 => size - slot - 1,
                        _ => (slot * 157 + 31) % size,
                    };
                    let key = format!("key{number:04}");
                    test_eq!(
                        budget_errors
                            .run(|out| index.insert(key.clone(), out))
                            .test_value()?,
                        Some(EntryId(slot))
                    )?;
                    let value = ValueFactory::unsigned(number as u128)?;
                    budget_errors
                        .run(|out| index.set_value(EntryId(slot), value, out))
                        .test_value()?;
                    keys.push(key);
                }
                let comparisons = index.comparisons;
                test_assert!(comparisons <= size * (size.ilog2() as usize + 2))?;
                for entry in &index.entries {
                    let left = budget_errors
                        .run(|out| index.height(entry.left, out))
                        .test_value()?;
                    let right = budget_errors
                        .run(|out| index.height(entry.right, out))
                        .test_value()?;
                    test_assert!(left.abs_diff(right) <= 1)?;
                    test_eq!(entry.height, 1 + left.max(right))?;
                    for child in [entry.left, entry.right].into_iter().flatten() {
                        let parent = index
                            .entries
                            .get(child.slot())
                            .test_value()?
                            .parent
                            .test_value()?;
                        test_assert!(
                            index.entries.get(parent.slot()).test_value()?.left == Some(child)
                                || index.entries.get(parent.slot()).test_value()?.right
                                    == Some(child)
                        )?;
                    }
                }
                for key in &keys {
                    test_eq!(
                        budget_errors
                            .run(|out| index.insert(key.clone(), out))
                            .test_value()?,
                        None
                    )?;
                }
                let entries = diagnostics
                    .run(|out| index.into_entries(out))
                    .test_value()?;
                test_eq!(entries.len(), size)?;
                for (entry, key) in entries.iter().zip(&keys) {
                    test_eq!(entry.name(), key)?;
                    let number: u128 = key.trim_start_matches("key").parse().test_value()?;
                    test_assert!(
                        matches!(entry.value().as_ref(), ValueKind::Unsigned(value) if value == number)
                    )?;
                }
            }
        }

        Ok(())
    }
}
