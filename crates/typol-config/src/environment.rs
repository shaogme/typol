mod resolver;

pub(crate) use resolver::{
    EnvironmentAuthorizationRequest, EnvironmentResolveRequest, EnvironmentResolver,
    EnvironmentTextError,
};

use crate::{
    ConfigLimits,
    diagnostics::{BudgetDiagnostic, DetailedDiagnosticRequest, DiagnosticFactory},
    options::ConfigBudget,
};
use opeo::{ErrSlot, OResult, Out};
use typol_core::{
    BudgetError, CapacityExceeded, ChargedVec, Diagnostic, DiagnosticCode, DiagnosticContext,
    DiagnosticMessage, ExecutionBudget, InternalError, LocatedSpan, ResourceLimit, ValueSource,
    diagnostic_context,
};
use typol_i18n::Translator;
use typol_semantic::EnvironmentAuthorization;

#[cfg(test)]
use core::str::from_utf8;
#[cfg(test)]
use typol_core::ExecutionLimits;
#[cfg(test)]
use typol_test_support::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EnvironmentError {
    ResourceLimit(ResourceLimit),
    AllocationFailed,
    CapacityExceeded(CapacityExceeded),
    Internal(InternalError),
    HostFailure,
}

static ENVIRONMENT_CACHE_SLOT: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "environment-cache",
    "authorized-slot-unavailable",
);
static ENVIRONMENT_CACHE_BUDGET: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "environment-cache",
    "execution-budget-unavailable",
);
pub(super) static ENVIRONMENT_INTERNAL_FALLBACK: DiagnosticContext =
    diagnostic_context!("typol-config", "environment", "unclassified-internal-error",);

/// Explicitly reads an authorized variable; returned bytes need only outlive this mutable borrow.
/// `None` means unset, `Some(&[])` means set to empty, and raw bytes may be non-UTF-8.
/// Providers should charge host work to the shared budget and return no more than the current value byte limit.
/// The byte limit constrains the returned value, not temporary allocations made by the provider or operating system.
#[derive(Clone, Copy)]
pub struct EnvironmentReadContext<'budget> {
    execution: &'budget ExecutionBudget,
    max_value_bytes: usize,
    value_limit: ResourceLimit,
}

impl<'budget> EnvironmentReadContext<'budget> {
    /// Creates a standalone read context; parsers pass the call's shared budget and remaining byte quota.
    pub const fn new(execution: &'budget ExecutionBudget, max_value_bytes: usize) -> Self {
        Self {
            execution,
            max_value_bytes,
            value_limit: ResourceLimit::EnvironmentBytes,
        }
    }

    pub const fn with_value_limit(mut self, value_limit: ResourceLimit) -> Self {
        self.value_limit = value_limit;
        self
    }

    pub const fn execution(&self) -> &'budget ExecutionBudget {
        self.execution
    }

    /// Returns the maximum bytes allowed for this read, including the config layer's remaining cumulative quota.
    pub const fn max_value_bytes(&self) -> usize {
        self.max_value_bytes
    }

    pub const fn value_limit(&self) -> ResourceLimit {
        self.value_limit
    }
}

pub trait Environment {
    fn read<'env, 'slot, 'borrow>(
        &'env mut self,
        name: &str,
        context: EnvironmentReadContext<'_>,
        out: Out<'slot, 'borrow, EnvironmentError>,
    ) -> OResult<'slot, Option<&'env [u8]>>;
}

/// Never reads the host environment and treats every variable name as unset.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EmptyEnvironment;

impl Environment for EmptyEnvironment {
    fn read<'env, 'slot, 'borrow>(
        &'env mut self,
        _name: &str,
        _context: EnvironmentReadContext<'_>,
        _out: Out<'slot, 'borrow, EnvironmentError>,
    ) -> OResult<'slot, Option<&'env [u8]>> {
        Ok(None)
    }
}

pub(crate) enum CachedEnvironment {
    Missing,
    Bytes(ChargedVec<u8>),
    HostFailure,
}

struct EnvironmentCacheEntry {
    schema_slot: usize,
    value: CachedEnvironment,
}

enum CachePosition {
    Found(usize),
    Missing(usize),
}

pub(crate) struct EnvironmentCache {
    entries: ChargedVec<EnvironmentCacheEntry>,
    budget: ConfigBudget,
    provider_errors: ErrSlot<EnvironmentError>,
    translator: Translator,
}

pub(crate) struct EnvironmentReadRequest<'a> {
    pub(crate) provider: &'a mut dyn Environment,
    pub(crate) authorization: EnvironmentAuthorization<'a>,
    pub(crate) limits: &'a ConfigLimits,
    pub(crate) span: LocatedSpan,
}

