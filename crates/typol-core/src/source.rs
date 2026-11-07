use crate::{
    AllocationError, BudgetError, DiagnosticContext, ExecutionBudget, diagnostic_context,
    workspace::WorkspaceVec,
};
use core::{cmp::min, error::Error, fmt, num::NonZeroUsize};
use opeo::{OResult, Out, ResultOutExt};

static LOCATION_EMPTY_INDEX: DiagnosticContext =
    diagnostic_context!("typol-core", "source-location", "empty-line-index");
static LOCATION_LINE_START: DiagnosticContext =
    diagnostic_context!("typol-core", "source-location", "line-start-missing");
static LOCATION_NEXT_LINE: DiagnosticContext =
    diagnostic_context!("typol-core", "source-location", "next-line-index-overflow");
static LOCATION_LINE_SLICE: DiagnosticContext =
    diagnostic_context!("typol-core", "source-location", "line-slice-boundary");
static LOCATION_LINE_NUMBER: DiagnosticContext =
    diagnostic_context!("typol-core", "source-location", "line-number-overflow");
static LOCATION_COLUMN_NUMBER: DiagnosticContext =
    diagnostic_context!("typol-core", "source-location", "column-number-overflow");
static LOCATION_LINE_END: DiagnosticContext =
    diagnostic_context!("typol-core", "source-location", "line-end-underflow");
static LOCATION_BUDGET: DiagnosticContext =
    diagnostic_context!("typol-core", "source-location", "budget-accounting-failure");

/// An opaque source handle used within one compilation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SourceId(u32);

impl SourceId {
    /// Creates a source handle from an integer assigned by the caller.
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// Returns the handle’s integer value.
    pub const fn get(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpanError {
    ReversedRange { start: usize, end: usize },
}

impl SpanError {
    pub const fn attempted_range(self) -> (usize, usize) {
        match self {
            Self::ReversedRange { start, end } => (start, end),
        }
    }
}

/// A half-open byte range `[start, end)` in UTF-8 source text.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Span {
    start: usize,
    end: usize,
}

impl Span {
    pub const fn empty() -> Self {
        Self { start: 0, end: 0 }
    }

    /// Creates a zero-length point range at the specified byte offset.
    pub const fn point(offset: usize) -> Self {
        Self {
            start: offset,
            end: offset,
        }
    }

    /// Creates a range from the start of the source text to the specified end.
    pub const fn from_zero_to(end: usize) -> Self {
        Self { start: 0, end }
    }

    /// Creates a non-inverted byte range.
    pub fn new<'slot, 'borrow>(
        start: usize,
        end: usize,
        out: Out<'slot, 'borrow, SpanError>,
    ) -> OResult<'slot, Self> {
        if start <= end {
            Ok(Self { start, end })
        } else {
            Err(out.fail(SpanError::ReversedRange { start, end }))
        }
    }

    /// Converts an inverted-range error into the caller’s error slot.
    pub fn new_with<'slot, 'borrow, E>(
        start: usize,
        end: usize,
        out: Out<'slot, 'borrow, E>,
        convert: impl FnOnce(SpanError) -> E,
    ) -> OResult<'slot, Self> {
        if start <= end {
            Ok(Self { start, end })
        } else {
            Err(out.fail(convert(SpanError::ReversedRange { start, end })))
        }
    }

    pub(crate) const fn from_ordered(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    pub const fn start(self) -> usize {
        self.start
    }

    pub const fn end(self) -> usize {
        self.end
    }

    pub const fn len(self) -> usize {
        self.end - self.start
    }

    pub const fn is_empty(self) -> bool {
        self.start == self.end
    }
}

/// A location associating a source handle with a byte range.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SourceSpan {
    source_id: SourceId,
    span: Span,
}

impl SourceSpan {
    pub const fn new(source_id: SourceId, span: Span) -> Self {
        Self { source_id, span }
    }

    pub const fn source_id(self) -> SourceId {
        self.source_id
    }

    pub const fn span(self) -> Span {
        self.span
    }
}

