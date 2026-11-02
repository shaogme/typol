use typol_core::ExecutionLimits;

/// Limits accepted schema file count, file-content bytes, path bytes, and module-name bytes.
/// The path limit does not bound temporary allocations inside `canonicalize` on non-Linux systems or when procfs is unavailable.
#[derive(Clone, Copy, Debug)]
pub struct FileSchemaLimits {
    max_file_bytes: usize,
    max_files: usize,
    max_path_bytes: usize,
    max_name_bytes: usize,
}

pub struct FileSchemaLimitParams {
    pub max_file_bytes: usize,
    pub max_files: usize,
    pub max_path_bytes: usize,
    pub max_name_bytes: usize,
}

impl FileSchemaLimits {
    pub const fn new(params: FileSchemaLimitParams) -> Self {
        Self {
            max_file_bytes: params.max_file_bytes,
            max_files: params.max_files,
            max_path_bytes: params.max_path_bytes,
            max_name_bytes: params.max_name_bytes,
        }
    }

    pub const fn max_file_bytes(&self) -> usize {
        self.max_file_bytes
    }

    pub const fn max_files(&self) -> usize {
        self.max_files
    }

    pub const fn max_path_bytes(&self) -> usize {
        self.max_path_bytes
    }

    pub const fn max_name_bytes(&self) -> usize {
        self.max_name_bytes
    }

    pub const fn with_max_file_bytes(mut self, value: usize) -> Self {
        self.max_file_bytes = value;
        self
    }

    pub const fn with_max_files(mut self, value: usize) -> Self {
        self.max_files = value;
        self
    }

    pub const fn with_max_path_bytes(mut self, value: usize) -> Self {
        self.max_path_bytes = value;
        self
    }

    pub const fn with_max_name_bytes(mut self, value: usize) -> Self {
        self.max_name_bytes = value;
        self
    }
}

impl Default for FileSchemaLimits {
    fn default() -> Self {
        Self::new(FileSchemaLimitParams {
            max_file_bytes: 1_048_576,
            max_files: 64,
            max_path_bytes: 16_384,
            max_name_bytes: 4096,
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SourceNameLimits {
    max_files: usize,
    max_name_bytes: usize,
}

impl SourceNameLimits {
    pub const fn new(max_files: usize, max_name_bytes: usize) -> Self {
        Self {
            max_files,
            max_name_bytes,
        }
    }

    pub const fn max_files(&self) -> usize {
        self.max_files
    }

    pub const fn max_name_bytes(&self) -> usize {
        self.max_name_bytes
    }

    pub const fn with_max_files(mut self, value: usize) -> Self {
        self.max_files = value;
        self
    }

    pub const fn with_max_name_bytes(mut self, value: usize) -> Self {
        self.max_name_bytes = value;
        self
    }
}

impl Default for SourceNameLimits {
    fn default() -> Self {
        Self::new(64, 4096)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SourceNameEditOptions {
    limits: SourceNameLimits,
    execution: ExecutionLimits,
}

impl SourceNameEditOptions {
    pub const fn new(limits: SourceNameLimits, execution: ExecutionLimits) -> Self {
        Self { limits, execution }
    }

    pub const fn limits(&self) -> SourceNameLimits {
        self.limits
    }

    pub const fn execution(&self) -> ExecutionLimits {
        self.execution
    }

    pub const fn with_limits(mut self, limits: SourceNameLimits) -> Self {
        self.limits = limits;
        self
    }

    pub const fn with_execution(mut self, execution: ExecutionLimits) -> Self {
        self.execution = execution;
        self
    }
}

impl Default for SourceNameEditOptions {
    fn default() -> Self {
        Self::new(
            SourceNameLimits::default(),
            ExecutionLimits::new(16_777_216, 16_777_216),
        )
    }
}
