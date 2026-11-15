use super::binding::BindingConversion;
use crate::diagnostics::{
    CodecErrors, CodecStage, ConversionDiagnosticInput, ConversionErrorKind, ConversionPath,
    Failure,
};
use typol_core::{BudgetError, ResourceLimit, ValueError, ValueKind};
use typol_i18n::Translator;
use typol_semantic::TypeKind;

pub(super) struct ShapeMismatch {
    pub(super) kind: ConversionErrorKind,
    pub(super) expected: &'static str,
    pub(super) actual: &'static str,
}

pub(super) struct ConversionDiagnostics;

impl ConversionDiagnostics {
    pub(super) fn binding_context<'path>(
        path: ConversionPath<'path>,
    ) -> ConversionDiagnosticInput<'path> {
        ConversionDiagnosticInput::new(
            ConversionErrorKind::InternalBindingInvariant,
            "initialized binding context",
            "binding context is not initialized",
            path,
        )
    }

    pub(super) fn budget_error(error: BudgetError, translator: Translator) -> CodecErrors {
        CodecErrors::emergency(CodecStage::Conversion, error.into(), translator)
    }

    pub(super) fn value_error(error: ValueError, translator: Translator) -> CodecErrors {
        CodecErrors::emergency(CodecStage::Conversion, error.into(), translator)
    }

    pub(super) fn conversion_error(translator: Translator) -> CodecErrors {
        CodecErrors::emergency(CodecStage::Conversion, Failure::Conversion, translator)
    }

    pub(super) fn resource_error(limit: ResourceLimit, translator: Translator) -> CodecErrors {
        CodecErrors::emergency(
            CodecStage::Conversion,
            Failure::ResourceLimit(limit),
            translator,
        )
    }

    pub(super) fn allocation_error(translator: Translator) -> CodecErrors {
        CodecErrors::emergency(CodecStage::Conversion, Failure::Allocation, translator)
    }

    pub(super) fn shape_mismatch<'schema>(
        expected: TypeKind<'schema>,
        actual: ValueKind<'_>,
    ) -> ShapeMismatch {
        match (expected, actual) {
            (
                TypeKind::Array {
                    length: Some(_), ..
                },
                ValueKind::Array(_),
            ) => ShapeMismatch {
                kind: ConversionErrorKind::ArrayLengthMismatch,
                expected: "array with the schema length",
                actual: "array with a mismatched length",
            },
            (TypeKind::Struct { .. }, ValueKind::Structure(_)) => ShapeMismatch {
                kind: ConversionErrorKind::FieldCountMismatch,
                expected: "struct containing all schema fields",
                actual: "struct with a mismatched field count",
            },
            (expected, actual) => ShapeMismatch {
                kind: ConversionErrorKind::ValueShapeMismatch,
                expected: BindingConversion::type_shape(expected),
                actual: Self::value_shape(actual),
            },
        }
    }

    fn value_shape(kind: ValueKind<'_>) -> &'static str {
        match kind {
            ValueKind::Signed(_) => "signed integer",
            ValueKind::Unsigned(_) => "unsigned integer",
            ValueKind::Float32(_) => "f32 value",
            ValueKind::Float64(_) => "f64 value",
            ValueKind::String(_) => "string",
            ValueKind::Bool(_) => "boolean value",
            ValueKind::Optional(_) => "optional value",
            ValueKind::Array(_) => "array",
            ValueKind::Map(_) => "map",
            ValueKind::Structure(_) => "struct",
            ValueKind::Enumeration { .. } => "enum",
        }
    }
}
