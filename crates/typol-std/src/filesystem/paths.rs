use std::{
    ffi::OsStr,
    fs::{File, Metadata, canonicalize},
    path::Path,
};

use opeo::{OResult, Out, ResultOutExt};
use typol_core::{ChargedVec, ExecutionBudget};

use crate::{HostError, HostErrorKind, HostFailureStep, HostOperation};

#[cfg(target_os = "linux")]
use std::{
    io,
    os::{fd::AsRawFd, raw::c_char},
};

#[cfg(unix)]
use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};

#[cfg(windows)]
use std::{
    ffi::c_void,
    os::windows::{ffi::OsStrExt, io::AsRawHandle},
};

#[cfg(target_os = "linux")]
unsafe extern "C" {
    fn readlink(path: *const c_char, buffer: *mut c_char, size: usize) -> isize;
}

#[cfg(windows)]
#[derive(Default)]
#[repr(C)]
struct WindowsFileTime {
    low: u32,
    high: u32,
}

#[cfg(windows)]
#[derive(Default)]
#[repr(C)]
struct WindowsFileInformation {
    attributes: u32,
    creation_time: WindowsFileTime,
    access_time: WindowsFileTime,
    write_time: WindowsFileTime,
    volume_serial: u32,
    size_high: u32,
    size_low: u32,
    links: u32,
    file_index_high: u32,
    file_index_low: u32,
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetFileInformationByHandle(
        file: *mut c_void,
        information: *mut WindowsFileInformation,
    ) -> i32;
}

pub(crate) struct NativePath {
    encoded: ChargedVec<u8>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) struct FileIdentity {
    volume: u64,
    file: u64,
}

pub(crate) struct PathCandidate<'a> {
    pub(crate) module: &'a str,
    pub(crate) alternate: bool,
}

pub(crate) struct PathResolutionParams<'budget> {
    pub(crate) max_path_bytes: usize,
    pub(crate) budget: &'budget ExecutionBudget,
    pub(crate) operation: HostOperation,
}

