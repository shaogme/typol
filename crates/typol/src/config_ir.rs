mod serializer;

use alloc::vec::Vec;
use core::{error::Error, fmt};
use opeo::ErrSlot;
pub use typol_config::{
    ConfigValidationOptions, ConfigValidationRequest, ConfigValueErrors, ConfigValueProcessor,
    NormalizedConfigValue, ValidatedConfigValue,
};
use typol_core::ConfigValue;
use typol_semantic::CompiledSchema;

const DEFAULT_MAX_OUTPUT_BYTES: usize = 16_777_216;
const DEFAULT_MAX_WORKSPACE_BYTES: usize = 16_777_216;
const DEFAULT_MAX_NESTING_DEPTH: usize = 128;

pub struct ConfigIr;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigIrValueMode {
    AsProvided,
    WithDefaults,
    Validated,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConfigIrOptions {
    pretty: bool,
    max_output_bytes: usize,
    max_workspace_bytes: usize,
    max_nesting_depth: usize,
    value_mode: ConfigIrValueMode,
    validation_options: Option<ConfigValidationOptions>,
}

/// A stable category for Config IR errors, without their specific payloads.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigIrErrorKind {
    SchemaAccess,
    ValueShapeMismatch,
    UnsupportedFloat,
    OutputLimitExceeded,
    WorkspaceLimitExceeded,
    NestingLimitExceeded,
    InvalidNestingDepthLimit,
    AllocationFailed,
    SerializationFailed,
    NormalizationFailed,
    ValidationFailed,
}

#[derive(Debug)]
pub enum ConfigIrError {
    SchemaAccess,
    ValueShapeMismatch,
    UnsupportedFloat,
    OutputLimitExceeded,
    WorkspaceLimitExceeded,
    NestingLimitExceeded,
    InvalidNestingDepthLimit { requested: usize, maximum: usize },
    AllocationFailed,
    SerializationFailed,
    NormalizationFailed(ConfigValueErrors),
    ValidationFailed(ConfigValueErrors),
}

impl ConfigIrError {
    /// Return the error category; normalization and validation diagnostics remain in the variant.
    pub const fn kind(&self) -> ConfigIrErrorKind {
        match self {
            Self::SchemaAccess => ConfigIrErrorKind::SchemaAccess,
            Self::ValueShapeMismatch => ConfigIrErrorKind::ValueShapeMismatch,
            Self::UnsupportedFloat => ConfigIrErrorKind::UnsupportedFloat,
            Self::OutputLimitExceeded => ConfigIrErrorKind::OutputLimitExceeded,
            Self::WorkspaceLimitExceeded => ConfigIrErrorKind::WorkspaceLimitExceeded,
            Self::NestingLimitExceeded => ConfigIrErrorKind::NestingLimitExceeded,
            Self::InvalidNestingDepthLimit { .. } => ConfigIrErrorKind::InvalidNestingDepthLimit,
            Self::AllocationFailed => ConfigIrErrorKind::AllocationFailed,
            Self::SerializationFailed => ConfigIrErrorKind::SerializationFailed,
            Self::NormalizationFailed(_) => ConfigIrErrorKind::NormalizationFailed,
            Self::ValidationFailed(_) => ConfigIrErrorKind::ValidationFailed,
        }
    }
}

impl ConfigIr {
    /// Process an owned configuration value and encode Config IR v1.
    pub fn serialize(
        schema: &CompiledSchema,
        value: ConfigValue,
        options: ConfigIrOptions,
    ) -> Result<Vec<u8>, ConfigIrError> {
        match options.value_mode() {
            ConfigIrValueMode::AsProvided => {
                serializer::ConfigIrSerializer::serialize(schema, &value, options)
            }
            ConfigIrValueMode::WithDefaults => {
                let value = Self::normalize(schema, value, options.validation_options())?;
                serializer::ConfigIrSerializer::serialize(schema, value.as_config_value(), options)
            }
            ConfigIrValueMode::Validated => {
                let value = Self::validate(schema, value, options.validation_options())?;
                serializer::ConfigIrSerializer::serialize(schema, value.as_config_value(), options)
            }
        }
    }

