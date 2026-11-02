use opeo::{OResult, Out};

use crate::{BudgetError, DiagnosticCode, DiagnosticContext, SourceId, diagnostic_context};
use core::{error::Error, fmt};

static SLICE_SOURCE_OFFSET: DiagnosticContext =
    diagnostic_context!("typol-core", "input", "slice-offset-out-of-bounds");
static SLICE_SOURCE_END: DiagnosticContext =
    diagnostic_context!("typol-core", "input", "slice-end-overflow");
static SLICE_SOURCE_RANGE: DiagnosticContext =
    diagnostic_context!("typol-core", "input", "slice-range-unavailable");
static SLICE_OUTPUT_RANGE: DiagnosticContext =
    diagnostic_context!("typol-core", "input", "output-range-unavailable");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceReadErrorKind {
    ResourceLimitExceeded,
    AllocationFailed,
    CapacityExceeded,
    HostFailure,
    InternalInvariantFailure,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceReadError {
    kind: SourceReadErrorKind,
    offset: Option<usize>,
    requested_bytes: Option<usize>,
    consumed_bytes: Option<usize>,
    context: Option<DiagnosticContext>,
    source_id: Option<SourceId>,
}

/// Sequential input writes only to nonempty buffers; short reads are not EOF, while zero bytes are permanent EOF.
pub trait ByteSource<E = SourceReadError> {
    fn read<'slot, 'borrow>(
        &mut self,
        output: &mut [u8],
        source_id: SourceId,
        offset: usize,
        out: Out<'slot, 'borrow, E>,
    ) -> OResult<'slot, usize>;
}

impl SourceReadError {
    pub const fn new(kind: SourceReadErrorKind) -> Self {
        Self {
            kind,
            offset: None,
            requested_bytes: None,
            consumed_bytes: None,
            context: None,
            source_id: None,
        }
    }

    pub const fn with_offset(mut self, offset: usize) -> Self {
        self.offset = Some(offset);
        self
    }

    pub const fn with_requested_bytes(mut self, requested_bytes: usize) -> Self {
        self.requested_bytes = Some(requested_bytes);
        self
    }

    pub const fn with_consumed_bytes(mut self, consumed_bytes: usize) -> Self {
        self.consumed_bytes = Some(consumed_bytes);
        self
    }

    pub const fn with_context(mut self, context: DiagnosticContext) -> Self {
        self.context = Some(context);
        self
    }

    pub const fn with_input(mut self, source_id: SourceId, offset: usize) -> Self {
        self.source_id = Some(source_id);
        if self.offset.is_none() {
            self.offset = Some(offset);
        }
        self
    }

    pub const fn kind(self) -> SourceReadErrorKind {
        self.kind
    }

    pub const fn offset(self) -> Option<usize> {
        self.offset
    }

    pub const fn requested_bytes(self) -> Option<usize> {
        self.requested_bytes
    }

    pub const fn consumed_bytes(self) -> Option<usize> {
        self.consumed_bytes
    }

    pub const fn context(self) -> Option<DiagnosticContext> {
        self.context
    }

    pub const fn source_id(self) -> Option<SourceId> {
        self.source_id
    }
}

impl fmt::Display for SourceReadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self.kind {
            SourceReadErrorKind::ResourceLimitExceeded => "input resource limit exceeded",
            SourceReadErrorKind::AllocationFailed => "input buffer allocation failed",
            SourceReadErrorKind::CapacityExceeded => "input buffer capacity exceeded",
            SourceReadErrorKind::HostFailure => "host input failed",
            SourceReadErrorKind::InternalInvariantFailure => "input source invariant violated",
        };
        formatter.write_str(message)
    }
}

impl Error for SourceReadError {}

