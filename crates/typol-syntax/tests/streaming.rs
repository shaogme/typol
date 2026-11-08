use opeo::{OResult, Out, ResultOutExt};
use std::cell::Cell;
use typol_core::{
    AllocationError, BudgetError, Diagnostic, DiagnosticCode, DiagnosticContext, ExecutionBudget,
    ExecutionLimits, LineIndex, LocatedSpan, LocationError, Read, SliceSource, SourceId,
    SourceReadError, SourceReadErrorKind, diagnostic_context,
};
use typol_syntax::{
    InputConfig, LexEvent, StreamLimits, StreamLimitsError, SyntaxLimits, TokenCursor, TokenKind,
};
use typol_test_support::TestSlot;

struct TestCursor<'source> {
    cursor: TokenCursor<'source>,
    errors: TestSlot<Diagnostic>,
}

impl<'source> TestCursor<'source> {
    fn new(
        source_id: SourceId,
        source: &'source mut dyn Read<Diagnostic>,
        limits: &SyntaxLimits,
        stream: StreamLimits,
        execution: &ExecutionBudget,
    ) -> Result<Self, Diagnostic> {
        let input_limits = limits.input_limits();
        let mut errors = TestSlot::default();
        let cursor = errors.access(|out| {
            TokenCursor::new(
                InputConfig {
                    source_id,
                    source,
                    resources: &input_limits,
                    stream,
                    budget: execution,
                },
                out,
            )
        })?;
        Ok(Self { cursor, errors })
    }

    fn peek(&mut self) -> Result<&LexEvent, Diagnostic> {
        self.errors.access(|out| self.cursor.peek(out))
    }

    fn peek_next(&mut self) -> Result<&LexEvent, Diagnostic> {
        self.errors.access(|out| self.cursor.peek_next(out))
    }

    fn bump(&mut self) -> Result<LexEvent, Diagnostic> {
        self.errors.access(|out| self.cursor.bump(out))
    }
}

struct ShortSource<'a> {
    bytes: &'a [u8],
    offset: usize,
    cut: usize,
    calls: &'a Cell<usize>,
    invalid_count: bool,
}

struct FailingSource {
    error: SourceReadError,
}

impl Read<Diagnostic> for FailingSource {
    fn read<'slot, 'borrow>(
        &mut self,
        _output: &mut [u8],
        source_id: SourceId,
        offset: usize,
        out: Out<'slot, 'borrow, Diagnostic>,
    ) -> OResult<'slot, usize> {
        let error = self.error.with_input(source_id, offset);
        let error_offset = error.offset().unwrap_or(offset);
        let mut diagnostic = Diagnostic::from(error);
        let mut context = diagnostic.context().unwrap_or_else(|| {
            diagnostic_context!("typol-syntax", "stream-input", "source-adapter-read")
        });
        if context.object_id().is_none() {
            context = context.with_object_id(error_offset as u64);
        }
        diagnostic.set_context(Some(context));
        Err(out.fail(diagnostic))
    }
}

impl Read<Diagnostic> for ShortSource<'_> {
    fn read<'slot>(
        &mut self,
        output: &mut [u8],
        source_id: SourceId,
        offset: usize,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, usize> {
        self.calls.set(self.calls.get() + 1);
        if self.invalid_count {
            return Ok(output.len() + 1);
        }
        let remaining = if self.offset < self.cut {
            self.cut - self.offset
        } else {
            self.bytes.len() - self.offset
        };
        let count = remaining.min(output.len());
        let error = || {
            Diagnostic::from(
                SourceReadError::new(SourceReadErrorKind::InternalInvariantFailure)
                    .with_input(source_id, offset),
            )
        };
        let end = self
            .offset
            .checked_add(count)
            .ok_or_else(error)
            .store_err_into(out.reborrow())?;
        let output = output
            .get_mut(..count)
            .ok_or_else(error)
            .store_err_into(out.reborrow())?;
        let input = self
            .bytes
            .get(self.offset..end)
            .ok_or_else(error)
            .store_err_into(out.reborrow())?;
        output.copy_from_slice(input);
        self.offset = end;
        Ok(count)
    }
}

#[derive(Debug, PartialEq)]
struct TokenSnapshot {
    kind: TokenKind,
    span: LocatedSpan,
    text: Option<String>,
}

#[derive(Debug)]
struct TestFailure;

macro_rules! test_assert {
    ($condition:expr $(, $($message:tt)*)?) => {{
        if !$condition {
            return Err(TestFailure);
        }
    }};
}

macro_rules! test_assert_eq {
    ($left:expr, $right:expr $(, $($message:tt)*)?) => {{
        let left = &$left;
        let right = &$right;
        if left != right {
            return Err(TestFailure);
        }
    }};
}

type TestResult<T = ()> = Result<T, TestFailure>;

trait TestValue<T> {
    fn test_value(self) -> TestResult<T>;
}

impl<T, E> TestValue<T> for Result<T, E> {
    fn test_value(self) -> TestResult<T> {
        self.map_err(|_| TestFailure)
    }
}

