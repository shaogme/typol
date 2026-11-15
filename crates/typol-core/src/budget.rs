use crate::{
    AllocationError, CapacityExceeded, DiagnosticCode, DiagnosticContext,
    DiagnosticContextDescriptor, InternalError, ResourceLimit, diagnostic_context,
    workspace::WorkspaceVec,
};
use alloc::alloc::{Layout, alloc, dealloc};
use core::{
    cmp::Ordering,
    error::Error,
    fmt,
    hint::spin_loop,
    ptr::{self, NonNull},
    sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering as AtomicOrdering, fence},
};
use opeo::{OResult, Out, ResultOutExt};

const PENDING_FAILURE: usize = usize::MAX;

static COUNTER_UNDERFLOW: DiagnosticContext =
    diagnostic_context!("typol-core", "budget", "counter-underflow");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BudgetError {
    ResourceLimit(ResourceLimit),
    Allocation,
    CapacityExceeded(CapacityExceeded),
    Internal(InternalError),
}

impl BudgetError {
    pub const fn invariant(context: &'static DiagnosticContext) -> Self {
        Self::Internal(InternalError::BudgetInvariantFailure(*context))
    }

    pub const fn diagnostic_code(self) -> DiagnosticCode {
        match self {
            Self::ResourceLimit(limit) => limit.diagnostic_code(),
            Self::Allocation => DiagnosticCode::ALLOCATION_FAILED,
            Self::CapacityExceeded(capacity) => capacity.diagnostic_code(),
            Self::Internal(error) => error.diagnostic_code(),
        }
    }

    pub const fn context(self) -> Option<DiagnosticContext> {
        match self {
            Self::Internal(error) => Some(error.context()),
            Self::ResourceLimit(_) | Self::Allocation | Self::CapacityExceeded(_) => None,
        }
    }
}

impl fmt::Display for BudgetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::ResourceLimit(limit) => return write!(formatter, "{limit} limit exceeded"),
            Self::Allocation => "memory allocation failed",
            Self::CapacityExceeded(_) => "workspace capacity exceeded",
            Self::Internal(_) => "internal state invariant violated",
        };
        formatter.write_str(message)
    }
}

impl Error for BudgetError {}

/// Callers set logical-step and explicitly charged live-workspace limits; this does not bound process memory or uncharged operating-system/standard-library temporary allocations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExecutionLimits {
    max_steps: usize,
    max_workspace_bytes: usize,
}

impl ExecutionLimits {
    pub const fn new(max_steps: usize, max_workspace_bytes: usize) -> Self {
        Self {
            max_steps,
            max_workspace_bytes,
        }
    }

    pub fn max_steps(&self) -> usize {
        self.max_steps
    }

    pub fn max_workspace_bytes(&self) -> usize {
        self.max_workspace_bytes
    }
}

struct Counter(AtomicUsize);

impl Counter {
    const fn new(value: usize) -> Self {
        Self(AtomicUsize::new(value))
    }

    fn get(&self) -> usize {
        self.0.load(AtomicOrdering::Relaxed)
    }

    fn increase_optional(&self, amount: usize) -> Option<usize> {
        let mut old = self.get();
        loop {
            let next = old.checked_add(amount)?;
            match self.0.compare_exchange_weak(
                old,
                next,
                AtomicOrdering::Relaxed,
                AtomicOrdering::Relaxed,
            ) {
                Ok(_) => return Some(next),
                Err(actual) => old = actual,
            }
        }
    }

    fn increase<'slot, 'borrow, E>(
        &self,
        amount: usize,
        out: Out<'slot, 'borrow, E>,
        convert: impl FnOnce(BudgetError) -> E,
    ) -> OResult<'slot, usize> {
        let mut old = self.get();
        loop {
            let Some(next) = old.checked_add(amount) else {
                return Err(out.fail(convert(BudgetError::CapacityExceeded(
                    CapacityExceeded::BudgetBookkeeping,
                ))));
            };
            match self.0.compare_exchange_weak(
                old,
                next,
                AtomicOrdering::Relaxed,
                AtomicOrdering::Relaxed,
            ) {
                Ok(_) => return Ok(next),
                Err(actual) => old = actual,
            }
        }
    }

    fn decrease_optional(&self, amount: usize) -> Option<usize> {
        let mut old = self.get();
        loop {
            let next = old.checked_sub(amount)?;
            match self.0.compare_exchange_weak(
                old,
                next,
                AtomicOrdering::Relaxed,
                AtomicOrdering::Relaxed,
            ) {
                Ok(_) => return Some(next),
                Err(actual) => old = actual,
            }
        }
    }

    fn decrease<'slot, 'borrow, E>(
        &self,
        amount: usize,
        out: Out<'slot, 'borrow, E>,
        convert: impl FnOnce(BudgetError) -> E,
    ) -> OResult<'slot, usize> {
        let mut old = self.get();
        loop {
            let Some(next) = old.checked_sub(amount) else {
                let error = BudgetError::invariant(&COUNTER_UNDERFLOW);
                return Err(out.fail(convert(error)));
            };
            match self.0.compare_exchange_weak(
                old,
                next,
                AtomicOrdering::Relaxed,
                AtomicOrdering::Relaxed,
            ) {
                Ok(_) => return Ok(next),
                Err(actual) => old = actual,
            }
        }
    }
}

