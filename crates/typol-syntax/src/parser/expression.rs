use crate::{
    Keyword, Symbol, TokenKind,
    error::SyntaxDiagnostic,
    parser::context::{ParserControl, ParserState},
    storage::{
        BinaryExpressionParts, BinaryOperator, Expr, ExpressionBuilder, Id, MethodCallParts, Name,
        ScalarLiteral, UnaryOperator,
    },
};
use opeo::{OResult, Out};
use typol_core::{ChargedVec, Diagnostic, SourceSpan};
use typol_i18n::diagnostic_message;

struct BinaryOperation {
    operator: BinaryOperator,
    precedence: u8,
    is_comparison: bool,
}

// Each continuation frame stores the operation to run when its subexpression completes and does not borrow the frame table while it grows.
enum ExpressionFrame {
    Unary {
        operator: UnaryOperator,
        start: SourceSpan,
        precedence: u8,
    },
    Group {
        start: SourceSpan,
        precedence: u8,
    },
    Binary {
        left: Id<Expr>,
        operator: BinaryOperator,
        precedence: u8,
        comparison_seen: bool,
    },
    Method {
        receiver: Id<Expr>,
        method: Id<Name>,
        parameter: Option<Id<Name>>,
        argument_start: SourceSpan,
        precedence: u8,
    },
}

enum ExpressionStart {
    Nested { precedence: u8 },
    Ready(Id<Expr>),
}

enum PostfixResult {
    Ready(Id<Expr>),
    Nested { precedence: u8 },
    NotApplicable(Id<Expr>),
}

struct CompletedExpression {
    expression: Id<Expr>,
    precedence: u8,
    comparison_seen: bool,
    postfix: bool,
}

pub(crate) struct ExpressionParser;

