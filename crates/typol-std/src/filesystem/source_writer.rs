use std::{
    error::Error,
    ffi::OsString,
    fmt,
    fs::{self, File, OpenOptions, Permissions},
    io::{ErrorKind, Read as IoRead, Write as IoWrite},
    path::{Path, PathBuf},
    process,
    sync::atomic::{AtomicU64, Ordering},
};

use typol_config::{ConfigSourceExpectation, ConfigSourceRevision, ConfigSourceWriter};

#[cfg(any(test, unix, windows))]
use std::io::Error as IoError;

static NEXT_TEMPORARY_ID: AtomicU64 = AtomicU64::new(0);
static NEXT_REVISION: AtomicU64 = AtomicU64::new(0);

#[cfg(windows)]
const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;

#[cfg(windows)]
const MOVEFILE_WRITE_THROUGH: u32 = 0x8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigSourceWriteErrorKind {
    InvalidTarget,
    ConcurrentModification,
    TargetAlreadyExists,
    ResourceLimitExceeded,
    AllocationFailed,
    ReadFailure,
    TemporaryFileFailure,
    WriteFailure,
    FlushFailure,
    SyncFailure,
    DirectorySyncFailure,
    AtomicReplaceUnsupported,
    RenameFailure,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConfigSourceWriteError {
    kind: ConfigSourceWriteErrorKind,
}

enum ExpectedTarget<'a> {
    Snapshot(&'a [u8]),
    Absent,
    Any,
}

struct TargetState {
    permissions: Option<Permissions>,
}

struct TemporaryPath {
    path: PathBuf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FailurePoint {
    TemporaryCreate,
    Write,
    Flush,
    FileSync,
    Rename,
    DirectorySync,
}

/// Replaces local configuration files using a same-directory temporary file.
pub struct FileConfigSourceWriter {
    max_path_bytes: usize,
    #[cfg(test)]
    failure_point: Option<FailurePoint>,
}

impl ConfigSourceWriteError {
    const fn new(kind: ConfigSourceWriteErrorKind) -> Self {
        Self { kind }
    }

    pub const fn kind(&self) -> ConfigSourceWriteErrorKind {
        self.kind
    }
}

impl fmt::Display for ConfigSourceWriteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.kind {
            ConfigSourceWriteErrorKind::InvalidTarget => "configuration source target is invalid",
            ConfigSourceWriteErrorKind::ConcurrentModification => {
                "configuration source changed since it was read"
            }
            ConfigSourceWriteErrorKind::TargetAlreadyExists => {
                "configuration source target already exists"
            }
            ConfigSourceWriteErrorKind::ResourceLimitExceeded => {
                "configuration source file limit exceeded"
            }
            ConfigSourceWriteErrorKind::AllocationFailed => {
                "configuration source allocation failed"
            }
            ConfigSourceWriteErrorKind::ReadFailure => "configuration source could not be read",
            ConfigSourceWriteErrorKind::TemporaryFileFailure => {
                "configuration source temporary file could not be created"
            }
            ConfigSourceWriteErrorKind::WriteFailure => {
                "configuration source temporary file could not be written"
            }
            ConfigSourceWriteErrorKind::FlushFailure => {
                "configuration source temporary file could not be flushed"
            }
            ConfigSourceWriteErrorKind::SyncFailure => {
                "configuration source temporary file could not be synchronized"
            }
            ConfigSourceWriteErrorKind::DirectorySyncFailure => {
                "configuration source was replaced but its directory could not be synchronized"
            }
            ConfigSourceWriteErrorKind::AtomicReplaceUnsupported => {
                "atomic replacement is unsupported for this target"
            }
            ConfigSourceWriteErrorKind::RenameFailure => {
                "configuration source atomic replacement failed"
            }
        })
    }
}

impl Error for ConfigSourceWriteError {}

impl FileConfigSourceWriter {
    pub const fn new(max_path_bytes: usize) -> Self {
        Self {
            max_path_bytes,
            #[cfg(test)]
            failure_point: None,
        }
    }
}

impl ConfigSourceWriter for FileConfigSourceWriter {
    type Target = Path;
    type Error = ConfigSourceWriteError;