struct State {
    references: Counter,
    limits: ExecutionLimits,
    steps: Counter,
    workspace: Counter,
    peak: Counter,
    // Regex sessions reuse this otherwise idle counter for their immutable per-search limit.
    text: Counter,
    exhausted: AtomicBool,
    stage: AtomicUsize,
    failure: AtomicUsize,
    failure_context: AtomicPtr<()>,
    finished: AtomicBool,
    regex: bool,
}

struct CounterIncrease<'counter, 'slot, 'borrow, E> {
    current: &'counter Counter,
    amount: usize,
    limit: usize,
    resource: ResourceLimit,
    out: Out<'slot, 'borrow, E>,
}

/// A fallibly created atomic shared budget; handles explicitly increment the atomic reference count through `try_clone`.
pub struct ExecutionBudget {
    state: NonNull<State>,
}

/// A separately accounted Regex execution session. Its fixed bookkeeping allocation is excluded
/// from `max_workspace_bytes`; only Regex-owned buffers and retained compiled storage are charged.
pub struct RegexExecutionBudget {
    execution: ExecutionBudget,
}

impl ExecutionBudget {
    pub fn new<'slot>(
        limits: ExecutionLimits,
        out: Out<'slot, '_, BudgetError>,
    ) -> OResult<'slot, Self> {
        Self::new_inner(limits, true, None, out)
    }

    fn new_inner<'slot>(
        limits: ExecutionLimits,
        charge_state: bool,
        regex_search_steps: Option<usize>,
        mut out: Out<'slot, '_, BudgetError>,
    ) -> OResult<'slot, Self> {
        let bytes = Layout::new::<State>().size();
        if charge_state && bytes > limits.max_workspace_bytes {
            return Err(out.fail(BudgetError::ResourceLimit(ResourceLimit::WorkspaceBytes)));
        }
        // SAFETY: Allocates with the `State` layout, checks for a null pointer, then initializes uniquely owned storage.
        let pointer = unsafe { alloc(Layout::new::<State>()) }.cast::<State>();
        let state = NonNull::new(pointer)
            .ok_or(BudgetError::Allocation)
            .store_err_into(out.reborrow())?;
        unsafe {
            pointer.write(State {
                references: Counter::new(1),
                limits,
                steps: Counter::new(0),
                workspace: Counter::new(if charge_state { bytes } else { 0 }),
                peak: Counter::new(if charge_state { bytes } else { 0 }),
                text: Counter::new(regex_search_steps.unwrap_or(0)),
                exhausted: AtomicBool::new(false),
                stage: AtomicUsize::new(0),
                failure: AtomicUsize::new(0),
                failure_context: AtomicPtr::new(ptr::null_mut()),
                finished: AtomicBool::new(false),
                regex: regex_search_steps.is_some(),
            });
        }
        Ok(Self { state })
    }

    pub fn try_clone<'slot, E>(&self, mut out: Out<'slot, '_, E>) -> OResult<'slot, Self>
    where
        E: From<BudgetError>,
    {
        self.state()
            .references
            .increase(1, out.reborrow(), |error| {
                self.record_failure(error);
                E::from(error)
            })?;
        Ok(Self { state: self.state })
    }

    pub fn try_clone_with<'slot, E>(
        &self,
        mut out: Out<'slot, '_, E>,
        convert: impl FnOnce(BudgetError) -> E,
    ) -> OResult<'slot, Self> {
        self.state()
            .references
            .increase(1, out.reborrow(), |error| {
                self.record_failure(error);
                convert(error)
            })
            .map(|_| Self { state: self.state })
    }

    pub(crate) fn try_clone_optional(&self) -> Option<Self> {
        match self.state().references.increase_optional(1) {
            Some(_) => Some(Self { state: self.state }),
            None => {
                self.record_failure(BudgetError::CapacityExceeded(
                    CapacityExceeded::BudgetBookkeeping,
                ));
                None
            }
        }
    }

    fn state(&self) -> &State {
        // SAFETY: Each handle owns one reference, keeping storage valid until the last handle is dropped.
        unsafe { self.state.as_ref() }
    }

    /// The stage is a call-level hint on the shared budget; concurrent writes are last-writer-wins.
    pub fn set_stage(&self, stage: usize) {
        self.state().stage.store(stage, AtomicOrdering::Relaxed);
    }

    /// Returns the most recently written call-level hint, not a concurrent task's exact stage.
    pub fn stage(&self) -> usize {
        self.state().stage.load(AtomicOrdering::Relaxed)
    }

    pub fn record_failure(&self, error: BudgetError) {
        let (value, context_pointer) = match error {
            BudgetError::ResourceLimit(limit) => (100 + limit.id(), ptr::null_mut()),
            BudgetError::Allocation => (2, ptr::null_mut()),
            BudgetError::CapacityExceeded(capacity) => {
                (capacity.diagnostic_code().get() as usize, ptr::null_mut())
            }
            BudgetError::Internal(internal) => (
                4 + internal.tag(),
                internal.context().descriptor() as *const DiagnosticContextDescriptor as *mut (),
            ),
        };
        if self
            .state()
            .failure
            .compare_exchange(
                0,
                PENDING_FAILURE,
                AtomicOrdering::Acquire,
                AtomicOrdering::Relaxed,
            )
            .is_ok()
        {
            self.state()
                .failure_context
                .store(context_pointer, AtomicOrdering::Relaxed);
            self.state().failure.store(value, AtomicOrdering::Release);
        }
    }

    pub fn failure(&self) -> Option<BudgetError> {
        let value = loop {
            let value = self.state().failure.load(AtomicOrdering::Acquire);
            if value != PENDING_FAILURE {
                break value;
            }
            spin_loop();
        };
        match value {
            2 => Some(BudgetError::Allocation),
            capacity if (70..=84).contains(&capacity) => DiagnosticCode::new(capacity as u32)
                .and_then(CapacityExceeded::from_diagnostic_code)
                .map(BudgetError::CapacityExceeded),
            internal if (4..32).contains(&internal) => {
                let context_pointer = self.state().failure_context.load(AtomicOrdering::Relaxed);
                // SAFETY: The pointer refers to the static descriptor retained by the internal error.
                let descriptor = unsafe {
                    context_pointer
                        .cast::<DiagnosticContextDescriptor>()
                        .as_ref()
                }?;
                InternalError::from_tag(internal - 4, DiagnosticContext::new(descriptor))
                    .map(BudgetError::Internal)
            }
            resource if resource >= 101 => {
                ResourceLimit::from_id(resource - 100).map(BudgetError::ResourceLimit)
            }
            _ => None,
        }
    }

    pub fn finish(&self) {
        self.state().finished.store(true, AtomicOrdering::Relaxed);
    }

    pub fn finished(&self) -> bool {
        self.state().finished.load(AtomicOrdering::Relaxed)
    }

    pub fn limits(&self) -> ExecutionLimits {
        self.state().limits
    }

    pub fn same_call(&self, other: &Self) -> bool {
        self.state == other.state
    }

    pub fn resource_error(&self) -> BudgetError {
        self.limit_error(if self.state().regex {
            ResourceLimit::RegexExecutionSteps
        } else {
            ResourceLimit::ExecutionSteps
        })
    }

    pub fn invariant_error(&self, context: &'static DiagnosticContext) -> BudgetError {
        let error = BudgetError::invariant(context);
        self.record_failure(error);
        error
    }

    pub fn steps(&self) -> usize {
        self.state().steps.get()
    }

    pub fn workspace_bytes(&self) -> usize {
        self.state().workspace.get()
    }

    pub fn peak_workspace_bytes(&self) -> usize {
        self.state().peak.get()
    }

    pub fn constructed_text_bytes(&self) -> usize {
        self.state().text.get()
    }

    pub fn exhausted(&self) -> bool {
        self.state().exhausted.load(AtomicOrdering::Relaxed)
    }

    fn increase_with<'slot, 'borrow, E>(
        &self,
        request: CounterIncrease<'_, 'slot, 'borrow, E>,
        convert: impl FnOnce(BudgetError) -> E,
    ) -> OResult<'slot, usize> {
        let CounterIncrease {
            current,
            amount,
            limit,
            resource,
            out,
        } = request;
        if self.exhausted() {
            return Err(out.fail(convert(self.failure().unwrap_or(
                BudgetError::CapacityExceeded(CapacityExceeded::BudgetBookkeeping),
            ))));
        }
        let mut old = current.get();
        loop {
            if self.exhausted() {
                return Err(out.fail(convert(self.failure().unwrap_or(
                    BudgetError::CapacityExceeded(CapacityExceeded::BudgetBookkeeping),
                ))));
            }
            let Some(next) = old.checked_add(amount) else {
                return Err(out.fail(convert(self.capacity_error())));
            };
            if next > limit {
                return Err(out.fail(convert(self.limit_error(resource))));
            }
            match current.0.compare_exchange_weak(
                old,
                next,
                AtomicOrdering::Relaxed,
                AtomicOrdering::Relaxed,
            ) {
                Ok(_) => return Ok(next),
                Err(actual) => old = actual,
            }
        }
    }

    fn increase<'slot, 'borrow, E>(
        &self,
        request: CounterIncrease<'_, 'slot, 'borrow, E>,
    ) -> OResult<'slot, usize>
    where
        E: From<BudgetError>,
    {
        self.increase_with(request, E::from)
    }

    pub fn consume<'slot, E>(&self, steps: usize, mut out: Out<'slot, '_, E>) -> OResult<'slot, ()>
    where
        E: From<BudgetError>,
    {
        self.increase(CounterIncrease {
            current: &self.state().steps,
            amount: steps,
            limit: self.state().limits.max_steps,
            resource: ResourceLimit::ExecutionSteps,
            out: out.reborrow(),
        })?;
        Ok(())
    }

    pub fn consume_with<'slot, 'borrow, E>(
        &self,
        steps: usize,
        out: Out<'slot, 'borrow, E>,
        convert: impl FnOnce(BudgetError) -> E,
    ) -> OResult<'slot, ()> {
        self.increase_with(
            CounterIncrease {
                current: &self.state().steps,
                amount: steps,
                limit: self.state().limits.max_steps,
                resource: ResourceLimit::ExecutionSteps,
                out,
            },
            convert,
        )
        .map(|_| ())
    }

    pub fn construct_text<'slot, E>(
        &self,
        bytes: usize,
        limit: usize,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, ()>
    where
        E: From<BudgetError>,
    {
        self.increase(CounterIncrease {
            current: &self.state().text,
            amount: bytes,
            limit,
            resource: ResourceLimit::ConfigTextBytes,
            out: out.reborrow(),
        })?;
        Ok(())
    }

    /// Optional reporting work may stop at its limit without changing the workload session’s failure state.
    pub fn consume_optional(&self, steps: usize) -> bool {
        self.increase_optional(&self.state().steps, steps, self.limits().max_steps)
            .is_some()
    }

    pub(crate) fn reserve_optional_workspace(&self, bytes: usize) -> bool {
        let state = self.state();
        let Some(total) =
            self.increase_optional(&state.workspace, bytes, state.limits.max_workspace_bytes)
        else {
            if state.regex {
                self.workspace_error();
            }
            return false;
        };
        state.peak.0.fetch_max(total, AtomicOrdering::Relaxed);
        true
    }

    fn increase_optional(&self, counter: &Counter, amount: usize, limit: usize) -> Option<usize> {
        if self.exhausted() {
            return None;
        }
        let mut old = counter.get();
        loop {
            let next = old.checked_add(amount).filter(|next| *next <= limit)?;
            match counter.0.compare_exchange_weak(
                old,
                next,
                AtomicOrdering::Relaxed,
                AtomicOrdering::Relaxed,
            ) {
                Ok(_) => return Some(next),
                Err(actual) => old = actual,
            }
        }
    }

    pub(crate) fn reserve_workspace<'slot, E>(
        &self,
        bytes: usize,
        out: Out<'slot, '_, E>,
    ) -> OResult<'slot, ()>
    where
        E: From<BudgetError>,
    {
        let state = self.state();
        let limit = if state.regex {
            ResourceLimit::RegexWorkspaceBytes
        } else {
            ResourceLimit::WorkspaceBytes
        };
        let total = self.increase(CounterIncrease {
            current: &state.workspace,
            amount: bytes,
            limit: state.limits.max_workspace_bytes,
            resource: limit,
            out,
        })?;
        state.peak.0.fetch_max(total, AtomicOrdering::Relaxed);
        Ok(())
    }

    pub fn reserve_workspace_with<'slot, 'borrow, E>(
        &self,
        bytes: usize,
        out: Out<'slot, 'borrow, E>,
        convert: impl FnOnce(BudgetError) -> E,
    ) -> OResult<'slot, ()> {
        let state = self.state();
        let limit = if state.regex {
            ResourceLimit::RegexWorkspaceBytes
        } else {
            ResourceLimit::WorkspaceBytes
        };
        let total = self.increase_with(
            CounterIncrease {
                current: &state.workspace,
                amount: bytes,
                limit: state.limits.max_workspace_bytes,
                resource: limit,
                out,
            },
            convert,
        )?;
        state.peak.0.fetch_max(total, AtomicOrdering::Relaxed);
        Ok(())
    }

    pub(crate) fn return_workspace<'slot, 'borrow, E>(
        &self,
        bytes: usize,
        out: Out<'slot, 'borrow, E>,
        convert: impl FnOnce(BudgetError) -> E,
    ) -> OResult<'slot, ()> {
        self.state()
            .workspace
            .decrease(bytes, out, convert)
            .map(|_| ())
    }

    pub(crate) fn return_workspace_optional(&self, bytes: usize) -> bool {
        self.state().workspace.decrease_optional(bytes).is_some()
    }

    fn workspace_error(&self) -> BudgetError {
        let limit = if self.state().regex {
            ResourceLimit::RegexWorkspaceBytes
        } else {
            ResourceLimit::WorkspaceBytes
        };
        self.limit_error(limit)
    }

    fn capacity_error(&self) -> BudgetError {
        let error = BudgetError::CapacityExceeded(CapacityExceeded::BudgetBookkeeping);
        self.record_failure(error);
        self.state().exhausted.store(true, AtomicOrdering::Relaxed);
        error
    }

    fn limit_error(&self, limit: ResourceLimit) -> BudgetError {
        let error = BudgetError::ResourceLimit(limit);
        self.record_failure(error);
        self.state().exhausted.store(true, AtomicOrdering::Relaxed);
        error
    }

    pub(crate) fn is_regex(&self) -> bool {
        self.state().regex
    }

    pub(crate) fn try_regex_clone<'slot, E>(
        &self,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, RegexExecutionBudget>
    where
        E: From<BudgetError>,
    {
        if !self.is_regex() {
            return Err(out.fail(E::from(BudgetError::CapacityExceeded(
                CapacityExceeded::BudgetBookkeeping,
            ))));
        }
        let execution = self.try_clone(out.reborrow())?;
        Ok(RegexExecutionBudget { execution })
    }

    pub fn compare_text<'slot, E>(
        &self,
        left: &str,
        right: &str,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, Ordering>
    where
        E: From<BudgetError>,
    {
        let left = left.as_bytes();
        let right = right.as_bytes();
        let mut left = left.iter();
        let mut right = right.iter();
        let order = loop {
            let a = if let Some(byte) = left.next() {
                self.consume(1, out.reborrow())?;
                Some(*byte)
            } else {
                None
            };
            let b = if let Some(byte) = right.next() {
                self.consume(1, out.reborrow())?;
                Some(*byte)
            } else {
                None
            };
            match (a, b) {
                (Some(a), Some(b)) if a == b => {}
                (a, b) => break a.cmp(&b),
            }
        };
        Ok(order)
    }
}

