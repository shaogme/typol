use std::{cell::RefCell, cmp::Ordering, fmt, mem::take, str::from_utf8};

use opeo::{ErrSlot, OResult, Out, ResultOutExt};
use typol_core::{BudgetError, ChargedString, ChargedVec, ExecutionBudget, SourceId};
use typol_schema::{
    SchemaCompileContext, SourceLoadedRequest, SourceObserver, SourceObserverError,
};

use crate::{
    HostError, HostErrorKind, HostFailureStep, HostOperation, SourceNameEditOptions,
    SourceNameLimits,
};

struct SourceName {
    id: SourceId,
    identity: ChargedVec<u8>,
    text: ChargedString,
}

struct SourceNameInput<'a> {
    identity: &'a [u8],
    text: &'a str,
}

/// Registers only actual source notifications; registration failure is returned to the compiler immediately, and the first failure reason is retained for queries.
pub struct SourceNames {
    limits: SourceNameLimits,
    context: SchemaCompileContext,
    entries: ChargedVec<SourceName>,
    failure: Option<HostError>,
    errors: ErrSlot<HostError>,
    budget_errors: RefCell<ErrSlot<BudgetError>>,
}

impl SourceNames {
    pub fn new<'slot>(
        limits: SourceNameLimits,
        context: &SchemaCompileContext,
        mut out: Out<'slot, '_, BudgetError>,
    ) -> OResult<'slot, Self> {
        let context = context.try_clone(out.reborrow())?;
        let entries = ChargedVec::new(Some(context.execution()), out.reborrow())?;
        Ok(Self {
            limits,
            context,
            entries,
            failure: None,
            errors: ErrSlot::new(),
            budget_errors: RefCell::new(ErrSlot::new()),
        })
    }

    pub fn name(&self, source_id: SourceId) -> Option<&str> {
        let index = self
            .entries
            .as_slice()
            .binary_search_by(|entry| entry.id.get().cmp(&source_id.get()));
        index
            .ok()
            .and_then(|index| self.entries.get(index))
            .map(|entry| entry.text.as_str())
    }

    pub fn identity(&self, source_id: SourceId) -> Option<&[u8]> {
        let index = self
            .entries
            .as_slice()
            .binary_search_by(|entry| entry.id.get().cmp(&source_id.get()));
        index
            .ok()
            .and_then(|index| self.entries.get(index))
            .map(|entry| entry.identity.as_slice())
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Replaces only registered names; an earlier registration failure prevents changes, while other failures preserve the old name and latched state.
    pub fn set_name<'slot>(
        &mut self,
        source_id: SourceId,
        name: &str,
        options: SourceNameEditOptions,
        mut out: Out<'slot, '_, HostError>,
    ) -> OResult<'slot, ()> {
        self.set_name_inner(source_id, name, options, out.reborrow())?;
        Ok(())
    }

    fn set_name_inner<'slot>(
        &mut self,
        source_id: SourceId,
        name: &str,
        options: SourceNameEditOptions,
        mut out: Out<'slot, '_, HostError>,
    ) -> OResult<'slot, ()> {
        self.check_editable(Some(source_id), out.reborrow())?;
        let _budget = self.active_budget(source_id, out.reborrow())?;
        let budget = {
            let mut errors = self.budget_errors.borrow_mut();
            match errors
                .try_call(|budget_out| ExecutionBudget::new(options.execution(), budget_out))
            {
                Ok(budget) => budget,
                Err(caught) => {
                    return Err(out.fail(
                        HostError::budget(HostOperation::SourceNames, *caught.get())
                            .with_source_id(source_id),
                    ));
                }
            }
        };
        let (index, found) = self.find_position(source_id, &budget, out.reborrow())?;
        if !found {
            return Err(out.fail(
                HostError::with_step(
                    HostOperation::SourceNames,
                    HostErrorKind::InvalidRequest,
                    HostFailureStep::SourceNameOperation,
                )
                .with_source_id(source_id),
            ));
        }
        let identity = self
            .entries
            .get(index)
            .map(|entry| entry.identity.as_slice())
            .ok_or_else(|| {
                HostError::internal(
                    HostOperation::SourceNames,
                    HostFailureStep::SourceNameOperation,
                )
                .with_source_id(source_id)
            })
            .store_err_into(out.reborrow())?;
        let entry = Self::copy_name(
            source_id,
            SourceNameInput {
                identity,
                text: name,
            },
            options.limits(),
            &budget,
            out.reborrow(),
        )?;
        let Some(target) = self.entries.get_mut(index) else {
            return Err(out.fail(
                HostError::internal(
                    HostOperation::SourceNames,
                    HostFailureStep::SourceNameOperation,
                )
                .with_source_id(source_id),
            ));
        };
        *target = entry;
        Ok(())
    }

    pub fn try_copy<'slot>(
        &self,
        options: SourceNameEditOptions,
        mut out: Out<'slot, '_, HostError>,
    ) -> OResult<'slot, Self> {
        self.check_editable(None, out.reborrow())?;
        let budget = {
            let mut errors = self.budget_errors.borrow_mut();
            match errors
                .try_call(|budget_out| ExecutionBudget::new(options.execution(), budget_out))
            {
                Ok(budget) => budget,
                Err(caught) => {
                    return Err(
                        out.fail(HostError::budget(HostOperation::SourceNames, *caught.get()))
                    );
                }
            }
        };
        let source_context = &self.context;
        let context = {
            let mut errors = self.budget_errors.borrow_mut();
            match errors.try_call(|budget_out| source_context.try_clone(budget_out)) {
                Ok(context) => context,
                Err(caught) => {
                    return Err(
                        out.fail(HostError::budget(HostOperation::SourceNames, *caught.get()))
                    );
                }
            }
        };
        let entries = HostError::run_budgeted(
            HostOperation::SourceNames,
            None,
            out.reborrow(),
            |budget_out| ChargedVec::new(Some(&budget), budget_out),
        )?;
        let mut copy = Self {
            limits: options.limits(),
            context,
            entries,
            failure: self.failure,
            errors: ErrSlot::new(),
            budget_errors: RefCell::new(ErrSlot::new()),
        };
        for entry in &self.entries {
            budget.consume_with(1, out.reborrow(), |error| {
                HostError::budget(HostOperation::SourceNames, error)
            })?;
            copy.insert_with_budget(
                entry.id,
                SourceNameInput {
                    identity: entry.identity.as_slice(),
                    text: &entry.text,
                },
                &budget,
                out.reborrow(),
            )?;
        }
        Ok(copy)
    }

    pub const fn failure(&self) -> Option<HostErrorKind> {
        match self.failure {
            Some(error) => Some(error.kind()),
            None => None,
        }
    }

    pub const fn failure_context(&self) -> Option<HostError> {
        self.failure
    }

    fn check_editable<'slot, 'borrow>(
        &self,
        source_id: Option<SourceId>,
        out: Out<'slot, 'borrow, HostError>,
    ) -> OResult<'slot, ()> {
        if !self.context.execution().finished() {
            let error = HostError::with_step(
                HostOperation::SourceNames,
                HostErrorKind::InvalidRequest,
                HostFailureStep::SourceNameOperation,
            );
            return Err(out.fail(match source_id {
                Some(source_id) => error.with_source_id(source_id),
                None => error,
            }));
        }
        Ok(())
    }

    fn active_budget<'slot, 'borrow>(
        &self,
        source_id: SourceId,
        out: Out<'slot, 'borrow, HostError>,
    ) -> OResult<'slot, &ExecutionBudget> {
        if let Some(error) = self.failure {
            return Err(out.fail(error.with_source_id(source_id)));
        }
        Ok(self.context.execution())
    }

    fn copy_name<'slot>(
        id: SourceId,
        input: SourceNameInput<'_>,
        limits: SourceNameLimits,
        budget: &ExecutionBudget,
        mut out: Out<'slot, '_, HostError>,
    ) -> OResult<'slot, SourceName> {
        if input.text.len() > limits.max_name_bytes() {
            return Err(out.fail(
                HostError::with_step(
                    HostOperation::SourceNames,
                    HostErrorKind::ResourceLimitExceeded,
                    HostFailureStep::SourceNameOperation,
                )
                .with_source_id(id),
            ));
        }
        let mut identity_copy = HostError::run_budgeted(
            HostOperation::SourceNames,
            Some(id),
            out.reborrow(),
            |budget_out| ChargedVec::new(Some(budget), budget_out),
        )?;
        HostError::run_budgeted(
            HostOperation::SourceNames,
            Some(id),
            out.reborrow(),
            |budget_out| identity_copy.try_reserve_exact(input.identity.len(), budget_out),
        )?;
        HostError::run_budgeted(
            HostOperation::SourceNames,
            Some(id),
            out.reborrow(),
            |budget_out| identity_copy.extend_from_slice(input.identity, budget_out),
        )?;
        let text = HostError::run_budgeted(
            HostOperation::SourceNames,
            Some(id),
            out.reborrow(),
            |budget_out| ChargedString::copy(input.text, Some(budget), budget_out),
        )?;
        Ok(SourceName {
            id,
            identity: identity_copy,
            text,
        })
    }

    fn find_position<'slot>(
        &self,
        source_id: SourceId,
        budget: &ExecutionBudget,
        mut out: Out<'slot, '_, HostError>,
    ) -> OResult<'slot, (usize, bool)> {
        if let Some(last) = self.entries.last() {
            // The compiler reports files in increasing ID order, so larger IDs append directly; out-of-order IDs use binary search.
            budget.consume_with(1, out.reborrow(), |error| {
                HostError::budget(HostOperation::SourceNames, error).with_source_id(source_id)
            })?;
            match last.id.get().cmp(&source_id.get()) {
                Ordering::Less => return Ok((self.entries.len(), false)),
                Ordering::Equal => return Ok((self.entries.len() - 1, true)),
                Ordering::Greater => {}
            }
        }

        let mut low = 0;
        let mut high = self.entries.len();
        while low < high {
            budget.consume_with(1, out.reborrow(), |error| {
                HostError::budget(HostOperation::SourceNames, error).with_source_id(source_id)
            })?;
            let middle = low + (high - low) / 2;
            let Some(entry) = self.entries.get(middle) else {
                return Err(out.fail(
                    HostError::internal(
                        HostOperation::SourceNames,
                        HostFailureStep::SourceNameOperation,
                    )
                    .with_source_id(source_id),
                ));
            };
            if entry.id.get() < source_id.get() {
                low = middle + 1;
            } else {
                high = middle;
            }
        }

        let found = if let Some(entry) = self.entries.get(low) {
            budget.consume_with(1, out.reborrow(), |error| {
                HostError::budget(HostOperation::SourceNames, error).with_source_id(source_id)
            })?;
            entry.id == source_id
        } else {
            false
        };
        Ok((low, found))
    }

    fn insert_with_budget<'slot>(
        &mut self,
        id: SourceId,
        input: SourceNameInput<'_>,
        budget: &ExecutionBudget,
        mut out: Out<'slot, '_, HostError>,
    ) -> OResult<'slot, ()> {
        let (index, found) = self.find_position(id, budget, out.reborrow())?;
        if found {
            return Ok(());
        }
        if self.entries.len() >= self.limits.max_files() {
            return Err(out.fail(
                HostError::with_step(
                    HostOperation::SourceNames,
                    HostErrorKind::ResourceLimitExceeded,
                    HostFailureStep::SourceNameOperation,
                )
                .with_source_id(id),
            ));
        }
        let entry = Self::copy_name(id, input, self.limits, budget, out.reborrow())?;
        self.insert_at_with_budget(index, entry, budget, out.reborrow())
    }

    fn insert_at_with_budget<'slot>(
        &mut self,
        index: usize,
        entry: SourceName,
        budget: &ExecutionBudget,
        mut out: Out<'slot, '_, HostError>,
    ) -> OResult<'slot, ()> {
        let id = entry.id;
        let original_len = self.entries.len();
        if index > original_len {
            return Err(out.fail(
                HostError::internal(
                    HostOperation::SourceNames,
                    HostFailureStep::SourceNameOperation,
                )
                .with_source_id(id),
            ));
        }
        let moved = original_len - index;
        if moved > 0 {
            budget.consume_with(moved, out.reborrow(), |error| {
                HostError::budget(HostOperation::SourceNames, error).with_source_id(id)
            })?;
        }
        HostError::run_budgeted(
            HostOperation::SourceNames,
            Some(id),
            out.reborrow(),
            |budget_out| self.entries.try_push(entry, budget_out),
        )?;
        if moved > 0 {
            let Some(tail) = self.entries.get_mut(index..) else {
                return Err(out.fail(
                    HostError::internal(
                        HostOperation::SourceNames,
                        HostFailureStep::SourceNameOperation,
                    )
                    .with_source_id(id),
                ));
            };
            tail.rotate_right(1);
        }
        Ok(())
    }

    fn observe<'slot>(
        &mut self,
        id: SourceId,
        identity: &[u8],
        mut out: Out<'slot, '_, HostError>,
    ) -> OResult<'slot, ()> {
        let active = self.active_budget(id, out.reborrow())?;
        let budget = active.try_clone_with(out.reborrow(), |error| {
            HostError::budget(HostOperation::SourceNames, error).with_source_id(id)
        })?;
        let (index, found) = self.find_position(id, &budget, out.reborrow())?;
        if found {
            return Ok(());
        }
        let mut number = id.get();
        let mut digits = [b'0'; u32::BITS as usize];
        let mut start = digits.len();
        loop {
            budget.consume_with(1, out.reborrow(), |error| {
                HostError::budget(HostOperation::SourceNames, error).with_source_id(id)
            })?;
            start = start
                .checked_sub(1)
                .ok_or_else(|| {
                    HostError::internal(
                        HostOperation::SourceNames,
                        HostFailureStep::SourceNameFormatting,
                    )
                    .with_source_id(id)
                })
                .store_err_into(out.reborrow())?;
            let digit = digits
                .get_mut(start)
                .ok_or_else(|| {
                    HostError::internal(
                        HostOperation::SourceNames,
                        HostFailureStep::SourceNameFormatting,
                    )
                    .with_source_id(id)
                })
                .store_err_into(out.reborrow())?;
            *digit = b'0' + (number % 10) as u8;
            number /= 10;
            if number == 0 {
                break;
            }
        }
        let digit_count = digits
            .len()
            .checked_sub(start)
            .ok_or_else(|| {
                HostError::internal(
                    HostOperation::SourceNames,
                    HostFailureStep::SourceNameFormatting,
                )
                .with_source_id(id)
            })
            .store_err_into(out.reborrow())?;
        let prefix_length = b"schema#".len();
        let name_length = prefix_length
            .checked_add(digit_count)
            .ok_or_else(|| {
                HostError::internal(
                    HostOperation::SourceNames,
                    HostFailureStep::SourceNameFormatting,
                )
                .with_source_id(id)
            })
            .store_err_into(out.reborrow())?;
        let charged_steps = name_length
            .checked_mul(3)
            .ok_or_else(|| {
                HostError::internal(
                    HostOperation::SourceNames,
                    HostFailureStep::SourceNameFormatting,
                )
                .with_source_id(id)
            })
            .store_err_into(out.reborrow())?;
        budget.consume_with(charged_steps, out.reborrow(), |error| {
            HostError::budget(HostOperation::SourceNames, error).with_source_id(id)
        })?;
        let mut text = [0; 7 + u32::BITS as usize];
        let prefix = text
            .get_mut(..prefix_length)
            .ok_or_else(|| {
                HostError::internal(
                    HostOperation::SourceNames,
                    HostFailureStep::SourceNameFormatting,
                )
                .with_source_id(id)
            })
            .store_err_into(out.reborrow())?;
        prefix.copy_from_slice(b"schema#");
        let end = prefix_length
            .checked_add(digit_count)
            .ok_or_else(|| {
                HostError::internal(
                    HostOperation::SourceNames,
                    HostFailureStep::SourceNameFormatting,
                )
                .with_source_id(id)
            })
            .store_err_into(out.reborrow())?;
        let destination = text
            .get_mut(prefix_length..end)
            .ok_or_else(|| {
                HostError::internal(
                    HostOperation::SourceNames,
                    HostFailureStep::SourceNameFormatting,
                )
                .with_source_id(id)
            })
            .store_err_into(out.reborrow())?;
        let source = digits
            .get(start..)
            .ok_or_else(|| {
                HostError::internal(
                    HostOperation::SourceNames,
                    HostFailureStep::SourceNameFormatting,
                )
                .with_source_id(id)
            })
            .store_err_into(out.reborrow())?;
        destination.copy_from_slice(source);
        let name_bytes = text
            .get(..end)
            .ok_or_else(|| {
                HostError::internal(
                    HostOperation::SourceNames,
                    HostFailureStep::SourceNameFormatting,
                )
                .with_source_id(id)
            })
            .store_err_into(out.reborrow())?;
        let name = from_utf8(name_bytes)
            .map_err(|_| {
                HostError::internal(
                    HostOperation::SourceNames,
                    HostFailureStep::SourceNameFormatting,
                )
                .with_source_id(id)
            })
            .store_err_into(out.reborrow())?;
        if self.entries.len() >= self.limits.max_files() {
            return Err(out.fail(
                HostError::with_step(
                    HostOperation::SourceNames,
                    HostErrorKind::ResourceLimitExceeded,
                    HostFailureStep::SourceNameOperation,
                )
                .with_source_id(id),
            ));
        }
        let entry = Self::copy_name(
            id,
            SourceNameInput {
                identity,
                text: name,
            },
            self.limits,
            &budget,
            out.reborrow(),
        )?;
        self.insert_at_with_budget(index, entry, &budget, out.reborrow())
    }
}