impl<T> TestValue<T> for Option<T> {
    fn test_value(self) -> TestResult<T> {
        self.ok_or(TestFailure)
    }
}

fn execution_budget(steps: usize, bytes: usize) -> TestResult<ExecutionBudget> {
    TestSlot::<BudgetError>::default()
        .access(|out| ExecutionBudget::new(ExecutionLimits::new(steps, bytes), out))
        .map_err(|_| TestFailure)
}

fn tokens(source: &mut dyn Read<Diagnostic>, chunk: usize) -> TestResult<Vec<TokenSnapshot>> {
    let budget = execution_budget(16_777_216, 67_108_864)?;
    let mut cursor = TestCursor::new(
        SourceId::new(1),
        source,
        &SyntaxLimits::default(),
        StreamLimits::try_new(chunk, 1024).test_value()?,
        &budget,
    )
    .test_value()?;
    let mut snapshots = Vec::new();
    loop {
        let event = cursor.bump().test_value()?;
        let token = event.token().test_value()?;
        snapshots.push(TokenSnapshot {
            kind: token.kind(),
            span: token.located_span(),
            text: token.text().or(token.decoded_string()).map(String::from),
        });
        if token.kind() == TokenKind::Eof {
            break;
        }
    }
    test_assert!(matches!(cursor.peek().test_value()?, LexEvent::Halted(_)));
    Ok(snapshots)
}

#[test]
fn zero_stream_window_is_rejected_during_option_construction() -> TestResult {
    test_assert_eq!(
        StreamLimits::try_new(0, 1024),
        Err(StreamLimitsError::ChunkBytesMustBePositive)
    );
    let tokenless = StreamLimits::try_new(1, 0).test_value()?;
    test_assert_eq!(tokenless.max_token_bytes(), 0);

    Ok(())
}

#[test]
fn all_symbols_and_utf8_scalars_survive_every_split_and_match_line_index() -> TestResult {
    let text = "/*é€😀\r\n*/ env Name 000123 0x2a 12.50 \"é€😀\\n\\t\\\\\\\"\" :: == != <= >= && || : = ! < > | / //end😀\r\n{}\n";
    let expected = tokens(&mut SliceSource::new(text.as_bytes()), 8192)?;
    let mut allocation_errors = TestSlot::<AllocationError>::default();
    let index = allocation_errors
        .access(|out| LineIndex::new(text, out))
        .test_value()?;
    let mut location_errors = TestSlot::<LocationError>::default();
    for token in &expected {
        let span = token.span.span();
        let location = token.span.location().test_value()?;
        test_assert_eq!(
            location.start,
            location_errors
                .access(|out| index.location(span.start(), out))
                .test_value()?
        );
        test_assert_eq!(
            location.end,
            location_errors
                .access(|out| index.location(span.end(), out))
                .test_value()?
        );
    }
    for chunk in [1, 2, 3, 7, 8192] {
        for cut in 1..text.len() {
            let calls = Cell::new(0);
            let mut source = ShortSource {
                bytes: text.as_bytes(),
                offset: 0,
                cut,
                calls: &calls,
                invalid_count: false,
            };
            test_assert_eq!(
                tokens(&mut source, chunk)?,
                expected,
                "chunk {chunk}, cut {cut}"
            );
        }
    }

    Ok(())
}

#[test]
fn block_string_delimiters_escapes_and_crlf_survive_stream_splits() -> TestResult {
    let text =
        "{value: \"\"\"\r\n    alpha\\nβ\r\n    [}, /* //, \\\"\"\"\r\n    tail\r\n    \"\"\",}";
    let expected = tokens(&mut SliceSource::new(text.as_bytes()), 8192)?;
    for chunk in [1, 2, 3, 7, 8192] {
        for cut in 1..text.len() {
            let calls = Cell::new(0);
            let mut source = ShortSource {
                bytes: text.as_bytes(),
                offset: 0,
                cut,
                calls: &calls,
                invalid_count: false,
            };
            test_assert_eq!(
                tokens(&mut source, chunk)?,
                expected,
                "chunk {chunk}, cut {cut}"
            );
        }
    }

    Ok(())
}

