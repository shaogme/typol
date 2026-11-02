mod input;
mod paths;

#[cfg(test)]
mod allocation_tests;

use std::{fmt, fs::File, mem::take, path::Path};

use opeo::{ErrSlot, OResult, Out, ResultOutExt};
use typol_core::{BudgetError, ChargedVec, SourceId};
use typol_schema::{
    ResolverError, SchemaCompileContext, SchemaCompileStage, SchemaResolveRequest, SchemaResolver,
    SchemaSource,
};

use self::{
    input::{FileSchemaInput, OpenedSchemaFile},
    paths::{FileIdentity, NativePath, PathCandidate, PathResolutionParams},
};
use crate::{FileSchemaLimits, HostError, HostErrorKind, HostFailureStep, HostOperation};

pub use input::{FileConfigInput, ReadSource};

struct SelectedFile {
    path: NativePath,
    file: OpenedSchemaFile,
}

struct RegisteredFile {
    path: NativePath,
    identity: Option<FileIdentity>,
    _handle: File,
}

pub struct FileSchemaSource {
    context: SchemaCompileContext,
    identity: [u8; 8],
    input: FileSchemaInput,
    path: NativePath,
}

/// A path-identity index for one compilation, with no file-content cache across calls.
///
/// File metadata and contents are read through an open handle. On Linux with procfs available, the path is read from that handle into a bounded, metered buffer;
/// otherwise, `canonicalize` resolves the Schema path, and its internal temporary allocations are not bounded by `FileSchemaLimits::max_path_bytes`.
///
/// File metadata and contents are read through opened handles. On Linux with procfs available, paths
/// are read from those handles into a bounded charged buffer; otherwise `canonicalize` resolves
/// schema paths, and its internal temporary allocations are not bounded by `max_path_bytes`.
pub struct FileSchemaResolver {
    limits: FileSchemaLimits,
    context: SchemaCompileContext,
    files: ChargedVec<RegisteredFile>,
    current: Option<FileSchemaInput>,
    identity: [u8; 8],
    root_attempted: bool,
    failure_context: Option<HostError>,
    errors: ErrSlot<HostError>,
    selection_errors: ErrSlot<HostError>,
    budget_errors: ErrSlot<BudgetError>,
}