/// A 1-based line number and Unicode scalar column number.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SourceLocation {
    line: NonZeroUsize,
    column: NonZeroUsize,
}

impl SourceLocation {
    pub const fn new(line: usize, column: usize) -> Option<Self> {
        let Some(line) = NonZeroUsize::new(line) else {
            return None;
        };
        let Some(column) = NonZeroUsize::new(column) else {
            return None;
        };
        Some(Self { line, column })
    }

    pub const fn from_nonzero(line: NonZeroUsize, column: NonZeroUsize) -> Self {
        Self { line, column }
    }

    pub const fn line(self) -> usize {
        self.line.get()
    }

    pub const fn column(self) -> usize {
        self.column.get()
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SpanLocation {
    pub start: SourceLocation,
    pub end: SourceLocation,
}

/// An inline location lives only as long as the instance or diagnostic that needs it and does not retain source text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LocatedSpan {
    span: SourceSpan,
    location: Option<SpanLocation>,
}

impl LocatedSpan {
    pub const fn new(span: SourceSpan, location: Option<SpanLocation>) -> Self {
        Self { span, location }
    }

    pub const fn source_span(self) -> SourceSpan {
        self.span
    }

    pub const fn location(self) -> Option<SpanLocation> {
        self.location
    }

    pub const fn source_id(self) -> SourceId {
        self.span.source_id()
    }

    pub const fn span(self) -> Span {
        self.span.span()
    }

    pub const fn with_location(mut self, location: Option<SpanLocation>) -> Self {
        self.location = location;
        self
    }

    pub fn through(self, end: Self) -> Option<Self> {
        if self.source_id() != end.source_id()
            || self.span().start() > end.span().start()
            || self.span().end() > end.span().end()
        {
            return None;
        }
        let span = Span::from_ordered(self.span().start(), end.span().end());
        let location = self
            .location
            .zip(end.location)
            .map(|(start, end)| SpanLocation {
                start: start.start,
                end: end.end,
            });
        Some(Self::new(SourceSpan::new(self.source_id(), span), location))
    }
}

impl From<SourceSpan> for LocatedSpan {
    fn from(span: SourceSpan) -> Self {
        Self::new(span, None)
    }
}

/// An invalid offset encountered while querying a source location.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocationErrorKind {
    OffsetOutOfBounds,
    NotCharBoundary,
    ResourceLimitExceeded,
    InternalInvariantFailure,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LocationError {
    kind: LocationErrorKind,
    offset: Option<usize>,
    related_offset: Option<usize>,
    context: Option<DiagnosticContext>,
}

impl LocationError {
    pub const fn new(kind: LocationErrorKind) -> Self {
        Self {
            kind,
            offset: None,
            related_offset: None,
            context: None,
        }
    }

    pub const fn with_offset(mut self, offset: usize) -> Self {
        self.offset = Some(offset);
        self
    }

    pub const fn with_related_offset(mut self, offset: usize) -> Self {
        self.related_offset = Some(offset);
        self
    }

    pub const fn with_context(mut self, context: DiagnosticContext) -> Self {
        self.context = Some(context);
        self
    }

    pub const fn kind(self) -> LocationErrorKind {
        self.kind
    }

    pub const fn offset(self) -> Option<usize> {
        self.offset
    }

    pub const fn related_offset(self) -> Option<usize> {
        self.related_offset
    }

    pub const fn context(self) -> Option<DiagnosticContext> {
        self.context
    }

    fn internal(context: DiagnosticContext, offset: usize) -> Self {
        Self::new(LocationErrorKind::InternalInvariantFailure)
            .with_offset(offset)
            .with_context(context)
    }
}

impl fmt::Display for LocationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self.kind {
            LocationErrorKind::OffsetOutOfBounds => "source position is out of bounds",
            LocationErrorKind::NotCharBoundary => {
                "source position is not a UTF-8 character boundary"
            }
            LocationErrorKind::ResourceLimitExceeded => "source location exceeds resource limits",
            LocationErrorKind::InternalInvariantFailure => {
                "source location index invariant violated"
            }
        };
        formatter.write_str(message)
    }
}

