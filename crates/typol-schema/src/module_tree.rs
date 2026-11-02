use opeo::{OResult, Out};
use typol_core::{ChargedVec, SourceId, SourceSpan, Span};
use typol_syntax::{
    AstAccessError, Declaration, DeclarationList, EnumDecl, StructDecl, TypeAliasDecl,
};

use crate::{
    context::CompileContext,
    diagnostics::{BuildFailure, DiagnosticCollector, EmergencyDiagnostics},
    loader::{ImportEdge, LoadedFile},
    session::{ObjectState, SourceCatalog},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ModuleId(usize);

impl ModuleId {
    pub(crate) const fn index(self) -> usize {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TypeDeclId(usize);

impl TypeDeclId {
    pub(crate) const fn index(self) -> usize {
        self.0
    }
}

#[derive(Clone, Copy)]
pub(crate) enum TypeDeclarationKind<'a> {
    Alias(TypeAliasDecl<'a>),
    Struct(StructDecl<'a>),
    Enum(EnumDecl<'a>),
}

pub(crate) struct TypeDeclaration<'a> {
    id: TypeDeclId,
    module: ModuleId,
    source_id: SourceId,
    is_root_entry: bool,
    name_span: SourceSpan,
    kind: TypeDeclarationKind<'a>,
}

#[derive(Clone, Copy)]
pub(crate) enum ModuleMember {
    Module(ModuleId),
    Type(TypeDeclId),
}

pub(crate) struct ModuleNode<'a> {
    id: ModuleId,
    parent: Option<ModuleId>,
    name: Option<&'a str>,
    name_span: Option<SourceSpan>,
    source_id: SourceId,
    members: ChargedVec<ModuleMember>,
    state: ObjectState,
}

pub(crate) struct ModuleTree<'a> {
    modules: ChargedVec<ModuleNode<'a>>,
    declarations: ChargedVec<TypeDeclaration<'a>>,
    root_entries: ChargedVec<TypeDeclId>,
    root_span: SourceSpan,
}

impl<'a> TypeDeclaration<'a> {
    pub(crate) const fn id(&self) -> TypeDeclId {
        self.id
    }

    pub(crate) const fn module(&self) -> ModuleId {
        self.module
    }

    pub(crate) const fn source_id(&self) -> SourceId {
        self.source_id
    }

    pub(crate) const fn is_root_entry(&self) -> bool {
        self.is_root_entry
    }

    pub(crate) fn name_span(&self) -> SourceSpan {
        self.name_span
    }

    pub(crate) fn span(&self) -> SourceSpan {
        match self.kind {
            TypeDeclarationKind::Alias(declaration) => declaration.span(),
            TypeDeclarationKind::Struct(declaration) => declaration.span(),
            TypeDeclarationKind::Enum(declaration) => declaration.span(),
        }
    }

    pub(crate) const fn kind(&self) -> TypeDeclarationKind<'a> {
        self.kind
    }
}

impl<'a> ModuleTree<'a> {
    pub(crate) fn modules(&self) -> &[ModuleNode<'a>] {
        &self.modules
    }

    pub(crate) fn module(&self, id: ModuleId) -> Option<&ModuleNode<'a>> {
        self.modules.get(id.index())
    }

    pub(crate) fn declarations(&self) -> &[TypeDeclaration<'a>] {
        &self.declarations
    }

    pub(crate) fn declaration(&self, id: TypeDeclId) -> Option<&TypeDeclaration<'a>> {
        self.declarations.get(id.index())
    }

    pub(crate) fn root_entries(&self) -> &[TypeDeclId] {
        &self.root_entries
    }

    pub(crate) const fn root_span(&self) -> SourceSpan {
        self.root_span
    }
}

impl<'a> ModuleNode<'a> {
    pub(crate) const fn state(&self) -> ObjectState {
        self.state
    }

    pub(crate) const fn id(&self) -> ModuleId {
        self.id
    }

    pub(crate) const fn parent(&self) -> Option<ModuleId> {
        self.parent
    }

