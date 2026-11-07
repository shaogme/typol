use std::{error::Error, fmt, io::ErrorKind};

use opeo::{ErrSlot, OResult, Out};
use typol_core::{
    BudgetError, DiagnosticContext, SourceId, SourceReadError, SourceReadErrorKind,
    diagnostic_context,
};
use typol_i18n::{Language, Translator};

use crate::{DiagnosticRenderError, DiagnosticRenderer, RenderOptions};

static HOST_REQUEST_CONTEXT: DiagnosticContext =
    diagnostic_context!("typol-std", "host", "request-validation",);
static HOST_BUDGET_CONTEXT: DiagnosticContext =
    diagnostic_context!("typol-std", "host", "budget-accounting",);
static HOST_BUDGET_INVARIANT_CONTEXT: DiagnosticContext =
    diagnostic_context!("typol-std", "host", "budget-invariant",);
static HOST_PATH_CONTEXT: DiagnosticContext =
    diagnostic_context!("typol-std", "filesystem-path", "path-processing",);
static HOST_FILE_METADATA_CONTEXT: DiagnosticContext =
    diagnostic_context!("typol-std", "filesystem-read", "metadata-check",);
static HOST_FILE_OPEN_CONTEXT: DiagnosticContext =
    diagnostic_context!("typol-std", "filesystem-read", "file-open",);
static HOST_FILE_READ_CONTEXT: DiagnosticContext =
    diagnostic_context!("typol-std", "filesystem-read", "stream-read",);
static HOST_FILE_BUFFER_CONTEXT: DiagnosticContext =
    diagnostic_context!("typol-std", "filesystem-read", "read-buffer-invariant",);
static HOST_FILE_DECODE_CONTEXT: DiagnosticContext =
    diagnostic_context!("typol-std", "filesystem-read", "utf8-validation",);
static HOST_SOURCE_NAMES_CONTEXT: DiagnosticContext =
    diagnostic_context!("typol-std", "source-names", "name-table-operation",);
static HOST_SOURCE_NAMES_FORMAT_CONTEXT: DiagnosticContext =
    diagnostic_context!("typol-std", "source-names", "source-id-name-formatting",);
static HOST_SOURCE_OBSERVER_CONTEXT: DiagnosticContext =
    diagnostic_context!("typol-std", "source-names", "observer-session-validation",);
static HOST_SOURCE_READ_CONTEXT: DiagnosticContext =
    diagnostic_context!("typol-std", "source-read", "source-read-call",);
static HOST_SOURCE_LIMIT_CONTEXT: DiagnosticContext =
    diagnostic_context!("typol-std", "source-read", "source-byte-limit",);
static HOST_SOURCE_ALLOCATION_CONTEXT: DiagnosticContext =
    diagnostic_context!("typol-std", "source-read", "source-buffer-allocation",);
static HOST_SOURCE_CAPACITY_CONTEXT: DiagnosticContext =
    diagnostic_context!("typol-std", "source-read", "source-buffer-capacity",);
static HOST_SOURCE_INVARIANT_CONTEXT: DiagnosticContext =
    diagnostic_context!("typol-std", "source-read", "source-read-invariant",);

/// Finite failure stages retained by the host adapter; paths and native error details are not included.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostFailureStep {
    RequestValidation,
    BudgetAccounting,
    BudgetInvariant,
    PathProcessing,
    FileMetadata,
    FileOpen,
    FileRead,
    FileBufferInvariant,
    FileUtf8Validation,
    SourceNameOperation,
    SourceNameFormatting,
    SourceObserverValidation,
    SourceRead,
    SourceResourceLimit,
    SourceAllocation,
    SourceCapacity,
    SourceInvariant,
}