    fn replace_atomically(
        &mut self,
        target: &Self::Target,
        expectation: ConfigSourceExpectation<'_>,
        replacement: &[u8],
        max_bytes: usize,
    ) -> Result<ConfigSourceRevision, Self::Error> {
        if target.as_os_str().as_encoded_bytes().len() > self.max_path_bytes
            || target.file_name().is_none()
        {
            return Err(ConfigSourceWriteError::new(
                ConfigSourceWriteErrorKind::InvalidTarget,
            ));
        }
        if replacement.len() > max_bytes {
            return Err(ConfigSourceWriteError::new(
                ConfigSourceWriteErrorKind::ResourceLimitExceeded,
            ));
        }
        let expectation = match expectation {
            ConfigSourceExpectation::ExactSnapshot(snapshot) => ExpectedTarget::Snapshot(snapshot),
            ConfigSourceExpectation::DocumentSnapshot => {
                return Err(ConfigSourceWriteError::new(
                    ConfigSourceWriteErrorKind::ConcurrentModification,
                ));
            }
            ConfigSourceExpectation::MustBeAbsent => ExpectedTarget::Absent,
            ConfigSourceExpectation::ReplaceAny => ExpectedTarget::Any,
        };
        let original = check_target(target, &expectation, max_bytes)?;
        if matches!(expectation, ExpectedTarget::Snapshot(_)) {
            return Err(ConfigSourceWriteError::new(
                ConfigSourceWriteErrorKind::AtomicReplaceUnsupported,
            ));
        }
        self.fail_if(FailurePoint::TemporaryCreate)?;
        let (mut temporary, temporary_path) = open_temporary(target)?;
        if let Some(permissions) = original.permissions {
            temporary.set_permissions(permissions).map_err(|_| {
                ConfigSourceWriteError::new(ConfigSourceWriteErrorKind::WriteFailure)
            })?;
        }
        self.fail_if(FailurePoint::Write)?;
        temporary
            .write_all(replacement)
            .map_err(|_| ConfigSourceWriteError::new(ConfigSourceWriteErrorKind::WriteFailure))?;
        self.fail_if(FailurePoint::Flush)?;
        temporary
            .flush()
            .map_err(|_| ConfigSourceWriteError::new(ConfigSourceWriteErrorKind::FlushFailure))?;
        self.fail_if(FailurePoint::FileSync)?;
        temporary
            .sync_all()
            .map_err(|_| ConfigSourceWriteError::new(ConfigSourceWriteErrorKind::SyncFailure))?;
        drop(temporary);

        self.fail_if(FailurePoint::Rename)?;
        match expectation {
            ExpectedTarget::Absent => create_absent(&temporary_path.path, target)?,
            ExpectedTarget::Any => replace_file(&temporary_path.path, target)?,
            ExpectedTarget::Snapshot(_) => {
                return Err(ConfigSourceWriteError::new(
                    ConfigSourceWriteErrorKind::AtomicReplaceUnsupported,
                ));
            }
        }
        drop(temporary_path);
        self.fail_if(FailurePoint::DirectorySync).map_err(|_| {
            ConfigSourceWriteError::new(ConfigSourceWriteErrorKind::DirectorySyncFailure)
        })?;
        sync_parent(target)?;
        let revision = NEXT_REVISION
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        Ok(ConfigSourceRevision::new(revision))
    }
}

impl FileConfigSourceWriter {
    fn fail_if(&self, point: FailurePoint) -> Result<(), ConfigSourceWriteError> {
        #[cfg(test)]
        if self.failure_point == Some(point) {
            return Err(ConfigSourceWriteError::new(match point {
                FailurePoint::TemporaryCreate => ConfigSourceWriteErrorKind::TemporaryFileFailure,
                FailurePoint::Write => ConfigSourceWriteErrorKind::WriteFailure,
                FailurePoint::Flush => ConfigSourceWriteErrorKind::FlushFailure,
                FailurePoint::FileSync => ConfigSourceWriteErrorKind::SyncFailure,
                FailurePoint::Rename => ConfigSourceWriteErrorKind::RenameFailure,
                FailurePoint::DirectorySync => ConfigSourceWriteErrorKind::DirectorySyncFailure,
            }));
        }
        let _ = point;
        Ok(())
    }
}

