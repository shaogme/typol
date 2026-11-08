use super::{
    super::ids::{Id, ListId},
    AnnotationData, BraceKey, Declaration, DefaultValueKind, EnumDefaultPayloadKind, ExprKind,
    MethodArgumentKind, Name, NumberLiteral, StringLiteral, TextRef, TypeExprKind,
    VariantPayloadKind,
};
use crate::token::NumberKind;
use typol_core::{ChargedVec, SourceSpan};

#[derive(Debug)]
pub(crate) struct NamedArgument<T> {
    pub(crate) name: Id<Name>,
    pub(crate) value: T,
    pub(crate) span: SourceSpan,
}

#[derive(Debug)]
pub(crate) struct MethodArgument {
    pub(crate) kind: MethodArgumentKind,
    pub(crate) span: SourceSpan,
}

#[derive(Debug)]
pub(crate) struct Expr {
    pub(crate) kind: ExprKind,
    pub(crate) span: SourceSpan,
}

#[derive(Debug)]
pub(crate) struct NamedBounds {
    pub(crate) first: NamedArgument<Id<NumberLiteral>>,
    pub(crate) second: Option<NamedArgument<Id<NumberLiteral>>>,
}

#[derive(Debug)]
pub(crate) struct Annotation {
    pub(crate) data: AnnotationData,
    pub(crate) span: SourceSpan,
}

#[derive(Debug)]
pub(crate) struct Assertion {
    pub(crate) expression: Id<Expr>,
    pub(crate) message: Id<StringLiteral>,
    pub(crate) span: SourceSpan,
}

#[derive(Debug)]
pub(crate) struct DefaultValue {
    pub(crate) kind: DefaultValueKind,
    pub(crate) span: SourceSpan,
}

#[derive(Debug)]
pub(crate) struct ArrayDefault {
    pub(crate) elements: ListId<Id<DefaultValue>>,
    pub(crate) span: SourceSpan,
}

#[derive(Debug)]
pub(crate) struct BraceEntry {
    pub(crate) key: BraceKey,
    pub(crate) value: Id<DefaultValue>,
    pub(crate) span: SourceSpan,
}

#[derive(Debug)]
pub(crate) struct BracedDefault {
    pub(crate) entries: ListId<Id<BraceEntry>>,
    pub(crate) span: SourceSpan,
}

#[derive(Debug)]
pub(crate) struct EnumDefaultPayload {
    pub(crate) kind: EnumDefaultPayloadKind,
    pub(crate) span: SourceSpan,
}

#[derive(Debug)]
pub(crate) struct EnumDefault {
    pub(crate) variant: Id<Name>,
    pub(crate) payload: Id<EnumDefaultPayload>,
    pub(crate) span: SourceSpan,
}

#[derive(Debug)]
pub(crate) struct ArrayLength {
    pub(crate) raw: TextRef,
    pub(crate) span: SourceSpan,
}

#[derive(Debug)]
pub(crate) struct TypePath {
    pub(crate) segments: ListId<Id<Name>>,
    pub(crate) span: SourceSpan,
}

#[derive(Debug)]
pub(crate) struct TypeExpr {
    pub(crate) kind: TypeExprKind,
    pub(crate) annotations: ListId<Id<Annotation>>,
    pub(crate) span: SourceSpan,
}

#[derive(Debug)]
pub(crate) struct ExternModDecl {
    pub(crate) name: Id<Name>,
    pub(crate) span: SourceSpan,
}

#[derive(Debug)]
pub(crate) struct ModuleDecl {
    pub(crate) name: Id<Name>,
    pub(crate) declarations: ListId<Declaration>,
    pub(crate) span: SourceSpan,
}

#[derive(Debug)]
pub(crate) struct TypeAliasDecl {
    pub(crate) name: Id<Name>,
    pub(crate) target: Id<TypeExpr>,
    pub(crate) span: SourceSpan,
}

#[derive(Debug)]
pub(crate) struct FieldDecl {
    pub(crate) name: Id<Name>,
    pub(crate) ty: Id<TypeExpr>,
    pub(crate) default: Option<Id<DefaultValue>>,
    pub(crate) span: SourceSpan,
}

#[derive(Debug)]
pub(crate) struct StructDecl {
    pub(crate) name: Id<Name>,
    pub(crate) fields: ListId<Id<FieldDecl>>,
    pub(crate) assertions: ListId<Id<Assertion>>,
    pub(crate) span: SourceSpan,
}

#[derive(Debug)]
pub(crate) struct SingleVariantPayload {
    pub(crate) ty: Id<TypeExpr>,
}

#[derive(Debug)]
pub(crate) struct StructureVariantPayload {
    pub(crate) fields: ListId<Id<FieldDecl>>,
}

#[derive(Debug)]
pub(crate) struct VariantPayload {
    pub(crate) kind: VariantPayloadKind,
    pub(crate) span: SourceSpan,
}

#[derive(Debug)]
pub(crate) struct VariantDecl {
    pub(crate) name: Id<Name>,
    pub(crate) payload: Id<VariantPayload>,
    pub(crate) assertions: ListId<Id<Assertion>>,
    pub(crate) span: SourceSpan,
}

#[derive(Debug)]
pub(crate) struct EnumDecl {
    pub(crate) name: Id<Name>,
    pub(crate) variants: ListId<Id<VariantDecl>>,
    pub(crate) span: SourceSpan,
}

pub(crate) struct FieldParams {
    pub(crate) name: Id<Name>,
    pub(crate) ty: Id<TypeExpr>,
    pub(crate) default: Option<Id<DefaultValue>>,
    pub(crate) span: SourceSpan,
}

pub(crate) struct NumberLiteralParams<'a> {
    pub(crate) raw: &'a str,
    pub(crate) kind: NumberKind,
    pub(crate) negative: bool,
    pub(crate) span: SourceSpan,
}

pub(crate) struct StructDeclParams {
    pub(crate) name: Id<Name>,
    pub(crate) fields: ChargedVec<Id<FieldDecl>>,
    pub(crate) assertions: ChargedVec<Id<Assertion>>,
    pub(crate) span: SourceSpan,
}

pub(crate) struct TypeAliasDeclParams {
    pub(crate) name: Id<Name>,
    pub(crate) target: Id<TypeExpr>,
    pub(crate) span: SourceSpan,
}

pub(crate) struct VariantDeclParams {
    pub(crate) name: Id<Name>,
    pub(crate) payload: Id<VariantPayload>,
    pub(crate) assertions: ChargedVec<Id<Assertion>>,
    pub(crate) span: SourceSpan,
}
