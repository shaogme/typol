use crate::error::SyntaxDiagnostic;
use core::{cell::Cell, error::Error, fmt, num::NonZeroUsize, str::from_utf8};
use opeo::{OResult, Out, ResultOutExt};
use typol_core::{
    BudgetError, ChargedVec, Diagnostic, DiagnosticCode, DiagnosticContext, ExecutionBudget,
    LocatedSpan, Read, SourceId, SourceLocation, SourceReadError, SourceReadErrorKind, SourceSpan,
    Span, SpanLocation, diagnostic_context,
};

/// Source and string ceilings for byte-stream lexing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InputLimits {
    max_source_bytes: usize,
    max_string_bytes: usize,
}

impl InputLimits {
    pub const fn new(max_source_bytes: usize, max_string_bytes: usize) -> Self {
        Self {
            max_source_bytes,
            max_string_bytes,
        }
    }

    pub const fn max_source_bytes(&self) -> usize {
        self.max_source_bytes
    }

    pub const fn max_string_bytes(&self) -> usize {
        self.max_string_bytes
    }
}

impl Default for InputLimits {
    fn default() -> Self {
        Self {
            max_source_bytes: 1_048_576,
            max_string_bytes: 262_144,
        }
    }
}

/// Input, diagnostic, and structure ceilings for Schema parsing and AST construction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SyntaxLimits {
    max_source_bytes: usize,
    max_diagnostics: usize,
    max_string_bytes: usize,
    max_schema_ast_nodes: usize,
    max_nesting_depth: usize,
}

impl SyntaxLimits {
    pub const fn input_limits(&self) -> InputLimits {
        InputLimits {
            max_source_bytes: self.max_source_bytes,
            max_string_bytes: self.max_string_bytes,
        }
    }

    pub const fn max_source_bytes(&self) -> usize {
        self.max_source_bytes
    }

    pub const fn max_diagnostics(&self) -> usize {
        self.max_diagnostics
    }

    pub const fn max_string_bytes(&self) -> usize {
        self.max_string_bytes
    }

    pub const fn max_schema_ast_nodes(&self) -> usize {
        self.max_schema_ast_nodes
    }

    pub const fn max_nesting_depth(&self) -> usize {
        self.max_nesting_depth
    }

    pub fn set_max_source_bytes(&mut self, value: usize) {
        self.max_source_bytes = value;
    }

    pub fn set_max_diagnostics(&mut self, value: usize) {
        self.max_diagnostics = value;
    }

    pub fn set_max_string_bytes(&mut self, value: usize) {
        self.max_string_bytes = value;
    }

    pub fn set_max_schema_ast_nodes(&mut self, value: usize) {
        self.max_schema_ast_nodes = value;
    }

    pub fn set_max_nesting_depth(&mut self, value: usize) {
        self.max_nesting_depth = value;
    }
}

impl Default for SyntaxLimits {
    fn default() -> Self {
        Self {
            max_source_bytes: 1_048_576,
            max_diagnostics: 100,
            max_string_bytes: 262_144,
            max_schema_ast_nodes: 100_000,
            max_nesting_depth: 128,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StreamLimits {
    chunk_bytes: NonZeroUsize,
    max_token_bytes: usize,
}

/// Reasons why stream input limits cannot be configured.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamLimitsError {
    /// The input window must contain at least one byte.
    ChunkBytesMustBePositive,
}

impl fmt::Display for StreamLimitsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ChunkBytesMustBePositive => {
                formatter.write_str("stream window size must be greater than zero")
            }
        }
    }
}

impl Error for StreamLimitsError {}

impl StreamLimits {
    /// Default stream window and per-token ceiling.
    pub const DEFAULT: Self = Self {
        chunk_bytes: match NonZeroUsize::new(8192) {
            Some(value) => value,
            None => NonZeroUsize::MIN,
        },
        max_token_bytes: 1_048_576,
    };

    /// Creates valid stream limits; a zero window returns a named option error immediately.
    pub const fn try_new(
        chunk_bytes: usize,
        max_token_bytes: usize,
    ) -> Result<Self, StreamLimitsError> {
        let Some(chunk_bytes) = NonZeroUsize::new(chunk_bytes) else {
            return Err(StreamLimitsError::ChunkBytesMustBePositive);
        };
        Ok(Self {
            chunk_bytes,
            max_token_bytes,
        })
    }