fn check_target(
    target: &Path,
    expectation: &ExpectedTarget<'_>,
    max_bytes: usize,
) -> Result<TargetState, ConfigSourceWriteError> {
    let metadata = match fs::symlink_metadata(target) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return match expectation {
                ExpectedTarget::Snapshot(_) => Err(ConfigSourceWriteError::new(
                    ConfigSourceWriteErrorKind::ConcurrentModification,
                )),
                ExpectedTarget::Absent | ExpectedTarget::Any => {
                    Ok(TargetState { permissions: None })
                }
            };
        }
        Err(_) => {
            return Err(ConfigSourceWriteError::new(
                ConfigSourceWriteErrorKind::ReadFailure,
            ));
        }
    };
    if !metadata.file_type().is_file() {
        return Err(ConfigSourceWriteError::new(
            ConfigSourceWriteErrorKind::InvalidTarget,
        ));
    }
    match expectation {
        ExpectedTarget::Absent => {
            return Err(ConfigSourceWriteError::new(
                ConfigSourceWriteErrorKind::TargetAlreadyExists,
            ));
        }
        ExpectedTarget::Snapshot(expected) => {
            if metadata.len() > u64::try_from(max_bytes).unwrap_or(u64::MAX) {
                return Err(ConfigSourceWriteError::new(
                    ConfigSourceWriteErrorKind::ResourceLimitExceeded,
                ));
            }
            let current = read_bounded(target, max_bytes)?;
            if current.as_slice() != *expected {
                return Err(ConfigSourceWriteError::new(
                    ConfigSourceWriteErrorKind::ConcurrentModification,
                ));
            }
        }
        ExpectedTarget::Any => {}
    }
    Ok(TargetState {
        permissions: Some(metadata.permissions()),
    })
}

fn read_bounded(target: &Path, max_bytes: usize) -> Result<Vec<u8>, ConfigSourceWriteError> {
    let capacity = max_bytes.checked_add(1).ok_or_else(|| {
        ConfigSourceWriteError::new(ConfigSourceWriteErrorKind::ResourceLimitExceeded)
    })?;
    let mut contents = Vec::new();
    contents
        .try_reserve_exact(capacity)
        .map_err(|_| ConfigSourceWriteError::new(ConfigSourceWriteErrorKind::AllocationFailed))?;
    let limit = u64::try_from(capacity).map_err(|_| {
        ConfigSourceWriteError::new(ConfigSourceWriteErrorKind::ResourceLimitExceeded)
    })?;
    File::open(target)
        .map_err(|_| ConfigSourceWriteError::new(ConfigSourceWriteErrorKind::ReadFailure))?
        .take(limit)
        .read_to_end(&mut contents)
        .map_err(|_| ConfigSourceWriteError::new(ConfigSourceWriteErrorKind::ReadFailure))?;
    if contents.len() > max_bytes {
        return Err(ConfigSourceWriteError::new(
            ConfigSourceWriteErrorKind::ResourceLimitExceeded,
        ));
    }
    Ok(contents)
}

fn open_temporary(target: &Path) -> Result<(File, TemporaryPath), ConfigSourceWriteError> {
    for _ in 0..64 {
        let path = temporary_path(target)?;
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((file, TemporaryPath { path })),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            Err(_) => {
                return Err(ConfigSourceWriteError::new(
                    ConfigSourceWriteErrorKind::TemporaryFileFailure,
                ));
            }
        }
    }
    Err(ConfigSourceWriteError::new(
        ConfigSourceWriteErrorKind::TemporaryFileFailure,
    ))
}

fn create_absent(temporary: &Path, target: &Path) -> Result<(), ConfigSourceWriteError> {
    #[cfg(unix)]
    fs::hard_link(temporary, target).map_err(map_commit_error)?;
    #[cfg(windows)]
    move_file_windows(temporary, target, MOVEFILE_WRITE_THROUGH).map_err(map_commit_error)?;
    #[cfg(not(any(unix, windows)))]
    return Err(ConfigSourceWriteError::new(
        ConfigSourceWriteErrorKind::AtomicReplaceUnsupported,
    ));
    Ok(())
}