impl FileSchemaResolver {
    pub fn new<'slot>(
        limits: FileSchemaLimits,
        context: &SchemaCompileContext,
        mut out: Out<'slot, '_, BudgetError>,
    ) -> OResult<'slot, Self> {
        let context = context.try_clone(out.reborrow())?;
        let files = ChargedVec::new(Some(context.execution()), out.reborrow())?;
        Ok(Self {
            limits,
            context,
            files,
            current: None,
            identity: [0; 8],
            root_attempted: false,
            failure_context: None,
            errors: ErrSlot::new(),
            selection_errors: ErrSlot::new(),
            budget_errors: ErrSlot::new(),
        })
    }

    /// This must be the first root read in the session; relative paths are resolved against the current directory at the time of the call.
    pub fn read_root<'slot, 'borrow>(
        &mut self,
        path: &Path,
        out: Out<'slot, 'borrow, HostError>,
    ) -> OResult<'slot, FileSchemaSource> {
        let mut errors = take(&mut self.errors);
        let result = {
            let result = errors.try_call(|host_out| self.read_root_once(path, host_out));
            match result {
                Ok(source) => Ok(source),
                Err(caught) => Err(caught.take()),
            }
        };
        self.errors = errors;
        match result {
            Ok(source) => Ok(source),
            Err(error) => Err(out.fail(self.record_failure(error))),
        }
    }

    pub(crate) const fn failure_context(&self) -> Option<HostError> {
        self.failure_context
    }

    fn read_root_once<'slot>(
        &mut self,
        path: &Path,
        mut out: Out<'slot, '_, HostError>,
    ) -> OResult<'slot, FileSchemaSource> {
        if self.root_attempted || self.context.execution().finished() {
            return Err(out.fail(HostError::new(
                HostOperation::SchemaRead,
                HostErrorKind::InvalidRequest,
            )));
        }
        self.root_attempted = true;
        self.context.set_stage(SchemaCompileStage::RootRead);
        let budget = self.context.execution();
        budget.consume_with(1, out.reborrow(), |error| {
            HostError::budget(HostOperation::SchemaRead, error)
        })?;
        let resources = self.context.resources();
        if self.limits.max_files() == 0 || resources.max_schema_files() == 0 {
            return Err(out.fail(HostError::new(
                HostOperation::SchemaRead,
                HostErrorKind::ResourceLimitExceeded,
            )));
        }
        let source_byte_limit = resources
            .max_source_bytes()
            .min(resources.max_schema_total_bytes());
        let opened = OpenedSchemaFile::open(path, HostOperation::SchemaRead, out.reborrow())?;
        let file_identity = opened.identity();
        let path = NativePath::from_opened_file(
            path,
            opened.handle(),
            PathResolutionParams {
                max_path_bytes: self.limits.max_path_bytes(),
                budget,
                operation: HostOperation::SchemaRead,
            },
            out.reborrow(),
        )?;
        let source_path = path.copy(budget, HostOperation::SchemaRead, out.reborrow())?;
        let (input, pin) = opened.into_input_and_pin(
            source_byte_limit.min(self.limits.max_file_bytes()),
            budget,
            HostOperation::SchemaRead,
            out.reborrow(),
        )?;
        let identity = self.register(path, file_identity, pin, out.reborrow())?;
        let context = {
            let source_context = &self.context;
            match self
                .budget_errors
                .try_call(|budget_out| source_context.try_clone(budget_out))
            {
                Ok(context) => context,
                Err(caught) => {
                    return Err(
                        out.fail(HostError::budget(HostOperation::SchemaRead, *caught.get()))
                    );
                }
            }
        };
        Ok(FileSchemaSource {
            identity,
            input,
            path: source_path,
            context,
        })
    }

    fn register<'slot>(
        &mut self,
        path: NativePath,
        file_identity: Option<FileIdentity>,
        handle: File,
        mut out: Out<'slot, '_, HostError>,
    ) -> OResult<'slot, [u8; 8]> {
        let budget = self.context.execution();
        for (index, existing) in self.files.iter().enumerate() {
            budget.consume_with(1, out.reborrow(), |error| {
                HostError::budget(HostOperation::SchemaRead, error)
            })?;
            let same_file = match (file_identity, existing.identity) {
                (Some(identity), Some(existing_identity)) => identity == existing_identity,
                (None, None) => existing.path.equals(&path, budget, out.reborrow())?,
                _ => false,
            };
            if same_file {
                let identity = u64::try_from(index)
                    .map(u64::to_le_bytes)
                    .map_err(|_| {
                        HostError::new(
                            HostOperation::SchemaRead,
                            HostErrorKind::ResourceLimitExceeded,
                        )
                    })
                    .store_err_into(out.reborrow())?;
                return Ok(identity);
            }
        }
        if self.files.len() >= self.limits.max_files() {
            return Err(out.fail(HostError::new(
                HostOperation::SchemaRead,
                HostErrorKind::ResourceLimitExceeded,
            )));
        }
        let identity = u64::try_from(self.files.len())
            .map_err(|_| {
                HostError::new(
                    HostOperation::SchemaRead,
                    HostErrorKind::ResourceLimitExceeded,
                )
            })
            .map(u64::to_le_bytes)
            .store_err_into(out.reborrow())?;
        HostError::run_budgeted(
            HostOperation::SchemaRead,
            None,
            out.reborrow(),
            |budget_out| self.files.try_reserve(1, budget_out),
        )?;
        HostError::run_budgeted(
            HostOperation::SchemaRead,
            None,
            out.reborrow(),
            |budget_out| {
                self.files.try_push(
                    RegisteredFile {
                        path,
                        identity: file_identity,
                        _handle: handle,
                    },
                    budget_out,
                )
            },
        )?;
        Ok(identity)
    }

    fn import<'slot>(
        &mut self,
        from: &[u8],
        module: &str,
        limit: usize,
        mut out: Out<'slot, '_, HostError>,
    ) -> OResult<'slot, ()> {
        let operation = HostOperation::SchemaRead;
        let budget = self.context.execution();
        budget.consume_with(from.len(), out.reborrow(), |error| {
            HostError::budget(operation, error)
        })?;
        if module.len() > self.limits.max_name_bytes() {
            return Err(out.fail(HostError::new(
                operation,
                HostErrorKind::ResourceLimitExceeded,
            )));
        }
        budget.consume_with(module.len(), out.reborrow(), |error| {
            HostError::budget(operation, error)
        })?;
        let identity = <[u8; 8]>::try_from(from)
            .map_err(|_| HostError::new(operation, HostErrorKind::InvalidRequest))
            .store_err_into(out.reborrow())?;
        let index = usize::try_from(u64::from_le_bytes(identity))
            .map_err(|_| HostError::new(operation, HostErrorKind::InvalidRequest))
            .store_err_into(out.reborrow())?;
        if !valid_module(module) || !self.root_attempted {
            return Err(out.fail(HostError::new(operation, HostErrorKind::InvalidRequest)));
        }
        let mut selection_errors = take(&mut self.selection_errors);
        let selection = select_candidate(
            &mut selection_errors,
            |selection_out| self.select(index, module, false, selection_out),
            |selection_out| self.select(index, module, true, selection_out),
            out.reborrow(),
        );
        self.selection_errors = selection_errors;
        let selected = selection?;
        // Once the file is open, later read or content errors do not trigger fallback lookup.
        self.current = None;
        let file_identity = selected.file.identity();
        let (input, pin) = selected.file.into_input_and_pin(
            limit.min(self.limits.max_file_bytes()),
            budget,
            operation,
            out.reborrow(),
        )?;
        self.identity = self.register(selected.path, file_identity, pin, out.reborrow())?;
        self.current = Some(input);
        Ok(())
    }

    fn select<'slot>(
        &self,
        index: usize,
        module: &str,
        alternate: bool,
        mut out: Out<'slot, '_, HostError>,
    ) -> OResult<'slot, SelectedFile> {
        let operation = HostOperation::SchemaRead;
        let budget = self.context.execution();
        let base = self
            .files
            .get(index)
            .ok_or(HostError::new(operation, HostErrorKind::InvalidRequest))
            .store_err_into(out.reborrow())?;
        let candidate = base.path.candidate(
            PathCandidate { module, alternate },
            self.limits.max_path_bytes(),
            budget,
            out.reborrow(),
        )?;
        let file = OpenedSchemaFile::open(candidate.path(), operation, out.reborrow())?;
        let path = NativePath::from_opened_file(
            candidate.path(),
            file.handle(),
            PathResolutionParams {
                max_path_bytes: self.limits.max_path_bytes(),
                budget,
                operation,
            },
            out.reborrow(),
        )?;
        budget.consume_with(1, out.reborrow(), |error| {
            HostError::budget(operation, error)
        })?;
        Ok(SelectedFile { path, file })
    }

    fn record_failure(&mut self, error: HostError) -> HostError {
        self.failure_context = Some(error);
        let failure = match error.kind() {
            HostErrorKind::ResourceLimitExceeded => Some(BudgetError::Resource),
            HostErrorKind::AllocationFailed => Some(BudgetError::Allocation),
            HostErrorKind::CapacityExceeded => Some(BudgetError::CapacityExceeded),
            HostErrorKind::InternalError => Some(BudgetError::invariant(error.budget_context())),
            _ => None,
        };
        if let Some(failure) = failure {
            self.context.execution().record_failure(failure);
        }
        error
    }
}

