mod diagnostic;

pub(super) use diagnostic::{
    ContextDiagnosticRequest, FixedArrayLengthMismatchRequest, LiteralDiagnostic,
};

use super::{
    containers::ContainerSupport,
    context::{ParserContext, ParserControl, ValueCallContext},
    recovery::ParseFailure,
};
use crate::{
    context::CompletionState, diagnostics::DiagnosticFactory, scalar::Scalar, value::ValueFactory,
};
use opeo::{ErrSlot, OResult, Out};
use typol_core::{
    ConfigValue, Diagnostic, DiagnosticCode, DiagnosticContext, DiagnosticMessage, InstancePath,
    LocatedSpan, ValueError, ValueErrorKind, ValueSource, diagnostic_context,
};
use typol_syntax::{BuiltinType, Keyword, Symbol, TokenKind};

use typol_i18n::t;

#[cfg(test)]
use typol_core::{BudgetError, SourceId, SourceSpan, Span};
#[cfg(test)]
use typol_test_support::{TestFailure, TestResult, test_eq};

static PARSER_SCALAR_TOKEN_PAYLOAD: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "parser-scalar",
    "string-token-decoded-content-missing",
);
static PARSER_SCALAR_INTEGER_TYPE: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "parser-scalar",
    "schema-integer-width-unavailable",
);

#[derive(Clone, Copy)]
enum ScalarFailureKind {
    ExpectedValue,
    TypeMismatch,
    NegativeWithoutNumber,
    StringTokenPayloadMissing,
    StringLimitExceeded,
    IntegerOutOfRange,
    FloatOutOfRange,
    IntegerTypeUnavailable,
    ValueConstruction {
        kind: ValueErrorKind,
        context: Option<DiagnosticContext>,
    },
}

impl ScalarFailureKind {
    const fn code(self) -> DiagnosticCode {
        match self {
            Self::ExpectedValue | Self::NegativeWithoutNumber => {
                DiagnosticCode::CONFIG_SYNTAX_ERROR
            }
            Self::TypeMismatch | Self::IntegerOutOfRange | Self::FloatOutOfRange => {
                DiagnosticCode::CONFIG_VALUE_ERROR
            }
            Self::StringTokenPayloadMissing | Self::IntegerTypeUnavailable => {
                DiagnosticCode::INTERNAL_ERROR
            }
            Self::StringLimitExceeded => DiagnosticCode::RESOURCE_LIMIT_EXCEEDED,
            Self::ValueConstruction { kind, .. } => ValueFactory::failure_code_for_kind(kind),
        }
    }

    fn context(self) -> Option<DiagnosticContext> {
        match self {
            Self::StringTokenPayloadMissing => Some(PARSER_SCALAR_TOKEN_PAYLOAD),
            Self::IntegerTypeUnavailable => Some(PARSER_SCALAR_INTEGER_TYPE),
            Self::ValueConstruction { context, .. } => context,
            Self::ExpectedValue
            | Self::TypeMismatch
            | Self::NegativeWithoutNumber
            | Self::StringLimitExceeded
            | Self::IntegerOutOfRange
            | Self::FloatOutOfRange => None,
        }
    }
}

struct ScalarFailure {
    kind: ScalarFailureKind,
    span: LocatedSpan,
}

struct ValueToken<'a> {
    span: LocatedSpan,
    kind: TokenKind,
    raw_text: &'a str,
    decoded_text: Option<&'a str>,
}

pub(super) struct ParsedValue {
    pub(super) value: ConfigValue,
    pub(super) span: LocatedSpan,
    pub(super) completion: CompletionState,
}

pub(super) struct ExpectedSymbol {
    pub(super) expected: Symbol,
    pub(super) code: DiagnosticCode,
    pub(super) message: DiagnosticMessage<'static>,
}

pub(super) struct ExpectedValueSymbol {
    pub(super) expected: Symbol,
    pub(super) syntax_message: DiagnosticMessage<'static>,
    pub(super) value_message: DiagnosticMessage<'static>,
}

