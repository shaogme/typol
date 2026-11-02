use crate::{serde::model::Error, session::SessionBudget, value::MapEntry};
use opeo::{OResult, Out, ResultOutExt};
use typol_core::{ChargedVec, DiagnosticContext, diagnostic_context};

static MAP_KEY_INDEX_INVARIANT: DiagnosticContext =
    diagnostic_context!("typol", "serde", "map-key-index-invariant");

pub(crate) struct MapKeys {
    slots: ChargedVec<usize>,
}

impl MapKeys {
    pub(crate) fn new<'slot>(
        session: &SessionBudget,
        mut out: Out<'slot, '_, Error>,
    ) -> OResult<'slot, Self> {
        Ok(Self {
            slots: session.budget_as(|budget_out| session.workspace(budget_out), out.reborrow())?,
        })
    }

    fn hash<'slot>(
        text: &str,
        session: &SessionBudget,
        mut out: Out<'slot, '_, Error>,
    ) -> OResult<'slot, usize> {
        session.budget_as(
            |budget_out| session.execution().consume(text.len(), budget_out),
            out.reborrow(),
        )?;
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for byte in text.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        Ok(hash as usize)
    }

    fn empty_slot<'slot>(
        &self,
        hash: usize,
        session: &SessionBudget,
        mut out: Out<'slot, '_, Error>,
    ) -> OResult<'slot, usize> {
        let mask = self
            .slots
            .len()
            .checked_sub(1)
            .ok_or_else(|| Error::internal(MAP_KEY_INDEX_INVARIANT))
            .store_err_into(out.reborrow())?;
        let mut slot = hash & mask;
        for _ in 0..self.slots.len() {
            session.budget_as(|budget_out| session.visit(budget_out), out.reborrow())?;
            if *self
                .slots
                .get(slot)
                .ok_or_else(|| Error::internal(MAP_KEY_INDEX_INVARIANT.with_object_id(slot as u64)))
                .store_err_into(out.reborrow())?
                == usize::MAX
            {
                return Ok(slot);
            }
            slot = (slot + 1) & mask;
        }
        Err(out.fail(Error::internal(MAP_KEY_INDEX_INVARIANT)))
    }

    fn grow<'slot>(
        &mut self,
        entries: &[MapEntry],
        session: &SessionBudget,
        mut out: Out<'slot, '_, Error>,
    ) -> OResult<'slot, ()> {
        let needed = entries
            .len()
            .checked_add(1)
            .and_then(|count| count.checked_mul(2))
            .ok_or_else(|| Error::from(session.execution().resource_error()))
            .store_err_into(out.reborrow())?;
        if needed <= self.slots.len() {
            return Ok(());
        }
        let capacity = needed
            .checked_next_power_of_two()
            .ok_or_else(|| Error::from(session.execution().resource_error()))
            .store_err_into(out.reborrow())?
            .max(8);
        let mut slots =
            session.budget_as(|budget_out| session.workspace(budget_out), out.reborrow())?;
        session.budget_as(
            |budget_out| slots.try_reserve_exact(capacity, budget_out),
            out.reborrow(),
        )?;
        session.budget_as(
            |budget_out| slots.resize(capacity, usize::MAX, budget_out),
            out.reborrow(),
        )?;
        // Keep both capacity charges while the old and new indexes are alive.
        let mut next = Self { slots };
        for (index, entry) in entries.iter().enumerate() {
            let hash = Self::hash(entry.name(), session, out.reborrow())?;
            let slot = next.empty_slot(hash, session, out.reborrow())?;
            *next
                .slots
                .get_mut(slot)
                .ok_or_else(|| Error::internal(MAP_KEY_INDEX_INVARIANT.with_object_id(slot as u64)))
                .store_err_into(out.reborrow())? = index;
        }
        *self = next;
        Ok(())
    }

    pub(crate) fn insert<'slot>(
        &mut self,
        key: &str,
        entries: &[MapEntry],
        session: &SessionBudget,
        mut out: Out<'slot, '_, Error>,
    ) -> OResult<'slot, ()> {
        self.grow(entries, session, out.reborrow())?;
        let hash = Self::hash(key, session, out.reborrow())?;
        let mask = self
            .slots
            .len()
            .checked_sub(1)
            .ok_or_else(|| Error::internal(MAP_KEY_INDEX_INVARIANT))
            .store_err_into(out.reborrow())?;
        let mut slot = hash & mask;
        for _ in 0..self.slots.len() {
            session.budget_as(|budget_out| session.visit(budget_out), out.reborrow())?;
            let index = *self
                .slots
                .get(slot)
                .ok_or_else(|| Error::internal(MAP_KEY_INDEX_INVARIANT.with_object_id(slot as u64)))
                .store_err_into(out.reborrow())?;
            if index == usize::MAX {
                *self
                    .slots
                    .get_mut(slot)
                    .ok_or_else(|| {
                        Error::internal(MAP_KEY_INDEX_INVARIANT.with_object_id(slot as u64))
                    })
                    .store_err_into(out.reborrow())? = entries.len();
                return Ok(());
            }
            let entry = entries
                .get(index)
                .ok_or_else(|| {
                    Error::internal(MAP_KEY_INDEX_INVARIANT.with_object_id(index as u64))
                })
                .store_err_into(out.reborrow())?;
            let ordering = session.budget_as(
                |budget_out| session.compare_text(entry.name(), key, budget_out),
                out.reborrow(),
            )?;
            if ordering.is_eq() {
                return Err(out.fail(Error::shape_mismatch()));
            }
            slot = (slot + 1) & mask;
        }
        Err(out.fail(Error::internal(MAP_KEY_INDEX_INVARIANT)))
    }
}
