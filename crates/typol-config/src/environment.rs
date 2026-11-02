use crate::{
    ConfigLimits,
    context::{
        AssignmentContext, CompletionState, EnvironmentSpans, EnvironmentValue, FallbackValue,
        ValueOrigin,
    },
    diagnostics::{BudgetDiagnostic, DiagnosticFactory},
    options::ConfigBudget,
    scalar::{Scalar, ScalarRequest},
    session::SessionResources,
    value::ValueFactory,
};
use core::str::from_utf8_unchecked;
use opeo::{ErrSlot, OResult, Out};
use typol_core::{
    BudgetError, ChargedVec, Diagnostic, DiagnosticCode, DiagnosticContext, DiagnosticMessage,
    DiagnosticMessageKey, ExecutionBudget, LocatedSpan, ValueSource, diagnostic_context,
};
use typol_i18n::{diagnostic_key, diagnostic_message};
use typol_model::BuiltinType;
use typol_semantic::{EnvironmentAuthorization, TypeKind};

#[cfg(test)]
#[cfg(test)]
use core::str::from_utf8;
#[cfg(test)]
use typol_core::ExecutionLimits;
#[cfg(test)]
use typol_test_support::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EnvironmentError {
    ResourceLimitExceeded,
    AllocationFailed,
    CapacityExceeded,
    InternalError,
    HostFailure,
}

static ENVIRONMENT_PROVIDER_READ: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "environment-read",
    "provider-internal-error",
);
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
static ENVIRONMENT_AUTHORIZATION_MISSING: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "environment-authorization",
    "authorized-slot-missing",
);
static ENVIRONMENT_TARGET_TYPE: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "environment-conversion",
    "authorized-field-is-not-scalar",
);
static ENVIRONMENT_INTERNAL_FALLBACK: DiagnosticContext =
    diagnostic_context!("typol-config", "environment", "unclassified-internal-error",);

/// Explicitly reads an authorized variable; returned bytes need only outlive this mutable borrow.
/// `None` means unset, `Some(&[])` means set to empty, and raw bytes may be non-UTF-8.
/// Providers should charge host work to the shared budget and return no more than the current value byte limit.
/// The byte limit constrains the returned value, not temporary allocations made by the provider or operating system.
#[derive(Clone, Copy)]
pub struct EnvironmentReadContext<'budget> {
    execution: &'budget ExecutionBudget,
    max_value_bytes: usize,
}

impl<'budget> EnvironmentReadContext<'budget> {
    /// Creates a standalone read context; parsers pass the call's shared budget and remaining byte quota.
    pub const fn new(execution: &'budget ExecutionBudget, max_value_bytes: usize) -> Self {
        Self {
            execution,
            max_value_bytes,
        }
    }