impl EnvironmentCache {
    pub(crate) fn new<'slot, E>(
        budget: &ExecutionBudget,
        limits: ConfigLimits,
        translator: Translator,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, Self>
    where
        E: From<BudgetError>,
    {
        let entries = ChargedVec::new(Some(budget), out.reborrow())?;
        let budget = ConfigBudget::new(limits, budget, out.reborrow())?;
        Ok(Self {
            entries,
            budget,
            provider_errors: ErrSlot::new(),
            translator,
        })
    }

    /// Callers must check name and field authorization first; the cache does not store field conversion results.
    pub(crate) fn read<'slot>(
        &mut self,
        request: EnvironmentReadRequest<'_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, &CachedEnvironment> {
        let EnvironmentReadRequest {
            provider,
            authorization,
            limits,
            span,
        } = request;
        let name = authorization.name();
        let index = authorization.slot();
        if name.len() > limits.max_string_bytes() {
            return Err(out.fail(EnvironmentDiagnostics::resource(
                ResourceLimit::StringBytes,
                span,
                self.translator,
            )));
        }
        if index >= authorization.slot_count() {
            return Err(out.fail(cache_internal(
                ENVIRONMENT_CACHE_SLOT,
                index,
                span,
                self.translator,
            )));
        }

        let position = match self.find_slot(index, span, out.reborrow())? {
            CachePosition::Found(position) => {
                return self.cached_value(position, index, span, out.reborrow());
            }
            CachePosition::Missing(position) => position,
        };
        let mut quota_errors = ErrSlot::new();
        if let Err(caught) =
            quota_errors.try_call(|quota_out| self.budget.environment_entry(quota_out))
        {
            return Err(out.fail(EnvironmentDiagnostics::located_error(caught.take(), span)));
        }
        let mut reserve_errors = ErrSlot::<BudgetError>::new();
        if let Err(caught) =
            reserve_errors.try_call(|reserve_out| self.entries.try_reserve(1, reserve_out))
        {
            return Err(out.fail(EnvironmentDiagnostics::located_error(
                BudgetDiagnostic::from_error(caught.take(), self.translator),
                span,
            )));
        }
        let Some(shifted_entries) = self.entries.len().checked_sub(position) else {
            return Err(out.fail(cache_internal(
                ENVIRONMENT_CACHE_SLOT,
                index,
                span,
                self.translator,
            )));
        };
        let Some(insertion_steps) = shifted_entries.checked_add(1) else {
            return Err(out.fail(EnvironmentDiagnostics::located_error(
                BudgetDiagnostic::from_error(
                    BudgetError::CapacityExceeded(CapacityExceeded::EnvironmentCache),
                    self.translator,
                ),
                span,
            )));
        };
        let Some(execution) = self.entries.budget() else {
            return Err(out.fail(cache_internal(
                ENVIRONMENT_CACHE_BUDGET,
                index,
                span,
                self.translator,
            )));
        };
        charge_environment_work(
            execution,
            insertion_steps,
            span,
            self.translator,
            out.reborrow(),
        )?;

        let environment_bytes_remaining = self.budget.environment_bytes_remaining();
        let max_string_bytes = limits.max_string_bytes();
        let (max_value_bytes, value_limit) = if environment_bytes_remaining <= max_string_bytes {
            (environment_bytes_remaining, ResourceLimit::EnvironmentBytes)
        } else {
            (max_string_bytes, ResourceLimit::StringBytes)
        };
        let read_context = EnvironmentReadContext::new(self.budget.execution(), max_value_bytes)
            .with_value_limit(value_limit);
        let value = match self
            .provider_errors
            .try_call(|provider_out| provider.read(name, read_context, provider_out))
        {
            Err(caught) => match caught.take() {
                EnvironmentError::HostFailure => CachedEnvironment::HostFailure,
                EnvironmentError::AllocationFailed => {
                    return Err(out.fail(DiagnosticFactory::allocation_diagnostic(
                        Some(span),
                        None,
                        None,
                        self.translator,
                    )));
                }
                EnvironmentError::ResourceLimit(limit) => {
                    return Err(out.fail(EnvironmentDiagnostics::resource(
                        limit,
                        span,
                        self.translator,
                    )));
                }
                EnvironmentError::CapacityExceeded(capacity) => {
                    return Err(out.fail(EnvironmentDiagnostics::located_error(
                        BudgetDiagnostic::from_error(
                            BudgetError::CapacityExceeded(capacity),
                            self.translator,
                        ),
                        span,
                    )));
                }
                EnvironmentError::Internal(internal) => {
                    return Err(out.fail(EnvironmentDiagnostics::located_error(
                        DiagnosticFactory::internal_error(
                            internal,
                            Some(span),
                            None,
                            Some(ValueSource::EnvironmentVariable),
                            self.translator,
                        ),
                        span,
                    )));
                }
            },
            Ok(None) => CachedEnvironment::Missing,
            Ok(Some(bytes)) => {
                if bytes.len() > read_context.max_value_bytes() {
                    return Err(out.fail(EnvironmentDiagnostics::resource(
                        read_context.value_limit(),
                        span,
                        self.translator,
                    )));
                }
                self.budget.environment_bytes(bytes.len(), out.reborrow())?;
                let mut copy = ChargedVec::new(self.entries.budget(), out.reborrow())?;
                copy.try_reserve_exact(bytes.len(), out.reborrow())?;
                copy.extend(bytes.iter().copied(), out.reborrow())?;
                CachedEnvironment::Bytes(copy)
            }
        };

