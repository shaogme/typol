use crate::{
    diagnostics::{BudgetDiagnostic, DiagnosticFactory},
    value::ValueFactory,
};
use opeo::{ErrSlot, OResult, Out};
use typol_core::{
    BudgetError, ConfigValue, Diagnostic, DiagnosticCode, DiagnosticMessage, ExecutionBudget,
    LocatedSpan, NumericLiteral, ValueSource,
};
use typol_i18n::diagnostic_key;
use typol_syntax::BuiltinType;

pub(crate) struct Scalar;

pub(crate) struct ScalarRequest<'a> {
    pub(crate) text: &'a str,
    pub(crate) builtin: BuiltinType,
    pub(crate) span: LocatedSpan,
    pub(crate) budget: Option<&'a ExecutionBudget>,
}

enum ScalarValue {
    Signed(i128),
    Unsigned(u128),
    Float32(f32),
    Float64(f64),
}

impl Scalar {
    pub(crate) fn signed_width(expected: BuiltinType) -> Option<u32> {
        match expected {
            BuiltinType::I8 => Some(8),
            BuiltinType::I16 => Some(16),
            BuiltinType::I32 => Some(32),
            BuiltinType::I64 => Some(64),
            BuiltinType::I128 => Some(128),
            _ => None,
        }
    }

    pub(crate) fn unsigned_width(expected: BuiltinType) -> Option<u32> {
        match expected {
            BuiltinType::U8 => Some(8),
            BuiltinType::U16 => Some(16),
            BuiltinType::U32 => Some(32),
            BuiltinType::U64 => Some(64),
            BuiltinType::U128 => Some(128),
            _ => None,
        }
    }

    fn parse_unsigned_magnitude(literal: &str) -> Option<u128> {
        if let Some(hex) = literal.strip_prefix("0x") {
            u128::from_str_radix(hex, 16).ok()
        } else {
            literal.parse::<u128>().ok()
        }
    }

    pub(crate) fn parse_signed_integer(literal: &str, negative: bool, bits: u32) -> Option<i128> {
        let magnitude = Self::parse_unsigned_magnitude(literal)?;
        let sign_limit = 1_u128.checked_shl(bits.checked_sub(1)?)?;
        if negative {
            if magnitude > sign_limit {
                return None;
            }
            if bits == 128 && magnitude == sign_limit {
                return Some(i128::MIN);
            }
            i128::try_from(magnitude).ok()?.checked_neg()
        } else {
            if magnitude >= sign_limit {
                return None;
            }
            i128::try_from(magnitude).ok()
        }
    }

    pub(crate) fn parse_unsigned_integer(literal: &str, bits: u32) -> Option<u128> {
        let value = Self::parse_unsigned_magnitude(literal)?;
        let max = if bits == 128 {
            u128::MAX
        } else {
            1_u128.checked_shl(bits)?.checked_sub(1)?
        };
        (value <= max).then_some(value)
    }

    pub(crate) fn parse_float32(literal: &str, negative: bool) -> Option<f32> {
        NumericLiteral::strict_f32(literal, negative).ok()
    }

    pub(crate) fn parse_float64(literal: &str, negative: bool) -> Option<f64> {
        NumericLiteral::strict_f64(literal, negative).ok()
    }

    /// Validate the complete text against the numeric grammar first, then reuse the target-width and rounding logic for configuration literals.
    #[cfg(test)]
    pub(crate) fn convert<'slot>(
        text: &str,
        builtin: BuiltinType,
        span: LocatedSpan,
        budget_errors: &mut ErrSlot<BudgetError>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ConfigValue> {
        Self::convert_budgeted(
            ScalarRequest {
                text,
                builtin,
                span,
                budget: None,
            },
            budget_errors,
            out.reborrow(),
        )
    }

