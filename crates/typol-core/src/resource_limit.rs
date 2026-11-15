use core::fmt;
use typol_i18n::{Translator, t};
use u18n::RenderError;

use crate::DiagnosticCode;

/// Identifies the specific configured or accounting limit that was exceeded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResourceLimit {
    SourceBytes,
    SchemaTotalBytes,
    SchemaSourceCount,
    SchemaImportDepth,
    ExecutionSteps,
    WorkspaceBytes,
    SchemaParseDepth,
    TokenBytes,
    StringBytes,
    SchemaAstNodes,
    ConfigParseDepth,
    ConfigValueDepth,
    ConfigValueNodes,
    ConfigTextBytes,
    EnvironmentEntries,
    EnvironmentBytes,
    ValidationTraversal,
    RegexCompilationBudget,
    RegexPatternBytes,
    RegexNestingDepth,
    RegexInstructionCount,
    RegexCharacterRanges,
    RegexExecutionSteps,
    RegexSearchSteps,
    RegexWorkspaceBytes,
    RegexScratchStates,
    TypeBindingDepth,
    SchemaFormatOutputBytes,
    ConfigFormatOutputBytes,
    FormattingWorkspaceBytes,
    FormattingNestingDepth,
    TypeBindingNodes,
}

impl ResourceLimit {
    pub const fn from_diagnostic_code(code: DiagnosticCode) -> Option<Self> {
        match code.get() {
            21 => Some(Self::SourceBytes),
            22 => Some(Self::SchemaTotalBytes),
            23 => Some(Self::SchemaSourceCount),
            24 => Some(Self::SchemaImportDepth),
            25 => Some(Self::ExecutionSteps),
            26 => Some(Self::WorkspaceBytes),
            27 => Some(Self::SchemaParseDepth),
            28 => Some(Self::TokenBytes),
            29 => Some(Self::StringBytes),
            30 => Some(Self::SchemaAstNodes),
            31 => Some(Self::ConfigParseDepth),
            32 => Some(Self::ConfigValueDepth),
            33 => Some(Self::ConfigValueNodes),
            34 => Some(Self::ConfigTextBytes),
            35 => Some(Self::EnvironmentEntries),
            36 => Some(Self::EnvironmentBytes),
            37 => Some(Self::ValidationTraversal),
            38 => Some(Self::RegexCompilationBudget),
            39 => Some(Self::RegexPatternBytes),
            40 => Some(Self::RegexNestingDepth),
            41 => Some(Self::RegexInstructionCount),
            42 => Some(Self::RegexCharacterRanges),
            43 => Some(Self::RegexExecutionSteps),
            44 => Some(Self::RegexSearchSteps),
            45 => Some(Self::RegexWorkspaceBytes),
            46 => Some(Self::RegexScratchStates),
            47 => Some(Self::TypeBindingDepth),
            48 => Some(Self::SchemaFormatOutputBytes),
            49 => Some(Self::ConfigFormatOutputBytes),
            50 => Some(Self::FormattingWorkspaceBytes),
            51 => Some(Self::FormattingNestingDepth),
            52 => Some(Self::TypeBindingNodes),
            _ => None,
        }
    }