fn starts_value(kind: TokenKind, raw_text: &str) -> bool {
    matches!(
        kind,
        TokenKind::Integer
            | TokenKind::Float
            | TokenKind::String
            | TokenKind::Keyword(Keyword::True | Keyword::False | Keyword::None)
            | TokenKind::Symbol(
                Symbol::LeftBrace | Symbol::LeftBracket | Symbol::Dot | Symbol::Minus
            )
    ) || (kind == TokenKind::Identifier && raw_text == "env")
}

fn is_numeric(expected: BuiltinType) -> bool {
    matches!(
        expected,
        BuiltinType::I8
            | BuiltinType::I16
            | BuiltinType::I32
            | BuiltinType::I64
            | BuiltinType::I128
            | BuiltinType::U8
            | BuiltinType::U16
            | BuiltinType::U32
            | BuiltinType::U64
            | BuiltinType::U128
            | BuiltinType::F32
            | BuiltinType::F64
    )
}

pub(super) struct Syntax;

impl Syntax {
    pub(super) fn expect_symbol<'slot>(
        context: &mut ParserContext<'_, '_, '_>,
        expectation: ExpectedSymbol,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, LocatedSpan> {
        let ExpectedSymbol {
            expected,
            code,
            message,
        } = expectation;
        let (span, matches) = {
            let token = context.cursor.peek(out.reborrow())?;
            (token.span(), token.kind() == TokenKind::Symbol(expected))
        };
        if !matches {
            let diagnostic = ParserControl::diagnostic_call(
                &context.diagnostic_errors,
                |diagnostic_out| {
                    LiteralParser::context_diagnostic(
                        ContextDiagnosticRequest {
                            context,
                            code,
                            message,
                            span,
                            value_source: None,
                        },
                        diagnostic_out,
                    )
                },
                out.reborrow(),
            )?;
            return Err(out.fail(diagnostic.into()));
        }
        context.cursor.bump(out.reborrow())?;
        Ok(span)
    }

    pub(super) fn expect_value_symbol<'slot>(
        context: &mut ParserContext<'_, '_, '_>,
        expectation: ExpectedValueSymbol,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, LocatedSpan> {
        let ExpectedValueSymbol {
            expected,
            syntax_message,
            value_message,
        } = expectation;
        let ValueToken {
            span,
            kind,
            raw_text,
            ..
        } = {
            let token = context.cursor.peek(out.reborrow())?;
            ValueToken {
                span: token.span(),
                kind: token.kind(),
                raw_text: token.text().unwrap_or(""),
                decoded_text: None,
            }
        };
        if kind != TokenKind::Symbol(expected) {
            let code = if Self::is_value_start(kind, raw_text) {
                DiagnosticCode::CONFIG_VALUE_ERROR
            } else {
                DiagnosticCode::CONFIG_SYNTAX_ERROR
            };
            let diagnostic = ParserControl::diagnostic_call(
                &context.diagnostic_errors,
                |diagnostic_out| {
                    LiteralParser::context_diagnostic(
                        ContextDiagnosticRequest {
                            context,
                            code,
                            message: if code == DiagnosticCode::CONFIG_VALUE_ERROR {
                                value_message
                            } else {
                                syntax_message
                            },
                            span,
                            value_source: None,
                        },
                        diagnostic_out,
                    )
                },
                out.reborrow(),
            )?;
            return Err(out.fail(diagnostic.into()));
        }
        context.cursor.bump(out.reborrow())?;
        Ok(span)
    }

    fn is_value_start(kind: TokenKind, raw_text: &str) -> bool {
        matches!(
            kind,
            TokenKind::Integer
                | TokenKind::Float
                | TokenKind::String
                | TokenKind::Keyword(Keyword::True | Keyword::False | Keyword::None)
                | TokenKind::Symbol(
                    Symbol::LeftBrace | Symbol::LeftBracket | Symbol::Dot | Symbol::Minus
                )
        ) || (kind == TokenKind::Identifier && raw_text == "env")
    }
}

