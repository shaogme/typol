use crate::{
    module_tree::ModuleId,
    session::ObjectState,
    symbols::index::{NameIndex, NameMatch},
};
use typol_core::ChargedVec;
use typol_semantic::{FieldId as FieldSymbolId, VariantId as VariantSymbolId};
use typol_syntax::{FieldDecl, VariantDecl};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TypeSymbolId(usize);

impl TypeSymbolId {
    pub(crate) const fn new(index: usize) -> Self {
        Self(index)
    }

    pub(crate) const fn index(self) -> usize {
        self.0
    }
}

pub(crate) type SymbolState = ObjectState;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NamespaceTarget {
    Module(ModuleId),
    Type(TypeSymbolId),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LookupResult<T> {
    Missing,
    Found(T),
    Ambiguous,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FieldOwner {
    Type(TypeSymbolId),
    Variant(VariantSymbolId),
}

pub(crate) struct FieldSymbol<'a> {
    id: FieldSymbolId,
    owner: FieldOwner,
    slot: usize,
    declaration: FieldDecl<'a>,
    state: SymbolState,
}

impl<'a> FieldSymbol<'a> {
    pub(super) fn new(
        id: FieldSymbolId,
        owner: FieldOwner,
        slot: usize,
        declaration: FieldDecl<'a>,
        state: SymbolState,
    ) -> Self {
        Self {
            id,
            owner,
            slot,
            declaration,
            state,
        }
    }

    pub(crate) const fn id(&self) -> FieldSymbolId {
        self.id
    }

    pub(crate) const fn owner(&self) -> FieldOwner {
        self.owner
    }

    pub(crate) const fn slot(&self) -> usize {
        self.slot
    }

    pub(crate) const fn declaration(&self) -> FieldDecl<'a> {
        self.declaration
    }

    pub(crate) const fn state(&self) -> SymbolState {
        self.state
    }

    pub(super) fn set_state(&mut self, state: SymbolState) {
        self.state = state;
    }
}

pub(crate) struct VariantSymbol<'a> {
    id: VariantSymbolId,
    owner: TypeSymbolId,
    declaration: VariantDecl<'a>,
    state: SymbolState,
    fields: ChargedVec<FieldSymbolId>,
    field_index: NameIndex<'a, FieldSymbolId>,
}

pub(super) struct VariantSymbolInput<'a> {
    pub(super) id: VariantSymbolId,
    pub(super) owner: TypeSymbolId,
    pub(super) declaration: VariantDecl<'a>,
    pub(super) state: SymbolState,
    pub(super) fields: ChargedVec<FieldSymbolId>,
    pub(super) field_index: NameIndex<'a, FieldSymbolId>,
}

impl<'a> VariantSymbol<'a> {
    pub(super) fn new(input: VariantSymbolInput<'a>) -> Self {
        Self {
            id: input.id,
            owner: input.owner,
            declaration: input.declaration,
            state: input.state,
            fields: input.fields,
            field_index: input.field_index,
        }
    }

    pub(crate) const fn id(&self) -> VariantSymbolId {
        self.id
    }

    pub(crate) const fn owner(&self) -> TypeSymbolId {
        self.owner
    }

    pub(crate) const fn declaration(&self) -> VariantDecl<'a> {
        self.declaration
    }

    pub(crate) const fn state(&self) -> SymbolState {
        self.state
    }

    pub(crate) fn fields(&self) -> &[FieldSymbolId] {
        &self.fields
    }

    pub(super) fn field_index(&self) -> &NameIndex<'a, FieldSymbolId> {
        &self.field_index
    }

    pub(super) fn set_fields(
        &mut self,
        fields: ChargedVec<FieldSymbolId>,
        field_index: NameIndex<'a, FieldSymbolId>,
    ) {
        self.fields = fields;
        self.field_index = field_index;
    }

    pub(super) fn set_state(&mut self, state: SymbolState) {
        self.state = state;
    }
}

impl<T> From<NameMatch<T>> for LookupResult<T> {
    fn from(value: NameMatch<T>) -> Self {
        match value {
            NameMatch::Missing => Self::Missing,
            NameMatch::Found(target) => Self::Found(target),
            NameMatch::Ambiguous => Self::Ambiguous,
        }
    }
}