        let mut insert_errors = ErrSlot::<BudgetError>::new();
        if let Err(caught) = insert_errors.try_call(|insert_out| {
            self.entries.try_push(
                EnvironmentCacheEntry {
                    schema_slot: index,
                    value,
                },
                insert_out,
            )
        }) {
            return Err(out.fail(EnvironmentDiagnostics::located_error(
                BudgetDiagnostic::from_error(caught.take(), self.translator),
                span,
            )));
        }
        let inserted_len = self.entries.len();
        let Some(ordered_entries) = self.entries.get_mut(position..inserted_len) else {
            return Err(out.fail(cache_internal(
                ENVIRONMENT_CACHE_SLOT,
                index,
                span,
                self.translator,
            )));
        };
        ordered_entries.rotate_right(1);
        self.cached_value(position, index, span, out)
    }

    fn find_slot<'slot>(
        &self,
        slot: usize,
        span: LocatedSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, CachePosition> {
        let Some(execution) = self.entries.budget() else {
            return Err(out.fail(cache_internal(
                ENVIRONMENT_CACHE_BUDGET,
                slot,
                span,
                self.translator,
            )));
        };
        let mut lower = 0;
        let mut upper = self.entries.len();
        while lower < upper {
            charge_environment_work(execution, 1, span, self.translator, out.reborrow())?;
            let middle = lower + (upper - lower) / 2;
            let Some(entry) = self.entries.get(middle) else {
                return Err(out.fail(cache_internal(
                    ENVIRONMENT_CACHE_SLOT,
                    slot,
                    span,
                    self.translator,
                )));
            };
            if entry.schema_slot < slot {
                lower = middle + 1;
            } else {
                upper = middle;
            }
        }
        match self.entries.get(lower) {
            Some(entry) if entry.schema_slot == slot => Ok(CachePosition::Found(lower)),
            _ => Ok(CachePosition::Missing(lower)),
        }
    }

    fn cached_value<'slot>(
        &self,
        position: usize,
        slot: usize,
        span: LocatedSpan,
        out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, &CachedEnvironment> {
        match self.entries.get(position) {
            Some(entry) if entry.schema_slot == slot => Ok(&entry.value),
            _ => Err(out.fail(cache_internal(
                ENVIRONMENT_CACHE_SLOT,
                slot,
                span,
                self.translator,
            ))),
        }
    }
}

fn charge_environment_work<'slot>(
    budget: &ExecutionBudget,
    steps: usize,
    span: LocatedSpan,
    translator: Translator,
    out: Out<'slot, '_, Diagnostic>,
) -> OResult<'slot, ()> {
    let mut budget_errors = ErrSlot::<BudgetError>::new();
    match budget_errors.try_call(|budget_out| budget.consume(steps, budget_out)) {
        Ok(()) => Ok(()),
        Err(caught) => Err(out.fail(EnvironmentDiagnostics::located_error(
            BudgetDiagnostic::from_error(caught.take(), translator),
            span,
        ))),
    }
}

fn cache_internal(
    context: DiagnosticContext,
    slot: usize,
    span: LocatedSpan,
    translator: Translator,
) -> Diagnostic {
    DiagnosticFactory::internal_error(
        InternalError::EnvironmentInvariantFailure(context.with_object_id(slot as u64)),
        Some(span),
        None,
        None,
        translator,
    )
}

pub(super) struct EnvironmentDiagnosticRequest<'a> {
    pub(super) code: DiagnosticCode,
    pub(super) message: DiagnosticMessage<'a>,
    pub(super) span: LocatedSpan,
    pub(super) budget: &'a ExecutionBudget,
    pub(super) context: Option<DiagnosticContext>,
    pub(super) translator: Translator,
}

pub(super) struct EnvironmentDiagnostics;

