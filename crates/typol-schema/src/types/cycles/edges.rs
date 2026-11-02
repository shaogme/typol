use crate::{
    context::CompileContext,
    diagnostics::{BuildFailure, DiagnosticCollector},
    module_tree::TypeDeclarationKind,
    symbols::{SymbolState, SymbolTable, TypeSymbolId},
    types::model::{ResolvedTypeGraph, ResolvedTypeKind, TypeUseId},
};

use opeo::{OResult, Out};
use typol_core::{ChargedVec, SourceSpan};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum GraphState {
    Unvisited,
    Visiting,
    Complete,
}

pub(super) struct TypeFrame {
    pub(super) symbol: TypeSymbolId,
    pub(super) next_edge: usize,
}

#[derive(Clone, Copy)]
pub(super) struct TypeEdge {
    pub(super) target: TypeSymbolId,
    pub(super) span: SourceSpan,
}

pub(super) struct TypeEdgeCollector<'a, 'src, 'd> {
    symbols: &'a SymbolTable<'src>,
    context: &'a CompileContext,
    graph: &'d mut ResolvedTypeGraph,
    diagnostics: &'d mut DiagnosticCollector,
    root_span: SourceSpan,
}

pub(super) struct TypeEdgeCollectionInput<'edges> {
    pub(super) owner: TypeSymbolId,
    pub(super) root: TypeUseId,
    pub(super) span: SourceSpan,
    pub(super) edges: &'edges mut [ChargedVec<TypeEdge>],
}

impl<'a, 'src, 'd> TypeEdgeCollector<'a, 'src, 'd> {
    pub(super) fn new(
        symbols: &'a SymbolTable<'src>,
        context: &'a CompileContext,
        graph: &'d mut ResolvedTypeGraph,
        diagnostics: &'d mut DiagnosticCollector,
        root_span: SourceSpan,
    ) -> Self {
        Self {
            symbols,
            context,
            graph,
            diagnostics,
            root_span,
        }
    }

    pub(super) fn collect_type_edges<'slot>(
        &mut self,
        input: TypeEdgeCollectionInput<'_>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ()> {
        let TypeEdgeCollectionInput {
            owner,
            root,
            span,
            edges,
        } = input;
        let mut pending = self
            .diagnostics
            .new_vec(self.context.execution(), out.reborrow())?;
        pending.try_reserve(1, out.reborrow())?;
        pending.try_push(root, out.reborrow())?;
        while let Some(id) = pending.pop() {
            self.context
                .consume(1, self.root_span, self.diagnostics, out.reborrow())?;
            let Some(type_use) = self.graph.type_use(id) else {
                self.diagnostics.push(schema_internal!(span));
                return Err(out.fail(BuildFailure::empty()));
            };
            match type_use.kind() {
                ResolvedTypeKind::Builtin(_) => {}
                ResolvedTypeKind::Option(inner) | ResolvedTypeKind::Map(inner) => {
                    pending.try_reserve(1, out.reborrow())?;
                    pending.try_push(inner, out.reborrow())?;
                }
                ResolvedTypeKind::Array { element, .. } => {
                    pending.try_reserve(1, out.reborrow())?;
                    pending.try_push(element, out.reborrow())?;
                }
                ResolvedTypeKind::User(target) => {
                    let Some(target_symbol) = self.symbols.type_symbol(target) else {
                        self.diagnostics.push(schema_internal!(span));
                        return Err(out.fail(BuildFailure::empty()));
                    };
                    if target_symbol.state() == SymbolState::Valid
                        && !matches!(target_symbol.declaration(), TypeDeclarationKind::Alias(_))
                    {
                        let owner_edges = self.diagnostics.require(
                            edges.get_mut(owner.index()),
                            span,
                            out.reborrow(),
                        )?;
                        owner_edges.try_reserve(1, out.reborrow())?;
                        owner_edges.try_push(TypeEdge { target, span }, out.reborrow())?;
                    }
                }
            }
        }
        Ok(())
    }
}
