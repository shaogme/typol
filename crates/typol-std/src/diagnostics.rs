mod localized;
mod renderer;

pub use localized::{
    DiagnosticRenderError, DiagnosticRenderer, DiagnosticReport, DiagnosticReportStage,
    DiagnosticReportTitle,
};

use std::{error::Error, fmt};

use typol_core::{
    BudgetError, DiagnosticContext as ErrorContext, ExecutionLimits, SourceExcerptSet, SourceId,
    SourceSpan, diagnostic_context,
};
use typol_semantic::{SchemaAccessError, SpanLocation};

static DISPLAY_BUDGET_CONTEXT: ErrorContext =
    diagnostic_context!("typol-std", "display", "budget-invariant",);
static DISPLAY_SCHEMA_ACCESS_CONTEXT: ErrorContext =
    diagnostic_context!("typol-std", "display", "schema-access",);
static DISPLAY_LITERAL_FORMAT_CONTEXT: ErrorContext =
    diagnostic_context!("typol-std", "display-literal", "format-number",);
static DISPLAY_LITERAL_EXPANSION_CONTEXT: ErrorContext =
    diagnostic_context!("typol-std", "display-literal", "expand-float-exponent",);
static DISPLAY_LITERAL_BUFFER_CONTEXT: ErrorContext =
    diagnostic_context!("typol-std", "display-literal", "literal-buffer-access",);
static DISPLAY_LITERAL_UTF8_CONTEXT: ErrorContext =
    diagnostic_context!("typol-std", "display-literal", "literal-utf8-validation",);
static DISPLAY_RENDER_NUMBER_CONTEXT: ErrorContext =
    diagnostic_context!("typol-std", "diagnostic-rendering", "format-number",);
static DISPLAY_OUTPUT_UTF8_CONTEXT: ErrorContext =
    diagnostic_context!("typol-std", "display-output", "utf8-validation",);
static DISPLAY_OUTPUT_WRITE_CONTEXT: ErrorContext =
    diagnostic_context!("typol-std", "diagnostic-rendering", "writer-failure",);
static DISPLAY_COLLECTION_CONTEXT: ErrorContext =
    diagnostic_context!("typol-std", "value-display", "map-sort-invariant",);
static DISPLAY_ENUM_CONTEXT: ErrorContext =
    diagnostic_context!("typol-std", "value-display", "enum-structure-lookup",);
static DISPLAY_DEPTH_CONTEXT: ErrorContext =
    diagnostic_context!("typol-std", "value-display", "depth-increment",);

/// Static call sites for internal renderer failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisplayFailurePoint {
    BudgetInvariant,
    SchemaAccess,
    NumberFormatting,
    FloatExpansion,
    LiteralBufferAccess,
    LiteralUtf8Validation,
    OutputNumberFormatting,
    OutputUtf8Validation,
    OutputWrite,
    TranslationFailure,
    CollectionInvariant,
    EnumStructureLookup,
    DepthIncrement,
}

impl DisplayFailurePoint {
    const fn context(self) -> ErrorContext {
        match self {
            Self::BudgetInvariant => DISPLAY_BUDGET_CONTEXT,
            Self::SchemaAccess => DISPLAY_SCHEMA_ACCESS_CONTEXT,
            Self::NumberFormatting => DISPLAY_LITERAL_FORMAT_CONTEXT,
            Self::FloatExpansion => DISPLAY_LITERAL_EXPANSION_CONTEXT,
            Self::LiteralBufferAccess => DISPLAY_LITERAL_BUFFER_CONTEXT,
            Self::LiteralUtf8Validation => DISPLAY_LITERAL_UTF8_CONTEXT,
            Self::OutputNumberFormatting => DISPLAY_RENDER_NUMBER_CONTEXT,
            Self::OutputUtf8Validation => DISPLAY_OUTPUT_UTF8_CONTEXT,
            Self::OutputWrite => DISPLAY_OUTPUT_WRITE_CONTEXT,
            Self::TranslationFailure => DISPLAY_OUTPUT_WRITE_CONTEXT,
            Self::CollectionInvariant => DISPLAY_COLLECTION_CONTEXT,
            Self::EnumStructureLookup => DISPLAY_ENUM_CONTEXT,
            Self::DepthIncrement => DISPLAY_DEPTH_CONTEXT,
        }
    }
}