    pub const fn diagnostic_code(self) -> DiagnosticCode {
        match self {
            Self::SourceBytes => DiagnosticCode::SOURCE_BYTES_LIMIT_EXCEEDED,
            Self::SchemaTotalBytes => DiagnosticCode::SCHEMA_TOTAL_BYTES_LIMIT_EXCEEDED,
            Self::SchemaSourceCount => DiagnosticCode::SCHEMA_SOURCE_COUNT_LIMIT_EXCEEDED,
            Self::SchemaImportDepth => DiagnosticCode::SCHEMA_IMPORT_DEPTH_LIMIT_EXCEEDED,
            Self::ExecutionSteps => DiagnosticCode::EXECUTION_STEPS_LIMIT_EXCEEDED,
            Self::WorkspaceBytes => DiagnosticCode::WORKSPACE_BYTES_LIMIT_EXCEEDED,
            Self::SchemaParseDepth => DiagnosticCode::SCHEMA_PARSE_DEPTH_LIMIT_EXCEEDED,
            Self::TokenBytes => DiagnosticCode::TOKEN_BYTES_LIMIT_EXCEEDED,
            Self::StringBytes => DiagnosticCode::STRING_BYTES_LIMIT_EXCEEDED,
            Self::SchemaAstNodes => DiagnosticCode::SCHEMA_AST_NODES_LIMIT_EXCEEDED,
            Self::ConfigParseDepth => DiagnosticCode::CONFIG_PARSE_DEPTH_LIMIT_EXCEEDED,
            Self::ConfigValueDepth => DiagnosticCode::CONFIG_VALUE_DEPTH_LIMIT_EXCEEDED,
            Self::ConfigValueNodes => DiagnosticCode::CONFIG_VALUE_NODES_LIMIT_EXCEEDED,
            Self::ConfigTextBytes => DiagnosticCode::CONFIG_TEXT_BYTES_LIMIT_EXCEEDED,
            Self::EnvironmentEntries => DiagnosticCode::ENVIRONMENT_ENTRIES_LIMIT_EXCEEDED,
            Self::EnvironmentBytes => DiagnosticCode::ENVIRONMENT_BYTES_LIMIT_EXCEEDED,
            Self::ValidationTraversal => DiagnosticCode::VALIDATION_TRAVERSAL_LIMIT_EXCEEDED,
            Self::RegexCompilationBudget => DiagnosticCode::REGEX_COMPILATION_BUDGET_EXCEEDED,
            Self::RegexPatternBytes => DiagnosticCode::REGEX_PATTERN_BYTES_LIMIT_EXCEEDED,
            Self::RegexNestingDepth => DiagnosticCode::REGEX_NESTING_DEPTH_LIMIT_EXCEEDED,
            Self::RegexInstructionCount => DiagnosticCode::REGEX_INSTRUCTION_COUNT_LIMIT_EXCEEDED,
            Self::RegexCharacterRanges => DiagnosticCode::REGEX_CHARACTER_RANGES_LIMIT_EXCEEDED,
            Self::RegexExecutionSteps => DiagnosticCode::REGEX_EXECUTION_STEPS_LIMIT_EXCEEDED,
            Self::RegexSearchSteps => DiagnosticCode::REGEX_SEARCH_STEPS_LIMIT_EXCEEDED,
            Self::RegexWorkspaceBytes => DiagnosticCode::REGEX_WORKSPACE_BYTES_LIMIT_EXCEEDED,
            Self::RegexScratchStates => DiagnosticCode::REGEX_SCRATCH_STATES_LIMIT_EXCEEDED,
            Self::TypeBindingDepth => DiagnosticCode::TYPE_BINDING_DEPTH_LIMIT_EXCEEDED,
            Self::SchemaFormatOutputBytes => {
                DiagnosticCode::SCHEMA_FORMAT_OUTPUT_BYTES_LIMIT_EXCEEDED
            }
            Self::ConfigFormatOutputBytes => {
                DiagnosticCode::CONFIG_FORMAT_OUTPUT_BYTES_LIMIT_EXCEEDED
            }
            Self::FormattingWorkspaceBytes => {
                DiagnosticCode::FORMATTING_WORKSPACE_BYTES_LIMIT_EXCEEDED
            }
            Self::FormattingNestingDepth => DiagnosticCode::FORMATTING_NESTING_DEPTH_LIMIT_EXCEEDED,
            Self::TypeBindingNodes => DiagnosticCode::TYPE_BINDING_NODES_LIMIT_EXCEEDED,
        }
    }

