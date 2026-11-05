use super::{FileConfigInput, FileSchemaInput, IoWriteSink, ReadSource};
use crate::{FileConfigInputLimits, HostError, HostErrorKind, HostOperation};
use opeo::ErrSlot;
use std::{
    env::temp_dir,
    error::Error,
    fs::{OpenOptions, remove_file, rename, write},
    io::{self, ErrorKind, Read as IoRead, Write as IoWrite},
    path::{Path, PathBuf},
    process::id,
    sync::atomic::{AtomicUsize, Ordering},
};

use typol_core::{
    BudgetError, ExecutionBudget, ExecutionLimits, Read as SourceRead, SourceId, SourceReadError,
    SourceReadErrorKind, Write as SinkWrite, WriteError,
};

#[cfg(target_os = "linux")]
use std::{
    ffi::{CString, c_char, c_int},
    os::unix::ffi::OsStrExt,
};

#[cfg(target_os = "linux")]
unsafe extern "C" {
    fn mkfifo(path: *const c_char, mode: u32) -> c_int;
}

macro_rules! ensure {
    ($condition:expr $(,)?) => {
        if !$condition {
            return Err(
                io::Error::other(concat!("assertion failed: ", stringify!($condition))).into(),
            );
        }
    };
}

macro_rules! ensure_eq {
    ($left:expr, $right:expr $(,)?) => {{
        let left = &$left;
        let right = &$right;
        if left != right {
            return Err(io::Error::other(format!(
                "assertion failed: {} and {} differ (actual {:?}, expected {:?})",
                stringify!($left),
                stringify!($right),
                left,
                right,
            ))
            .into());
        }
    }};
}

static TEMP_FILE_COUNTER: AtomicUsize = AtomicUsize::new(0);

struct TempInputFile {
    path: PathBuf,
}

impl TempInputFile {
    fn new(contents: &[u8]) -> Result<Self, Box<dyn Error>> {
        let sequence = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = temp_dir().join(format!("typol-std-input-{}-{sequence}.tschem", id()));
        write(&path, contents)?;
        Ok(Self { path })
    }

    #[cfg(target_os = "linux")]
    fn fifo() -> Result<Self, Box<dyn Error>> {
        let sequence = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = temp_dir().join(format!("typol-std-input-{}-{sequence}.fifo", id()));
        let native_path = CString::new(path.as_os_str().as_bytes())?;
        // SAFETY: mkfifo receives a valid NUL-terminated path and does not retain the pointer.
        let result = unsafe { mkfifo(native_path.as_ptr(), 0o600) };
        if result != 0 {
            return Err(io::Error::last_os_error().into());
        }
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn append(&self, bytes: &[u8]) -> Result<(), Box<dyn Error>> {
        let mut file = OpenOptions::new().append(true).open(&self.path)?;
        file.write_all(bytes)?;
        Ok(())
    }
}

impl Drop for TempInputFile {
    fn drop(&mut self) {
        let _ = remove_file(&self.path);
    }
}

fn new_budget() -> Result<ExecutionBudget, BudgetError> {
    let mut errors = ErrSlot::<BudgetError>::new();
    match errors.try_call(|out| ExecutionBudget::new(ExecutionLimits::new(10_000, 10_000), out)) {
        Ok(budget) => Ok(budget),
        Err(caught) => Err(caught.take()),
    }
}

fn open_schema_input(
    path: &Path,
    limit: usize,
    budget: &ExecutionBudget,
) -> Result<FileSchemaInput, HostError> {
    let mut errors = ErrSlot::<HostError>::new();
    match errors
        .try_call(|out| FileSchemaInput::open(path, limit, budget, HostOperation::SchemaRead, out))
    {
        Ok(input) => Ok(input),
        Err(caught) => Err(caught.take()),
    }
}

fn open_config_input(path: &Path, budget: &ExecutionBudget) -> Result<FileConfigInput, HostError> {
    let mut errors = ErrSlot::<HostError>::new();
    match errors.try_call(|out| {
        FileConfigInput::open(path, FileConfigInputLimits::default(), 10, budget, out)
    }) {
        Ok(input) => Ok(input),
        Err(caught) => Err(caught.take()),
    }
}

fn read_config_input(
    input: &mut FileConfigInput,
    output: &mut [u8],
) -> Result<usize, SourceReadError> {
    let mut errors = ErrSlot::<SourceReadError>::new();
    match errors.try_call(|out| input.read(output, SourceId::new(0), 0, out)) {
        Ok(count) => Ok(count),
        Err(caught) => Err(caught.take()),
    }
}

fn read_schema_input(
    input: &mut FileSchemaInput,
    output: &mut [u8],
    offset: usize,
) -> Result<usize, SourceReadError> {
    let mut errors = ErrSlot::<SourceReadError>::new();
    match errors.try_call(|out| input.read(output, SourceId::new(0), offset, out)) {
        Ok(count) => Ok(count),
        Err(caught) => Err(caught.take()),
    }
}

fn new_read_source<'reader>(
    reader: &'reader mut dyn IoRead,
    budget: &ExecutionBudget,
) -> Result<ReadSource<'reader>, SourceReadError> {
    let mut errors = ErrSlot::<SourceReadError>::new();
    match errors.try_call(|out| ReadSource::new(reader, budget, out)) {
        Ok(source) => Ok(source),
        Err(caught) => Err(caught.take()),
    }
}

