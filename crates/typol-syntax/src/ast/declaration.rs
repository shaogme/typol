use crate::{
    ast::{
        AssertionList, DeclarationList, DefaultValue, FieldDeclList, Name, TypeExpr,
        VariantDeclList,
    },
    storage::{
        AstAccessError, Declaration as StoredDeclaration, EnumDecl as StoredEnumDecl,
        ExternModDecl as StoredExternModDecl, FieldDecl as StoredFieldDecl,
        ModuleDecl as StoredModuleDecl, SingleVariantPayload as StoredSingleVariantPayload,
        Storage, StructDecl as StoredStructDecl,
        StructureVariantPayload as StoredStructureVariantPayload,
        TopLevelItem as StoredTopLevelItem, TypeAliasDecl as StoredTypeAliasDecl,
        VariantDecl as StoredVariantDecl, VariantPayload as StoredVariantPayload,
        VariantPayloadKind as StoredVariantPayloadKind,
    },
};
use core::fmt::{Debug, Formatter, Result as FmtResult};
use opeo::{OResult, Out};
use typol_core::SourceSpan;

#[derive(Clone, Copy)]
pub struct ExternModDecl<'a> {
    storage: &'a Storage,
    record: &'a StoredExternModDecl,
}

#[derive(Clone, Copy)]
pub struct ModuleDecl<'a> {
    storage: &'a Storage,
    record: &'a StoredModuleDecl,
}

#[derive(Clone, Copy)]
pub struct TypeAliasDecl<'a> {
    storage: &'a Storage,
    record: &'a StoredTypeAliasDecl,
}

#[derive(Clone, Copy)]
pub struct FieldDecl<'a> {
    storage: &'a Storage,
    record: &'a StoredFieldDecl,
}

#[derive(Clone, Copy)]
pub struct StructDecl<'a> {
    storage: &'a Storage,
    record: &'a StoredStructDecl,
}

#[derive(Clone, Copy)]
pub struct SingleVariantPayload<'a> {
    storage: &'a Storage,
    record: &'a StoredSingleVariantPayload,
}

#[derive(Clone, Copy)]
pub struct StructureVariantPayload<'a> {
    storage: &'a Storage,
    record: &'a StoredStructureVariantPayload,
}

#[derive(Clone, Copy)]
pub struct VariantPayload<'a> {
    storage: &'a Storage,
    record: &'a StoredVariantPayload,
}

#[derive(Clone, Copy)]
pub struct VariantDecl<'a> {
    storage: &'a Storage,
    record: &'a StoredVariantDecl,
}

#[derive(Clone, Copy)]
pub struct EnumDecl<'a> {
    storage: &'a Storage,
    record: &'a StoredEnumDecl,
}