impl HostFailureStep {
    const fn context_ref(self) -> &'static DiagnosticContext {
        match self {
            Self::RequestValidation => &HOST_REQUEST_CONTEXT,
            Self::BudgetAccounting => &HOST_BUDGET_CONTEXT,
            Self::BudgetInvariant => &HOST_BUDGET_INVARIANT_CONTEXT,
            Self::PathProcessing => &HOST_PATH_CONTEXT,
            Self::FileMetadata => &HOST_FILE_METADATA_CONTEXT,
            Self::FileOpen => &HOST_FILE_OPEN_CONTEXT,
            Self::FileRead => &HOST_FILE_READ_CONTEXT,
            Self::FileBufferInvariant => &HOST_FILE_BUFFER_CONTEXT,
            Self::FileUtf8Validation => &HOST_FILE_DECODE_CONTEXT,
            Self::SourceNameOperation => &HOST_SOURCE_NAMES_CONTEXT,
            Self::SourceNameFormatting => &HOST_SOURCE_NAMES_FORMAT_CONTEXT,
            Self::SourceObserverValidation => &HOST_SOURCE_OBSERVER_CONTEXT,
            Self::SourceRead => &HOST_SOURCE_READ_CONTEXT,
            Self::SourceResourceLimit => &HOST_SOURCE_LIMIT_CONTEXT,
            Self::SourceAllocation => &HOST_SOURCE_ALLOCATION_CONTEXT,
            Self::SourceCapacity => &HOST_SOURCE_CAPACITY_CONTEXT,
            Self::SourceInvariant => &HOST_SOURCE_INVARIANT_CONTEXT,
        }
    }

    const fn context(self) -> DiagnosticContext {
        *self.context_ref()
    }
}

/// Maximum accepted file-content and path-label bytes; this does not bound temporary allocations inside standard-library or operating-system path handling.
#[derive(Clone, Copy, Debug)]
pub struct FileConfigInputLimits {
    max_file_bytes: usize,
    max_path_bytes: usize,
}

pub struct FileConfigInputLimitParams {
    pub max_file_bytes: usize,
    pub max_path_bytes: usize,
}

impl FileConfigInputLimits {
    pub const fn new(params: FileConfigInputLimitParams) -> Self {
        Self {
            max_file_bytes: params.max_file_bytes,
            max_path_bytes: params.max_path_bytes,
        }
    }

    pub const fn max_file_bytes(&self) -> usize {
        self.max_file_bytes
    }

    pub const fn max_path_bytes(&self) -> usize {
        self.max_path_bytes
    }

    pub const fn with_max_file_bytes(mut self, value: usize) -> Self {
        self.max_file_bytes = value;
        self
    }

    pub const fn with_max_path_bytes(mut self, value: usize) -> Self {
        self.max_path_bytes = value;
        self
    }
}

impl Default for FileConfigInputLimits {
    fn default() -> Self {
        Self::new(FileConfigInputLimitParams {
            max_file_bytes: 1_048_576,
            max_path_bytes: 16_384,
        })
    }
}

/// Maximum accepted process-environment name and value bytes; the value limit is checked after `var_os` returns an owned value and cannot bound that query's temporary allocation.
#[derive(Clone, Copy, Debug)]
pub struct ProcessEnvironmentLimits {
    max_environment_name_bytes: usize,
    max_environment_value_bytes: usize,
}

pub struct ProcessEnvironmentLimitParams {
    pub max_environment_name_bytes: usize,
    pub max_environment_value_bytes: usize,
}

impl ProcessEnvironmentLimits {
    pub const fn new(params: ProcessEnvironmentLimitParams) -> Self {
        Self {
            max_environment_name_bytes: params.max_environment_name_bytes,
            max_environment_value_bytes: params.max_environment_value_bytes,
        }
    }

    pub const fn max_environment_name_bytes(&self) -> usize {
        self.max_environment_name_bytes
    }

    pub const fn max_environment_value_bytes(&self) -> usize {
        self.max_environment_value_bytes
    }

    pub const fn with_max_environment_name_bytes(mut self, value: usize) -> Self {
        self.max_environment_name_bytes = value;
        self
    }

