mod build;
mod compiler;
mod frames;
mod rules;
mod value_transfer;

use crate::{
    annotations::AnnotationCompilation,
    context::CompileContext,
    defaults::{build::DefaultBuilder, compiler::DefaultCompiler},
    diagnostics::{BuildFailure, DiagnosticCollector, EmergencyDiagnostics},
    expressions::ExpressionCompilation,
    symbols::{SymbolState, SymbolTable},
    types::ResolvedTypeGraph,
};
use typol_i18n::diagnostic_message;
use typol_semantic::{EvalError, EvalFailure, Numeric, SchemaDefaultValidation};
use typol_syntax::{NumberKind, NumberLiteral};

use opeo::{OResult, Out};
use typol_core::{
    ChargedVec, ConfigValue, Diagnostic, DiagnosticCode, DiagnosticMessage, FloatConversionError,
    NumericLiteral, SourceSpan, ValueKind,
};
use typol_model::BuiltinType;

/// Temporary output from the default-value pass, consumed only by the complete Schema aggregation phase.
pub(crate) struct DefaultCompilation {
    fields: ChargedVec<Option<ConfigValue>>,
    validation: Option<SchemaDefaultValidation>,
}

impl DefaultCompilation {
    pub(crate) fn new(
        fields: ChargedVec<Option<ConfigValue>>,
        validation: Option<SchemaDefaultValidation>,
    ) -> Self {
        Self { fields, validation }
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        ChargedVec<Option<ConfigValue>>,
        Option<SchemaDefaultValidation>,
    ) {
        (self.fields, self.validation)
    }
}

pub(crate) struct DefaultInputs<'a> {
    pub(crate) symbols: &'a SymbolTable<'a>,
    pub(crate) types: &'a ResolvedTypeGraph,
    pub(crate) annotations: &'a AnnotationCompilation,
    pub(crate) expressions: &'a ExpressionCompilation,
    pub(crate) context: &'a CompileContext,
    pub(crate) fallback: SourceSpan,
}

pub(crate) struct DefaultPass;

impl DefaultPass {
    pub(crate) fn compile<'slot>(
        inputs: DefaultInputs<'_>,
        diagnostics: &mut DiagnosticCollector,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, DefaultCompilation> {
        let symbols = inputs.symbols;
        let context = inputs.context;
        let mut compiler = DefaultCompiler::new(inputs, diagnostics, out.reborrow())?;
        for field in symbols.fields() {
            symbols.context().consume(
                1,
                field.declaration().span(),
                compiler.diagnostics,
                out.reborrow(),
            )?;
            if compiler.diagnostics.is_stopped() {
                return Err(out.fail(BuildFailure::empty()));
            }
            let default = context.ast_access(
                out.reborrow(),
                |ast_out| field.declaration().default(ast_out),
                BuildFailure::from,
            )?;
            if field.state() == SymbolState::Valid && default.is_some() {
                let _ = DefaultBuilder::new(&mut compiler)
                    .ensure_field_default(field.id(), out.reborrow())?;
            }
        }
        compiler.finish(out.reborrow())
    }
}

#[derive(Clone, Copy)]
pub(crate) enum DefaultNumber {
    Signed(i128),
    Unsigned(u128),
    Float32(f32),
    Float64(f64),
}

pub(crate) struct DefaultLiterals;

