use super::{FieldView, TypeView};
use crate::{
    expressions::{
        ExpressionLiteral, ExpressionMethodInfo, StoredBindingInfo, StoredExpressionArgument,
        StoredExpressionKind, TypedExpression,
    },
    model::{CompiledSchema, SchemaAccessError, TypeId},
};
use core::{fmt, ptr};
use opeo::{OResult, Out, ResultOutExt};
use typol_core::SourceSpan;
use typol_model::{BinaryOperator, UnaryOperator};

/// A read-only expression bound to its owning Schema and program; roots and child nodes are returned as views.
#[derive(Clone, Copy)]
pub struct ExpressionView<'a> {
    schema: &'a CompiledSchema,
    raw: &'a TypedExpression,
}

#[derive(Clone, Copy)]
pub struct ExpressionNodeView<'a> {
    expression: ExpressionView<'a>,
    index: usize,
}

#[derive(Clone, Copy)]
pub struct ClosureView<'a> {
    expression: ExpressionView<'a>,
    local: u32,
}

#[derive(Clone, Copy)]
pub enum ExpressionBindingInfo<'a> {
    SelfValue,
    Root,
    Item,
    Key,
    Value,
    Closure(ClosureView<'a>),
}

#[derive(Clone, Copy)]
pub enum ExpressionArgument<'a> {
    Expression(ExpressionNodeView<'a>),
    Closure {
        parameter: ClosureView<'a>,
        body: ExpressionNodeView<'a>,
    },
}

pub enum ExpressionKind<'a> {
    Literal(ExpressionLiteral<'a>),
    Binding(ExpressionBindingInfo<'a>),
    Field {
        receiver: ExpressionNodeView<'a>,
        field: FieldView<'a>,
    },
    Method {
        receiver: ExpressionNodeView<'a>,
        method: ExpressionMethodInfo,
        argument: Option<ExpressionArgument<'a>>,
    },
    Unary {
        operator: UnaryOperator,
        operand: ExpressionNodeView<'a>,
    },
    Binary {
        operator: BinaryOperator,
        left: ExpressionNodeView<'a>,
        right: ExpressionNodeView<'a>,
    },
}

impl PartialEq for ClosureView<'_> {
    fn eq(&self, other: &Self) -> bool {
        ptr::eq(self.expression.schema, other.expression.schema)
            && ptr::eq(self.expression.raw, other.expression.raw)
            && self.local == other.local
    }
}

impl Eq for ClosureView<'_> {}

impl PartialEq for ExpressionNodeView<'_> {
    fn eq(&self, other: &Self) -> bool {
        ptr::eq(self.expression.schema, other.expression.schema)
            && ptr::eq(self.expression.raw, other.expression.raw)
            && self.index == other.index
    }
}

impl Eq for ExpressionNodeView<'_> {}

impl<'a> ExpressionView<'a> {
    pub(crate) fn schema(self) -> &'a CompiledSchema {
        self.schema
    }

    pub(crate) fn raw(self) -> &'a TypedExpression {
        self.raw
    }

    pub(crate) fn new(schema: &'a CompiledSchema, raw: &'a TypedExpression) -> Self {
        Self { schema, raw }
    }

    pub fn root<'slot>(
        self,
        mut out: Out<'slot, '_, SchemaAccessError>,
    ) -> OResult<'slot, ExpressionNodeView<'a>> {
        self.checked_child(self.raw.root())
            .store_err_into(out.reborrow())
    }

    fn checked_child(self, index: usize) -> Result<ExpressionNodeView<'a>, SchemaAccessError> {
        self.raw
            .node(index)
            .ok_or_else(|| SchemaAccessError::new("expression node", index))?;
        Ok(ExpressionNodeView {
            expression: self,
            index,
        })
    }

    pub fn nodes<'iter, 'slot>(
        self,
        mut out: Out<'slot, 'iter, SchemaAccessError>,
    ) -> impl ExactSizeIterator<Item = OResult<'slot, ExpressionNodeView<'a>>> + 'iter
    where
        'a: 'iter,
        'slot: 'iter,
    {
        (0..self.raw.node_count())
            .map(move |index| self.checked_child(index).store_err_into(out.reborrow()))
    }

    pub fn node_count(self) -> usize {
        self.raw.node_count()
    }

    pub fn ty<'slot>(
        self,
        mut out: Out<'slot, '_, SchemaAccessError>,
    ) -> OResult<'slot, TypeView<'a>> {
        checked_type_view(self.schema, self.raw.ty()).store_err_into(out.reborrow())
    }

    pub fn span(self) -> SourceSpan {
        self.raw.span()
    }
}

