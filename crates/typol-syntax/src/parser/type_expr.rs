use crate::{
    Keyword, Symbol, TokenKind,
    error::SyntaxDiagnostic,
    parser::{
        annotation::AnnotationParser,
        context::{ParserControl, ParserState},
    },
    storage::{ArrayTypeExprParts, BuiltinType, Id, TypeBuilder, TypeExpr},
};
use opeo::{OResult, Out};
use typol_core::{Diagnostic, SourceSpan};
use typol_i18n::diagnostic_message;

enum TypeFrame {
    Option(SourceSpan),
    Map(SourceSpan),
    Array(SourceSpan),
}

pub(crate) struct TypeParser;

impl TypeParser {
    pub(crate) fn parse_type_expr<'slot>(
        parser: &mut ParserState<'_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<TypeExpr>> {
        let saved_depth = parser.prefix_frames;
        let result = TypeParser::parse_type_frames(parser, out.reborrow());
        // On failure, unmatched delimiters are left to recovery; angle-bracket frames belong to this type parse.
        (ParserControl::restore_prefix_frames(parser, saved_depth, out.reborrow()))?;
        result
    }

    fn parse_type_frames<'slot>(
        parser: &mut ParserState<'_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<TypeExpr>> {
        let mut frames =
            ParserControl::new_vec(parser, ParserControl::eof_span(parser), out.reborrow())?;
        let mut expression = loop {
            parser.execution.consume_with(1, out.reborrow(), |error| {
                SyntaxDiagnostic::budget(error, ParserControl::eof_span(parser))
            })?;

            let info = ParserControl::peek(parser, out.reborrow())?;
            match info.kind {
                TokenKind::Keyword(Keyword::Option | Keyword::Map) => {
                    ParserControl::enter_prefix_frame(parser, info.span, out.reborrow())?;
                    ParserControl::bump(parser, out.reborrow())?;
                    ParserControl::expect_symbol(parser, Symbol::Less, out.reborrow())?;
                    let frame = if info.kind == TokenKind::Keyword(Keyword::Map) {
                        ParserControl::expect_keyword(parser, Keyword::String, out.reborrow())?;
                        ParserControl::expect_symbol(parser, Symbol::Comma, out.reborrow())?;
                        TypeFrame::Map(info.span)
                    } else {
                        TypeFrame::Option(info.span)
                    };
                    ParserControl::try_push(&mut frames, frame, info.span, out.reborrow())?;
                }
                TokenKind::Symbol(Symbol::LeftBracket) => {
                    ParserControl::bump(parser, out.reborrow())?;
                    ParserControl::try_push(
                        &mut frames,
                        TypeFrame::Array(info.span),
                        info.span,
                        out.reborrow(),
                    )?;
                }
                TokenKind::Keyword(keyword) => {
                    let Some(builtin) = builtin_type(keyword) else {
                        return ParserControl::syntax_failure(
                            parser,
                            info.span,
                            diagnostic_message!(
                                "diagnostic.syntax.invalid_type",
                                "not a valid type here"
                            ),
                            out.reborrow(),
                        );
                    };
                    ParserControl::bump(parser, out.reborrow())?;
                    let annotations = AnnotationParser::parse_annotations(parser, out.reborrow())?;
                    break TypeBuilder::type_expr_builtin(
                        &mut parser.ast,
                        builtin,
                        annotations,
                        info.span,
                        out.reborrow(),
                    )?;
                }
                TokenKind::Identifier => {
                    break TypeParser::parse_path_type(parser, info.span, out.reborrow())?;
                }
                _ => {
                    return ParserControl::syntax_failure(
                        parser,
                        info.span,
                        diagnostic_message!(
                            "diagnostic.syntax.expected_type_expression",
                            "expected a type expression here"
                        ),
                        out.reborrow(),
                    );
                }
            }
        };

        while let Some(frame) = frames.pop() {
            parser.execution.consume_with(1, out.reborrow(), |error| {
                SyntaxDiagnostic::budget(error, ParserControl::eof_span(parser))
            })?;

            expression = match frame {
                TypeFrame::Option(start) => {
                    ParserControl::expect_symbol(parser, Symbol::Greater, out.reborrow())?;
                    ParserControl::leave_prefix_frame(parser, out.reborrow())?;
                    let span = ParserControl::span_from_start(parser, start, out.reborrow())?;
                    let annotations = AnnotationParser::parse_annotations(parser, out.reborrow())?;
                    TypeBuilder::type_expr_option(
                        &mut parser.ast,
                        expression,
                        annotations,
                        span,
                        out.reborrow(),
                    )?
                }
                TypeFrame::Map(start) => {
                    ParserControl::expect_symbol(parser, Symbol::Greater, out.reborrow())?;
                    ParserControl::leave_prefix_frame(parser, out.reborrow())?;
                    let span = ParserControl::span_from_start(parser, start, out.reborrow())?;
                    let annotations = AnnotationParser::parse_annotations(parser, out.reborrow())?;
                    TypeBuilder::type_expr_map(
                        &mut parser.ast,
                        expression,
                        annotations,
                        span,
                        out.reborrow(),
                    )?
                }
                TypeFrame::Array(start) => {
                    let length =
                        if ParserControl::eat_symbol(parser, Symbol::Semicolon, out.reborrow())?
                            .is_some()
                        {
                            let literal =
                                ParserControl::take_number(parser, false, true, out.reborrow())?;
                            let literal_span = parser.ast.span(literal, out.reborrow())?;
                            Some(parser.ast.array_length_number(
                                literal,
                                literal_span,
                                out.reborrow(),
                            )?)
                        } else {
                            None
                        };
                    ParserControl::expect_symbol(parser, Symbol::RightBracket, out.reborrow())?;
                    let span = ParserControl::span_from_start(parser, start, out.reborrow())?;
                    let annotations = AnnotationParser::parse_annotations(parser, out.reborrow())?;
                    TypeBuilder::type_expr_array(
                        &mut parser.ast,
                        ArrayTypeExprParts {
                            element: expression,
                            length,
                        },
                        annotations,
                        span,
                        out.reborrow(),
                    )?
                }
            };
        }
        Ok(expression)
    }

    fn parse_path_type<'slot>(
        parser: &mut ParserState<'_>,
        start: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<TypeExpr>> {
        let mut segments =
            ParserControl::new_vec(parser, ParserControl::eof_span(parser), out.reborrow())?;
        let first = ParserControl::take_name(parser, out.reborrow())?;
        ParserControl::try_push(&mut segments, first, start, out.reborrow())?;
        while ParserControl::eat_symbol(parser, Symbol::PathSeparator, out.reborrow())?.is_some() {
            parser.execution.consume_with(1, out.reborrow(), |error| {
                SyntaxDiagnostic::budget(error, ParserControl::eof_span(parser))
            })?;

            let segment = ParserControl::take_name(parser, out.reborrow())?;
            let span = ParserControl::span_from_start(parser, start, out.reborrow())?;
            ParserControl::try_push(&mut segments, segment, span, out.reborrow())?;
        }
        let span = ParserControl::span_from_start(parser, start, out.reborrow())?;
        let annotations = AnnotationParser::parse_annotations(parser, out.reborrow())?;
        TypeBuilder::type_expr_path(&mut parser.ast, segments, annotations, span, out.reborrow())
    }
}

fn builtin_type(keyword: Keyword) -> Option<BuiltinType> {
    Some(match keyword {
        Keyword::I8 => BuiltinType::I8,
        Keyword::I16 => BuiltinType::I16,
        Keyword::I32 => BuiltinType::I32,
        Keyword::I64 => BuiltinType::I64,
        Keyword::I128 => BuiltinType::I128,
        Keyword::U8 => BuiltinType::U8,
        Keyword::U16 => BuiltinType::U16,
        Keyword::U32 => BuiltinType::U32,
        Keyword::U64 => BuiltinType::U64,
        Keyword::U128 => BuiltinType::U128,
        Keyword::F32 => BuiltinType::F32,
        Keyword::F64 => BuiltinType::F64,
        Keyword::String => BuiltinType::String,
        Keyword::Bool => BuiltinType::Bool,
        _ => return None,
    })
}
