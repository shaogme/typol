/// Categories of floating-point conversion failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FloatConversionError {
    /// The token text could not be parsed.
    Invalid,
    /// The converted value is not finite.
    NonFinite,
}

/// The lexical kind of a contextual numeric token.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NumericTokenKind {
    /// An integer token.
    Integer,
    /// A floating-point token.
    Float,
}

/// Parses numbers using strict value rules or contextual target precision.
pub struct NumericLiteral;

impl NumericLiteral {
    /// Parses an unsigned floating-point token directly as `f32`.
    pub fn strict_f32(raw: &str, negative: bool) -> Result<f32, FloatConversionError> {
        let value = raw
            .parse::<f32>()
            .map_err(|_| FloatConversionError::Invalid)?;
        Self::finite_f32(value, negative)
    }

    /// Parses an unsigned floating-point token directly as `f64`.
    pub fn strict_f64(raw: &str, negative: bool) -> Result<f64, FloatConversionError> {
        let value = raw
            .parse::<f64>()
            .map_err(|_| FloatConversionError::Invalid)?;
        Self::finite_f64(value, negative)
    }

    /// Rounds directly to contextual `f32` without an intermediate `f64` conversion.
    pub fn contextual_f32(
        raw: &str,
        kind: NumericTokenKind,
        negative: bool,
    ) -> Result<f32, FloatConversionError> {
        let value = match kind {
            NumericTokenKind::Integer => Self::integer_magnitude(raw)? as f32,
            NumericTokenKind::Float => raw
                .parse::<f32>()
                .map_err(|_| FloatConversionError::Invalid)?,
        };
        Self::finite_f32(value, negative)
    }

    /// Rounds directly to contextual `f64`.
    pub fn contextual_f64(
        raw: &str,
        kind: NumericTokenKind,
        negative: bool,
    ) -> Result<f64, FloatConversionError> {
        let value = match kind {
            NumericTokenKind::Integer => Self::integer_magnitude(raw)? as f64,
            NumericTokenKind::Float => raw
                .parse::<f64>()
                .map_err(|_| FloatConversionError::Invalid)?,
        };
        Self::finite_f64(value, negative)
    }

    fn integer_magnitude(raw: &str) -> Result<u128, FloatConversionError> {
        if let Some(hex) = raw.strip_prefix("0x") {
            u128::from_str_radix(hex, 16).map_err(|_| FloatConversionError::Invalid)
        } else {
            raw.parse::<u128>()
                .map_err(|_| FloatConversionError::Invalid)
        }
    }

    fn finite_f32(value: f32, negative: bool) -> Result<f32, FloatConversionError> {
        let value = if negative { -value } else { value };
        value
            .is_finite()
            .then_some(value)
            .ok_or(FloatConversionError::NonFinite)
    }

    fn finite_f64(value: f64, negative: bool) -> Result<f64, FloatConversionError> {
        let value = if negative { -value } else { value };
        value
            .is_finite()
            .then_some(value)
            .ok_or(FloatConversionError::NonFinite)
    }
}
