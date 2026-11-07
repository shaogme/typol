mod index;
mod model;

pub(crate) use model::{
    FieldOwner, FieldSymbol, LookupResult, NamespaceTarget, SymbolState, TypeSymbolId,
    VariantSymbol,
};

use model::VariantSymbolInput;
use typol_i18n::diagnostic_message;

use opeo::{OResult, Out};
use typol_core::{BudgetError, ChargedVec, SourceSpan};
use typol_syntax::{AstAccessError, FieldDeclList, VariantPayloadKind};

use crate::{
    context::CompileContext,
    diagnostics::{BudgetDiagnostics, BuildFailure, DiagnosticCollector},
    module_tree::{
        ModuleId, ModuleMember, ModuleTree, TypeDeclId, TypeDeclaration, TypeDeclarationKind,
    },
    symbols::index::NameIndex,
};

pub(crate) use typol_semantic::{FieldId as FieldSymbolId, VariantId as VariantSymbolId};

pub(crate) struct TypeSymbol<'a> {
    id: TypeSymbolId,
    declaration_id: TypeDeclId,
    module: ModuleId,
    name: &'a str,
    name_span: SourceSpan,
    declaration: TypeDeclarationKind<'a>,
    state: SymbolState,
    fields: ChargedVec<FieldSymbolId>,
    field_index: NameIndex<'a, FieldSymbolId>,
    variants: ChargedVec<VariantSymbolId>,
    variant_index: NameIndex<'a, VariantSymbolId>,
}

pub(crate) struct SymbolTable<'a> {
    namespaces: ChargedVec<NameIndex<'a, NamespaceTarget>>,
    types: ChargedVec<TypeSymbol<'a>>,
    fields: ChargedVec<FieldSymbol<'a>>,
    variants: ChargedVec<VariantSymbol<'a>>,
    entry_type: Option<TypeSymbolId>,
    context: CompileContext,
}

impl<'a> SymbolTable<'a> {
    pub(crate) fn context(&self) -> &CompileContext {
        &self.context
    }

    pub(crate) fn types(&self) -> &[TypeSymbol<'a>] {
        &self.types
    }

    pub(crate) fn fields(&self) -> &[FieldSymbol<'a>] {
        &self.fields
    }

    pub(crate) fn variants(&self) -> &[VariantSymbol<'a>] {
        &self.variants
    }

    pub(crate) fn type_symbol(&self, id: TypeSymbolId) -> Option<&TypeSymbol<'a>> {
        self.types.get(id.index())
    }

    pub(crate) fn field_symbol(&self, id: FieldSymbolId) -> Option<&FieldSymbol<'a>> {
        self.fields.get(id.index())
    }

    pub(crate) fn variant_symbol(&self, id: VariantSymbolId) -> Option<&VariantSymbol<'a>> {
        self.variants.get(id.index())
    }

    pub(crate) fn lookup_namespace<'slot, E>(
        &self,
        module: ModuleId,
        name: &str,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, LookupResult<NamespaceTarget>>
    where
        E: From<BudgetError>,
    {
        self.context.execution().consume::<E>(1, out.reborrow())?;
        let result = match self.namespaces.get(module.index()) {
            Some(index) => index.lookup::<E>(name, &self.context, out.reborrow())?,
            None => {
                return Err(
                    out.fail(schema_invariant!("symbol-lookup", "module-index-missing").into())
                );
            }
        };
        Ok(result.into())
    }

    pub(crate) fn lookup_field<'slot, E>(
        &self,
        owner: TypeSymbolId,
        name: &str,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, LookupResult<FieldSymbolId>>
    where
        E: From<BudgetError>,
    {
        self.context.execution().consume::<E>(1, out.reborrow())?;
        let result = match self.type_symbol(owner) {
            Some(owner) => owner
                .field_index
                .lookup::<E>(name, &self.context, out.reborrow())?,
            None => {
                return Err(
                    out.fail(schema_invariant!("symbol-lookup", "field-owner-missing").into())
                );
            }
        };
        Ok(result.into())
    }

    pub(crate) fn lookup_variant<'slot, E>(
        &self,
        owner: TypeSymbolId,
        name: &str,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, LookupResult<VariantSymbolId>>
    where
        E: From<BudgetError>,
    {
        self.context.execution().consume::<E>(1, out.reborrow())?;
        let result = match self.type_symbol(owner) {
            Some(owner) => owner
                .variant_index
                .lookup::<E>(name, &self.context, out.reborrow())?,
            None => {
                return Err(
                    out.fail(schema_invariant!("symbol-lookup", "variant-owner-missing").into())
                );
            }
        };
        Ok(result.into())
    }

    pub(crate) fn lookup_variant_field<'slot, E>(
        &self,
        owner: VariantSymbolId,
        name: &str,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, LookupResult<FieldSymbolId>>
    where
        E: From<BudgetError>,
    {
        self.context.execution().consume::<E>(1, out.reborrow())?;
        let result = match self.variant_symbol(owner) {
            Some(owner) => owner
                .field_index()
                .lookup::<E>(name, &self.context, out.reborrow())?,
            None => {
                return Err(out.fail(
                    schema_invariant!("symbol-lookup", "variant-field-owner-missing").into(),
                ));
            }
        };
        Ok(result.into())
    }

    pub(crate) const fn entry_type(&self) -> Option<TypeSymbolId> {
        self.entry_type
    }
}

