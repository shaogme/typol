mod nodes;

use super::{ids::Id, text::TextRef};
use crate::token::NumberKind;
pub(crate) use nodes::{
    Annotation, ArrayDefault, ArrayLength, Assertion, BraceEntry, BracedDefault, DefaultValue,
    EnumDecl, EnumDefault, EnumDefaultPayload, Expr, ExternModDecl, FieldDecl, FieldParams,
    MethodArgument, ModuleDecl, NamedArgument, NamedBounds, NumberLiteralParams,
    SingleVariantPayload, StructDecl, StructDeclParams, StructureVariantPayload, TypeAliasDecl,
    TypeAliasDeclParams, TypeExpr, TypePath, VariantDecl, VariantDeclParams, VariantPayload,
};
use typol_core::SourceSpan;
use typol_model::{BinaryOperator, BuiltinType, UnaryOperator};

#[derive(Debug)]
pub(crate) struct Name {
    pub(crate) text: TextRef,
    pub(crate) span: SourceSpan,
}

#[derive(Debug)]
pub(crate) struct NumberLiteral {
    pub(crate) raw: TextRef,
    pub(crate) kind: NumberKind,
    pub(crate) negative: bool,
    pub(crate) span: SourceSpan,
}

#[derive(Debug)]
pub(crate) struct StringLiteral {
    pub(crate) value: TextRef,
    pub(crate) span: SourceSpan,
}

#[derive(Debug)]
pub(crate) struct BooleanLiteral {
    pub(crate) value: bool,
    pub(crate) span: SourceSpan,
}

#[derive(Debug)]
pub(crate) enum ScalarLiteral {
    String(Id<StringLiteral>),
    Number(Id<NumberLiteral>),
    Boolean(Id<BooleanLiteral>),
}

#[derive(Debug)]
pub(crate) enum BraceKey {
    Identifier(Id<Name>),
    String(Id<StringLiteral>),
}

#[derive(Debug)]
pub(crate) enum SizeArguments {
    Positional {
        min: Id<NumberLiteral>,
        max: Id<NumberLiteral>,
    },
    Named(NamedBounds),
}

#[derive(Debug)]
pub(crate) enum AnnotationData {
    Min(Id<NumberLiteral>),
    Max(Id<NumberLiteral>),
    Range {
        min: Id<NumberLiteral>,
        max: Id<NumberLiteral>,
    },
    Length {
        min: Id<NumberLiteral>,
        max: Id<NumberLiteral>,
    },
    Regex(Id<StringLiteral>),
    Host,
    Email,
    Cidr(NamedArgument<Id<BooleanLiteral>>),
    Env(Id<StringLiteral>),
    Size(SizeArguments),
    Unique(Option<NamedArgument<Id<StringLiteral>>>),
    All(Id<Assertion>),
    Sum(NamedBounds),
    Secret,
}

#[derive(Debug)]
pub(crate) enum Declaration {
    Module(Id<ModuleDecl>),
    TypeAlias(Id<TypeAliasDecl>),
    Struct(Id<StructDecl>),
    Enum(Id<EnumDecl>),
}

#[derive(Debug)]
pub(crate) enum TopLevelItem {
    ExternMod(Id<ExternModDecl>),
    Declaration(Declaration),
    EntryStruct(Id<StructDecl>),
}

#[derive(Debug)]
pub(crate) enum VariantPayloadKind {
    Unit,
    Single(SingleVariantPayload),
    Structure(StructureVariantPayload),
}

#[derive(Debug)]
pub(crate) enum MethodArgumentKind {
    Expression(Id<Expr>),
    Closure { parameter: Id<Name>, body: Id<Expr> },
}

#[derive(Debug)]
pub(crate) enum ExprKind {
    Literal(ScalarLiteral),
    None,
    Name(Id<Name>),
    FieldAccess {
        receiver: Id<Expr>,
        field: Id<Name>,
    },
    MethodCall {
        receiver: Id<Expr>,
        method: Id<Name>,
        argument: Option<Id<MethodArgument>>,
    },
    Unary {
        operator: UnaryOperator,
        operand: Id<Expr>,
    },
    Binary {
        operator: BinaryOperator,
        left: Id<Expr>,
        right: Id<Expr>,
    },
}

#[derive(Debug)]
pub(crate) enum TypeExprKind {
    Builtin(BuiltinType),
    Option(Id<TypeExpr>),
    Array {
        element: Id<TypeExpr>,
        length: Option<Id<ArrayLength>>,
    },
    Map {
        value: Id<TypeExpr>,
    },
    Path(Id<TypePath>),
}

#[derive(Debug)]
pub(crate) enum EnumDefaultPayloadKind {
    Unit,
    Single(Id<DefaultValue>),
    Structure(Id<BracedDefault>),
}

#[derive(Debug)]
pub(crate) enum DefaultValueKind {
    Scalar(ScalarLiteral),
    None,
    Array(Id<ArrayDefault>),
    Braced(Id<BracedDefault>),
    Enum(Id<EnumDefault>),
}
