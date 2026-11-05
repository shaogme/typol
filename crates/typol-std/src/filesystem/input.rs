#[cfg(test)]
mod tests;

use crate::{
    FileConfigInputLimits, HostError, HostErrorKind, HostFailureStep, HostOperation,
    filesystem::paths::{FileIdentity, NativePath, PathResolutionParams},
};
use opeo::{OResult, Out, ResultOutExt};
use std::{
    fmt,
    fs::{File, Metadata, OpenOptions},
    io::{ErrorKind, Read as IoRead, Write as IoWrite},
    path::Path,
};

use typol_core::{
    DiagnosticContext, ExecutionBudget, Read as SourceRead, SourceId, SourceReadError,
    SourceReadErrorKind, Write, WriteError, diagnostic_context,
};

#[cfg(target_os = "linux")]
use std::os::unix::fs::OpenOptionsExt;

static READ_SOURCE_HOST_FAILURE: DiagnosticContext =
    diagnostic_context!("typol-std", "read-source", "host-reader-call",);
static READ_SOURCE_OFFSET_OVERFLOW: DiagnosticContext =
    diagnostic_context!("typol-std", "read-source", "read-offset-overflow",);
static READ_SOURCE_COUNT_INVARIANT: DiagnosticContext =
    diagnostic_context!("typol-std", "read-source", "read-count-exceeds-request",);
static FILE_CONFIG_INPUT_INVARIANT: DiagnosticContext =
    diagnostic_context!("typol-std", "file-config-input", "stream-state-invariant",);
static FILE_CONFIG_INPUT_LIMIT: DiagnosticContext =
    diagnostic_context!("typol-std", "file-config-input", "source-byte-limit",);
static FILE_SCHEMA_INPUT_INVARIANT: DiagnosticContext =
    diagnostic_context!("typol-std", "file-schema-input", "stream-state-invariant",);
static FILE_SCHEMA_INPUT_LIMIT: DiagnosticContext =
    diagnostic_context!("typol-std", "file-schema-input", "source-byte-limit",);

#[cfg(target_os = "linux")]
const LINUX_OPEN_NONBLOCK: i32 = 0o4000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SourceReadState {
    Reading,
    Finished,
    Failed,
}

#[derive(Clone, Copy)]
enum LimitedReadFailure {
    LimitExceeded {
        offset: usize,
        requested_bytes: usize,
        consumed_bytes: usize,
    },
    InvariantFailure {
        offset: usize,
        requested_bytes: usize,
    },
}

impl LimitedReadFailure {
    fn into_source_error(
        self,
        source_id: SourceId,
        input_offset: usize,
        limit_context: DiagnosticContext,
        invariant_context: DiagnosticContext,
    ) -> SourceReadError {
        match self {
            Self::LimitExceeded {
                offset,
                requested_bytes,
                consumed_bytes,
            } => SourceReadError::new(SourceReadErrorKind::ResourceLimitExceeded)
                .with_offset(offset)
                .with_requested_bytes(requested_bytes)
                .with_consumed_bytes(consumed_bytes)
                .with_context(limit_context)
                .with_input(source_id, input_offset),
            Self::InvariantFailure {
                offset,
                requested_bytes,
            } => SourceReadError::new(SourceReadErrorKind::InternalInvariantFailure)
                .with_offset(offset)
                .with_requested_bytes(requested_bytes)
                .with_context(invariant_context)
                .with_input(source_id, input_offset),
        }
    }
}

struct LimitedSourceState {
    limit: usize,
    fetched: usize,
    state: SourceReadState,
}

impl LimitedSourceState {
    const fn new(limit: usize) -> Self {
        Self {
            limit,
            fetched: 0,
            state: SourceReadState::Reading,
        }
    }

    const fn fetched(&self) -> usize {
        self.fetched
    }

    const fn is_finished(&self) -> bool {
        matches!(self.state, SourceReadState::Finished)
    }

