use core::fmt;
use opeo::{OResult, Out};
use typol_core::{BudgetError, ByteSource, ExecutionBudget, ExecutionLimits};
use typol_policy::{ResourceLimits, ValueLimits};
use typol_syntax::{InputLimits, StreamLimits};

/// Configuration-specific cumulative limits with finite defaults that hosts can tune.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConfigLimits {
    max_source_bytes: usize,
    max_diagnostics: usize,
    values: ValueLimits,
    max_environment_entries: usize,
    max_environment_bytes: usize,
    max_constructed_text_bytes: usize,
    max_display_name_bytes: usize,
}

impl ConfigLimits {
    /// Hard limit for diagnostics; larger requests are capped at this value.
    pub const MAX_DIAGNOSTICS: usize = 1024;

    pub const fn max_source_bytes(&self) -> usize {
        self.max_source_bytes
    }

    pub const fn max_diagnostics(&self) -> usize {
        self.max_diagnostics
    }

    pub const fn values(&self) -> &ValueLimits {
        &self.values
    }

    pub const fn max_nesting_depth(&self) -> usize {
        self.values.max_nesting_depth()
    }

    pub const fn max_string_bytes(&self) -> usize {
        self.values.max_string_bytes()
    }

    pub const fn max_value_nodes(&self) -> usize {
        self.values.max_value_nodes()
    }

    pub const fn input_limits(&self) -> InputLimits {
        InputLimits::new(self.max_source_bytes, self.values.max_string_bytes())
    }

    pub const fn max_environment_entries(&self) -> usize {
        self.max_environment_entries
    }

    pub const fn max_environment_bytes(&self) -> usize {
        self.max_environment_bytes
    }

    pub const fn max_constructed_text_bytes(&self) -> usize {
        self.max_constructed_text_bytes
    }

    pub const fn max_display_name_bytes(&self) -> usize {
        self.max_display_name_bytes
    }

    pub const fn with_max_environment_entries(mut self, value: usize) -> Self {
        self.max_environment_entries = value;
        self
    }

    pub const fn with_max_source_bytes(mut self, value: usize) -> Self {
        self.max_source_bytes = value;
        self
    }

    pub const fn with_max_diagnostics(mut self, value: usize) -> Self {
        self.max_diagnostics = if value > Self::MAX_DIAGNOSTICS {
            Self::MAX_DIAGNOSTICS
        } else {
            value
        };
        self
    }

    pub const fn with_max_environment_bytes(mut self, value: usize) -> Self {
        self.max_environment_bytes = value;
        self
    }

    pub const fn with_max_constructed_text_bytes(mut self, value: usize) -> Self {
        self.max_constructed_text_bytes = value;
        self
    }

    pub const fn with_max_display_name_bytes(mut self, value: usize) -> Self {
        self.max_display_name_bytes = value;
        self
    }

    pub const fn with_max_nesting_depth(mut self, value: usize) -> Self {
        self.values = ValueLimits::new(
            value,
            self.values.max_string_bytes(),
            self.values.max_value_nodes(),
        );
        self
    }

    pub const fn with_max_string_bytes(mut self, value: usize) -> Self {
        self.values = ValueLimits::new(
            self.values.max_nesting_depth(),
            value,
            self.values.max_value_nodes(),
        );
        self
    }

    pub const fn with_max_value_nodes(mut self, value: usize) -> Self {
        self.values = ValueLimits::new(
            self.values.max_nesting_depth(),
            self.values.max_string_bytes(),
            value,
        );
        self
    }
}