    pub const fn chunk_bytes(&self) -> usize {
        self.chunk_bytes.get()
    }

    pub const fn max_token_bytes(&self) -> usize {
        self.max_token_bytes
    }
}

impl Default for StreamLimits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// The source, resource limits, and execution budget for lexical input.
pub struct InputConfig<'input, 'config> {
    /// Stable ID of the input source.
    pub source_id: SourceId,
    /// Input object that provides the byte stream.
    pub source: &'input mut dyn Read<Diagnostic>,
    /// Resource ceilings for input bytes and strings.
    pub resources: &'config InputLimits,
    /// Input-window and token-size limits.
    pub stream: StreamLimits,
    /// Execution budget shared by this parse.
    pub budget: &'config ExecutionBudget,
}

pub(crate) struct Input<'input> {
    source: &'input mut dyn Read<Diagnostic>,
    window: ChargedVec<u8>,
    index: usize,
    length: usize,
    fetched: usize,
    limit: usize,
    pending: [u8; 4],
    pending_len: usize,
    eof: bool,
    offset: usize,
    line: NonZeroUsize,
    column: NonZeroUsize,
    source_id: SourceId,
    budget: ExecutionBudget,
    invalid_utf8: bool,
    fatal: Cell<bool>,
}

impl<'input> Input<'input> {
    pub(crate) fn new<'slot, 'config>(
        config: InputConfig<'input, 'config>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Self> {
        let InputConfig {
            source_id,
            source,
            resources,
            stream,
            budget,
        } = config;
        let initial_span = SourceSpan::new(source_id, Span::empty());
        let window = SyntaxDiagnostic::new_vec(budget, initial_span, out.reborrow())?;
        let budget = budget.try_clone_with(out.reborrow(), |error| {
            SyntaxDiagnostic::budget(error, initial_span)
        })?;
        let mut input = Self {
            source,
            window,
            index: 0,
            length: 0,
            fetched: 0,
            limit: resources.max_source_bytes(),
            pending: [0; 4],
            pending_len: 0,
            eof: false,
            offset: 0,
            line: NonZeroUsize::MIN,
            column: NonZeroUsize::MIN,
            source_id,
            budget,
            invalid_utf8: false,
            fatal: Cell::new(false),
        };
        let window_bytes = stream.chunk_bytes().min(input.limit).max(1);
        SyntaxDiagnostic::try_reserve_exact(
            &mut input.window,
            window_bytes,
            initial_span,
            out.reborrow(),
        )?;
        let zeros = [0; 256];
        while input.window.len() < window_bytes {
            let count = zeros.len().min(window_bytes - input.window.len());
            let Some(chunk) = zeros.get(..count) else {
                return Err(out
                    .reborrow()
                    .fail(input.internal_failure(diagnostic_context!(
                        "typol-syntax",
                        "stream-input",
                        "zero-window-chunk-range"
                    ))));
            };
            SyntaxDiagnostic::budgeted(initial_span, out.reborrow(), |out| {
                input.window.extend_from_slice(chunk, out)
            })?;
        }
        Ok(input)
    }

    fn next_byte<'slot>(
        &mut self,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Option<u8>> {
        if self.index == self.length {
            if self.eof {
                return Ok(None);
            }
            self.budget
                .consume_with(1, out.reborrow(), |error| self.budget_error(error))?;
            let remaining = self
                .limit
                .checked_sub(self.fetched)
                .ok_or_else(|| {
                    self.internal_failure(diagnostic_context!(
                        "typol-syntax",
                        "stream-input",
                        "fetched-byte-limit-underflow"
                    ))
                })
                .store_err_into(out.reborrow())?;
            let size = self.window.len().min(remaining).max(1);
            let Some(buffer) = self.window.get_mut(..size) else {
                return Err(out
                    .reborrow()
                    .fail(self.internal_failure(diagnostic_context!(
                        "typol-syntax",
                        "stream-input",
                        "read-buffer-range-unavailable"
                    ))));
            };
            let read = self
                .source
                .read(buffer, self.source_id, self.fetched, out.reborrow());
            if read.is_err() {
                self.fatal.set(true);
            }
            let count = read?;
            if count > size {
                return Err(out.reborrow().fail(
                    self.source_error(
                        SourceReadError::new(SourceReadErrorKind::InternalInvariantFailure)
                            .with_context(diagnostic_context!(
                                "typol-syntax",
                                "stream-input",
                                "source-read-count-exceeds-buffer"
                            ))
                            .with_input(self.source_id, self.fetched),
                    ),
                ));
            }
            self.budget
                .consume_with(count, out.reborrow(), |error| self.budget_error(error))?;
            if count == 0 {
                self.eof = true;
                return Ok(None);
            }
            if remaining == 0 {
                return Err(out
                    .reborrow()
                    .fail(self.failure(DiagnosticCode::RESOURCE_LIMIT_EXCEEDED)));
            }
            self.fetched = self
                .fetched
                .checked_add(count)
                .ok_or_else(|| self.failure(DiagnosticCode::RESOURCE_LIMIT_EXCEEDED))
                .store_err_into(out.reborrow())?;
            self.index = 0;
            self.length = count;
        }
        let byte = self
            .window
            .get(self.index)
            .copied()
            .ok_or_else(|| {
                self.internal_failure(diagnostic_context!(
                    "typol-syntax",
                    "stream-input",
                    "window-byte-index-out-of-range"
                ))
            })
            .store_err_into(out.reborrow())?;
        self.index = self
            .index
            .checked_add(1)
            .ok_or_else(|| self.failure(DiagnosticCode::RESOURCE_LIMIT_EXCEEDED))
            .store_err_into(out.reborrow())?;
        Ok(Some(byte))
    }

    pub(crate) fn byte<'slot>(
        &mut self,
        ahead: usize,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Option<u8>> {
        if ahead >= self.pending.len() {
            return Err(out
                .reborrow()
                .fail(self.internal_failure(diagnostic_context!(
                    "typol-syntax",
                    "stream-input",
                    "lookahead-exceeds-pending-buffer"
                ))));
        }
        while self.pending_len <= ahead {
            let Some(byte) = self.next_byte(out.reborrow())? else {
                return Ok(None);
            };
            let Some(slot) = self.pending.get_mut(self.pending_len) else {
                return Err(out
                    .reborrow()
                    .fail(self.internal_failure(diagnostic_context!(
                        "typol-syntax",
                        "stream-input",
                        "pending-byte-slot-unavailable"
                    ))));
            };
            *slot = byte;
            self.pending_len = self
                .pending_len
                .checked_add(1)
                .ok_or_else(|| {
                    self.internal_failure(diagnostic_context!(
                        "typol-syntax",
                        "stream-input",
                        "pending-byte-count-overflow"
                    ))
                })
                .store_err_into(out.reborrow())?;
        }
        Ok(self.pending.get(ahead).copied())
    }

    pub(crate) fn character<'slot>(
        &mut self,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Option<char>> {
        let Some(first) = self.byte(0, out.reborrow())? else {
            return Ok(None);
        };
        let width = match first {
            0..=0x7f => 1,
            0xc2..=0xdf => 2,
            0xe0..=0xef => 3,
            0xf0..=0xf4 => 4,
            _ => {
                let error = self.utf8_error(out.reborrow())?;
                return Err(out.reborrow().fail(error));
            }
        };
        for index in 1..width {
            if self.byte(index, out.reborrow())?.is_none() {
                let error = self.utf8_error(out.reborrow())?;
                return Err(out.reborrow().fail(error));
            }
        }
        let Some(bytes) = self.pending.get(..width) else {
            return Err(out
                .reborrow()
                .fail(self.internal_failure(diagnostic_context!(
                    "typol-syntax",
                    "stream-input",
                    "utf8-width-exceeds-pending-buffer"
                ))));
        };
        match from_utf8(bytes) {
            Ok(text) => Ok(text.chars().next()),
            Err(_) => {
                let error = self.utf8_error(out.reborrow())?;
                Err(out.reborrow().fail(error))
            }
        }
    }

    fn utf8_error<'slot>(
        &mut self,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Diagnostic> {
        self.invalid_utf8 = true;
        self.fatal.set(true);
        let mut error = self.failure(DiagnosticCode::LEXICAL_ERROR);
        let Some(end) = self.offset.checked_add(self.pending_len) else {
            return Err(out
                .reborrow()
                .fail(self.internal_failure(diagnostic_context!(
                    "typol-syntax",
                    "stream-input",
                    "invalid-utf8-span-end-overflow"
                ))));
        };
        let span = Span::new_with(self.offset, end, out.reborrow(), |_| {
            self.internal_failure(diagnostic_context!(
                "typol-syntax",
                "stream-input",
                "invalid-utf8-span-reversed"
            ))
        })?;
        error.set_primary(Some(SourceSpan::new(self.source_id, span)));
        Ok(error)
    }

    pub(crate) fn advance<'slot>(
        &mut self,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, char> {
        let Some(character) = self.character(out.reborrow())? else {
            return Err(out
                .reborrow()
                .fail(self.internal_failure(diagnostic_context!(
                    "typol-syntax",
                    "stream-input",
                    "advance-at-end-of-input"
                ))));
        };
        let width = character.len_utf8();
        self.budget
            .consume_with(width, out.reborrow(), |error| self.budget_error(error))?;
        let paired_cr = character == '\r' && self.byte(1, out.reborrow())? == Some(b'\n');
        let offset = self
            .offset
            .checked_add(width)
            .ok_or_else(|| self.failure(DiagnosticCode::RESOURCE_LIMIT_EXCEEDED))
            .store_err_into(out.reborrow())?;
        let (line, column) = if character == '\n' {
            let line = self
                .line
                .get()
                .checked_add(1)
                .and_then(NonZeroUsize::new)
                .ok_or_else(|| self.failure(DiagnosticCode::RESOURCE_LIMIT_EXCEEDED))
                .store_err_into(out.reborrow())?;
            (line, NonZeroUsize::MIN)
        } else if !paired_cr {
            let column = self
                .column
                .get()
                .checked_add(1)
                .and_then(NonZeroUsize::new)
                .ok_or_else(|| self.failure(DiagnosticCode::RESOURCE_LIMIT_EXCEEDED))
                .store_err_into(out.reborrow())?;
            (self.line, column)
        } else {
            (self.line, self.column)
        };
        if self.pending.get(width..self.pending_len).is_none() {
            return Err(out
                .reborrow()
                .fail(self.internal_failure(diagnostic_context!(
                    "typol-syntax",
                    "stream-input",
                    "advance-pending-range-invalid"
                ))));
        }
        let pending_len = self
            .pending_len
            .checked_sub(width)
            .ok_or_else(|| {
                self.internal_failure(diagnostic_context!(
                    "typol-syntax",
                    "stream-input",
                    "advance-pending-count-underflow"
                ))
            })
            .store_err_into(out.reborrow())?;
        let Some(source) = self.pending.get(width..self.pending_len) else {
            return Err(out
                .reborrow()
                .fail(self.internal_failure(diagnostic_context!(
                    "typol-syntax",
                    "stream-input",
                    "advance-pending-source-range-invalid"
                ))));
        };
        let mut remaining = [0; 4];
        let Some(destination) = remaining.get_mut(..pending_len) else {
            return Err(out
                .reborrow()
                .fail(self.internal_failure(diagnostic_context!(
                    "typol-syntax",
                    "stream-input",
                    "advance-pending-target-range-invalid"
                ))));
        };
        destination.copy_from_slice(source);
        self.pending.copy_from_slice(&remaining);
        self.offset = offset;
        self.line = line;
        self.column = column;
        self.pending_len = pending_len;
        Ok(character)
    }

    pub(crate) fn mark(&self) -> LocatedSpan {
        let location = SourceLocation::from_nonzero(self.line, self.column);
        LocatedSpan::new(
            SourceSpan::new(self.source_id, Span::point(self.offset)),
            (!self.invalid_utf8).then_some(SpanLocation {
                start: location,
                end: location,
            }),
        )
    }

    pub(crate) fn span_from<'slot>(
        &self,
        start: LocatedSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, LocatedSpan> {
        let span = start
            .through(self.mark())
            .ok_or_else(|| {
                self.internal_failure(diagnostic_context!(
                    "typol-syntax",
                    "stream-input",
                    "token-span-cannot-be-combined"
                ))
            })
            .store_err_into(out.reborrow())?;
        Ok(span)
    }

    pub(crate) fn failure(&self, code: DiagnosticCode) -> Diagnostic {
        if matches!(
            code,
            DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
                | DiagnosticCode::ALLOCATION_FAILED
                | DiagnosticCode::SOURCE_INPUT_ERROR
                | DiagnosticCode::INTERNAL_ERROR
        ) {
            self.fatal.set(true);
        }
        let mut error = Diagnostic::emergency(code);
        error.set_located_primary(Some(self.mark()));
        error
    }

    pub(crate) fn internal_failure(&self, context: DiagnosticContext) -> Diagnostic {
        self.fatal.set(true);
        let mut diagnostic = SyntaxDiagnostic::internal(context, self.mark().source_span());
        diagnostic.set_located_primary(Some(self.mark()));
        diagnostic
    }

    pub(crate) fn source_error(&self, error: SourceReadError) -> Diagnostic {
        self.fatal.set(true);
        let mut diagnostic = self.failure(error.diagnostic_code());
        let mut context = error.context().unwrap_or_else(|| {
            diagnostic_context!("typol-syntax", "stream-input", "source-adapter-read")
        });
        if context.object_id().is_none() {
            if let Ok(offset) = u64::try_from(error.offset().unwrap_or(self.fetched)) {
                context = context.with_object_id(offset);
            }
        }
        diagnostic.set_context(Some(context));
        diagnostic
    }

    pub(crate) fn budget_error(&self, error: BudgetError) -> Diagnostic {
        self.fatal.set(true);
        let mut diagnostic = self.failure(error.diagnostic_code());
        diagnostic.set_context(error.context());
        diagnostic
    }

    pub(crate) fn invalid_utf8(&self) -> bool {
        self.invalid_utf8
    }

    pub(crate) fn fatal(&self) -> bool {
        self.fatal.get() || self.invalid_utf8()
    }
}

#[cfg(test)]
mod tests {
    use super::{Input, InputConfig, InputLimits, StreamLimits};
    use opeo::{ErrSlot, OResult, Out};
    use typol_core::{
        BudgetError, Diagnostic, DiagnosticCode, ExecutionBudget, ExecutionLimits, Read,
        SliceSource, SourceId,
    };