impl DefaultLiterals {
    pub(crate) fn build_number<'slot>(
        number: NumberLiteral<'_>,
        builtin: BuiltinType,
        context: &CompileContext,
        diagnostics: &mut DiagnosticCollector,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, Option<DefaultNumber>> {
        let raw = context.ast_access(
            out.reborrow(),
            |ast_out| number.raw(ast_out),
            BuildFailure::from,
        )?;
        context.consume(raw.len(), number.span(), diagnostics, out.reborrow())?;
        if DefaultLiterals::is_float(builtin) {
            if number.kind() != NumberKind::Float {
                Self::invalid(
                    diagnostic_message!(
                        "diagnostic.schema.default_numeric_type_mismatch",
                        "numeric default does not match the target type"
                    ),
                    number.span(),
                    diagnostics,
                    out.reborrow(),
                )?;
                return Ok(None);
            }
            if builtin == BuiltinType::F32 {
                match NumericLiteral::strict_f32(raw, number.is_negative()) {
                    Ok(value) => return Ok(Some(DefaultNumber::Float32(value))),
                    Err(error) => {
                        let message = match error {
                            FloatConversionError::Invalid => diagnostic_message!(
                                "diagnostic.schema.default_float_unrepresentable",
                                "floating-point default cannot be represented"
                            ),
                            FloatConversionError::NonFinite => diagnostic_message!(
                                "diagnostic.schema.default_f32_out_of_range",
                                "floating-point default is out of f32 range"
                            ),
                        };
                        Self::invalid(message, number.span(), diagnostics, out.reborrow())?;
                        return Ok(None);
                    }
                }
            }
            match NumericLiteral::strict_f64(raw, number.is_negative()) {
                Ok(value) => return Ok(Some(DefaultNumber::Float64(value))),
                Err(error) => {
                    let message = match error {
                        FloatConversionError::Invalid => diagnostic_message!(
                            "diagnostic.schema.default_float_unrepresentable",
                            "floating-point default cannot be represented"
                        ),
                        FloatConversionError::NonFinite => diagnostic_message!(
                            "diagnostic.schema.default_float_must_be_finite",
                            "floating-point default must be finite"
                        ),
                    };
                    Self::invalid(message, number.span(), diagnostics, out.reborrow())?;
                    return Ok(None);
                }
            }
        }
        if number.kind() != NumberKind::Integer {
            Self::invalid(
                diagnostic_message!(
                    "diagnostic.schema.default_float_for_integer",
                    "floating-point literal cannot be used with an integer type"
                ),
                number.span(),
                diagnostics,
                out.reborrow(),
            )?;
            return Ok(None);
        }
        let Some(magnitude) = DefaultLiterals::parse_integer(raw) else {
            Self::invalid(
                diagnostic_message!(
                    "diagnostic.schema.default_integer_unrepresentable",
                    "integer literal is out of the representable range"
                ),
                number.span(),
                diagnostics,
                out.reborrow(),
            )?;
            return Ok(None);
        };
        if DefaultLiterals::is_signed(builtin) {
            let Some((minimum, maximum)) = Numeric::signed_bounds(builtin) else {
                return Ok(None);
            };
            let value = if number.is_negative() {
                let limit = minimum.unsigned_abs();
                if magnitude > limit {
                    Self::invalid(
                        diagnostic_message!(
                            "diagnostic.schema.default_integer_out_of_range",
                            "integer literal is out of the target type's range"
                        ),
                        number.span(),
                        diagnostics,
                        out.reborrow(),
                    )?;
                    return Ok(None);
                }
                if magnitude == limit {
                    minimum
                } else {
                    let Ok(value) = i128::try_from(magnitude) else {
                        Self::invalid(
                            diagnostic_message!(
                                "diagnostic.schema.default_integer_out_of_range",
                                "integer literal is out of the target type's range"
                            ),
                            number.span(),
                            diagnostics,
                            out.reborrow(),
                        )?;
                        return Ok(None);
                    };
                    -value
                }
            } else {
                let Ok(value) = i128::try_from(magnitude) else {
                    Self::invalid(
                        diagnostic_message!(
                            "diagnostic.schema.default_integer_out_of_range",
                            "integer literal is out of the target type's range"
                        ),
                        number.span(),
                        diagnostics,
                        out.reborrow(),
                    )?;
                    return Ok(None);
                };
                value
            };
            if value < minimum || value > maximum {
                Self::invalid(
                    diagnostic_message!(
                        "diagnostic.schema.default_integer_out_of_range",
                        "integer literal is out of the target type's range"
                    ),
                    number.span(),
                    diagnostics,
                    out.reborrow(),
                )?;
                return Ok(None);
            }
            Ok(Some(DefaultNumber::Signed(value)))
        } else if DefaultLiterals::is_unsigned(builtin) {
            if number.is_negative() || magnitude > Numeric::unsigned_max(builtin).unwrap_or(0) {
                Self::invalid(
                    diagnostic_message!(
                        "diagnostic.schema.default_integer_out_of_range",
                        "integer literal is out of the target type's range"
                    ),
                    number.span(),
                    diagnostics,
                    out.reborrow(),
                )?;
                return Ok(None);
            }
            Ok(Some(DefaultNumber::Unsigned(magnitude)))
        } else {
            Self::invalid(
                diagnostic_message!(
                    "diagnostic.schema.default_numeric_type_mismatch",
                    "numeric default does not match the target type"
                ),
                number.span(),
                diagnostics,
                out.reborrow(),
            )?;
            Ok(None)
        }
    }