impl SourceReadError {
    pub const fn diagnostic_code(self) -> DiagnosticCode {
        match self.kind {
            SourceReadErrorKind::ResourceLimitExceeded => DiagnosticCode::RESOURCE_LIMIT_EXCEEDED,
            SourceReadErrorKind::AllocationFailed => DiagnosticCode::ALLOCATION_FAILED,
            SourceReadErrorKind::CapacityExceeded => DiagnosticCode::CAPACITY_EXCEEDED,
            SourceReadErrorKind::HostFailure => DiagnosticCode::SOURCE_INPUT_ERROR,
            SourceReadErrorKind::InternalInvariantFailure => DiagnosticCode::INTERNAL_ERROR,
        }
    }
}

/// In-memory and host input follow the same consumption and limit-exceeded ordering.
pub struct SliceSource<'input> {
    bytes: &'input [u8],
    offset: usize,
}

impl<'input> SliceSource<'input> {
    pub const fn new(bytes: &'input [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    pub const fn total_bytes(&self) -> usize {
        self.bytes.len()
    }

    pub const fn consumed_bytes(&self) -> usize {
        self.offset
    }

    pub fn remaining_bytes(&self) -> usize {
        self.bytes.len().saturating_sub(self.offset)
    }

    pub fn is_finished(&self) -> bool {
        self.remaining_bytes() == 0
    }

    fn copy_next<'slot, 'borrow, E>(
        &mut self,
        output: &mut [u8],
        out: Out<'slot, 'borrow, E>,
        convert: impl Fn(SourceReadError) -> E,
    ) -> OResult<'slot, usize> {
        if self.offset > self.bytes.len() {
            let error = SourceReadError::new(SourceReadErrorKind::InternalInvariantFailure)
                .with_offset(self.offset)
                .with_requested_bytes(output.len())
                .with_context(SLICE_SOURCE_OFFSET);
            return Err(out.fail(convert(error)));
        }
        let count = output.len().min(self.remaining_bytes());
        let Some(end) = self.offset.checked_add(count) else {
            let error = SourceReadError::new(SourceReadErrorKind::InternalInvariantFailure)
                .with_offset(self.offset)
                .with_requested_bytes(count)
                .with_context(SLICE_SOURCE_END);
            return Err(out.fail(convert(error)));
        };
        let Some(source) = self.bytes.get(self.offset..end) else {
            let error = SourceReadError::new(SourceReadErrorKind::InternalInvariantFailure)
                .with_offset(self.offset)
                .with_requested_bytes(count)
                .with_context(SLICE_SOURCE_RANGE);
            return Err(out.fail(convert(error)));
        };
        let Some(destination) = output.get_mut(..count) else {
            let error = SourceReadError::new(SourceReadErrorKind::InternalInvariantFailure)
                .with_offset(self.offset)
                .with_requested_bytes(count)
                .with_context(SLICE_OUTPUT_RANGE);
            return Err(out.fail(convert(error)));
        };
        destination.copy_from_slice(source);
        self.offset = end;
        Ok(count)
    }
}

impl<E> ByteSource<E> for SliceSource<'_>
where
    E: From<SourceReadError>,
{
    fn read<'slot>(
        &mut self,
        output: &mut [u8],
        source_id: SourceId,
        offset: usize,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, usize> {
        let count = self.copy_next(output, out.reborrow(), |error| {
            E::from(error.with_input(source_id, offset))
        })?;
        Ok(count)
    }
}

impl fmt::Debug for SliceSource<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SliceSource")
            .field("total_bytes", &self.total_bytes())
            .field("consumed_bytes", &self.consumed_bytes())
            .field("remaining_bytes", &self.remaining_bytes())
            .finish()
    }
}

impl From<BudgetError> for SourceReadError {
    fn from(error: BudgetError) -> Self {
        let context = error.context();
        let kind = match error {
            BudgetError::Resource => SourceReadErrorKind::ResourceLimitExceeded,
            BudgetError::Allocation => SourceReadErrorKind::AllocationFailed,
            BudgetError::CapacityExceeded => SourceReadErrorKind::CapacityExceeded,
            BudgetError::InvariantViolation(_) => SourceReadErrorKind::InternalInvariantFailure,
        };
        let error = Self::new(kind);
        match context {
            Some(context) => error.with_context(context),
            None => error,
        }
    }
}