#[derive(Clone, Copy, Debug)]
pub enum Declaration<'a> {
    Module(ModuleDecl<'a>),
    TypeAlias(TypeAliasDecl<'a>),
    Struct(StructDecl<'a>),
    Enum(EnumDecl<'a>),
}

#[derive(Clone, Copy, Debug)]
pub enum TopLevelItem<'a> {
    ExternMod(ExternModDecl<'a>),
    Declaration(Declaration<'a>),
    EntryStruct(StructDecl<'a>),
}

#[derive(Clone, Copy, Debug)]
pub enum VariantPayloadKind<'a> {
    Unit,
    Single(SingleVariantPayload<'a>),
    Structure(StructureVariantPayload<'a>),
}

impl<'a> TopLevelItem<'a> {
    pub(super) fn project<'slot>(
        storage: &'a Storage,
        record: &'a StoredTopLevelItem,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, Self> {
        match record {
            StoredTopLevelItem::ExternMod(value) => {
                let record = storage.get(*value, out.reborrow())?;
                Ok(Self::ExternMod(ExternModDecl::project(storage, record)))
            }
            StoredTopLevelItem::Declaration(value) => {
                Declaration::project(storage, value, out.reborrow()).map(Self::Declaration)
            }
            StoredTopLevelItem::EntryStruct(value) => {
                let record = storage.get(*value, out.reborrow())?;
                Ok(Self::EntryStruct(StructDecl::project(storage, record)))
            }
        }
    }

    pub fn span(self) -> SourceSpan {
        match self {
            Self::ExternMod(item) => item.span(),
            Self::Declaration(item) => item.span(),
            Self::EntryStruct(item) => item.span(),
        }
    }

    pub fn extern_mod(self) -> Option<ExternModDecl<'a>> {
        if let Self::ExternMod(item) = self {
            Some(item)
        } else {
            None
        }
    }

    pub fn declaration(self) -> Option<Declaration<'a>> {
        if let Self::Declaration(item) = self {
            Some(item)
        } else {
            None
        }
    }

    pub fn entry_struct(self) -> Option<StructDecl<'a>> {
        if let Self::EntryStruct(item) = self {
            Some(item)
        } else {
            None
        }
    }
}

impl<'a> Declaration<'a> {
    pub(super) fn project<'slot>(
        storage: &'a Storage,
        record: &'a StoredDeclaration,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, Self> {
        match record {
            StoredDeclaration::Module(value) => {
                let record = storage.get(*value, out.reborrow())?;
                Ok(Self::Module(ModuleDecl::project(storage, record)))
            }
            StoredDeclaration::TypeAlias(value) => {
                let record = storage.get(*value, out.reborrow())?;
                Ok(Self::TypeAlias(TypeAliasDecl::project(storage, record)))
            }
            StoredDeclaration::Struct(value) => {
                let record = storage.get(*value, out.reborrow())?;
                Ok(Self::Struct(StructDecl::project(storage, record)))
            }
            StoredDeclaration::Enum(value) => {
                let record = storage.get(*value, out.reborrow())?;
                Ok(Self::Enum(EnumDecl::project(storage, record)))
            }
        }
    }

    pub fn span(self) -> SourceSpan {
        match self {
            Self::Module(item) => item.span(),
            Self::TypeAlias(item) => item.span(),
            Self::Struct(item) => item.span(),
            Self::Enum(item) => item.span(),
        }
    }
}

impl<'a> VariantPayloadKind<'a> {
    pub(super) fn project<'slot, 'borrow>(
        storage: &'a Storage,
        record: &'a StoredVariantPayloadKind,
        _out: Out<'slot, 'borrow, AstAccessError>,
    ) -> OResult<'slot, Self> {
        match record {
            StoredVariantPayloadKind::Unit => Ok(Self::Unit),
            StoredVariantPayloadKind::Single(value) => {
                Ok(Self::Single(SingleVariantPayload::project(storage, value)))
            }
            StoredVariantPayloadKind::Structure(value) => Ok(Self::Structure(
                StructureVariantPayload::project(storage, value),
            )),
        }
    }
}

impl Debug for ExternModDecl<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct("ExternModDecl")
            .field("span", &self.span())
            .finish_non_exhaustive()
    }
}

impl<'a> ExternModDecl<'a> {
    pub fn name<'slot>(self, mut out: Out<'slot, '_, AstAccessError>) -> OResult<'slot, &'a str> {
        let name = self.storage.name_text(self.record.name, out.reborrow())?;
        Ok(name)
    }

    pub fn name_node<'slot>(
        self,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, Name<'a>> {
        let record = self.storage.get(self.record.name, out.reborrow())?;
        Ok(Name::project(self.storage, record))
    }

    pub fn span(self) -> SourceSpan {
        self.record.span
    }
}

impl Debug for ModuleDecl<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct("ModuleDecl")
            .field("span", &self.span())
            .finish_non_exhaustive()
    }
}

impl<'a> ModuleDecl<'a> {
    pub fn name<'slot>(self, mut out: Out<'slot, '_, AstAccessError>) -> OResult<'slot, &'a str> {
        let name = self.storage.name_text(self.record.name, out.reborrow())?;
        Ok(name)
    }

    pub fn name_node<'slot>(
        self,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, Name<'a>> {
        let record = self.storage.get(self.record.name, out.reborrow())?;
        Ok(Name::project(self.storage, record))
    }

    pub fn declarations<'slot>(
        self,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, DeclarationList<'a>> {
        let records = self
            .storage
            .list(self.record.declarations, out.reborrow())?;
        Ok(DeclarationList::project(self.storage, records))
    }

    pub fn span(self) -> SourceSpan {
        self.record.span
    }
}