impl SourceObserver for SourceNames {
    fn source_loaded<'slot, 'borrow>(
        &mut self,
        request: SourceLoadedRequest<'_>,
        out: Out<'slot, 'borrow, SourceObserverError>,
    ) -> OResult<'slot, ()> {
        if !self.context.same_session(request.context)
            || !self
                .entries
                .budget()
                .is_some_and(|budget| budget.same_call(self.context.execution()))
            || self.context.execution().finished()
        {
            let error = HostError::internal(
                HostOperation::SourceNames,
                HostFailureStep::SourceObserverValidation,
            )
            .with_source_id(request.source_id);
            self.failure.get_or_insert(error);
            return Err(out.fail(SourceObserverError::HostFailure));
        }
        let mut errors = take(&mut self.errors);
        let error = {
            let result = errors
                .try_call(|host_out| self.observe(request.source_id, request.identity, host_out));
            match result {
                Ok(()) => None,
                Err(caught) => Some(caught.take()),
            }
        };
        self.errors = errors;
        match error {
            None => Ok(()),
            Some(error) => {
                self.failure.get_or_insert(error);
                let failure = match error.kind() {
                    HostErrorKind::ResourceLimitExceeded => Some(BudgetError::Resource),
                    HostErrorKind::AllocationFailed => Some(BudgetError::Allocation),
                    HostErrorKind::CapacityExceeded => Some(BudgetError::CapacityExceeded),
                    HostErrorKind::InternalError => {
                        Some(BudgetError::invariant(error.budget_context()))
                    }
                    _ => None,
                };
                if let Some(failure) = failure {
                    self.context.execution().record_failure(failure);
                }
                let error = match error.kind() {
                    HostErrorKind::ResourceLimitExceeded => {
                        SourceObserverError::ResourceLimitExceeded
                    }
                    HostErrorKind::AllocationFailed => SourceObserverError::AllocationFailed,
                    HostErrorKind::CapacityExceeded => SourceObserverError::CapacityExceeded,
                    HostErrorKind::InternalError => SourceObserverError::InternalInvariantFailure,
                    _ => SourceObserverError::HostFailure,
                };
                Err(out.fail(error))
            }
        }
    }
}

impl fmt::Debug for SourceNames {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SourceNames")
            .field("count", &self.entries.len())
            .field("failure", &self.failure)
            .finish()
    }
}