    fn normalize(
        schema: &CompiledSchema,
        value: ConfigValue,
        options: ConfigValidationOptions,
    ) -> Result<NormalizedConfigValue, ConfigIrError> {
        let mut errors = ErrSlot::<ConfigValueErrors>::new();
        match errors.try_call(|out| {
            ConfigValueProcessor::new(schema).normalize(
                value,
                ConfigValidationRequest::new(options),
                out,
            )
        }) {
            Ok(value) => Ok(value),
            Err(caught) => Err(ConfigIrError::NormalizationFailed(caught.take())),
        }
    }

    fn validate(
        schema: &CompiledSchema,
        value: ConfigValue,
        options: ConfigValidationOptions,
    ) -> Result<ValidatedConfigValue, ConfigIrError> {
        let mut errors = ErrSlot::<ConfigValueErrors>::new();
        match errors.try_call(|out| {
            ConfigValueProcessor::new(schema).validate(
                value,
                ConfigValidationRequest::new(options),
                out,
            )
        }) {
            Ok(value) => Ok(value),
            Err(caught) => Err(ConfigIrError::ValidationFailed(caught.take())),
        }
    }
}

impl ConfigIrOptions {
    pub const fn new() -> Self {
        Self {
            pretty: false,
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
            max_workspace_bytes: DEFAULT_MAX_WORKSPACE_BYTES,
            max_nesting_depth: DEFAULT_MAX_NESTING_DEPTH,
            value_mode: ConfigIrValueMode::Validated,
            validation_options: None,
        }
    }

    pub const fn with_pretty(mut self, pretty: bool) -> Self {
        self.pretty = pretty;
        self
    }

    pub const fn with_max_output_bytes(mut self, max_output_bytes: usize) -> Self {
        self.max_output_bytes = max_output_bytes;
        self
    }

    pub const fn with_max_workspace_bytes(mut self, max_workspace_bytes: usize) -> Self {
        self.max_workspace_bytes = max_workspace_bytes;
        self
    }

    /// Set the IR nesting limit and reject requests above the fixed safety cap.
    pub const fn try_with_max_nesting_depth(
        mut self,
        max_nesting_depth: usize,
    ) -> Result<Self, ConfigIrError> {
        if max_nesting_depth > DEFAULT_MAX_NESTING_DEPTH {
            return Err(ConfigIrError::InvalidNestingDepthLimit {
                requested: max_nesting_depth,
                maximum: DEFAULT_MAX_NESTING_DEPTH,
            });
        }
        self.max_nesting_depth = max_nesting_depth;
        Ok(self)
    }

    pub const fn with_value_mode(mut self, value_mode: ConfigIrValueMode) -> Self {
        self.value_mode = value_mode;
        self
    }

    pub const fn with_validation_options(mut self, options: ConfigValidationOptions) -> Self {
        self.validation_options = Some(options);
        self
    }

    pub const fn pretty(&self) -> bool {
        self.pretty
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

    pub const fn value_mode(&self) -> ConfigIrValueMode {
        self.value_mode
    }

    pub fn validation_options(&self) -> ConfigValidationOptions {
        self.validation_options.unwrap_or_default()
    }
}

impl Default for ConfigIrOptions {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for ConfigIrError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SchemaAccess => formatter.write_str("failed to read compiled schema"),
            Self::ValueShapeMismatch => {
                formatter.write_str("configuration value does not match the compiled type")
            }
            Self::UnsupportedFloat => {
                formatter.write_str("IR does not support non-finite floating-point values")
            }
            Self::OutputLimitExceeded => formatter.write_str("IR output limit exceeded"),
            Self::WorkspaceLimitExceeded => {
                formatter.write_str("IR sorting workspace limit exceeded")
            }
            Self::NestingLimitExceeded => formatter.write_str("IR nesting depth limit exceeded"),
            Self::InvalidNestingDepthLimit { requested, maximum } => write!(
                formatter,
                "requested IR nesting depth limit {requested} exceeds the fixed safety limit {maximum}"
            ),
            Self::AllocationFailed => formatter.write_str("IR serialization allocation failed"),
            Self::SerializationFailed => formatter.write_str("IR JSON serialization failed"),
            Self::NormalizationFailed(errors) => {
                write!(
                    formatter,
                    "configuration value normalization failed: {errors}"
                )
            }
            Self::ValidationFailed(errors) => {
                write!(formatter, "configuration value validation failed: {errors}")
            }
        }
    }
}

impl Error for ConfigIrError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::NormalizationFailed(errors) | Self::ValidationFailed(errors) => Some(errors),
            _ => None,
        }
    }
}
