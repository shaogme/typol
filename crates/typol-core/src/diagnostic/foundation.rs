use core::fmt;
use typol_i18n::{Translator, t};
use u18n::RenderError;

/// A diagnostic context description composed of static identifiers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiagnosticContextDescriptor {
    subsystem: &'static str,
    stage: &'static str,
    failure_point: &'static str,
}

impl DiagnosticContextDescriptor {
    pub const fn new(
        subsystem: &'static str,
        stage: &'static str,
        failure_point: &'static str,
    ) -> Self {
        Self {
            subsystem,
            stage,
            failure_point,
        }
    }

    pub const fn subsystem(self) -> &'static str {
        self.subsystem
    }

    pub const fn stage(self) -> &'static str {
        self.stage
    }

    pub const fn failure_point(self) -> &'static str {
        self.failure_point
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticObjectIdentifier {
    None,
    Id(u64),
    Index(usize),
    Range { start: usize, end: usize },
}

/// A compact context pointing to a static description with an optional bounded numeric object identifier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiagnosticContext {
    descriptor: &'static DiagnosticContextDescriptor,
    object: DiagnosticObjectIdentifier,
}

impl DiagnosticContext {
    pub const fn new(descriptor: &'static DiagnosticContextDescriptor) -> Self {
        Self {
            descriptor,
            object: DiagnosticObjectIdentifier::None,
        }
    }

    pub const fn with_object_id(mut self, object_id: u64) -> Self {
        self.object = DiagnosticObjectIdentifier::Id(object_id);
        self
    }

    /// Retains an exact `usize` index for structured error queries.
    pub const fn with_object_index(mut self, object_index: usize) -> Self {
        self.object = DiagnosticObjectIdentifier::Index(object_index);
        self
    }

    /// Retains both `usize` endpoints of a range exactly.
    pub const fn with_object_range(mut self, start: usize, end: usize) -> Self {
        self.object = DiagnosticObjectIdentifier::Range { start, end };
        self
    }

    pub const fn subsystem(self) -> &'static str {
        self.descriptor.subsystem()
    }

    pub const fn stage(self) -> &'static str {
        self.descriptor.stage()
    }

    pub const fn failure_point(self) -> &'static str {
        self.descriptor.failure_point()
    }

    pub const fn object_id(self) -> Option<u64> {
        match self.object {
            DiagnosticObjectIdentifier::Id(object_id) => Some(object_id),
            DiagnosticObjectIdentifier::None
            | DiagnosticObjectIdentifier::Index(_)
            | DiagnosticObjectIdentifier::Range { .. } => None,
        }
    }

    pub const fn object_identifier(self) -> DiagnosticObjectIdentifier {
        self.object
    }

    pub(crate) const fn descriptor(self) -> &'static DiagnosticContextDescriptor {
        self.descriptor
    }

    /// Returns the exactly retained `usize` object index.
    pub const fn object_index(self) -> Option<usize> {
        match self.object {
            DiagnosticObjectIdentifier::Index(index) => Some(index),
            _ => None,
        }
    }

    /// Returns exactly retained range endpoints.
    pub const fn object_range(self) -> Option<(usize, usize)> {
        match self.object {
            DiagnosticObjectIdentifier::Range { start, end } => Some((start, end)),
            _ => None,
        }
    }
}

/// A stable diagnostic code. Once assigned, its value and meaning do not change within the same major version.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct DiagnosticCode(u32);