impl Debug for TypeAliasDecl<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct("TypeAliasDecl")
            .field("span", &self.span())
            .finish_non_exhaustive()
    }
}

impl<'a> TypeAliasDecl<'a> {
    pub fn name<'slot>(self, mut out: Out<'slot, '_, AstAccessError>) -> OResult<'slot, &'a str> {
        let name = self.storage.name_text(self.record.name, out.reborrow())?;
        Ok(name)
    }

    pub fn name_node<'slot>(
        self,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, Name<'a>> {
        let record = self.storage.get(self.record.name, out.reborrow())?;
        Ok(Name::project(self.storage, record))
    }

    pub fn target<'slot>(
        self,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, TypeExpr<'a>> {
        let record = self.storage.get(self.record.target, out.reborrow())?;
        Ok(TypeExpr::project(self.storage, record))
    }

    pub fn span(self) -> SourceSpan {
        self.record.span
    }
}

impl Debug for StructDecl<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct("StructDecl")
            .field("span", &self.span())
            .finish_non_exhaustive()
    }
}

impl<'a> StructDecl<'a> {
    pub fn name<'slot>(self, mut out: Out<'slot, '_, AstAccessError>) -> OResult<'slot, &'a str> {
        let name = self.storage.name_text(self.record.name, out.reborrow())?;
        Ok(name)
    }

    pub fn name_node<'slot>(
        self,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, Name<'a>> {
        let record = self.storage.get(self.record.name, out.reborrow())?;
        Ok(Name::project(self.storage, record))
    }

    pub fn fields<'slot>(
        self,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, FieldDeclList<'a>> {
        let records = self.storage.list(self.record.fields, out.reborrow())?;
        Ok(FieldDeclList::project(self.storage, records))
    }

    pub fn assertions<'slot>(
        self,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, AssertionList<'a>> {
        let records = self.storage.list(self.record.assertions, out.reborrow())?;
        Ok(AssertionList::project(self.storage, records))
    }

    pub fn span(self) -> SourceSpan {
        self.record.span
    }
}

impl Debug for FieldDecl<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct("FieldDecl")
            .field("span", &self.span())
            .finish_non_exhaustive()
    }
}

impl<'a> FieldDecl<'a> {
    pub fn name<'slot>(self, mut out: Out<'slot, '_, AstAccessError>) -> OResult<'slot, &'a str> {
        let name = self.storage.name_text(self.record.name, out.reborrow())?;
        Ok(name)
    }

    pub fn name_node<'slot>(
        self,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, Name<'a>> {
        let record = self.storage.get(self.record.name, out.reborrow())?;
        Ok(Name::project(self.storage, record))
    }

    pub fn ty<'slot>(
        self,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, TypeExpr<'a>> {
        let record = self.storage.get(self.record.ty, out.reborrow())?;
        Ok(TypeExpr::project(self.storage, record))
    }

    pub fn default<'slot>(
        self,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, Option<DefaultValue<'a>>> {
        match self.record.default {
            Some(value) => {
                let record = self.storage.get(value, out.reborrow())?;
                Ok(Some(DefaultValue::project(self.storage, record)))
            }
            None => Ok(None),
        }
    }

    pub fn span(self) -> SourceSpan {
        self.record.span
    }
}

impl Debug for EnumDecl<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct("EnumDecl")
            .field("span", &self.span())
            .finish_non_exhaustive()
    }
}

impl<'a> EnumDecl<'a> {
    pub fn name<'slot>(self, mut out: Out<'slot, '_, AstAccessError>) -> OResult<'slot, &'a str> {
        let name = self.storage.name_text(self.record.name, out.reborrow())?;
        Ok(name)
    }

    pub fn name_node<'slot>(
        self,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, Name<'a>> {
        let record = self.storage.get(self.record.name, out.reborrow())?;
        Ok(Name::project(self.storage, record))
    }

    pub fn variants<'slot>(
        self,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, VariantDeclList<'a>> {
        let records = self.storage.list(self.record.variants, out.reborrow())?;
        Ok(VariantDeclList::project(self.storage, records))
    }

    pub fn span(self) -> SourceSpan {
        self.record.span
    }
}

