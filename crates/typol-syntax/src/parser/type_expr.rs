use crate::{
    Keyword, Symbol, TokenKind,
    error::SyntaxDiagnostic,
    parser::{
        annotation::AnnotationParser,
        context::{ParserControl, ParserState},
    },
    storage::{
        Annotation, ArrayLength, ArrayTypeExprParts, BuiltinType, Id, TypeBuilder, TypeExpr,
    },
};
use opeo::{OResult, Out};
use typol_core::{ChargedVec, Diagnostic, SourceSpan};
use typol_i18n::t;

enum TypeFrame {
    Option(SourceSpan),
    Map(SourceSpan),
    Array(SourceSpan),
}

enum TypeStart {
    Nested,
    Complete(Id<TypeExpr>),
}

enum ClosedTypeFrame {
    Option {
        expression: Id<TypeExpr>,
        span: SourceSpan,
    },
    Map {
        expression: Id<TypeExpr>,
        span: SourceSpan,
    },
    Array {
        expression: Id<TypeExpr>,
        length: Option<Id<ArrayLength>>,
        span: SourceSpan,
    },
}

enum TypeParseResult {
    Complete(Id<TypeExpr>),
    Pending(ClosedTypeFrame),
}

pub(crate) struct TypeParser;

impl TypeParser {
    pub(crate) fn parse_type_expr<'slot>(
        parser: &mut ParserState<'_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<TypeExpr>> {
        let saved_depth = parser.prefix_frames;
        let parsed = TypeParser::parse_type_frames(parser, out.reborrow());
        let result = match parsed {
            Ok(TypeParseResult::Complete(expression)) => Ok(expression),
            Ok(TypeParseResult::Pending(frame)) => {
                match AnnotationParser::parse_annotations(parser, out.reborrow()) {
                    Ok(annotations) => {
                        TypeParser::attach_annotations(parser, frame, annotations, out.reborrow())
                    }
                    Err(error) => Err(error),
                }
            }
            Err(error) => Err(error),
        };
        // On failure, unmatched delimiters are left to recovery; angle-bracket frames belong to this type parse.
        (ParserControl::restore_prefix_frames(parser, saved_depth, out.reborrow()))?;
        result
    }

    fn parse_type_frames<'slot>(
        parser: &mut ParserState<'_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, TypeParseResult> {
        let mut frames =
            ParserControl::new_vec(parser, ParserControl::eof_span(parser), out.reborrow())?;
        let mut expression = loop {
            match TypeParser::begin_type(parser, &mut frames, out.reborrow())? {
                TypeStart::Nested => {}
                TypeStart::Complete(expression) => break expression,
            }
        };

        while let Some(frame) = frames.pop() {
            parser.execution.consume_with(1, out.reborrow(), |error| {
                SyntaxDiagnostic::budget(error, ParserControl::eof_span(parser), parser.translator)
            })?;
            let closed = TypeParser::close_frame(parser, frame, expression, out.reborrow())?;
            if frames.is_empty() {
                return Ok(TypeParseResult::Pending(closed));
            }
            let annotations = AnnotationParser::parse_annotations(parser, out.reborrow())?;
            expression =
                TypeParser::attach_annotations(parser, closed, annotations, out.reborrow())?;
        }
        Ok(TypeParseResult::Complete(expression))
    }

    fn begin_type<'slot>(
        parser: &mut ParserState<'_>,
        frames: &mut ChargedVec<TypeFrame>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, TypeStart> {
        parser.execution.consume_with(1, out.reborrow(), |error| {
            SyntaxDiagnostic::budget(error, ParserControl::eof_span(parser), parser.translator)
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
                ParserControl::try_push(parser, frames, frame, info.span, out.reborrow())?;
                Ok(TypeStart::Nested)
            }
            TokenKind::Symbol(Symbol::LeftBracket) => {
                ParserControl::bump(parser, out.reborrow())?;
                ParserControl::try_push(
                    parser,
                    frames,
                    TypeFrame::Array(info.span),
                    info.span,
                    out.reborrow(),
                )?;
                Ok(TypeStart::Nested)
            }
            TokenKind::Keyword(keyword) => {
                let Some(builtin) = builtin_type(keyword) else {
                    return ParserControl::syntax_failure(
                        parser,
                        info.span,
                        t! {
                            name: "diagnostic.syntax.invalid_type",
                            locale: parser.translator,
                        },
                        out.reborrow(),
                    );
                };
                ParserControl::bump(parser, out.reborrow())?;
                let annotations = AnnotationParser::parse_annotations(parser, out.reborrow())?;
                let expression = TypeBuilder::type_expr_builtin(
                    &mut parser.ast,
                    builtin,
                    annotations,
                    info.span,
                    out.reborrow(),
                )?;
                Ok(TypeStart::Complete(expression))
            }
            TokenKind::Identifier => Ok(TypeStart::Complete(TypeParser::parse_path_type(
                parser,
                info.span,
                out.reborrow(),
            )?)),
            _ => ParserControl::syntax_failure(
                parser,
                info.span,
                t! {
                    name: "diagnostic.syntax.expected_type_expression",
                    locale: parser.translator,
                },
                out.reborrow(),
            ),
        }
    }

    fn close_frame<'slot>(
        parser: &mut ParserState<'_>,
        frame: TypeFrame,
        expression: Id<TypeExpr>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ClosedTypeFrame> {
        match frame {
            TypeFrame::Option(start) => {
                ParserControl::expect_symbol(parser, Symbol::Greater, out.reborrow())?;
                ParserControl::leave_prefix_frame(parser, out.reborrow())?;
                let span = ParserControl::span_from_start(parser, start, out.reborrow())?;
                Ok(ClosedTypeFrame::Option { expression, span })
            }
            TypeFrame::Map(start) => {
                ParserControl::expect_symbol(parser, Symbol::Greater, out.reborrow())?;
                ParserControl::leave_prefix_frame(parser, out.reborrow())?;
                let span = ParserControl::span_from_start(parser, start, out.reborrow())?;
                Ok(ClosedTypeFrame::Map { expression, span })
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
                Ok(ClosedTypeFrame::Array {
                    expression,
                    length,
                    span,
                })
            }
        }
    }

    fn attach_annotations<'slot>(
        parser: &mut ParserState<'_>,
        frame: ClosedTypeFrame,
        annotations: ChargedVec<Id<Annotation>>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<TypeExpr>> {
        match frame {
            ClosedTypeFrame::Option { expression, span } => TypeBuilder::type_expr_option(
                &mut parser.ast,
                expression,
                annotations,
                span,
                out.reborrow(),
            ),
            ClosedTypeFrame::Map { expression, span } => TypeBuilder::type_expr_map(
                &mut parser.ast,
                expression,
                annotations,
                span,
                out.reborrow(),
            ),
            ClosedTypeFrame::Array {
                expression,
                length,
                span,
            } => TypeBuilder::type_expr_array(
                &mut parser.ast,
                ArrayTypeExprParts {
                    element: expression,
                    length,
                },
                annotations,
                span,
                out.reborrow(),
            ),
        }
    }

    fn parse_path_type<'slot>(
        parser: &mut ParserState<'_>,
        start: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<TypeExpr>> {
        let mut segments =
            ParserControl::new_vec(parser, ParserControl::eof_span(parser), out.reborrow())?;
        let first = ParserControl::take_name(parser, out.reborrow())?;
        ParserControl::try_push(parser, &mut segments, first, start, out.reborrow())?;
        while ParserControl::eat_symbol(parser, Symbol::PathSeparator, out.reborrow())?.is_some() {
            parser.execution.consume_with(1, out.reborrow(), |error| {
                SyntaxDiagnostic::budget(error, ParserControl::eof_span(parser), parser.translator)
            })?;

            let segment = ParserControl::take_name(parser, out.reborrow())?;
            let span = ParserControl::span_from_start(parser, start, out.reborrow())?;
            ParserControl::try_push(parser, &mut segments, segment, span, out.reborrow())?;
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