pub(crate) struct LiteralParser;

impl LiteralParser {
    fn build_value<'slot>(
        errors: &mut ErrSlot<ValueError>,
        span: LocatedSpan,
        operation: impl for<'call> FnOnce(Out<'call, 'call, ValueError>) -> OResult<'call, ConfigValue>,
    ) -> Result<ConfigValue, ScalarFailure> {
        match errors.try_call(operation) {
            Ok(value) => Ok(value),
            Err(caught) => {
                let error = caught.take();
                Err(ScalarFailure {
                    kind: ScalarFailureKind::ValueConstruction {
                        kind: error.kind(),
                        context: ValueFactory::failure_context(error),
                    },
                    span,
                })
            }
        }
    }

    pub(super) fn parse_scalar<'slot>(
        context: &mut ParserContext<'_, '_, '_>,
        expected: BuiltinType,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, ParsedValue> {
        let negative_span = if is_numeric(expected) {
            let span = {
                let token = context.cursor.peek(out.reborrow())?;
                (token.kind() == TokenKind::Symbol(Symbol::Minus)).then_some(token.span())
            };
            if let Some(span) = span {
                context.cursor.bump(out.reborrow())?;
                Some(span)
            } else {
                None
            }
        } else {
            None
        };

        let budget = context.budget.try_clone(out.reborrow())?;
        let text_limit = context.config_limits.max_constructed_text_bytes();
        let ValueToken {
            span: token_span,
            kind: token_kind,
            raw_text: token_text,
            decoded_text,
        } = {
            let token = context.cursor.peek(out.reborrow())?;
            ValueToken {
                span: token.span(),
                kind: token.kind(),
                raw_text: token.text().unwrap_or(""),
                decoded_text: token.decoded_string(),
            }
        };
        let parsed = {
            let full_span = match negative_span {
                Some(negative) => ParserControl::diagnostic_call(
                    &context.diagnostic_errors,
                    |diagnostic_out| {
                        DiagnosticFactory::span_through(
                            negative,
                            token_span,
                            context.session.translator(),
                            diagnostic_out,
                        )
                    },
                    out.reborrow(),
                )?,
                None => token_span,
            };
            if matches!(token_kind, TokenKind::Integer | TokenKind::Float) {
                budget.consume(token_text.len(), out.reborrow())?;
            }
            let negative = negative_span.is_some();
            let mismatch_kind = if starts_value(token_kind, token_text) {
                ScalarFailureKind::TypeMismatch
            } else {
                ScalarFailureKind::ExpectedValue
            };

            if negative && !matches!(token_kind, TokenKind::Integer | TokenKind::Float) {
                Err(ScalarFailure {
                    kind: ScalarFailureKind::NegativeWithoutNumber,
                    span: token_span,
                })
            } else {
                match expected {
                    BuiltinType::String if token_kind == TokenKind::String => match decoded_text {
                        None => Err(ScalarFailure {
                            kind: ScalarFailureKind::StringTokenPayloadMissing,
                            span: token_span,
                        }),
                        Some(decoded) if decoded.len() > context.limits.max_string_bytes() => {
                            Err(ScalarFailure {
                                kind: ScalarFailureKind::StringLimitExceeded,
                                span: token_span,
                            })
                        }
                        Some(decoded) => {
                            budget.construct_text(decoded.len(), text_limit, out.reborrow())?;
                            let source = context.value_source;
                            Self::build_value(&mut context.value_errors, token_span, |value_out| {
                                ValueFactory::string(decoded, source, Some(&budget), value_out)
                            })
                            .map(|value| (value, full_span))
                        }
                    },
                    BuiltinType::Bool => match token_kind {
                        TokenKind::Keyword(Keyword::True | Keyword::False) => {
                            let value = token_kind == TokenKind::Keyword(Keyword::True);
                            let source = context.value_source;
                            Self::build_value(&mut context.value_errors, token_span, |value_out| {
                                ValueFactory::boolean(value, source, Some(&budget), value_out)
                            })
                            .map(|value| (value, token_span))
                        }
                        _ => Err(ScalarFailure {
                            kind: mismatch_kind,
                            span: token_span,
                        }),
                    },
                    BuiltinType::I8
                    | BuiltinType::I16
                    | BuiltinType::I32
                    | BuiltinType::I64
                    | BuiltinType::I128 => {
                        if token_kind != TokenKind::Integer {
                            Err(ScalarFailure {
                                kind: mismatch_kind,
                                span: full_span,
                            })
                        } else {
                            match Scalar::signed_width(expected) {
                                Some(bits) => {
                                    match Scalar::parse_signed_integer(token_text, negative, bits) {
                                        Some(value) => {
                                            let source = context.value_source;
                                            Self::build_value(
                                                &mut context.value_errors,
                                                full_span,
                                                |value_out| {
                                                    ValueFactory::signed(
                                                        value,
                                                        source,
                                                        Some(&budget),
                                                        value_out,
                                                    )
                                                },
                                            )
                                            .map(|value| (value, full_span))
                                        }
                                        None => Err(ScalarFailure {
                                            kind: ScalarFailureKind::IntegerOutOfRange,
                                            span: full_span,
                                        }),
                                    }
                                }
                                None => Err(ScalarFailure {
                                    kind: ScalarFailureKind::IntegerTypeUnavailable,
                                    span: full_span,
                                }),
                            }
                        }
                    }
                    BuiltinType::U8
                    | BuiltinType::U16
                    | BuiltinType::U32
                    | BuiltinType::U64
                    | BuiltinType::U128 => {
                        if token_kind != TokenKind::Integer {
                            Err(ScalarFailure {
                                kind: mismatch_kind,
                                span: full_span,
                            })
                        } else if negative {
                            Err(ScalarFailure {
                                kind: ScalarFailureKind::IntegerOutOfRange,
                                span: full_span,
                            })
                        } else {
                            match Scalar::unsigned_width(expected) {
                                Some(bits) => {
                                    match Scalar::parse_unsigned_integer(token_text, bits) {
                                        Some(value) => {
                                            let source = context.value_source;
                                            Self::build_value(
                                                &mut context.value_errors,
                                                full_span,
                                                |value_out| {
                                                    ValueFactory::unsigned(
                                                        value,
                                                        source,
                                                        Some(&budget),
                                                        value_out,
                                                    )
                                                },
                                            )
                                            .map(|value| (value, full_span))
                                        }
                                        None => Err(ScalarFailure {
                                            kind: ScalarFailureKind::IntegerOutOfRange,
                                            span: full_span,
                                        }),
                                    }
                                }
                                None => Err(ScalarFailure {
                                    kind: ScalarFailureKind::IntegerTypeUnavailable,
                                    span: full_span,
                                }),
                            }
                        }
                    }
                    BuiltinType::F32 if token_kind == TokenKind::Float => {
                        match Scalar::parse_float32(token_text, negative) {
                            Some(value) => {
                                let source = context.value_source;
                                Self::build_value(
                                    &mut context.value_errors,
                                    full_span,
                                    |value_out| {
                                        ValueFactory::float32(
                                            value,
                                            source,
                                            Some(&budget),
                                            value_out,
                                        )
                                    },
                                )
                                .map(|value| (value, full_span))
                            }
                            None => Err(ScalarFailure {
                                kind: ScalarFailureKind::FloatOutOfRange,
                                span: full_span,
                            }),
                        }
                    }
                    BuiltinType::F64 if token_kind == TokenKind::Float => {
                        match Scalar::parse_float64(token_text, negative) {
                            Some(value) => {
                                let source = context.value_source;
                                Self::build_value(
                                    &mut context.value_errors,
                                    full_span,
                                    |value_out| {
                                        ValueFactory::float64(
                                            value,
                                            source,
                                            Some(&budget),
                                            value_out,
                                        )
                                    },
                                )
                                .map(|value| (value, full_span))
                            }
                            None => Err(ScalarFailure {
                                kind: ScalarFailureKind::FloatOutOfRange,
                                span: full_span,
                            }),
                        }
                    }
                    _ => Err(ScalarFailure {
                        kind: mismatch_kind,
                        span: full_span,
                    }),
                }
            }
        };

        match parsed {
            Ok((value, span)) => {
                context.cursor.bump(out.reborrow())?;
                Ok(ParsedValue {
                    value,
                    span,
                    completion: CompletionState::ChildrenComplete,
                })
            }
            Err(failure) if failure.kind.code() == DiagnosticCode::ALLOCATION_FAILED => {
                let span = failure.span;
                let mut diagnostic = DiagnosticFactory::allocation_diagnostic(
                    Some(span),
                    None,
                    Some(context.value_source),
                    context.session.translator(),
                );
                diagnostic.set_context(failure.kind.context());
                Err(out.fail(diagnostic.into()))
            }
            Err(failure) => {
                let translator = context.session.translator();
                let code = failure.kind.code();
                let message = match failure.kind {
                    ScalarFailureKind::ExpectedValue => t! {
                        name: "diagnostic.config.expected_value",
                        locale: translator,
                    }
                    .into(),
                    ScalarFailureKind::TypeMismatch => t! {
                        name: "diagnostic.config.scalar_type_mismatch",
                        locale: translator,
                    }
                    .into(),
                    ScalarFailureKind::NegativeWithoutNumber => t! {
                        name: "diagnostic.syntax.expected_decimal_after_minus",
                        locale: translator,
                    }
                    .into(),
                    ScalarFailureKind::StringTokenPayloadMissing => t! {
                        name: "diagnostic.config.string_token_payload_missing",
                        locale: translator,
                    }
                    .into(),
                    ScalarFailureKind::StringLimitExceeded => t! {
                        name: "diagnostic.config.decoded_string_limit_exceeded",
                        locale: translator,
                    }
                    .into(),
                    ScalarFailureKind::IntegerOutOfRange | ScalarFailureKind::FloatOutOfRange => {
                        t! {
                            name: "diagnostic.config.scalar_out_of_range",
                            locale: translator,
                        }
                        .into()
                    }
                    ScalarFailureKind::IntegerTypeUnavailable => t! {
                        name: "diagnostic.config.integer_type_unavailable",
                        locale: translator,
                    }
                    .into(),
                    ScalarFailureKind::ValueConstruction { kind, .. } => match kind {
                        ValueErrorKind::ResourceLimitExceeded => t! {
                            name: "diagnostic.config.value_construction_resource_limit",
                            locale: translator,
                        }
                        .into(),
                        ValueErrorKind::AllocationFailed => t! {
                            name: "diagnostic.config.value_construction_allocation_failed",
                            locale: translator,
                        }
                        .into(),
                        ValueErrorKind::CapacityExceeded => t! {
                            name: "diagnostic.config.value_construction_capacity_exceeded",
                            locale: translator,
                        }
                        .into(),
                        ValueErrorKind::DuplicateMapKey
                        | ValueErrorKind::DuplicateFieldName
                        | ValueErrorKind::EnvironmentReferenceRequired
                        | ValueErrorKind::InternalInvariantFailure => t! {
                            name: "diagnostic.config.value_construction_internal_failure",
                            locale: translator,
                        }
                        .into(),
                    },
                };
                let value_source = if code == DiagnosticCode::CONFIG_SYNTAX_ERROR {
                    None
                } else {
                    Some(context.value_source)
                };
                let diagnostic = ParserControl::diagnostic_call(
                    &context.diagnostic_errors,
                    |diagnostic_out| {
                        LiteralParser::context_diagnostic(
                            ContextDiagnosticRequest {
                                context,
                                code,
                                message,
                                span: failure.span,
                                value_source,
                            },
                            diagnostic_out,
                        )
                    },
                    out.reborrow(),
                )?;
                let mut diagnostic = diagnostic;
                diagnostic.set_context(failure.kind.context());
                Err(out.fail(diagnostic.into()))
            }
        }
    }

    pub(super) fn take_none<'slot>(
        context: &mut ParserContext<'_, '_, '_>,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, Option<ParsedValue>> {
        let span = {
            let token = context.cursor.peek(out.reborrow())?;
            if token.kind() == TokenKind::Keyword(Keyword::None) {
                Some(token.span())
            } else {
                None
            }
        };
        let Some(span) = span else {
            return Ok(None);
        };

        context.cursor.bump(out.reborrow())?;
        let value = ContainerSupport::value_call(
            &mut context.value_errors,
            |value_out| ValueFactory::none(context.value_source, Some(&context.budget), value_out),
            ValueCallContext {
                span,
                source: context.value_source,
                translator: context.session.translator(),
            },
            out.reborrow(),
        )?;
        Ok(Some(ParsedValue {
            completion: CompletionState::ChildrenComplete,
            value,
            span,
        }))
    }

    pub(super) fn context_diagnostic<'slot>(
        request: ContextDiagnosticRequest<'_, '_, '_, '_>,
        out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Diagnostic> {
        LiteralDiagnostic::context_diagnostic(request, out)
    }

    pub(super) fn copy_path_with_source<'slot, 'borrow>(
        context: &ParserContext<'_, '_, '_>,
        primary: Option<LocatedSpan>,
        value_source: Option<ValueSource>,
        out: Out<'slot, 'borrow, Diagnostic>,
    ) -> OResult<'slot, InstancePath> {
        LiteralDiagnostic::copy_path_with_source(context, primary, value_source, out)
    }
}