    /// Returns the localized static emergency message, preserving translation failures.
    pub fn emergency_message(self, translator: Translator) -> Result<&'static str, RenderError> {
        match self {
            Self::SourceBytes => {
                t! { name: "diagnostic.emergency.source_bytes_limit_exceeded", locale: translator }
            }
            Self::SchemaTotalBytes => {
                t! { name: "diagnostic.emergency.schema_total_bytes_limit_exceeded", locale: translator }
            }
            Self::SchemaSourceCount => {
                t! { name: "diagnostic.emergency.schema_source_count_limit_exceeded", locale: translator }
            }
            Self::SchemaImportDepth => {
                t! { name: "diagnostic.emergency.schema_import_depth_limit_exceeded", locale: translator }
            }
            Self::ExecutionSteps => {
                t! { name: "diagnostic.emergency.execution_steps_limit_exceeded", locale: translator }
            }
            Self::WorkspaceBytes => {
                t! { name: "diagnostic.emergency.workspace_bytes_limit_exceeded", locale: translator }
            }
            Self::SchemaParseDepth => {
                t! { name: "diagnostic.emergency.schema_parse_depth_limit_exceeded", locale: translator }
            }
            Self::TokenBytes => {
                t! { name: "diagnostic.emergency.token_bytes_limit_exceeded", locale: translator }
            }
            Self::StringBytes => {
                t! { name: "diagnostic.emergency.string_bytes_limit_exceeded", locale: translator }
            }
            Self::SchemaAstNodes => {
                t! { name: "diagnostic.emergency.schema_ast_nodes_limit_exceeded", locale: translator }
            }
            Self::ConfigParseDepth => {
                t! { name: "diagnostic.emergency.config_parse_depth_limit_exceeded", locale: translator }
            }
            Self::ConfigValueDepth => {
                t! { name: "diagnostic.emergency.config_value_depth_limit_exceeded", locale: translator }
            }
            Self::ConfigValueNodes => {
                t! { name: "diagnostic.emergency.config_value_nodes_limit_exceeded", locale: translator }
            }
            Self::ConfigTextBytes => {
                t! { name: "diagnostic.emergency.config_text_bytes_limit_exceeded", locale: translator }
            }
            Self::EnvironmentEntries => {
                t! { name: "diagnostic.emergency.environment_entries_limit_exceeded", locale: translator }
            }
            Self::EnvironmentBytes => {
                t! { name: "diagnostic.emergency.environment_bytes_limit_exceeded", locale: translator }
            }
            Self::ValidationTraversal => {
                t! { name: "diagnostic.emergency.validation_traversal_limit_exceeded", locale: translator }
            }
            Self::RegexCompilationBudget => {
                t! { name: "diagnostic.emergency.regex_compilation_budget_exceeded", locale: translator }
            }
            Self::RegexPatternBytes => {
                t! { name: "diagnostic.emergency.regex_pattern_bytes_limit_exceeded", locale: translator }
            }
            Self::RegexNestingDepth => {
                t! { name: "diagnostic.emergency.regex_nesting_depth_limit_exceeded", locale: translator }
            }
            Self::RegexInstructionCount => {
                t! { name: "diagnostic.emergency.regex_instruction_count_limit_exceeded", locale: translator }
            }
            Self::RegexCharacterRanges => {
                t! { name: "diagnostic.emergency.regex_character_ranges_limit_exceeded", locale: translator }
            }
            Self::RegexExecutionSteps => {
                t! { name: "diagnostic.emergency.regex_execution_steps_limit_exceeded", locale: translator }
            }
            Self::RegexSearchSteps => {
                t! { name: "diagnostic.emergency.regex_search_steps_limit_exceeded", locale: translator }
            }
            Self::RegexWorkspaceBytes => {
                t! { name: "diagnostic.emergency.regex_workspace_bytes_limit_exceeded", locale: translator }
            }
            Self::RegexScratchStates => {
                t! { name: "diagnostic.emergency.regex_scratch_states_limit_exceeded", locale: translator }
            }
            Self::TypeBindingDepth => {
                t! { name: "diagnostic.emergency.type_binding_depth_limit_exceeded", locale: translator }
            }
            Self::SchemaFormatOutputBytes => {
                t! { name: "diagnostic.emergency.schema_format_output_bytes_limit_exceeded", locale: translator }
            }
            Self::ConfigFormatOutputBytes => {
                t! { name: "diagnostic.emergency.config_format_output_bytes_limit_exceeded", locale: translator }
            }
            Self::FormattingWorkspaceBytes => {
                t! { name: "diagnostic.emergency.formatting_workspace_bytes_limit_exceeded", locale: translator }
            }
            Self::FormattingNestingDepth => {
                t! { name: "diagnostic.emergency.formatting_nesting_depth_limit_exceeded", locale: translator }
            }
            Self::TypeBindingNodes => {
                t! { name: "diagnostic.emergency.type_binding_nodes_limit_exceeded", locale: translator }
            }
        }
    }

    pub(crate) const fn id(self) -> usize {
        match self {
            Self::SourceBytes => 1,
            Self::SchemaTotalBytes => 2,
            Self::SchemaSourceCount => 3,
            Self::SchemaImportDepth => 4,
            Self::ExecutionSteps => 5,
            Self::WorkspaceBytes => 6,
            Self::SchemaParseDepth => 7,
            Self::TokenBytes => 8,
            Self::StringBytes => 9,
            Self::SchemaAstNodes => 10,
            Self::ConfigParseDepth => 11,
            Self::ConfigValueDepth => 12,
            Self::ConfigValueNodes => 13,
            Self::ConfigTextBytes => 14,
            Self::EnvironmentEntries => 15,
            Self::EnvironmentBytes => 16,
            Self::ValidationTraversal => 17,
            Self::RegexCompilationBudget => 18,
            Self::RegexPatternBytes => 19,
            Self::RegexNestingDepth => 20,
            Self::RegexInstructionCount => 21,
            Self::RegexCharacterRanges => 22,
            Self::RegexExecutionSteps => 23,
            Self::RegexSearchSteps => 24,
            Self::RegexWorkspaceBytes => 25,
            Self::RegexScratchStates => 26,
            Self::TypeBindingDepth => 27,
            Self::SchemaFormatOutputBytes => 28,
            Self::ConfigFormatOutputBytes => 29,
            Self::FormattingWorkspaceBytes => 30,
            Self::FormattingNestingDepth => 31,
            Self::TypeBindingNodes => 32,
        }
    }

    pub(crate) const fn from_id(id: usize) -> Option<Self> {
        match id {
            1 => Some(Self::SourceBytes),
            2 => Some(Self::SchemaTotalBytes),
            3 => Some(Self::SchemaSourceCount),
            4 => Some(Self::SchemaImportDepth),
            5 => Some(Self::ExecutionSteps),
            6 => Some(Self::WorkspaceBytes),
            7 => Some(Self::SchemaParseDepth),
            8 => Some(Self::TokenBytes),
            9 => Some(Self::StringBytes),
            10 => Some(Self::SchemaAstNodes),
            11 => Some(Self::ConfigParseDepth),
            12 => Some(Self::ConfigValueDepth),
            13 => Some(Self::ConfigValueNodes),
            14 => Some(Self::ConfigTextBytes),
            15 => Some(Self::EnvironmentEntries),
            16 => Some(Self::EnvironmentBytes),
            17 => Some(Self::ValidationTraversal),
            18 => Some(Self::RegexCompilationBudget),
            19 => Some(Self::RegexPatternBytes),
            20 => Some(Self::RegexNestingDepth),
            21 => Some(Self::RegexInstructionCount),
            22 => Some(Self::RegexCharacterRanges),
            23 => Some(Self::RegexExecutionSteps),
            24 => Some(Self::RegexSearchSteps),
            25 => Some(Self::RegexWorkspaceBytes),
            26 => Some(Self::RegexScratchStates),
            27 => Some(Self::TypeBindingDepth),
            28 => Some(Self::SchemaFormatOutputBytes),
            29 => Some(Self::ConfigFormatOutputBytes),
            30 => Some(Self::FormattingWorkspaceBytes),
            31 => Some(Self::FormattingNestingDepth),
            32 => Some(Self::TypeBindingNodes),
            _ => None,
        }
    }
}