    pub(crate) const fn name(&self) -> Option<&'a str> {
        self.name
    }

    pub(crate) const fn name_span(&self) -> Option<SourceSpan> {
        self.name_span
    }

    pub(crate) const fn source_id(&self) -> SourceId {
        self.source_id
    }

    pub(crate) fn members(&self) -> &[ModuleMember] {
        &self.members
    }
}

#[derive(Clone, Copy)]
enum FrameItems<'a> {
    File(&'a LoadedFile),
    Declarations(DeclarationList<'a>),
}

#[derive(Clone, Copy)]
struct Frame<'a> {
    items: FrameItems<'a>,
    module: ModuleId,
    depth: usize,
    is_root_file: bool,
    next_index: usize,
    next_import_index: usize,
}

struct ModuleDeclarationInfo<'a> {
    name: &'a str,
    name_span: SourceSpan,
    declarations: DeclarationList<'a>,
}

struct TreeBuilder<'a, 'd> {
    modules: ChargedVec<ModuleNode<'a>>,
    declarations: ChargedVec<TypeDeclaration<'a>>,
    root_entries: ChargedVec<TypeDeclId>,
    frames: ChargedVec<Frame<'a>>,
    context: &'d CompileContext,
    diagnostics: &'d mut DiagnosticCollector,
}

struct ModuleInput<'a> {
    parent: ModuleId,
    name: &'a str,
    name_span: SourceSpan,
    source_id: SourceId,
}

struct TypePushInput<'a> {
    kind: TypeDeclarationKind<'a>,
    frame: Frame<'a>,
    source_id: SourceId,
    is_root_entry: bool,
}

impl<'a> ModuleTree<'a> {
    pub(crate) fn build<'slot>(
        loaded: &'a SourceCatalog,
        context: &CompileContext,
        diagnostics: &mut DiagnosticCollector,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, Self> {
        let empty = SourceSpan::new(SourceId::new(0), Span::empty());
        context
            .execution()
            .consume::<BuildFailure>(1, out.reborrow())?;
        let Some(root_file) = loaded.files().first() else {
            let span = SourceSpan::new(SourceId::new(0), Span::empty());
            diagnostics.push(schema_internal!(span));
            return Err(out.fail(BuildFailure::empty()));
        };
        let Some(root_ast) = root_file.ast() else {
            diagnostics.abort(schema_internal!(empty));
            return Err(out.fail(BuildFailure::empty()));
        };
        let root_span = root_ast.span();
        let execution = context.execution();
        let root_members = diagnostics.new_vec(execution, out.reborrow())?;
        let mut builder = TreeBuilder {
            modules: diagnostics.new_vec(execution, out.reborrow())?,
            declarations: diagnostics.new_vec(execution, out.reborrow())?,
            root_entries: diagnostics.new_vec(execution, out.reborrow())?,
            frames: diagnostics.new_vec(execution, out.reborrow())?,
            context,
            diagnostics,
        };
        let root_module = ModuleNode {
            id: ModuleId(0),
            parent: None,
            name: None,
            name_span: None,
            source_id: root_file.source_id(),
            members: root_members,
            state: ObjectState::Valid,
        };
        let build_context = builder.context;
        let diagnostics = &mut *builder.diagnostics;
        let modules = &mut builder.modules;
        build_context.budget_access(out.reborrow(), root_span, diagnostics, |budget_out| {
            modules.try_push(root_module, budget_out)
        })?;
        let root_frame = Frame {
            items: FrameItems::File(root_file),
            module: ModuleId(0),
            depth: 0,
            is_root_file: true,
            next_index: 0,
            next_import_index: 0,
        };
        let diagnostics = &mut *builder.diagnostics;
        let frames = &mut builder.frames;
        build_context.budget_access(out.reborrow(), root_span, diagnostics, |budget_out| {
            frames.try_push(root_frame, budget_out)
        })?;
        builder.walk(loaded, root_span, out.reborrow())?;
        builder.normalize(root_span, out.reborrow())?;
        Ok(Self {
            modules: builder.modules,
            declarations: builder.declarations,
            root_entries: builder.root_entries,
            root_span,
        })
    }
}