#[cfg(any(unix, windows))]
fn map_commit_error(error: IoError) -> ConfigSourceWriteError {
    let kind = match error.kind() {
        ErrorKind::AlreadyExists => ConfigSourceWriteErrorKind::TargetAlreadyExists,
        ErrorKind::Unsupported => ConfigSourceWriteErrorKind::AtomicReplaceUnsupported,
        _ => ConfigSourceWriteErrorKind::RenameFailure,
    };
    ConfigSourceWriteError::new(kind)
}

fn temporary_path(target: &Path) -> Result<PathBuf, ConfigSourceWriteError> {
    let parent = target
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let filename = target
        .file_name()
        .ok_or_else(|| ConfigSourceWriteError::new(ConfigSourceWriteErrorKind::InvalidTarget))?;
    let id = NEXT_TEMPORARY_ID.fetch_add(1, Ordering::Relaxed);
    let mut name = OsString::from(".");
    name.push(filename);
    name.push(".typol-tmp-");
    name.push(process::id().to_string());
    name.push("-");
    name.push(id.to_string());
    Ok(parent.join(name))
}

fn replace_file(temporary: &Path, target: &Path) -> Result<(), ConfigSourceWriteError> {
    #[cfg(unix)]
    fs::rename(temporary, target).map_err(|error| {
        let kind = if error.kind() == ErrorKind::Unsupported {
            ConfigSourceWriteErrorKind::AtomicReplaceUnsupported
        } else {
            ConfigSourceWriteErrorKind::RenameFailure
        };
        ConfigSourceWriteError::new(kind)
    })?;
    #[cfg(windows)]
    replace_windows(temporary, target)?;
    #[cfg(not(any(unix, windows)))]
    return Err(ConfigSourceWriteError::new(
        ConfigSourceWriteErrorKind::AtomicReplaceUnsupported,
    ));
    Ok(())
}

#[cfg(windows)]
fn replace_windows(temporary: &Path, target: &Path) -> Result<(), ConfigSourceWriteError> {
    move_file_windows(
        temporary,
        target,
        MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
    )
    .map_err(map_commit_error)
}

#[cfg(windows)]
fn move_file_windows(temporary: &Path, target: &Path, flags: u32) -> Result<(), IoError> {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(
            existing_file_name: *const u16,
            new_file_name: *const u16,
            flags: u32,
        ) -> i32;
    }

    let temporary: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
    let target: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    // The UTF-16 buffers remain alive and NUL-terminated for this synchronous system call.
    let succeeded = unsafe { MoveFileExW(temporary.as_ptr(), target.as_ptr(), flags) };
    if succeeded == 0 {
        Err(IoError::last_os_error())
    } else {
        Ok(())
    }
}

fn sync_parent(target: &Path) -> Result<(), ConfigSourceWriteError> {
    #[cfg(unix)]
    {
        let parent = target
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let directory = File::open(parent).map_err(|_| {
            ConfigSourceWriteError::new(ConfigSourceWriteErrorKind::DirectorySyncFailure)
        })?;
        directory.sync_all().map_err(|_| {
            ConfigSourceWriteError::new(ConfigSourceWriteErrorKind::DirectorySyncFailure)
        })?;
    }
    #[cfg(not(unix))]
    let _ = target;
    Ok(())
}

impl Drop for TemporaryPath {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
struct TestDirectory {
    path: PathBuf,
}

#[cfg(test)]
impl TestDirectory {
    fn new() -> Result<Self, IoError> {
        let id = NEXT_TEMPORARY_ID.fetch_add(1, Ordering::Relaxed);
        let path = PathBuf::from(format!("typol-source-writer-{}-{id}", process::id()));
        fs::create_dir(&path)?;
        Ok(Self { path })
    }