    const fn is_failed(&self) -> bool {
        matches!(self.state, SourceReadState::Failed)
    }

    fn fail(&mut self) {
        self.state = SourceReadState::Failed;
    }

    fn request_size(&mut self, requested_bytes: usize) -> Result<usize, LimitedReadFailure> {
        let Some(remaining) = self.limit.checked_sub(self.fetched) else {
            self.fail();
            return Err(LimitedReadFailure::InvariantFailure {
                offset: self.fetched,
                requested_bytes,
            });
        };
        Ok(remaining.min(requested_bytes).max(1))
    }

    fn record(
        &mut self,
        requested_bytes: usize,
        consumed_bytes: usize,
    ) -> Result<(), LimitedReadFailure> {
        let offset = self.fetched;
        let Some(remaining) = self.limit.checked_sub(offset) else {
            self.fail();
            return Err(LimitedReadFailure::InvariantFailure {
                offset,
                requested_bytes,
            });
        };
        if consumed_bytes > requested_bytes {
            self.fail();
            return Err(LimitedReadFailure::InvariantFailure {
                offset,
                requested_bytes,
            });
        }
        if consumed_bytes > remaining {
            self.fail();
            return Err(LimitedReadFailure::LimitExceeded {
                offset,
                requested_bytes,
                consumed_bytes,
            });
        }
        if consumed_bytes == 0 {
            self.state = SourceReadState::Finished;
            return Ok(());
        }
        let Some(fetched) = offset
            .checked_add(consumed_bytes)
            .filter(|fetched| *fetched <= self.limit)
        else {
            self.fail();
            return Err(LimitedReadFailure::InvariantFailure {
                offset,
                requested_bytes,
            });
        };
        self.fetched = fetched;
        Ok(())
    }
}

pub(crate) struct FileSchemaInput {
    file: File,
    budget: ExecutionBudget,
    progress: LimitedSourceState,
    issued: bool,
}

pub(crate) struct OpenedSchemaFile {
    file: File,
    metadata: Metadata,
}

struct SchemaInputParams<'budget> {
    limit: usize,
    budget: &'budget ExecutionBudget,
    operation: HostOperation,
}

impl OpenedSchemaFile {
    pub(crate) fn open<'slot>(
        path: &Path,
        operation: HostOperation,
        mut out: Out<'slot, '_, HostError>,
    ) -> OResult<'slot, Self> {
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(target_os = "linux")]
        options.custom_flags(LINUX_OPEN_NONBLOCK);
        let file = options
            .open(path)
            .map_err(|error| HostError::io_at(operation, error.kind(), HostFailureStep::FileOpen))
            .store_err_into(out.reborrow())?;
        let metadata = file
            .metadata()
            .map_err(|error| {
                HostError::io_at(operation, error.kind(), HostFailureStep::FileMetadata)
            })
            .store_err_into(out.reborrow())?;
        if !metadata.is_file() {
            return Err(out.fail(HostError::with_step(
                operation,
                HostErrorKind::NotRegularFile,
                HostFailureStep::FileMetadata,
            )));
        }
        Ok(Self { file, metadata })
    }

    pub(crate) fn identity(&self) -> Option<FileIdentity> {
        NativePath::file_identity(&self.file, &self.metadata)
    }

    pub(crate) fn handle(&self) -> &File {
        &self.file
    }

    pub(crate) fn into_parts(self) -> (File, Metadata) {
        (self.file, self.metadata)
    }

    pub(crate) fn clone_handle<'slot>(
        &self,
        operation: HostOperation,
        mut out: Out<'slot, '_, HostError>,
    ) -> OResult<'slot, File> {
        self.file
            .try_clone()
            .map_err(|error| HostError::io_at(operation, error.kind(), HostFailureStep::FileOpen))
            .store_err_into(out.reborrow())
    }

    pub(crate) fn into_input_and_pin<'slot>(
        self,
        limit: usize,
        budget: &ExecutionBudget,
        operation: HostOperation,
        mut out: Out<'slot, '_, HostError>,
    ) -> OResult<'slot, (FileSchemaInput, File)> {
        let pin = self.clone_handle(operation, out.reborrow())?;
        let input = FileSchemaInput::from_opened_metadata(
            self.file,
            self.metadata,
            SchemaInputParams {
                limit,
                budget,
                operation,
            },
            out.reborrow(),
        )?;
        Ok((input, pin))
    }
}