    pub const fn execution(&self) -> &'budget ExecutionBudget {
        self.execution
    }

    /// Returns the maximum bytes allowed for this read, including the config layer's remaining cumulative quota.
    pub const fn max_value_bytes(&self) -> usize {
        self.max_value_bytes
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
            return Err(out.fail(resource(span)));
        }
        if index >= authorization.slot_count() {
            return Err(out.fail(cache_internal(ENVIRONMENT_CACHE_SLOT, index, span)));
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
            return Err(out.fail(located_error(caught.take(), span)));
        }
        let mut reserve_errors = ErrSlot::<BudgetError>::new();
        if let Err(caught) =
            reserve_errors.try_call(|reserve_out| self.entries.try_reserve(1, reserve_out))
        {
            return Err(out.fail(located_error(
                BudgetDiagnostic::from_error(caught.take()),
                span,
            )));
        }
        let Some(shifted_entries) = self.entries.len().checked_sub(position) else {
            return Err(out.fail(cache_internal(ENVIRONMENT_CACHE_SLOT, index, span)));
        };
        let Some(insertion_steps) = shifted_entries.checked_add(1) else {
            return Err(out.fail(located_error(
                BudgetDiagnostic::from_error(BudgetError::CapacityExceeded),
                span,
            )));
        };
        let Some(execution) = self.entries.budget() else {
            return Err(out.fail(cache_internal(ENVIRONMENT_CACHE_BUDGET, index, span)));
        };
        charge_environment_work(execution, insertion_steps, span, out.reborrow())?;

        let read_context = EnvironmentReadContext::new(
            self.budget.execution(),
            self.budget
                .environment_bytes_remaining()
                .min(limits.max_string_bytes()),
        );
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
                    )));
                }
                EnvironmentError::ResourceLimitExceeded => {
                    return Err(out.fail(resource(span)));
                }
                EnvironmentError::CapacityExceeded => {
                    return Err(out.fail(located_error(
                        BudgetDiagnostic::from_error(BudgetError::CapacityExceeded),
                        span,
                    )));
                }
                EnvironmentError::InternalError => {
                    return Err(out.fail(cache_internal(ENVIRONMENT_PROVIDER_READ, index, span)));
                }
            },
            Ok(None) => CachedEnvironment::Missing,
            Ok(Some(bytes)) => {
                if bytes.len() > read_context.max_value_bytes() {
                    return Err(out.fail(resource(span)));
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
            return Err(out.fail(located_error(
                BudgetDiagnostic::from_error(caught.take()),
                span,
            )));
        }
        let inserted_len = self.entries.len();
        let Some(ordered_entries) = self.entries.get_mut(position..inserted_len) else {
            return Err(out.fail(cache_internal(ENVIRONMENT_CACHE_SLOT, index, span)));
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
            return Err(out.fail(cache_internal(ENVIRONMENT_CACHE_BUDGET, slot, span)));
        };
        let mut lower = 0;
        let mut upper = self.entries.len();
        while lower < upper {
            charge_environment_work(execution, 1, span, out.reborrow())?;
            let middle = lower + (upper - lower) / 2;
            let Some(entry) = self.entries.get(middle) else {
                return Err(out.fail(cache_internal(ENVIRONMENT_CACHE_SLOT, slot, span)));
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
            _ => Err(out.fail(cache_internal(ENVIRONMENT_CACHE_SLOT, slot, span))),
        }
    }
}

fn charge_environment_work<'slot>(
    budget: &ExecutionBudget,
    steps: usize,
    span: LocatedSpan,
    out: Out<'slot, '_, Diagnostic>,
) -> OResult<'slot, ()> {
    let mut budget_errors = ErrSlot::<BudgetError>::new();
    match budget_errors.try_call(|budget_out| budget.consume(steps, budget_out)) {
        Ok(()) => Ok(()),
        Err(caught) => Err(out.fail(located_error(
            BudgetDiagnostic::from_error(caught.take()),
            span,
        ))),
    }
}

fn cache_internal(context: DiagnosticContext, slot: usize, span: LocatedSpan) -> Diagnostic {
    DiagnosticFactory::internal(context.with_object_id(slot as u64), Some(span), None, None)
}

fn located_error(mut diagnostic: Diagnostic, span: LocatedSpan) -> Diagnostic {
    if diagnostic.located_primary().is_none() {
        diagnostic.set_located_primary(Some(span));
    }
    diagnostic
}

fn resource(span: LocatedSpan) -> Diagnostic {
    DiagnosticFactory::make_diagnostic(
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED,
        diagnostic_message!(
            "diagnostic.environment.resource_limit",
            "environment read exceeded the resource limit"
        ),
        Some(span),
        None,
        None,
    )
}

struct EnvironmentDiagnosticRequest<'a> {
    code: DiagnosticCode,
    key: Option<DiagnosticMessageKey>,
    message: &'a str,
    span: LocatedSpan,
    budget: &'a ExecutionBudget,
    context: Option<DiagnosticContext>,
}

