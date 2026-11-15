use core::{error::Error, fmt};
use typol_core::{
    BudgetError, CapacityExceeded, ChargedString, InternalError, ResourceLimit, Write, WriteError,
};

const DEFAULT_MAX_OUTPUT_BYTES: usize = 16_777_216;
const DEFAULT_MAX_WORKSPACE_BYTES: usize = 16_777_216;
const DEFAULT_MAX_NESTING_DEPTH: usize = 128;

/// Stable resource and structure categories for configuration formatting failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigFormatErrorKind {
    ResourceLimit(ResourceLimit),
    AllocationFailed,
    CapacityExceeded(CapacityExceeded),
    Internal(InternalError),
    InvalidStructure,
}

/// A formatting failure that never exposes configuration source text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConfigFormatError {
    kind: ConfigFormatErrorKind,
}

/// Limits for formatter-owned output and explicit nesting workspaces.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConfigFormatOptions {
    max_output_bytes: usize,
    max_workspace_bytes: usize,
    max_nesting_depth: usize,
}

/// A fully formatted configuration string owned independently from its source document.
pub struct FormattedConfig {
    source: ChargedString,
}

impl ConfigFormatError {
    pub(super) const fn new(kind: ConfigFormatErrorKind) -> Self {
        Self { kind }
    }

    pub const fn kind(self) -> ConfigFormatErrorKind {
        self.kind
    }
}

impl From<BudgetError> for ConfigFormatError {
    fn from(error: BudgetError) -> Self {
        Self::new(match error {
            BudgetError::ResourceLimit(limit) => ConfigFormatErrorKind::ResourceLimit(limit),
            BudgetError::CapacityExceeded(capacity) => {
                ConfigFormatErrorKind::CapacityExceeded(capacity)
            }
            BudgetError::Allocation => ConfigFormatErrorKind::AllocationFailed,
            BudgetError::Internal(error) => ConfigFormatErrorKind::Internal(error),
        })
    }
}

impl fmt::Display for ConfigFormatError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            ConfigFormatErrorKind::ResourceLimit(limit) => {
                write!(formatter, "{limit} limit exceeded")
            }
            ConfigFormatErrorKind::AllocationFailed => {
                formatter.write_str("configuration formatting allocation failed")
            }
            ConfigFormatErrorKind::CapacityExceeded(_) => {
                formatter.write_str("configuration formatting capacity exceeded")
            }
            ConfigFormatErrorKind::Internal(_) => {
                formatter.write_str("configuration formatting invariant failed")
            }
            ConfigFormatErrorKind::InvalidStructure => {
                formatter.write_str("configuration formatting encountered invalid structure")
            }
        }
    }
}

impl Error for ConfigFormatError {}

impl ConfigFormatOptions {
    pub const fn new() -> Self {
        Self {
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
            max_workspace_bytes: DEFAULT_MAX_WORKSPACE_BYTES,
            max_nesting_depth: DEFAULT_MAX_NESTING_DEPTH,
        }
    }

    pub const fn with_max_output_bytes(mut self, max_output_bytes: usize) -> Self {
        self.max_output_bytes = max_output_bytes;
        self
    }

    pub const fn with_max_workspace_bytes(mut self, max_workspace_bytes: usize) -> Self {
        self.max_workspace_bytes = max_workspace_bytes;
        self
    }

    pub const fn with_max_nesting_depth(mut self, max_nesting_depth: usize) -> Self {
        self.max_nesting_depth = max_nesting_depth;
        self
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
}

impl Default for ConfigFormatOptions {
    fn default() -> Self {
        Self::new()
    }
}

impl FormattedConfig {
    pub(super) const fn from_source(source: ChargedString) -> Self {
        Self { source }
    }

    /// Borrows the complete formatted text.
    pub fn as_str(&self) -> &str {
        self.source.as_str()
    }

    /// Consumes the formatted result after writing all of it to the supplied sink.
    pub fn write_to(self, output: &mut dyn Write) -> Result<(), WriteError> {
        output.write_all(self.source.as_str().as_bytes())
    }
}