impl FileSchemaInput {
    #[cfg(test)]
    pub(crate) fn open<'slot>(
        path: &Path,
        limit: usize,
        budget: &ExecutionBudget,
        operation: HostOperation,
        mut out: Out<'slot, '_, HostError>,
    ) -> OResult<'slot, Self> {
        let opened = OpenedSchemaFile::open(path, operation, out.reborrow())?;
        let (file, info) = opened.into_parts();
        Self::from_opened_metadata(
            file,
            info,
            SchemaInputParams {
                limit,
                budget,
                operation,
            },
            out,
        )
    }

    fn from_opened_metadata<'slot>(
        file: File,
        info: Metadata,
        params: SchemaInputParams<'_>,
        mut out: Out<'slot, '_, HostError>,
    ) -> OResult<'slot, Self> {
        let SchemaInputParams {
            limit,
            budget,
            operation,
        } = params;
        budget.consume_with(1, out.reborrow(), |error| {
            HostError::budget(operation, error)
        })?;
        if !info.is_file() {
            return Err(out.fail(HostError::with_step(
                operation,
                HostErrorKind::NotRegularFile,
                HostFailureStep::FileMetadata,
            )));
        }
        if info.len() > u64::try_from(limit).unwrap_or(u64::MAX) {
            return Err(out.fail(HostError::with_step(
                operation,
                HostErrorKind::ResourceLimitExceeded,
                HostFailureStep::FileMetadata,
            )));
        }
        let budget =
            budget.try_clone_with(out.reborrow(), |error| HostError::budget(operation, error))?;
        Ok(Self {
            file,
            budget,
            progress: LimitedSourceState::new(limit),
            issued: false,
        })
    }

    pub(crate) fn claim(&mut self) {
        if self.issued {
            self.progress.fail();
        } else {
            self.issued = true;
        }
    }
}

impl SourceRead for FileSchemaInput {
    fn read<'slot>(
        &mut self,
        output: &mut [u8],
        source_id: SourceId,
        offset: usize,
        mut out: Out<'slot, '_, SourceReadError>,
    ) -> OResult<'slot, usize> {
        if self.progress.is_failed() || !self.issued {
            return Err(out.fail(
                SourceReadError::new(SourceReadErrorKind::InternalInvariantFailure)
                    .with_context(FILE_SCHEMA_INPUT_INVARIANT)
                    .with_input(source_id, offset),
            ));
        }
        if self.progress.is_finished() || output.is_empty() {
            return Ok(0);
        }
        let size = self
            .progress
            .request_size(output.len())
            .map_err(|failure| {
                failure.into_source_error(
                    source_id,
                    offset,
                    FILE_SCHEMA_INPUT_LIMIT,
                    FILE_SCHEMA_INPUT_INVARIANT,
                )
            })
            .store_err_into(out.reborrow())?;
        let Some(buffer) = output.get_mut(..size) else {
            self.progress.fail();
            return Err(out.fail(
                SourceReadError::new(SourceReadErrorKind::InternalInvariantFailure)
                    .with_context(FILE_SCHEMA_INPUT_INVARIANT)
                    .with_input(source_id, offset),
            ));
        };
        let mut source = ReadSource::new(&mut self.file, &self.budget, out.reborrow())?;
        let count = source.read(buffer, source_id, offset, out.reborrow())?;
        self.progress
            .record(size, count)
            .map_err(|failure| {
                failure.into_source_error(
                    source_id,
                    offset,
                    FILE_SCHEMA_INPUT_LIMIT,
                    FILE_SCHEMA_INPUT_INVARIANT,
                )
            })
            .store_err_into(out.reborrow())?;
        Ok(count)
    }
}

