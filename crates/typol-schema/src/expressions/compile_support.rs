use crate::{
    annotations::{CompiledConstraint, ConstraintKind},
    context::CompileContext,
    diagnostics::{BuildFailure, DiagnosticCollector},
    expressions::ExpressionType,
};

use opeo::{OResult, Out};
use typol_core::{ChargedString, ChargedVec, SourceSpan};
use typol_syntax::{
    Annotation, AnnotationData, AnnotationList, BinaryOperator, BuiltinType, Expr, ExprKind,
    ScalarLiteral, StringLiteral, UnaryOperator,
};

#[derive(Clone, Copy)]
pub(super) enum AllBindings {
    Array(ExpressionType),
    Map(ExpressionType),
}

#[derive(Clone, Copy)]
pub(super) struct Context {
    self_type: Option<ExpressionType>,
    root_type: Option<ExpressionType>,
    all_bindings: Option<AllBindings>,
}

impl Context {
    pub(super) const fn new(
        self_type: Option<ExpressionType>,
        root_type: Option<ExpressionType>,
        all_bindings: Option<AllBindings>,
    ) -> Self {
        Self {
            self_type,
            root_type,
            all_bindings,
        }
    }

    pub(super) const fn self_type(self) -> Option<ExpressionType> {
        self.self_type
    }

    pub(super) const fn root_type(self) -> Option<ExpressionType> {
        self.root_type
    }

    pub(super) const fn all_bindings(self) -> Option<AllBindings> {
        self.all_bindings
    }
}

fn all_arguments<'slot, 'a>(
    annotation: Annotation<'a>,
    context: &CompileContext,
    mut out: Out<'slot, '_, BuildFailure>,
) -> OResult<'slot, Option<(Expr<'a>, StringLiteral<'a>)>> {
    context.ast_access(
        out.reborrow(),
        |mut ast_out| {
            let data = annotation.data(ast_out.reborrow())?;
            match data {
                AnnotationData::All(assertion) => {
                    let expression = assertion.expression(ast_out.reborrow())?;
                    let message = assertion.message(ast_out.reborrow())?;
                    Ok(Some((expression, message)))
                }
                _ => Ok(None),
            }
        },
        BuildFailure::from,
    )
}

fn is_arithmetic(operator: BinaryOperator) -> bool {
    matches!(
        operator,
        BinaryOperator::Add
            | BinaryOperator::Subtract
            | BinaryOperator::Multiply
            | BinaryOperator::Divide
    )
}

fn is_contextual_expression<'slot>(
    expression: Expr<'_>,
    context: &CompileContext,
    diagnostics: &mut DiagnosticCollector,
    mut out: Out<'slot, '_, BuildFailure>,
) -> OResult<'slot, bool> {
    match context.ast_access(
        out.reborrow(),
        |ast_out| expression.kind(ast_out),
        BuildFailure::from,
    )? {
        ExprKind::Literal(ScalarLiteral::Number(_)) | ExprKind::None => {
            return Ok(true);
        }
        ExprKind::Unary {
            operator: UnaryOperator::Negate,
            operand,
        } => {
            let operand_kind = context.ast_access(
                out.reborrow(),
                |mut ast_out| {
                    let kind = operand.kind(ast_out.reborrow())?;
                    Ok(kind)
                },
                BuildFailure::from,
            )?;
            return Ok(matches!(
                operand_kind,
                ExprKind::Literal(ScalarLiteral::Number(_))
            ));
        }
        ExprKind::Binary { operator, .. } if is_arithmetic(operator) => {}
        _ => return Ok(false),
    }
    let mut pending = diagnostics.new_vec(context.execution(), out.reborrow())?;
    let span = expression.span();
    pending.try_reserve(1, out.reborrow())?;
    pending.try_push(expression, out.reborrow())?;
    while let Some(expression) = pending.pop() {
        context.consume(1, span, diagnostics, out.reborrow())?;
        let kind = context.ast_access(
            out.reborrow(),
            |ast_out| expression.kind(ast_out),
            BuildFailure::from,
        )?;
        match kind {
            ExprKind::Literal(ScalarLiteral::Number(_)) | ExprKind::None => {}
            ExprKind::Unary {
                operator: UnaryOperator::Negate,
                operand,
            } => {
                let operand_kind = context.ast_access(
                    out.reborrow(),
                    |mut ast_out| {
                        let kind = operand.kind(ast_out.reborrow())?;
                        Ok(kind)
                    },
                    BuildFailure::from,
                )?;
                if !matches!(operand_kind, ExprKind::Literal(ScalarLiteral::Number(_))) {
                    return Ok(false);
                }
            }
            ExprKind::Binary {
                operator,
                left,
                right,
            } if is_arithmetic(operator) => {
                pending.try_reserve(2, out.reborrow())?;
                pending.try_push(right, out.reborrow())?;
                pending.try_push(left, out.reborrow())?;
            }
            _ => return Ok(false),
        }
    }
    Ok(true)
}