fn environment_diagnostic(
    request: EnvironmentDiagnosticRequest<'_>,
    budget_errors: &mut ErrSlot<BudgetError>,
) -> Diagnostic {
    let EnvironmentDiagnosticRequest {
        code,
        key,
        message,
        span,
        budget,
        context,
    } = request;
    let mut diagnostic = match budget_errors.try_call(|budget_out| {
        Diagnostic::try_new(
            code,
            match key {
                Some(key) => DiagnosticMessage::localized(message, key),
                None => DiagnosticMessage::custom(message),
            },
            Some(budget),
            budget_out,
        )
    }) {
        Ok(diagnostic) => diagnostic,
        Err(caught) => {
            return located_error(BudgetDiagnostic::from_error(caught.take()), span);
        }
    };
    diagnostic.set_located_primary(Some(span));
    diagnostic.set_value_source(Some(ValueSource::EnvironmentVariable));
    diagnostic.set_context(context);
    if code == DiagnosticCode::INTERNAL_ERROR {
        diagnostic.set_context(Some(context.unwrap_or(ENVIRONMENT_INTERNAL_FALLBACK)));
    }
    diagnostic
}

#[derive(Debug)]
pub(crate) enum EnvironmentTextError {
    Budget(BudgetError),
    Utf8,
}

impl From<BudgetError> for EnvironmentTextError {
    fn from(error: BudgetError) -> Self {
        Self::Budget(error)
    }
}

struct EnvironmentText;

impl EnvironmentText {
    fn decode<'bytes, 'slot>(
        bytes: &'bytes [u8],
        budget: &ExecutionBudget,
        mut out: Out<'slot, '_, EnvironmentTextError>,
    ) -> OResult<'slot, &'bytes str> {
        let mut index = 0;
        while index < bytes.len() {
            budget.consume(1, out.reborrow())?;
            let Some(first) = bytes.get(index) else {
                return Err(out.fail(EnvironmentTextError::Utf8));
            };
            let first = *first;
            let width = match first {
                0x00..=0x7f => 1,
                0xc2..=0xdf => 2,
                0xe0..=0xef => 3,
                0xf0..=0xf4 => 4,
                _ => return Err(out.fail(EnvironmentTextError::Utf8)),
            };
            for offset in 1..width {
                let Some(byte_index) = index.checked_add(offset) else {
                    return Err(out.fail(EnvironmentTextError::Utf8));
                };
                budget.consume(1, out.reborrow())?;
                let Some(byte) = bytes.get(byte_index) else {
                    return Err(out.fail(EnvironmentTextError::Utf8));
                };
                let byte = *byte;
                let valid = match (first, offset) {
                    (0xe0, 1) => (0xa0..=0xbf).contains(&byte),
                    (0xed, 1) => (0x80..=0x9f).contains(&byte),
                    (0xf0, 1) => (0x90..=0xbf).contains(&byte),
                    (0xf4, 1) => (0x80..=0x8f).contains(&byte),
                    _ => (0x80..=0xbf).contains(&byte),
                };
                if !valid {
                    return Err(out.fail(EnvironmentTextError::Utf8));
                }
            }
            let Some(next_index) = index.checked_add(width) else {
                return Err(out.fail(EnvironmentTextError::Utf8));
            };
            index = next_index;
        }
        // SAFETY: Each byte is checked for ASCII, continuation-byte form, shortest encoding, surrogate range, and the Unicode upper bound.
        Ok(unsafe { from_utf8_unchecked(bytes) })
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
                    .run(|out| EnvironmentText::decode(&bytes, &budget, out))
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
                .run(|out| EnvironmentText::decode(bytes, &budget, out))
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
                    .run(|out| EnvironmentText::decode(bytes, &budget, out))
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
        let result = decode_errors.run(|out| EnvironmentText::decode(&bytes, &budget, out));
        test_eq!(result.is_ok(), steps == 4)?;
        test_eq!(budget.steps(), steps)?;
    }

    Ok(())
}

pub(crate) struct EnvironmentResolver;

pub(crate) struct EnvironmentAuthorizationRequest<'name, 'auth> {
    pub(crate) name: &'name str,
    pub(crate) authorization: Option<EnvironmentAuthorization<'auth>>,
    pub(crate) span: LocatedSpan,
    pub(crate) budget: &'name ExecutionBudget,
}