impl RegexExecutionBudget {
    pub fn new<'slot>(
        max_steps: usize,
        max_search_steps: usize,
        max_workspace_bytes: usize,
        out: Out<'slot, '_, BudgetError>,
    ) -> OResult<'slot, Self> {
        let execution = ExecutionBudget::new_inner(
            ExecutionLimits::new(max_steps, max_workspace_bytes),
            false,
            Some(max_search_steps),
            out,
        )?;
        Ok(Self { execution })
    }

    pub fn try_clone<'slot, E>(&self, mut out: Out<'slot, '_, E>) -> OResult<'slot, Self>
    where
        E: From<BudgetError>,
    {
        let execution = self.execution.try_clone(out.reborrow())?;
        Ok(Self { execution })
    }

    pub fn same_call(&self, other: &Self) -> bool {
        self.execution.same_call(&other.execution)
    }

    pub fn max_steps(&self) -> usize {
        self.execution.limits().max_steps
    }

    pub fn max_search_steps(&self) -> usize {
        self.execution.state().text.get()
    }

    pub fn max_workspace_bytes(&self) -> usize {
        self.execution.limits().max_workspace_bytes
    }

    pub fn steps(&self) -> usize {
        self.execution.steps()
    }

    pub fn workspace_bytes(&self) -> usize {
        self.execution.workspace_bytes()
    }

    pub fn peak_workspace_bytes(&self) -> usize {
        self.execution.peak_workspace_bytes()
    }

    pub fn failure(&self) -> Option<BudgetError> {
        self.execution.failure()
    }

    pub fn exhausted(&self) -> bool {
        self.execution.exhausted()
    }

    pub fn consume<'slot, E>(&self, steps: usize, out: Out<'slot, '_, E>) -> OResult<'slot, ()>
    where
        E: From<BudgetError>,
    {
        self.execution.consume(steps, out)
    }

    pub fn consume_with<'slot, 'borrow, E>(
        &self,
        steps: usize,
        out: Out<'slot, 'borrow, E>,
        convert: impl FnOnce(BudgetError) -> E,
    ) -> OResult<'slot, ()> {
        self.execution.consume_with(steps, out, convert)
    }

    pub fn consume_optional(&self, steps: usize) -> bool {
        self.execution.consume_optional(steps)
    }

    pub fn search_limit_error(&self) -> BudgetError {
        self.fail(BudgetError::ResourceLimit(ResourceLimit::RegexSearchSteps))
    }

    pub fn reserve_workspace<'slot, E>(
        &self,
        bytes: usize,
        out: Out<'slot, '_, E>,
    ) -> OResult<'slot, ()>
    where
        E: From<BudgetError>,
    {
        self.execution.reserve_workspace(bytes, out)
    }

    pub(crate) fn execution(&self) -> &ExecutionBudget {
        &self.execution
    }

    fn fail(&self, error: BudgetError) -> BudgetError {
        self.execution.record_failure(error);
        self.execution
            .state()
            .exhausted
            .store(true, AtomicOrdering::Relaxed);
        error
    }
}