/// No cache is added; each interrupted-read retry is charged, and other native errors map only to finite categories.
pub struct ReadSource<'input> {
    reader: &'input mut dyn IoRead,
    budget: ExecutionBudget,
    finished: bool,
}

impl<'input> ReadSource<'input> {
    pub fn new<'slot>(
        reader: &'input mut dyn IoRead,
        budget: &ExecutionBudget,
        mut out: Out<'slot, '_, SourceReadError>,
    ) -> OResult<'slot, Self> {
        let budget = budget.try_clone(out.reborrow())?;
        Ok(Self {
            reader,
            budget,
            finished: false,
        })
    }
}

impl SourceRead for ReadSource<'_> {
    fn read<'slot>(
        &mut self,
        output: &mut [u8],
        source_id: SourceId,
        offset: usize,
        mut out: Out<'slot, '_, SourceReadError>,
    ) -> OResult<'slot, usize> {
        if self.finished || output.is_empty() {
            return Ok(0);
        }
        let requested_bytes = output.len();
        loop {
            self.budget.consume_with(1, out.reborrow(), |error| {
                SourceReadError::from(error)
                    .with_input(source_id, offset)
                    .with_requested_bytes(requested_bytes)
            })?;
            match self.reader.read(output) {
                Ok(count) => {
                    if count > requested_bytes {
                        return Err(out.fail(
                            SourceReadError::new(SourceReadErrorKind::InternalInvariantFailure)
                                .with_input(source_id, offset)
                                .with_requested_bytes(requested_bytes)
                                .with_context(READ_SOURCE_COUNT_INVARIANT),
                        ));
                    }
                    self.finished = count == 0;
                    let _ = offset
                        .checked_add(count)
                        .ok_or_else(|| {
                            SourceReadError::new(SourceReadErrorKind::InternalInvariantFailure)
                                .with_input(source_id, offset)
                                .with_requested_bytes(requested_bytes)
                                .with_context(READ_SOURCE_OFFSET_OVERFLOW)
                        })
                        .store_err_into(out.reborrow())?;
                    return Ok(count);
                }
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(_) => {
                    return Err(out.fail(
                        SourceReadError::new(SourceReadErrorKind::HostFailure)
                            .with_input(source_id, offset)
                            .with_requested_bytes(requested_bytes)
                            .with_context(READ_SOURCE_HOST_FAILURE),
                    ));
                }
            }
        }
    }
}

impl fmt::Debug for ReadSource<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("ReadSource").finish_non_exhaustive()
    }
}

pub struct IoWriteSink<'sink> {
    writer: &'sink mut dyn IoWrite,
}

impl<'sink> IoWriteSink<'sink> {
    pub const fn new(writer: &'sink mut dyn IoWrite) -> Self {
        Self { writer }
    }
}

impl Write for IoWriteSink<'_> {
    fn write_all(&mut self, mut bytes: &[u8]) -> Result<(), WriteError> {
        while !bytes.is_empty() {
            match self.writer.write(bytes) {
                Ok(0) => return Err(WriteError::HostFailure),
                Ok(count) if count <= bytes.len() => {
                    bytes = bytes
                        .get(count..)
                        .ok_or(WriteError::InternalInvariantFailure)?;
                }
                Ok(_) => return Err(WriteError::InternalInvariantFailure),
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(_) => return Err(WriteError::HostFailure),
            }
        }
        Ok(())
    }
}

/// Opens and checks a handle while the lexer pulls contents on demand; file-size and path-label limits constrain accepted data, not internal system-call allocations.
pub struct FileConfigInput {
    file: File,
    path: NativePath,
    budget: ExecutionBudget,
    progress: LimitedSourceState,
}

