use alloc::string::String;
use core::{error::Error, fmt};
use typol_core::{
    CapacityExceeded, DiagnosticContext, InternalError, ResourceLimit, diagnostic_context,
};

static FORMAT_SOURCE_INCONSISTENT: DiagnosticContext = diagnostic_context!(
    "typol-syntax",
    "schema-formatting",
    "source-token-range-invalid"
);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchemaFormatErrorKind {
    ResourceLimit(ResourceLimit),
    CapacityExceeded(CapacityExceeded),
    AllocationFailed,
    Internal(InternalError),
    OutputSinkFailure,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchemaFormatError {
    kind: SchemaFormatErrorKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchemaFormatOptions {
    max_output_bytes: usize,
    max_workspace_bytes: usize,
    max_nesting_depth: usize,
}

#[derive(Debug)]
pub struct FormattedSchema {
    text: String,
}

impl SchemaFormatError {
    pub(super) const fn new(kind: SchemaFormatErrorKind) -> Self {
        Self { kind }
    }

    pub(super) const fn inconsistent_source() -> Self {
        Self::new(SchemaFormatErrorKind::Internal(
            InternalError::SyntaxStorageInvariantFailure(FORMAT_SOURCE_INCONSISTENT),
        ))
    }

    pub(super) const fn capacity_exceeded() -> Self {
        Self::new(SchemaFormatErrorKind::CapacityExceeded(
            CapacityExceeded::FormatOutput,
        ))
    }

    pub const fn kind(self) -> SchemaFormatErrorKind {
        self.kind
    }

    pub const fn context(self) -> Option<DiagnosticContext> {
        match self.kind {
            SchemaFormatErrorKind::Internal(error) => Some(error.context()),
            SchemaFormatErrorKind::ResourceLimit(_)
            | SchemaFormatErrorKind::CapacityExceeded(_)
            | SchemaFormatErrorKind::AllocationFailed
            | SchemaFormatErrorKind::OutputSinkFailure => None,
        }
    }
}

impl fmt::Display for SchemaFormatError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            SchemaFormatErrorKind::ResourceLimit(limit) => {
                write!(formatter, "schema formatting {limit} limit exceeded")
            }
            SchemaFormatErrorKind::CapacityExceeded(_) => {
                formatter.write_str("schema formatting capacity exceeded")
            }
            SchemaFormatErrorKind::AllocationFailed => {
                formatter.write_str("schema formatting allocation failed")
            }
            SchemaFormatErrorKind::Internal(_) => {
                formatter.write_str("schema formatting invariant violated")
            }
            SchemaFormatErrorKind::OutputSinkFailure => {
                formatter.write_str("schema formatting output sink failed")
            }
        }
    }
}

impl Error for SchemaFormatError {}

impl SchemaFormatOptions {
    pub const fn new(
        max_output_bytes: usize,
        max_workspace_bytes: usize,
        max_nesting_depth: usize,
    ) -> Self {
        Self {
            max_output_bytes,
            max_workspace_bytes,
            max_nesting_depth,
        }
    }

    pub const fn max_output_bytes(&self) -> usize {
        self.max_output_bytes
    }

    pub const fn max_workspace_bytes(&self) -> usize {
        self.max_workspace_bytes
    }

    pub const fn max_nesting_depth(&self) -> usize {
        self.max_nesting_depth
    }

    pub const fn with_max_output_bytes(mut self, value: usize) -> Self {
        self.max_output_bytes = value;
        self
    }

    pub const fn with_max_workspace_bytes(mut self, value: usize) -> Self {
        self.max_workspace_bytes = value;
        self
    }

    pub const fn with_max_nesting_depth(mut self, value: usize) -> Self {
        self.max_nesting_depth = value;
        self
    }
}

impl Default for SchemaFormatOptions {
    fn default() -> Self {
        Self::new(1_048_576, 4_194_304, 128)
    }
}

impl FormattedSchema {
    pub(super) fn new(text: String) -> Self {
        Self { text }
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn into_string(self) -> String {
        self.text
    }

    pub fn write_to(self, output: &mut impl fmt::Write) -> Result<(), SchemaFormatError> {
        output
            .write_str(&self.text)
            .map_err(|_| SchemaFormatError::new(SchemaFormatErrorKind::OutputSinkFailure))
    }
}