impl<'a> TypeSymbol<'a> {
    pub(crate) const fn id(&self) -> TypeSymbolId {
        self.id
    }

    pub(crate) const fn declaration_id(&self) -> TypeDeclId {
        self.declaration_id
    }

    pub(crate) const fn module(&self) -> ModuleId {
        self.module
    }

    pub(crate) const fn name(&self) -> &str {
        self.name
    }

    pub(crate) const fn name_span(&self) -> SourceSpan {
        self.name_span
    }

    pub(crate) const fn declaration(&self) -> TypeDeclarationKind<'a> {
        self.declaration
    }

    pub(crate) const fn state(&self) -> SymbolState {
        self.state
    }

    pub(crate) fn fields(&self) -> &[FieldSymbolId] {
        &self.fields
    }

    pub(crate) fn variants(&self) -> &[VariantSymbolId] {
        &self.variants
    }
}

struct SymbolBuilder<'a, 'd> {
    symbols: SymbolTable<'a>,
    diagnostics: &'d mut DiagnosticCollector,
}

impl<'a> SymbolTable<'a> {
    pub(crate) fn build<'slot>(
        tree: &ModuleTree<'a>,
        context: &CompileContext,
        diagnostics: &mut DiagnosticCollector,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, Self> {
        let mut builder = SymbolBuilder {
            symbols: {
                let internal_context = CompileContext::new::<BuildFailure>(
                    *context.limits(),
                    context.regex_limits(),
                    context.execution(),
                    context.regex_execution(),
                    out.reborrow(),
                )?;
                Self {
                    namespaces: diagnostics.new_vec(context.execution(), out.reborrow())?,
                    types: diagnostics.new_vec(context.execution(), out.reborrow())?,
                    fields: diagnostics.new_vec(context.execution(), out.reborrow())?,
                    variants: diagnostics.new_vec(context.execution(), out.reborrow())?,
                    entry_type: None,
                    context: internal_context,
                }
            },
            diagnostics,
        };
        builder.declarations(tree, out.reborrow())?;
        if tree.root_entries().is_empty() {
            builder.diagnostics.push(builder.diagnostics.make_semantic(
                diagnostic_message!(
                    "diagnostic.schema.root_missing_entry",
                    "root schema must declare an entry struct"
                ),
                tree.root_span(),
            ));
        }
        builder.namespaces(tree, out.reborrow())?;
        if let [entry] = tree.root_entries() {
            let entry = TypeSymbolId::new(entry.index());
            builder.consume(tree.root_span(), out.reborrow())?;
            let symbol = builder.diagnostics.require(
                builder.symbols.types.get(entry.index()),
                tree.root_span(),
                out.reborrow(),
            )?;
            if symbol.state == SymbolState::Valid {
                builder.symbols.entry_type = Some(entry);
            }
        }
        Ok(builder.symbols)
    }
}