    fn invalid<'slot>(
        message: DiagnosticMessage<'static>,
        span: SourceSpan,
        diagnostics: &mut DiagnosticCollector,
        out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ()> {
        diagnostics.push(diagnostics.make_semantic(message, span));
        if diagnostics.is_stopped() {
            Err(out.fail(BuildFailure::empty()))
        } else {
            Ok(())
        }
    }

    pub(crate) fn parse_integer(raw: &str) -> Option<u128> {
        if let Some(hex) = raw.strip_prefix("0x") {
            u128::from_str_radix(hex, 16).ok()
        } else {
            raw.parse::<u128>().ok()
        }
    }

    pub(crate) fn is_float(builtin: BuiltinType) -> bool {
        matches!(builtin, BuiltinType::F32 | BuiltinType::F64)
    }

    pub(crate) fn is_signed(builtin: BuiltinType) -> bool {
        matches!(
            builtin,
            BuiltinType::I8
                | BuiltinType::I16
                | BuiltinType::I32
                | BuiltinType::I64
                | BuiltinType::I128
        )
    }

    pub(crate) fn is_unsigned(builtin: BuiltinType) -> bool {
        matches!(
            builtin,
            BuiltinType::U8
                | BuiltinType::U16
                | BuiltinType::U32
                | BuiltinType::U64
                | BuiltinType::U128
        )
    }

    pub(crate) fn optional_string<'slot, 'a>(
        mut value: ValueKind<'a>,
        context: &CompileContext,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, Option<Option<&'a str>>> {
        let result = loop {
            context
                .execution()
                .consume::<BuildFailure>(1, out.reborrow())?;
            match value {
                ValueKind::Optional(None) => break Some(None),
                ValueKind::Optional(Some(inner)) => value = inner.as_ref(),
                ValueKind::String(value) => break Some(Some(value)),
                _ => break None,
            }
        };
        Ok(result)
    }
}

pub(crate) struct DefaultDiagnostic;

pub(super) struct ConstraintLabelInput<'a> {
    pub(super) diagnostic: &'a mut Diagnostic,
    pub(super) constraint: SourceSpan,
    pub(super) fallback: SourceSpan,
    pub(super) context: &'a CompileContext,
    pub(super) sink: &'a mut DiagnosticCollector,
}

