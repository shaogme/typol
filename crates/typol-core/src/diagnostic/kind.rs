use super::foundation::{DiagnosticCode, DiagnosticContext};
use core::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticStage {
    Input,
    Lexing,
    Parsing,
    SchemaResolution,
    SchemaCompilation,
    ConfigurationDecoding,
    ConstraintValidation,
    Environment,
    TypeBinding,
    Conversion,
    Encoding,
    HostIo,
    Internal,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct DiagnosticKind {
    id: &'static str,
    stage: DiagnosticStage,
}

impl DiagnosticStage {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Lexing => "lexing",
            Self::Parsing => "parsing",
            Self::SchemaResolution => "schema_resolution",
            Self::SchemaCompilation => "schema_compilation",
            Self::ConfigurationDecoding => "configuration_decoding",
            Self::ConstraintValidation => "constraint_validation",
            Self::Environment => "environment",
            Self::TypeBinding => "type_binding",
            Self::Conversion => "conversion",
            Self::Encoding => "encoding",
            Self::HostIo => "host_io",
            Self::Internal => "internal",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticFieldStatus {
    NotApplicable,
    Available,
    Unavailable,
    Redacted,
}

impl DiagnosticKind {
    pub const fn new(id: &'static str, stage: DiagnosticStage) -> Self {
        Self { id, stage }
    }

    pub const fn id(self) -> &'static str {
        self.id
    }

    pub const fn stage(self) -> DiagnosticStage {
        self.stage
    }

    pub(super) const fn custom(code: DiagnosticCode) -> Self {
        Self::new(code.name(), stage_for_code(code))
    }

    pub(super) fn emergency(code: DiagnosticCode, context: Option<DiagnosticContext>) -> Self {
        let failure_point = context.map(|context| context.failure_point());
        let (id, stage) = match (code, failure_point) {
            (DiagnosticCode::RESOURCE_LIMIT_EXCEEDED, Some("conversion-depth-limit")) => (
                "typol.binding.conversion_depth_exceeded",
                DiagnosticStage::TypeBinding,
            ),
            (DiagnosticCode::TYPE_BINDING_ERROR, Some("recursive-rust-type")) => (
                "typol.binding.recursive_rust_type",
                DiagnosticStage::TypeBinding,
            ),
            (DiagnosticCode::TYPE_BINDING_ERROR, Some("array-length-unrepresentable")) => (
                "typol.binding.array_length_unrepresentable",
                DiagnosticStage::TypeBinding,
            ),
            (DiagnosticCode::TYPE_BINDING_ERROR, Some("schema-type-mismatch")) => {
                ("typol.binding.type_mismatch", DiagnosticStage::TypeBinding)
            }
            (DiagnosticCode::RUST_CONVERSION_ERROR, _) => {
                ("typol.conversion.failed", DiagnosticStage::Conversion)
            }
            (DiagnosticCode::CONFIG_ENCODING_ERROR, Some("secret-output-rejected")) => (
                "typol.encoding.secret_output_rejected",
                DiagnosticStage::Encoding,
            ),
            (DiagnosticCode::CONFIG_ENCODING_ERROR, _) => {
                ("typol.encoding.unrepresentable", DiagnosticStage::Encoding)
            }
            (DiagnosticCode::SCHEMA_CONTEXT_MISMATCH, _) => (
                "typol.schema.root_context_mismatch",
                DiagnosticStage::SchemaCompilation,
            ),
            (DiagnosticCode::SCHEMA_SESSION_FINISHED, _) => (
                "typol.schema.session_finished",
                DiagnosticStage::SchemaCompilation,
            ),
            (DiagnosticCode::CAPACITY_EXCEEDED, Some("error-payload-capacity-exceeded")) => (
                "typol.internal.error_payload_capacity_exceeded",
                DiagnosticStage::Internal,
            ),
            _ => return Self::custom(code),
        };
        Self::new(id, stage)
    }
}

impl fmt::Debug for DiagnosticKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DiagnosticKind")
            .field("id", &self.id)
            .field("stage", &self.stage)
            .finish()
    }
}

const fn stage_for_code(code: DiagnosticCode) -> DiagnosticStage {
    match code {
        DiagnosticCode::SOURCE_INPUT_ERROR => DiagnosticStage::Input,
        DiagnosticCode::LEXICAL_ERROR => DiagnosticStage::Lexing,
        DiagnosticCode::SCHEMA_SYNTAX_ERROR | DiagnosticCode::CONFIG_SYNTAX_ERROR => {
            DiagnosticStage::Parsing
        }
        DiagnosticCode::SCHEMA_RESOLVER_ERROR => DiagnosticStage::SchemaResolution,
        DiagnosticCode::SCHEMA_SEMANTIC_ERROR
        | DiagnosticCode::SCHEMA_CONTEXT_MISMATCH
        | DiagnosticCode::SCHEMA_SESSION_FINISHED => DiagnosticStage::SchemaCompilation,
        DiagnosticCode::CONFIG_VALUE_ERROR => DiagnosticStage::ConfigurationDecoding,
        DiagnosticCode::CONFIG_VALIDATION_ERROR => DiagnosticStage::ConstraintValidation,
        DiagnosticCode::CONFIG_ENVIRONMENT_ERROR => DiagnosticStage::Environment,
        DiagnosticCode::TYPE_BINDING_ERROR => DiagnosticStage::TypeBinding,
        DiagnosticCode::RUST_CONVERSION_ERROR => DiagnosticStage::Conversion,
        DiagnosticCode::CONFIG_ENCODING_ERROR => DiagnosticStage::Encoding,
        _ => DiagnosticStage::Internal,
    }
}