impl NativePath {
    pub(crate) fn from_opened_file<'slot>(
        requested_path: &Path,
        file: &File,
        params: PathResolutionParams<'_>,
        mut out: Out<'slot, '_, HostError>,
    ) -> OResult<'slot, Self> {
        let PathResolutionParams {
            max_path_bytes,
            budget,
            operation,
        } = params;
        Self::check(
            requested_path,
            max_path_bytes,
            budget,
            operation,
            out.reborrow(),
        )?;
        budget.consume_with(1, out.reborrow(), |error| {
            HostError::budget(operation, error)
        })?;
        #[cfg(target_os = "linux")]
        if let Some(native) = Self::from_linux_descriptor(
            file.as_raw_fd(),
            requested_path.as_os_str().as_encoded_bytes().len(),
            PathResolutionParams {
                max_path_bytes,
                budget,
                operation,
            },
            out.reborrow(),
        )? {
            return Ok(native);
        }
        #[cfg(not(target_os = "linux"))]
        let _ = file;
        let native = canonicalize(requested_path)
            .map_err(|error| {
                HostError::io_at(operation, error.kind(), HostFailureStep::PathProcessing)
            })
            .store_err_into(out.reborrow())?;
        Self::check(&native, max_path_bytes, budget, operation, out.reborrow())?;
        Self::from_path(&native, budget, operation, out.reborrow())
    }

    pub(crate) fn label_for_opened_file<'slot>(
        requested_path: &Path,
        file: &File,
        params: PathResolutionParams<'_>,
        mut out: Out<'slot, '_, HostError>,
    ) -> OResult<'slot, Self> {
        let PathResolutionParams {
            max_path_bytes,
            budget,
            operation,
        } = params;
        Self::check(
            requested_path,
            max_path_bytes,
            budget,
            operation,
            out.reborrow(),
        )?;
        budget.consume_with(1, out.reborrow(), |error| {
            HostError::budget(operation, error)
        })?;
        #[cfg(target_os = "linux")]
        if let Some(native) = Self::from_linux_descriptor(
            file.as_raw_fd(),
            requested_path.as_os_str().as_encoded_bytes().len(),
            PathResolutionParams {
                max_path_bytes,
                budget,
                operation,
            },
            out.reborrow(),
        )? {
            return Ok(native);
        }
        #[cfg(not(target_os = "linux"))]
        let _ = file;
        Self::from_path(requested_path, budget, operation, out)
    }

    pub(crate) fn file_identity(file: &File, _metadata: &Metadata) -> Option<FileIdentity> {
        #[cfg(unix)]
        {
            let _ = file;
            Some(FileIdentity {
                volume: _metadata.dev(),
                file: _metadata.ino(),
            })
        }
        #[cfg(windows)]
        {
            let mut information = WindowsFileInformation::default();
            // The handle is borrowed from an open File and the output points to a valid structure.
            let succeeded =
                unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) };
            if succeeded == 0 {
                return None;
            }
            Some(FileIdentity {
                volume: u64::from(information.volume_serial),
                file: (u64::from(information.file_index_high) << 32)
                    | u64::from(information.file_index_low),
            })
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (file, _metadata);
            None
        }
    }

    #[cfg(target_os = "linux")]
    fn from_linux_descriptor<'slot>(
        descriptor: i32,
        path_length: usize,
        params: PathResolutionParams<'_>,
        mut out: Out<'slot, '_, HostError>,
    ) -> OResult<'slot, Option<Self>> {
        let PathResolutionParams {
            max_path_bytes,
            budget,
            operation,
        } = params;
        let Some(link_path) = Self::linux_handle_path(descriptor) else {
            return Err(out.fail(HostError::internal(
                operation,
                HostFailureStep::PathProcessing,
            )));
        };
        let maximum = max_path_bytes.saturating_add(1).max(1);
        let initial = path_length.saturating_add(1).min(64).min(maximum);
        let mut encoded =
            HostError::run_budgeted(operation, None, out.reborrow(), |mut budget_out| {
                let mut encoded = ChargedVec::new(Some(budget), budget_out.reborrow())?;
                encoded.try_reserve_exact(initial, budget_out.reborrow())?;
                encoded.resize_with(initial, || 0, budget_out.reborrow())?;
                Ok(encoded)
            })?;
        loop {
            let count = match Self::read_linux_handle_path(&link_path, &mut encoded) {
                Ok(count) => count,
                Err(_) => return Ok(None),
            };
            if count < encoded.len() {
                if count > max_path_bytes {
                    return Err(out.fail(HostError::new(
                        operation,
                        HostErrorKind::ResourceLimitExceeded,
                    )));
                }
                encoded.truncate(count);
                budget.consume_with(count, out.reborrow(), |error| {
                    HostError::budget(operation, error)
                })?;
                return Ok(Some(Self { encoded }));
            }
            if encoded.len() == maximum {
                return Err(out.fail(HostError::new(
                    operation,
                    HostErrorKind::ResourceLimitExceeded,
                )));
            }
            let next = encoded.len().saturating_mul(2).min(maximum);
            HostError::run_budgeted(operation, None, out.reborrow(), |mut budget_out| {
                encoded.try_reserve_exact(next - encoded.len(), budget_out.reborrow())?;
                encoded.resize_with(next, || 0, budget_out)
            })?;
        }
    }

    #[cfg(target_os = "linux")]
    fn linux_handle_path(descriptor: i32) -> Option<[c_char; 32]> {
        let mut path = [0; 32];
        let prefix = b"/proc/self/fd/";
        let mut path_iter = path.iter_mut();
        for byte in prefix {
            *path_iter.next()? = *byte as c_char;
        }
        let mut digits = [0_u8; 10];
        let mut value = descriptor as u32;
        let mut count = 0;
        loop {
            *digits.get_mut(count)? = b'0' + (value % 10) as u8;
            count += 1;
            value /= 10;
            if value == 0 {
                break;
            }
        }
        for digit in digits.iter().take(count).rev() {
            *path_iter.next()? = *digit as c_char;
        }
        *path_iter.next()? = 0;
        Some(path)
    }

    #[cfg(target_os = "linux")]
    fn read_linux_handle_path(link_path: &[c_char], buffer: &mut [u8]) -> io::Result<usize> {
        // SAFETY: link_path is NUL-terminated, and buffer points to a valid writable slice with the matching length.
        let count =
            unsafe { readlink(link_path.as_ptr(), buffer.as_mut_ptr().cast(), buffer.len()) };
        if count < 0 {
            return Err(io::Error::last_os_error());
        }
        usize::try_from(count).map_err(|_| io::Error::from(io::ErrorKind::InvalidData))
    }

    fn from_path<'slot>(
        path: &Path,
        budget: &ExecutionBudget,
        operation: HostOperation,
        mut out: Out<'slot, '_, HostError>,
    ) -> OResult<'slot, Self> {
        let encoded =
            HostError::run_budgeted(operation, None, out.reborrow(), |mut budget_out| {
                let mut encoded = ChargedVec::new(Some(budget), budget_out.reborrow())?;
                let source = path.as_os_str().as_encoded_bytes();
                encoded.try_reserve_exact(source.len(), budget_out.reborrow())?;
                encoded.extend_from_slice(source, budget_out.reborrow())?;
                Ok(encoded)
            })?;
        Ok(Self { encoded })
    }

    pub(crate) fn copy<'slot>(
        &self,
        budget: &ExecutionBudget,
        operation: HostOperation,
        mut out: Out<'slot, '_, HostError>,
    ) -> OResult<'slot, Self> {
        Self::from_path(self.path(), budget, operation, out.reborrow())
    }

    pub(crate) fn candidate<'slot>(
        &self,
        candidate: PathCandidate<'_>,
        max_path_bytes: usize,
        budget: &ExecutionBudget,
        mut out: Out<'slot, '_, HostError>,
    ) -> OResult<'slot, Self> {
        let PathCandidate { module, alternate } = candidate;
        let operation = HostOperation::SchemaRead;
        let traversal_steps = self
            .encoded
            .len()
            .checked_mul(3)
            .ok_or_else(|| HostError::budget(operation, budget.resource_error()))
            .store_err_into(out.reborrow())?;
        budget.consume_with(traversal_steps, out.reborrow(), |error| {
            HostError::budget(operation, error)
        })?;
        let path = self.path();
        let parent = path
            .parent()
            .ok_or(HostError::new(operation, HostErrorKind::InvalidRequest));
        let parent = parent.store_err_into(out.reborrow())?;
        let stem = path
            .file_stem()
            .ok_or(HostError::new(operation, HostErrorKind::InvalidRequest));
        let stem = stem.store_err_into(out.reborrow())?;
        let unit = if cfg!(windows) { 2 } else { 1 };
        let parent_length = Self::length(parent, out.reborrow())?;
        let stem_length = if alternate {
            Self::length(Path::new(stem), out.reborrow())?
        } else {
            0
        };
        let parent = parent.as_os_str().as_encoded_bytes();
        let stem = if alternate {
            stem.as_encoded_bytes()
        } else {
            &[]
        };
        let terminated = parent
            .last()
            .is_some_and(|byte| *byte == b'/' || (cfg!(windows) && *byte == b'\\'));
        let _ = parent_length
            .checked_add(stem_length)
            .and_then(|size| size.checked_add(usize::from(!terminated) * unit))
            .and_then(|size| size.checked_add(usize::from(alternate) * unit))
            .and_then(|size| size.checked_add(module.len().checked_add(7)?.checked_mul(unit)?))
            .filter(|size| *size <= max_path_bytes)
            .ok_or(HostError::new(
                operation,
                HostErrorKind::ResourceLimitExceeded,
            ))
            .store_err_into(out.reborrow())?;
        let size = parent
            .len()
            .checked_add(stem.len())
            .and_then(|size| size.checked_add(usize::from(!terminated)))
            .and_then(|size| size.checked_add(usize::from(alternate)))
            .and_then(|size| size.checked_add(module.len()))
            .and_then(|size| size.checked_add(7))
            .ok_or(HostError::new(
                operation,
                HostErrorKind::ResourceLimitExceeded,
            ))
            .store_err_into(out.reborrow())?;
        let mut encoded =
            HostError::run_budgeted(operation, None, out.reborrow(), |mut budget_out| {
                let mut encoded = ChargedVec::new(Some(budget), budget_out.reborrow())?;
                encoded.try_reserve_exact(size, budget_out.reborrow())?;
                Ok(encoded)
            })?;
        let separator = if cfg!(windows) { b'\\' } else { b'/' };
        HostError::run_budgeted(operation, None, out.reborrow(), |budget_out| {
            encoded.extend_from_slice(parent, budget_out)
        })?;
        if !terminated {
            HostError::run_budgeted(operation, None, out.reborrow(), |budget_out| {
                encoded.try_push(separator, budget_out)
            })?;
        }
        if alternate {
            HostError::run_budgeted(operation, None, out.reborrow(), |budget_out| {
                encoded.extend_from_slice(stem, budget_out)
            })?;
            HostError::run_budgeted(operation, None, out.reborrow(), |budget_out| {
                encoded.try_push(separator, budget_out)
            })?;
        }
        HostError::run_budgeted(operation, None, out.reborrow(), |budget_out| {
            encoded.extend_from_slice(module.as_bytes(), budget_out)
        })?;
        HostError::run_budgeted(operation, None, out.reborrow(), |budget_out| {
            encoded.extend_from_slice(b".tschem", budget_out)
        })?;
        let result = Self { encoded };
        Self::check(
            result.path(),
            max_path_bytes,
            budget,
            operation,
            out.reborrow(),
        )?;
        Ok(result)
    }

    pub(crate) fn path(&self) -> &Path {
        // SAFETY: The bytes use the current platform’s `OsStr` encoding; candidates append only ASCII separators and module names between complete path components,
        // so no non-ASCII encoding unit is split and reconstruction remains valid for the current platform encoding.
        let native = unsafe { OsStr::from_encoded_bytes_unchecked(&self.encoded) };
        Path::new(native)
    }

    pub(crate) fn equals<'slot>(
        &self,
        other: &Self,
        budget: &ExecutionBudget,
        mut out: Out<'slot, '_, HostError>,
    ) -> OResult<'slot, bool> {
        let size = self
            .encoded
            .len()
            .checked_add(other.encoded.len())
            .ok_or(HostError::new(
                HostOperation::SchemaRead,
                HostErrorKind::ResourceLimitExceeded,
            ))
            .store_err_into(out.reborrow())?;
        budget.consume_with(size, out.reborrow(), |error| {
            HostError::budget(HostOperation::SchemaRead, error)
        })?;
        Ok(self.encoded.as_slice() == other.encoded.as_slice())
    }

    fn check<'slot>(
        path: &Path,
        max_path_bytes: usize,
        budget: &ExecutionBudget,
        operation: HostOperation,
        mut out: Out<'slot, '_, HostError>,
    ) -> OResult<'slot, ()> {
        budget.consume_with(
            path.as_os_str().as_encoded_bytes().len(),
            out.reborrow(),
            |error| HostError::budget(operation, error),
        )?;
        let size = Self::length(path, out.reborrow())?;
        if size > max_path_bytes {
            return Err(out.fail(HostError::new(
                operation,
                HostErrorKind::ResourceLimitExceeded,
            )));
        }
        budget.consume_with(size, out.reborrow(), |error| {
            HostError::budget(operation, error)
        })?;
        Ok(())
    }

    #[cfg(unix)]
    fn length<'slot, 'borrow>(
        path: &Path,
        _out: Out<'slot, 'borrow, HostError>,
    ) -> OResult<'slot, usize> {
        Ok(path.as_os_str().as_bytes().len())
    }

    #[cfg(windows)]
    fn length<'slot>(path: &Path, mut out: Out<'slot, '_, HostError>) -> OResult<'slot, usize> {
        let length = path
            .as_os_str()
            .encode_wide()
            .count()
            .checked_mul(2)
            .ok_or(HostError::new(
                HostOperation::SchemaRead,
                HostErrorKind::ResourceLimitExceeded,
            ));
        let length = length.store_err_into(out.reborrow())?;
        Ok(length)
    }

    #[cfg(not(any(unix, windows)))]
    fn length<'slot, 'borrow>(
        _: &Path,
        out: Out<'slot, 'borrow, HostError>,
    ) -> OResult<'slot, usize> {
        Err(out.fail(HostError::new(
            HostOperation::SchemaRead,
            HostErrorKind::InvalidRequest,
        )))
    }
}