impl FileConfigInput {
    pub fn open<'slot>(
        path: &Path,
        limits: FileConfigInputLimits,
        source_byte_limit: usize,
        budget: &ExecutionBudget,
        mut out: Out<'slot, '_, HostError>,
    ) -> OResult<'slot, Self> {
        let operation = HostOperation::ConfigRead;
        budget.consume_with(1, out.reborrow(), |error| {
            HostError::budget(operation, error)
        })?;
        let opened = OpenedSchemaFile::open(path, operation, out.reborrow())?;
        budget.consume_with(1, out.reborrow(), |error| {
            HostError::budget(operation, error)
        })?;
        let display_path = NativePath::label_for_opened_file(
            path,
            opened.handle(),
            PathResolutionParams {
                max_path_bytes: limits.max_path_bytes(),
                budget,
                operation,
            },
            out.reborrow(),
        )?;
        let (file, info) = opened.into_parts();
        let limit = source_byte_limit.min(limits.max_file_bytes());
        if info.len() > u64::try_from(limit).unwrap_or(u64::MAX) {
            return Err(out.fail(HostError::with_step(
                operation,
                HostErrorKind::ResourceLimitExceeded,
                HostFailureStep::FileMetadata,
            )));
        }
        let budget =
            budget.try_clone_with(out.reborrow(), |error| HostError::budget(operation, error))?;
        Ok(Self {
            file,
            path: display_path,
            budget,
            progress: LimitedSourceState::new(limit),
        })
    }

    /// Returns a path label for display by the caller; the label is not guaranteed to identify the file actually opened.
    ///
    /// Returns the path label for callers to display; it does not guarantee the identity of the opened file.
    pub fn path(&self) -> &Path {
        self.path.path()
    }
}

impl SourceRead for FileConfigInput {
    fn read<'slot>(
        &mut self,
        output: &mut [u8],
        source_id: SourceId,
        offset: usize,
        mut out: Out<'slot, '_, SourceReadError>,
    ) -> OResult<'slot, usize> {
        if self.progress.is_failed() {
            return Err(out.fail(
                file_input_error(self.progress.fetched(), output.len())
                    .with_input(source_id, offset),
            ));
        }
        if self.progress.is_finished() || output.is_empty() {
            return Ok(0);
        }
        let size = self
            .progress
            .request_size(output.len())
            .map_err(|failure| {
                failure.into_source_error(
                    source_id,
                    offset,
                    FILE_CONFIG_INPUT_LIMIT,
                    FILE_CONFIG_INPUT_INVARIANT,
                )
            })
            .store_err_into(out.reborrow())?;
        let fetched = self.progress.fetched();
        let mut source = ReadSource::new(&mut self.file, &self.budget, out.reborrow())?;
        let Some(buffer) = output.get_mut(..size) else {
            self.progress.fail();
            return Err(out.fail(file_input_error(fetched, size).with_input(source_id, offset)));
        };
        let count = source.read(buffer, source_id, offset, out.reborrow())?;
        self.progress
            .record(size, count)
            .map_err(|failure| {
                failure.into_source_error(
                    source_id,
                    offset,
                    FILE_CONFIG_INPUT_LIMIT,
                    FILE_CONFIG_INPUT_INVARIANT,
                )
            })
            .store_err_into(out.reborrow())?;
        Ok(count)
    }
}

fn file_input_error(offset: usize, requested_bytes: usize) -> SourceReadError {
    SourceReadError::new(SourceReadErrorKind::InternalInvariantFailure)
        .with_offset(offset)
        .with_requested_bytes(requested_bytes)
        .with_context(FILE_CONFIG_INPUT_INVARIANT)
}

impl fmt::Debug for FileConfigInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FileConfigInput")
            .field("fetched_bytes", &self.progress.fetched())
            .field("state", &self.progress.state)
            .finish_non_exhaustive()
    }
}