/// Rendering failures are separate from business diagnostics and carry no input or partial output.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisplayError {
    ResourceLimitExceeded,
    AllocationFailed,
    CapacityExceeded,
    ShapeMismatch,
    InternalError {
        failure_point: DisplayFailurePoint,
        source_context: Option<ErrorContext>,
    },
}

impl DisplayError {
    pub const fn internal(failure_point: DisplayFailurePoint) -> Self {
        Self::internal_with_source(failure_point, None)
    }

    const fn internal_with_source(
        failure_point: DisplayFailurePoint,
        source_context: Option<ErrorContext>,
    ) -> Self {
        Self::InternalError {
            failure_point,
            source_context,
        }
    }

    pub const fn context(&self) -> Option<ErrorContext> {
        match self {
            Self::InternalError { failure_point, .. } => Some(failure_point.context()),
            Self::ResourceLimitExceeded
            | Self::AllocationFailed
            | Self::CapacityExceeded
            | Self::ShapeMismatch => None,
        }
    }

    pub const fn source_context(&self) -> Option<ErrorContext> {
        match self {
            Self::InternalError { source_context, .. } => *source_context,
            Self::ResourceLimitExceeded
            | Self::AllocationFailed
            | Self::CapacityExceeded
            | Self::ShapeMismatch => None,
        }
    }
}

impl From<BudgetError> for DisplayError {
    fn from(error: BudgetError) -> Self {
        match error {
            BudgetError::Resource => Self::ResourceLimitExceeded,
            BudgetError::Allocation => Self::AllocationFailed,
            BudgetError::CapacityExceeded => Self::CapacityExceeded,
            BudgetError::InvariantViolation(context) => {
                Self::internal_with_source(DisplayFailurePoint::BudgetInvariant, Some(*context))
            }
        }
    }
}

impl From<SchemaAccessError> for DisplayError {
    fn from(error: SchemaAccessError) -> Self {
        Self::internal_with_source(DisplayFailurePoint::SchemaAccess, Some(error.context()))
    }
}

impl fmt::Display for DisplayError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ResourceLimitExceeded => "Diagnostic rendering failed: resource limit exceeded",
            Self::AllocationFailed => "Diagnostic rendering failed: memory allocation failed",
            Self::CapacityExceeded => "Diagnostic rendering failed: workspace capacity exceeded",
            Self::ShapeMismatch => {
                "Diagnostic rendering failed: value does not match the schema shape"
            }
            Self::InternalError { .. } => {
                "Diagnostic rendering failed: invalid internal relationship"
            }
        })
    }
}

impl Error for DisplayError {}

/// Callbacks query existing names and span snapshots without rescanning source; excerpts are safe snapshots owned by the error.
/// Work, allocations, and panics inside caller-provided callbacks are outside the rendering budget.
pub struct DiagnosticContext<'a> {
    pub source_name: &'a dyn Fn(SourceId) -> Option<&'a str>,
    pub span_location: &'a dyn Fn(SourceSpan) -> Option<SpanLocation>,
    pub excerpts: &'a SourceExcerptSet,
}

#[derive(Clone, Copy, Debug)]
pub struct RenderOptions {
    execution: ExecutionLimits,
    max_output_bytes: usize,
}

impl RenderOptions {
    pub const fn new(execution: ExecutionLimits, max_output_bytes: usize) -> Self {
        Self {
            execution,
            max_output_bytes,
        }
    }

    pub const fn execution(&self) -> ExecutionLimits {
        self.execution
    }

    pub const fn max_output_bytes(&self) -> usize {
        self.max_output_bytes
    }
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self::new(ExecutionLimits::new(16_777_216, 8_388_608), 1_048_576)
    }
}