impl<'a> ExpressionNodeView<'a> {
    pub fn ty<'slot>(
        self,
        mut out: Out<'slot, '_, SchemaAccessError>,
    ) -> OResult<'slot, TypeView<'a>> {
        let node = self
            .expression
            .raw
            .node(self.index)
            .ok_or_else(|| SchemaAccessError::new("expression node", self.index))
            .store_err_into(out.reborrow())?;
        checked_type_view(self.expression.schema, node.ty()).store_err_into(out.reborrow())
    }

    pub fn span<'slot>(
        self,
        mut out: Out<'slot, '_, SchemaAccessError>,
    ) -> OResult<'slot, SourceSpan> {
        self.expression
            .raw
            .node(self.index)
            .map(|node| node.span())
            .ok_or_else(|| SchemaAccessError::new("expression node", self.index))
            .store_err_into(out.reborrow())
    }

    pub fn kind<'slot>(
        self,
        mut out: Out<'slot, '_, SchemaAccessError>,
    ) -> OResult<'slot, ExpressionKind<'a>> {
        let e = self.expression;
        let node = e
            .raw
            .node(self.index)
            .ok_or_else(|| SchemaAccessError::new("expression node", self.index))
            .store_err_into(out.reborrow())?;
        let kind = match node.kind() {
            StoredExpressionKind::Literal(v) => ExpressionKind::Literal(v),
            StoredExpressionKind::Binding(v) => ExpressionKind::Binding(match v {
                StoredBindingInfo::SelfValue => ExpressionBindingInfo::SelfValue,
                StoredBindingInfo::Root => ExpressionBindingInfo::Root,
                StoredBindingInfo::Item => ExpressionBindingInfo::Item,
                StoredBindingInfo::Key => ExpressionBindingInfo::Key,
                StoredBindingInfo::Value => ExpressionBindingInfo::Value,
                StoredBindingInfo::Closure(local) => ExpressionBindingInfo::Closure(ClosureView {
                    expression: e,
                    local,
                }),
            }),
            StoredExpressionKind::Field { receiver, field } => ExpressionKind::Field {
                receiver: e.checked_child(receiver).store_err_into(out.reborrow())?,
                field: FieldView {
                    schema: e.schema,
                    id: field,
                },
            },
            StoredExpressionKind::Unary { operator, operand } => ExpressionKind::Unary {
                operator,
                operand: e.checked_child(operand).store_err_into(out.reborrow())?,
            },
            StoredExpressionKind::Binary {
                operator,
                left,
                right,
            } => ExpressionKind::Binary {
                operator,
                left: e.checked_child(left).store_err_into(out.reborrow())?,
                right: e.checked_child(right).store_err_into(out.reborrow())?,
            },
            StoredExpressionKind::Method {
                receiver,
                method,
                argument,
            } => ExpressionKind::Method {
                receiver: e.checked_child(receiver).store_err_into(out.reborrow())?,
                method,
                argument: match argument {
                    Some(StoredExpressionArgument::Expression(index)) => {
                        Some(ExpressionArgument::Expression(
                            e.checked_child(index).store_err_into(out.reborrow())?,
                        ))
                    }
                    Some(StoredExpressionArgument::Closure { parameter, body }) => {
                        Some(ExpressionArgument::Closure {
                            parameter: ClosureView {
                                expression: e,
                                local: parameter,
                            },
                            body: e.checked_child(body).store_err_into(out.reborrow())?,
                        })
                    }
                    None => None,
                },
            },
        };
        if let ExpressionKind::Field { field, .. } = &kind {
            e.schema
                .field_info(field.id)
                .and_then(|info| {
                    info.ok_or_else(|| SchemaAccessError::new("field", field.id.index()))
                })
                .store_err_into(out.reborrow())?;
        }
        Ok(kind)
    }
}

fn checked_type_view<'a>(
    schema: &'a CompiledSchema,
    id: TypeId,
) -> Result<TypeView<'a>, SchemaAccessError> {
    schema
        .type_info(id)?
        .ok_or_else(|| SchemaAccessError::new("type", id.index()))?;
    Ok(schema.view(id))
}

impl fmt::Debug for ExpressionNodeView<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let span = self.expression.raw.node(self.index).map(|node| node.span());
        f.debug_struct("ExpressionNodeView")
            .field("span", &span)
            .finish()
    }
}

impl PartialEq for ExpressionView<'_> {
    fn eq(&self, other: &Self) -> bool {
        ptr::eq(self.schema, other.schema) && ptr::eq(self.raw, other.raw)
    }
}

impl Eq for ExpressionView<'_> {}

impl fmt::Debug for ExpressionView<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExpressionView")
            .field("node_count", &self.node_count())
            .field("span", &self.span())
            .finish()
    }
}