impl ExpressionParser {
    pub(crate) fn parse_expression<'slot>(
        parser: &mut ParserState<'_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<Expr>> {
        let saved_depth = parser.prefix_frames;
        let result = ExpressionParser::parse_expression_frames(parser, out.reborrow());
        ParserControl::restore_prefix_frames(parser, saved_depth, out.reborrow())?;
        result
    }

    fn parse_expression_frames<'slot>(
        parser: &mut ParserState<'_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<Expr>> {
        let mut frames =
            ParserControl::new_vec(parser, ParserControl::eof_span(parser), out.reborrow())?;
        let mut precedence = 1;
        let mut comparison_seen = false;
        let mut left = None;
        let mut postfix = true;

        loop {
            parser.execution.consume_with(1, out.reborrow(), |error| {
                SyntaxDiagnostic::budget(error, ParserControl::eof_span(parser))
            })?;

            if left.is_none() {
                match ExpressionParser::begin_expression(
                    parser,
                    &mut frames,
                    precedence,
                    out.reborrow(),
                )? {
                    ExpressionStart::Nested {
                        precedence: nested_precedence,
                    } => precedence = nested_precedence,
                    ExpressionStart::Ready(expression) => left = Some(expression),
                }
                comparison_seen = false;
                postfix = true;
                continue;
            }

            let Some(mut expression) = left.take() else {
                return Err(out.reborrow().fail(parser_internal_error!(
                    "expression-frame-has-no-left-value",
                    ParserControl::eof_span(parser),
                )));
            };

            if postfix {
                match ExpressionParser::parse_postfix(
                    parser,
                    &mut frames,
                    expression,
                    precedence,
                    out.reborrow(),
                )? {
                    PostfixResult::Ready(completed) => {
                        left = Some(completed);
                        continue;
                    }
                    PostfixResult::Nested {
                        precedence: nested_precedence,
                    } => {
                        precedence = nested_precedence;
                        comparison_seen = false;
                        left = None;
                        continue;
                    }
                    PostfixResult::NotApplicable(original) => expression = original,
                }
            }

            let info = ParserControl::peek(parser, out.reborrow())?;
            if let Some(BinaryOperation {
                operator,
                precedence: next_precedence,
                is_comparison,
            }) = binary_operator(info.kind)
            {
                if next_precedence >= precedence {
                    if is_comparison && comparison_seen {
                        return ParserControl::syntax_failure(
                            parser,
                            info.span,
                            diagnostic_message!(
                                "diagnostic.syntax.comparison_operators_cannot_be_chained",
                                "comparison operators cannot be chained"
                            ),
                            out.reborrow(),
                        );
                    }
                    ParserControl::bump(parser, out.reborrow())?;
                    ParserControl::try_push(
                        &mut frames,
                        ExpressionFrame::Binary {
                            left: expression,
                            operator,
                            precedence,
                            comparison_seen: comparison_seen || is_comparison,
                        },
                        info.span,
                        out.reborrow(),
                    )?;
                    precedence = next_precedence + 1;
                    comparison_seen = false;
                    postfix = true;
                    continue;
                }
            }

            let Some(frame) = frames.pop() else {
                break Ok(expression);
            };
            let completed =
                ExpressionParser::complete_frame(parser, frame, expression, out.reborrow())?;
            precedence = completed.precedence;
            comparison_seen = completed.comparison_seen;
            postfix = completed.postfix;
            left = Some(completed.expression);
        }
    }

    fn begin_expression<'slot>(
        parser: &mut ParserState<'_>,
        frames: &mut ChargedVec<ExpressionFrame>,
        precedence: u8,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ExpressionStart> {
        let info = ParserControl::peek(parser, out.reborrow())?;
        match info.kind {
            TokenKind::Symbol(Symbol::Bang | Symbol::Minus) => {
                let operator = if info.kind == TokenKind::Symbol(Symbol::Bang) {
                    UnaryOperator::Not
                } else {
                    UnaryOperator::Negate
                };
                ParserControl::enter_prefix_frame(parser, info.span, out.reborrow())?;
                ParserControl::bump(parser, out.reborrow())?;
                ParserControl::try_push(
                    frames,
                    ExpressionFrame::Unary {
                        operator,
                        start: info.span,
                        precedence,
                    },
                    info.span,
                    out.reborrow(),
                )?;
                Ok(ExpressionStart::Nested { precedence: 6 })
            }
            TokenKind::Symbol(Symbol::LeftParen) => {
                ParserControl::bump(parser, out.reborrow())?;
                ParserControl::try_push(
                    frames,
                    ExpressionFrame::Group {
                        start: info.span,
                        precedence,
                    },
                    info.span,
                    out.reborrow(),
                )?;
                Ok(ExpressionStart::Nested { precedence: 1 })
            }
            _ => Ok(ExpressionStart::Ready(
                ExpressionParser::parse_expression_atom(parser, out.reborrow())?,
            )),
        }
    }

    fn parse_postfix<'slot>(
        parser: &mut ParserState<'_>,
        frames: &mut ChargedVec<ExpressionFrame>,
        expression: Id<Expr>,
        precedence: u8,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, PostfixResult> {
        if ParserControl::eat_symbol(parser, Symbol::Dot, out.reborrow())?.is_none() {
            return Ok(PostfixResult::NotApplicable(expression));
        }
        let member = ParserControl::take_name(parser, out.reborrow())?;
        if ParserControl::eat_symbol(parser, Symbol::LeftParen, out.reborrow())?.is_some() {
            if ParserControl::eat_symbol(parser, Symbol::RightParen, out.reborrow())?.is_some() {
                let expression_span = parser.ast.span(expression, out.reborrow())?;
                let span = ParserControl::span_from_start(parser, expression_span, out.reborrow())?;
                let call = ExpressionBuilder::expr_method_call(
                    &mut parser.ast,
                    MethodCallParts {
                        receiver: expression,
                        method: member,
                        argument: None,
                    },
                    span,
                    out.reborrow(),
                )?;
                return Ok(PostfixResult::Ready(call));
            }
            let argument_start = ParserControl::peek(parser, out.reborrow())?.span;
            let parameter =
                if ParserControl::eat_symbol(parser, Symbol::Pipe, out.reborrow())?.is_some() {
                    let parameter = ParserControl::take_name(parser, out.reborrow())?;
                    ParserControl::expect_symbol(parser, Symbol::Pipe, out.reborrow())?;
                    Some(parameter)
                } else {
                    None
                };
            ParserControl::try_push(
                frames,
                ExpressionFrame::Method {
                    receiver: expression,
                    method: member,
                    parameter,
                    argument_start,
                    precedence,
                },
                argument_start,
                out.reborrow(),
            )?;
            return Ok(PostfixResult::Nested { precedence: 1 });
        }
        let expression_span = parser.ast.span(expression, out.reborrow())?;
        let member_span = parser.ast.span(member, out.reborrow())?;
        let span =
            ParserControl::span_between(parser, expression_span, member_span, out.reborrow())?;
        let access = ExpressionBuilder::expr_field_access(
            &mut parser.ast,
            expression,
            member,
            span,
            out.reborrow(),
        )?;
        Ok(PostfixResult::Ready(access))
    }

    fn complete_frame<'slot>(
        parser: &mut ParserState<'_>,
        frame: ExpressionFrame,
        mut expression: Id<Expr>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, CompletedExpression> {
        match frame {
            ExpressionFrame::Unary {
                operator,
                start,
                precedence: parent_precedence,
            } => {
                ParserControl::leave_prefix_frame(parser, out.reborrow())?;
                let expression_span = parser.ast.span(expression, out.reborrow())?;
                let span =
                    ParserControl::span_between(parser, start, expression_span, out.reborrow())?;
                expression = ExpressionBuilder::expr_unary(
                    &mut parser.ast,
                    operator,
                    expression,
                    span,
                    out.reborrow(),
                )?;
                Ok(CompletedExpression {
                    expression,
                    precedence: parent_precedence,
                    comparison_seen: false,
                    postfix: true,
                })
            }
            ExpressionFrame::Group {
                start,
                precedence: parent_precedence,
            } => {
                let end =
                    ParserControl::expect_symbol(parser, Symbol::RightParen, out.reborrow())?.span;
                let span = ParserControl::span_between(parser, start, end, out.reborrow())?;
                expression = parser.ast.expr_set_span(expression, span, out.reborrow())?;
                Ok(CompletedExpression {
                    expression,
                    precedence: parent_precedence,
                    comparison_seen: false,
                    postfix: true,
                })
            }
            ExpressionFrame::Binary {
                left: parent_left,
                operator,
                precedence: parent_precedence,
                comparison_seen: parent_comparison_seen,
            } => {
                let left_span = parser.ast.span(parent_left, out.reborrow())?;
                let right_span = parser.ast.span(expression, out.reborrow())?;
                let span =
                    ParserControl::span_between(parser, left_span, right_span, out.reborrow())?;
                expression = ExpressionBuilder::expr_binary(
                    &mut parser.ast,
                    BinaryExpressionParts {
                        operator,
                        left: parent_left,
                        right: expression,
                    },
                    span,
                    out.reborrow(),
                )?;
                Ok(CompletedExpression {
                    expression,
                    precedence: parent_precedence,
                    comparison_seen: parent_comparison_seen,
                    postfix: false,
                })
            }
            ExpressionFrame::Method {
                receiver,
                method,
                parameter,
                argument_start,
                precedence: parent_precedence,
            } => {
                let expression_span = parser.ast.span(expression, out.reborrow())?;
                let argument_span = ParserControl::span_between(
                    parser,
                    argument_start,
                    expression_span,
                    out.reborrow(),
                )?;
                ParserControl::expect_symbol(parser, Symbol::RightParen, out.reborrow())?;
                let receiver_span = parser.ast.span(receiver, out.reborrow())?;
                let span = ParserControl::span_from_start(parser, receiver_span, out.reborrow())?;
                let argument = if let Some(parameter) = parameter {
                    parser.ast.method_argument_closure(
                        parameter,
                        expression,
                        argument_span,
                        out.reborrow(),
                    )?
                } else {
                    parser.ast.method_argument_expression(
                        expression,
                        argument_span,
                        out.reborrow(),
                    )?
                };
                expression = ExpressionBuilder::expr_method_call(
                    &mut parser.ast,
                    MethodCallParts {
                        receiver,
                        method,
                        argument: Some(argument),
                    },
                    span,
                    out.reborrow(),
                )?;
                Ok(CompletedExpression {
                    expression,
                    precedence: parent_precedence,
                    comparison_seen: false,
                    postfix: true,
                })
            }
        }
    }

    fn parse_expression_atom<'slot>(
        parser: &mut ParserState<'_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<Expr>> {
        let info = ParserControl::peek(parser, out.reborrow())?;
        match info.kind {
            TokenKind::String => {
                let literal = ParserControl::take_string(parser, out.reborrow())?;
                let span = parser.ast.span(literal, out.reborrow())?;
                ExpressionBuilder::expr_literal(
                    &mut parser.ast,
                    ScalarLiteral::String(literal),
                    span,
                    out.reborrow(),
                )
            }
            TokenKind::Integer | TokenKind::Float => {
                let number = ParserControl::take_number(parser, false, false, out.reborrow())?;
                let span = parser.ast.span(number, out.reborrow())?;
                ExpressionBuilder::expr_literal(
                    &mut parser.ast,
                    ScalarLiteral::Number(number),
                    span,
                    out.reborrow(),
                )
            }
            TokenKind::Keyword(Keyword::True | Keyword::False) => {
                let value = info.kind == TokenKind::Keyword(Keyword::True);
                ParserControl::bump(parser, out.reborrow())?;
                let literal = parser
                    .ast
                    .boolean_literal_new(value, info.span, out.reborrow())?;
                ExpressionBuilder::expr_literal(
                    &mut parser.ast,
                    ScalarLiteral::Boolean(literal),
                    info.span,
                    out.reborrow(),
                )
            }
            TokenKind::Keyword(Keyword::None) => {
                ParserControl::bump(parser, out.reborrow())?;
                ExpressionBuilder::expr_none(&mut parser.ast, info.span, out.reborrow())
            }
            TokenKind::Identifier => {
                let name = ParserControl::take_name(parser, out.reborrow())?;
                let span = parser.ast.span(name, out.reborrow())?;
                ExpressionBuilder::expr_name(&mut parser.ast, name, span, out.reborrow())
            }
            _ => ParserControl::syntax_failure(
                parser,
                info.span,
                diagnostic_message!(
                    "diagnostic.syntax.expected_expression",
                    "expected an expression here"
                ),
                out.reborrow(),
            ),
        }
    }
}