impl DefaultDiagnostic {
    pub(crate) fn scalar_type_mismatch() -> DiagnosticMessage<'static> {
        diagnostic_message!(
            "diagnostic.schema.default_scalar_type_mismatch",
            "scalar default does not match the target type"
        )
    }

    pub(crate) fn string_type_mismatch() -> DiagnosticMessage<'static> {
        diagnostic_message!(
            "diagnostic.schema.default_string_type_mismatch",
            "string default does not match the target type"
        )
    }

    pub(crate) fn boolean_type_mismatch() -> DiagnosticMessage<'static> {
        diagnostic_message!(
            "diagnostic.schema.default_boolean_type_mismatch",
            "boolean default does not match the target type"
        )
    }

    pub(crate) fn local_constraint_failed() -> DiagnosticMessage<'static> {
        diagnostic_message!(
            "diagnostic.schema.default_local_constraint_failed",
            "schema default does not satisfy the local constraint"
        )
    }

    pub(crate) fn regex_constraint_failed() -> DiagnosticMessage<'static> {
        diagnostic_message!(
            "diagnostic.schema.default_regex_constraint_failed",
            "schema default does not satisfy the regex constraint"
        )
    }

    pub(crate) fn semantic<'slot>(
        message: DiagnosticMessage<'static>,
        span: SourceSpan,
        context: &CompileContext,
        sink: &mut DiagnosticCollector,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, Diagnostic> {
        let mut diagnostic = context.budget_access(out.reborrow(), span, sink, |budget_out| {
            Diagnostic::try_new(
                DiagnosticCode::SCHEMA_SEMANTIC_ERROR,
                message,
                Some(context.execution()),
                budget_out,
            )
        })?;
        diagnostic.set_primary(Some(span));
        Ok(diagnostic)
    }

    pub(crate) fn constraint_label<'slot>(
        input: ConstraintLabelInput<'_>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ()> {
        input
            .context
            .budget_access(out.reborrow(), input.fallback, input.sink, |budget_out| {
                input.diagnostic.try_add_label_with_text(
                    input.constraint,
                    diagnostic_message!(
                        "diagnostic.label.constraint_defined_here",
                        "constraint is defined here"
                    ),
                    Some(input.context.execution()),
                    budget_out,
                )
            })?;
        Ok(())
    }

    pub(crate) fn all_failure<'slot>(
        primary: SourceSpan,
        constraint: SourceSpan,
        context: &CompileContext,
        sink: &mut DiagnosticCollector,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, Diagnostic> {
        let mut diagnostic =
            context.budget_access(out.reborrow(), primary, sink, |budget_out| {
                Diagnostic::try_new(
                    DiagnosticCode::SCHEMA_SEMANTIC_ERROR,
                    diagnostic_message!(
                        "diagnostic.schema.default_all_constraint_failed",
                        "schema default does not satisfy the @all constraint"
                    ),
                    Some(context.execution()),
                    budget_out,
                )
            })?;
        diagnostic.set_primary(Some(primary));
        Self::constraint_label(
            ConstraintLabelInput {
                diagnostic: &mut diagnostic,
                constraint,
                fallback: primary,
                context,
                sink,
            },
            out.reborrow(),
        )?;
        Ok(diagnostic)
    }

    pub(crate) fn execution(
        error: EvalError,
        span: SourceSpan,
        diagnostics: &DiagnosticCollector,
    ) -> Diagnostic {
        let span = error.source_span().unwrap_or(span);
        if let Some(access) = error.schema_access() {
            return EmergencyDiagnostics::internal(
                span,
                EmergencyDiagnostics::schema_access_context(access),
            );
        }
        if let Some(context) = error.context() {
            return EmergencyDiagnostics::internal(span, context);
        }
        match error.kind() {
            EvalFailure::Internal | EvalFailure::InternalAt(_) => schema_internal!(span),
            EvalFailure::Allocation => EmergencyDiagnostics::allocation(span),
            EvalFailure::Resource => EmergencyDiagnostics::resource(span),
            EvalFailure::Capacity => EmergencyDiagnostics::capacity(span),
            EvalFailure::Arithmetic => diagnostics.make_semantic(
                diagnostic_message!(
                    "diagnostic.schema.default_numeric_evaluation_failed",
                    "schema default numeric evaluation failed"
                ),
                span,
            ),
            EvalFailure::SchemaAccess(_) => schema_internal!(span),
        }
    }
}