fn read_source(source: &mut ReadSource<'_>, output: &mut [u8]) -> Result<usize, SourceReadError> {
    let mut errors = ErrSlot::<SourceReadError>::new();
    match errors.try_call(|out| source.read(output, SourceId::new(0), 0, out)) {
        Ok(count) => Ok(count),
        Err(caught) => Err(caught.take()),
    }
}

struct ShortWriter {
    bytes: Vec<u8>,
    interrupted: bool,
}

impl IoWrite for ShortWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if !self.interrupted {
            self.interrupted = true;
            return Err(io::Error::from(ErrorKind::Interrupted));
        }
        let count = bytes.len().min(2);
        let written = bytes
            .get(..count)
            .ok_or_else(|| io::Error::other("write count exceeded the input slice"))?;
        self.bytes.extend_from_slice(written);
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct ZeroWriter;

impl IoWrite for ZeroWriter {
    fn write(&mut self, _bytes: &[u8]) -> io::Result<usize> {
        Ok(0)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct FailingWriter;

impl IoWrite for FailingWriter {
    fn write(&mut self, _bytes: &[u8]) -> io::Result<usize> {
        Err(io::Error::from(ErrorKind::BrokenPipe))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn io_write_sink_retries_interrupted_and_short_writes() {
    let mut writer = ShortWriter {
        bytes: Vec::new(),
        interrupted: false,
    };
    let mut sink = IoWriteSink::new(&mut writer);
    assert_eq!(SinkWrite::write_all(&mut sink, b"abcdef"), Ok(()));
    assert_eq!(writer.bytes.as_slice(), b"abcdef");
}

#[test]
fn io_write_sink_rejects_zero_progress() {
    let mut writer = ZeroWriter;
    let mut sink = IoWriteSink::new(&mut writer);
    assert_eq!(
        SinkWrite::write_all(&mut sink, b"data"),
        Err(WriteError::HostFailure)
    );
}

#[test]
fn io_write_sink_maps_host_errors_to_a_finite_category() {
    let mut writer = FailingWriter;
    let mut sink = IoWriteSink::new(&mut writer);
    assert_eq!(
        SinkWrite::write_all(&mut sink, b"data"),
        Err(WriteError::HostFailure)
    );
}

#[test]
fn config_input_reads_from_the_opened_file_after_path_replacement() -> Result<(), Box<dyn Error>> {
    let file = TempInputFile::new(b"original")?;
    let replacement = TempInputFile::new(b"replacement")?;
    let budget = new_budget()?;
    let mut input = open_config_input(file.path(), &budget)?;

    remove_file(file.path())?;
    rename(replacement.path(), file.path())?;

    let mut output = [0; 10];
    let count = read_config_input(&mut input, &mut output)?;
    let actual = output
        .get(..count)
        .ok_or_else(|| io::Error::other("file read length exceeded the output buffer"))?;
    ensure_eq!(actual, b"original");
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn config_input_rejects_a_fifo_without_waiting_for_a_writer() -> Result<(), Box<dyn Error>> {
    let fifo = TempInputFile::fifo()?;
    let budget = new_budget()?;
    let error = open_config_input(fifo.path(), &budget)
        .err()
        .ok_or_else(|| io::Error::other("FIFO should be rejected as a non-regular file"))?;
    ensure_eq!(error.operation(), HostOperation::ConfigRead);
    ensure_eq!(error.kind(), HostErrorKind::NotRegularFile);
    Ok(())
}

#[test]
fn file_schema_input_streams_short_reads_and_permanent_eof() -> Result<(), Box<dyn Error>> {
    let file = TempInputFile::new(b"abcdef")?;
    let budget = new_budget()?;
    let mut input = open_schema_input(file.path(), 6, &budget)?;
    input.claim();
    let mut output = [0; 2];
    ensure_eq!(read_schema_input(&mut input, &mut output, 0)?, 2);
    ensure_eq!(&output, b"ab");
    ensure_eq!(read_schema_input(&mut input, &mut output, 2)?, 2);
    ensure_eq!(&output, b"cd");
    ensure_eq!(read_schema_input(&mut input, &mut output, 4)?, 2);
    ensure_eq!(&output, b"ef");
    ensure_eq!(read_schema_input(&mut input, &mut output, 6)?, 0);
    ensure_eq!(read_schema_input(&mut input, &mut output, 6)?, 0);
    Ok(())
}

#[test]
fn file_schema_input_rejects_unclaimed_or_reissued_sources() -> Result<(), Box<dyn Error>> {
    let file = TempInputFile::new(b"schema")?;
    let budget = new_budget()?;
    let mut input = open_schema_input(file.path(), 6, &budget)?;
    let error = read_schema_input(&mut input, &mut [0; 1], 0)
        .err()
        .ok_or_else(|| io::Error::other("unclaimed source should reject reads"))?;
    ensure_eq!(error.kind(), SourceReadErrorKind::InternalInvariantFailure);
    input.claim();
    input.claim();
    let error = read_schema_input(&mut input, &mut [0; 1], 0)
        .err()
        .ok_or_else(|| io::Error::other("source claimed more than once should reject reads"))?;
    ensure_eq!(error.kind(), SourceReadErrorKind::InternalInvariantFailure);
    ensure_eq!(
        error
            .context()
            .ok_or_else(|| io::Error::other("input state context is missing"))?
            .failure_point(),
        "stream-state-invariant"
    );
    Ok(())
}

#[test]
fn file_growth_after_metadata_is_detected_by_one_byte_probe() -> Result<(), Box<dyn Error>> {
    let file = TempInputFile::new(b"abc")?;
    let budget = new_budget()?;
    let mut input = open_schema_input(file.path(), 3, &budget)?;
    input.claim();
    let mut output = [0; 8];
    ensure_eq!(read_schema_input(&mut input, &mut output, 0)?, 3);
    ensure_eq!(&output[..3], b"abc");
    file.append(b"d")?;
    let error = read_schema_input(&mut input, &mut output, 3)
        .err()
        .ok_or_else(|| io::Error::other("file growth should exceed the source byte limit"))?;
    ensure_eq!(error.kind(), SourceReadErrorKind::ResourceLimitExceeded);
    ensure_eq!(error.offset(), Some(3));
    ensure_eq!(error.requested_bytes(), Some(1));
    ensure_eq!(error.consumed_bytes(), Some(1));
    ensure_eq!(
        error
            .context()
            .ok_or_else(|| io::Error::other("file growth context is missing"))?
            .failure_point(),
        "source-byte-limit"
    );
    Ok(())
}

#[test]
fn file_schema_input_rejects_metadata_size_above_limit() -> Result<(), Box<dyn Error>> {
    let file = TempInputFile::new(b"abc")?;
    let budget = new_budget()?;
    let error = open_schema_input(file.path(), 2, &budget)
        .err()
        .ok_or_else(|| io::Error::other("oversized file should be rejected while opening"))?;
    ensure_eq!(error.kind(), HostErrorKind::ResourceLimitExceeded);
    Ok(())
}

struct FailingReader;

impl IoRead for FailingReader {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "PRIVATE-READER-DETAIL",
        ))
    }
}

struct OverreportingReader;

impl IoRead for OverreportingReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        Ok(output.len() + 1)
    }
}

