use typol_core::{
    AllocationError, BudgetError, Diagnostic, DiagnosticCode, ExecutionBudget, ExecutionLimits,
    LineIndex, LocationError, SliceSource, SourceId,
};
use typol_syntax::{
    InputConfig, InputLimits, Keyword, LexEvent, StreamLimits, Symbol, SyntaxLimits, TokenCursor,
    TokenKind,
    opeo::{OResult, Out},
};
use typol_test_support::TestSlot;

struct TestErrors {
    budget: TestSlot<BudgetError>,
    diagnostic: TestSlot<Diagnostic>,
}

impl TestErrors {
    fn new() -> Self {
        Self {
            budget: TestSlot::default(),
            diagnostic: TestSlot::default(),
        }
    }

    fn execution(&mut self, limits: ExecutionLimits) -> Result<ExecutionBudget, BudgetError> {
        self.budget.access(|out| ExecutionBudget::new(limits, out))
    }

    fn diagnostic<T>(
        &mut self,
        operation: impl for<'slot> FnOnce(Out<'slot, 'slot, Diagnostic>) -> OResult<'slot, T>,
    ) -> Result<T, Diagnostic> {
        self.diagnostic.access(operation)
    }
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

struct BlockStringErrorCase<'source> {
    source: &'source str,
    needle: &'source str,
    source_length: usize,
    line: usize,
    start_column: usize,
    end_column: usize,
}

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

#[test]
fn keywords_identifiers_and_longest_match_symbols_are_classified() -> TestResult {
    let mut errors = TestErrors::new();
    let execution = errors
        .execution(ExecutionLimits::new(67_108_864, 67_108_864))
        .test_value()?;

    let source = "extern mod type struct entry enum option map i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 string bool true false none env Name _field item2 :: : ( ) { } [ ] , ; . @ == = != ! <= < >= > && || | + - * /";
    let mut byte_input_1 = SliceSource::new((source).as_bytes());
    let mut cursor = errors
        .diagnostic(|out| {
            TokenCursor::new(
                InputConfig {
                    source_id: SOURCE_ID,
                    source: &mut byte_input_1,
                    resources: &InputLimits::default(),
                    stream: StreamLimits::default(),
                    budget: &execution,
                },
                out,
            )
        })
        .test_value()?;
    let expected = [
        TokenKind::Keyword(Keyword::Extern),
        TokenKind::Keyword(Keyword::Mod),
        TokenKind::Keyword(Keyword::Type),
        TokenKind::Keyword(Keyword::Struct),
        TokenKind::Keyword(Keyword::Entry),
        TokenKind::Keyword(Keyword::Enum),
        TokenKind::Keyword(Keyword::Option),
        TokenKind::Keyword(Keyword::Map),
        TokenKind::Keyword(Keyword::I8),
        TokenKind::Keyword(Keyword::I16),
        TokenKind::Keyword(Keyword::I32),
        TokenKind::Keyword(Keyword::I64),
        TokenKind::Keyword(Keyword::I128),
        TokenKind::Keyword(Keyword::U8),
        TokenKind::Keyword(Keyword::U16),
        TokenKind::Keyword(Keyword::U32),
        TokenKind::Keyword(Keyword::U64),
        TokenKind::Keyword(Keyword::U128),
        TokenKind::Keyword(Keyword::F32),
        TokenKind::Keyword(Keyword::F64),
        TokenKind::Keyword(Keyword::String),
        TokenKind::Keyword(Keyword::Bool),
        TokenKind::Keyword(Keyword::True),
        TokenKind::Keyword(Keyword::False),
        TokenKind::Keyword(Keyword::None),
        TokenKind::Identifier,
        TokenKind::Identifier,
        TokenKind::Identifier,
        TokenKind::Identifier,
        TokenKind::Symbol(Symbol::PathSeparator),
        TokenKind::Symbol(Symbol::Colon),
        TokenKind::Symbol(Symbol::LeftParen),
        TokenKind::Symbol(Symbol::RightParen),
        TokenKind::Symbol(Symbol::LeftBrace),
        TokenKind::Symbol(Symbol::RightBrace),
        TokenKind::Symbol(Symbol::LeftBracket),
        TokenKind::Symbol(Symbol::RightBracket),
        TokenKind::Symbol(Symbol::Comma),
        TokenKind::Symbol(Symbol::Semicolon),
        TokenKind::Symbol(Symbol::Dot),
        TokenKind::Symbol(Symbol::At),
        TokenKind::Symbol(Symbol::EqualEqual),
        TokenKind::Symbol(Symbol::Equal),
        TokenKind::Symbol(Symbol::NotEqual),
        TokenKind::Symbol(Symbol::Bang),
        TokenKind::Symbol(Symbol::LessEqual),
        TokenKind::Symbol(Symbol::Less),
        TokenKind::Symbol(Symbol::GreaterEqual),
        TokenKind::Symbol(Symbol::Greater),
        TokenKind::Symbol(Symbol::AndAnd),
        TokenKind::Symbol(Symbol::OrOr),
        TokenKind::Symbol(Symbol::Pipe),
        TokenKind::Symbol(Symbol::Plus),
        TokenKind::Symbol(Symbol::Minus),
        TokenKind::Symbol(Symbol::Star),
        TokenKind::Symbol(Symbol::Slash),
    ];

    for kind in expected {
        test_assert_eq!(
            errors
                .diagnostic(|out| cursor.peek(out))
                .test_value()?
                .token()
                .test_value()?
                .kind(),
            kind
        );
        errors.diagnostic(|out| cursor.bump(out)).test_value()?;
    }
    test_assert_eq!(
        errors
            .diagnostic(|out| cursor.peek(out))
            .test_value()?
            .token()
            .test_value()?
            .kind(),
        TokenKind::Eof
    );

    Ok(())
}

