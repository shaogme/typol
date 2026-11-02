use crate::model::{FieldId, SchemaAccessError, TypeId};
use core::slice::Iter;
use typol_core::{ChargedString, ChargedVec, SourceSpan};
use typol_model::{BinaryOperator, UnaryOperator};
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExpressionNodeId(usize);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClosureLocalId(usize);

pub enum TypedLiteral {
    String(ChargedString),
    Boolean(bool),
    Integer(u128),
    Float(f64),
    None,
}

#[derive(Clone, Copy)]
pub enum ExpressionBinding {
    SelfValue,
    Root,
    Item,
    Key,
    Value,
    Closure(ClosureLocalId),
}

#[derive(Clone, Copy)]
pub enum ExpressionMethod {
    Length,
    IsEmpty,
    IsSome,
    IsNone,
    StartsWith,
    IsVariant,
    Any,
    All,
    Sum,
}

pub enum TypedMethodArgument {
    Expression(ExpressionNodeId),
    Closure {
        parameter: ClosureLocalId,
        body: ExpressionNodeId,
    },
}

pub enum TypedExpressionKind {
    Literal(TypedLiteral),
    Binding(ExpressionBinding),
    Field {
        receiver: ExpressionNodeId,
        field: FieldId,
    },
    Method {
        receiver: ExpressionNodeId,
        method: ExpressionMethod,
        argument: Option<TypedMethodArgument>,
    },
    Unary {
        operator: UnaryOperator,
        operand: ExpressionNodeId,
    },
    Binary {
        operator: BinaryOperator,
        left: ExpressionNodeId,
        right: ExpressionNodeId,
    },
}

pub struct TypedExpressionNode {
    kind: TypedExpressionKind,
    ty: TypeId,
    span: SourceSpan,
}

pub struct TypedExpression {
    nodes: ChargedVec<TypedExpressionNode>,
    root: ExpressionNodeId,
    result_type: TypeId,
    span: SourceSpan,
}