impl DiagnosticCode {
    pub const ALLOCATION_FAILED: Self = Self(2);
    pub const LEXICAL_ERROR: Self = Self(3);
    pub const SCHEMA_SYNTAX_ERROR: Self = Self(4);
    pub const DIAGNOSTIC_LIMIT_REACHED: Self = Self(5);
    pub const REGEX_SYNTAX_ERROR: Self = Self(6);
    pub const SCHEMA_SEMANTIC_ERROR: Self = Self(7);
    pub const SCHEMA_RESOLVER_ERROR: Self = Self(8);
    pub const CONFIG_SYNTAX_ERROR: Self = Self(10);
    pub const CONFIG_VALUE_ERROR: Self = Self(11);
    pub const CONFIG_VALIDATION_ERROR: Self = Self(12);
    pub const CONFIG_ENVIRONMENT_ERROR: Self = Self(13);
    pub const TYPE_BINDING_ERROR: Self = Self(14);
    pub const RUST_CONVERSION_ERROR: Self = Self(15);
    pub const CONFIG_ENCODING_ERROR: Self = Self(16);
    pub const SOURCE_INPUT_ERROR: Self = Self(17);
    pub const SCHEMA_CONTEXT_MISMATCH: Self = Self(19);
    pub const SCHEMA_SESSION_FINISHED: Self = Self(20);
    pub const SOURCE_BYTES_LIMIT_EXCEEDED: Self = Self(21);
    pub const SCHEMA_TOTAL_BYTES_LIMIT_EXCEEDED: Self = Self(22);
    pub const SCHEMA_SOURCE_COUNT_LIMIT_EXCEEDED: Self = Self(23);
    pub const SCHEMA_IMPORT_DEPTH_LIMIT_EXCEEDED: Self = Self(24);
    pub const EXECUTION_STEPS_LIMIT_EXCEEDED: Self = Self(25);
    pub const WORKSPACE_BYTES_LIMIT_EXCEEDED: Self = Self(26);
    pub const SCHEMA_PARSE_DEPTH_LIMIT_EXCEEDED: Self = Self(27);
    pub const TOKEN_BYTES_LIMIT_EXCEEDED: Self = Self(28);
    pub const STRING_BYTES_LIMIT_EXCEEDED: Self = Self(29);
    pub const SCHEMA_AST_NODES_LIMIT_EXCEEDED: Self = Self(30);
    pub const CONFIG_PARSE_DEPTH_LIMIT_EXCEEDED: Self = Self(31);
    pub const CONFIG_VALUE_DEPTH_LIMIT_EXCEEDED: Self = Self(32);
    pub const CONFIG_VALUE_NODES_LIMIT_EXCEEDED: Self = Self(33);
    pub const CONFIG_TEXT_BYTES_LIMIT_EXCEEDED: Self = Self(34);
    pub const ENVIRONMENT_ENTRIES_LIMIT_EXCEEDED: Self = Self(35);
    pub const ENVIRONMENT_BYTES_LIMIT_EXCEEDED: Self = Self(36);
    pub const VALIDATION_TRAVERSAL_LIMIT_EXCEEDED: Self = Self(37);
    pub const REGEX_COMPILATION_BUDGET_EXCEEDED: Self = Self(38);
    pub const REGEX_PATTERN_BYTES_LIMIT_EXCEEDED: Self = Self(39);
    pub const REGEX_NESTING_DEPTH_LIMIT_EXCEEDED: Self = Self(40);
    pub const REGEX_INSTRUCTION_COUNT_LIMIT_EXCEEDED: Self = Self(41);
    pub const REGEX_CHARACTER_RANGES_LIMIT_EXCEEDED: Self = Self(42);
    pub const REGEX_EXECUTION_STEPS_LIMIT_EXCEEDED: Self = Self(43);
    pub const REGEX_SEARCH_STEPS_LIMIT_EXCEEDED: Self = Self(44);
    pub const REGEX_WORKSPACE_BYTES_LIMIT_EXCEEDED: Self = Self(45);
    pub const REGEX_SCRATCH_STATES_LIMIT_EXCEEDED: Self = Self(46);
    pub const TYPE_BINDING_DEPTH_LIMIT_EXCEEDED: Self = Self(47);
    pub const SCHEMA_FORMAT_OUTPUT_BYTES_LIMIT_EXCEEDED: Self = Self(48);
    pub const CONFIG_FORMAT_OUTPUT_BYTES_LIMIT_EXCEEDED: Self = Self(49);
    pub const FORMATTING_WORKSPACE_BYTES_LIMIT_EXCEEDED: Self = Self(50);
    pub const FORMATTING_NESTING_DEPTH_LIMIT_EXCEEDED: Self = Self(51);
    pub const TYPE_BINDING_NODES_LIMIT_EXCEEDED: Self = Self(52);
    pub const BUDGET_CAPACITY_EXCEEDED: Self = Self(70);
    pub const SOURCE_INPUT_CAPACITY_EXCEEDED: Self = Self(71);
    pub const DIAGNOSTIC_PAYLOAD_CAPACITY_EXCEEDED: Self = Self(72);
    pub const SYNTAX_STORAGE_CAPACITY_EXCEEDED: Self = Self(73);
    pub const SCHEMA_STORAGE_CAPACITY_EXCEEDED: Self = Self(74);
    pub const CONFIG_VALUE_CAPACITY_EXCEEDED: Self = Self(75);
    pub const CONFIG_DIAGNOSTIC_CAPACITY_EXCEEDED: Self = Self(76);
    pub const REGEX_COMPILATION_CAPACITY_EXCEEDED: Self = Self(77);
    pub const REGEX_EXECUTION_CAPACITY_EXCEEDED: Self = Self(78);
    pub const ENVIRONMENT_CACHE_CAPACITY_EXCEEDED: Self = Self(79);
    pub const FORMAT_OUTPUT_CAPACITY_EXCEEDED: Self = Self(80);
    pub const CONFIG_EDIT_CAPACITY_EXCEEDED: Self = Self(81);
    pub const HOST_SOURCE_CAPACITY_EXCEEDED: Self = Self(82);
    pub const RUST_CONVERSION_CAPACITY_EXCEEDED: Self = Self(83);
    pub const CODEC_CAPACITY_EXCEEDED: Self = Self(84);
    pub const BUDGET_INVARIANT_FAILURE: Self = Self(1000);
    pub const SOURCE_INPUT_INVARIANT_FAILURE: Self = Self(1001);
    pub const SOURCE_LOCATION_INVARIANT_FAILURE: Self = Self(1002);
    pub const VALUE_STRUCTURE_INVARIANT_FAILURE: Self = Self(1003);
    pub const SYNTAX_STORAGE_INVARIANT_FAILURE: Self = Self(1004);
    pub const SCHEMA_AST_ACCESS_INVARIANT_FAILURE: Self = Self(1005);
    pub const SCHEMA_ACCESS_INVARIANT_FAILURE: Self = Self(1006);
    pub const SCHEMA_RESOLVER_INVARIANT_FAILURE: Self = Self(1007);
    pub const SCHEMA_COMPILATION_INVARIANT_FAILURE: Self = Self(1008);
    pub const CONFIG_PARSER_INVARIANT_FAILURE: Self = Self(1009);
    pub const CONFIG_VALUE_INVARIANT_FAILURE: Self = Self(1010);
    pub const CONFIG_VALIDATION_INVARIANT_FAILURE: Self = Self(1011);
    pub const ENVIRONMENT_INVARIANT_FAILURE: Self = Self(1012);
    pub const SECRET_POLICY_INVARIANT_FAILURE: Self = Self(1013);
    pub const SEMANTIC_EVALUATION_INVARIANT_FAILURE: Self = Self(1014);
    pub const DEFAULT_CONSTRUCTION_INVARIANT_FAILURE: Self = Self(1015);
    pub const VALUE_COPY_INVARIANT_FAILURE: Self = Self(1016);
    pub const REGEX_COMPILATION_INVARIANT_FAILURE: Self = Self(1017);
    pub const REGEX_EXECUTION_INVARIANT_FAILURE: Self = Self(1018);
    pub const TYPE_BINDING_INVARIANT_FAILURE: Self = Self(1019);
    pub const RUST_CONVERSION_INVARIANT_FAILURE: Self = Self(1020);
    pub const CONFIG_ENCODING_INVARIANT_FAILURE: Self = Self(1021);
    pub const CONFIG_EDIT_INVARIANT_FAILURE: Self = Self(1022);
    pub const CONFIG_IR_INVARIANT_FAILURE: Self = Self(1023);
    pub const HOST_ADAPTER_INVARIANT_FAILURE: Self = Self(1024);
    pub const DIAGNOSTIC_COLLECTION_INVARIANT_FAILURE: Self = Self(1025);
    pub const DIAGNOSTIC_RENDERING_INVARIANT_FAILURE: Self = Self(1026);
    pub const SOURCE_EXCERPT_INVARIANT_FAILURE: Self = Self(1027);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapacityExceeded {
    BudgetBookkeeping,
    SourceInput,
    DiagnosticPayload,
    SyntaxStorage,
    SchemaStorage,
    ConfigValue,
    ConfigDiagnostic,
    RegexCompilation,
    RegexExecution,
    EnvironmentCache,
    FormatOutput,
    ConfigEdit,
    HostSource,
    RustConversion,
    Codec,
}

impl CapacityExceeded {
    pub const fn from_diagnostic_code(code: DiagnosticCode) -> Option<Self> {
        match code {
            DiagnosticCode::BUDGET_CAPACITY_EXCEEDED => Some(Self::BudgetBookkeeping),
            DiagnosticCode::SOURCE_INPUT_CAPACITY_EXCEEDED => Some(Self::SourceInput),
            DiagnosticCode::DIAGNOSTIC_PAYLOAD_CAPACITY_EXCEEDED => Some(Self::DiagnosticPayload),
            DiagnosticCode::SYNTAX_STORAGE_CAPACITY_EXCEEDED => Some(Self::SyntaxStorage),
            DiagnosticCode::SCHEMA_STORAGE_CAPACITY_EXCEEDED => Some(Self::SchemaStorage),
            DiagnosticCode::CONFIG_VALUE_CAPACITY_EXCEEDED => Some(Self::ConfigValue),
            DiagnosticCode::CONFIG_DIAGNOSTIC_CAPACITY_EXCEEDED => Some(Self::ConfigDiagnostic),
            DiagnosticCode::REGEX_COMPILATION_CAPACITY_EXCEEDED => Some(Self::RegexCompilation),
            DiagnosticCode::REGEX_EXECUTION_CAPACITY_EXCEEDED => Some(Self::RegexExecution),
            DiagnosticCode::ENVIRONMENT_CACHE_CAPACITY_EXCEEDED => Some(Self::EnvironmentCache),
            DiagnosticCode::FORMAT_OUTPUT_CAPACITY_EXCEEDED => Some(Self::FormatOutput),
            DiagnosticCode::CONFIG_EDIT_CAPACITY_EXCEEDED => Some(Self::ConfigEdit),
            DiagnosticCode::HOST_SOURCE_CAPACITY_EXCEEDED => Some(Self::HostSource),
            DiagnosticCode::RUST_CONVERSION_CAPACITY_EXCEEDED => Some(Self::RustConversion),
            DiagnosticCode::CODEC_CAPACITY_EXCEEDED => Some(Self::Codec),
            _ => None,
        }
    }