pub(crate) struct EnvironmentResolveRequest<'schema, 'auth, 'resources, 'limits> {
    pub(crate) metadata: AssignmentContext<'schema>,
    pub(crate) authorization: EnvironmentAuthorization<'auth>,
    pub(crate) fallback: Option<FallbackValue>,
    pub(crate) spans: EnvironmentSpans,
    pub(crate) resources: &'resources mut SessionResources<'limits>,
}

impl EnvironmentResolver {
    /// Names and authorization are checked before parsing a fallback, and the cache entry point performs the same checks.
    pub(crate) fn authorize<'auth, 'slot, 'borrow>(
        request: EnvironmentAuthorizationRequest<'_, 'auth>,
        budget_errors: &mut ErrSlot<BudgetError>,
        out: Out<'slot, 'borrow, Diagnostic>,
    ) -> OResult<'slot, EnvironmentAuthorization<'auth>> {
        let EnvironmentAuthorizationRequest {
            name,
            authorization,
            span,
            budget,
        } = request;
        let mut valid = !name.is_empty();
        for (index, byte) in name.bytes().enumerate() {
            if let Err(caught) = budget_errors.try_call(|budget_out| budget.consume(1, budget_out))
            {
                return Err(out.fail(located_error(
                    BudgetDiagnostic::from_error(caught.take()),
                    span,
                )));
            }
            if !(byte == b'_' || byte.is_ascii_alphabetic() || (index > 0 && byte.is_ascii_digit()))
            {
                valid = false;
                break;
            }
        }
        let authorized = match authorization {
            Some(authorization) => match budget_errors
                .try_call(|budget_out| budget.compare_text(authorization.name(), name, budget_out))
            {
                Ok(ordering) => ordering.is_eq(),
                Err(caught) => {
                    return Err(out.fail(located_error(
                        BudgetDiagnostic::from_error(caught.take()),
                        span,
                    )));
                }
            },
            None => false,
        };
        if valid && authorized {
            match authorization {
                Some(authorization) => Ok(authorization),
                None => Err(out.fail(DiagnosticFactory::internal(
                    ENVIRONMENT_AUTHORIZATION_MISSING,
                    Some(span),
                    None,
                    Some(ValueSource::EnvironmentVariable),
                ))),
            }
        } else {
            Err(out.fail(environment_diagnostic(
                EnvironmentDiagnosticRequest {
                    code: DiagnosticCode::CONFIG_VALUE_ERROR,
                    key: Some(diagnostic_key!("diagnostic.environment.name_invalid")),
                    message: "environment name is invalid or does not match field authorization",
                    span,
                    budget,
                    context: None,
                },
                budget_errors,
            )))
        }
    }

    pub(crate) fn resolve_environment<'slot>(
        request: EnvironmentResolveRequest<'_, '_, '_, '_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Option<EnvironmentValue>> {
        let EnvironmentResolveRequest {
            metadata,
            authorization,
            fallback,
            spans,
            resources,
        } = request;
        let expected_kind = match resources
            .validation
            .schema_access_errors
            .try_call(|access_out| metadata.expected.kind(access_out))
        {
            Ok(kind) => kind,
            Err(caught) => {
                return Err(out.fail(DiagnosticFactory::schema_access(caught.take())));
            }
        };
        let (builtin, optional) = match expected_kind {
            TypeKind::Builtin(builtin) => (builtin, false),
            TypeKind::Option { inner } => {
                let inner_kind = match resources
                    .validation
                    .schema_access_errors
                    .try_call(|access_out| inner.kind(access_out))
                {
                    Ok(kind) => kind,
                    Err(caught) => {
                        return Err(out.fail(DiagnosticFactory::schema_access(caught.take())));
                    }
                };
                match inner_kind {
                    TypeKind::Builtin(builtin) => (builtin, true),
                    _ => {
                        return Err(out.fail(environment_diagnostic(
                            EnvironmentDiagnosticRequest {
                                code: DiagnosticCode::INTERNAL_ERROR,
                                key: None,
                                message: "environment target is not a scalar",
                                span: spans.call,
                                budget: &resources.budget,
                                context: Some(
                                    ENVIRONMENT_TARGET_TYPE
                                        .with_object_id(authorization.slot() as u64),
                                ),
                            },
                            &mut resources.validation.budget_errors,
                        )));
                    }
                }
            }
            _ => {
                return Err(out.fail(environment_diagnostic(
                    EnvironmentDiagnosticRequest {
                        code: DiagnosticCode::INTERNAL_ERROR,
                        key: None,
                        message: "environment target is not a scalar",
                        span: spans.call,
                        budget: &resources.budget,
                        context: Some(
                            ENVIRONMENT_TARGET_TYPE.with_object_id(authorization.slot() as u64),
                        ),
                    },
                    &mut resources.validation.budget_errors,
                )));
            }
        };
        let cached = resources.cache.read(
            EnvironmentReadRequest {
                provider: resources.environment,
                authorization,
                limits: &resources.config_limits,
                span: spans.call,
            },
            out.reborrow(),
        )?;
        let bytes = match cached {
            CachedEnvironment::Missing => {
                return Ok(fallback.map(|fallback| EnvironmentValue {
                    value: fallback.value,
                    origin: ValueOrigin::ExplicitFallback {
                        value: fallback.span,
                    },
                    completion: CompletionState::Validated(fallback.proof),
                }));
            }
            CachedEnvironment::HostFailure => {
                return Err(out.fail(environment_diagnostic(
                    EnvironmentDiagnosticRequest {
                        code: DiagnosticCode::CONFIG_ENVIRONMENT_ERROR,
                        key: Some(diagnostic_key!("diagnostic.environment.read_failed")),
                        message: "environment read failed",
                        span: spans.call,
                        budget: &resources.budget,
                        context: None,
                    },
                    &mut resources.validation.budget_errors,
                )));
            }
            CachedEnvironment::Bytes(bytes) => bytes,
        };
        let text = match resources
            .validation
            .environment_text_errors
            .try_call(|text_out| EnvironmentText::decode(bytes, &resources.budget, text_out))
        {
            Ok(text) => text,
            Err(caught) => match caught.take() {
                EnvironmentTextError::Budget(error) => {
                    return Err(out.fail(located_error(
                        BudgetDiagnostic::from_error(error),
                        spans.call,
                    )));
                }
                EnvironmentTextError::Utf8 => {
                    return Err(out.fail(environment_diagnostic(
                        EnvironmentDiagnosticRequest {
                            code: DiagnosticCode::CONFIG_ENVIRONMENT_ERROR,
                            key: Some(diagnostic_key!("diagnostic.environment.value_invalid_utf8")),
                            message: "environment value is not valid UTF-8",
                            span: spans.call,
                            budget: &resources.budget,
                            context: None,
                        },
                        &mut resources.validation.budget_errors,
                    )));
                }
            },
        };
        if builtin == BuiltinType::String {
            ConfigBudget::construct_text(
                &resources.budget,
                resources.config_limits,
                text.len(),
                out.reborrow(),
            )?;
        }
        let value = Scalar::convert_budgeted(
            ScalarRequest {
                text,
                builtin,
                span: spans.call,
                budget: Some(&resources.budget),
            },
            &mut resources.validation.budget_errors,
            out.reborrow(),
        )?;
        let value = if optional {
            let mut value_errors = ErrSlot::new();
            match value_errors.try_call(move |value_out| {
                ValueFactory::optional_owned(
                    value,
                    ValueSource::EnvironmentVariable,
                    Some(&resources.budget),
                    value_out,
                )
            }) {
                Ok(value) => value,
                Err(caught) => {
                    return Err(out.fail(ValueFactory::failure(
                        caught.take(),
                        Some(spans.call),
                        Some(ValueSource::EnvironmentVariable),
                    )));
                }
            }
        } else {
            value
        };
        Ok(Some(EnvironmentValue {
            value,
            origin: ValueOrigin::Environment { call: spans.call },
            completion: CompletionState::NeedsTraversal,
        }))
    }
}
