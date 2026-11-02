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

pub use opeo;

pub use budget::{BudgetError, ExecutionBudget, ExecutionLimits, FilledTerminalSlot, TerminalSlot};
pub use diagnostic::{
    Diagnostic, DiagnosticCode, DiagnosticContext, DiagnosticContextDescriptor, DiagnosticLabel,
    DiagnosticMessage, DiagnosticMessageKey, DiagnosticTranslationError,
};
pub use excerpt_capture::{SecretRange, SourceCapture};
pub use excerpt_limits::ExcerptLimits;
pub use excerpts::{ExcerptStatus, SourceExcerpt, SourceExcerptSet};
pub use input::{ByteSource, SliceSource, SourceReadError, SourceReadErrorKind};
pub use numeric::{FloatConversionError, NumericLiteral, NumericTokenKind};
pub use path::{InstancePath, PathSegment};
pub use source::{
    LineIndex, LocatedSpan, LocationError, LocationErrorKind, SourceId, SourceLocation, SourceSpan,
    Span, SpanError, SpanLocation,
};
pub use storage::WorkspaceString as ChargedString;
pub use value::{
    AllocationError, ConfigValue, ConfigValueBuilder, NamedValue, NamedValueEntryView,
    ValueEntries, ValueEntryView, ValueError, ValueErrorKind, ValueId, ValueKind, ValuePayload,
    ValuePayloadView, ValueSource, ValueView,
};
pub use workspace::WorkspaceVec as ChargedVec;
