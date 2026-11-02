pub use crate::storage::{BinaryOperator, UnaryOperator};
use crate::{
    ast::{Name, ScalarLiteral},
    storage::{
        AstAccessError, Expr as StoredExpr, ExprKind as StoredExprKind,
        MethodArgument as StoredMethodArgument, MethodArgumentKind as StoredMethodArgumentKind,
        Storage,
    },
};
use core::fmt::{Debug, Formatter, Result as FmtResult};
use opeo::{OResult, Out};
use typol_core::SourceSpan;

#[derive(Clone, Copy)]
pub struct Expr<'a> {
    storage: &'a Storage,
    record: &'a StoredExpr,
}

#[derive(Clone, Copy)]
pub struct MethodArgument<'a> {
    storage: &'a Storage,
    record: &'a StoredMethodArgument,
}

#[derive(Clone, Copy, Debug)]
pub enum MethodArgumentKind<'a> {
    Expression(Expr<'a>),
    Closure { parameter: Name<'a>, body: Expr<'a> },
}

#[derive(Clone, Copy, Debug)]
pub enum ExprKind<'a> {
    Literal(ScalarLiteral<'a>),
    None,
    Name(Name<'a>),
    FieldAccess {
        receiver: Expr<'a>,
        field: Name<'a>,
    },
    MethodCall {
        receiver: Expr<'a>,
        method: Name<'a>,
        argument: Option<MethodArgument<'a>>,
    },
    Unary {
        operator: UnaryOperator,
        operand: Expr<'a>,
    },
    Binary {
        operator: BinaryOperator,
        left: Expr<'a>,
        right: Expr<'a>,
    },
}

impl<'a> MethodArgumentKind<'a> {
    pub(super) fn project<'slot>(
        storage: &'a Storage,
        record: &'a StoredMethodArgumentKind,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, Self> {
        match record {
            StoredMethodArgumentKind::Expression(value) => {
                let record = storage.get(*value, out.reborrow())?;
                Ok(Self::Expression(Expr::project(storage, record)))
            }
            StoredMethodArgumentKind::Closure { parameter, body } => {
                let parameter = storage.get(*parameter, out.reborrow())?;
                let body = storage.get(*body, out.reborrow())?;
                Ok(Self::Closure {
                    parameter: Name::project(storage, parameter),
                    body: Expr::project(storage, body),
                })
            }
        }
    }
}

impl<'a> ExprKind<'a> {
    pub(super) fn project<'slot>(
        storage: &'a Storage,
        record: &'a StoredExprKind,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, Self> {
        match record {
            StoredExprKind::Literal(value) => {
                ScalarLiteral::project(storage, value, out.reborrow()).map(Self::Literal)
            }
            StoredExprKind::None => Ok(Self::None),
            StoredExprKind::Name(value) => {
                let record = storage.get(*value, out.reborrow())?;
                Ok(Self::Name(Name::project(storage, record)))
            }
            StoredExprKind::FieldAccess { receiver, field } => {
                let receiver = storage.get(*receiver, out.reborrow())?;
                let field = storage.get(*field, out.reborrow())?;
                Ok(Self::FieldAccess {
                    receiver: Expr::project(storage, receiver),
                    field: Name::project(storage, field),
                })
            }
            StoredExprKind::MethodCall {
                receiver,
                method,
                argument,
            } => {
                let argument = if let Some(value) = argument {
                    let record = storage.get(*value, out.reborrow())?;
                    Some(MethodArgument::project(storage, record))
                } else {
                    None
                };
                let receiver = storage.get(*receiver, out.reborrow())?;
                let method = storage.get(*method, out.reborrow())?;
                Ok(Self::MethodCall {
                    receiver: Expr::project(storage, receiver),
                    method: Name::project(storage, method),
                    argument,
                })
            }
            StoredExprKind::Unary { operator, operand } => {
                let operand = storage.get(*operand, out.reborrow())?;
                Ok(Self::Unary {
                    operator: *operator,
                    operand: Expr::project(storage, operand),
                })
            }
            StoredExprKind::Binary {
                operator,
                left,
                right,
            } => {
                let left = storage.get(*left, out.reborrow())?;
                let right = storage.get(*right, out.reborrow())?;
                Ok(Self::Binary {
                    operator: *operator,
                    left: Expr::project(storage, left),
                    right: Expr::project(storage, right),
                })
            }
        }
    }
}

impl Debug for MethodArgument<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct("MethodArgument")
            .field("span", &self.span())
            .finish_non_exhaustive()
    }
}

impl<'a> MethodArgument<'a> {
    pub fn kind<'slot>(
        self,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, MethodArgumentKind<'a>> {
        MethodArgumentKind::project(self.storage, &self.record.kind, out.reborrow())
    }

    pub fn span(self) -> SourceSpan {
        self.record.span
    }
}

impl Debug for Expr<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct("Expr")
            .field("span", &self.span())
            .finish_non_exhaustive()
    }
}

impl<'a> Expr<'a> {
    pub fn kind<'slot>(
        self,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, ExprKind<'a>> {
        ExprKind::project(self.storage, &self.record.kind, out.reborrow())
    }

    pub fn span(self) -> SourceSpan {
        self.record.span
    }
}

impl<'a> MethodArgument<'a> {
    pub(crate) fn project(storage: &'a Storage, record: &'a StoredMethodArgument) -> Self {
        Self { storage, record }
    }
}

impl<'a> Expr<'a> {
    pub(crate) fn project(storage: &'a Storage, record: &'a StoredExpr) -> Self {
        Self { storage, record }
    }
}