#[test]
fn decimal_hexadecimal_and_float_tokens_preserve_their_spelling() -> TestResult {
    let mut errors = TestErrors::new();
    let execution = errors
        .execution(ExecutionLimits::new(67_108_864, 67_108_864))
        .test_value()?;

    let source = "0 007 0x2a 12.50";
    let mut byte_input_2 = SliceSource::new((source).as_bytes());
    let mut cursor = errors
        .diagnostic(|out| {
            TokenCursor::new(
                InputConfig {
                    source_id: SOURCE_ID,
                    source: &mut byte_input_2,
                    resources: &InputLimits::default(),
                    stream: StreamLimits::default(),
                    budget: &execution,
                },
                out,
            )
        })
        .test_value()?;
    let expected = [
        (TokenKind::Integer, "0"),
        (TokenKind::Integer, "007"),
        (TokenKind::Integer, "0x2a"),
        (TokenKind::Float, "12.50"),
    ];

    for (kind, text) in expected {
        let token = errors
            .diagnostic(|out| cursor.peek(out))
            .test_value()?
            .token()
            .test_value()?;
        test_assert_eq!(token.kind(), kind);
        test_assert_eq!(token.text().unwrap_or(""), text);
        errors.diagnostic(|out| cursor.bump(out)).test_value()?;
    }
    test_assert_eq!(
        errors
            .diagnostic(|out| cursor.peek(out))
            .test_value()?
            .token()
            .test_value()?
            .kind(),
        TokenKind::Eof
    );

    Ok(())
}