impl Drop for ExecutionBudget {
    fn drop(&mut self) {
        let references = &self.state().references.0;
        let mut current = references.load(AtomicOrdering::Relaxed);
        while let Some(next) = current.checked_sub(1) {
            match references.compare_exchange_weak(
                current,
                next,
                AtomicOrdering::Release,
                AtomicOrdering::Relaxed,
            ) {
                Ok(_) if current == 1 => {
                    fence(AtomicOrdering::Acquire);
                    // SAFETY: The last reference drops the value and deallocates the storage with its original layout.
                    unsafe {
                        self.state.as_ptr().drop_in_place();
                        dealloc(self.state.as_ptr().cast(), Layout::new::<State>());
                    }
                    break;
                }
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
    }
}

impl From<AllocationError> for BudgetError {
    fn from(_: AllocationError) -> Self {
        Self::Allocation
    }
}

// SAFETY: Shared state is accessed only through atomic fields, configured limits are immutable, and the reference count keeps storage alive.
unsafe impl Send for ExecutionBudget {}

// SAFETY: Concurrent charges use CAS, and the final release uses acquire/release synchronization.
unsafe impl Sync for ExecutionBudget {}

/// Reserves an owned slot for failure handling without requesting more capacity.
pub struct TerminalSlot<T> {
    values: WorkspaceVec<T>,
}

/// An owned value stored in a terminal slot.
pub struct FilledTerminalSlot<T> {
    values: WorkspaceVec<T>,
}

impl<T> TerminalSlot<T> {
    pub fn new<'slot>(
        budget: &ExecutionBudget,
        mut out: Out<'slot, '_, BudgetError>,
    ) -> OResult<'slot, Self> {
        let mut values = WorkspaceVec::new(Some(budget), out.reborrow())?;
        values.try_reserve_exact(1, out.reborrow())?;
        Ok(Self { values })
    }

    pub fn store(mut self, value: T) -> Option<FilledTerminalSlot<T>> {
        self.values
            .push_reserved(value)
            .then_some(FilledTerminalSlot {
                values: self.values,
            })
    }
}

impl<T> FilledTerminalSlot<T> {
    pub fn get(&self) -> Option<&T> {
        self.values.first()
    }
}