    pub const fn diagnostic_code(self) -> DiagnosticCode {
        match self {
            Self::BudgetBookkeeping => DiagnosticCode::BUDGET_CAPACITY_EXCEEDED,
            Self::SourceInput => DiagnosticCode::SOURCE_INPUT_CAPACITY_EXCEEDED,
            Self::DiagnosticPayload => DiagnosticCode::DIAGNOSTIC_PAYLOAD_CAPACITY_EXCEEDED,
            Self::SyntaxStorage => DiagnosticCode::SYNTAX_STORAGE_CAPACITY_EXCEEDED,
            Self::SchemaStorage => DiagnosticCode::SCHEMA_STORAGE_CAPACITY_EXCEEDED,
            Self::ConfigValue => DiagnosticCode::CONFIG_VALUE_CAPACITY_EXCEEDED,
            Self::ConfigDiagnostic => DiagnosticCode::CONFIG_DIAGNOSTIC_CAPACITY_EXCEEDED,
            Self::RegexCompilation => DiagnosticCode::REGEX_COMPILATION_CAPACITY_EXCEEDED,
            Self::RegexExecution => DiagnosticCode::REGEX_EXECUTION_CAPACITY_EXCEEDED,
            Self::EnvironmentCache => DiagnosticCode::ENVIRONMENT_CACHE_CAPACITY_EXCEEDED,
            Self::FormatOutput => DiagnosticCode::FORMAT_OUTPUT_CAPACITY_EXCEEDED,
            Self::ConfigEdit => DiagnosticCode::CONFIG_EDIT_CAPACITY_EXCEEDED,
            Self::HostSource => DiagnosticCode::HOST_SOURCE_CAPACITY_EXCEEDED,
            Self::RustConversion => DiagnosticCode::RUST_CONVERSION_CAPACITY_EXCEEDED,
            Self::Codec => DiagnosticCode::CODEC_CAPACITY_EXCEEDED,
        }
    }