impl<'a> SymbolBuilder<'a, '_> {
    fn ast_access<'slot, 'borrow, T>(
        &self,
        out: Out<'slot, 'borrow, BuildFailure>,
        operation: impl for<'call> FnOnce(Out<'call, 'call, AstAccessError>) -> OResult<'call, T>,
    ) -> OResult<'slot, T> {
        self.symbols
            .context
            .ast_access(out, operation, BuildFailure::from)
    }

    fn declaration_name<'slot>(
        &self,
        declaration: &TypeDeclaration<'a>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, &'a str> {
        let name = self.ast_access(out.reborrow(), |ast_out| match declaration.kind() {
            TypeDeclarationKind::Alias(item) => item.name(ast_out),
            TypeDeclarationKind::Struct(item) => item.name(ast_out),
            TypeDeclarationKind::Enum(item) => item.name(ast_out),
        })?;
        Ok(name)
    }

    fn consume<'slot>(
        &mut self,
        span: SourceSpan,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ()> {
        self.symbols
            .context
            .consume(1, span, self.diagnostics, out.reborrow())?;
        Ok(())
    }

    fn declarations<'slot>(
        &mut self,
        tree: &ModuleTree<'a>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ()> {
        let mut first_entry = None;
        for declaration in tree.declarations() {
            let declaration_name_span = declaration.name_span();
            self.consume(declaration.name_span(), out.reborrow())?;
            let id = TypeSymbolId::new(self.symbols.types.len());
            if declaration.id().index() != id.index() {
                self.diagnostics
                    .abort(schema_internal!(declaration.name_span()));
                return Err(out.fail(BuildFailure::empty()));
            }
            let fields = self
                .diagnostics
                .new_vec(self.symbols.context.execution(), out.reborrow())?;
            let variants = self
                .diagnostics
                .new_vec(self.symbols.context.execution(), out.reborrow())?;
            let field_index =
                NameIndex::new(&self.symbols.context, self.diagnostics, out.reborrow())?;
            let variant_index =
                NameIndex::new(&self.symbols.context, self.diagnostics, out.reborrow())?;
            let symbol = TypeSymbol {
                id,
                declaration_id: declaration.id(),
                module: declaration.module(),
                name: self.declaration_name(declaration, out.reborrow())?,
                name_span: declaration.name_span(),
                declaration: declaration.kind(),
                state: SymbolState::Valid,
                fields,
                variants,
                field_index,
                variant_index,
            };
            let context = &self.symbols.context;
            let diagnostics = &mut *self.diagnostics;
            let types = &mut self.symbols.types;
            context.budget_access(
                out.reborrow(),
                declaration_name_span,
                diagnostics,
                |budget_out| types.try_push(symbol, budget_out),
            )?;
            if declaration.is_root_entry() {
                if let Some(first_id) = first_entry {
                    self.consume(declaration.name_span(), out.reborrow())?;
                    let first = self.diagnostics.require(
                        tree.declaration(first_id),
                        declaration.name_span(),
                        out.reborrow(),
                    )?;
                    self.diagnostics.push(self.diagnostics.make_duplicate(
                        diagnostic_message!(
                            "diagnostic.schema.duplicate_entry_struct",
                            "root schema can declare only one entry struct"
                        ),
                        declaration.name_span(),
                        first.name_span(),
                        diagnostic_message!(
                            "diagnostic.label.first_entry_struct",
                            "the first entry struct is here"
                        ),
                    ));
                } else {
                    first_entry = Some(declaration.id());
                }
            }
            match declaration.kind() {
                TypeDeclarationKind::Alias(_) => {}
                TypeDeclarationKind::Struct(declaration) => {
                    let fields =
                        self.ast_access(out.reborrow(), |ast_out| declaration.fields(ast_out))?;
                    let (ids, index) = self.fields(
                        fields,
                        FieldOwner::Type(id),
                        declaration_name_span,
                        out.reborrow(),
                    )?;
                    let symbol = self.diagnostics.require(
                        self.symbols.types.get_mut(id.index()),
                        declaration.span(),
                        out.reborrow(),
                    )?;
                    symbol.fields = ids;
                    symbol.field_index = index;
                }
                TypeDeclarationKind::Enum(declaration) => {
                    let variants =
                        self.ast_access(out.reborrow(), |ast_out| declaration.variants(ast_out))?;
                    let mut index =
                        NameIndex::new(&self.symbols.context, self.diagnostics, out.reborrow())?;
                    let first_variant = self.symbols.variants.len();
                    for offset in 0..variants.len() {
                        let Some(variant) = self
                            .ast_access(out.reborrow(), |ast_out| variants.get(offset, ast_out))?
                        else {
                            self.diagnostics.abort(schema_internal!(declaration.span()));
                            return Err(out.fail(BuildFailure::empty()));
                        };
                        self.consume(variant.span(), out.reborrow())?;
                        let Some(target) = first_variant.checked_add(offset) else {
                            let error = self.symbols.context.execution().resource_error();
                            self.diagnostics
                                .abort(BudgetDiagnostics::make(error, variant.span()));
                            return Err(out.fail(BuildFailure::empty()));
                        };
                        let (name, name_span) =
                            self.ast_access(out.reborrow(), |mut ast_out| {
                                let name = variant.name(ast_out.reborrow())?;
                                let name_node = variant.name_node(ast_out.reborrow())?;
                                Ok((name, name_node.span()))
                            })?;
                        index.push::<BuildFailure>(
                            name,
                            name_span,
                            VariantSymbolId::new(target),
                            out.reborrow(),
                        )?;
                    }
                    let duplicates = index.finish(
                        &self.symbols.context,
                        self.diagnostics,
                        declaration_name_span,
                        out.reborrow(),
                    )?;
                    let mut duplicates = duplicates.into_iter().peekable();
                    for offset in 0..variants.len() {
                        let Some(variant) = self
                            .ast_access(out.reborrow(), |ast_out| variants.get(offset, ast_out))?
                        else {
                            self.diagnostics.abort(schema_internal!(declaration.span()));
                            return Err(out.fail(BuildFailure::empty()));
                        };
                        self.consume(variant.span(), out.reborrow())?;
                        let variant_id = VariantSymbolId::new(self.symbols.variants.len());
                        let mut state = SymbolState::Valid;
                        if duplicates
                            .peek()
                            .is_some_and(|duplicate| duplicate.later == variant_id)
                        {
                            let Some(duplicate) = duplicates.next() else {
                                self.diagnostics.abort(schema_internal!(variant.span()));
                                return Err(out.fail(BuildFailure::empty()));
                            };
                            self.consume(duplicate.later_span, out.reborrow())?;
                            self.diagnostics
                                .require(
                                    self.symbols.variants.get_mut(duplicate.first.index()),
                                    duplicate.later_span,
                                    out.reborrow(),
                                )?
                                .set_state(SymbolState::Ambiguous);
                            state = SymbolState::Ambiguous;
                            self.diagnostics.push(self.diagnostics.make_duplicate(
                                diagnostic_message!(
                                    "diagnostic.schema.duplicate_variant_name",
                                    "duplicate variant name in the same enum"
                                ),
                                duplicate.later_span,
                                duplicate.first_span,
                                diagnostic_message!(
                                    "diagnostic.label.earlier_variant",
                                    "the earlier variant is here"
                                ),
                            ));
                        }
                        let owner_symbol = self.diagnostics.require(
                            self.symbols.types.get_mut(id.index()),
                            variant.span(),
                            out.reborrow(),
                        )?;
                        owner_symbol.variants.try_push(variant_id, out.reborrow())?;
                        let fields = self
                            .diagnostics
                            .new_vec(self.symbols.context.execution(), out.reborrow())?;
                        let field_index = NameIndex::new(
                            &self.symbols.context,
                            self.diagnostics,
                            out.reborrow(),
                        )?;
                        let mut symbol = VariantSymbol::new(VariantSymbolInput {
                            id: variant_id,
                            owner: id,
                            declaration: variant,
                            state,
                            fields,
                            field_index,
                        });
                        let payload_kind = self.ast_access(out.reborrow(), |mut ast_out| {
                            let payload = variant.payload(ast_out.reborrow())?;
                            payload.kind(ast_out.reborrow())
                        })?;
                        if let VariantPayloadKind::Structure(payload) = payload_kind {
                            let fields =
                                self.ast_access(out.reborrow(), |ast_out| payload.fields(ast_out))?;
                            let (ids, field_index) = self.fields(
                                fields,
                                FieldOwner::Variant(variant_id),
                                variant.span(),
                                out.reborrow(),
                            )?;
                            symbol.set_fields(ids, field_index);
                        }
                        self.symbols.variants.try_push(symbol, out.reborrow())?;
                    }
                    self.diagnostics
                        .require(
                            self.symbols.types.get_mut(id.index()),
                            declaration.span(),
                            out.reborrow(),
                        )?
                        .variant_index = index;
                }
            }
        }
        Ok(())
    }

    fn fields<'slot>(
        &mut self,
        declarations: FieldDeclList<'a>,
        owner: FieldOwner,
        span: SourceSpan,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, (ChargedVec<FieldSymbolId>, NameIndex<'a, FieldSymbolId>)> {
        let mut ids = self
            .diagnostics
            .new_vec(self.symbols.context.execution(), out.reborrow())?;
        let mut name_index =
            NameIndex::new(&self.symbols.context, self.diagnostics, out.reborrow())?;
        for position in 0..declarations.len() {
            let Some(declaration) = self.ast_access(out.reborrow(), |ast_out| {
                declarations.get(position, ast_out)
            })?
            else {
                self.diagnostics.abort(schema_internal!(span));
                return Err(out.fail(BuildFailure::empty()));
            };
            let (name, span) = self.ast_access(out.reborrow(), |mut ast_out| {
                let name = declaration.name(ast_out.reborrow())?;
                let name_node = declaration.name_node(ast_out.reborrow())?;
                Ok((name, name_node.span()))
            })?;
            self.consume(span, out.reborrow())?;
            let id = FieldSymbolId::new(self.symbols.fields.len());
            name_index.push::<BuildFailure>(name, span, id, out.reborrow())?;
            ids.try_push(id, out.reborrow())?;
            self.symbols.fields.try_push(
                FieldSymbol::new(id, owner, position, declaration, SymbolState::Valid),
                out.reborrow(),
            )?;
        }
        let duplicates = name_index.finish(
            &self.symbols.context,
            self.diagnostics,
            span,
            out.reborrow(),
        )?;
        for duplicate in duplicates {
            self.consume(duplicate.later_span, out.reborrow())?;
            self.diagnostics
                .require(
                    self.symbols.fields.get_mut(duplicate.first.index()),
                    duplicate.later_span,
                    out.reborrow(),
                )?
                .set_state(SymbolState::Ambiguous);
            self.diagnostics
                .require(
                    self.symbols.fields.get_mut(duplicate.later.index()),
                    duplicate.later_span,
                    out.reborrow(),
                )?
                .set_state(SymbolState::Ambiguous);
            self.diagnostics.push(self.diagnostics.make_duplicate(
                diagnostic_message!(
                    "diagnostic.schema.duplicate_field_name",
                    "duplicate field name in the same struct"
                ),
                duplicate.later_span,
                duplicate.first_span,
                diagnostic_message!(
                    "diagnostic.label.earlier_field",
                    "the earlier field is here"
                ),
            ));
        }
        Ok((ids, name_index))
    }

    fn namespaces<'slot>(
        &mut self,
        tree: &ModuleTree<'a>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ()> {
        for module in tree.modules() {
            let module_span = module.name_span().unwrap_or(tree.root_span());
            self.consume(tree.root_span(), out.reborrow())?;
            if module.id().index() != self.symbols.namespaces.len() {
                self.diagnostics.abort(schema_internal!(tree.root_span()));
                return Err(out.fail(BuildFailure::empty()));
            }
            let mut index =
                NameIndex::new(&self.symbols.context, self.diagnostics, out.reborrow())?;
            for member in module.members() {
                self.consume(tree.root_span(), out.reborrow())?;
                match *member {
                    ModuleMember::Module(id) => {
                        let child = self.diagnostics.require(
                            tree.module(id),
                            tree.root_span(),
                            out.reborrow(),
                        )?;
                        let span = self.diagnostics.require(
                            child.name_span(),
                            tree.root_span(),
                            out.reborrow(),
                        )?;
                        let name = self
                            .diagnostics
                            .require(child.name(), span, out.reborrow())?;
                        index.push::<BuildFailure>(
                            name,
                            span,
                            NamespaceTarget::Module(id),
                            out.reborrow(),
                        )?;
                    }
                    ModuleMember::Type(id) => {
                        let declaration = self.diagnostics.require(
                            tree.declaration(id),
                            tree.root_span(),
                            out.reborrow(),
                        )?;
                        let name = self.declaration_name(declaration, out.reborrow())?;
                        index.push::<BuildFailure>(
                            name,
                            declaration.name_span(),
                            NamespaceTarget::Type(TypeSymbolId::new(id.index())),
                            out.reborrow(),
                        )?;
                    }
                }
            }
            let duplicates = index.finish(
                &self.symbols.context,
                self.diagnostics,
                module_span,
                out.reborrow(),
            )?;
            for duplicate in duplicates {
                self.consume(duplicate.later_span, out.reborrow())?;
                for target in [duplicate.first, duplicate.later] {
                    self.consume(duplicate.later_span, out.reborrow())?;
                    if let NamespaceTarget::Type(id) = target {
                        self.diagnostics
                            .require(
                                self.symbols.types.get_mut(id.index()),
                                duplicate.later_span,
                                out.reborrow(),
                            )?
                            .state = SymbolState::Ambiguous;
                    }
                }
                self.diagnostics.push(self.diagnostics.make_duplicate(
                    diagnostic_message!(
                        "diagnostic.schema.duplicate_declaration_name",
                        "duplicate declaration name in the same module"
                    ),
                    duplicate.later_span,
                    duplicate.first_span,
                    diagnostic_message!(
                        "diagnostic.label.earlier_declaration",
                        "the earlier declaration is here"
                    ),
                ));
            }
            self.symbols.namespaces.try_push(index, out.reborrow())?;
        }
        Ok(())
    }
}