    #[derive(Debug)]
    struct TestFailure;

    type TestResult = Result<(), TestFailure>;

    fn call<T>(
        slot: &mut ErrSlot<Diagnostic>,
        operation: impl for<'slot> FnOnce(Out<'slot, 'slot, Diagnostic>) -> OResult<'slot, T>,
    ) -> Result<T, Diagnostic> {
        match slot.try_call(operation) {
            Ok(value) => Ok(value),
            Err(caught) => Err(caught.take()),
        }
    }

    #[test]
    fn byte_lookahead_stops_at_the_four_byte_utf8_boundary() -> TestResult {
        let mut budget_errors = ErrSlot::<BudgetError>::new();
        let budget = match budget_errors
            .try_call(|out| ExecutionBudget::new(ExecutionLimits::new(67_108_864, 67_108_864), out))
        {
            Ok(budget) => budget,
            Err(_) => return Err(TestFailure),
        };
        let mut source = SliceSource::new("🦀".as_bytes());
        let source: &mut dyn Read<Diagnostic> = &mut source;
        let mut errors = ErrSlot::<Diagnostic>::new();
        let mut input = call(&mut errors, |out| {
            Input::new(
                InputConfig {
                    source_id: SourceId::new(7),
                    source,
                    resources: &InputLimits::default(),
                    stream: StreamLimits::default(),
                    budget: &budget,
                },
                out,
            )
        })
        .map_err(|_| TestFailure)?;

        if call(&mut errors, |out| input.byte(3, out)).map_err(|_| TestFailure)? != Some(0x80)
            || input.pending_len != 4
        {
            return Err(TestFailure);
        }
        let error = call(&mut errors, |out| input.byte(4, out))
            .err()
            .ok_or(TestFailure)?;
        if error.code() != DiagnosticCode::INTERNAL_ERROR || input.pending_len != 4 {
            return Err(TestFailure);
        }
        let context = error.context().ok_or(TestFailure)?;
        if context.subsystem() != "typol-syntax"
            || context.stage() != "stream-input"
            || context.failure_point() != "lookahead-exceeds-pending-buffer"
        {
            return Err(TestFailure);
        }
        if error
            .located_primary()
            .and_then(|primary| primary.location())
            .is_none()
        {
            return Err(TestFailure);
        }
        if call(&mut errors, |out| input.advance(out)).map_err(|_| TestFailure)? != '🦀' {
            return Err(TestFailure);
        }
        if input.mark().span().end() != 4 || input.pending_len != 0 {
            return Err(TestFailure);
        }

        Ok(())
    }
}
