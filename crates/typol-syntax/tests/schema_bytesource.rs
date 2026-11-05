use typol_core::{
    BudgetError, Diagnostic, DiagnosticCode, ExecutionBudget, ExecutionLimits, Read, SliceSource,
    SourceId, SourceReadError, SourceReadErrorKind, SourceSpan,
};
use typol_syntax::{
    AstAccessError, SchemaAst, SchemaParseOptions, SchemaParser, StreamLimits, SyntaxErrors,
    SyntaxLimits,
    opeo::{OResult, Out},
};
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

trait TestError<T> {
    fn test_error(self) -> TestResult<T>;
}

impl<T, E> TestError<E> for Result<T, E> {
    fn test_error(self) -> TestResult<E> {
        self.err().ok_or(TestFailure)
    }
}

const SOURCE_ID: SourceId = SourceId::new(7);

struct ShortReader<'source> {
    bytes: &'source [u8],
    offset: usize,
    chunk_bytes: usize,
}

struct FailingReader<'source> {
    bytes: &'source [u8],
    offset: usize,
    fail_at: usize,
}

#[derive(Debug, Eq, PartialEq)]
struct AstSummary {
    span: SourceSpan,
    item_spans: Vec<SourceSpan>,
    node_count: usize,
    structure_height: usize,
}

#[derive(Debug, Eq, PartialEq)]
struct DiagnosticSummary {
    code: DiagnosticCode,
    span: SourceSpan,
}

impl Read<Diagnostic> for ShortReader<'_> {
    fn read<'slot, 'borrow>(
        &mut self,
        output: &mut [u8],
        source_id: SourceId,
        offset: usize,
        out: Out<'slot, 'borrow, Diagnostic>,
    ) -> OResult<'slot, usize> {
        let remaining = self.bytes.len().saturating_sub(self.offset);
        let count = remaining.min(output.len()).min(self.chunk_bytes);
        let Some(end) = self.offset.checked_add(count) else {
            return Err(out.fail(Diagnostic::from(
                SourceReadError::new(SourceReadErrorKind::InternalInvariantFailure)
                    .with_input(source_id, offset),
            )));
        };
        let Some(source) = self.bytes.get(self.offset..end) else {
            return Err(out.fail(Diagnostic::from(
                SourceReadError::new(SourceReadErrorKind::InternalInvariantFailure)
                    .with_input(source_id, offset),
            )));
        };
        let Some(destination) = output.get_mut(..count) else {
            return Err(out.fail(Diagnostic::from(
                SourceReadError::new(SourceReadErrorKind::InternalInvariantFailure)
                    .with_input(source_id, offset),
            )));
        };
        destination.copy_from_slice(source);
        self.offset = end;
        Ok(count)
    }
}

impl Read<Diagnostic> for FailingReader<'_> {
    fn read<'slot, 'borrow>(
        &mut self,
        output: &mut [u8],
        source_id: SourceId,
        offset: usize,
        out: Out<'slot, 'borrow, Diagnostic>,
    ) -> OResult<'slot, usize> {
        if self.offset >= self.fail_at {
            let error = SourceReadError::new(SourceReadErrorKind::HostFailure)
                .with_input(source_id, offset);
            return Err(out.fail(Diagnostic::from(error)));
        }
        let count = self
            .bytes
            .len()
            .saturating_sub(self.offset)
            .min(output.len())
            .min(self.fail_at.saturating_sub(self.offset));
        let end = self.offset + count;
        let Some(source) = self.bytes.get(self.offset..end) else {
            let error = SourceReadError::new(SourceReadErrorKind::InternalInvariantFailure)
                .with_input(source_id, offset);
            return Err(out.fail(Diagnostic::from(error)));
        };
        let Some(destination) = output.get_mut(..count) else {
            let error = SourceReadError::new(SourceReadErrorKind::InternalInvariantFailure)
                .with_input(source_id, offset);
            return Err(out.fail(Diagnostic::from(error)));
        };
        destination.copy_from_slice(source);
        self.offset = end;
        Ok(count)
    }
}

