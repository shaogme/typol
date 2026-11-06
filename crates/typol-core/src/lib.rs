#![no_std]

extern crate alloc;

mod budget;
mod budget_sort;
mod diagnostic;
mod excerpt_capture;
mod excerpt_limits;
mod excerpts;
mod input;
mod numeric;
mod optional_storage;
mod path;
mod source;
mod storage;
mod value;
mod workspace;

use core::{error::Error, fmt};

pub use opeo;

pub use budget::{BudgetError, ExecutionBudget, ExecutionLimits, FilledTerminalSlot, TerminalSlot};
pub use diagnostic::{
    Diagnostic, DiagnosticCode, DiagnosticContext, DiagnosticContextDescriptor, DiagnosticLabel,
    DiagnosticMessage, DiagnosticMessageKey, DiagnosticTranslationError,
};
pub use excerpt_capture::{SecretRange, SourceCapture};
pub use excerpt_limits::ExcerptLimits;
pub use excerpts::{ExcerptStatus, SourceExcerpt, SourceExcerptSet};
pub use input::{
    FmtWriteSink, Read, ReadRequest, SliceSource, SourceReadError, SourceReadErrorKind, Write,
    WriteError,
};
pub use numeric::{FloatConversionError, NumericLiteral, NumericTokenKind};
pub use path::{InstancePath, PathSegment};
pub use source::{
    LineIndex, LocatedSpan, LocationError, LocationErrorKind, SourceId, SourceLocation, SourceSpan,
    Span, SpanError, SpanLocation,
};
pub use storage::WorkspaceString as ChargedString;
pub use value::{
    AllocationError, ConfigValue, ConfigValueBuilder, NamedValue, NamedValueEntryView,
    ValueCopyLimits, ValueEntries, ValueEntryView, ValueError, ValueErrorKind, ValueId, ValueKind,
    ValuePayload, ValuePayloadView, ValueSource, ValueView,
};
pub use workspace::WorkspaceVec as ChargedVec;

/// The target state required before an atomic source replacement.
pub enum SourceExpectation<'snapshot> {
    /// Require exact byte equality with the supplied prior version.
    ExactSnapshot(&'snapshot [u8]),
    /// Use the snapshot retained by the caller as the expected prior version.
    DocumentSnapshot,
    /// Require that the target does not exist.
    MustBeAbsent,
    /// Replace any existing target explicitly.
    ReplaceAny,
}

/// A backend revision returned after an atomic source replacement succeeds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceRevision(u64);

/// A failure reported by a source writer or by the bounded document layer.
pub enum SourceCommitError<E> {
    ResourceLimitExceeded,
    AllocationFailed,
    Writer(E),
}

/// Performs bounded, transactional replacement of a platform-specific source target.
///
/// An implementation must keep the target unchanged for failures before the atomic commit and
/// return success only after the complete replacement is committed. If durability confirmation
/// can fail after visibility changes, the error must identify that the commit occurred and that
/// durability is uncertain.
pub trait SourceWriter {
    type Target: ?Sized;
    type Error;

    fn replace_atomically(
        &mut self,
        target: &Self::Target,
        expectation: SourceExpectation<'_>,
        replacement: &[u8],
        max_bytes: usize,
    ) -> Result<SourceRevision, Self::Error>;
}

impl SourceRevision {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u64 {
        self.0
    }
}

impl<E> fmt::Debug for SourceCommitError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ResourceLimitExceeded => formatter.write_str("ResourceLimitExceeded"),
            Self::AllocationFailed => formatter.write_str("AllocationFailed"),
            Self::Writer(_) => formatter.write_str("Writer(..)"),
        }
    }
}

impl<E: fmt::Display> fmt::Display for SourceCommitError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ResourceLimitExceeded => formatter.write_str("source write limit exceeded"),
            Self::AllocationFailed => formatter.write_str("source write allocation failed"),
            Self::Writer(error) => fmt::Display::fmt(error, formatter),
        }
    }
}

impl<E: Error + 'static> Error for SourceCommitError<E> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Writer(error) => Some(error),
            Self::ResourceLimitExceeded | Self::AllocationFailed => None,
        }
    }
}
