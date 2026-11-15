use super::foundation::{DiagnosticCode, DiagnosticContext};
use core::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticStage {
    Input,
    ExecutionBudget,
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
    Formatting,
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
            Self::ExecutionBudget => "execution_budget",
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
            Self::Formatting => "formatting",
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

    pub(super) fn custom(code: DiagnosticCode, context: Option<DiagnosticContext>) -> Self {
        Self::new(
            code.name(),
            context
                .map(stage_for_context)
                .unwrap_or_else(|| stage_for_code(code)),
        )
    }

    pub(super) fn emergency(code: DiagnosticCode, context: Option<DiagnosticContext>) -> Self {
        let failure_point = context.map(|context| context.failure_point());
        let (id, stage) = match (code, failure_point) {
            (DiagnosticCode::TYPE_BINDING_DEPTH_LIMIT_EXCEEDED, Some("conversion-depth-limit")) => {
                (
                    "typol.binding.conversion_depth_exceeded",
                    DiagnosticStage::TypeBinding,
                )
            }
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
            _ => return Self::custom(code, context),
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
        DiagnosticCode::SOURCE_BYTES_LIMIT_EXCEEDED
        | DiagnosticCode::SOURCE_INPUT_CAPACITY_EXCEEDED
        | DiagnosticCode::SOURCE_INPUT_INVARIANT_FAILURE
        | DiagnosticCode::SOURCE_LOCATION_INVARIANT_FAILURE => DiagnosticStage::Input,
        DiagnosticCode::EXECUTION_STEPS_LIMIT_EXCEEDED
        | DiagnosticCode::WORKSPACE_BYTES_LIMIT_EXCEEDED
        | DiagnosticCode::BUDGET_CAPACITY_EXCEEDED
        | DiagnosticCode::BUDGET_INVARIANT_FAILURE => DiagnosticStage::ExecutionBudget,
        DiagnosticCode::LEXICAL_ERROR => DiagnosticStage::Lexing,
        DiagnosticCode::SCHEMA_SYNTAX_ERROR
        | DiagnosticCode::CONFIG_SYNTAX_ERROR
        | DiagnosticCode::SCHEMA_PARSE_DEPTH_LIMIT_EXCEEDED
        | DiagnosticCode::TOKEN_BYTES_LIMIT_EXCEEDED
        | DiagnosticCode::STRING_BYTES_LIMIT_EXCEEDED
        | DiagnosticCode::SCHEMA_AST_NODES_LIMIT_EXCEEDED
        | DiagnosticCode::SYNTAX_STORAGE_CAPACITY_EXCEEDED
        | DiagnosticCode::SYNTAX_STORAGE_INVARIANT_FAILURE
        | DiagnosticCode::CONFIG_PARSER_INVARIANT_FAILURE => DiagnosticStage::Parsing,
        DiagnosticCode::SCHEMA_RESOLVER_ERROR
        | DiagnosticCode::SCHEMA_TOTAL_BYTES_LIMIT_EXCEEDED
        | DiagnosticCode::SCHEMA_SOURCE_COUNT_LIMIT_EXCEEDED
        | DiagnosticCode::SCHEMA_IMPORT_DEPTH_LIMIT_EXCEEDED
        | DiagnosticCode::SCHEMA_RESOLVER_INVARIANT_FAILURE => DiagnosticStage::SchemaResolution,
        DiagnosticCode::SCHEMA_SEMANTIC_ERROR
        | DiagnosticCode::SCHEMA_CONTEXT_MISMATCH
        | DiagnosticCode::SCHEMA_SESSION_FINISHED
        | DiagnosticCode::REGEX_COMPILATION_BUDGET_EXCEEDED
        | DiagnosticCode::REGEX_PATTERN_BYTES_LIMIT_EXCEEDED
        | DiagnosticCode::REGEX_NESTING_DEPTH_LIMIT_EXCEEDED
        | DiagnosticCode::REGEX_INSTRUCTION_COUNT_LIMIT_EXCEEDED
        | DiagnosticCode::REGEX_CHARACTER_RANGES_LIMIT_EXCEEDED
        | DiagnosticCode::REGEX_COMPILATION_CAPACITY_EXCEEDED
        | DiagnosticCode::REGEX_COMPILATION_INVARIANT_FAILURE
        | DiagnosticCode::SCHEMA_STORAGE_CAPACITY_EXCEEDED
        | DiagnosticCode::SCHEMA_AST_ACCESS_INVARIANT_FAILURE
        | DiagnosticCode::SCHEMA_ACCESS_INVARIANT_FAILURE
        | DiagnosticCode::SCHEMA_COMPILATION_INVARIANT_FAILURE
        | DiagnosticCode::DEFAULT_CONSTRUCTION_INVARIANT_FAILURE => {
            DiagnosticStage::SchemaCompilation
        }
        DiagnosticCode::CONFIG_VALUE_ERROR
        | DiagnosticCode::CONFIG_PARSE_DEPTH_LIMIT_EXCEEDED
        | DiagnosticCode::CONFIG_VALUE_DEPTH_LIMIT_EXCEEDED
        | DiagnosticCode::CONFIG_VALUE_NODES_LIMIT_EXCEEDED
        | DiagnosticCode::CONFIG_TEXT_BYTES_LIMIT_EXCEEDED
        | DiagnosticCode::CONFIG_VALUE_CAPACITY_EXCEEDED
        | DiagnosticCode::CONFIG_VALUE_INVARIANT_FAILURE
        | DiagnosticCode::CONFIG_ENCODING_INVARIANT_FAILURE
        | DiagnosticCode::CONFIG_IR_INVARIANT_FAILURE => DiagnosticStage::ConfigurationDecoding,
        DiagnosticCode::CONFIG_VALIDATION_ERROR
        | DiagnosticCode::VALIDATION_TRAVERSAL_LIMIT_EXCEEDED
        | DiagnosticCode::REGEX_EXECUTION_STEPS_LIMIT_EXCEEDED
        | DiagnosticCode::REGEX_SEARCH_STEPS_LIMIT_EXCEEDED
        | DiagnosticCode::REGEX_WORKSPACE_BYTES_LIMIT_EXCEEDED
        | DiagnosticCode::REGEX_SCRATCH_STATES_LIMIT_EXCEEDED
        | DiagnosticCode::CONFIG_DIAGNOSTIC_CAPACITY_EXCEEDED
        | DiagnosticCode::CONFIG_VALIDATION_INVARIANT_FAILURE
        | DiagnosticCode::SEMANTIC_EVALUATION_INVARIANT_FAILURE
        | DiagnosticCode::VALUE_COPY_INVARIANT_FAILURE
        | DiagnosticCode::REGEX_EXECUTION_CAPACITY_EXCEEDED
        | DiagnosticCode::REGEX_EXECUTION_INVARIANT_FAILURE => {
            DiagnosticStage::ConstraintValidation
        }
        DiagnosticCode::CONFIG_ENVIRONMENT_ERROR
        | DiagnosticCode::ENVIRONMENT_ENTRIES_LIMIT_EXCEEDED
        | DiagnosticCode::ENVIRONMENT_BYTES_LIMIT_EXCEEDED
        | DiagnosticCode::ENVIRONMENT_CACHE_CAPACITY_EXCEEDED
        | DiagnosticCode::ENVIRONMENT_INVARIANT_FAILURE => DiagnosticStage::Environment,
        DiagnosticCode::TYPE_BINDING_ERROR
        | DiagnosticCode::TYPE_BINDING_DEPTH_LIMIT_EXCEEDED
        | DiagnosticCode::TYPE_BINDING_NODES_LIMIT_EXCEEDED
        | DiagnosticCode::TYPE_BINDING_INVARIANT_FAILURE => DiagnosticStage::TypeBinding,
        DiagnosticCode::RUST_CONVERSION_ERROR
        | DiagnosticCode::RUST_CONVERSION_CAPACITY_EXCEEDED
        | DiagnosticCode::RUST_CONVERSION_INVARIANT_FAILURE => DiagnosticStage::Conversion,
        DiagnosticCode::CONFIG_ENCODING_ERROR | DiagnosticCode::CODEC_CAPACITY_EXCEEDED => {
            DiagnosticStage::Encoding
        }
        DiagnosticCode::SCHEMA_FORMAT_OUTPUT_BYTES_LIMIT_EXCEEDED
        | DiagnosticCode::CONFIG_FORMAT_OUTPUT_BYTES_LIMIT_EXCEEDED
        | DiagnosticCode::FORMATTING_WORKSPACE_BYTES_LIMIT_EXCEEDED
        | DiagnosticCode::FORMATTING_NESTING_DEPTH_LIMIT_EXCEEDED
        | DiagnosticCode::FORMAT_OUTPUT_CAPACITY_EXCEEDED
        | DiagnosticCode::CONFIG_EDIT_CAPACITY_EXCEEDED
        | DiagnosticCode::CONFIG_EDIT_INVARIANT_FAILURE
        | DiagnosticCode::DIAGNOSTIC_RENDERING_INVARIANT_FAILURE
        | DiagnosticCode::SOURCE_EXCERPT_INVARIANT_FAILURE => DiagnosticStage::Formatting,
        DiagnosticCode::HOST_SOURCE_CAPACITY_EXCEEDED
        | DiagnosticCode::HOST_ADAPTER_INVARIANT_FAILURE => DiagnosticStage::HostIo,
        DiagnosticCode::DIAGNOSTIC_PAYLOAD_CAPACITY_EXCEEDED
        | DiagnosticCode::VALUE_STRUCTURE_INVARIANT_FAILURE
        | DiagnosticCode::SECRET_POLICY_INVARIANT_FAILURE
        | DiagnosticCode::DIAGNOSTIC_COLLECTION_INVARIANT_FAILURE => DiagnosticStage::Internal,
        _ => DiagnosticStage::Internal,
    }
}

fn stage_for_context(context: DiagnosticContext) -> DiagnosticStage {
    match context.stage() {
        "input" | "source-input" => DiagnosticStage::Input,
        "lexing" | "lexer" => DiagnosticStage::Lexing,
        "parsing" | "parser" | "schema-parser" | "config-parser" => DiagnosticStage::Parsing,
        "schema-resolution" | "schema-resolver" | "schema-loader" => {
            DiagnosticStage::SchemaResolution
        }
        "schema-compilation" | "schema-compiler" | "regex-compile" => {
            DiagnosticStage::SchemaCompilation
        }
        "configuration-decoding" | "config-decoding" => DiagnosticStage::ConfigurationDecoding,
        "constraint-validation" | "validation" => DiagnosticStage::ConstraintValidation,
        "environment" => DiagnosticStage::Environment,
        "type-binding" => DiagnosticStage::TypeBinding,
        "conversion" => DiagnosticStage::Conversion,
        "encoding" => DiagnosticStage::Encoding,
        "formatting" | "document-formatting" | "schema-formatting" => DiagnosticStage::Formatting,
        "host-io" | "filesystem" | "source-writer" => DiagnosticStage::HostIo,
        _ => DiagnosticStage::Internal,
    }
}