impl SchemaResolver for FileSchemaResolver {
    fn resolve<'src, 'slot>(
        &'src mut self,
        request: SchemaResolveRequest<'_>,
        mut out: Out<'slot, '_, ResolverError>,
    ) -> OResult<'slot, SchemaSource<'src>> {
        if !self.context.same_session(request.context) {
            return Err(out.fail(ResolverError::ContextMismatch));
        }
        if self.context.execution().finished() {
            return Err(out.fail(ResolverError::HostFailure));
        }
        self.context
            .execution()
            .consume_with(0, out.reborrow(), |_| ResolverError::ResourceLimitExceeded)?;
        let source_id = <[u8; 8]>::try_from(request.from_identity)
            .ok()
            .map(u64::from_le_bytes)
            .and_then(|identity| u32::try_from(identity).ok())
            .map(SourceId::new);
        let mut errors = take(&mut self.errors);
        let error = {
            let result = errors.try_call(|host_out| {
                self.import(
                    request.from_identity,
                    request.module_name,
                    request.source_byte_limit,
                    host_out,
                )
            });
            match result {
                Ok(()) => None,
                Err(caught) => Some(caught.take()),
            }
        };
        self.errors = errors;
        if let Some(error) = error {
            let error = match source_id {
                Some(source_id) => error.with_source_id(source_id),
                None => error,
            };
            self.record_failure(error);
            return Err(out.fail(resolver_error(error)));
        }
        let Some(current_input) = self.current.as_mut() else {
            let error = HostError::internal(
                HostOperation::SchemaRead,
                HostFailureStep::FileBufferInvariant,
            );
            let error = match source_id {
                Some(source_id) => error.with_source_id(source_id),
                None => error,
            };
            self.failure_context = Some(error);
            self.context
                .execution()
                .record_failure(BudgetError::invariant(error.budget_context()));
            return Err(out.fail(ResolverError::InternalInvariantFailure));
        };
        current_input.claim();
        Ok(SchemaSource::from_source_in_context(
            &self.identity,
            current_input,
            &self.context,
        ))
    }
}

