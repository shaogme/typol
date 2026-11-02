use crate::storage::{
    Storage,
    height::Height,
    records::{
        Annotation, ArrayDefault, ArrayLength, Assertion, BooleanLiteral, BraceEntry,
        BracedDefault, Declaration, DefaultValue, EnumDecl, EnumDefault, EnumDefaultPayload, Expr,
        ExternModDecl, FieldDecl, MethodArgument, ModuleDecl, Name, NumberLiteral, StringLiteral,
        StructDecl, TypeAliasDecl, TypeExpr, TypePath, VariantDecl, VariantPayload,
    },
};
use core::{
    fmt::{Debug, Formatter, Result as FmtResult},
    marker::PhantomData,
};
use opeo::{OResult, Out};
use typol_core::{ChargedVec, Diagnostic, SourceSpan};

/// An ID stores only a table index; only the storage module can create one, so IDs cannot be forged externally or resolved across trees.
pub(crate) struct Id<T> {
    index: usize,
    marker: PhantomData<fn() -> T>,
}

impl<T> Copy for Id<T> {}

impl<T> Clone for Id<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Debug for Id<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct("NodeId").finish_non_exhaustive()
    }
}

impl<T> Id<T> {
    pub(crate) const fn new(index: usize) -> Self {
        Self {
            index,
            marker: PhantomData,
        }
    }

    pub(crate) const fn index(self) -> usize {
        self.index
    }
}

pub(crate) struct Stored<T> {
    pub(crate) value: T,
    pub(crate) height: usize,
}

pub(crate) trait Node {
    type Value;

    fn table(storage: &Storage) -> &ChargedVec<Stored<Self::Value>>;

    fn table_mut(storage: &mut Storage) -> &mut ChargedVec<Stored<Self::Value>>;

    fn span(value: &Self::Value) -> SourceSpan;

    fn children<'slot, 'borrow>(
        value: &Self::Value,
        storage: &Storage,
        out: Out<'slot, 'borrow, Diagnostic>,
    ) -> OResult<'slot, Option<usize>>;
}

macro_rules! node_table {
    ($($ty:ident => $table:ident),+ $(,)?) => {
        $(impl Node for Id<$ty> {
            type Value = $ty;
            fn table(storage: &Storage) -> &ChargedVec<Stored<Self::Value>> { &storage.$table }
            fn table_mut(storage: &mut Storage) -> &mut ChargedVec<Stored<Self::Value>> { &mut storage.$table }
            fn span(value: &$ty) -> SourceSpan { value.span }
            fn children<'slot, 'borrow>(
                value: &$ty,
                storage: &Storage,
                out: Out<'slot, 'borrow, Diagnostic>,
            ) -> OResult<'slot, Option<usize>> {
                Height::children(value, storage, out)
            }
        })+
    };
}

/// An ordered child range, like a node ID, is resolved only within its owning tree.
pub(crate) struct ListId<T> {
    start: usize,
    end: usize,
    marker: PhantomData<fn() -> T>,
}

impl<T> Copy for ListId<T> {}

impl<T> Clone for ListId<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Debug for ListId<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct("ListId").finish_non_exhaustive()
    }
}

impl<T> ListId<T> {
    pub(crate) const fn new(start: usize, end: usize) -> Self {
        Self {
            start,
            end,
            marker: PhantomData,
        }
    }

    pub(crate) const fn start(self) -> usize {
        self.start
    }

    pub(crate) const fn end(self) -> usize {
        self.end
    }
}

pub(crate) trait Edge {
    type Value;

    fn table(storage: &Storage) -> &ChargedVec<Self::Value>;

    fn table_mut(storage: &mut Storage) -> &mut ChargedVec<Self::Value>;
}

macro_rules! edge_table {
    ($($ty:ty => $table:ident),+ $(,)?) => {
        $(impl Edge for ListId<$ty> {
            type Value = $ty;
            fn table(storage: &Storage) -> &ChargedVec<Self::Value> { &storage.$table }
            fn table_mut(storage: &mut Storage) -> &mut ChargedVec<Self::Value> { &mut storage.$table }
        })+
    };
}

node_table! {
    Annotation => annotation,
    ArrayDefault => array_default,
    ArrayLength => array_length,
    Assertion => assertion,
    BooleanLiteral => boolean_literal,
    BraceEntry => brace_entry,
    BracedDefault => braced_default,
    DefaultValue => default_value,
    EnumDecl => enum_decl,
    EnumDefault => enum_default,
    EnumDefaultPayload => enum_default_payload,
    Expr => expr,
    ExternModDecl => extern_mod_decl,
    FieldDecl => field_decl,
    MethodArgument => method_argument,
    ModuleDecl => module_decl,
    Name => name,
    NumberLiteral => number_literal,
    StringLiteral => string_literal,
    StructDecl => struct_decl,
    TypeAliasDecl => type_alias_decl,
    TypeExpr => type_expr,
    TypePath => type_path,
    VariantDecl => variant_decl,
    VariantPayload => variant_payload
}

edge_table! {
    Id<Annotation> => annotation_edges,
    Id<Assertion> => assertion_edges,
    Id<BraceEntry> => brace_entry_edges,
    Declaration => declaration_edges,
    Id<DefaultValue> => default_value_edges,
    Id<FieldDecl> => field_decl_edges,
    Id<Name> => name_edges,
    Id<VariantDecl> => variant_decl_edges
}
