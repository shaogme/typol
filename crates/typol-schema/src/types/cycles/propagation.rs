use crate::{
    context::CompileContext,
    diagnostics::{BuildFailure, DiagnosticCollector},
    module_tree::{ModuleTree, TypeDeclarationKind},
    session::ObjectState,
    symbols::SymbolTable,
    types::{
        cycles::edges::TypeEdge,
        model::{ResolvedTypeGraph, ResolvedTypeKind},
    },
};

use opeo::{OResult, Out};
use typol_core::ChargedVec;
use typol_syntax::{AstAccessError, VariantPayloadKind};

pub(super) struct InvalidTypePropagationInput<'a, 'src, 'd> {
    pub(super) tree: &'a ModuleTree<'src>,
    pub(super) symbols: &'a SymbolTable<'src>,
    pub(super) context: &'a CompileContext,
    pub(super) graph: &'d mut ResolvedTypeGraph,
    pub(super) diagnostics: &'d mut DiagnosticCollector,
}

pub(super) struct InvalidTypePropagator<'a, 'src, 'd> {
    tree: &'a ModuleTree<'src>,
    symbols: &'a SymbolTable<'src>,
    context: &'a CompileContext,
    graph: &'d mut ResolvedTypeGraph,
    diagnostics: &'d mut DiagnosticCollector,
}

impl<'a, 'src, 'd> InvalidTypePropagator<'a, 'src, 'd> {
    pub(super) fn new(input: InvalidTypePropagationInput<'a, 'src, 'd>) -> Self {
        Self {
            tree: input.tree,
            symbols: input.symbols,
            context: input.context,
            graph: input.graph,
            diagnostics: input.diagnostics,
        }
    }

