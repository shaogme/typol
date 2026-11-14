use crate::{
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
    BudgetError, Diagnostic, DiagnosticCode, DiagnosticContext, DiagnosticMessage, ExecutionBudget,
    LocatedSpan, ValueKind, ValueScalar, ValueSource, diagnostic_context,
};
use typol_i18n::{Translator, t};
use typol_model::BuiltinType;
use typol_semantic::{EnvironmentAuthorization, TypeKind};

use super::{
    CachedEnvironment, ENVIRONMENT_INTERNAL_FALLBACK, EnvironmentDiagnosticRequest,
    EnvironmentDiagnostics, EnvironmentReadRequest,
};

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

pub(crate) struct EnvironmentText;

impl EnvironmentText {
    pub(super) fn decode<'bytes, 'slot>(
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

pub(crate) struct EnvironmentResolver;

pub(crate) struct EnvironmentAuthorizationRequest<'name, 'auth> {
    pub(crate) name: &'name str,
    pub(crate) authorization: Option<EnvironmentAuthorization<'auth>>,
    pub(crate) span: LocatedSpan,
    pub(crate) budget: &'name ExecutionBudget,
    pub(crate) translator: Translator,
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
            translator,
        } = request;
        let mut valid = !name.is_empty();
        for (index, byte) in name.bytes().enumerate() {
            if let Err(caught) = budget_errors.try_call(|budget_out| budget.consume(1, budget_out))
            {
                return Err(out.fail(EnvironmentDiagnostics::located_error(
                    BudgetDiagnostic::from_error(caught.take(), translator),
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
                    return Err(out.fail(EnvironmentDiagnostics::located_error(
                        BudgetDiagnostic::from_error(caught.take(), translator),
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
                    translator,
                ))),
            }
        } else {
            Err(out.fail(EnvironmentDiagnostics::environment(
                EnvironmentDiagnosticRequest {
                    code: DiagnosticCode::CONFIG_VALUE_ERROR,
                    message: DiagnosticMessage::from(
                        t! { name: "diagnostic.environment.name_invalid", locale: translator },
                    ),
                    span,
                    budget,
                    context: None,
                    translator,
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
        let translator = resources.validation.translator;
        let expected_kind = match resources
            .validation
            .schema_access_errors
            .try_call(|access_out| metadata.expected.kind(access_out))
        {
            Ok(kind) => kind,
            Err(caught) => {
                return Err(out.fail(DiagnosticFactory::schema_access(caught.take(), translator)));
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
                        return Err(
                            out.fail(DiagnosticFactory::schema_access(caught.take(), translator))
                        );
                    }
                };
                match inner_kind {
                    TypeKind::Builtin(builtin) => (builtin, true),
                    _ => {
                        return Err(out.fail(Self::target_type_diagnostic(
                            spans.call,
                            authorization.slot(),
                            &resources.budget,
                            translator,
                            &mut resources.validation.budget_errors,
                        )));
                    }
                }
            }
            _ => {
                return Err(out.fail(Self::target_type_diagnostic(
                    spans.call,
                    authorization.slot(),
                    &resources.budget,
                    translator,
                    &mut resources.validation.budget_errors,
                )));
            }
        };
        let name = authorization.name();
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
                return Err(out.fail(Self::environment_error(
                    EnvironmentDiagnosticRequest {
                        code: DiagnosticCode::CONFIG_ENVIRONMENT_ERROR,
                        message: DiagnosticMessage::from(
                            t! { name: "diagnostic.environment.read_failed", locale: translator },
                        ),
                        span: spans.call,
                        budget: &resources.budget,
                        context: None,
                        translator,
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
                    return Err(out.fail(EnvironmentDiagnostics::located_error(
                        BudgetDiagnostic::from_error(error, translator),
                        spans.call,
                    )));
                }
                EnvironmentTextError::Utf8 => {
                    return Err(out.fail(Self::environment_error(
                        EnvironmentDiagnosticRequest {
                            code: DiagnosticCode::CONFIG_ENVIRONMENT_ERROR,
                            message: DiagnosticMessage::from(
                                t! { name: "diagnostic.environment.value_invalid_utf8", locale: translator },
                            ),
                            span: spans.call,
                            budget: &resources.budget,
                            context: None,
                            translator,
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
                translator,
                text,
                builtin,
                span: spans.call,
                budget: Some(&resources.budget),
            },
            &mut resources.validation.budget_errors,
            out.reborrow(),
        )?;
        let fallback_scalar = match fallback.as_ref() {
            Some(fallback) => {
                let value = match (optional, fallback.value.root().as_ref()) {
                    (true, ValueKind::Optional(Some(value))) => value,
                    (false, _) => fallback.value.root(),
                    _ => {
                        return Err(out.fail(DiagnosticFactory::internal(
                            ENVIRONMENT_INTERNAL_FALLBACK,
                            Some(fallback.span),
                            None,
                            Some(ValueSource::ExplicitFallback),
                            translator,
                        )));
                    }
                };
                Some(match value.as_ref() {
                    ValueKind::Signed(value) => ValueScalar::Signed(value),
                    ValueKind::Unsigned(value) => ValueScalar::Unsigned(value),
                    ValueKind::Float32(value) => ValueScalar::Float32(value),
                    ValueKind::Float64(value) => ValueScalar::Float64(value),
                    ValueKind::String(value) => ValueScalar::String(value),
                    ValueKind::Bool(value) => ValueScalar::Bool(value),
                    _ => {
                        return Err(out.fail(DiagnosticFactory::internal(
                            ENVIRONMENT_INTERNAL_FALLBACK,
                            Some(fallback.span),
                            None,
                            Some(ValueSource::ExplicitFallback),
                            translator,
                        )));
                    }
                })
            }
            None => None,
        };
        let budget = &resources.budget;
        let value = if optional {
            let mut value_errors = ErrSlot::new();
            match value_errors.try_call(move |value_out| {
                ValueFactory::optional_owned(value, ValueSource::RustValue, Some(budget), value_out)
            }) {
                Ok(value) => value,
                Err(caught) => {
                    return Err(out.fail(ValueFactory::failure(
                        caught.take(),
                        Some(spans.call),
                        Some(ValueSource::EnvironmentVariable),
                        translator,
                    )));
                }
            }
        } else {
            value
        };
        let value = {
            let mut value_errors = ErrSlot::new();
            match value_errors.try_call(|value_out| {
                ValueFactory::environment(value, name, fallback_scalar, Some(budget), value_out)
            }) {
                Ok(value) => value,
                Err(caught) => {
                    return Err(out.fail(ValueFactory::failure(
                        caught.take(),
                        Some(spans.call),
                        Some(ValueSource::EnvironmentVariable),
                        translator,
                    )));
                }
            }
        };
        Ok(Some(EnvironmentValue {
            value,
            origin: ValueOrigin::Environment { call: spans.call },
            completion: CompletionState::NeedsTraversal,
        }))
    }

    fn target_type_diagnostic(
        span: LocatedSpan,
        slot: usize,
        budget: &ExecutionBudget,
        translator: Translator,
        budget_errors: &mut ErrSlot<BudgetError>,
    ) -> Diagnostic {
        Self::environment_error(
            EnvironmentDiagnosticRequest {
                code: DiagnosticCode::INTERNAL_ERROR,
                message: DiagnosticMessage::from(
                    t! { name: "diagnostic.environment.target_type_invalid", locale: translator },
                ),
                span,
                budget,
                context: Some(ENVIRONMENT_TARGET_TYPE.with_object_id(slot as u64)),
                translator,
            },
            budget_errors,
        )
    }

    fn environment_error(
        request: EnvironmentDiagnosticRequest<'_>,
        budget_errors: &mut ErrSlot<BudgetError>,
    ) -> Diagnostic {
        EnvironmentDiagnostics::environment(request, budget_errors)
    }
}