    pub const fn with_max_environment_value_bytes(mut self, value: usize) -> Self {
        self.max_environment_value_bytes = value;
        self
    }
}

impl Default for ProcessEnvironmentLimits {
    fn default() -> Self {
        Self::new(ProcessEnvironmentLimitParams {
            max_environment_name_bytes: 4096,
            max_environment_value_bytes: 1_048_576,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostOperation {
    SchemaRead,
    ConfigRead,
    SourceNames,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostErrorKind {
    NotFound,
    PermissionDenied,
    NotRegularFile,
    InvalidRequest,
    ReadFailure,
    ResourceLimitExceeded,
    AllocationFailed,
    CapacityExceeded,
    InternalError,
}

/// Owns only finite categories and does not retain paths, input, or raw native error text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HostError {
    operation: HostOperation,
    kind: HostErrorKind,
    step: HostFailureStep,
    source_id: Option<SourceId>,
    source_context: Option<&'static DiagnosticContext>,
    input_context: Option<DiagnosticContext>,
    offset: Option<usize>,
}

impl HostError {
    pub(crate) const fn new(operation: HostOperation, kind: HostErrorKind) -> Self {
        Self::with_step(operation, kind, HostFailureStep::RequestValidation)
    }

    pub(crate) const fn with_step(
        operation: HostOperation,
        kind: HostErrorKind,
        step: HostFailureStep,
    ) -> Self {
        Self {
            operation,
            kind,
            step,
            source_id: None,
            source_context: None,
            input_context: None,
            offset: None,
        }
    }

    pub(crate) const fn internal(operation: HostOperation, step: HostFailureStep) -> Self {
        Self::with_step(operation, HostErrorKind::InternalError, step)
    }

    pub(crate) const fn with_source_id(mut self, source_id: SourceId) -> Self {
        self.source_id = Some(source_id);
        self
    }

    pub const fn operation(&self) -> HostOperation {
        self.operation
    }

    pub const fn kind(&self) -> HostErrorKind {
        self.kind
    }

    pub const fn failure_step(&self) -> HostFailureStep {
        self.step
    }

    pub const fn budget_error(&self) -> Option<BudgetError> {
        match self.step {
            HostFailureStep::BudgetAccounting => match self.kind {
                HostErrorKind::ResourceLimitExceeded => Some(BudgetError::Resource),
                HostErrorKind::AllocationFailed => Some(BudgetError::Allocation),
                HostErrorKind::CapacityExceeded => Some(BudgetError::CapacityExceeded),
                _ => None,
            },
            HostFailureStep::BudgetInvariant => Some(BudgetError::invariant(self.budget_context())),
            _ => None,
        }
    }

    pub const fn source_id(&self) -> Option<SourceId> {
        self.source_id
    }

    pub const fn offset(&self) -> Option<usize> {
        self.offset
    }

    pub const fn context(&self) -> DiagnosticContext {
        if let Some(context) = self.input_context {
            return context;
        }
        match self.source_id {
            Some(source_id) => self.step.context().with_object_id(source_id.get() as u64),
            None => self.step.context(),
        }
    }

    pub const fn source_context(&self) -> Option<DiagnosticContext> {
        match self.source_context {
            Some(context) => Some(*context),
            None => self.input_context,
        }
    }

    pub(crate) const fn budget_context(&self) -> &'static DiagnosticContext {
        match self.source_context {
            Some(context) => context,
            None => self.step.context_ref(),
        }
    }

    /// The name copy and load-error slot have separate bounded budgets; errors retain only safe categories.
    pub fn from_budget(error: BudgetError) -> Self {
        Self::budget(HostOperation::SourceNames, error)
    }

    pub fn budget(operation: HostOperation, error: BudgetError) -> Self {
        let (step, source_context) = match error {
            BudgetError::InvariantViolation(context) => {
                (HostFailureStep::BudgetInvariant, Some(context))
            }
            BudgetError::Resource
            | BudgetError::RegexSteps
            | BudgetError::RegexSearchSteps
            | BudgetError::RegexWorkspace
            | BudgetError::Allocation
            | BudgetError::CapacityExceeded => (HostFailureStep::BudgetAccounting, None),
        };
        let mut host_error = Self::with_step(
            operation,
            match error {
                BudgetError::Resource
                | BudgetError::RegexSteps
                | BudgetError::RegexSearchSteps
                | BudgetError::RegexWorkspace => HostErrorKind::ResourceLimitExceeded,
                BudgetError::Allocation => HostErrorKind::AllocationFailed,
                BudgetError::CapacityExceeded => HostErrorKind::CapacityExceeded,
                BudgetError::InvariantViolation(_) => HostErrorKind::InternalError,
            },
            step,
        );
        host_error.source_context = source_context;
        host_error
    }

    /// Maps a byte-source read error to a safe host category while retaining its source location and context.
    pub fn from_source_read_error(operation: HostOperation, error: SourceReadError) -> Self {
        let (kind, step) = match error.kind() {
            SourceReadErrorKind::HostFailure => {
                (HostErrorKind::ReadFailure, HostFailureStep::SourceRead)
            }
            SourceReadErrorKind::ResourceLimitExceeded => (
                HostErrorKind::ResourceLimitExceeded,
                HostFailureStep::SourceResourceLimit,
            ),
            SourceReadErrorKind::AllocationFailed => (
                HostErrorKind::AllocationFailed,
                HostFailureStep::SourceAllocation,
            ),
            SourceReadErrorKind::CapacityExceeded => (
                HostErrorKind::CapacityExceeded,
                HostFailureStep::SourceCapacity,
            ),
            SourceReadErrorKind::InternalInvariantFailure => (
                HostErrorKind::InternalError,
                HostFailureStep::SourceInvariant,
            ),
        };
        let mut host_error = Self::with_step(operation, kind, step);
        host_error.source_id = error.source_id();
        host_error.offset = error.offset();
        host_error.input_context = error.context().or_else(|| {
            (error.kind() == SourceReadErrorKind::InternalInvariantFailure)
                .then_some(HOST_SOURCE_INVARIANT_CONTEXT)
        });
        host_error
    }

    pub(crate) fn run_budgeted<'host, T>(
        operation: HostOperation,
        source_id: Option<SourceId>,
        out: Out<'host, '_, HostError>,
        action: impl for<'slot> FnOnce(Out<'slot, 'slot, BudgetError>) -> OResult<'slot, T>,
    ) -> OResult<'host, T> {
        let mut errors = ErrSlot::<BudgetError>::new();
        match errors.try_call(action) {
            Ok(value) => Ok(value),
            Err(caught) => {
                let error = Self::budget(operation, caught.take());
                Err(out.fail(match source_id {
                    Some(source_id) => error.with_source_id(source_id),
                    None => error,
                }))
            }
        }
    }

    pub(crate) fn io_at(operation: HostOperation, kind: ErrorKind, step: HostFailureStep) -> Self {
        Self::with_step(
            operation,
            match kind {
                ErrorKind::NotFound => HostErrorKind::NotFound,
                ErrorKind::PermissionDenied => HostErrorKind::PermissionDenied,
                _ => HostErrorKind::ReadFailure,
            },
            step,
        )
    }
}

impl fmt::Display for HostError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut errors = ErrSlot::<DiagnosticRenderError>::new();
        match errors.try_call(|out| {
            DiagnosticRenderer::render_host(
                self,
                RenderOptions::default(),
                Translator::new(Language::English),
                out,
            )
        }) {
            Ok(rendered) => formatter.write_str(&rendered),
            Err(_) => formatter.write_str("host operation failed"),
        }
    }
}

impl Error for HostError {}