fn execution_budget(steps: usize) -> TestResult<ExecutionBudget> {
    TestSlot::<BudgetError>::default()
        .access(|out| ExecutionBudget::new(ExecutionLimits::new(steps, 67_108_864), out))
        .map_err(|_| TestFailure)
}

fn parse_bytes(
    source: &mut dyn Read<Diagnostic>,
    limits: &SyntaxLimits,
    steps: usize,
) -> TestResult<Result<SchemaAst, SyntaxErrors>> {
    let execution = execution_budget(steps)?;
    let options = SchemaParseOptions::new(*limits, StreamLimits::try_new(8, 1024).test_value()?);
    let mut errors = TestSlot::<SyntaxErrors>::default();
    Ok(errors.access(|out| SchemaParser::parse(SOURCE_ID, source, &options, &execution, out)))
}

fn parse_text(
    source: &str,
    limits: &SyntaxLimits,
    steps: usize,
) -> TestResult<Result<SchemaAst, SyntaxErrors>> {
    let execution = execution_budget(steps)?;
    let options = SchemaParseOptions::new(*limits, StreamLimits::try_new(8, 1024).test_value()?);
    let mut errors = TestSlot::<SyntaxErrors>::default();
    Ok(errors.access(|out| SchemaParser::parse_str(SOURCE_ID, source, &options, &execution, out)))
}

fn summarize(ast: &SchemaAst) -> TestResult<AstSummary> {
    let mut errors = TestSlot::<AstAccessError>::default();
    let mut item_spans = Vec::new();
    for index in 0..ast.items().len() {
        let item = errors
            .access(|out| ast.items().get(index, out))
            .map_err(|_| TestFailure)?
            .test_value()?;
        item_spans.push(item.span());
    }
    let structure_height = errors
        .access(|out| ast.structure_height(out))
        .map_err(|_| TestFailure)?;
    Ok(AstSummary {
        span: ast.span(),
        item_spans,
        node_count: ast.node_count(),
        structure_height,
    })
}

fn summarize_errors(errors: &SyntaxErrors) -> TestResult<Vec<DiagnosticSummary>> {
    errors
        .diagnostics()
        .map(|diagnostic| {
            let span = diagnostic.primary().test_value()?;
            Ok(DiagnosticSummary {
                code: diagnostic.code(),
                span,
            })
        })
        .collect()
}

#[test]
fn string_slice_and_short_sources_build_the_same_schema_ast() -> TestResult {
    let source = "extern mod shared;\nmod net { struct Host { name: string, } }\ntype Port = u16;\nentry struct Root { host: net::Host, port: Port }\n";
    let limits = SyntaxLimits::default();
    let string_ast = parse_text(source, &limits, 67_108_864)?.test_value()?;
    let mut slice = SliceSource::new(source.as_bytes());
    let slice_ast = parse_bytes(&mut slice, &limits, 67_108_864)?.test_value()?;
    let mut reader = ShortReader {
        bytes: source.as_bytes(),
        offset: 0,
        chunk_bytes: 3,
    };
    let reader_ast = parse_bytes(&mut reader, &limits, 67_108_864)?.test_value()?;

    let expected = summarize(&string_ast)?;
    test_assert_eq!(summarize(&slice_ast)?, expected);
    test_assert_eq!(summarize(&reader_ast)?, expected);
    test_assert_eq!(expected.span.span().end(), source.len());
    test_assert_eq!(reader.offset, source.len());
    Ok(())
}

#[test]
fn string_slice_and_short_sources_report_the_same_recovered_errors() -> TestResult {
    let source = "struct Broken { first: } struct Later { second: }";
    let limits = SyntaxLimits::default();
    let string_errors = parse_text(source, &limits, 67_108_864)?.test_error()?;
    let mut slice = SliceSource::new(source.as_bytes());
    let slice_errors = parse_bytes(&mut slice, &limits, 67_108_864)?.test_error()?;
    let mut reader = ShortReader {
        bytes: source.as_bytes(),
        offset: 0,
        chunk_bytes: 2,
    };
    let reader_errors = parse_bytes(&mut reader, &limits, 67_108_864)?.test_error()?;

    let expected = summarize_errors(&string_errors)?;
    test_assert_eq!(summarize_errors(&slice_errors)?, expected);
    test_assert_eq!(summarize_errors(&reader_errors)?, expected);
    test_assert_eq!(expected.len(), 2);
    Ok(())
}

