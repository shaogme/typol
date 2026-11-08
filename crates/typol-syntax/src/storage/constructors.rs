use crate::storage::{
    Declaration, EnumDecl, ExternModDecl, FieldDecl, FieldParams, Id, ModuleDecl, Name, Storage,
    StructDecl, StructDeclParams, TypeAliasDecl, TypeAliasDeclParams, VariantDecl,
    VariantDeclParams,
};
use opeo::{OResult, Out};
use typol_core::{ChargedVec, Diagnostic, SourceSpan};

pub(crate) struct StorageConstructor<'storage> {
    storage: &'storage mut Storage,
}

impl<'storage> StorageConstructor<'storage> {
    pub(crate) fn new(storage: &'storage mut Storage) -> Self {
        Self { storage }
    }

    pub(crate) fn enum_decl_new<'slot>(
        &mut self,
        name: Id<Name>,
        variants: ChargedVec<Id<VariantDecl>>,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<EnumDecl>> {
        let variants = self.storage.store_list(variants, span, out.reborrow())?;
        let value = EnumDecl {
            name,
            variants,
            span,
        };
        self.storage.insert(value, out.reborrow())
    }

    pub(crate) fn extern_mod_decl_new<'slot>(
        &mut self,
        name: Id<Name>,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<ExternModDecl>> {
        self.storage
            .insert(ExternModDecl { name, span }, out.reborrow())
    }

    pub(crate) fn field_decl_new<'slot>(
        &mut self,
        params: FieldParams,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<FieldDecl>> {
        let FieldParams {
            name,
            ty,
            default,
            span,
        } = params;
        self.storage.insert(
            FieldDecl {
                name,
                ty,
                default,
                span,
            },
            out.reborrow(),
        )
    }

    pub(crate) fn module_decl_new<'slot>(
        &mut self,
        name: Id<Name>,
        declarations: ChargedVec<Declaration>,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<ModuleDecl>> {
        let declarations = self
            .storage
            .store_list(declarations, span, out.reborrow())?;
        let value = ModuleDecl {
            name,
            declarations,
            span,
        };
        self.storage.insert(value, out.reborrow())
    }

    pub(crate) fn struct_decl_new<'slot>(
        &mut self,
        params: StructDeclParams,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<StructDecl>> {
        let StructDeclParams {
            name,
            fields,
            assertions,
            span,
        } = params;
        let fields = self.storage.store_list(fields, span, out.reborrow())?;
        let assertions = self.storage.store_list(assertions, span, out.reborrow())?;
        let value = StructDecl {
            name,
            fields,
            assertions,
            span,
        };
        self.storage.insert(value, out.reborrow())
    }

    pub(crate) fn type_alias_decl_new<'slot>(
        &mut self,
        params: TypeAliasDeclParams,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<TypeAliasDecl>> {
        let TypeAliasDeclParams { name, target, span } = params;
        let value = TypeAliasDecl { name, target, span };
        self.storage.insert(value, out.reborrow())
    }

    pub(crate) fn variant_decl_new<'slot>(
        &mut self,
        params: VariantDeclParams,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<VariantDecl>> {
        let VariantDeclParams {
            name,
            payload,
            assertions,
            span,
        } = params;
        let assertions = self.storage.store_list(assertions, span, out.reborrow())?;
        let value = VariantDecl {
            name,
            payload,
            assertions,
            span,
        };
        self.storage.insert(value, out.reborrow())
    }
}