impl Error for LocationError {}

/// Borrows UTF-8 source text and caches the starting byte offset of each line.
pub struct LineIndex<'a> {
    source: &'a str,
    line_starts: WorkspaceVec<usize>,
}

impl<'a> LineIndex<'a> {
    /// Builds a line-start index for source text; only LF and CRLF begin a new line.
    pub fn new<'slot, 'borrow>(
        source: &'a str,
        out: Out<'slot, 'borrow, AllocationError>,
    ) -> OResult<'slot, Self> {
        Self::new_budgeted(source, None, out)
    }

    pub fn new_budgeted<'slot, E>(
        source: &'a str,
        budget: Option<&ExecutionBudget>,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, Self>
    where
        E: From<BudgetError>,
    {
        let mut line_count = 1usize;
        for byte in source.bytes() {
            if let Some(budget) = budget {
                budget.consume(1, out.reborrow())?;
            }
            if byte == b'\n' {
                line_count = line_count
                    .checked_add(1)
                    .ok_or(BudgetError::Resource)
                    .store_err_into(out.reborrow())?;
            }
        }
        let mut line_starts = WorkspaceVec::new(budget, out.reborrow())?;
        line_starts.try_reserve_exact(line_count, out.reborrow())?;
        line_starts.try_push(0, out.reborrow())?;
        for (offset, byte) in source.bytes().enumerate() {
            if let Some(budget) = budget {
                budget.consume(1, out.reborrow())?;
            }
            if byte == b'\n' {
                line_starts.try_push(offset + 1, out.reborrow())?;
            }
        }
        Ok(Self {
            source,
            line_starts,
        })
    }

    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    /// Converts a UTF-8 byte offset to a 1-based line number and Unicode scalar column number.
    pub fn location<'slot>(
        &self,
        offset: usize,
        mut out: Out<'slot, '_, LocationError>,
    ) -> OResult<'slot, SourceLocation> {
        if offset > self.source.len() {
            return Err(out.fail(
                LocationError::new(LocationErrorKind::OffsetOutOfBounds)
                    .with_offset(offset)
                    .with_related_offset(self.source.len()),
            ));
        }
        if !self.source.is_char_boundary(offset) {
            return Err(out
                .fail(LocationError::new(LocationErrorKind::NotCharBoundary).with_offset(offset)));
        }

        let mut comparisons = 0;
        let line_count = self.line_starts.partition_point(|start| {
            comparisons += 1;
            *start <= offset
        });
        self.charge(comparisons, offset, out.reborrow())?;
        let line_index = line_count
            .checked_sub(1)
            .ok_or_else(|| LocationError::internal(LOCATION_EMPTY_INDEX, offset))
            .store_err_into(out.reborrow())?;
        let line_start = self
            .line_starts
            .get(line_index)
            .copied()
            .ok_or_else(|| LocationError::internal(LOCATION_LINE_START, offset))
            .store_err_into(out.reborrow())?;
        let next_index = line_index
            .checked_add(1)
            .ok_or_else(|| LocationError::internal(LOCATION_NEXT_LINE, offset))
            .store_err_into(out.reborrow())?;
        let line_end = self
            .line_starts
            .get(next_index)
            .copied()
            .unwrap_or(self.source.len());
        let content_end = self.line_content_end(line_start, line_end, offset, out.reborrow())?;
        let column_end = min(offset, content_end);
        let line_text = self
            .source
            .get(line_start..column_end)
            .ok_or_else(|| {
                LocationError::internal(LOCATION_LINE_SLICE, offset).with_related_offset(line_start)
            })
            .store_err_into(out.reborrow())?;

        let mut column = 1;
        for byte in line_text.bytes() {
            self.charge(1, offset, out.reborrow())?;
            if byte & 0xc0 != 0x80 {
                column += 1;
            }
        }
        let line = line_index
            .checked_add(1)
            .and_then(NonZeroUsize::new)
            .ok_or_else(|| LocationError::internal(LOCATION_LINE_NUMBER, offset))
            .store_err_into(out.reborrow())?;
        let column = NonZeroUsize::new(column)
            .ok_or_else(|| LocationError::internal(LOCATION_COLUMN_NUMBER, offset))
            .store_err_into(out.reborrow())?;
        Ok(SourceLocation::from_nonzero(line, column))
    }

    fn charge<'slot>(
        &self,
        count: usize,
        offset: usize,
        mut out: Out<'slot, '_, LocationError>,
    ) -> OResult<'slot, ()> {
        if let Some(budget) = self.line_starts.budget() {
            budget.consume_with(count, out.reborrow(), |error| match error {
                BudgetError::Resource
                | BudgetError::RegexSteps
                | BudgetError::RegexSearchSteps
                | BudgetError::RegexWorkspace => {
                    LocationError::new(LocationErrorKind::ResourceLimitExceeded).with_offset(offset)
                }
                BudgetError::Allocation | BudgetError::CapacityExceeded => {
                    LocationError::internal(LOCATION_BUDGET, offset)
                }
                BudgetError::InvariantViolation(context) => {
                    LocationError::internal(*context, offset)
                }
            })?;
        }
        Ok(())
    }

    fn line_content_end<'slot>(
        &self,
        line_start: usize,
        line_end: usize,
        offset: usize,
        mut out: Out<'slot, '_, LocationError>,
    ) -> OResult<'slot, usize> {
        if line_end <= line_start {
            return Ok(line_end);
        }

        self.charge(1, offset, out.reborrow())?;
        let bytes = self.source.as_bytes();
        let last_index = line_end
            .checked_sub(1)
            .ok_or_else(|| LocationError::internal(LOCATION_LINE_END, offset))
            .store_err_into(out.reborrow())?;
        if bytes.get(last_index) == Some(&b'\n') {
            self.charge(1, offset, out.reborrow())?;
            let cr_index = last_index.checked_sub(1);
            if cr_index
                .and_then(|index| bytes.get(index))
                .is_some_and(|byte| *byte == b'\r')
            {
                Ok(last_index - 1)
            } else {
                Ok(last_index)
            }
        } else {
            Ok(line_end)
        }
    }
}