impl EnvironmentDiagnostics {
    pub(super) fn located_error(mut diagnostic: Diagnostic, span: LocatedSpan) -> Diagnostic {
        if diagnostic.located_primary().is_none() {
            diagnostic.set_located_primary(Some(span));
        }
        diagnostic
    }

    pub(super) fn resource(
        limit: ResourceLimit,
        span: LocatedSpan,
        translator: Translator,
    ) -> Diagnostic {
        DiagnosticFactory::make_diagnostic(DetailedDiagnosticRequest {
            code: limit.diagnostic_code(),
            message: limit.emergency_message(translator).into(),
            primary: Some(span),
            path: None,
            value_source: None,
            context: None,
            translator,
        })
    }

    pub(super) fn environment(
        request: EnvironmentDiagnosticRequest<'_>,
        budget_errors: &mut ErrSlot<BudgetError>,
    ) -> Diagnostic {
        let EnvironmentDiagnosticRequest {
            code,
            message,
            span,
            budget,
            context,
            translator,
        } = request;
        let mut diagnostic = match budget_errors
            .try_call(|budget_out| Diagnostic::try_new(code, message, Some(budget), budget_out))
        {
            Ok(diagnostic) => diagnostic,
            Err(caught) => {
                return Self::located_error(
                    BudgetDiagnostic::from_error(caught.take(), translator),
                    span,
                );
            }
        };
        diagnostic.set_located_primary(Some(span));
        diagnostic.set_value_source(Some(ValueSource::EnvironmentVariable));
        diagnostic.set_context(context);
        if code.is_internal_error() {
            diagnostic.set_context(Some(context.unwrap_or(ENVIRONMENT_INTERNAL_FALLBACK)));
        }
        diagnostic
    }
}

#[cfg(test)]
#[test]
fn metered_utf8_validation_matches_core_for_boundaries_and_two_byte_inputs() -> TestResult<()> {
    let mut budget_errors = TestSlot::default();
    let budget = budget_errors
        .run(|out| ExecutionBudget::new(ExecutionLimits::new(1_000_000, 1_048_576), out))
        .test_value()?;
    let mut decode_errors = TestSlot::default();
    for first in 0..=u8::MAX {
        for second in 0..=u8::MAX {
            let bytes = [first, second];
            test_eq!(
                decode_errors
                    .run(|out| resolver::EnvironmentText::decode(&bytes, &budget, out))
                    .value()
                    .copied(),
                from_utf8(&bytes).ok()
            )?;
        }
    }
    let cases: &[&[u8]] = &[
        b"",
        b"ASCII",
        &[0xc0, 0x80],
        &[0xc1, 0xbf],
        &[0xe0, 0xa0, 0x80],
        &[0xed, 0x9f, 0xbf],
        &[0xed, 0xa0, 0x80],
        &[0xf0, 0x90, 0x80, 0x80],
        &[0xf4, 0x8f, 0xbf, 0xbf],
        &[0xf4, 0x90, 0x80, 0x80],
        &[0xf5, 0x80, 0x80, 0x80],
        &[0xe0, 0x80, 0x80],
        &[0xf0, 0x80, 0x80, 0x80],
        &[0xe2, 0x82],
        &[0xf0, 0x9f, 0x98],
    ];
    for bytes in cases {
        test_eq!(
            decode_errors
                .run(|out| resolver::EnvironmentText::decode(bytes, &budget, out))
                .value()
                .copied(),
            from_utf8(bytes).ok()
        )?;
    }
    let mut seed = 0x1937_abcdu32;
    for _ in 0..10_000 {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let bytes = seed.to_le_bytes();
        for length in 0..=4 {
            let bytes = bytes.get(..length).test_value()?;
            test_eq!(
                decode_errors
                    .run(|out| resolver::EnvironmentText::decode(bytes, &budget, out))
                    .value()
                    .copied(),
                from_utf8(bytes).ok()
            )?;
        }
    }

    Ok(())
}

#[cfg(test)]
#[test]
fn utf8_validator_stops_at_exact_byte_boundary() -> TestResult<()> {
    let bytes = [0xf0, 0x9f, 0x98, 0x80];
    let mut budget_errors = TestSlot::default();
    let mut decode_errors = TestSlot::default();
    for steps in [4, 3] {
        let budget = budget_errors
            .run(|out| ExecutionBudget::new(ExecutionLimits::new(steps, 1_048_576), out))
            .test_value()?;
        let result =
            decode_errors.run(|out| resolver::EnvironmentText::decode(&bytes, &budget, out));
        test_eq!(result.is_ok(), steps == 4)?;
        test_eq!(budget.steps(), steps)?;
    }

    Ok(())
}