fn binary_operator(kind: TokenKind) -> Option<BinaryOperation> {
    let TokenKind::Symbol(symbol) = kind else {
        return None;
    };
    Some(match symbol {
        Symbol::OrOr => BinaryOperation {
            operator: BinaryOperator::Or,
            precedence: 1,
            is_comparison: false,
        },
        Symbol::AndAnd => BinaryOperation {
            operator: BinaryOperator::And,
            precedence: 2,
            is_comparison: false,
        },
        Symbol::EqualEqual => BinaryOperation {
            operator: BinaryOperator::Equal,
            precedence: 3,
            is_comparison: true,
        },
        Symbol::NotEqual => BinaryOperation {
            operator: BinaryOperator::NotEqual,
            precedence: 3,
            is_comparison: true,
        },
        Symbol::Less => BinaryOperation {
            operator: BinaryOperator::Less,
            precedence: 3,
            is_comparison: true,
        },
        Symbol::LessEqual => BinaryOperation {
            operator: BinaryOperator::LessEqual,
            precedence: 3,
            is_comparison: true,
        },
        Symbol::Greater => BinaryOperation {
            operator: BinaryOperator::Greater,
            precedence: 3,
            is_comparison: true,
        },
        Symbol::GreaterEqual => BinaryOperation {
            operator: BinaryOperator::GreaterEqual,
            precedence: 3,
            is_comparison: true,
        },
        Symbol::Plus => BinaryOperation {
            operator: BinaryOperator::Add,
            precedence: 4,
            is_comparison: false,
        },
        Symbol::Minus => BinaryOperation {
            operator: BinaryOperator::Subtract,
            precedence: 4,
            is_comparison: false,
        },
        Symbol::Star => BinaryOperation {
            operator: BinaryOperator::Multiply,
            precedence: 5,
            is_comparison: false,
        },
        Symbol::Slash => BinaryOperation {
            operator: BinaryOperator::Divide,
            precedence: 5,
            is_comparison: false,
        },
        _ => return None,
    })
}