#[test]
fn cached_events_never_read_or_charge_again_and_eof_is_emitted_once() -> TestResult {
    let calls = Cell::new(0);
    let mut source = ShortSource {
        bytes: b"name 1",
        offset: 0,
        cut: 2,
        calls: &calls,
        invalid_count: false,
    };
    let budget = execution_budget(16_777_216, 65536)?;
    let mut cursor = TestCursor::new(
        SourceId::new(1),
        &mut source,
        &SyntaxLimits::default(),
        StreamLimits::try_new(1, 1024).test_value()?,
        &budget,
    )
    .test_value()?;
    test_assert!(matches!(cursor.peek().test_value()?, LexEvent::Token(_)));
    let before = calls.get();
    let steps = budget.steps();
    for _ in 0..10 {
        cursor.peek().test_value()?;
    }
    cursor.bump().test_value()?;
    test_assert_eq!(calls.get(), before);
    test_assert_eq!(budget.steps(), steps);
    test_assert!(matches!(cursor.bump().test_value()?, LexEvent::Token(_)));
    test_assert!(matches!(cursor.bump().test_value()?, LexEvent::Eof(_)));
    let before = calls.get();
    let steps = budget.steps();
    for _ in 0..10 {
        test_assert!(matches!(
            cursor.peek_next().test_value()?,
            LexEvent::Halted(_)
        ));
        test_assert!(matches!(cursor.bump().test_value()?, LexEvent::Halted(_)));
    }
    test_assert_eq!(calls.get(), before);
    test_assert_eq!(budget.steps(), steps);

    Ok(())
}

#[test]
fn invalid_reader_counts_and_utf8_emit_one_fatal_event_then_halt() -> TestResult {
    for bytes in [
        b"\xff".as_slice(),
        b"\xc0\xaf",
        b"\xe0\x80\x80",
        b"\xed\xa0\x80",
        b"\xf4\x90\x80\x80",
        b"\xf0\x9f\x98",
    ] {
        for invalid_count in [false, true] {
            let calls = Cell::new(0);
            let mut source = ShortSource {
                bytes,
                offset: 0,
                cut: 1,
                calls: &calls,
                invalid_count,
            };
            let budget = execution_budget(16_777_216, 65536)?;
            let mut cursor = TestCursor::new(
                SourceId::new(1),
                &mut source,
                &SyntaxLimits::default(),
                StreamLimits::try_new(1, 1024).test_value()?,
                &budget,
            )
            .test_value()?;
            let LexEvent::Fatal(error) = cursor.bump().test_value()? else {
                return Err(TestFailure);
            };
            test_assert_eq!(
                error.code(),
                if invalid_count {
                    DiagnosticCode::INTERNAL_ERROR
                } else {
                    DiagnosticCode::LEXICAL_ERROR
                }
            );
            if invalid_count {
                let context = error.context().test_value()?;
                test_assert_eq!(context.subsystem(), "typol-syntax");
                test_assert_eq!(context.stage(), "stream-input");
                test_assert_eq!(context.failure_point(), "source-read-count-exceeds-buffer");
            }
            if invalid_count {
                test_assert!(error.located_primary().test_value()?.location().is_some());
            } else {
                test_assert!(error.located_primary().test_value()?.location().is_none());
            }
            let before = calls.get();
            for _ in 0..3 {
                test_assert!(matches!(cursor.bump().test_value()?, LexEvent::Halted(_)));
            }
            test_assert_eq!(calls.get(), before);
        }
    }

    Ok(())
}

#[test]
fn source_adapter_failures_keep_their_category_and_context() -> TestResult {
    static ADAPTER_CONTEXT: DiagnosticContext =
        diagnostic_context!("test-source", "read", "fixture-read-failure");

    for (error, expected_context) in [
        (
            SourceReadError::new(SourceReadErrorKind::HostFailure),
            diagnostic_context!("typol-syntax", "stream-input", "source-adapter-read")
                .with_object_id(0),
        ),
        (
            SourceReadError::new(SourceReadErrorKind::InternalInvariantFailure)
                .with_offset(2)
                .with_context(ADAPTER_CONTEXT),
            ADAPTER_CONTEXT.with_object_id(2),
        ),
    ] {
        let mut source = FailingSource { error };
        let budget = execution_budget(16_777_216, 65_536)?;
        let mut cursor = TestCursor::new(
            SourceId::new(4),
            &mut source,
            &SyntaxLimits::default(),
            StreamLimits::default(),
            &budget,
        )
        .test_value()?;
        let LexEvent::Fatal(diagnostic) = cursor.bump().test_value()? else {
            return Err(TestFailure);
        };

        test_assert_eq!(diagnostic.code(), error.diagnostic_code());
        test_assert_eq!(diagnostic.context(), Some(expected_context));
    }

    Ok(())
}

#[test]
fn raw_token_and_decoded_string_limits_have_independent_exact_boundaries() -> TestResult {
    for text in ["name", "0000", "0x2a", "12.0", "\"\\n\"", "=="] {
        let budget = execution_budget(16_777_216, 65536)?;
        for limit in [text.len() - 1, text.len()] {
            let mut source = SliceSource::new(text.as_bytes());
            let mut cursor = TestCursor::new(
                SourceId::new(1),
                &mut source,
                &SyntaxLimits::default(),
                StreamLimits::try_new(1, limit).test_value()?,
                &budget,
            )
            .test_value()?;
            if limit == text.len() {
                test_assert!(matches!(cursor.bump().test_value()?, LexEvent::Token(_)));
            } else {
                test_assert_eq!(
                    cursor
                        .bump()
                        .test_value()?
                        .diagnostic()
                        .test_value()?
                        .code(),
                    DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
                );
            }
        }
    }

    Ok(())
}
