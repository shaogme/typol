use crate::{
    Keyword, Symbol, TokenKind,
    parser::{
        context::{ParserControl, ParserState},
        expression::ExpressionParser,
        value::DefaultParser,
    },
    storage::{
        Annotation, AnnotationData, AnnotationKind, BooleanLiteral, Id, NamedArgument, NamedBounds,
        NumberLiteral, SizeArguments, StringLiteral,
    },
};
use opeo::{OResult, Out};
use typol_core::{BudgetError, ChargedVec, Diagnostic, ExecutionBudget};
use typol_i18n::diagnostic_message;

pub(crate) struct AnnotationParser;

impl AnnotationParser {
    pub(crate) fn parse_annotations<'slot>(
        parser: &mut ParserState<'_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ChargedVec<Id<Annotation>>> {
        let mut annotations =
            ParserControl::new_vec(parser, ParserControl::eof_span(parser), out.reborrow())?;
        while ParserControl::peek(parser, out.reborrow())?.kind == TokenKind::Symbol(Symbol::At) {
            let annotation = AnnotationParser::parse_annotation(parser, out.reborrow())?;
            let span = parser.ast.span(annotation, out.reborrow())?;
            ParserControl::try_push(&mut annotations, annotation, span, out.reborrow())?;
        }
        Ok(annotations)
    }

    fn parse_annotation<'slot>(
        parser: &mut ParserState<'_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<Annotation>> {
        let start = ParserControl::expect_symbol(parser, Symbol::At, out.reborrow())?.span;
        let name = ParserControl::take_name(parser, out.reborrow())?;
        let name_span = parser.ast.span(name, out.reborrow())?;
        let name_text = parser.ast.name_text_for_diagnostic(name, out.reborrow())?;
        let kind = annotation_kind(name_text, &parser.execution, out.reborrow())?;
        let Some(kind) = kind else {
            return ParserControl::syntax_failure(
                parser,
                name_span,
                diagnostic_message!(
                    "diagnostic.syntax.unknown_annotation",
                    "unknown Schema annotation"
                ),
                out.reborrow(),
            );
        };
        let data = AnnotationParser::parse_annotation_data(parser, kind, out.reborrow())?;
        parser.ast.annotation_new(
            data,
            ParserControl::span_from_start(parser, start, out.reborrow())?,
            out.reborrow(),
        )
    }

    fn parse_annotation_data<'slot>(
        parser: &mut ParserState<'_>,
        kind: AnnotationKind,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, AnnotationData> {
        match kind {
            AnnotationKind::Host => Ok(AnnotationData::Host),
            AnnotationKind::Email => Ok(AnnotationData::Email),
            AnnotationKind::Secret => Ok(AnnotationData::Secret),
            AnnotationKind::Unique => AnnotationParser::parse_unique(parser, out.reborrow()),
            AnnotationKind::Min
            | AnnotationKind::Max
            | AnnotationKind::Range
            | AnnotationKind::Length => {
                AnnotationParser::parse_numeric_bounds(parser, kind, out.reborrow())
            }
            AnnotationKind::Regex | AnnotationKind::Env => {
                AnnotationParser::parse_text_annotation(parser, kind, out.reborrow())
            }
            AnnotationKind::Cidr => AnnotationParser::parse_cidr(parser, out.reborrow()),
            AnnotationKind::Size => AnnotationParser::parse_size(parser, out.reborrow()),
            AnnotationKind::All => AnnotationParser::parse_all(parser, out.reborrow()),
            AnnotationKind::Sum => AnnotationParser::parse_sum(parser, out.reborrow()),
        }
    }

    fn parse_unique<'slot>(
        parser: &mut ParserState<'_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, AnnotationData> {
        let by = if ParserControl::eat_symbol(parser, Symbol::LeftParen, out.reborrow())?.is_some()
        {
            let argument = AnnotationParser::parse_named_string(
                parser,
                "by",
                "@unique argument must be by",
                out.reborrow(),
            )?;
            ParserControl::expect_symbol(parser, Symbol::RightParen, out.reborrow())?;
            Some(argument)
        } else {
            None
        };
        Ok(AnnotationData::Unique(by))
    }

    fn parse_numeric_bounds<'slot>(
        parser: &mut ParserState<'_>,
        kind: AnnotationKind,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, AnnotationData> {
        ParserControl::expect_symbol(parser, Symbol::LeftParen, out.reborrow())?;
        if kind == AnnotationKind::Min || kind == AnnotationKind::Max {
            let number = DefaultParser::parse_number_annotation(parser, false, out.reborrow())?;
            ParserControl::expect_symbol(parser, Symbol::RightParen, out.reborrow())?;
            return if kind == AnnotationKind::Min {
                Ok(AnnotationData::Min(number))
            } else {
                Ok(AnnotationData::Max(number))
            };
        }
        let decimal_only = kind == AnnotationKind::Length;
        let min = DefaultParser::parse_number_annotation(parser, decimal_only, out.reborrow())?;
        ParserControl::expect_symbol(parser, Symbol::Comma, out.reborrow())?;
        let max = DefaultParser::parse_number_annotation(parser, decimal_only, out.reborrow())?;
        ParserControl::expect_symbol(parser, Symbol::RightParen, out.reborrow())?;
        if decimal_only {
            Ok(AnnotationData::Length { min, max })
        } else {
            Ok(AnnotationData::Range { min, max })
        }
    }

    fn parse_text_annotation<'slot>(
        parser: &mut ParserState<'_>,
        kind: AnnotationKind,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, AnnotationData> {
        ParserControl::expect_symbol(parser, Symbol::LeftParen, out.reborrow())?;
        let value = ParserControl::take_string(parser, out.reborrow())?;
        ParserControl::expect_symbol(parser, Symbol::RightParen, out.reborrow())?;
        if kind == AnnotationKind::Regex {
            Ok(AnnotationData::Regex(value))
        } else {
            Ok(AnnotationData::Env(value))
        }
    }

    fn parse_cidr<'slot>(
        parser: &mut ParserState<'_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, AnnotationData> {
        ParserControl::expect_symbol(parser, Symbol::LeftParen, out.reborrow())?;
        let name = ParserControl::take_name(parser, out.reborrow())?;
        if !ParserControl::name_is(parser, name, "v4_only", out.reborrow())? {
            let span = parser.ast.span(name, out.reborrow())?;
            return ParserControl::syntax_failure(
                parser,
                span,
                diagnostic_message!(
                    "diagnostic.syntax.cidr_v4_only_argument",
                    "@cidr argument must be v4_only"
                ),
                out.reborrow(),
            );
        }
        ParserControl::expect_symbol(parser, Symbol::Equal, out.reborrow())?;
        let value = AnnotationParser::take_boolean(parser, out.reborrow())?;
        let span = ParserControl::span_between(
            parser,
            parser.ast.span(name, out.reborrow())?,
            parser.ast.span(value, out.reborrow())?,
            out.reborrow(),
        )?;
        ParserControl::expect_symbol(parser, Symbol::RightParen, out.reborrow())?;
        Ok(AnnotationData::Cidr(NamedArgument { name, value, span }))
    }

    fn parse_size<'slot>(
        parser: &mut ParserState<'_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, AnnotationData> {
        ParserControl::expect_symbol(parser, Symbol::LeftParen, out.reborrow())?;
        let bounds = if ParserControl::peek(parser, out.reborrow())?.kind == TokenKind::Integer {
            let min = DefaultParser::parse_number_annotation(parser, true, out.reborrow())?;
            ParserControl::expect_symbol(parser, Symbol::Comma, out.reborrow())?;
            let max = DefaultParser::parse_number_annotation(parser, true, out.reborrow())?;
            SizeArguments::Positional { min, max }
        } else {
            SizeArguments::Named(AnnotationParser::parse_named_bounds(
                parser,
                true,
                out.reborrow(),
            )?)
        };
        ParserControl::expect_symbol(parser, Symbol::RightParen, out.reborrow())?;
        Ok(AnnotationData::Size(bounds))
    }

    fn parse_all<'slot>(
        parser: &mut ParserState<'_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, AnnotationData> {
        ParserControl::expect_symbol(parser, Symbol::LeftParen, out.reborrow())?;
        let expression = ExpressionParser::parse_expression(parser, out.reborrow())?;
        ParserControl::expect_symbol(parser, Symbol::Comma, out.reborrow())?;
        let message = ParserControl::take_string(parser, out.reborrow())?;
        let span = ParserControl::span_between(
            parser,
            parser.ast.span(expression, out.reborrow())?,
            parser.ast.span(message, out.reborrow())?,
            out.reborrow(),
        )?;
        ParserControl::expect_symbol(parser, Symbol::RightParen, out.reborrow())?;
        Ok(AnnotationData::All(parser.ast.assertion_new(
            expression,
            message,
            span,
            out.reborrow(),
        )?))
    }

    fn parse_sum<'slot>(
        parser: &mut ParserState<'_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, AnnotationData> {
        ParserControl::expect_symbol(parser, Symbol::LeftParen, out.reborrow())?;
        let bounds = AnnotationParser::parse_named_bounds(parser, false, out.reborrow())?;
        ParserControl::expect_symbol(parser, Symbol::RightParen, out.reborrow())?;
        Ok(AnnotationData::Sum(bounds))
    }

    fn parse_named_string<'slot>(
        parser: &mut ParserState<'_>,
        expected: &str,
        message: &'static str,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, NamedArgument<Id<StringLiteral>>> {
        let name = ParserControl::take_name(parser, out.reborrow())?;
        if !ParserControl::name_is(parser, name, expected, out.reborrow())? {
            let span = parser.ast.span(name, out.reborrow())?;
            return ParserControl::syntax_failure(parser, span, message, out.reborrow());
        }
        ParserControl::expect_symbol(parser, Symbol::Equal, out.reborrow())?;
        let value = ParserControl::take_string(parser, out.reborrow())?;
        let span = ParserControl::span_between(
            parser,
            parser.ast.span(name, out.reborrow())?,
            parser.ast.span(value, out.reborrow())?,
            out.reborrow(),
        )?;
        Ok(NamedArgument { name, value, span })
    }

    fn parse_named_bounds<'slot>(
        parser: &mut ParserState<'_>,
        decimal_only: bool,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, NamedBounds> {
        let first = AnnotationParser::parse_named_bound(parser, decimal_only, out.reborrow())?;
        let second = if ParserControl::eat_symbol(parser, Symbol::Comma, out.reborrow())?.is_some()
        {
            Some(AnnotationParser::parse_named_bound(
                parser,
                decimal_only,
                out.reborrow(),
            )?)
        } else {
            None
        };
        Ok(NamedBounds { first, second })
    }

    fn parse_named_bound<'slot>(
        parser: &mut ParserState<'_>,
        decimal_only: bool,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, NamedArgument<Id<NumberLiteral>>> {
        let name = ParserControl::take_name(parser, out.reborrow())?;
        if !ParserControl::name_is(parser, name, "min", out.reborrow())?
            && !ParserControl::name_is(parser, name, "max", out.reborrow())?
        {
            let span = parser.ast.span(name, out.reborrow())?;
            return ParserControl::syntax_failure(
                parser,
                span,
                diagnostic_message!(
                    "diagnostic.syntax.boundary_argument",
                    "boundary argument must be min or max"
                ),
                out.reborrow(),
            );
        }
        ParserControl::expect_symbol(parser, Symbol::Equal, out.reborrow())?;
        let number = DefaultParser::parse_number_annotation(parser, decimal_only, out.reborrow())?;
        let span = ParserControl::span_between(
            parser,
            parser.ast.span(name, out.reborrow())?,
            parser.ast.span(number, out.reborrow())?,
            out.reborrow(),
        )?;
        Ok(NamedArgument {
            name,
            value: number,
            span,
        })
    }

    fn take_boolean<'slot>(
        parser: &mut ParserState<'_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<BooleanLiteral>> {
        let info = ParserControl::peek(parser, out.reborrow())?;
        let value = match info.kind {
            TokenKind::Keyword(Keyword::True) => true,
            TokenKind::Keyword(Keyword::False) => false,
            _ => {
                return ParserControl::syntax_failure(
                    parser,
                    info.span,
                    diagnostic_message!(
                        "diagnostic.syntax.expected_boolean_literal",
                        "expected a boolean literal here"
                    ),
                    out.reborrow(),
                );
            }
        };
        ParserControl::bump(parser, out.reborrow())?;
        parser
            .ast
            .boolean_literal_new(value, info.span, out.reborrow())
    }
}

fn annotation_kind<'slot, E>(
    name: &str,
    execution: &ExecutionBudget,
    mut out: Out<'slot, '_, E>,
) -> OResult<'slot, Option<AnnotationKind>>
where
    E: From<BudgetError>,
{
    for (text, kind) in [
        ("min", AnnotationKind::Min),
        ("max", AnnotationKind::Max),
        ("range", AnnotationKind::Range),
        ("length", AnnotationKind::Length),
        ("regex", AnnotationKind::Regex),
        ("host", AnnotationKind::Host),
        ("email", AnnotationKind::Email),
        ("cidr", AnnotationKind::Cidr),
        ("env", AnnotationKind::Env),
        ("size", AnnotationKind::Size),
        ("unique", AnnotationKind::Unique),
        ("all", AnnotationKind::All),
        ("sum", AnnotationKind::Sum),
        ("secret", AnnotationKind::Secret),
    ] {
        execution.consume_with(1, out.reborrow(), E::from)?;
        if execution.compare_text(name, text, out.reborrow())?.is_eq() {
            return Ok(Some(kind));
        }
    }
    Ok(None)
}
