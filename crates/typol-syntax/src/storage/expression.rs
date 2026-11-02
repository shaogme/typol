use crate::storage::{Expr, ExprKind, Id, MethodArgument, Name, ScalarLiteral, Storage};
use opeo::{OResult, Out};
use typol_core::{Diagnostic, SourceSpan};
use typol_model::{BinaryOperator, UnaryOperator};

pub(crate) struct MethodCallParts {
    pub(crate) receiver: Id<Expr>,
    pub(crate) method: Id<Name>,
    pub(crate) argument: Option<Id<MethodArgument>>,
}

pub(crate) struct BinaryExpressionParts {
    pub(crate) operator: BinaryOperator,
    pub(crate) left: Id<Expr>,
    pub(crate) right: Id<Expr>,
}

pub(crate) struct ExpressionBuilder;

impl ExpressionBuilder {
    pub(crate) fn expr_new<'slot>(
        storage: &mut Storage,
        kind: ExprKind,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<Expr>> {
        let value = Expr { kind, span };
        storage.insert(value, out.reborrow())
    }

    pub(crate) fn expr_literal<'slot>(
        storage: &mut Storage,
        value: ScalarLiteral,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<Expr>> {
        Self::expr_new(storage, ExprKind::Literal(value), span, out.reborrow())
    }

    pub(crate) fn expr_none<'slot>(
        storage: &mut Storage,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<Expr>> {
        Self::expr_new(storage, ExprKind::None, span, out.reborrow())
    }

    pub(crate) fn expr_name<'slot>(
        storage: &mut Storage,
        name: Id<Name>,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<Expr>> {
        Self::expr_new(storage, ExprKind::Name(name), span, out.reborrow())
    }

    pub(crate) fn expr_field_access<'slot>(
        storage: &mut Storage,
        receiver: Id<Expr>,
        field: Id<Name>,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<Expr>> {
        Self::expr_new(
            storage,
            ExprKind::FieldAccess { receiver, field },
            span,
            out.reborrow(),
        )
    }

    pub(crate) fn expr_method_call<'slot>(
        storage: &mut Storage,
        parts: MethodCallParts,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<Expr>> {
        let MethodCallParts {
            receiver,
            method,
            argument,
        } = parts;
        Self::expr_new(
            storage,
            ExprKind::MethodCall {
                receiver,
                method,
                argument,
            },
            span,
            out.reborrow(),
        )
    }

    pub(crate) fn expr_unary<'slot>(
        storage: &mut Storage,
        operator: UnaryOperator,
        operand: Id<Expr>,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<Expr>> {
        Self::expr_new(
            storage,
            ExprKind::Unary { operator, operand },
            span,
            out.reborrow(),
        )
    }

    pub(crate) fn expr_binary<'slot>(
        storage: &mut Storage,
        parts: BinaryExpressionParts,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<Expr>> {
        let BinaryExpressionParts {
            operator,
            left,
            right,
        } = parts;
        Self::expr_new(
            storage,
            ExprKind::Binary {
                operator,
                left,
                right,
            },
            span,
            out.reborrow(),
        )
    }
}
