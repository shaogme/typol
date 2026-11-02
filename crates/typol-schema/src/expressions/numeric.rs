use opeo::{OResult, Out, ResultOutExt};
use typol_core::{
    DiagnosticMessage, FloatConversionError, NumericLiteral, NumericTokenKind, SourceSpan,
};
use typol_i18n::diagnostic_message;
use typol_syntax::{BuiltinType, NumberKind, NumberLiteral};

use crate::{
    context::CompileContext,
    diagnostics::{BuildFailure, DiagnosticCollector},
    expressions::TypedLiteral,
};

fn is_numeric(ty: BuiltinType) -> bool {
    matches!(
        ty,
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

fn is_float(ty: BuiltinType) -> bool {
    matches!(ty, BuiltinType::F32 | BuiltinType::F64)
}

fn is_signed_numeric(ty: BuiltinType) -> bool {
    matches!(
        ty,
        BuiltinType::I8
            | BuiltinType::I16
            | BuiltinType::I32
            | BuiltinType::I64
            | BuiltinType::I128
            | BuiltinType::F32
            | BuiltinType::F64
    )
}

pub(super) struct ExpressionNumbers;

pub(super) struct CompileLiteralInput<'number, 'context, 'diagnostics> {
    pub(super) number: NumberLiteral<'number>,
    pub(super) target: BuiltinType,
    pub(super) allow_negative_minimum: bool,
    pub(super) span: SourceSpan,
    pub(super) context: &'context CompileContext,
    pub(super) diagnostics: &'diagnostics mut DiagnosticCollector,
}

impl ExpressionNumbers {
    pub(super) fn is_signed(builtin: BuiltinType) -> bool {
        is_signed_numeric(builtin)
    }

    pub(super) fn is_numeric(builtin: BuiltinType) -> bool {
        is_numeric(builtin)
    }

    pub(super) fn is_float_type(builtin: BuiltinType) -> bool {
        is_float(builtin)
    }

    pub(super) fn compile_literal<'slot>(
        input: CompileLiteralInput<'_, '_, '_>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, TypedLiteral> {
        let CompileLiteralInput {
            number,
            target,
            allow_negative_minimum,
            span,
            context,
            diagnostics,
        } = input;
        let raw = context.ast_access(
            out.reborrow(),
            |ast_out| number.raw(ast_out),
            BuildFailure::from,
        )?;
        if is_float(target) {
            let kind = match number.kind() {
                NumberKind::Integer => NumericTokenKind::Integer,
                NumberKind::Float => NumericTokenKind::Float,
            };
            let converted = if target == BuiltinType::F32 {
                NumericLiteral::contextual_f32(raw, kind, number.is_negative()).map(f64::from)
            } else {
                NumericLiteral::contextual_f64(raw, kind, number.is_negative())
            };
            match converted {
                Ok(value) => Ok(TypedLiteral::Float(value)),
                Err(FloatConversionError::Invalid) if kind == NumericTokenKind::Integer => {
                    fail_number(
                        diagnostics,
                        diagnostic_message!(
                            "diagnostic.schema.numeric_integer_unrepresentable",
                            "integer literal is out of the representable range"
                        ),
                        span,
                        out.reborrow(),
                    )
                }
                Err(FloatConversionError::Invalid) => fail_number(
                    diagnostics,
                    diagnostic_message!(
                        "diagnostic.schema.numeric_float_unrepresentable",
                        "floating-point literal cannot be represented"
                    ),
                    span,
                    out.reborrow(),
                ),
                Err(FloatConversionError::NonFinite) if target == BuiltinType::F32 => fail_number(
                    diagnostics,
                    diagnostic_message!(
                        "diagnostic.schema.numeric_float_f32_out_of_range",
                        "floating-point literal is out of f32 range"
                    ),
                    span,
                    out.reborrow(),
                ),
                Err(FloatConversionError::NonFinite) => fail_number(
                    diagnostics,
                    diagnostic_message!(
                        "diagnostic.schema.numeric_float_not_finite",
                        "floating-point literal must be finite"
                    ),
                    span,
                    out.reborrow(),
                ),
            }
        } else {
            if number.kind() != NumberKind::Integer {
                return fail_number(
                    diagnostics,
                    diagnostic_message!(
                        "diagnostic.schema.numeric_float_for_integer",
                        "floating-point literal cannot be used with an integer type"
                    ),
                    span,
                    out.reborrow(),
                );
            }
            let magnitude = parse_integer(raw)
                .ok_or_else(|| {
                    diagnostics.push(diagnostics.make_semantic(
                        diagnostic_message!(
                            "diagnostic.schema.numeric_integer_unrepresentable",
                            "integer literal is out of the representable range"
                        ),
                        span,
                    ));
                    BuildFailure::empty()
                })
                .store_err_into(out.reborrow())?;
            if magnitude > integer_magnitude_limit(target, allow_negative_minimum) {
                return fail_number(
                    diagnostics,
                    diagnostic_message!(
                        "diagnostic.schema.numeric_integer_out_of_target_range",
                        "integer literal is out of the target type's range"
                    ),
                    span,
                    out.reborrow(),
                );
            }
            Ok(TypedLiteral::Integer(magnitude))
        }
    }
}

fn fail_number<'slot, T>(
    diagnostics: &mut DiagnosticCollector,
    message: DiagnosticMessage<'static>,
    span: SourceSpan,
    out: Out<'slot, '_, BuildFailure>,
) -> OResult<'slot, T> {
    diagnostics.push(diagnostics.make_semantic(message, span));
    Err(out.fail(BuildFailure::empty()))
}

fn integer_magnitude_limit(ty: BuiltinType, allow_negative_minimum: bool) -> u128 {
    let bits = match ty {
        BuiltinType::I8 | BuiltinType::U8 => 8,
        BuiltinType::I16 | BuiltinType::U16 => 16,
        BuiltinType::I32 | BuiltinType::U32 => 32,
        BuiltinType::I64 | BuiltinType::U64 => 64,
        BuiltinType::I128 | BuiltinType::U128 => 128,
        _ => return 0,
    };
    if is_signed_numeric(ty) {
        if bits == 128 {
            (1_u128 << 127) - 1 + u128::from(allow_negative_minimum)
        } else {
            (1_u128 << (bits - 1)) - 1 + u128::from(allow_negative_minimum)
        }
    } else if bits == 128 {
        u128::MAX
    } else {
        (1_u128 << bits) - 1
    }
}

fn parse_integer(raw: &str) -> Option<u128> {
    if let Some(hex) = raw.strip_prefix("0x") {
        u128::from_str_radix(hex, 16).ok()
    } else {
        raw.parse().ok()
    }
}