impl<'a> TreeBuilder<'a, '_> {
    fn ast_access<'slot, 'borrow, T>(
        &self,
        out: Out<'slot, 'borrow, BuildFailure>,
        operation: impl for<'call> FnOnce(Out<'call, 'call, AstAccessError>) -> OResult<'call, T>,
    ) -> OResult<'slot, T> {
        self.context.ast_access(out, operation, BuildFailure::from)
    }

    fn walk<'slot>(
        &mut self,
        loaded: &'a SourceCatalog,
        span: SourceSpan,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ()> {
        while let Some(frame) = self.frames.last().copied() {
            self.consume(span, out.reborrow())?;
            if self.diagnostics.is_stopped() {
                return Err(out.fail(BuildFailure::empty()));
            }
            let frame_index = self.frames.len() - 1;
            match frame.items {
                FrameItems::File(file) => {
                    let Some(ast) = file.ast() else {
                        self.diagnostics.abort(schema_internal!(span));
                        return Err(out.fail(BuildFailure::empty()));
                    };
                    let Some(item) = self.ast_access(out.reborrow(), |ast_out| {
                        ast.items().get(frame.next_index, ast_out)
                    })?
                    else {
                        self.frames.pop();
                        continue;
                    };
                    self.diagnostics
                        .require(self.frames.get_mut(frame_index), span, out.reborrow())?
                        .next_index += 1;
                    if let Some(extern_mod) = item.extern_mod() {
                        self.consume(extern_mod.span(), out.reborrow())?;
                        let Some(edge) = file.import_targets().get(frame.next_import_index) else {
                            self.diagnostics.push(schema_internal!(extern_mod.span()));
                            return Err(out.fail(BuildFailure::empty()));
                        };
                        let (target, is_resolved) = match *edge {
                            ImportEdge::Pending => {
                                self.diagnostics.push(schema_internal!(extern_mod.span()));
                                return Err(out.fail(BuildFailure::empty()));
                            }
                            ImportEdge::Resolved { target } => {
                                let Some(file) = loaded.files().get(target) else {
                                    self.diagnostics.push(schema_internal!(extern_mod.span()));
                                    return Err(out.fail(BuildFailure::empty()));
                                };
                                if file.parsed_ast().is_none() {
                                    self.diagnostics.push(schema_internal!(extern_mod.span()));
                                    return Err(out.fail(BuildFailure::empty()));
                                }
                                (Some(file), true)
                            }
                            ImportEdge::Failed { source } => {
                                let file = match source {
                                    Some(index) => match loaded.files().get(index) {
                                        Some(file) => Some(file),
                                        None => {
                                            self.diagnostics
                                                .push(schema_internal!(extern_mod.span()));
                                            return Err(out.fail(BuildFailure::empty()));
                                        }
                                    },
                                    None => None,
                                };
                                (file, false)
                            }
                        };
                        self.diagnostics
                            .require(
                                self.frames.get_mut(frame_index),
                                extern_mod.span(),
                                out.reborrow(),
                            )?
                            .next_import_index += 1;
                        let (name, name_span) =
                            self.ast_access(out.reborrow(), |mut ast_out| {
                                let name = extern_mod.name(ast_out.reborrow())?;
                                let name_node = extern_mod.name_node(ast_out.reborrow())?;
                                Ok((name, name_node.span()))
                            })?;
                        let depth =
                            self.next_depth(frame.depth, extern_mod.span(), out.reborrow())?;
                        let child = self.push_module(
                            ModuleInput {
                                parent: frame.module,
                                name,
                                name_span,
                                source_id: target.map_or(file.source_id(), LoadedFile::source_id),
                            },
                            out.reborrow(),
                        )?;
                        if !is_resolved {
                            self.diagnostics
                                .require(
                                    self.modules.get_mut(child.index()),
                                    extern_mod.span(),
                                    out.reborrow(),
                                )?
                                .state = ObjectState::Invalid;
                            continue;
                        }
                        let Some(target) = target else {
                            self.diagnostics.push(schema_internal!(extern_mod.span()));
                            return Err(out.fail(BuildFailure::empty()));
                        };
                        self.frames.try_push(
                            Frame {
                                items: FrameItems::File(target),
                                module: child,
                                depth,
                                is_root_file: false,
                                next_index: 0,
                                next_import_index: 0,
                            },
                            out.reborrow(),
                        )?;
                    } else if let Some(declaration) = item.declaration() {
                        self.push_declaration(
                            declaration,
                            frame,
                            file.source_id(),
                            out.reborrow(),
                        )?;
                    } else if let Some(entry) = item.entry_struct() {
                        self.push_type(
                            TypePushInput {
                                kind: TypeDeclarationKind::Struct(entry),
                                frame,
                                source_id: file.source_id(),
                                is_root_entry: frame.is_root_file,
                            },
                            out.reborrow(),
                        )?;
                    }
                }
                FrameItems::Declarations(items) => {
                    let Some(declaration) = self.ast_access(out.reborrow(), |ast_out| {
                        items.get(frame.next_index, ast_out)
                    })?
                    else {
                        self.frames.pop();
                        continue;
                    };
                    self.diagnostics
                        .require(self.frames.get_mut(frame_index), span, out.reborrow())?
                        .next_index += 1;
                    self.consume(span, out.reborrow())?;
                    let source_id = self
                        .diagnostics
                        .require(self.modules.get(frame.module.index()), span, out.reborrow())?
                        .source_id();
                    self.push_declaration(declaration, frame, source_id, out.reborrow())?;
                }
            }
        }
        Ok(())
    }

    fn normalize<'slot>(
        &mut self,
        span: SourceSpan,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ()> {
        let context = self.context;
        context.sort(
            &mut self.declarations,
            |left, right, mut sort_out| {
                context.execution().consume(3, sort_out.reborrow())?;
                Ok(left
                    .source_id()
                    .get()
                    .cmp(&right.source_id().get())
                    .then_with(|| {
                        left.name_span()
                            .span()
                            .start()
                            .cmp(&right.name_span().span().start())
                    })
                    .then_with(|| left.id().index().cmp(&right.id().index())))
            },
            out.reborrow(),
        )?;
        let mut remap = self
            .diagnostics
            .new_vec(context.execution(), out.reborrow())?;
        remap.try_reserve_exact(self.declarations.len(), out.reborrow())?;
        for _ in 0..self.declarations.len() {
            remap.try_push(usize::MAX, out.reborrow())?;
        }
        for (new_index, declaration) in self.declarations.iter_mut().enumerate() {
            context.execution().consume(2, out.reborrow())?;
            let Some(mapped) = remap.get_mut(declaration.id().index()) else {
                self.diagnostics
                    .abort(schema_internal!(declaration.name_span()));
                return Err(out.fail(BuildFailure::empty()));
            };
            *mapped = new_index;
            declaration.id = TypeDeclId(new_index);
        }
        for module in self.modules.iter_mut() {
            context.execution().consume(1, out.reborrow())?;
            for member in module.members.iter_mut() {
                context.execution().consume(2, out.reborrow())?;
                if let ModuleMember::Type(old_id) = member {
                    let Some(index) = remap.get(old_id.index()).copied() else {
                        self.diagnostics.abort(schema_internal!(span));
                        return Err(out.fail(BuildFailure::empty()));
                    };
                    *old_id = TypeDeclId(index);
                }
            }
        }
        for entry in self.root_entries.iter_mut() {
            context.execution().consume(2, out.reborrow())?;
            let Some(index) = remap.get(entry.index()).copied() else {
                self.diagnostics.abort(schema_internal!(span));
                return Err(out.fail(BuildFailure::empty()));
            };
            *entry = TypeDeclId(index);
        }
        Ok(())
    }

    fn push_declaration<'slot>(
        &mut self,
        declaration: Declaration<'a>,
        frame: Frame<'a>,
        source_id: SourceId,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ()> {
        match declaration {
            Declaration::Module(module_decl) => {
                let info = self.ast_access(out.reborrow(), |mut ast_out| {
                    let name = module_decl.name(ast_out.reborrow())?;
                    let name_node = module_decl.name_node(ast_out.reborrow())?;
                    let declarations = module_decl.declarations(ast_out.reborrow())?;
                    Ok(ModuleDeclarationInfo {
                        name,
                        name_span: name_node.span(),
                        declarations,
                    })
                })?;
                let ModuleDeclarationInfo {
                    name,
                    name_span,
                    declarations,
                } = info;
                let depth = self.next_depth(frame.depth, module_decl.span(), out.reborrow())?;
                let child = self.push_module(
                    ModuleInput {
                        parent: frame.module,
                        name,
                        name_span,
                        source_id,
                    },
                    out.reborrow(),
                )?;
                self.frames.try_push(
                    Frame {
                        items: FrameItems::Declarations(declarations),
                        module: child,
                        depth,
                        is_root_file: false,
                        next_index: 0,
                        next_import_index: 0,
                    },
                    out.reborrow(),
                )?;
            }
            Declaration::TypeAlias(alias) => self.push_type(
                TypePushInput {
                    kind: TypeDeclarationKind::Alias(alias),
                    frame,
                    source_id,
                    is_root_entry: false,
                },
                out.reborrow(),
            )?,
            Declaration::Struct(value) => self.push_type(
                TypePushInput {
                    kind: TypeDeclarationKind::Struct(value),
                    frame,
                    source_id,
                    is_root_entry: false,
                },
                out.reborrow(),
            )?,
            Declaration::Enum(value) => self.push_type(
                TypePushInput {
                    kind: TypeDeclarationKind::Enum(value),
                    frame,
                    source_id,
                    is_root_entry: false,
                },
                out.reborrow(),
            )?,
        }
        Ok(())
    }

    fn push_type<'slot>(
        &mut self,
        input: TypePushInput<'a>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ()> {
        let TypePushInput {
            kind,
            frame,
            source_id,
            is_root_entry,
        } = input;
        let id = TypeDeclId(self.declarations.len());
        let name_span = self.ast_access(out.reborrow(), |mut ast_out| {
            let name_node = match kind {
                TypeDeclarationKind::Alias(declaration) => {
                    declaration.name_node(ast_out.reborrow())?
                }
                TypeDeclarationKind::Struct(declaration) => {
                    declaration.name_node(ast_out.reborrow())?
                }
                TypeDeclarationKind::Enum(declaration) => {
                    declaration.name_node(ast_out.reborrow())?
                }
            };
            Ok(name_node.span())
        })?;
        let declaration = TypeDeclaration {
            id,
            module: frame.module,
            source_id,
            is_root_entry,
            name_span,
            kind,
        };
        let span = declaration.name_span();
        self.declarations.try_push(declaration, out.reborrow())?;
        let module = self.diagnostics.require(
            self.modules.get_mut(frame.module.index()),
            span,
            out.reborrow(),
        )?;
        module
            .members
            .try_push(ModuleMember::Type(id), out.reborrow())?;
        if is_root_entry {
            self.root_entries.try_push(id, out.reborrow())?;
        }
        Ok(())
    }

    fn push_module<'slot>(
        &mut self,
        input: ModuleInput<'a>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ModuleId> {
        let id = ModuleId(self.modules.len());
        let members = self
            .diagnostics
            .new_vec(self.context.execution(), out.reborrow())?;
        self.modules.try_push(
            ModuleNode {
                id,
                parent: Some(input.parent),
                name: Some(input.name),
                name_span: Some(input.name_span),
                source_id: input.source_id,
                members,
                state: ObjectState::Valid,
            },
            out.reborrow(),
        )?;
        let parent = self.diagnostics.require(
            self.modules.get_mut(input.parent.index()),
            input.name_span,
            out.reborrow(),
        )?;
        parent
            .members
            .try_push(ModuleMember::Module(id), out.reborrow())?;
        Ok(id)
    }

    fn consume<'slot>(
        &mut self,
        span: SourceSpan,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ()> {
        self.context
            .consume(1, span, self.diagnostics, out.reborrow())?;
        Ok(())
    }

    fn next_depth<'slot>(
        &mut self,
        depth: usize,
        span: SourceSpan,
        out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, usize> {
        match depth
            .checked_add(1)
            .filter(|next| *next <= self.context.limits().max_nesting_depth())
        {
            Some(next) => Ok(next),
            None => {
                self.diagnostics.abort(EmergencyDiagnostics::resource(span));
                Err(out.fail(BuildFailure::empty()))
            }
        }
    }
}