#[test]
fn eof_spans_cover_empty_whitespace_newlines_utf8_and_exact_source_limits() -> TestResult {
    for source in [
        "",
        " \t",
        "// end\n",
        "// end\r\n",
        "// €😀\r\nentry struct Root { label: string = \"€\" }\n",
    ] {
        let mut limits = SyntaxLimits::default();
        limits.set_max_source_bytes(source.len());
        let mut reader = ShortReader {
            bytes: source.as_bytes(),
            offset: 0,
            chunk_bytes: 1,
        };
        let ast = parse_bytes(&mut reader, &limits, 67_108_864)?.test_value()?;
        test_assert_eq!(ast.span().span().start(), 0);
        test_assert_eq!(ast.span().span().end(), source.len());
    }
    Ok(())
}

#[test]
fn source_limit_read_failure_and_invalid_utf8_never_return_an_ast() -> TestResult {
    let source = "entry struct Root {}";
    let mut limited = SyntaxLimits::default();
    limited.set_max_source_bytes(source.len());
    let mut exact_reader = ShortReader {
        bytes: source.as_bytes(),
        offset: 0,
        chunk_bytes: 5,
    };
    let ast = parse_bytes(&mut exact_reader, &limited, 67_108_864)?.test_value()?;
    test_assert_eq!(ast.span().span().end(), source.len());

    limited.set_max_source_bytes(source.len() - 1);
    let mut oversized = ShortReader {
        bytes: source.as_bytes(),
        offset: 0,
        chunk_bytes: 4,
    };
    let error = parse_bytes(&mut oversized, &limited, 67_108_864)?.test_error()?;
    let diagnostics = error.diagnostics();
    let first = diagnostics.first().test_value()?;
    test_assert_eq!(first.code(), DiagnosticCode::RESOURCE_LIMIT_EXCEEDED);
    test_assert_eq!(
        first.primary().test_value()?.span().start(),
        source.len() - 1
    );

    limited.set_max_source_bytes(1024);
    let mut failing = FailingReader {
        bytes: source.as_bytes(),
        offset: 0,
        fail_at: 7,
    };
    let error = parse_bytes(&mut failing, &limited, 67_108_864)?.test_error()?;
    let diagnostics = error.diagnostics();
    let first = diagnostics.first().test_value()?;
    test_assert_eq!(first.code(), DiagnosticCode::SOURCE_INPUT_ERROR);
    test_assert_eq!(first.primary().test_value()?.span().start(), 7);

    let invalid = b"entry struct Root {}\xff";
    let mut invalid_reader = ShortReader {
        bytes: invalid,
        offset: 0,
        chunk_bytes: 2,
    };
    let error = parse_bytes(&mut invalid_reader, &limited, 67_108_864)?.test_error()?;
    let diagnostics = error.diagnostics();
    let first = diagnostics.first().test_value()?;
    test_assert_eq!(first.code(), DiagnosticCode::LEXICAL_ERROR);
    test_assert_eq!(
        first.primary().test_value()?.span().start(),
        invalid.len() - 1
    );
    Ok(())
}

#[test]
fn low_execution_budget_terminates_recovery_for_known_and_unknown_length_inputs() -> TestResult {
    let source = format!("{}entry struct Root {{}}", "? ".repeat(32));
    let limits = SyntaxLimits::default();
    let string_errors = parse_text(&source, &limits, 128)?.test_error()?;
    let mut slice = SliceSource::new(source.as_bytes());
    let slice_errors = parse_bytes(&mut slice, &limits, 128)?.test_error()?;

    let expected = summarize_errors(&string_errors)?;
    test_assert_eq!(summarize_errors(&slice_errors)?, expected);
    test_assert!(
        expected
            .iter()
            .any(|error| { error.code == DiagnosticCode::RESOURCE_LIMIT_EXCEEDED })
    );
    Ok(())
}