impl fmt::Debug for LineIndex<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LineIndex")
            .field("line_count", &self.line_starts.len())
            .field("source_bytes", &self.source.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::LineIndex;
    use alloc::boxed::Box;
    use core::{error::Error, fmt};
    use opeo::{ErrSlot, OResult, Out};

    #[derive(Debug)]
    struct AssertionFailure(&'static str);

    impl fmt::Display for AssertionFailure {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str(self.0)
        }
    }

    impl Error for AssertionFailure {}

    fn run_opeo<E, T>(
        operation: impl for<'slot> FnOnce(Out<'slot, 'slot, E>) -> OResult<'slot, T>,
    ) -> Result<T, E> {
        let mut errors = ErrSlot::<E>::new();
        match errors.try_call(operation) {
            Ok(value) => Ok(value),
            Err(caught) => Err(caught.take()),
        }
    }

    #[test]
    fn eof_after_trailing_lf_starts_an_empty_line() -> Result<(), Box<dyn Error>> {
        let index = run_opeo(|out| LineIndex::new("value\n", out))?;
        let location = run_opeo(|out| index.location("value\n".len(), out))?;

        if (location.line(), location.column()) != (2, 1) {
            return Err(Box::new(AssertionFailure(
                "EOF after a trailing newline must be at column one of line two",
            )));
        }
        Ok(())
    }

    #[test]
    fn eof_after_trailing_crlf_starts_an_empty_line() -> Result<(), Box<dyn Error>> {
        let index = run_opeo(|out| LineIndex::new("value\r\n", out))?;
        let location = run_opeo(|out| index.location("value\r\n".len(), out))?;

        if (location.line(), location.column()) != (2, 1) {
            return Err(Box::new(AssertionFailure(
                "EOF after trailing CRLF must be at column one of line two",
            )));
        }
        Ok(())
    }
}