    fn path(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

#[cfg(test)]
impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
fn check_precommit_failures_leave_the_target_and_directory_clean() -> Result<(), IoError> {
    let directory = TestDirectory::new()?;
    let failures = [
        (
            FailurePoint::TemporaryCreate,
            ConfigSourceWriteErrorKind::TemporaryFileFailure,
        ),
        (
            FailurePoint::Write,
            ConfigSourceWriteErrorKind::WriteFailure,
        ),
        (
            FailurePoint::Flush,
            ConfigSourceWriteErrorKind::FlushFailure,
        ),
        (
            FailurePoint::FileSync,
            ConfigSourceWriteErrorKind::SyncFailure,
        ),
        (
            FailurePoint::Rename,
            ConfigSourceWriteErrorKind::RenameFailure,
        ),
    ];
    for (index, (point, expected_kind)) in failures.into_iter().enumerate() {
        let target = directory.path(&format!("precommit-{index}.tconf"));
        fs::write(&target, b"original")?;
        let mut writer = FileConfigSourceWriter::new(4096);
        writer.failure_point = Some(point);
        let error = match writer.replace_atomically(
            &target,
            ConfigSourceExpectation::ReplaceAny,
            b"replacement",
            128,
        ) {
            Ok(_) => {
                return Err(IoError::other(
                    "injected precommit failure unexpectedly succeeded",
                ));
            }
            Err(error) => error,
        };
        test_equal(error.kind(), expected_kind)?;
        test_equal(fs::read(&target)?, b"original".to_vec())?;
        test_equal(fs::read_dir(&directory.path)?.count(), index + 1)?;
    }
    Ok(())
}

#[cfg(test)]
#[test]
fn precommit_failures_leave_the_target_and_directory_clean() {
    let result = check_precommit_failures_leave_the_target_and_directory_clean();
    assert!(result.is_ok(), "{result:?}");
}

#[cfg(test)]
fn check_directory_sync_failure_reports_the_already_visible_commit() -> Result<(), IoError> {
    let directory = TestDirectory::new()?;
    let target = directory.path("directory-sync.tconf");
    fs::write(&target, b"original")?;
    let mut writer = FileConfigSourceWriter::new(4096);
    writer.failure_point = Some(FailurePoint::DirectorySync);
    let error = match writer.replace_atomically(
        &target,
        ConfigSourceExpectation::ReplaceAny,
        b"replacement",
        128,
    ) {
        Ok(_) => {
            return Err(IoError::other(
                "injected directory sync failure unexpectedly succeeded",
            ));
        }
        Err(error) => error,
    };
    test_equal(
        error.kind(),
        ConfigSourceWriteErrorKind::DirectorySyncFailure,
    )?;
    test_equal(fs::read(&target)?, b"replacement".to_vec())?;
    test_equal(fs::read_dir(&directory.path)?.count(), 1)?;
    Ok(())
}

#[cfg(test)]
#[test]
fn directory_sync_failure_reports_the_already_visible_commit() {
    let result = check_directory_sync_failure_reports_the_already_visible_commit();
    assert!(result.is_ok(), "{result:?}");
}

#[cfg(test)]
fn check_atomic_absent_create_rejects_a_target_that_appeared_after_the_check() -> Result<(), IoError>
{
    let directory = TestDirectory::new()?;
    let temporary = directory.path("prepared");
    let target = directory.path("raced.tconf");
    fs::write(&temporary, b"replacement")?;
    fs::write(&target, b"concurrent writer")?;
    let error = create_absent(&temporary, &target).err();
    test_equal(
        error.map(|error| error.kind()),
        Some(ConfigSourceWriteErrorKind::TargetAlreadyExists),
    )?;
    test_equal(fs::read(&target)?, b"concurrent writer".to_vec())?;
    Ok(())
}

#[cfg(test)]
#[test]
fn atomic_absent_create_rejects_a_target_that_appeared_after_the_check() {
    let result = check_atomic_absent_create_rejects_a_target_that_appeared_after_the_check();
    assert!(result.is_ok(), "{result:?}");
}

#[cfg(test)]
fn test_equal<T: fmt::Debug + PartialEq>(actual: T, expected: T) -> Result<(), IoError> {
    if actual == expected {
        Ok(())
    } else {
        Err(IoError::other(format!(
            "expected {expected:?}, got {actual:?}"
        )))
    }
}