fn bool_type() -> ExpressionType {
    ExpressionType::Builtin(BuiltinType::Bool)
}

fn builtin_string() -> ExpressionType {
    ExpressionType::Builtin(BuiltinType::String)
}

fn builtin_u64() -> ExpressionType {
    ExpressionType::Builtin(BuiltinType::U64)
}

fn empty_slots<'slot, T>(
    length: usize,
    context: &CompileContext,
    diagnostics: &mut DiagnosticCollector,
    mut out: Out<'slot, '_, BuildFailure>,
) -> OResult<'slot, ChargedVec<ChargedVec<T>>> {
    let mut slots = diagnostics.new_vec(context.execution(), out.reborrow())?;
    slots.try_reserve_exact(length, out.reborrow())?;
    for _ in 0..length {
        let slot = diagnostics.new_vec(context.execution(), out.reborrow())?;
        slots.try_push(slot, out.reborrow())?;
    }
    Ok(slots)
}

fn copy_text<'slot>(
    value: &str,
    context: &CompileContext,
    mut out: Out<'slot, '_, BuildFailure>,
) -> OResult<'slot, ChargedString> {
    ChargedString::copy(value, Some(context.execution()), out.reborrow())
}

pub(super) struct ExpressionCompileSupport;

pub(super) struct KeywordInput<'a, 'c, 'd> {
    pub(super) name: &'a str,
    pub(super) keywords: &'a [&'a str],
    pub(super) context: &'c CompileContext,
    pub(super) span: SourceSpan,
    pub(super) diagnostics: &'d mut DiagnosticCollector,
}

impl ExpressionCompileSupport {
    pub(super) fn all_rule<'slot, 'a>(
        constraints: Option<&'a [CompiledConstraint]>,
        context: &CompileContext,
        span: SourceSpan,
        sink: &mut DiagnosticCollector,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, Option<&'a CompiledConstraint>> {
        for rule in constraints.unwrap_or(&[]) {
            context.consume(1, span, sink, out.reborrow())?;
            if rule.kind() == ConstraintKind::All {
                return Ok(Some(rule));
            }
        }
        Ok(None)
    }

    pub(super) fn annotation<'slot, 'a>(
        annotations: AnnotationList<'a>,
        span: SourceSpan,
        context: &CompileContext,
        sink: &mut DiagnosticCollector,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, Option<Annotation<'a>>> {
        for index in 0..annotations.len() {
            context.consume(1, span, sink, out.reborrow())?;
            let annotation = context.ast_access(
                out.reborrow(),
                |ast_out| annotations.get(index, ast_out),
                BuildFailure::from,
            )?;
            let Some(annotation) = annotation else {
                continue;
            };
            if annotation.span() == span {
                return Ok(Some(annotation));
            }
        }
        Ok(None)
    }

    pub(super) fn keyword<'slot, 'a, 'c, 'd>(
        input: KeywordInput<'a, 'c, 'd>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, Option<&'a str>> {
        let KeywordInput {
            name,
            keywords,
            context,
            span,
            diagnostics,
        } = input;
        for keyword in keywords {
            context.consume(1, span, diagnostics, out.reborrow())?;
            if context.compare_text(name, keyword, out.reborrow())?.is_eq() {
                return Ok(Some(keyword));
            }
        }
        Ok(None)
    }

    pub(super) fn all_arguments<'slot, 'a>(
        annotation: Annotation<'a>,
        context: &CompileContext,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, Option<(Expr<'a>, StringLiteral<'a>)>> {
        all_arguments(annotation, context, out.reborrow())
    }

    pub(super) fn is_arithmetic(operator: BinaryOperator) -> bool {
        is_arithmetic(operator)
    }

    pub(super) fn is_contextual_expression<'slot>(
        expression: Expr<'_>,
        context: &CompileContext,
        diagnostics: &mut DiagnosticCollector,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, bool> {
        is_contextual_expression(expression, context, diagnostics, out.reborrow())
    }

    pub(super) fn bool_type() -> ExpressionType {
        bool_type()
    }

    pub(super) fn builtin_string() -> ExpressionType {
        builtin_string()
    }

    pub(super) fn builtin_u64() -> ExpressionType {
        builtin_u64()
    }

    pub(super) fn empty_slots<'slot, T>(
        length: usize,
        context: &CompileContext,
        diagnostics: &mut DiagnosticCollector,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ChargedVec<ChargedVec<T>>> {
        empty_slots(length, context, diagnostics, out.reborrow())
    }

    pub(super) fn copy_text<'slot>(
        value: &str,
        context: &CompileContext,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ChargedString> {
        copy_text(value, context, out.reborrow())
    }
}