    pub(crate) fn convert_budgeted<'slot>(
        request: ScalarRequest<'_>,
        budget_errors: &mut ErrSlot<BudgetError>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ConfigValue> {
        let ScalarRequest {
            text,
            builtin,
            span,
            budget,
        } = request;
        if builtin == BuiltinType::String {
            let mut value_errors = ErrSlot::new();
            let value = value_errors.try_call(|value_out| {
                ValueFactory::string(text, ValueSource::RustValue, budget, value_out)
            });
            return match value {
                Ok(value) => Ok(value),
                Err(caught) => Err(out.fail(ValueFactory::failure(
                    caught.take(),
                    Some(span),
                    Some(ValueSource::EnvironmentVariable),
                ))),
            };
        }
        if builtin == BuiltinType::Bool {
            for (expected, value) in [("true", true), ("false", false)] {
                let equal = if let Some(budget) = budget {
                    budget.compare_text(text, expected, out.reborrow())?.is_eq()
                } else {
                    text == expected
                };
                if equal {
                    let mut value_errors = ErrSlot::new();
                    return match value_errors.try_call(|value_out| {
                        ValueFactory::boolean(value, ValueSource::RustValue, budget, value_out)
                    }) {
                        Ok(value) => Ok(value),
                        Err(caught) => Err(out.fail(ValueFactory::failure(
                            caught.take(),
                            Some(span),
                            Some(ValueSource::EnvironmentVariable),
                        ))),
                    };
                }
            }
            let diagnostic =
                Self::invalid_scalar_budgeted(span, budget, budget_errors, out.reborrow())?;
            return Err(out.fail(diagnostic));
        }
        if !text.is_empty() {
            Self::charge(budget, 1, out.reborrow())?;
        }
        let (literal, negative) = text
            .strip_prefix('-')
            .map_or((text, false), |literal| (literal, true));
        let (integer, float) = if matches!(builtin, BuiltinType::F32 | BuiltinType::F64) {
            let mut dot = None;
            for (index, byte) in literal.bytes().enumerate() {
                Self::charge(budget, 1, out.reborrow())?;
                if byte == b'.' {
                    dot = Some(index);
                    break;
                }
            }
            let valid = if let Some(dot) = dot {
                Self::digits(&literal[..dot], false, budget, out.reborrow())?
                    && Self::digits(&literal[dot + 1..], false, budget, out.reborrow())?
            } else {
                false
            };
            (false, valid)
        } else {
            Self::charge(budget, literal.len().min(2), out.reborrow())?;
            let valid = if let Some(hex) = literal.strip_prefix("0x") {
                Self::digits(hex, true, budget, out.reborrow())?
            } else {
                Self::digits(literal, false, budget, out.reborrow())?
            };
            (valid, false)
        };
        if integer || float {
            Self::charge(budget, literal.len(), out.reborrow())?;
        }
        let value = if let Some(bits) = Self::signed_width(builtin) {
            integer
                .then(|| Self::parse_signed_integer(literal, negative, bits))
                .flatten()
                .map(ScalarValue::Signed)
        } else if let Some(bits) = Self::unsigned_width(builtin) {
            (integer && !negative)
                .then(|| Self::parse_unsigned_integer(literal, bits))
                .flatten()
                .map(ScalarValue::Unsigned)
        } else {
            match builtin {
                BuiltinType::F32 if float => {
                    Self::parse_float32(literal, negative).map(ScalarValue::Float32)
                }
                BuiltinType::F64 if float => {
                    Self::parse_float64(literal, negative).map(ScalarValue::Float64)
                }
                _ => None,
            }
        };
        match value {
            Some(value) => {
                let mut value_errors = ErrSlot::new();
                match value_errors.try_call(|value_out| match value {
                    ScalarValue::Signed(value) => {
                        ValueFactory::signed(value, ValueSource::RustValue, budget, value_out)
                    }
                    ScalarValue::Unsigned(value) => {
                        ValueFactory::unsigned(value, ValueSource::RustValue, budget, value_out)
                    }
                    ScalarValue::Float32(value) => {
                        ValueFactory::float32(value, ValueSource::RustValue, budget, value_out)
                    }
                    ScalarValue::Float64(value) => {
                        ValueFactory::float64(value, ValueSource::RustValue, budget, value_out)
                    }
                }) {
                    Ok(value) => Ok(value),
                    Err(caught) => Err(out.fail(ValueFactory::failure(
                        caught.take(),
                        Some(span),
                        Some(ValueSource::EnvironmentVariable),
                    ))),
                }
            }
            None => {
                let diagnostic =
                    Self::invalid_scalar_budgeted(span, budget, budget_errors, out.reborrow())?;
                Err(out.fail(diagnostic))
            }
        }
    }