#[cfg(test)]
#[test]
fn internal_scalar_contexts_are_selected_by_failure_kind() -> TestResult<()> {
    test_eq!(
        ScalarFailureKind::StringTokenPayloadMissing.code(),
        DiagnosticCode::INTERNAL_ERROR
    )?;
    test_eq!(
        ScalarFailureKind::StringTokenPayloadMissing.context(),
        Some(PARSER_SCALAR_TOKEN_PAYLOAD)
    )?;
    test_eq!(
        ScalarFailureKind::IntegerTypeUnavailable.code(),
        DiagnosticCode::INTERNAL_ERROR
    )?;
    test_eq!(
        ScalarFailureKind::IntegerTypeUnavailable.context(),
        Some(PARSER_SCALAR_INTEGER_TYPE)
    )?;
    Ok(())
}

#[cfg(test)]
#[test]
fn scalar_construction_failure_retains_the_original_value_error() -> TestResult<()> {
    static TEST_SCALAR_INVARIANT: DiagnosticContext = diagnostic_context!(
        "typol-config-tests",
        "parser-scalar",
        "injected-value-invariant",
    );

    let span: LocatedSpan = SourceSpan::new(SourceId::new(4), Span::from_zero_to(2)).into();
    let mut errors = ErrSlot::new();
    let failure = LiteralParser::build_value(&mut errors, span, |out| {
        Err(out.fail(ValueError::from(BudgetError::invariant(
            &TEST_SCALAR_INVARIANT,
        ))))
    })
    .err()
    .ok_or_else(|| TestFailure::new("test value construction should fail"))?;

    let ScalarFailure {
        kind: ScalarFailureKind::ValueConstruction { kind, context },
        span: failure_span,
    } = failure
    else {
        return Err(TestFailure::new(
            "scalar failure should preserve the ValueError",
        ));
    };
    test_eq!(kind, ValueErrorKind::InternalInvariantFailure)?;
    test_eq!(context, Some(TEST_SCALAR_INVARIANT))?;
    test_eq!(failure_span, span)?;
    test_eq!(
        ScalarFailureKind::ValueConstruction { kind, context }.code(),
        DiagnosticCode::INTERNAL_ERROR
    )?;
    test_eq!(
        ScalarFailureKind::ValueConstruction { kind, context }.context(),
        Some(TEST_SCALAR_INVARIANT)
    )?;

    Ok(())
}