impl fmt::Display for ResourceLimit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::SourceBytes => "source bytes",
            Self::SchemaTotalBytes => "total schema bytes",
            Self::SchemaSourceCount => "schema source count",
            Self::SchemaImportDepth => "schema import depth",
            Self::ExecutionSteps => "execution steps",
            Self::WorkspaceBytes => "workspace bytes",
            Self::SchemaParseDepth => "schema parse depth",
            Self::TokenBytes => "token bytes",
            Self::StringBytes => "string bytes",
            Self::SchemaAstNodes => "schema AST nodes",
            Self::ConfigParseDepth => "configuration parse depth",
            Self::ConfigValueDepth => "configuration value depth",
            Self::ConfigValueNodes => "configuration value nodes",
            Self::ConfigTextBytes => "configuration text bytes",
            Self::EnvironmentEntries => "environment entries",
            Self::EnvironmentBytes => "environment bytes",
            Self::ValidationTraversal => "validation traversal",
            Self::RegexCompilationBudget => "regular-expression compilation budget",
            Self::RegexPatternBytes => "regular-expression pattern bytes",
            Self::RegexNestingDepth => "regular-expression nesting depth",
            Self::RegexInstructionCount => "regular-expression instruction count",
            Self::RegexCharacterRanges => "regular-expression character ranges",
            Self::RegexExecutionSteps => "regular-expression execution steps",
            Self::RegexSearchSteps => "regular-expression search steps",
            Self::RegexWorkspaceBytes => "regular-expression workspace bytes",
            Self::RegexScratchStates => "regular-expression scratch states",
            Self::TypeBindingDepth => "type binding depth",
            Self::SchemaFormatOutputBytes => "schema format output bytes",
            Self::ConfigFormatOutputBytes => "configuration format output bytes",
            Self::FormattingWorkspaceBytes => "formatting workspace bytes",
            Self::FormattingNestingDepth => "formatting nesting depth",
            Self::TypeBindingNodes => "type binding nodes",
        };
        formatter.write_str(name)
    }
}
