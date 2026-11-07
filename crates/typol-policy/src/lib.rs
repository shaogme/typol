#![no_std]

pub use typol_core::ExcerptLimits;

/// Shared tree limits for configuration values and semantic evaluation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ValueLimits {
    max_nesting_depth: usize,
    max_string_bytes: usize,
    max_value_nodes: usize,
}

impl ValueLimits {
    pub const fn new(
        max_nesting_depth: usize,
        max_string_bytes: usize,
        max_value_nodes: usize,
    ) -> Self {
        Self {
            max_nesting_depth,
            max_string_bytes,
            max_value_nodes,
        }
    }

    pub const fn max_nesting_depth(&self) -> usize {
        self.max_nesting_depth
    }

    /// The root value has depth 0, and a depth equal to the limit is allowed.
    pub const fn allows_depth(&self, depth: usize) -> bool {
        depth <= self.max_nesting_depth
    }

    pub const fn max_string_bytes(&self) -> usize {
        self.max_string_bytes
    }

    pub const fn max_value_nodes(&self) -> usize {
        self.max_value_nodes
    }

    pub fn set_max_nesting_depth(&mut self, value: usize) {
        self.max_nesting_depth = value;
    }

    pub fn set_max_string_bytes(&mut self, value: usize) {
        self.max_string_bytes = value;
    }

    pub fn set_max_value_nodes(&mut self, value: usize) {
        self.max_value_nodes = value;
    }
}

impl Default for ValueLimits {
    fn default() -> Self {
        Self {
            max_nesting_depth: 128,
            max_string_bytes: 262_144,
            max_value_nodes: 100_000,
        }
    }
}

/// Resource ceilings used by regular-expression compilation and matching.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RegexLimits {
    max_regex_pattern_bytes: usize,
    max_regex_total_pattern_bytes: usize,
    max_regex_count: usize,
    max_regex_nesting_depth: usize,
    max_regex_instructions_per_pattern: usize,
    max_regex_ranges_per_pattern: usize,
    max_regex_instructions_total: usize,
    max_regex_ranges_total: usize,
    max_regex_scratch_states: usize,
}

impl RegexLimits {
    pub const fn max_regex_pattern_bytes(&self) -> usize {
        self.max_regex_pattern_bytes
    }

    pub const fn max_regex_total_pattern_bytes(&self) -> usize {
        self.max_regex_total_pattern_bytes
    }

    pub const fn max_regex_count(&self) -> usize {
        self.max_regex_count
    }

    pub const fn max_regex_nesting_depth(&self) -> usize {
        self.max_regex_nesting_depth
    }

    pub const fn max_regex_instructions_per_pattern(&self) -> usize {
        self.max_regex_instructions_per_pattern
    }

    pub const fn max_regex_ranges_per_pattern(&self) -> usize {
        self.max_regex_ranges_per_pattern
    }

    pub const fn max_regex_instructions_total(&self) -> usize {
        self.max_regex_instructions_total
    }

    pub const fn max_regex_ranges_total(&self) -> usize {
        self.max_regex_ranges_total
    }

    pub const fn max_regex_scratch_states(&self) -> usize {
        self.max_regex_scratch_states
    }

    pub fn set_max_regex_pattern_bytes(&mut self, value: usize) {
        self.max_regex_pattern_bytes = value;
    }

    pub fn set_max_regex_total_pattern_bytes(&mut self, value: usize) {
        self.max_regex_total_pattern_bytes = value;
    }

    pub fn set_max_regex_count(&mut self, value: usize) {
        self.max_regex_count = value;
    }

    pub fn set_max_regex_nesting_depth(&mut self, value: usize) {
        self.max_regex_nesting_depth = value;
    }

    pub fn set_max_regex_instructions_per_pattern(&mut self, value: usize) {
        self.max_regex_instructions_per_pattern = value;
    }

    pub fn set_max_regex_ranges_per_pattern(&mut self, value: usize) {
        self.max_regex_ranges_per_pattern = value;
    }

    pub fn set_max_regex_instructions_total(&mut self, value: usize) {
        self.max_regex_instructions_total = value;
    }

    pub fn set_max_regex_ranges_total(&mut self, value: usize) {
        self.max_regex_ranges_total = value;
    }

    pub fn set_max_regex_scratch_states(&mut self, value: usize) {
        self.max_regex_scratch_states = value;
    }
}

impl Default for RegexLimits {
    fn default() -> Self {
        Self {
            max_regex_pattern_bytes: 8_192,
            max_regex_total_pattern_bytes: 65_536,
            max_regex_count: 16,
            max_regex_nesting_depth: 64,
            max_regex_instructions_per_pattern: 4_096,
            max_regex_ranges_per_pattern: 4_096,
            max_regex_instructions_total: 16_384,
            max_regex_ranges_total: 16_384,
            max_regex_scratch_states: 4_096,
        }
    }
}

/// Runtime quotas for cumulative Regex work, one NFA search, and live Regex-owned capacity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RegexExecutionLimits {
    max_steps: usize,
    max_search_steps: usize,
    max_workspace_bytes: usize,
}

impl RegexExecutionLimits {
    pub const fn new(
        max_steps: usize,
        max_search_steps: usize,
        max_workspace_bytes: usize,
    ) -> Self {
        Self {
            max_steps,
            max_search_steps,
            max_workspace_bytes,
        }
    }

    pub const fn max_steps(&self) -> usize {
        self.max_steps
    }

    pub const fn max_search_steps(&self) -> usize {
        self.max_search_steps
    }

    pub const fn max_workspace_bytes(&self) -> usize {
        self.max_workspace_bytes
    }

    pub const fn with_max_steps(mut self, value: usize) -> Self {
        self.max_steps = value;
        self
    }

    pub const fn with_max_search_steps(mut self, value: usize) -> Self {
        self.max_search_steps = value;
        self
    }

    pub const fn with_max_workspace_bytes(mut self, value: usize) -> Self {
        self.max_workspace_bytes = value;
        self
    }
}

impl Default for RegexExecutionLimits {
    fn default() -> Self {
        Self::new(67_108_864, 67_108_864, 67_108_864)
    }
}

/// Structural Regex bounds and a separate runtime execution budget.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RegexOptions {
    limits: RegexLimits,
    execution: RegexExecutionLimits,
}

impl RegexOptions {
    pub const fn new(limits: RegexLimits, execution: RegexExecutionLimits) -> Self {
        Self { limits, execution }
    }

    pub const fn limits(&self) -> &RegexLimits {
        &self.limits
    }

    pub const fn execution(&self) -> &RegexExecutionLimits {
        &self.execution
    }

    pub const fn with_limits(mut self, limits: RegexLimits) -> Self {
        self.limits = limits;
        self
    }

    pub const fn with_execution(mut self, execution: RegexExecutionLimits) -> Self {
        self.execution = execution;
        self
    }
}

impl Default for RegexOptions {
    fn default() -> Self {
        Self::new(RegexLimits::default(), RegexExecutionLimits::default())
    }
}
