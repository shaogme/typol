use crate::{BudgetError, DiagnosticContext, diagnostic_context, storage::WorkspaceCharge};
use core::cmp::Ordering;
use opeo::{OResult, Out, ResultOutExt};

static SORT_PREFIX: DiagnosticContext =
    diagnostic_context!("typol-core", "budget-sort", "sort-prefix-range-missing");
static SORT_CHILD_PAIR: DiagnosticContext =
    diagnostic_context!("typol-core", "budget-sort", "sort-child-pair-missing");
static SORT_ROOT: DiagnosticContext =
    diagnostic_context!("typol-core", "budget-sort", "sort-root-index-missing");
static SORT_CHILD: DiagnosticContext =
    diagnostic_context!("typol-core", "budget-sort", "sort-child-index-missing");

pub(crate) struct FallibleSort {
    charge: WorkspaceCharge,
}

impl FallibleSort {
    pub(crate) const fn new(charge: WorkspaceCharge) -> Self {
        Self { charge }
    }

    fn consume<'slot, E>(&self, steps: usize, out: Out<'slot, '_, E>) -> OResult<'slot, ()>
    where
        E: From<BudgetError>,
    {
        self.charge.consume(steps, out)
    }

    /// In-place heapsort allocates no additional payload, and each comparison or swap can propagate a budget failure.
    pub(crate) fn sort<'slot, T, E>(
        &self,
        values: &mut [T],
        mut compare: impl for<'call> FnMut(&T, &T, Out<'slot, 'call, E>) -> OResult<'slot, Ordering>,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, ()>
    where
        E: From<BudgetError>,
    {
        self.consume(1, out.reborrow())?;
        for root in (0..values.len() / 2).rev() {
            self.sift(values, root, &mut compare, out.reborrow())?;
        }
        for end in (1..values.len()).rev() {
            self.consume(2, out.reborrow())?;
            values.swap(0, end);
            let prefix = values
                .get_mut(..end)
                .ok_or_else(|| E::from(BudgetError::invariant(&SORT_PREFIX)));
            let prefix = prefix.store_err_into(out.reborrow())?;
            self.sift(prefix, 0, &mut compare, out.reborrow())?;
        }
        Ok(())
    }

    fn sift<'slot, T, E>(
        &self,
        values: &mut [T],
        mut root: usize,
        compare: &mut impl for<'call> FnMut(&T, &T, Out<'slot, 'call, E>) -> OResult<'slot, Ordering>,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, ()>
    where
        E: From<BudgetError>,
    {
        while root < values.len() / 2 {
            self.consume(1, out.reborrow())?;
            let mut child = root * 2 + 1;
            if child + 1 < values.len() {
                self.consume(1, out.reborrow())?;
                let pair = values
                    .get(child..child + 2)
                    .ok_or_else(|| E::from(BudgetError::invariant(&SORT_CHILD_PAIR)));
                let pair = pair.store_err_into(out.reborrow())?;
                if let [left, right] = pair {
                    let order = compare(left, right, out.reborrow())?;
                    if order == Ordering::Less {
                        child += 1;
                    }
                }
            }
            self.consume(1, out.reborrow())?;
            let root_value = values
                .get(root)
                .ok_or_else(|| E::from(BudgetError::invariant(&SORT_ROOT)));
            let root_value = root_value.store_err_into(out.reborrow())?;
            let child_value = values
                .get(child)
                .ok_or_else(|| E::from(BudgetError::invariant(&SORT_CHILD)));
            let child_value = child_value.store_err_into(out.reborrow())?;
            let order = compare(root_value, child_value, out.reborrow())?;
            if order != Ordering::Less {
                break;
            }
            self.consume(2, out.reborrow())?;
            values.swap(root, child);
            root = child;
        }
        Ok(())
    }
}
