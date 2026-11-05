use typol_core::{
    BudgetError, Diagnostic, DiagnosticCode, ExecutionBudget, ExecutionLimits, Read, SliceSource,
    SourceId,
};
use typol_syntax::{InputConfig, LexEvent, StreamLimits, SyntaxLimits, TokenCursor, TokenKind};
use typol_test_support::TestSlot;

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

const SOURCE_ID: SourceId = SourceId::new(7);

fn new_execution() -> TestResult<ExecutionBudget> {
    TestSlot::<BudgetError>::default()
        .access(|out| ExecutionBudget::new(ExecutionLimits::new(67_108_864, 67_108_864), out))
        .map_err(|_| TestFailure)
}

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

#[test]
fn lookahead_does_not_consume_and_eof_remains_stable() -> TestResult {
    let execution = new_execution()?;

    let mut byte_input_1 = SliceSource::new(("name 17").as_bytes());
    let mut cursor = TestCursor::new(
        SOURCE_ID,
        &mut byte_input_1,
        &SyntaxLimits::default(),
        StreamLimits::default(),
        &execution,
    )
    .test_value()?;
    let current = cursor.peek().test_value()?.token().test_value()?.span();
    let next = cursor.peek_next().test_value()?.token().test_value()?;
    test_assert_eq!(next.kind(), TokenKind::Integer);
    test_assert_eq!(next.text().unwrap_or(""), "17");
    test_assert_eq!(
        cursor.peek().test_value()?.token().test_value()?.span(),
        current
    );
    cursor.bump().test_value()?;
    test_assert_eq!(
        cursor
            .peek()
            .test_value()?
            .token()
            .test_value()?
            .text()
            .unwrap_or(""),
        "17"
    );
    cursor.bump().test_value()?;
    let eof = cursor.peek().test_value()?.token().test_value()?.span();
    test_assert!(matches!(cursor.peek().test_value()?, LexEvent::Eof(_)));
    test_assert_eq!(
        cursor
            .peek_next()
            .test_value()?
            .token()
            .test_value()?
            .span(),
        eof
    );
    cursor.bump().test_value()?;
    test_assert!(matches!(cursor.peek().test_value()?, LexEvent::Halted(span) if *span == eof));

    Ok(())
}

#[test]
fn current_consumption_and_cached_error_consumption_are_independent_of_scanning() -> TestResult {
    let execution = new_execution()?;

    let mut byte_input_2 = SliceSource::new(("before ? after").as_bytes());
    let mut cursor = TestCursor::new(
        SOURCE_ID,
        &mut byte_input_2,
        &SyntaxLimits::default(),
        StreamLimits::default(),
        &execution,
    )
    .test_value()?;
    test_assert_eq!(
        cursor
            .peek()
            .test_value()?
            .token()
            .test_value()?
            .text()
            .unwrap_or(""),
        "before"
    );
    for _ in 0..2 {
        let error = cursor.peek_next().test_value()?.diagnostic().test_value()?;
        test_assert_eq!(error.code(), DiagnosticCode::LEXICAL_ERROR);
        test_assert_eq!(error.primary().test_value()?.span().start(), 7);
    }
    let LexEvent::Token(token) = cursor.bump().test_value()? else {
        return Err(TestFailure);
    };
    test_assert_eq!(token.text().unwrap_or(""), "before");
    for _ in 0..2 {
        test_assert_eq!(
            cursor
                .peek()
                .test_value()?
                .diagnostic()
                .test_value()?
                .code(),
            DiagnosticCode::LEXICAL_ERROR
        );
    }
    test_assert!(matches!(cursor.bump().test_value()?, LexEvent::Error(_)));
    test_assert_eq!(
        cursor
            .peek()
            .test_value()?
            .token()
            .test_value()?
            .text()
            .unwrap_or(""),
        "after"
    );

    Ok(())
}

#[test]
fn uncached_consumption_does_not_scan_the_next_event() -> TestResult {
    let execution = new_execution()?;

    let mut byte_input_3 = SliceSource::new(("before ?").as_bytes());
    let mut cursor = TestCursor::new(
        SOURCE_ID,
        &mut byte_input_3,
        &SyntaxLimits::default(),
        StreamLimits::default(),
        &execution,
    )
    .test_value()?;
    test_assert!(matches!(cursor.bump().test_value()?, LexEvent::Token(_)));
    test_assert!(matches!(cursor.peek().test_value()?, LexEvent::Error(_)));

    Ok(())
}

#[test]
fn fatal_event_is_stable_and_never_turns_into_eof() -> TestResult {
    let execution = new_execution()?;

    for budget in [0, 1, 2, 100] {
        let mut limits = SyntaxLimits::default();
        limits.set_max_diagnostics(budget);
        limits.set_max_string_bytes(1);
        let mut byte_input_4 = SliceSource::new(("\"abc\" after").as_bytes());
        let mut cursor = TestCursor::new(
            SOURCE_ID,
            &mut byte_input_4,
            &limits,
            StreamLimits::default(),
            &execution,
        )
        .test_value()?;
        for _ in 0..2 {
            test_assert_eq!(
                cursor
                    .peek()
                    .test_value()?
                    .diagnostic()
                    .test_value()?
                    .code(),
                DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
            );
            test_assert!(matches!(
                cursor.peek_next().test_value()?,
                LexEvent::Fatal(_)
            ));
        }
        test_assert!(matches!(cursor.bump().test_value()?, LexEvent::Fatal(_)));
        test_assert!(matches!(cursor.peek().test_value()?, LexEvent::Halted(_)));
        test_assert!(matches!(cursor.bump().test_value()?, LexEvent::Halted(_)));
    }

    Ok(())
}

#[test]
fn cursor_does_not_own_the_diagnostic_budget() -> TestResult {
    let execution = new_execution()?;

    let mut limits = SyntaxLimits::default();
    limits.set_max_diagnostics(0);
    let mut byte_input_5 = SliceSource::new(("? ?").as_bytes());
    let mut cursor = TestCursor::new(
        SOURCE_ID,
        &mut byte_input_5,
        &limits,
        StreamLimits::default(),
        &execution,
    )
    .test_value()?;
    test_assert!(matches!(cursor.bump().test_value()?, LexEvent::Error(_)));
    test_assert!(matches!(cursor.bump().test_value()?, LexEvent::Error(_)));
    test_assert!(matches!(cursor.bump().test_value()?, LexEvent::Eof(_)));

    Ok(())
}

#[test]
fn source_byte_limit_is_checked_when_bytes_are_consumed() -> TestResult {
    let execution = new_execution()?;

    let mut limits = SyntaxLimits::default();
    limits.set_max_source_bytes(3);
    let mut byte_input_6 = SliceSource::new(("name").as_bytes());
    let mut cursor = TestCursor::new(
        SOURCE_ID,
        &mut byte_input_6,
        &limits,
        StreamLimits::default(),
        &execution,
    )
    .test_value()?;
    let diagnostic = cursor.peek().test_value()?.diagnostic().test_value()?;
    test_assert_eq!(diagnostic.code(), DiagnosticCode::RESOURCE_LIMIT_EXCEEDED);
    test_assert_eq!(diagnostic.primary().test_value()?.span().start(), 3);

    Ok(())
}