    pub fn emergency_message(self, translator: Translator) -> Result<&'static str, RenderError> {
        match self {
            Self::BudgetBookkeeping => t! {
                name: "diagnostic.emergency.budget_capacity_exceeded",
                locale: translator,
            },
            Self::SourceInput => t! {
                name: "diagnostic.emergency.source_input_capacity_exceeded",
                locale: translator,
            },
            Self::DiagnosticPayload => t! {
                name: "diagnostic.emergency.diagnostic_payload_capacity_exceeded",
                locale: translator,
            },
            Self::SyntaxStorage => t! {
                name: "diagnostic.emergency.syntax_storage_capacity_exceeded",
                locale: translator,
            },
            Self::SchemaStorage => t! {
                name: "diagnostic.emergency.schema_storage_capacity_exceeded",
                locale: translator,
            },
            Self::ConfigValue => t! {
                name: "diagnostic.emergency.config_value_capacity_exceeded",
                locale: translator,
            },
            Self::ConfigDiagnostic => t! {
                name: "diagnostic.emergency.config_diagnostic_capacity_exceeded",
                locale: translator,
            },
            Self::RegexCompilation => t! {
                name: "diagnostic.emergency.regex_compilation_capacity_exceeded",
                locale: translator,
            },
            Self::RegexExecution => t! {
                name: "diagnostic.emergency.regex_execution_capacity_exceeded",
                locale: translator,
            },
            Self::EnvironmentCache => t! {
                name: "diagnostic.emergency.environment_cache_capacity_exceeded",
                locale: translator,
            },
            Self::FormatOutput => t! {
                name: "diagnostic.emergency.format_output_capacity_exceeded",
                locale: translator,
            },
            Self::ConfigEdit => t! {
                name: "diagnostic.emergency.config_edit_capacity_exceeded",
                locale: translator,
            },
            Self::HostSource => t! {
                name: "diagnostic.emergency.host_source_capacity_exceeded",
                locale: translator,
            },
            Self::RustConversion => t! {
                name: "diagnostic.emergency.rust_conversion_capacity_exceeded",
                locale: translator,
            },
            Self::Codec => t! {
                name: "diagnostic.emergency.codec_capacity_exceeded",
                locale: translator,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InternalError {
    BudgetInvariantFailure(DiagnosticContext),
    SourceInputInvariantFailure(DiagnosticContext),
    SourceLocationInvariantFailure(DiagnosticContext),
    ValueStructureInvariantFailure(DiagnosticContext),
    SyntaxStorageInvariantFailure(DiagnosticContext),
    SchemaAstAccessInvariantFailure(DiagnosticContext),
    SchemaAccessInvariantFailure(DiagnosticContext),
    SchemaResolverInvariantFailure(DiagnosticContext),
    SchemaCompilationInvariantFailure(DiagnosticContext),
    ConfigParserInvariantFailure(DiagnosticContext),
    ConfigValueInvariantFailure(DiagnosticContext),
    ConfigValidationInvariantFailure(DiagnosticContext),
    EnvironmentInvariantFailure(DiagnosticContext),
    SecretPolicyInvariantFailure(DiagnosticContext),
    SemanticEvaluationInvariantFailure(DiagnosticContext),
    DefaultConstructionInvariantFailure(DiagnosticContext),
    ValueCopyInvariantFailure(DiagnosticContext),
    RegexCompilationInvariantFailure(DiagnosticContext),
    RegexExecutionInvariantFailure(DiagnosticContext),
    TypeBindingInvariantFailure(DiagnosticContext),
    RustConversionInvariantFailure(DiagnosticContext),
    ConfigEncodingInvariantFailure(DiagnosticContext),
    ConfigEditInvariantFailure(DiagnosticContext),
    ConfigIrInvariantFailure(DiagnosticContext),
    HostAdapterInvariantFailure(DiagnosticContext),
    DiagnosticCollectionInvariantFailure(DiagnosticContext),
    DiagnosticRenderingInvariantFailure(DiagnosticContext),
    SourceExcerptInvariantFailure(DiagnosticContext),
}

impl InternalError {
    pub(crate) const fn tag(self) -> usize {
        match self {
            Self::BudgetInvariantFailure(_) => 0,
            Self::SourceInputInvariantFailure(_) => 1,
            Self::SourceLocationInvariantFailure(_) => 2,
            Self::ValueStructureInvariantFailure(_) => 3,
            Self::SyntaxStorageInvariantFailure(_) => 4,
            Self::SchemaAstAccessInvariantFailure(_) => 5,
            Self::SchemaAccessInvariantFailure(_) => 6,
            Self::SchemaResolverInvariantFailure(_) => 7,
            Self::SchemaCompilationInvariantFailure(_) => 8,
            Self::ConfigParserInvariantFailure(_) => 9,
            Self::ConfigValueInvariantFailure(_) => 10,
            Self::ConfigValidationInvariantFailure(_) => 11,
            Self::EnvironmentInvariantFailure(_) => 12,
            Self::SecretPolicyInvariantFailure(_) => 13,
            Self::SemanticEvaluationInvariantFailure(_) => 14,
            Self::DefaultConstructionInvariantFailure(_) => 15,
            Self::ValueCopyInvariantFailure(_) => 16,
            Self::RegexCompilationInvariantFailure(_) => 17,
            Self::RegexExecutionInvariantFailure(_) => 18,
            Self::TypeBindingInvariantFailure(_) => 19,
            Self::RustConversionInvariantFailure(_) => 20,
            Self::ConfigEncodingInvariantFailure(_) => 21,
            Self::ConfigEditInvariantFailure(_) => 22,
            Self::ConfigIrInvariantFailure(_) => 23,
            Self::HostAdapterInvariantFailure(_) => 24,
            Self::DiagnosticCollectionInvariantFailure(_) => 25,
            Self::DiagnosticRenderingInvariantFailure(_) => 26,
            Self::SourceExcerptInvariantFailure(_) => 27,
        }
    }

    pub(crate) const fn from_tag(tag: usize, context: DiagnosticContext) -> Option<Self> {
        match tag {
            0 => Some(Self::BudgetInvariantFailure(context)),
            1 => Some(Self::SourceInputInvariantFailure(context)),
            2 => Some(Self::SourceLocationInvariantFailure(context)),
            3 => Some(Self::ValueStructureInvariantFailure(context)),
            4 => Some(Self::SyntaxStorageInvariantFailure(context)),
            5 => Some(Self::SchemaAstAccessInvariantFailure(context)),
            6 => Some(Self::SchemaAccessInvariantFailure(context)),
            7 => Some(Self::SchemaResolverInvariantFailure(context)),
            8 => Some(Self::SchemaCompilationInvariantFailure(context)),
            9 => Some(Self::ConfigParserInvariantFailure(context)),
            10 => Some(Self::ConfigValueInvariantFailure(context)),
            11 => Some(Self::ConfigValidationInvariantFailure(context)),
            12 => Some(Self::EnvironmentInvariantFailure(context)),
            13 => Some(Self::SecretPolicyInvariantFailure(context)),
            14 => Some(Self::SemanticEvaluationInvariantFailure(context)),
            15 => Some(Self::DefaultConstructionInvariantFailure(context)),
            16 => Some(Self::ValueCopyInvariantFailure(context)),
            17 => Some(Self::RegexCompilationInvariantFailure(context)),
            18 => Some(Self::RegexExecutionInvariantFailure(context)),
            19 => Some(Self::TypeBindingInvariantFailure(context)),
            20 => Some(Self::RustConversionInvariantFailure(context)),
            21 => Some(Self::ConfigEncodingInvariantFailure(context)),
            22 => Some(Self::ConfigEditInvariantFailure(context)),
            23 => Some(Self::ConfigIrInvariantFailure(context)),
            24 => Some(Self::HostAdapterInvariantFailure(context)),
            25 => Some(Self::DiagnosticCollectionInvariantFailure(context)),
            26 => Some(Self::DiagnosticRenderingInvariantFailure(context)),
            27 => Some(Self::SourceExcerptInvariantFailure(context)),
            _ => None,
        }
    }

    pub const fn from_diagnostic_code(
        code: DiagnosticCode,
        context: DiagnosticContext,
    ) -> Option<Self> {
        let Some(tag) = code.get().checked_sub(1000) else {
            return None;
        };
        if tag > 27 {
            return None;
        }
        Self::from_tag(tag as usize, context)
    }

    pub const fn context(self) -> DiagnosticContext {
        match self {
            Self::BudgetInvariantFailure(context) => context,
            Self::SourceInputInvariantFailure(context)
            | Self::SourceLocationInvariantFailure(context)
            | Self::ValueStructureInvariantFailure(context)
            | Self::SyntaxStorageInvariantFailure(context)
            | Self::SchemaAstAccessInvariantFailure(context)
            | Self::SchemaAccessInvariantFailure(context)
            | Self::SchemaResolverInvariantFailure(context)
            | Self::SchemaCompilationInvariantFailure(context)
            | Self::ConfigParserInvariantFailure(context)
            | Self::ConfigValueInvariantFailure(context)
            | Self::ConfigValidationInvariantFailure(context)
            | Self::EnvironmentInvariantFailure(context)
            | Self::SecretPolicyInvariantFailure(context)
            | Self::SemanticEvaluationInvariantFailure(context)
            | Self::DefaultConstructionInvariantFailure(context)
            | Self::ValueCopyInvariantFailure(context)
            | Self::RegexCompilationInvariantFailure(context)
            | Self::RegexExecutionInvariantFailure(context)
            | Self::TypeBindingInvariantFailure(context)
            | Self::RustConversionInvariantFailure(context)
            | Self::ConfigEncodingInvariantFailure(context)
            | Self::ConfigEditInvariantFailure(context)
            | Self::ConfigIrInvariantFailure(context)
            | Self::HostAdapterInvariantFailure(context)
            | Self::DiagnosticCollectionInvariantFailure(context)
            | Self::DiagnosticRenderingInvariantFailure(context)
            | Self::SourceExcerptInvariantFailure(context) => context,
        }
    }

    pub const fn diagnostic_code(self) -> DiagnosticCode {
        match self {
            Self::BudgetInvariantFailure(_) => DiagnosticCode::BUDGET_INVARIANT_FAILURE,
            Self::SourceInputInvariantFailure(_) => DiagnosticCode::SOURCE_INPUT_INVARIANT_FAILURE,
            Self::SourceLocationInvariantFailure(_) => {
                DiagnosticCode::SOURCE_LOCATION_INVARIANT_FAILURE
            }
            Self::ValueStructureInvariantFailure(_) => {
                DiagnosticCode::VALUE_STRUCTURE_INVARIANT_FAILURE
            }
            Self::SyntaxStorageInvariantFailure(_) => {
                DiagnosticCode::SYNTAX_STORAGE_INVARIANT_FAILURE
            }
            Self::SchemaAstAccessInvariantFailure(_) => {
                DiagnosticCode::SCHEMA_AST_ACCESS_INVARIANT_FAILURE
            }
            Self::SchemaAccessInvariantFailure(_) => {
                DiagnosticCode::SCHEMA_ACCESS_INVARIANT_FAILURE
            }
            Self::SchemaResolverInvariantFailure(_) => {
                DiagnosticCode::SCHEMA_RESOLVER_INVARIANT_FAILURE
            }
            Self::SchemaCompilationInvariantFailure(_) => {
                DiagnosticCode::SCHEMA_COMPILATION_INVARIANT_FAILURE
            }
            Self::ConfigParserInvariantFailure(_) => {
                DiagnosticCode::CONFIG_PARSER_INVARIANT_FAILURE
            }
            Self::ConfigValueInvariantFailure(_) => DiagnosticCode::CONFIG_VALUE_INVARIANT_FAILURE,
            Self::ConfigValidationInvariantFailure(_) => {
                DiagnosticCode::CONFIG_VALIDATION_INVARIANT_FAILURE
            }
            Self::EnvironmentInvariantFailure(_) => DiagnosticCode::ENVIRONMENT_INVARIANT_FAILURE,
            Self::SecretPolicyInvariantFailure(_) => {
                DiagnosticCode::SECRET_POLICY_INVARIANT_FAILURE
            }
            Self::SemanticEvaluationInvariantFailure(_) => {
                DiagnosticCode::SEMANTIC_EVALUATION_INVARIANT_FAILURE
            }
            Self::DefaultConstructionInvariantFailure(_) => {
                DiagnosticCode::DEFAULT_CONSTRUCTION_INVARIANT_FAILURE
            }
            Self::ValueCopyInvariantFailure(_) => DiagnosticCode::VALUE_COPY_INVARIANT_FAILURE,
            Self::RegexCompilationInvariantFailure(_) => {
                DiagnosticCode::REGEX_COMPILATION_INVARIANT_FAILURE
            }
            Self::RegexExecutionInvariantFailure(_) => {
                DiagnosticCode::REGEX_EXECUTION_INVARIANT_FAILURE
            }
            Self::TypeBindingInvariantFailure(_) => DiagnosticCode::TYPE_BINDING_INVARIANT_FAILURE,
            Self::RustConversionInvariantFailure(_) => {
                DiagnosticCode::RUST_CONVERSION_INVARIANT_FAILURE
            }
            Self::ConfigEncodingInvariantFailure(_) => {
                DiagnosticCode::CONFIG_ENCODING_INVARIANT_FAILURE
            }
            Self::ConfigEditInvariantFailure(_) => DiagnosticCode::CONFIG_EDIT_INVARIANT_FAILURE,
            Self::ConfigIrInvariantFailure(_) => DiagnosticCode::CONFIG_IR_INVARIANT_FAILURE,
            Self::HostAdapterInvariantFailure(_) => DiagnosticCode::HOST_ADAPTER_INVARIANT_FAILURE,
            Self::DiagnosticCollectionInvariantFailure(_) => {
                DiagnosticCode::DIAGNOSTIC_COLLECTION_INVARIANT_FAILURE
            }
            Self::DiagnosticRenderingInvariantFailure(_) => {
                DiagnosticCode::DIAGNOSTIC_RENDERING_INVARIANT_FAILURE
            }
            Self::SourceExcerptInvariantFailure(_) => {
                DiagnosticCode::SOURCE_EXCERPT_INVARIANT_FAILURE
            }
        }
    }

    pub fn emergency_message(self, translator: Translator) -> Result<&'static str, RenderError> {
        match self {
            Self::BudgetInvariantFailure(_) => t! {
                name: "diagnostic.emergency.budget_invariant_failure",
                locale: translator,
            },
            Self::SourceInputInvariantFailure(_) => t! {
                name: "diagnostic.emergency.source_input_invariant_failure",
                locale: translator,
            },
            Self::SourceLocationInvariantFailure(_) => t! {
                name: "diagnostic.emergency.source_location_invariant_failure",
                locale: translator,
            },
            Self::ValueStructureInvariantFailure(_) => t! {
                name: "diagnostic.emergency.value_structure_invariant_failure",
                locale: translator,
            },
            Self::SyntaxStorageInvariantFailure(_) => t! {
                name: "diagnostic.emergency.syntax_storage_invariant_failure",
                locale: translator,
            },
            Self::SchemaAstAccessInvariantFailure(_) => t! {
                name: "diagnostic.emergency.schema_ast_access_invariant_failure",
                locale: translator,
            },
            Self::SchemaAccessInvariantFailure(_) => t! {
                name: "diagnostic.emergency.schema_access_invariant_failure",
                locale: translator,
            },
            Self::SchemaResolverInvariantFailure(_) => t! {
                name: "diagnostic.emergency.schema_resolver_invariant_failure",
                locale: translator,
            },
            Self::SchemaCompilationInvariantFailure(_) => t! {
                name: "diagnostic.emergency.schema_compilation_invariant_failure",
                locale: translator,
            },
            Self::ConfigParserInvariantFailure(_) => t! {
                name: "diagnostic.emergency.config_parser_invariant_failure",
                locale: translator,
            },
            Self::ConfigValueInvariantFailure(_) => t! {
                name: "diagnostic.emergency.config_value_invariant_failure",
                locale: translator,
            },
            Self::ConfigValidationInvariantFailure(_) => t! {
                name: "diagnostic.emergency.config_validation_invariant_failure",
                locale: translator,
            },
            Self::EnvironmentInvariantFailure(_) => t! {
                name: "diagnostic.emergency.environment_invariant_failure",
                locale: translator,
            },
            Self::SecretPolicyInvariantFailure(_) => t! {
                name: "diagnostic.emergency.secret_policy_invariant_failure",
                locale: translator,
            },
            Self::SemanticEvaluationInvariantFailure(_) => t! {
                name: "diagnostic.emergency.semantic_evaluation_invariant_failure",
                locale: translator,
            },
            Self::DefaultConstructionInvariantFailure(_) => t! {
                name: "diagnostic.emergency.default_construction_invariant_failure",
                locale: translator,
            },
            Self::ValueCopyInvariantFailure(_) => t! {
                name: "diagnostic.emergency.value_copy_invariant_failure",
                locale: translator,
            },
            Self::RegexCompilationInvariantFailure(_) => t! {
                name: "diagnostic.emergency.regex_compilation_invariant_failure",
                locale: translator,
            },
            Self::RegexExecutionInvariantFailure(_) => t! {
                name: "diagnostic.emergency.regex_execution_invariant_failure",
                locale: translator,
            },
            Self::TypeBindingInvariantFailure(_) => t! {
                name: "diagnostic.emergency.type_binding_invariant_failure",
                locale: translator,
            },
            Self::RustConversionInvariantFailure(_) => t! {
                name: "diagnostic.emergency.rust_conversion_invariant_failure",
                locale: translator,
            },
            Self::ConfigEncodingInvariantFailure(_) => t! {
                name: "diagnostic.emergency.config_encoding_invariant_failure",
                locale: translator,
            },
            Self::ConfigEditInvariantFailure(_) => t! {
                name: "diagnostic.emergency.config_edit_invariant_failure",
                locale: translator,
            },
            Self::ConfigIrInvariantFailure(_) => t! {
                name: "diagnostic.emergency.config_ir_invariant_failure",
                locale: translator,
            },
            Self::HostAdapterInvariantFailure(_) => t! {
                name: "diagnostic.emergency.host_adapter_invariant_failure",
                locale: translator,
            },
            Self::DiagnosticCollectionInvariantFailure(_) => t! {
                name: "diagnostic.emergency.diagnostic_collection_invariant_failure",
                locale: translator,
            },
            Self::DiagnosticRenderingInvariantFailure(_) => t! {
                name: "diagnostic.emergency.diagnostic_rendering_invariant_failure",
                locale: translator,
            },
            Self::SourceExcerptInvariantFailure(_) => t! {
                name: "diagnostic.emergency.source_excerpt_invariant_failure",
                locale: translator,
            },
        }
    }
}

impl DiagnosticCode {
    pub const fn new(value: u32) -> Option<Self> {
        if value == 0 { None } else { Some(Self(value)) }
    }

    pub const fn get(self) -> u32 {
        self.0
    }

    pub const fn is_resource_limit(self) -> bool {
        self.0 >= Self::SOURCE_BYTES_LIMIT_EXCEEDED.0
            && self.0 <= Self::TYPE_BINDING_NODES_LIMIT_EXCEEDED.0
    }

    pub const fn is_capacity_exceeded(self) -> bool {
        self.0 >= Self::BUDGET_CAPACITY_EXCEEDED.0 && self.0 <= Self::CODEC_CAPACITY_EXCEEDED.0
    }

    pub const fn is_internal_error(self) -> bool {
        self.0 >= Self::BUDGET_INVARIANT_FAILURE.0
            && self.0 <= Self::SOURCE_EXCERPT_INVARIANT_FAILURE.0
    }

    pub const fn name(self) -> &'static str {
        match self.0 {
            2 => "ALLOCATION_FAILED",
            3 => "LEXICAL_ERROR",
            4 => "SCHEMA_SYNTAX_ERROR",
            5 => "DIAGNOSTIC_LIMIT_REACHED",
            6 => "REGEX_SYNTAX_ERROR",
            7 => "SCHEMA_SEMANTIC_ERROR",
            8 => "SCHEMA_RESOLVER_ERROR",
            10 => "CONFIG_SYNTAX_ERROR",
            11 => "CONFIG_VALUE_ERROR",
            12 => "CONFIG_VALIDATION_ERROR",
            13 => "CONFIG_ENVIRONMENT_ERROR",
            14 => "TYPE_BINDING_ERROR",
            15 => "RUST_CONVERSION_ERROR",
            16 => "CONFIG_ENCODING_ERROR",
            17 => "SOURCE_INPUT_ERROR",
            19 => "SCHEMA_CONTEXT_MISMATCH",
            20 => "SCHEMA_SESSION_FINISHED",
            21 => "SOURCE_BYTES_LIMIT_EXCEEDED",
            22 => "SCHEMA_TOTAL_BYTES_LIMIT_EXCEEDED",
            23 => "SCHEMA_SOURCE_COUNT_LIMIT_EXCEEDED",
            24 => "SCHEMA_IMPORT_DEPTH_LIMIT_EXCEEDED",
            25 => "EXECUTION_STEPS_LIMIT_EXCEEDED",
            26 => "WORKSPACE_BYTES_LIMIT_EXCEEDED",
            27 => "SCHEMA_PARSE_DEPTH_LIMIT_EXCEEDED",
            28 => "TOKEN_BYTES_LIMIT_EXCEEDED",
            29 => "STRING_BYTES_LIMIT_EXCEEDED",
            30 => "SCHEMA_AST_NODES_LIMIT_EXCEEDED",
            31 => "CONFIG_PARSE_DEPTH_LIMIT_EXCEEDED",
            32 => "CONFIG_VALUE_DEPTH_LIMIT_EXCEEDED",
            33 => "CONFIG_VALUE_NODES_LIMIT_EXCEEDED",
            34 => "CONFIG_TEXT_BYTES_LIMIT_EXCEEDED",
            35 => "ENVIRONMENT_ENTRIES_LIMIT_EXCEEDED",
            36 => "ENVIRONMENT_BYTES_LIMIT_EXCEEDED",
            37 => "VALIDATION_TRAVERSAL_LIMIT_EXCEEDED",
            38 => "REGEX_COMPILATION_BUDGET_EXCEEDED",
            39 => "REGEX_PATTERN_BYTES_LIMIT_EXCEEDED",
            40 => "REGEX_NESTING_DEPTH_LIMIT_EXCEEDED",
            41 => "REGEX_INSTRUCTION_COUNT_LIMIT_EXCEEDED",
            42 => "REGEX_CHARACTER_RANGES_LIMIT_EXCEEDED",
            43 => "REGEX_EXECUTION_STEPS_LIMIT_EXCEEDED",
            44 => "REGEX_SEARCH_STEPS_LIMIT_EXCEEDED",
            45 => "REGEX_WORKSPACE_BYTES_LIMIT_EXCEEDED",
            46 => "REGEX_SCRATCH_STATES_LIMIT_EXCEEDED",
            47 => "TYPE_BINDING_DEPTH_LIMIT_EXCEEDED",
            48 => "SCHEMA_FORMAT_OUTPUT_BYTES_LIMIT_EXCEEDED",
            49 => "CONFIG_FORMAT_OUTPUT_BYTES_LIMIT_EXCEEDED",
            50 => "FORMATTING_WORKSPACE_BYTES_LIMIT_EXCEEDED",
            51 => "FORMATTING_NESTING_DEPTH_LIMIT_EXCEEDED",
            52 => "TYPE_BINDING_NODES_LIMIT_EXCEEDED",
            70 => "BUDGET_CAPACITY_EXCEEDED",
            71 => "SOURCE_INPUT_CAPACITY_EXCEEDED",
            72 => "DIAGNOSTIC_PAYLOAD_CAPACITY_EXCEEDED",
            73 => "SYNTAX_STORAGE_CAPACITY_EXCEEDED",
            74 => "SCHEMA_STORAGE_CAPACITY_EXCEEDED",
            75 => "CONFIG_VALUE_CAPACITY_EXCEEDED",
            76 => "CONFIG_DIAGNOSTIC_CAPACITY_EXCEEDED",
            77 => "REGEX_COMPILATION_CAPACITY_EXCEEDED",
            78 => "REGEX_EXECUTION_CAPACITY_EXCEEDED",
            79 => "ENVIRONMENT_CACHE_CAPACITY_EXCEEDED",
            80 => "FORMAT_OUTPUT_CAPACITY_EXCEEDED",
            81 => "CONFIG_EDIT_CAPACITY_EXCEEDED",
            82 => "HOST_SOURCE_CAPACITY_EXCEEDED",
            83 => "RUST_CONVERSION_CAPACITY_EXCEEDED",
            84 => "CODEC_CAPACITY_EXCEEDED",
            1000 => "BUDGET_INVARIANT_FAILURE",
            1001 => "SOURCE_INPUT_INVARIANT_FAILURE",
            1002 => "SOURCE_LOCATION_INVARIANT_FAILURE",
            1003 => "VALUE_STRUCTURE_INVARIANT_FAILURE",
            1004 => "SYNTAX_STORAGE_INVARIANT_FAILURE",
            1005 => "SCHEMA_AST_ACCESS_INVARIANT_FAILURE",
            1006 => "SCHEMA_ACCESS_INVARIANT_FAILURE",
            1007 => "SCHEMA_RESOLVER_INVARIANT_FAILURE",
            1008 => "SCHEMA_COMPILATION_INVARIANT_FAILURE",
            1009 => "CONFIG_PARSER_INVARIANT_FAILURE",
            1010 => "CONFIG_VALUE_INVARIANT_FAILURE",
            1011 => "CONFIG_VALIDATION_INVARIANT_FAILURE",
            1012 => "ENVIRONMENT_INVARIANT_FAILURE",
            1013 => "SECRET_POLICY_INVARIANT_FAILURE",
            1014 => "SEMANTIC_EVALUATION_INVARIANT_FAILURE",
            1015 => "DEFAULT_CONSTRUCTION_INVARIANT_FAILURE",
            1016 => "VALUE_COPY_INVARIANT_FAILURE",
            1017 => "REGEX_COMPILATION_INVARIANT_FAILURE",
            1018 => "REGEX_EXECUTION_INVARIANT_FAILURE",
            1019 => "TYPE_BINDING_INVARIANT_FAILURE",
            1020 => "RUST_CONVERSION_INVARIANT_FAILURE",
            1021 => "CONFIG_ENCODING_INVARIANT_FAILURE",
            1022 => "CONFIG_EDIT_INVARIANT_FAILURE",
            1023 => "CONFIG_IR_INVARIANT_FAILURE",
            1024 => "HOST_ADAPTER_INVARIANT_FAILURE",
            1025 => "DIAGNOSTIC_COLLECTION_INVARIANT_FAILURE",
            1026 => "DIAGNOSTIC_RENDERING_INVARIANT_FAILURE",
            1027 => "SOURCE_EXCERPT_INVARIANT_FAILURE",
            _ => "UNKNOWN",
        }
    }
}

impl fmt::Debug for DiagnosticCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("DiagnosticCode")
            .field(&self.0)
            .finish()
    }
}