#[test]
fn malformed_numbers_report_errors_and_allow_scanning_to_resume() -> TestResult {
    let mut errors = TestErrors::new();
    let execution = errors
        .execution(ExecutionLimits::new(67_108_864, 67_108_864))
        .test_value()?;

    let mut byte_input_3 = SliceSource::new(("0x 1e2 .5 1. 1.2.3").as_bytes());
    let mut cursor = errors
        .diagnostic(|out| {
            TokenCursor::new(
                InputConfig {
                    source_id: SOURCE_ID,
                    source: &mut byte_input_3,
                    resources: &InputLimits::default(),
                    stream: StreamLimits::default(),
                    budget: &execution,
                },
                out,
            )
        })
        .test_value()?;
    let mut spans = Vec::new();

    loop {
        match errors.diagnostic(|out| cursor.bump(out)).test_value()? {
            LexEvent::Eof(_) => break,
            LexEvent::Token(_) => {}
            LexEvent::Error(diagnostic) => {
                test_assert_eq!(diagnostic.code(), DiagnosticCode::LEXICAL_ERROR);
                spans.push(diagnostic.primary().test_value()?.span());
            }
            _ => return Err(TestFailure),
        }
    }

    test_assert_eq!(spans.len(), 5);
    test_assert_eq!(
        (
            spans.first().test_value()?.start(),
            spans.first().test_value()?.end()
        ),
        (0, 2)
    );
    test_assert_eq!(
        (
            spans.get(1).test_value()?.start(),
            spans.get(1).test_value()?.end()
        ),
        (3, 6)
    );
    test_assert_eq!(
        (
            spans.get(2).test_value()?.start(),
            spans.get(2).test_value()?.end()
        ),
        (7, 9)
    );
    test_assert_eq!(
        (
            spans.get(3).test_value()?.start(),
            spans.get(3).test_value()?.end()
        ),
        (10, 12)
    );
    test_assert_eq!(
        (
            spans.get(4).test_value()?.start(),
            spans.get(4).test_value()?.end()
        ),
        (13, 18)
    );

    Ok(())
}

