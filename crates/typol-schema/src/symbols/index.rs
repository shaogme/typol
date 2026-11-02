use core::cmp::Ordering;

use opeo::{OResult, Out, ResultOutExt};
use typol_core::{BudgetError, ChargedVec, SourceSpan};

use crate::{
    context::CompileContext,
    diagnostics::{BuildFailure, DiagnosticCollector},
};

struct NameEntry<'a, T> {
    name: &'a str,
    span: SourceSpan,
    target: T,
    ordinal: usize,
}

pub(crate) struct NameIndex<'a, T> {
    entries: ChargedVec<NameEntry<'a, T>>,
}

pub(crate) struct Duplicate<T> {
    pub(crate) first: T,
    pub(crate) later: T,
    pub(crate) first_span: SourceSpan,
    pub(crate) later_span: SourceSpan,
    pub(crate) ordinal: usize,
}

pub(crate) enum NameMatch<T> {
    Missing,
    Found(T),
    Ambiguous,
}

impl<'a, T: Copy> NameIndex<'a, T> {
    pub(crate) fn new<'slot>(
        context: &CompileContext,
        diagnostics: &mut DiagnosticCollector,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, Self> {
        let entries = diagnostics.new_vec(context.execution(), out.reborrow())?;
        Ok(Self { entries })
    }

    pub(crate) fn push<'slot, E>(
        &mut self,
        name: &'a str,
        span: SourceSpan,
        target: T,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, ()>
    where
        E: From<BudgetError>,
    {
        self.entries.try_push(
            NameEntry {
                name,
                span,
                target,
                ordinal: self.entries.len(),
            },
            out.reborrow(),
        )?;
        Ok(())
    }

    pub(crate) fn finish<'slot>(
        &mut self,
        context: &CompileContext,
        diagnostics: &mut DiagnosticCollector,
        span: SourceSpan,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ChargedVec<Duplicate<T>>> {
        let duplicates =
            context.budget_access(out.reborrow(), span, diagnostics, |budget_out| {
                Self::finish_entries(self, context, budget_out)
            })?;
        Ok(duplicates)
    }

    fn finish_entries<'slot, E>(
        &mut self,
        context: &CompileContext,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, ChargedVec<Duplicate<T>>>
    where
        E: From<BudgetError>,
    {
        context.sort(
            &mut self.entries,
            |left, right, mut sort_out| {
                let order = context.compare_text(left.name, right.name, sort_out.reborrow())?;
                Ok(order.then_with(|| left.ordinal.cmp(&right.ordinal)))
            },
            out.reborrow(),
        )?;
        let mut duplicates = ChargedVec::new(Some(context.execution()), out.reborrow())?;
        let mut first = 0;
        for later in 1..self.entries.len() {
            context.execution().consume(2, out.reborrow())?;
            let a = self
                .entries
                .get(first)
                .ok_or_else(|| schema_invariant!("symbol-index", "first-entry-missing"))
                .store_err_into(out.reborrow())?;
            let b = self
                .entries
                .get(later)
                .ok_or_else(|| schema_invariant!("symbol-index", "later-entry-missing"))
                .store_err_into(out.reborrow())?;
            if context.compare_text(a.name, b.name, out.reborrow())? == Ordering::Equal {
                duplicates.try_push(
                    Duplicate {
                        first: a.target,
                        later: b.target,
                        first_span: a.span,
                        later_span: b.span,
                        ordinal: b.ordinal,
                    },
                    out.reborrow(),
                )?;
            } else {
                first = later;
            }
        }
        // The index is sorted by name, while diagnostics retain declaration order.
        context.sort(
            &mut duplicates,
            |left, right, _| Ok(left.ordinal.cmp(&right.ordinal)),
            out.reborrow(),
        )?;
        Ok(duplicates)
    }

    pub(crate) fn lookup<'slot, E>(
        &self,
        name: &str,
        context: &CompileContext,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, NameMatch<T>>
    where
        E: From<BudgetError>,
    {
        context.execution().consume::<E>(1, out.reborrow())?;
        let mut low = 0;
        let mut high = self.entries.len();
        while low < high {
            context.execution().consume::<E>(1, out.reborrow())?;
            let middle = low + (high - low) / 2;
            let entry = self
                .entries
                .get(middle)
                .ok_or_else(|| schema_invariant!("symbol-index", "lookup-entry-missing"))
                .store_err_into(out.reborrow())?;
            if context
                .execution()
                .compare_text::<E>(entry.name, name, out.reborrow())?
                == Ordering::Less
            {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        let Some(first) = self.entries.get(low) else {
            return Ok(NameMatch::Missing);
        };
        if context
            .execution()
            .compare_text::<E>(first.name, name, out.reborrow())?
            != Ordering::Equal
        {
            return Ok(NameMatch::Missing);
        }
        if let Some(next) = self.entries.get(low + 1) {
            context.execution().consume::<E>(1, out.reborrow())?;
            if context
                .execution()
                .compare_text::<E>(first.name, next.name, out.reborrow())?
                == Ordering::Equal
            {
                return Ok(NameMatch::Ambiguous);
            }
        }
        Ok(NameMatch::Found(first.target))
    }
}
