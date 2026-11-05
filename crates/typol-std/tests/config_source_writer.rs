#[macro_use]
#[path = "support/assertions.rs"]
mod assertions;

use std::{
    env::temp_dir,
    error::Error,
    fs, io,
    path::PathBuf,
    process,
    sync::atomic::{AtomicU64, Ordering},
};
use typol_config::{ConfigSourceExpectation, ConfigSourceWriter};
use typol_std::{ConfigSourceWriteErrorKind, FileConfigSourceWriter};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct TemporaryDirectory {
    path: PathBuf,
}

impl TemporaryDirectory {
    fn new() -> Result<Self, Box<dyn Error>> {
        let id = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = temp_dir().join(format!(
            "typol-config-source-writer-{}-{id}",
            std::process::id()
        ));
        fs::create_dir(&path)?;
        Ok(Self { path })
    }

    fn path(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

struct TemporaryRelativeFile {
    path: PathBuf,
}

impl TemporaryRelativeFile {
    fn new() -> Self {
        let id = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        Self {
            path: PathBuf::from(format!(
                "typol-relative-writer-{}-{id}.tconf",
                process::id()
            )),
        }
    }
}

impl Drop for TemporaryRelativeFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[test]
fn local_writer_refuses_snapshot_replacement_without_atomic_compare_and_swap()
-> Result<(), Box<dyn Error>> {
    let directory = TemporaryDirectory::new()?;
    let target = directory.path("settings.tconf");
    fs::write(&target, b"old source")?;
    let mut writer = FileConfigSourceWriter::new(4096);
    let error = match writer.replace_atomically(
        &target,
        ConfigSourceExpectation::ExactSnapshot(b"old source"),
        b"new source",
        128,
    ) {
        Ok(_) => {
            return Err(io::Error::other("snapshot replacement unexpectedly committed").into());
        }
        Err(error) => error,
    };
    ensure_eq!(
        error.kind(),
        ConfigSourceWriteErrorKind::AtomicReplaceUnsupported
    );
    ensure_eq!(fs::read(&target)?, b"old source");
    Ok(())
}

#[test]
fn stale_snapshot_and_existing_absent_target_leave_files_unchanged() -> Result<(), Box<dyn Error>> {
    let directory = TemporaryDirectory::new()?;
    let target = directory.path("settings.tconf");
    fs::write(&target, b"changed by another writer")?;
    let mut writer = FileConfigSourceWriter::new(4096);
    let error = match writer.replace_atomically(
        &target,
        ConfigSourceExpectation::ExactSnapshot(b"old source"),
        b"replacement",
        128,
    ) {
        Ok(_) => return Err(io::Error::other("stale snapshot unexpectedly committed").into()),
        Err(error) => error,
    };
    ensure_eq!(
        error.kind(),
        ConfigSourceWriteErrorKind::ConcurrentModification
    );
    ensure_eq!(fs::read(&target)?, b"changed by another writer");

    let absent_error = match writer.replace_atomically(
        &target,
        ConfigSourceExpectation::MustBeAbsent,
        b"replacement",
        128,
    ) {
        Ok(_) => {
            return Err(
                io::Error::other("MustBeAbsent unexpectedly replaced an existing file").into(),
            );
        }
        Err(error) => error,
    };
    ensure_eq!(
        absent_error.kind(),
        ConfigSourceWriteErrorKind::TargetAlreadyExists
    );
    ensure_eq!(fs::read(&target)?, b"changed by another writer");
    Ok(())
}

#[test]
fn new_target_requires_absence_and_replace_any_is_explicit() -> Result<(), Box<dyn Error>> {
    let directory = TemporaryDirectory::new()?;
    let target = directory.path("new.tconf");
    let mut writer = FileConfigSourceWriter::new(4096);
    writer.replace_atomically(
        &target,
        ConfigSourceExpectation::MustBeAbsent,
        b"first",
        128,
    )?;
    ensure_eq!(fs::read(&target)?, b"first");
    writer.replace_atomically(&target, ConfigSourceExpectation::ReplaceAny, b"second", 128)?;
    ensure_eq!(fs::read(&target)?, b"second");
    Ok(())
}

#[test]
fn relative_target_syncs_the_current_directory() -> Result<(), Box<dyn Error>> {
    let target = TemporaryRelativeFile::new();
    let mut writer = FileConfigSourceWriter::new(4096);
    writer.replace_atomically(
        &target.path,
        ConfigSourceExpectation::MustBeAbsent,
        b"relative source",
        128,
    )?;
    ensure_eq!(fs::read(&target.path)?, b"relative source");
    Ok(())
}

#[test]
fn replace_any_limits_the_new_source_without_reading_the_old_one() -> Result<(), Box<dyn Error>> {
    let directory = TemporaryDirectory::new()?;
    let target = directory.path("large-old.tconf");
    fs::write(&target, b"old source larger than the replacement limit")?;
    let mut writer = FileConfigSourceWriter::new(4096);
    writer.replace_atomically(&target, ConfigSourceExpectation::ReplaceAny, b"x", 1)?;
    ensure_eq!(fs::read(&target)?, b"x");
    Ok(())
}

#[test]
fn source_writer_rejects_directories_and_enforces_limits() -> Result<(), Box<dyn Error>> {
    let directory = TemporaryDirectory::new()?;
    let mut writer = FileConfigSourceWriter::new(4096);
    let invalid_target = match writer.replace_atomically(
        directory.path.as_path(),
        ConfigSourceExpectation::ReplaceAny,
        b"replacement",
        128,
    ) {
        Ok(_) => return Err(io::Error::other("directory target unexpectedly accepted").into()),
        Err(error) => error,
    };
    ensure_eq!(
        invalid_target.kind(),
        ConfigSourceWriteErrorKind::InvalidTarget
    );
    let target = directory.path("limited.tconf");
    let limit_error = match writer.replace_atomically(
        &target,
        ConfigSourceExpectation::MustBeAbsent,
        b"too large",
        2,
    ) {
        Ok(_) => return Err(io::Error::other("oversized source unexpectedly committed").into()),
        Err(error) => error,
    };
    ensure_eq!(
        limit_error.kind(),
        ConfigSourceWriteErrorKind::ResourceLimitExceeded
    );
    ensure!(!target.exists());
    Ok(())
}