#[test]
fn strings_decode_supported_escapes_and_reject_invalid_forms() -> TestResult {
    let mut errors = TestErrors::new();
    let execution = errors
        .execution(ExecutionLimits::new(67_108_864, 67_108_864))
        .test_value()?;

    let source = r#""a\"\\\n\r\t€""#;
    let mut byte_input_4 = SliceSource::new((source).as_bytes());
    let mut cursor = errors
        .diagnostic(|out| {
            TokenCursor::new(
                InputConfig {
                    source_id: SOURCE_ID,
                    source: &mut byte_input_4,
                    resources: &InputLimits::default(),
                    stream: StreamLimits::default(),
                    budget: &execution,
                },
                out,
            )
        })
        .test_value()?;
    let token = errors
        .diagnostic(|out| cursor.peek(out))
        .test_value()?
        .token()
        .test_value()?;
    test_assert_eq!(token.kind(), TokenKind::String);
    test_assert_eq!(token.decoded_string(), Some("a\"\\\n\r\t€"));
    test_assert!(token.text().is_none());

    let mut byte_input_5 = SliceSource::new((r#""bad\q" after"#).as_bytes());
    let mut invalid = errors
        .diagnostic(|out| {
            TokenCursor::new(
                InputConfig {
                    source_id: SOURCE_ID,
                    source: &mut byte_input_5,
                    resources: &InputLimits::default(),
                    stream: StreamLimits::default(),
                    budget: &execution,
                },
                out,
            )
        })
        .test_value()?;
    let diagnostic = errors
        .diagnostic(|out| invalid.peek(out))
        .test_value()?
        .diagnostic()
        .test_value()?;
    test_assert_eq!(diagnostic.code(), DiagnosticCode::LEXICAL_ERROR);
    test_assert_eq!(
        (
            diagnostic.primary().test_value()?.span().start(),
            diagnostic.primary().test_value()?.span().end()
        ),
        (4, 6)
    );
    errors.diagnostic(|out| invalid.bump(out)).test_value()?;
    test_assert_eq!(
        errors
            .diagnostic(|out| invalid.peek(out))
            .test_value()?
            .token()
            .test_value()?
            .text()
            .unwrap_or(""),
        "after"
    );

    let mut byte_input_6 = SliceSource::new(("\"unfinished").as_bytes());
    let mut unclosed = errors
        .diagnostic(|out| {
            TokenCursor::new(
                InputConfig {
                    source_id: SOURCE_ID,
                    source: &mut byte_input_6,
                    resources: &InputLimits::default(),
                    stream: StreamLimits::default(),
                    budget: &execution,
                },
                out,
            )
        })
        .test_value()?;
    let diagnostic = errors
        .diagnostic(|out| unclosed.peek(out))
        .test_value()?
        .diagnostic()
        .test_value()?;
    test_assert_eq!(diagnostic.code(), DiagnosticCode::LEXICAL_ERROR);
    test_assert_eq!(diagnostic.primary().test_value()?.span().end(), 11);

    Ok(())
}

#[test]
fn block_strings_dedent_normalize_lines_decode_escapes_and_preserve_spans() -> TestResult {
    let mut errors = TestErrors::new();
    let execution = errors
        .execution(ExecutionLimits::new(67_108_864, 67_108_864))
        .test_value()?;
    let cases = [
        ("\"\"\"\n\"\"\"", ""),
        ("\"\"\"\n   \t\n \n  \"\"\"", "\n"),
        ("\"\"\"\n    first\n    second\n    \"\"\"", "first\nsecond"),
        ("\"\"\"\n  first\n    second\n  \"\"\"", "first\n  second"),
        (
            "\"\"\"\r\n\t  alpha\\ntwo\r\n\t  beta\r\n\t  \"\"\"",
            "alpha\ntwo\nbeta",
        ),
        (
            "\"\"\"\n  first\n  second\n   \n  \"\"\"",
            "first\nsecond\n",
        ),
        (
            "\"\"\"\n    one\\ntwo\n    \\\"\"\"\n    \"\"\"",
            "one\ntwo\n\"\"\"",
        ),
    ];

    for (source, expected) in cases {
        let mut input = SliceSource::new(source.as_bytes());
        let mut cursor = errors
            .diagnostic(|out| {
                TokenCursor::new(
                    InputConfig {
                        source_id: SOURCE_ID,
                        source: &mut input,
                        resources: &InputLimits::default(),
                        stream: StreamLimits::default(),
                        budget: &execution,
                    },
                    out,
                )
            })
            .test_value()?;
        let token = errors
            .diagnostic(|out| cursor.peek(out))
            .test_value()?
            .token()
            .test_value()?;
        test_assert_eq!(token.kind(), TokenKind::String);
        test_assert_eq!(token.decoded_string(), Some(expected));
        test_assert_eq!(token.located_span().span().start(), 0);
        test_assert_eq!(token.located_span().span().end(), source.len());
        let location = token.located_span().location().test_value()?;
        test_assert_eq!(location.end.line(), source.lines().count());
    }

    let source = "\"\"\"\n  value\n  \"\"\",";
    let mut input = SliceSource::new(source.as_bytes());
    let mut cursor = errors
        .diagnostic(|out| {
            TokenCursor::new(
                InputConfig {
                    source_id: SOURCE_ID,
                    source: &mut input,
                    resources: &InputLimits::default(),
                    stream: StreamLimits::default(),
                    budget: &execution,
                },
                out,
            )
        })
        .test_value()?;
    let event = errors.diagnostic(|out| cursor.bump(out)).test_value()?;
    let token = event.token().test_value()?;
    test_assert_eq!(token.decoded_string(), Some("value"));
    test_assert_eq!(token.located_span().span().end(), source.len() - 1);
    test_assert_eq!(
        errors
            .diagnostic(|out| cursor.peek(out))
            .test_value()?
            .token()
            .test_value()?
            .kind(),
        TokenKind::Symbol(Symbol::Comma)
    );

    Ok(())
}

#[test]
fn malformed_block_strings_report_lexical_errors() -> TestResult {
    let mut errors = TestErrors::new();
    let execution = errors
        .execution(ExecutionLimits::new(67_108_864, 67_108_864))
        .test_value()?;
    for source in [
        "\"\"\" text\n\"\"\"",
        "\"\"\"\n  unfinished",
        "\"\"\"\rtext\n\"\"\"",
        "\"\"\"\n  bad\\q\n\"\"\"",
        "\"\"\"\n  bad\ttext\n\"\"\"",
    ] {
        let mut input = SliceSource::new(source.as_bytes());
        let mut cursor = errors
            .diagnostic(|out| {
                TokenCursor::new(
                    InputConfig {
                        source_id: SOURCE_ID,
                        source: &mut input,
                        resources: &InputLimits::default(),
                        stream: StreamLimits::default(),
                        budget: &execution,
                    },
                    out,
                )
            })
            .test_value()?;
        let diagnostic = errors
            .diagnostic(|out| cursor.peek(out))
            .test_value()?
            .diagnostic()
            .test_value()?;
        test_assert_eq!(diagnostic.code(), DiagnosticCode::LEXICAL_ERROR);
    }

    Ok(())
}

#[test]
fn block_string_decode_errors_point_at_offending_source_text() -> TestResult {
    let mut errors = TestErrors::new();
    let execution = errors
        .execution(ExecutionLimits::new(67_108_864, 67_108_864))
        .test_value()?;
    let cases = [
        BlockStringErrorCase {
            source: "\n\"\"\"\r\n  ẞ\\q\n  trailing\n  \"\"\"",
            needle: "\\q",
            source_length: 2,
            line: 3,
            start_column: 4,
            end_column: 6,
        },
        BlockStringErrorCase {
            source: "\n\"\"\"\r\n  alpha\u{0085}tail\n  trailing\n  \"\"\"",
            needle: "\u{0085}",
            source_length: 2,
            line: 3,
            start_column: 8,
            end_column: 9,
        },
    ];

    for case in cases {
        let start = case.source.find(case.needle).test_value()?;
        let mut input = SliceSource::new(case.source.as_bytes());
        let mut cursor = errors
            .diagnostic(|out| {
                TokenCursor::new(
                    InputConfig {
                        source_id: SOURCE_ID,
                        source: &mut input,
                        resources: &InputLimits::default(),
                        stream: StreamLimits::default(),
                        budget: &execution,
                    },
                    out,
                )
            })
            .test_value()?;
        let diagnostic = errors
            .diagnostic(|out| cursor.peek(out))
            .test_value()?
            .diagnostic()
            .test_value()?;
        let primary = diagnostic.located_primary().test_value()?;
        let span = primary.span();
        test_assert_eq!(
            (span.start(), span.end()),
            (start, start + case.source_length)
        );
        let location = primary.location().test_value()?;
        test_assert_eq!(
            (
                location.start.line(),
                location.start.column(),
                location.end.line(),
                location.end.column()
            ),
            (case.line, case.start_column, case.line, case.end_column)
        );
    }

    Ok(())
}

#[test]
fn comments_and_config_tokens_use_the_shared_lexical_rules() -> TestResult {
    let mut errors = TestErrors::new();
    let execution = errors
        .execution(ExecutionLimits::new(67_108_864, 67_108_864))
        .test_value()?;

    let source = r#"// first line comment
{ enabled: true, optional: none, values: [-1, 0x2a, 1.5], labels: {"tier": "prod",}, secret: env("TOKEN", "fallback"), } /* trailing comment */"#;
    let mut byte_input_7 = SliceSource::new((source).as_bytes());
    let mut cursor = errors
        .diagnostic(|out| {
            TokenCursor::new(
                InputConfig {
                    source_id: SOURCE_ID,
                    source: &mut byte_input_7,
                    resources: &InputLimits::default(),
                    stream: StreamLimits::default(),
                    budget: &execution,
                },
                out,
            )
        })
        .test_value()?;
    let mut seen_env = false;
    let mut count = 0;
    loop {
        let token = errors
            .diagnostic(|out| cursor.peek(out))
            .test_value()?
            .token()
            .test_value()?;
        if token.kind() == TokenKind::Eof {
            break;
        }
        seen_env |= token.kind() == TokenKind::Identifier && token.text().unwrap_or("") == "env";
        count += 1;
        errors.diagnostic(|out| cursor.bump(out)).test_value()?;
    }
    test_assert!(seen_env);
    test_assert!(count > 25);

    Ok(())
}

#[test]
fn malformed_comments_and_non_ascii_whitespace_have_precise_spans() -> TestResult {
    let mut errors = TestErrors::new();
    let execution = errors
        .execution(ExecutionLimits::new(67_108_864, 67_108_864))
        .test_value()?;

    let mut byte_input_8 = SliceSource::new(("// note\r next").as_bytes());
    let mut line_comment = errors
        .diagnostic(|out| {
            TokenCursor::new(
                InputConfig {
                    source_id: SOURCE_ID,
                    source: &mut byte_input_8,
                    resources: &InputLimits::default(),
                    stream: StreamLimits::default(),
                    budget: &execution,
                },
                out,
            )
        })
        .test_value()?;
    let diagnostic = errors
        .diagnostic(|out| line_comment.peek(out))
        .test_value()?
        .diagnostic()
        .test_value()?;
    test_assert_eq!(diagnostic.code(), DiagnosticCode::LEXICAL_ERROR);
    test_assert_eq!(
        (
            diagnostic.primary().test_value()?.span().start(),
            diagnostic.primary().test_value()?.span().end()
        ),
        (7, 8)
    );

    let mut byte_input_9 = SliceSource::new(("/* note\r next */").as_bytes());
    let mut block_comment = errors
        .diagnostic(|out| {
            TokenCursor::new(
                InputConfig {
                    source_id: SOURCE_ID,
                    source: &mut byte_input_9,
                    resources: &InputLimits::default(),
                    stream: StreamLimits::default(),
                    budget: &execution,
                },
                out,
            )
        })
        .test_value()?;
    let diagnostic = errors
        .diagnostic(|out| block_comment.peek(out))
        .test_value()?
        .diagnostic()
        .test_value()?;
    test_assert_eq!(diagnostic.code(), DiagnosticCode::LEXICAL_ERROR);
    test_assert_eq!(
        (
            diagnostic.primary().test_value()?.span().start(),
            diagnostic.primary().test_value()?.span().end()
        ),
        (7, 8)
    );

    let source = "name\u{00a0}next";
    let mut byte_input_10 = SliceSource::new((source).as_bytes());
    let mut invalid_space = errors
        .diagnostic(|out| {
            TokenCursor::new(
                InputConfig {
                    source_id: SOURCE_ID,
                    source: &mut byte_input_10,
                    resources: &InputLimits::default(),
                    stream: StreamLimits::default(),
                    budget: &execution,
                },
                out,
            )
        })
        .test_value()?;
    test_assert_eq!(
        errors
            .diagnostic(|out| invalid_space.peek(out))
            .test_value()?
            .token()
            .test_value()?
            .text()
            .unwrap_or(""),
        "name"
    );
    errors
        .diagnostic(|out| invalid_space.bump(out))
        .test_value()?;
    let diagnostic = errors
        .diagnostic(|out| invalid_space.peek(out))
        .test_value()?
        .diagnostic()
        .test_value()?;
    test_assert_eq!(diagnostic.code(), DiagnosticCode::LEXICAL_ERROR);
    test_assert_eq!(
        (
            diagnostic.primary().test_value()?.span().start(),
            diagnostic.primary().test_value()?.span().end()
        ),
        (4, 6)
    );

    let mut byte_input_11 = SliceSource::new(("/* unfinished").as_bytes());
    let mut unclosed_comment = errors
        .diagnostic(|out| {
            TokenCursor::new(
                InputConfig {
                    source_id: SOURCE_ID,
                    source: &mut byte_input_11,
                    resources: &InputLimits::default(),
                    stream: StreamLimits::default(),
                    budget: &execution,
                },
                out,
            )
        })
        .test_value()?;
    let diagnostic = errors
        .diagnostic(|out| unclosed_comment.peek(out))
        .test_value()?
        .diagnostic()
        .test_value()?;
    test_assert_eq!(diagnostic.code(), DiagnosticCode::LEXICAL_ERROR);
    test_assert_eq!(diagnostic.primary().test_value()?.span().end(), 13);

    Ok(())
}

#[test]
fn crlf_and_multibyte_error_locations_use_byte_spans_and_scalar_columns() -> TestResult {
    let mut errors = TestErrors::new();
    let execution = errors
        .execution(ExecutionLimits::new(67_108_864, 67_108_864))
        .test_value()?;

    let source = "a\r\nẞ\u{00a0}";
    let mut byte_input_12 = SliceSource::new((source).as_bytes());
    let mut cursor = errors
        .diagnostic(|out| {
            TokenCursor::new(
                InputConfig {
                    source_id: SOURCE_ID,
                    source: &mut byte_input_12,
                    resources: &InputLimits::default(),
                    stream: StreamLimits::default(),
                    budget: &execution,
                },
                out,
            )
        })
        .test_value()?;
    test_assert_eq!(
        errors
            .diagnostic(|out| cursor.peek(out))
            .test_value()?
            .token()
            .test_value()?
            .text()
            .unwrap_or(""),
        "a"
    );
    errors.diagnostic(|out| cursor.bump(out)).test_value()?;
    let diagnostic = errors
        .diagnostic(|out| cursor.peek(out))
        .test_value()?
        .diagnostic()
        .test_value()?;
    let span = diagnostic.primary().test_value()?.span();
    test_assert_eq!((span.start(), span.end()), (3, 6));

    let mut allocation_errors = TestSlot::<AllocationError>::default();
    let index = allocation_errors
        .access(|out| LineIndex::new(source, out))
        .test_value()?;
    let mut location_errors = TestSlot::<LocationError>::default();
    let location = location_errors
        .access(|out| index.location(span.start(), out))
        .test_value()?;
    test_assert_eq!((location.line(), location.column()), (2, 1));

    errors.diagnostic(|out| cursor.bump(out)).test_value()?;
    let diagnostic = errors
        .diagnostic(|out| cursor.peek(out))
        .test_value()?
        .diagnostic()
        .test_value()?;
    let span = diagnostic.primary().test_value()?.span();
    test_assert_eq!((span.start(), span.end()), (6, 8));
    let location = location_errors
        .access(|out| index.location(span.start(), out))
        .test_value()?;
    test_assert_eq!((location.line(), location.column()), (2, 2));

    Ok(())
}

#[test]
fn decoded_string_limit_counts_decoded_utf8_bytes() -> TestResult {
    let mut errors = TestErrors::new();
    let execution = errors
        .execution(ExecutionLimits::new(67_108_864, 67_108_864))
        .test_value()?;

    let mut limits = SyntaxLimits::default();
    limits.set_max_string_bytes(1);
    let input_limits = limits.input_limits();

    let mut byte_input_13 = SliceSource::new((r#""\n""#).as_bytes());
    let mut escaped = errors
        .diagnostic(|out| {
            TokenCursor::new(
                InputConfig {
                    source_id: SOURCE_ID,
                    source: &mut byte_input_13,
                    resources: &input_limits,
                    stream: StreamLimits::default(),
                    budget: &execution,
                },
                out,
            )
        })
        .test_value()?;
    test_assert_eq!(
        errors
            .diagnostic(|out| escaped.peek(out))
            .test_value()?
            .token()
            .test_value()?
            .decoded_string(),
        Some("\n")
    );

    let mut byte_input_14 = SliceSource::new(("\"€\"").as_bytes());
    let mut multibyte = errors
        .diagnostic(|out| {
            TokenCursor::new(
                InputConfig {
                    source_id: SOURCE_ID,
                    source: &mut byte_input_14,
                    resources: &input_limits,
                    stream: StreamLimits::default(),
                    budget: &execution,
                },
                out,
            )
        })
        .test_value()?;
    let diagnostic = errors
        .diagnostic(|out| multibyte.peek(out))
        .test_value()?
        .diagnostic()
        .test_value()?;
    test_assert_eq!(diagnostic.code(), DiagnosticCode::RESOURCE_LIMIT_EXCEEDED);

    Ok(())
}

#[test]
fn block_string_limits_count_source_token_and_decoded_output_separately() -> TestResult {
    let mut errors = TestErrors::new();
    let execution = errors
        .execution(ExecutionLimits::new(67_108_864, 67_108_864))
        .test_value()?;
    let source = format!(
        "\"\"\"\n{}value\n{}\"\"\"",
        " ".repeat(512),
        " ".repeat(512)
    );
    for token_limit in [source.len() - 1, source.len()] {
        for string_limit in [4, 5] {
            let stream = StreamLimits::try_new(1, token_limit).test_value()?;
            let mut input = SliceSource::new(source.as_bytes());
            let mut cursor = errors
                .diagnostic(|out| {
                    TokenCursor::new(
                        InputConfig {
                            source_id: SOURCE_ID,
                            source: &mut input,
                            resources: &InputLimits::new(source.len(), string_limit),
                            stream,
                            budget: &execution,
                        },
                        out,
                    )
                })
                .test_value()?;
            let event = errors.diagnostic(|out| cursor.peek(out)).test_value()?;
            if token_limit == source.len() && string_limit == 5 {
                test_assert_eq!(event.token().test_value()?.decoded_string(), Some("value"));
            } else {
                test_assert_eq!(
                    event.diagnostic().test_value()?.code(),
                    DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
                );
            }
        }
    }

    Ok(())
}
