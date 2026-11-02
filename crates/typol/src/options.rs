use core::{error::Error, fmt};
use typol_config::{ConfigLimits, ConfigParseOptions, ConfigValidationOptions};
use typol_core::ExecutionLimits;
use typol_schema::SchemaCompileOptions;

#[cfg(feature = "std")]
use typol_std::{FileConfigInputLimits, FileSchemaLimits, SourceNameEditOptions, SourceNameLimits};

/// Maximum nesting depth for library-managed synchronous binding and conversion contexts; it does not limit Rust recursion inside handwritten callbacks.
pub const MAX_CONVERSION_DEPTH: usize = 128;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Error returned when the requested depth exceeds the limit accepted by library-managed conversion contexts.
pub struct ConversionLimitError {
    requested_depth: usize,
    maximum_depth: usize,
}

impl ConversionLimitError {
    const fn new(requested_depth: usize) -> Self {
        Self {
            requested_depth,
            maximum_depth: MAX_CONVERSION_DEPTH,
        }
    }

    /// Returns the requested depth.
    pub const fn requested_depth(self) -> usize {
        self.requested_depth
    }

    /// Returns the hard limit for synchronous conversion.
    pub const fn maximum_depth(self) -> usize {
        self.maximum_depth
    }
}

impl fmt::Display for ConversionLimitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "requested conversion depth {} exceeds hard limit {}",
            self.requested_depth, self.maximum_depth
        )
    }
}

impl Error for ConversionLimitError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Error returned when binding depth exceeds the synchronous recursion limit.
pub struct BindingLimitError {
    requested_depth: usize,
    maximum_depth: usize,
}

impl BindingLimitError {
    const fn new(requested_depth: usize) -> Self {
        Self {
            requested_depth,
            maximum_depth: MAX_CONVERSION_DEPTH,
        }
    }

    /// Returns the requested depth.
    pub const fn requested_depth(self) -> usize {
        self.requested_depth
    }

    /// Returns the hard limit for binding depth.
    pub const fn maximum_depth(self) -> usize {
        self.maximum_depth
    }
}

impl fmt::Display for BindingLimitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "requested binding depth {} exceeds hard limit {}",
            self.requested_depth, self.maximum_depth
        )
    }
}

impl Error for BindingLimitError {}

/// Bounds library-managed conversion nodes and nesting depth dispatched through `ConversionContext`.
///
/// These limits do not constrain direct work performed by a handwritten `Typol` callback.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConversionLimits {
    nodes: usize,
    depth: usize,
}

impl ConversionLimits {
    /// Creates conversion limits after validating the depth.
    pub const fn new(nodes: usize, depth: usize) -> Result<Self, ConversionLimitError> {
        if depth > MAX_CONVERSION_DEPTH {
            Err(ConversionLimitError::new(depth))
        } else {
            Ok(Self { nodes, depth })
        }
    }

    pub const fn max_nodes(self) -> usize {
        self.nodes
    }

    pub const fn max_depth(self) -> usize {
        self.depth
    }

    pub const fn with_max_nodes(mut self, value: usize) -> Self {
        self.nodes = value;
        self
    }

    /// Sets depth or returns an error above the synchronous recursion limit.
    pub const fn with_max_depth(mut self, value: usize) -> Result<Self, ConversionLimitError> {
        if value > MAX_CONVERSION_DEPTH {
            Err(ConversionLimitError::new(value))
        } else {
            self.depth = value;
            Ok(self)
        }
    }
}

impl Default for ConversionLimits {
    fn default() -> Self {
        Self {
            nodes: 100_000,
            depth: MAX_CONVERSION_DEPTH,
        }
    }
}

/// Bounds library-managed type-description nodes, depth, diagnostics, and name storage.
///
/// These limits do not constrain arbitrary Rust work performed by handwritten `Typol::describe` callbacks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BindingLimits {
    nodes: usize,
    depth: usize,
    diagnostics: usize,
    name_bytes: usize,
}

impl BindingLimits {
    /// Creates binding limits after validating the depth.
    pub const fn new(
        nodes: usize,
        depth: usize,
        diagnostics: usize,
        name_bytes: usize,
    ) -> Result<Self, BindingLimitError> {
        if depth > MAX_CONVERSION_DEPTH {
            Err(BindingLimitError::new(depth))
        } else {
            Ok(Self {
                nodes,
                depth,
                diagnostics,
                name_bytes,
            })
        }
    }