    fn ast_access<'slot, 'borrow, T>(
        &self,
        out: Out<'slot, 'borrow, BuildFailure>,
        operation: impl for<'call> FnOnce(Out<'call, 'call, AstAccessError>) -> OResult<'call, T>,
    ) -> OResult<'slot, T> {
        self.context.ast_access(out, operation, BuildFailure::from)
    }

    pub(super) fn propagate<'slot>(
        &mut self,
        edges: &[ChargedVec<TypeEdge>],
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ()> {
        let span = self.tree.root_span();
        let count = self.symbols.types().len();
        let mut reverse = self
            .diagnostics
            .new_vec(self.context.execution(), out.reborrow())?;
        reverse.try_reserve_exact(count, out.reborrow())?;
        for _ in 0..count {
            let dependents = self
                .diagnostics
                .new_vec(self.context.execution(), out.reborrow())?;
            reverse.try_push(dependents, out.reborrow())?;
        }
        for symbol in self.symbols.types() {
            self.context
                .consume(1, self.tree.root_span(), self.diagnostics, out.reborrow())?;
            let id = symbol.id().index();
            if symbol.state() != ObjectState::Valid {
                *self.diagnostics.require(
                    self.graph.declarations.get_mut(id),
                    symbol.name_span(),
                    out.reborrow(),
                )? = symbol.state();
            } else {
                let incomplete = match symbol.declaration() {
                    TypeDeclarationKind::Alias(_) => self
                        .diagnostics
                        .require(
                            self.graph.aliases.get(id),
                            symbol.name_span(),
                            out.reborrow(),
                        )?
                        .target()
                        .is_none(),
                    _ => {
                        let mut incomplete = false;
                        for field in symbol.fields() {
                            self.context
                                .consume(1, span, self.diagnostics, out.reborrow())?;
                            let slot = self.diagnostics.require(
                                self.graph.fields.get(field.index()),
                                symbol.name_span(),
                                out.reborrow(),
                            )?;
                            if slot.target().is_none() {
                                incomplete = true;
                                break;
                            }
                        }
                        if !incomplete {
                            for variant in symbol.variants() {
                                self.context
                                    .consume(1, span, self.diagnostics, out.reborrow())?;
                                let variant = self.diagnostics.require(
                                    self.symbols.variant_symbol(*variant),
                                    symbol.name_span(),
                                    out.reborrow(),
                                )?;
                                if variant.state() != ObjectState::Valid {
                                    incomplete = true;
                                    break;
                                }
                                for field in variant.fields() {
                                    self.context.consume(
                                        1,
                                        span,
                                        self.diagnostics,
                                        out.reborrow(),
                                    )?;
                                    let slot = self.diagnostics.require(
                                        self.graph.fields.get(field.index()),
                                        variant.declaration().span(),
                                        out.reborrow(),
                                    )?;
                                    if slot.target().is_none() {
                                        incomplete = true;
                                        break;
                                    }
                                }
                                let payload = self.ast_access(out.reborrow(), |ast_out| {
                                    variant.declaration().payload(ast_out)
                                })?;
                                let payload_kind = self
                                    .ast_access(out.reborrow(), |ast_out| payload.kind(ast_out))?;
                                let payload_incomplete =
                                    if matches!(payload_kind, VariantPayloadKind::Single(_)) {
                                        self.diagnostics
                                            .require(
                                                self.graph
                                                    .variant_payloads
                                                    .get(variant.id().index()),
                                                variant.declaration().span(),
                                                out.reborrow(),
                                            )?
                                            .target()
                                            .is_none()
                                    } else {
                                        false
                                    };
                                if incomplete || payload_incomplete {
                                    incomplete = true;
                                    break;
                                }
                            }
                        }
                        incomplete
                    }
                };
                if incomplete {
                    *self.diagnostics.require(
                        self.graph.declarations.get_mut(id),
                        symbol.name_span(),
                        out.reborrow(),
                    )? = ObjectState::Invalid;
                } else if *self.diagnostics.require(
                    self.graph.declarations.get(id),
                    symbol.name_span(),
                    out.reborrow(),
                )? == ObjectState::Pending
                {
                    *self.diagnostics.require(
                        self.graph.declarations.get_mut(id),
                        symbol.name_span(),
                        out.reborrow(),
                    )? = ObjectState::Valid;
                }
            }
            let owner_edges =
                self.diagnostics
                    .require(edges.get(id), symbol.name_span(), out.reborrow())?;
            for edge in owner_edges {
                self.context
                    .consume(1, self.tree.root_span(), self.diagnostics, out.reborrow())?;
                let target_edges = self.diagnostics.require(
                    reverse.get_mut(edge.target.index()),
                    edge.span,
                    out.reborrow(),
                )?;
                target_edges.try_reserve(1, out.reborrow())?;
                target_edges.try_push(id, out.reborrow())?;
            }
        }
        let mut pending = self
            .diagnostics
            .new_vec(self.context.execution(), out.reborrow())?;
        pending.try_reserve_exact(count, out.reborrow())?;
        for (id, state) in self.graph.declarations.iter().enumerate() {
            self.context
                .consume(1, self.tree.root_span(), self.diagnostics, out.reborrow())?;
            if *state != ObjectState::Valid {
                pending.try_push(id, out.reborrow())?;
            }
        }
        while let Some(id) = pending.pop() {
            self.context
                .consume(1, self.tree.root_span(), self.diagnostics, out.reborrow())?;
            let owners =
                self.diagnostics
                    .require(reverse.get(id), self.tree.root_span(), out.reborrow())?;
            for owner in owners {
                self.context
                    .consume(1, self.tree.root_span(), self.diagnostics, out.reborrow())?;
                let state = *self.diagnostics.require(
                    self.graph.declarations.get(*owner),
                    self.tree.root_span(),
                    out.reborrow(),
                )?;
                if state == ObjectState::Valid {
                    *self.diagnostics.require(
                        self.graph.declarations.get_mut(*owner),
                        self.tree.root_span(),
                        out.reborrow(),
                    )? = ObjectState::Invalid;
                    pending.try_push(*owner, out.reborrow())?;
                }
            }
        }
        // Subtypes are inserted before their containers, so one forward pass propagates them to every use site.
        for index in 0..self.graph.uses.len() {
            self.context
                .consume(1, self.tree.root_span(), self.diagnostics, out.reborrow())?;
            let kind = self
                .diagnostics
                .require(self.graph.uses.get(index), span, out.reborrow())?
                .kind();
            let valid = match kind {
                ResolvedTypeKind::Builtin(_) => true,
                ResolvedTypeKind::User(id) => {
                    *self.diagnostics.require(
                        self.graph.declarations.get(id.index()),
                        span,
                        out.reborrow(),
                    )? == ObjectState::Valid
                }
                ResolvedTypeKind::Option(id) | ResolvedTypeKind::Map(id) => {
                    self.diagnostics
                        .require(self.graph.uses.get(id.index()), span, out.reborrow())?
                        .state()
                        == ObjectState::Valid
                }
                ResolvedTypeKind::Array { element, .. } => {
                    self.diagnostics
                        .require(self.graph.uses.get(element.index()), span, out.reborrow())?
                        .state()
                        == ObjectState::Valid
                }
            };
            if !valid {
                self.diagnostics
                    .require(self.graph.uses.get_mut(index), span, out.reborrow())?
                    .invalidate();
            }
        }
        let graph = &mut **self.graph;
        for symbol in self.symbols.types() {
            self.context
                .consume(1, self.tree.root_span(), self.diagnostics, out.reborrow())?;
            let state = *self.diagnostics.require(
                graph.declarations.get(symbol.id().index()),
                symbol.name_span(),
                out.reborrow(),
            )?;
            *self
                .diagnostics
                .require(
                    graph.aliases.get_mut(symbol.id().index()),
                    symbol.name_span(),
                    out.reborrow(),
                )?
                .state_mut() = state;
        }
        for field in self.symbols.fields() {
            self.context
                .consume(1, self.tree.root_span(), self.diagnostics, out.reborrow())?;
            let target = self
                .diagnostics
                .require(
                    graph.fields.get(field.id().index()),
                    self.tree.root_span(),
                    out.reborrow(),
                )?
                .target();
            let target_valid = match target {
                Some(id) => {
                    self.diagnostics
                        .require(
                            graph.uses.get(id.index()),
                            self.tree.root_span(),
                            out.reborrow(),
                        )?
                        .state()
                        == ObjectState::Valid
                }
                None => false,
            };
            let state = if field.state() != ObjectState::Valid {
                field.state()
            } else if target_valid {
                ObjectState::Valid
            } else {
                ObjectState::Invalid
            };
            *self
                .diagnostics
                .require(
                    graph.fields.get_mut(field.id().index()),
                    self.tree.root_span(),
                    out.reborrow(),
                )?
                .state_mut() = state;
        }
        for variant in self.symbols.variants() {
            self.context
                .consume(1, self.tree.root_span(), self.diagnostics, out.reborrow())?;
            let target = self
                .diagnostics
                .require(
                    graph.variant_payloads.get(variant.id().index()),
                    variant.declaration().span(),
                    out.reborrow(),
                )?
                .target();
            let target_valid = match target {
                Some(id) => {
                    self.diagnostics
                        .require(
                            graph.uses.get(id.index()),
                            variant.declaration().span(),
                            out.reborrow(),
                        )?
                        .state()
                        == ObjectState::Valid
                }
                None => false,
            };
            let state = if variant.state() != ObjectState::Valid {
                variant.state()
            } else {
                let payload = self.context.ast_access(
                    out.reborrow(),
                    |ast_out| variant.declaration().payload(ast_out),
                    BuildFailure::from,
                )?;
                let payload_kind = self.context.ast_access(
                    out.reborrow(),
                    |ast_out| payload.kind(ast_out),
                    BuildFailure::from,
                )?;
                if !matches!(payload_kind, VariantPayloadKind::Single(_)) || target_valid {
                    ObjectState::Valid
                } else {
                    ObjectState::Invalid
                }
            };
            *self
                .diagnostics
                .require(
                    graph.variant_payloads.get_mut(variant.id().index()),
                    variant.declaration().span(),
                    out.reborrow(),
                )?
                .state_mut() = state;
        }
        Ok(())
    }
}