#[test]
fn read_source_masks_host_error_details_and_keeps_context() -> Result<(), Box<dyn Error>> {
    let budget = new_budget()?;
    let mut reader = FailingReader;
    let mut source = new_read_source(&mut reader, &budget)?;
    let error = read_source(&mut source, &mut [0; 5])
        .err()
        .ok_or_else(|| io::Error::other("expected host read to fail"))?;
    ensure_eq!(error.kind(), SourceReadErrorKind::HostFailure);
    ensure_eq!(error.offset(), Some(0));
    ensure_eq!(error.requested_bytes(), Some(5));
    ensure_eq!(
        error
            .context()
            .ok_or_else(|| io::Error::other("host read context is missing"))?
            .failure_point(),
        "host-reader-call"
    );
    ensure!(!format!("{error:?} {error}").contains("PRIVATE-READER-DETAIL"));
    Ok(())
}

#[test]
fn read_source_rejects_counts_beyond_the_supplied_buffer() -> Result<(), Box<dyn Error>> {
    let budget = new_budget()?;
    let mut reader = OverreportingReader;
    let mut source = new_read_source(&mut reader, &budget)?;
    let error = read_source(&mut source, &mut [0; 2])
        .err()
        .ok_or_else(|| io::Error::other("reader cannot report more bytes than the buffer holds"))?;
    ensure_eq!(error.kind(), SourceReadErrorKind::InternalInvariantFailure);
    ensure_eq!(error.offset(), Some(0));
    ensure_eq!(error.requested_bytes(), Some(2));
    ensure_eq!(
        error
            .context()
            .ok_or_else(|| io::Error::other("read count context is missing"))?
            .failure_point(),
        "read-count-exceeds-request"
    );
    Ok(())
}