    fn charge<'slot>(
        budget: Option<&ExecutionBudget>,
        bytes: usize,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ()> {
        if let Some(budget) = budget {
            budget.consume(bytes, out.reborrow())?;
        }
        Ok(())
    }

    fn digits<'slot>(
        text: &str,
        hex: bool,
        budget: Option<&ExecutionBudget>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, bool> {
        if text.is_empty() {
            return Ok(false);
        }
        for byte in text.bytes() {
            Self::charge(budget, 1, out.reborrow())?;
            if !(if hex {
                byte.is_ascii_hexdigit()
            } else {
                byte.is_ascii_digit()
            }) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn invalid_scalar_budgeted<'slot, 'borrow>(
        span: LocatedSpan,
        budget: Option<&ExecutionBudget>,
        budget_errors: &mut ErrSlot<BudgetError>,
        out: Out<'slot, 'borrow, Diagnostic>,
    ) -> OResult<'slot, Diagnostic> {
        let Some(budget) = budget else {
            let mut diagnostic = Self::environment_error(
                DiagnosticCode::CONFIG_VALUE_ERROR,
                "environment text is incompatible with the target scalar type or range",
                span,
            );
            diagnostic.set_message_key(Some(diagnostic_key!(
                "diagnostic.environment.value_invalid"
            )));
            return Ok(diagnostic);
        };
        let mut diagnostic = match budget_errors.try_call(|budget_out| {
            Diagnostic::try_new(
                DiagnosticCode::CONFIG_VALUE_ERROR,
                DiagnosticMessage::localized(
                    "environment text is incompatible with the target scalar type or range",
                    diagnostic_key!("diagnostic.environment.value_invalid"),
                ),
                Some(budget),
                budget_out,
            )
        }) {
            Ok(diagnostic) => diagnostic,
            Err(caught) => {
                return Err(out.fail(BudgetDiagnostic::from_error(caught.take())));
            }
        };
        diagnostic.set_message_key(Some(diagnostic_key!(
            "diagnostic.environment.value_invalid"
        )));
        diagnostic.set_located_primary(Some(span));
        diagnostic.set_value_source(Some(ValueSource::EnvironmentVariable));
        Ok(diagnostic)
    }

    fn environment_error(code: DiagnosticCode, message: &str, span: LocatedSpan) -> Diagnostic {
        DiagnosticFactory::make_diagnostic(
            code,
            message,
            Some(span),
            None,
            Some(ValueSource::EnvironmentVariable),
        )
    }
}
#[cfg(test)]
mod tests {
    use crate::{diagnostics::DiagnosticFactory, scalar::Scalar};
    use typol_test_support::*;

    use alloc::{format, string::String};
    use typol_core::{
        BudgetError, ConfigValue, Diagnostic, DiagnosticCode, LocatedSpan, SourceId, Span,
        ValueKind,
    };
    use typol_syntax::BuiltinType;

    struct ScalarTestSlots {
        errors: TestSlot<Diagnostic>,
        budget_errors: TestSlot<BudgetError>,
    }

    impl ScalarTestSlots {
        fn new() -> Self {
            Self {
                errors: TestSlot::default(),
                budget_errors: TestSlot::default(),
            }
        }

        fn convert(
            &mut self,
            text: &str,
            builtin: BuiltinType,
        ) -> TestOpeoOutcome<ConfigValue, Diagnostic> {
            let Self {
                errors,
                budget_errors,
            } = self;
            errors.run(|out| Scalar::convert(text, builtin, span(), budget_errors.slot(), out))
        }
    }

    fn span() -> LocatedSpan {
        DiagnosticFactory::source_span(SourceId::new(0), Span::from_zero_to(1))
    }