    pub const fn max_nodes(self) -> usize {
        self.nodes
    }

    pub const fn max_depth(self) -> usize {
        self.depth
    }

    pub const fn max_diagnostics(self) -> usize {
        self.diagnostics
    }

    pub const fn max_name_bytes(self) -> usize {
        self.name_bytes
    }

    pub const fn with_max_nodes(mut self, value: usize) -> Self {
        self.nodes = value;
        self
    }

    /// Sets depth or returns an error above the synchronous recursion limit.
    pub const fn with_max_depth(mut self, value: usize) -> Result<Self, BindingLimitError> {
        if value > MAX_CONVERSION_DEPTH {
            Err(BindingLimitError::new(value))
        } else {
            self.depth = value;
            Ok(self)
        }
    }

    pub const fn with_max_diagnostics(mut self, value: usize) -> Self {
        self.diagnostics = value;
        self
    }

    pub const fn with_max_name_bytes(mut self, value: usize) -> Self {
        self.name_bytes = value;
        self
    }

    const fn with_conversion_depth(mut self, conversion: ConversionLimits) -> Self {
        self.depth = conversion.max_depth();
        self
    }
}

impl Default for BindingLimits {
    fn default() -> Self {
        Self {
            nodes: 100_000,
            depth: MAX_CONVERSION_DEPTH,
            diagnostics: 100,
            name_bytes: 262_144,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BindOptions {
    limits: BindingLimits,
    execution: ExecutionLimits,
}

impl BindOptions {
    pub const fn new(limits: BindingLimits, execution: ExecutionLimits) -> Self {
        Self { limits, execution }
    }

    pub const fn limits(self) -> BindingLimits {
        self.limits
    }

    pub const fn execution(self) -> ExecutionLimits {
        self.execution
    }

    pub const fn with_limits(mut self, limits: BindingLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Sets binding depth or returns an error above the synchronous recursion limit.
    pub const fn with_max_depth(mut self, value: usize) -> Result<Self, BindingLimitError> {
        match self.limits.with_max_depth(value) {
            Ok(limits) => {
                self.limits = limits;
                Ok(self)
            }
            Err(error) => Err(error),
        }
    }

    const fn with_conversion_depth(mut self, conversion: ConversionLimits) -> Self {
        self.limits = self.limits.with_conversion_depth(conversion);
        self
    }

    pub const fn with_execution(mut self, execution: ExecutionLimits) -> Self {
        self.execution = execution;
        self
    }
}

impl Default for BindOptions {
    fn default() -> Self {
        Self::new(
            BindingLimits::default(),
            ExecutionLimits::new(16_777_216, 67_108_864),
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CodecOptions {
    validation: ConfigValidationOptions,
    conversion: ConversionLimits,
    max_output_bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WriteOptions {
    max_output_bytes: usize,
    conversion: ConversionLimits,
}

impl WriteOptions {
    pub const fn new(max_output_bytes: usize) -> Self {
        Self {
            max_output_bytes,
            conversion: ConversionLimits {
                nodes: 100_000,
                depth: MAX_CONVERSION_DEPTH,
            },
        }
    }

    pub const fn max_output_bytes(self) -> usize {
        self.max_output_bytes
    }

    pub const fn conversion_limits(self) -> ConversionLimits {
        self.conversion
    }

    pub const fn with_conversion_limits(mut self, conversion: ConversionLimits) -> Self {
        self.conversion = conversion;
        self
    }
}

impl Default for WriteOptions {
    fn default() -> Self {
        Self::new(1_048_576)
    }
}

impl CodecOptions {
    pub const fn new(
        validation: ConfigValidationOptions,
        conversion: ConversionLimits,
        max_output_bytes: usize,
    ) -> Self {
        Self {
            validation,
            conversion,
            max_output_bytes,
        }
    }

    pub const fn validation(self) -> ConfigValidationOptions {
        self.validation
    }

    pub const fn conversion(self) -> ConversionLimits {
        self.conversion
    }

    pub const fn max_output_bytes(self) -> usize {
        self.max_output_bytes
    }

    pub const fn with_validation(mut self, validation: ConfigValidationOptions) -> Self {
        self.validation = validation;
        self
    }

    pub const fn with_conversion(mut self, conversion: ConversionLimits) -> Self {
        self.conversion = conversion;
        self
    }

    pub const fn with_max_output_bytes(mut self, max_output_bytes: usize) -> Self {
        self.max_output_bytes = max_output_bytes;
        self
    }
}

impl Default for CodecOptions {
    fn default() -> Self {
        Self::new(
            ConfigValidationOptions::default(),
            ConversionLimits::default(),
            1_048_576,
        )
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TypolOptions {
    schema: SchemaCompileOptions,
    bind: BindOptions,
    config: ConfigParseOptions,
    conversion: ConversionLimits,
    #[cfg(feature = "std")]
    files: FileSchemaLimits,
    #[cfg(feature = "std")]
    names: SourceNameLimits,
    #[cfg(feature = "std")]
    name_edits: SourceNameEditOptions,
    #[cfg(feature = "std")]
    file_input: FileConfigInputLimits,
}

impl TypolOptions {
    pub const fn schema(&self) -> &SchemaCompileOptions {
        &self.schema
    }

    pub const fn bind(&self) -> BindOptions {
        self.bind
    }

    pub const fn config(&self) -> ConfigParseOptions {
        self.config
    }

    pub const fn validation(&self) -> ConfigValidationOptions {
        ConfigValidationOptions::new(
            *self.config.resources(),
            *self.config.config(),
            *self.config.execution(),
        )
    }

    pub const fn conversion(&self) -> ConversionLimits {
        self.conversion
    }

    pub const fn with_schema(mut self, schema: SchemaCompileOptions) -> Self {
        self.schema = schema;
        self
    }

    pub const fn with_bind(mut self, bind: BindOptions) -> Self {
        self.bind = bind;
        self
    }

    pub const fn with_config(mut self, config: ConfigParseOptions) -> Self {
        self.config = config;
        self
    }

    pub const fn with_conversion(mut self, conversion: ConversionLimits) -> Self {
        self.conversion = conversion;
        self
    }

    pub const fn with_config_limits(mut self, limits: ConfigLimits) -> Self {
        self.config = self.config.with_config(limits);
        self
    }

    pub const fn with_binding_limits(mut self, limits: BindingLimits) -> Self {
        self.bind = self.bind.with_limits(limits);
        self
    }

    /// Sets the maximum zero-based depth shared by config, library-managed binding, and conversion contexts; handwritten callback recursion is not constrained.
    pub fn with_max_typed_depth(mut self, value: usize) -> Result<Self, ConversionLimitError> {
        let conversion = self.conversion.with_max_depth(value)?;
        self.schema = self.schema.with_max_typed_depth(value);
        self.config = self
            .config
            .with_config(self.config.config().with_max_nesting_depth(value));
        self.bind = self.bind.with_conversion_depth(conversion);
        self.conversion = conversion;
        Ok(self)
    }

    #[cfg(feature = "std")]
    pub const fn files(&self) -> FileSchemaLimits {
        self.files
    }

    #[cfg(feature = "std")]
    pub const fn names(&self) -> SourceNameLimits {
        self.names
    }

    #[cfg(feature = "std")]
    pub const fn name_edits(&self) -> SourceNameEditOptions {
        self.name_edits
    }

    #[cfg(feature = "std")]
    pub const fn file_input_limits(&self) -> FileConfigInputLimits {
        self.file_input
    }

    #[cfg(feature = "std")]
    pub const fn with_files(mut self, files: FileSchemaLimits) -> Self {
        self.files = files;
        self
    }

    #[cfg(feature = "std")]
    pub const fn with_names(mut self, names: SourceNameLimits) -> Self {
        self.names = names;
        self
    }

    #[cfg(feature = "std")]
    pub const fn with_name_edits(mut self, name_edits: SourceNameEditOptions) -> Self {
        self.name_edits = name_edits;
        self
    }

    #[cfg(feature = "std")]
    pub const fn with_file_input_limits(mut self, limits: FileConfigInputLimits) -> Self {
        self.file_input = limits;
        self
    }
}