impl Default for ConfigLimits {
    fn default() -> Self {
        Self {
            max_source_bytes: 1_048_576,
            max_diagnostics: 100,
            values: ValueLimits::default(),
            max_environment_entries: 1024,
            max_environment_bytes: 8_388_608,
            max_constructed_text_bytes: 16_777_216,
            max_display_name_bytes: 4096,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConfigParseOptions {
    resources: ResourceLimits,
    config: ConfigLimits,
    execution: ExecutionLimits,
    stream: StreamLimits,
}

impl ConfigParseOptions {
    pub const fn new(
        resources: ResourceLimits,
        config: ConfigLimits,
        execution: ExecutionLimits,
    ) -> Self {
        Self {
            resources,
            config,
            execution,
            stream: StreamLimits::DEFAULT,
        }
    }

    pub const fn with_stream(mut self, stream: StreamLimits) -> Self {
        self.stream = stream;
        self
    }

    pub const fn with_resources(mut self, resources: ResourceLimits) -> Self {
        self.resources = resources;
        self
    }

    pub const fn with_config(mut self, config: ConfigLimits) -> Self {
        self.config = config;
        self
    }

    pub const fn with_execution(mut self, execution: ExecutionLimits) -> Self {
        self.execution = execution;
        self
    }

    pub const fn stream(&self) -> StreamLimits {
        self.stream
    }

    pub const fn resources(&self) -> &ResourceLimits {
        &self.resources
    }

    pub const fn config(&self) -> &ConfigLimits {
        &self.config
    }

    pub const fn execution(&self) -> &ExecutionLimits {
        &self.execution
    }
}

impl Default for ConfigParseOptions {
    fn default() -> Self {
        Self::new(
            ResourceLimits::default(),
            ConfigLimits::default(),
            ExecutionLimits::new(16_777_216, 67_108_864),
        )
    }
}

/// Shared resource, configuration, and execution limits for external value normalization and validation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConfigValidationOptions {
    resources: ResourceLimits,
    config: ConfigLimits,
    execution: ExecutionLimits,
}

impl ConfigValidationOptions {
    pub const fn new(
        resources: ResourceLimits,
        config: ConfigLimits,
        execution: ExecutionLimits,
    ) -> Self {
        Self {
            resources,
            config,
            execution,
        }
    }

    pub const fn with_resources(mut self, resources: ResourceLimits) -> Self {
        self.resources = resources;
        self
    }

    pub const fn with_config(mut self, config: ConfigLimits) -> Self {
        self.config = config;
        self
    }

    pub const fn with_execution(mut self, execution: ExecutionLimits) -> Self {
        self.execution = execution;
        self
    }

    pub const fn resources(&self) -> &ResourceLimits {
        &self.resources
    }

    pub const fn config(&self) -> &ConfigLimits {
        &self.config
    }

    pub const fn execution(&self) -> &ExecutionLimits {
        &self.execution
    }
}

impl Default for ConfigValidationOptions {
    fn default() -> Self {
        Self::new(
            ResourceLimits::default(),
            ConfigLimits::default(),
            ExecutionLimits::new(16_777_216, 67_108_864),
        )
    }
}

pub struct ConfigParseRequest<'input> {
    input: &'input mut dyn ByteSource,
    display_name: &'input str,
    options: ConfigParseOptions,
}

pub(crate) struct RequestParts<'input> {
    pub(crate) input: &'input mut dyn ByteSource,
    pub(crate) display_name: &'input str,
    pub(crate) options: ConfigParseOptions,
}

impl<'input> ConfigParseRequest<'input> {
    pub fn new(
        input: &'input mut dyn ByteSource,
        display_name: &'input str,
        options: ConfigParseOptions,
    ) -> Self {
        Self {
            input,
            display_name,
            options,
        }
    }

    pub const fn options(&self) -> &ConfigParseOptions {
        &self.options
    }

    pub(crate) const fn display_name(&self) -> &str {
        self.display_name
    }

    pub(crate) fn into_parts(self) -> RequestParts<'input> {
        RequestParts {
            input: self.input,
            display_name: self.display_name,
            options: self.options,
        }
    }
}

impl fmt::Debug for ConfigParseRequest<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConfigParseRequest")
            .field("options", &self.options)
            .finish_non_exhaustive()
    }
}

/// The cumulative environment-cache quota and shared text-construction balance belong to this config call.
pub(crate) struct ConfigBudget {
    limits: ConfigLimits,
    execution: ExecutionBudget,
    entries: usize,
    environment_bytes: usize,
}

impl ConfigBudget {
    pub(crate) fn new<'slot, E>(
        limits: ConfigLimits,
        execution: &ExecutionBudget,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, Self>
    where
        E: From<BudgetError>,
    {
        let execution = execution.try_clone(out.reborrow())?;
        Ok(Self {
            limits,
            execution,
            entries: 0,
            environment_bytes: 0,
        })
    }

    pub(crate) fn environment_entry<'slot, 'borrow, E>(
        &mut self,
        out: Out<'slot, 'borrow, E>,
    ) -> OResult<'slot, ()>
    where
        E: From<BudgetError>,
    {
        let Some(entries) = self
            .entries
            .checked_add(1)
            .filter(|count| *count <= self.limits.max_environment_entries())
        else {
            return Err(out.fail(E::from(self.execution.resource_error())));
        };
        self.entries = entries;
        Ok(())
    }

    pub(crate) fn environment_bytes<'slot, 'borrow, E>(
        &mut self,
        bytes: usize,
        out: Out<'slot, 'borrow, E>,
    ) -> OResult<'slot, ()>
    where
        E: From<BudgetError>,
    {
        let Some(environment_bytes) = self
            .environment_bytes
            .checked_add(bytes)
            .filter(|count| *count <= self.limits.max_environment_bytes())
        else {
            return Err(out.fail(E::from(self.execution.resource_error())));
        };
        self.environment_bytes = environment_bytes;
        Ok(())
    }

    pub(crate) const fn execution(&self) -> &ExecutionBudget {
        &self.execution
    }

    pub(crate) const fn environment_bytes_remaining(&self) -> usize {
        self.limits
            .max_environment_bytes()
            .saturating_sub(self.environment_bytes)
    }

    pub(crate) fn construct_text<'slot, 'borrow, E>(
        execution: &ExecutionBudget,
        limits: ConfigLimits,
        bytes: usize,
        out: Out<'slot, 'borrow, E>,
    ) -> OResult<'slot, ()>
    where
        E: From<BudgetError>,
    {
        execution.construct_text(bytes, limits.max_constructed_text_bytes(), out)
    }
}

/// Call-level limits for validating a value tree without source text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConfigValidationRequest {
    options: ConfigValidationOptions,
}

impl ConfigValidationRequest {
    pub const fn new(options: ConfigValidationOptions) -> Self {
        Self { options }
    }

    pub const fn options(&self) -> &ConfigValidationOptions {
        &self.options
    }
}