    #[test]
    fn scalar_conversion_consumes_the_entire_text() -> TestResult<()> {
        let mut calls = ScalarTestSlots::new();
        for builtin in [
            BuiltinType::U8,
            BuiltinType::I8,
            BuiltinType::F32,
            BuiltinType::F64,
        ] {
            for text in [
                "", " 1", "1 ", "+1", "1e2", "1_0", "1//x", "1/*x*/", "--1", "0X12", "NaN", "inf",
                "１",
            ] {
                test_eq!(
                    calls.convert(text, builtin).test_error()?.code(),
                    DiagnosticCode::CONFIG_VALUE_ERROR
                )?;
            }
        }
        for text in ["1", ".1", "1.", "1.2.3", "0x1.2"] {
            test_assert!(calls.convert(text, BuiltinType::F32).is_err())?;
        }
        for text in ["true ", "TRUE", "1", "false\n"] {
            test_assert!(calls.convert(text, BuiltinType::Bool).is_err())?;
        }
        test_assert!(matches!(
            calls
                .convert("false", BuiltinType::Bool)
                .test_value()?
                .as_ref(),
            ValueKind::Bool(false)
        ))?;
        let text = " \"unchanged\"\n//content";
        test_assert!(
            matches!(calls.convert(text, BuiltinType::String).test_value()?.as_ref(), ValueKind::String(value) if value == text)
        )?;

        Ok(())
    }

    #[test]
    fn all_integer_widths_keep_exact_boundaries() -> TestResult<()> {
        let mut calls = ScalarTestSlots::new();
        for builtin in [
            BuiltinType::I8,
            BuiltinType::I16,
            BuiltinType::I32,
            BuiltinType::I64,
            BuiltinType::I128,
        ] {
            let bits = Scalar::signed_width(builtin).test_value()?;
            let magnitude = 1_u128 << (bits - 1);
            let min = format!("-{magnitude}");
            let max = format!("{}", magnitude - 1);
            test_assert!(calls.convert(&min, builtin).is_ok())?;
            test_assert!(calls.convert(&max, builtin).is_ok())?;
            test_assert!(calls.convert(&format!("{magnitude}"), builtin).is_err())?;
            test_assert!(
                calls
                    .convert(&format!("-{}", magnitude + 1), builtin)
                    .is_err()
            )?;
        }
        for builtin in [
            BuiltinType::U8,
            BuiltinType::U16,
            BuiltinType::U32,
            BuiltinType::U64,
            BuiltinType::U128,
        ] {
            let bits = Scalar::unsigned_width(builtin).test_value()?;
            let max = u128::MAX >> (128 - bits);
            test_assert!(calls.convert(&format!("{max}"), builtin).is_ok())?;
            test_assert!(calls.convert(&format!("0x{max:x}"), builtin).is_ok())?;
            test_assert!(calls.convert("-0", builtin).is_err())?;
            let overflow = max.checked_add(1).map_or_else(
                || String::from("340282366920938463463374607431768211456"),
                |value| format!("{value}"),
            );
            test_assert!(calls.convert(&overflow, builtin).is_err())?;
        }
        test_assert!(matches!(
            calls
                .convert("00012", BuiltinType::U8)
                .test_value()?
                .as_ref(),
            ValueKind::Unsigned(12)
        ))?;
        test_assert!(matches!(
            calls
                .convert("-0x80", BuiltinType::I8)
                .test_value()?
                .as_ref(),
            ValueKind::Signed(-128)
        ))?;

        Ok(())
    }

    #[test]
    fn floats_round_to_target_width_and_reject_nonfinite_results() -> TestResult<()> {
        let mut calls = ScalarTestSlots::new();
        test_assert!(
            matches!(calls.convert("16777217.0", BuiltinType::F32).test_value()?.as_ref(), ValueKind::Float32(value) if value == 16777216.0)
        )?;
        test_assert!(
            matches!(calls.convert("16777217.0", BuiltinType::F64).test_value()?.as_ref(), ValueKind::Float64(value) if value == 16777217.0)
        )?;
        test_assert!(
            calls
                .convert(
                    "340282400000000000000000000000000000000.0",
                    BuiltinType::F32,
                )
                .is_err()
        )?;
        test_assert!(
            calls
                .convert(&format!("{}.0", "9".repeat(310)), BuiltinType::F64,)
                .is_err()
        )?;
        test_assert!(
            matches!(calls.convert("-0.0", BuiltinType::F32).test_value()?.as_ref(), ValueKind::Float32(value) if value.is_sign_negative())
        )?;

        Ok(())
    }
}
