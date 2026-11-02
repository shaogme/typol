//! Bounded file reads, process-environment access, Schema file parsing, and source-name registration within a session.
//!
//!
//! File paths are exposed only through explicit accessors; default errors and debug output omit paths and source text.
//! File, path, and environment limits constrain accepted or retained data; `ExecutionLimits` tracks only workspace explicitly charged to the execution budget.
//! These limits do not bound total process memory or temporary allocations inside operating-system calls or the standard library; allocation failure may not be recoverable.
//! The adapters provide neither a directory snapshot nor a sandbox.
//! On Linux with procfs available, schema canonical paths and configuration path labels are read from opened handles into a charged buffer of at most `max_path_bytes + 1` bytes.
//! Schema paths on other platforms, or Linux without procfs, fall back to standard-library canonicalization whose temporary allocation is not bounded by the path limit.
//! Configuration path labels fall back to the requested path when the opened handle path is unavailable; they do not guarantee the identity of the opened file.
//! `ProcessEnvironment` checks value length after `var_os` returns an owned `OsString`; the query's allocation is not bounded by the value limit.
//! On Unix it adopts and charges that byte buffer after validation; on Windows it converts the value into a charged UTF-8 buffer. A caller may inject another `Environment` provider for a different lookup policy.

mod diagnostics;
mod display;
mod environment;
mod filesystem;
mod host;
mod schema_compiler;
mod schema_limits;
mod sources;

pub use opeo;

pub use diagnostics::{
    DiagnosticContext, DiagnosticRenderError, DiagnosticRenderer, DiagnosticReport,
    DiagnosticReportStage, DiagnosticReportTitle, DisplayError, DisplayFailurePoint, RenderOptions,
};
pub use display::{DisplayOptions, SafeValueFormatter};
pub use environment::ProcessEnvironment;
#[cfg(all(test, windows))]
pub(crate) use environment::{EnvironmentBytes, EnvironmentFailure};
pub use filesystem::{FileConfigInput, FileSchemaResolver, FileSchemaSource, ReadSource};
pub use host::{
    FileConfigInputLimitParams, FileConfigInputLimits, HostError, HostErrorKind, HostFailureStep,
    HostOperation, ProcessEnvironmentLimitParams, ProcessEnvironmentLimits,
};
pub use sources::SourceNames;

pub use schema_compiler::{
    FileSchemaCompilation, FileSchemaCompileErrors, FileSchemaCompileFailure,
    FileSchemaCompileOptions, FileSchemaCompiler,
};
pub use schema_limits::{
    FileSchemaLimitParams, FileSchemaLimits, SourceNameEditOptions, SourceNameLimits,
};