impl Debug for VariantDecl<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct("VariantDecl")
            .field("span", &self.span())
            .finish_non_exhaustive()
    }
}

impl<'a> VariantDecl<'a> {
    pub fn name<'slot>(self, mut out: Out<'slot, '_, AstAccessError>) -> OResult<'slot, &'a str> {
        let name = self.storage.name_text(self.record.name, out.reborrow())?;
        Ok(name)
    }

    pub fn name_node<'slot>(
        self,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, Name<'a>> {
        let record = self.storage.get(self.record.name, out.reborrow())?;
        Ok(Name::project(self.storage, record))
    }

    pub fn payload<'slot>(
        self,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, VariantPayload<'a>> {
        let record = self.storage.get(self.record.payload, out.reborrow())?;
        Ok(VariantPayload::project(self.storage, record))
    }

    pub fn assertions<'slot>(
        self,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, AssertionList<'a>> {
        let records = self.storage.list(self.record.assertions, out.reborrow())?;
        Ok(AssertionList::project(self.storage, records))
    }

    pub fn span(self) -> SourceSpan {
        self.record.span
    }
}

impl Debug for VariantPayload<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct("VariantPayload")
            .field("span", &self.span())
            .finish_non_exhaustive()
    }
}

impl<'a> VariantPayload<'a> {
    pub fn kind<'slot>(
        self,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, VariantPayloadKind<'a>> {
        VariantPayloadKind::project(self.storage, &self.record.kind, out.reborrow())
    }

    pub fn span(self) -> SourceSpan {
        self.record.span
    }
}

impl Debug for SingleVariantPayload<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct("SingleVariantPayload")
            .finish_non_exhaustive()
    }
}

impl<'a> SingleVariantPayload<'a> {
    pub(super) fn project(storage: &'a Storage, record: &'a StoredSingleVariantPayload) -> Self {
        Self { storage, record }
    }

    pub fn ty<'slot>(
        self,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, TypeExpr<'a>> {
        let record = self.storage.get(self.record.ty, out.reborrow())?;
        Ok(TypeExpr::project(self.storage, record))
    }
}

impl Debug for StructureVariantPayload<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct("StructureVariantPayload")
            .finish_non_exhaustive()
    }
}

impl<'a> StructureVariantPayload<'a> {
    pub(super) fn project(storage: &'a Storage, record: &'a StoredStructureVariantPayload) -> Self {
        Self { storage, record }
    }

    pub fn fields<'slot>(
        self,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, FieldDeclList<'a>> {
        let records = self.storage.list(self.record.fields, out.reborrow())?;
        Ok(FieldDeclList::project(self.storage, records))
    }
}

impl<'a> ExternModDecl<'a> {
    pub(crate) fn project(storage: &'a Storage, record: &'a StoredExternModDecl) -> Self {
        Self { storage, record }
    }
}

impl<'a> ModuleDecl<'a> {
    pub(crate) fn project(storage: &'a Storage, record: &'a StoredModuleDecl) -> Self {
        Self { storage, record }
    }
}

impl<'a> TypeAliasDecl<'a> {
    pub(crate) fn project(storage: &'a Storage, record: &'a StoredTypeAliasDecl) -> Self {
        Self { storage, record }
    }
}

impl<'a> StructDecl<'a> {
    pub(crate) fn project(storage: &'a Storage, record: &'a StoredStructDecl) -> Self {
        Self { storage, record }
    }
}

impl<'a> FieldDecl<'a> {
    pub(crate) fn project(storage: &'a Storage, record: &'a StoredFieldDecl) -> Self {
        Self { storage, record }
    }
}

impl<'a> EnumDecl<'a> {
    pub(crate) fn project(storage: &'a Storage, record: &'a StoredEnumDecl) -> Self {
        Self { storage, record }
    }
}

impl<'a> VariantDecl<'a> {
    pub(crate) fn project(storage: &'a Storage, record: &'a StoredVariantDecl) -> Self {
        Self { storage, record }
    }
}

impl<'a> VariantPayload<'a> {
    pub(crate) fn project(storage: &'a Storage, record: &'a StoredVariantPayload) -> Self {
        Self { storage, record }
    }
}