impl FileSchemaSource {
    pub fn source(&mut self) -> SchemaSource<'_> {
        self.input.claim();
        SchemaSource::from_source_in_context(&self.identity, &mut self.input, &self.context)
    }

    /// Returns the path label used for display and relative imports; temporary allocations by the path backend are described on [`FileSchemaResolver`].
    pub fn path(&self) -> &Path {
        self.path.path()
    }
}

impl fmt::Debug for FileSchemaSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FileSchemaSource")
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for FileSchemaResolver {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FileSchemaResolver")
            .field("file_count", &self.files.len())
            .finish_non_exhaustive()
    }
}

fn select_candidate<'slot, 'borrow>(
    errors: &mut ErrSlot<HostError>,
    preferred: impl for<'call> FnOnce(Out<'call, 'call, HostError>) -> OResult<'call, SelectedFile>,
    alternate: impl for<'call> FnOnce(Out<'call, 'call, HostError>) -> OResult<'call, SelectedFile>,
    out: Out<'slot, 'borrow, HostError>,
) -> OResult<'slot, SelectedFile> {
    let error = {
        let result = errors.try_call(preferred);
        match result {
            Ok(selected) => return Ok(selected),
            Err(caught) => caught.take(),
        }
    };
    if error.kind() != HostErrorKind::NotFound {
        return Err(out.fail(error));
    }
    match errors.try_call(alternate) {
        Ok(selected) => Ok(selected),
        Err(caught) => Err(out.fail(caught.take())),
    }
}

fn resolver_error(error: HostError) -> ResolverError {
    match error.kind() {
        HostErrorKind::NotFound => ResolverError::NotFound,
        HostErrorKind::ResourceLimitExceeded => ResolverError::ResourceLimitExceeded,
        HostErrorKind::AllocationFailed => ResolverError::AllocationFailed,
        HostErrorKind::CapacityExceeded => ResolverError::CapacityExceeded,
        HostErrorKind::InternalError => ResolverError::InternalInvariantFailure,
        _ => ResolverError::HostFailure,
    }
}

fn valid_module(module: &str) -> bool {
    let mut bytes = module.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        && !matches!(
            module,
            "extern"
                | "mod"
                | "type"
                | "struct"
                | "entry"
                | "enum"
                | "option"
                | "map"
                | "i8"
                | "i16"
                | "i32"
                | "i64"
                | "i128"
                | "u8"
                | "u16"
                | "u32"
                | "u64"
                | "u128"
                | "f32"
                | "f64"
                | "string"
                | "bool"
                | "true"
                | "false"
                | "none"
        )
}

#[cfg(test)]
#[test]
fn controlled_selection_failures_only_fall_back_for_not_found() {
    for kind in [
        HostErrorKind::PermissionDenied,
        HostErrorKind::NotRegularFile,
        HostErrorKind::ReadFailure,
        HostErrorKind::ResourceLimitExceeded,
        HostErrorKind::AllocationFailed,
        HostErrorKind::NotFound,
    ] {
        let mut calls = 0;
        let mut errors = ErrSlot::<HostError>::new();
        let mut call_errors = ErrSlot::<HostError>::new();
        let result = call_errors.try_call(|out| {
            select_candidate(
                &mut errors,
                |out: Out<'_, '_, HostError>| {
                    Err(out.fail(HostError::new(HostOperation::SchemaRead, kind)))
                },
                |out: Out<'_, '_, HostError>| {
                    calls += 1;
                    Err(out.fail(HostError::new(
                        HostOperation::SchemaRead,
                        HostErrorKind::InvalidRequest,
                    )))
                },
                out,
            )
        });
        assert!(result.is_err());
        assert_eq!(calls, usize::from(kind == HostErrorKind::NotFound));
    }
}