pub enum ExpressionLiteral<'a> {
    String(&'a str),
    Boolean(bool),
    Integer(u128),
    Float(f64),
    None,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoredBindingInfo {
    SelfValue,
    Root,
    Item,
    Key,
    Value,
    Closure(u32),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExpressionMethodInfo {
    Length,
    IsEmpty,
    IsSome,
    IsNone,
    StartsWith,
    IsVariant,
    Any,
    All,
    Sum,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoredExpressionArgument {
    Expression(usize),
    Closure { parameter: u32, body: usize },
}

pub enum StoredExpressionKind<'a> {
    Literal(ExpressionLiteral<'a>),
    Binding(StoredBindingInfo),
    Field {
        receiver: usize,
        field: FieldId,
    },
    Method {
        receiver: usize,
        method: ExpressionMethodInfo,
        argument: Option<StoredExpressionArgument>,
    },
    Unary {
        operator: UnaryOperator,
        operand: usize,
    },
    Binary {
        operator: BinaryOperator,
        left: usize,
        right: usize,
    },
}

pub struct AssertionInfo {
    expression: TypedExpression,
    message: ChargedString,
    span: SourceSpan,
}

pub struct StoredNodeInfo<'a> {
    data: &'a TypedExpressionNode,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProgramId(usize);

#[derive(Clone, Copy)]
pub struct AssertionList<'a> {
    programs: &'a [AssertionInfo],
    ids: &'a [ProgramId],
}

#[derive(Clone)]
pub struct AssertionIter<'a> {
    programs: &'a [AssertionInfo],
    ids: Iter<'a, ProgramId>,
}

impl ExpressionNodeId {
    #[cfg(feature = "compiler")]
    pub const fn new(index: usize) -> Self {
        Self(index)
    }

    pub const fn index(self) -> usize {
        self.0
    }
}

impl ClosureLocalId {
    #[cfg(feature = "compiler")]
    pub const fn new(index: usize) -> Self {
        Self(index)
    }

    pub const fn index(self) -> usize {
        self.0
    }
}

impl TypedExpression {
    #[cfg(feature = "compiler")]
    pub fn new(
        nodes: ChargedVec<TypedExpressionNode>,
        root: ExpressionNodeId,
        result_type: TypeId,
        span: SourceSpan,
    ) -> Self {
        Self {
            nodes,
            root,
            result_type,
            span,
        }
    }

    #[cfg(feature = "compiler")]
    pub(crate) fn nodes(&self) -> &[TypedExpressionNode] {
        &self.nodes
    }

    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

impl TypedExpressionNode {
    #[cfg(feature = "compiler")]
    pub fn new(kind: TypedExpressionKind, ty: TypeId, span: SourceSpan) -> Self {
        Self { kind, ty, span }
    }

    pub const fn kind(&self) -> &TypedExpressionKind {
        &self.kind
    }

    pub const fn ty(&self) -> TypeId {
        self.ty
    }

    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

impl TypedExpression {
    pub fn ty(&self) -> TypeId {
        self.result_type
    }

    pub(crate) fn root(&self) -> usize {
        self.root.index()
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub(crate) fn node(&self, index: usize) -> Option<StoredNodeInfo<'_>> {
        self.nodes.get(index).map(|data| StoredNodeInfo { data })
    }
}

impl<'a> StoredNodeInfo<'a> {
    pub fn kind(&self) -> StoredExpressionKind<'a> {
        expression_node_kind(self.data)
    }

    pub fn ty(&self) -> TypeId {
        self.data.ty()
    }

    pub fn span(&self) -> SourceSpan {
        self.data.span()
    }
}

fn expression_node_kind(node: &TypedExpressionNode) -> StoredExpressionKind<'_> {
    match node.kind() {
        TypedExpressionKind::Literal(TypedLiteral::String(value)) => {
            StoredExpressionKind::Literal(ExpressionLiteral::String(value.as_str()))
        }
        TypedExpressionKind::Literal(TypedLiteral::Boolean(value)) => {
            StoredExpressionKind::Literal(ExpressionLiteral::Boolean(*value))
        }
        TypedExpressionKind::Literal(TypedLiteral::Integer(value)) => {
            StoredExpressionKind::Literal(ExpressionLiteral::Integer(*value))
        }
        TypedExpressionKind::Literal(TypedLiteral::Float(value)) => {
            StoredExpressionKind::Literal(ExpressionLiteral::Float(*value))
        }
        TypedExpressionKind::Literal(TypedLiteral::None) => {
            StoredExpressionKind::Literal(ExpressionLiteral::None)
        }
        TypedExpressionKind::Binding(binding) => StoredExpressionKind::Binding(match binding {
            ExpressionBinding::SelfValue => StoredBindingInfo::SelfValue,
            ExpressionBinding::Root => StoredBindingInfo::Root,
            ExpressionBinding::Item => StoredBindingInfo::Item,
            ExpressionBinding::Key => StoredBindingInfo::Key,
            ExpressionBinding::Value => StoredBindingInfo::Value,
            ExpressionBinding::Closure(id) => StoredBindingInfo::Closure(id.index() as u32),
        }),
        TypedExpressionKind::Field { receiver, field } => StoredExpressionKind::Field {
            receiver: receiver.index(),
            field: *field,
        },
        TypedExpressionKind::Method {
            receiver,
            method,
            argument,
        } => StoredExpressionKind::Method {
            receiver: receiver.index(),
            method: match method {
                ExpressionMethod::Length => ExpressionMethodInfo::Length,
                ExpressionMethod::IsEmpty => ExpressionMethodInfo::IsEmpty,
                ExpressionMethod::IsSome => ExpressionMethodInfo::IsSome,
                ExpressionMethod::IsNone => ExpressionMethodInfo::IsNone,
                ExpressionMethod::StartsWith => ExpressionMethodInfo::StartsWith,
                ExpressionMethod::IsVariant => ExpressionMethodInfo::IsVariant,
                ExpressionMethod::Any => ExpressionMethodInfo::Any,
                ExpressionMethod::All => ExpressionMethodInfo::All,
                ExpressionMethod::Sum => ExpressionMethodInfo::Sum,
            },
            argument: argument.as_ref().map(|a| match a {
                TypedMethodArgument::Expression(id) => {
                    StoredExpressionArgument::Expression(id.index())
                }
                TypedMethodArgument::Closure { parameter, body } => {
                    StoredExpressionArgument::Closure {
                        parameter: parameter.index() as u32,
                        body: body.index(),
                    }
                }
            }),
        },
        TypedExpressionKind::Unary { operator, operand } => StoredExpressionKind::Unary {
            operator: *operator,
            operand: operand.index(),
        },
        TypedExpressionKind::Binary {
            operator,
            left,
            right,
        } => StoredExpressionKind::Binary {
            operator: *operator,
            left: left.index(),
            right: right.index(),
        },
    }
}

impl AssertionInfo {
    #[cfg(feature = "compiler")]
    pub fn new(expression: TypedExpression, message: ChargedString, span: SourceSpan) -> Self {
        Self {
            expression,
            message,
            span,
        }
    }

    pub fn expression(&self) -> &TypedExpression {
        &self.expression
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn span(&self) -> SourceSpan {
        self.span
    }
}

impl ProgramId {
    pub const fn index(self) -> usize {
        self.0
    }

    #[cfg(feature = "compiler")]
    pub const fn new(index: usize) -> Self {
        Self(index)
    }
}

impl<'a> AssertionList<'a> {
    pub(crate) fn new(programs: &'a [AssertionInfo], ids: &'a [ProgramId]) -> Self {
        Self { programs, ids }
    }

    pub fn get(self, index: usize) -> Result<Option<&'a AssertionInfo>, SchemaAccessError> {
        let Some(id) = self.ids.get(index) else {
            return Ok(None);
        };
        self.programs
            .get(id.index())
            .map(Some)
            .ok_or_else(|| SchemaAccessError::new("program", id.index()))
    }

    pub fn len(self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(self) -> bool {
        self.ids.is_empty()
    }

    pub fn iter(self) -> AssertionIter<'a> {
        AssertionIter {
            programs: self.programs,
            ids: self.ids.iter(),
        }
    }
}

impl<'a> IntoIterator for AssertionList<'a> {
    type Item = Result<&'a AssertionInfo, SchemaAccessError>;
    type IntoIter = AssertionIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a> Iterator for AssertionIter<'a> {
    type Item = Result<&'a AssertionInfo, SchemaAccessError>;

    fn next(&mut self) -> Option<Self::Item> {
        self.ids.next().map(|id| {
            self.programs
                .get(id.index())
                .ok_or_else(|| SchemaAccessError::new("program", id.index()))
        })
    }
}
