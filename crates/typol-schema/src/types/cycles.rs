mod constructibility;
mod edges;
mod propagation;

use crate::{
    context::CompileContext,
    diagnostics::{BuildFailure, DiagnosticCollector},
    module_tree::{ModuleTree, TypeDeclarationKind},
    symbols::{SymbolState, SymbolTable, TypeSymbolId},
    types::model::ResolvedTypeGraph,
};
use opeo::{OResult, Out};
use typol_core::{Diagnostic, DiagnosticCode, DiagnosticMessage, DiagnosticMessageKey, SourceSpan};
use typol_i18n::diagnostic_key;
use typol_syntax::{AstAccessError, VariantPayloadKind};

use constructibility::{TypeConstructibility, TypeConstructibilityInput};
use edges::{GraphState, TypeEdge, TypeEdgeCollectionInput, TypeEdgeCollector, TypeFrame};
use propagation::{InvalidTypePropagationInput, InvalidTypePropagator};

pub(crate) struct TypeCycleReport<'a> {
    pub(crate) key: DiagnosticMessageKey,
    pub(crate) message: &'static str,
    pub(crate) primary: SourceSpan,
    pub(crate) cycle: &'a [TypeSymbolId],
    pub(crate) label_key: DiagnosticMessageKey,
    pub(crate) label_message: &'static str,
}

pub(crate) struct TypeCycles<'a, 'src, 'd> {
    tree: &'a ModuleTree<'src>,
    symbols: &'a SymbolTable<'src>,
    context: &'a CompileContext,
    graph: &'d mut ResolvedTypeGraph,
    diagnostics: &'d mut DiagnosticCollector,
}

impl<'a, 'src, 'd> TypeCycles<'a, 'src, 'd> {
    pub(crate) fn new(
        tree: &'a ModuleTree<'src>,
        symbols: &'a SymbolTable<'src>,
        context: &'a CompileContext,
        graph: &'d mut ResolvedTypeGraph,
        diagnostics: &'d mut DiagnosticCollector,
    ) -> Self {
        Self {
            tree,
            symbols,
            context,
            graph,
            diagnostics,
        }
    }

    fn ast_access<'slot, 'borrow, T>(
        &self,
        out: Out<'slot, 'borrow, BuildFailure>,
        operation: impl for<'call> FnOnce(Out<'call, 'call, AstAccessError>) -> OResult<'call, T>,
    ) -> OResult<'slot, T> {
        self.context.ast_access(out, operation, BuildFailure::from)
    }

    pub(crate) fn check<'slot>(
        &mut self,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ()> {
        let count = self.symbols.types().len();
        let root_span = self.tree.root_span();
        let mut edges = self
            .diagnostics
            .new_vec(self.context.execution(), out.reborrow())?;
        edges.try_reserve_exact(count, out.reborrow())?;
        for _ in 0..count {
            self.context
                .consume(1, self.tree.root_span(), self.diagnostics, out.reborrow())?;
            let owner_edges = self
                .diagnostics
                .new_vec(self.context.execution(), out.reborrow())?;
            edges.try_push(owner_edges, out.reborrow())?;
        }

        for symbol in self.symbols.types() {
            self.context
                .consume(1, self.tree.root_span(), self.diagnostics, out.reborrow())?;
            if symbol.state() != SymbolState::Valid {
                continue;
            }
            match symbol.declaration() {
                TypeDeclarationKind::Alias(_) => {
                    if let Some(target) = self.graph.alias_target(symbol.id()) {
                        TypeEdgeCollector::new(
                            self.symbols,
                            self.context,
                            self.graph,
                            self.diagnostics,
                            root_span,
                        )
                        .collect_type_edges(
                            TypeEdgeCollectionInput {
                                owner: symbol.id(),
                                root: target,
                                span: symbol.name_span(),
                                edges: &mut edges,
                            },
                            out.reborrow(),
                        )?;
                    }
                }
                TypeDeclarationKind::Struct(_) => {
                    for field_id in symbol.fields() {
                        self.context.consume(
                            1,
                            self.tree.root_span(),
                            self.diagnostics,
                            out.reborrow(),
                        )?;
                        let Some(field) = self.symbols.field_symbol(*field_id) else {
                            self.diagnostics.push(schema_internal!(symbol.name_span()));
                            return Err(out.fail(BuildFailure::empty()));
                        };
                        if let Some(type_use) = self.graph.field_type(*field_id) {
                            let field_type = self.ast_access(out.reborrow(), |ast_out| {
                                field.declaration().ty(ast_out)
                            })?;
                            let span = field_type.span();
                            TypeEdgeCollector::new(
                                self.symbols,
                                self.context,
                                self.graph,
                                self.diagnostics,
                                root_span,
                            )
                            .collect_type_edges(
                                TypeEdgeCollectionInput {
                                    owner: symbol.id(),
                                    root: type_use,
                                    span,
                                    edges: &mut edges,
                                },
                                out.reborrow(),
                            )?;
                        }
                    }
                }
                TypeDeclarationKind::Enum(_) => {
                    for variant_id in symbol.variants() {
                        self.context.consume(
                            1,
                            self.tree.root_span(),
                            self.diagnostics,
                            out.reborrow(),
                        )?;
                        let Some(variant) = self.symbols.variant_symbol(*variant_id) else {
                            self.diagnostics.push(schema_internal!(symbol.name_span()));
                            return Err(out.fail(BuildFailure::empty()));
                        };
                        if let Some(type_use) = self.graph.variant_payload_type(*variant_id) {
                            let payload = self.ast_access(out.reborrow(), |ast_out| {
                                variant.declaration().payload(ast_out)
                            })?;
                            let payload_kind =
                                self.ast_access(out.reborrow(), |ast_out| payload.kind(ast_out))?;
                            let VariantPayloadKind::Single(payload) = payload_kind else {
                                self.diagnostics
                                    .push(schema_internal!(variant.declaration().span(),));
                                return Err(out.fail(BuildFailure::empty()));
                            };
                            let payload_type =
                                self.ast_access(out.reborrow(), |ast_out| payload.ty(ast_out))?;
                            let span = payload_type.span();
                            TypeEdgeCollector::new(
                                self.symbols,
                                self.context,
                                self.graph,
                                self.diagnostics,
                                root_span,
                            )
                            .collect_type_edges(
                                TypeEdgeCollectionInput {
                                    owner: symbol.id(),
                                    root: type_use,
                                    span,
                                    edges: &mut edges,
                                },
                                out.reborrow(),
                            )?;
                        }
                        for field_id in variant.fields() {
                            self.context.consume(
                                1,
                                self.tree.root_span(),
                                self.diagnostics,
                                out.reborrow(),
                            )?;
                            let Some(field) = self.symbols.field_symbol(*field_id) else {
                                self.diagnostics
                                    .push(schema_internal!(variant.declaration().span(),));
                                return Err(out.fail(BuildFailure::empty()));
                            };
                            if let Some(type_use) = self.graph.field_type(*field_id) {
                                let field_type = self.ast_access(out.reborrow(), |ast_out| {
                                    field.declaration().ty(ast_out)
                                })?;
                                let span = field_type.span();
                                TypeEdgeCollector::new(
                                    self.symbols,
                                    self.context,
                                    self.graph,
                                    self.diagnostics,
                                    root_span,
                                )
                                .collect_type_edges(
                                    TypeEdgeCollectionInput {
                                        owner: symbol.id(),
                                        root: type_use,
                                        span,
                                        edges: &mut edges,
                                    },
                                    out.reborrow(),
                                )?;
                            }
                        }
                    }
                }
            }
        }

        for owner_edges in edges.iter_mut() {
            self.context
                .consume(1, self.tree.root_span(), self.diagnostics, out.reborrow())?;
            self.context.sort(
                owner_edges,
                |left, right, mut sort_out| {
                    self.context.execution().consume(1, sort_out.reborrow())?;
                    Ok(left
                        .target
                        .index()
                        .cmp(&right.target.index())
                        .then_with(|| {
                            left.span
                                .source_id()
                                .get()
                                .cmp(&right.span.source_id().get())
                        })
                        .then_with(|| left.span.span().start().cmp(&right.span.span().start()))
                        .then_with(|| left.span.span().end().cmp(&right.span.span().end())))
                },
                out.reborrow(),
            )?;
            let mut retained = 0;
            for index in 0..owner_edges.len() {
                self.context
                    .consume(1, root_span, self.diagnostics, out.reborrow())?;
                let current =
                    *self
                        .diagnostics
                        .require(owner_edges.get(index), root_span, out.reborrow())?;
                let is_new = if retained == 0 {
                    true
                } else {
                    let previous = self.diagnostics.require(
                        owner_edges.get(retained - 1),
                        root_span,
                        out.reborrow(),
                    )?;
                    previous.target != current.target
                };
                if is_new {
                    if index != retained {
                        self.context
                            .consume(2, root_span, self.diagnostics, out.reborrow())?;
                        self.diagnostics.require(
                            owner_edges.get(retained),
                            root_span,
                            out.reborrow(),
                        )?;
                        self.diagnostics.require(
                            owner_edges.get(index),
                            root_span,
                            out.reborrow(),
                        )?;
                        owner_edges.swap(retained, index);
                    }
                    retained += 1;
                }
            }
            owner_edges.truncate(retained);
        }

        InvalidTypePropagator::new(InvalidTypePropagationInput {
            tree: self.tree,
            symbols: self.symbols,
            context: self.context,
            graph: self.graph,
            diagnostics: self.diagnostics,
        })
        .propagate(&edges, out.reborrow())?;

        let result = TypeConstructibility::new(TypeConstructibilityInput {
            symbols: self.symbols,
            context: self.context,
            graph: self.graph,
            diagnostics: self.diagnostics,
            root_span,
        })
        .evaluate(out.reborrow())?;
        let constructible = result.constructible;
        let eligible = result.eligible;

        let mut states = self
            .diagnostics
            .new_vec(self.context.execution(), out.reborrow())?;
        states.try_reserve_exact(count, out.reborrow())?;
        for index in 0..count {
            self.context
                .consume(1, self.tree.root_span(), self.diagnostics, out.reborrow())?;
            let state =
                if !*self
                    .diagnostics
                    .require(eligible.get(index), root_span, out.reborrow())?
                    || *self.diagnostics.require(
                        constructible.get(index),
                        root_span,
                        out.reborrow(),
                    )?
                {
                    GraphState::Complete
                } else {
                    GraphState::Unvisited
                };
            states.try_push(state, out.reborrow())?;
        }
        let mut reported_cycles = self
            .diagnostics
            .new_vec(self.context.execution(), out.reborrow())?;
        reported_cycles.try_reserve_exact(count, out.reborrow())?;
        for _ in 0..count {
            self.context
                .consume(1, root_span, self.diagnostics, out.reborrow())?;
            reported_cycles.try_push(false, out.reborrow())?;
        }
        let mut frames = self
            .diagnostics
            .new_vec(self.context.execution(), out.reborrow())?;
        frames.try_reserve(count, out.reborrow())?;

        for symbol in self.symbols.types() {
            self.context
                .consume(1, self.tree.root_span(), self.diagnostics, out.reborrow())?;
            if symbol.state() != SymbolState::Valid
                || matches!(symbol.declaration(), TypeDeclarationKind::Alias(_))
                || !*self.diagnostics.require(
                    eligible.get(symbol.id().index()),
                    symbol.name_span(),
                    out.reborrow(),
                )?
                || *self.diagnostics.require(
                    constructible.get(symbol.id().index()),
                    symbol.name_span(),
                    out.reborrow(),
                )?
                || *self.diagnostics.require(
                    states.get(symbol.id().index()),
                    symbol.name_span(),
                    out.reborrow(),
                )? != GraphState::Unvisited
            {
                continue;
            }
            *self.diagnostics.require(
                states.get_mut(symbol.id().index()),
                symbol.name_span(),
                out.reborrow(),
            )? = GraphState::Visiting;
            frames.try_push(
                TypeFrame {
                    symbol: symbol.id(),
                    next_edge: 0,
                },
                out.reborrow(),
            )?;

            while let Some(frame) = frames.last() {
                self.context
                    .consume(1, self.tree.root_span(), self.diagnostics, out.reborrow())?;
                if self.diagnostics.is_stopped() {
                    return Err(out.fail(BuildFailure::empty()));
                }
                let current = frame.symbol;
                let edge_index = frame.next_edge;
                let current_edges = self.diagnostics.require(
                    edges.get(current.index()),
                    root_span,
                    out.reborrow(),
                )?;
                if let Some(edge) = current_edges.get(edge_index) {
                    let edge = TypeEdge {
                        target: edge.target,
                        span: edge.span,
                    };
                    if let Some(frame) = frames.last_mut() {
                        frame.next_edge += 1;
                    }
                    let target_state = *self.diagnostics.require(
                        states.get(edge.target.index()),
                        edge.span,
                        out.reborrow(),
                    )?;
                    match target_state {
                        GraphState::Unvisited => {
                            *self.diagnostics.require(
                                states.get_mut(edge.target.index()),
                                edge.span,
                                out.reborrow(),
                            )? = GraphState::Visiting;
                            frames.try_push(
                                TypeFrame {
                                    symbol: edge.target,
                                    next_edge: 0,
                                },
                                out.reborrow(),
                            )?;
                        }
                        GraphState::Visiting => {
                            self.context.consume(
                                frames.len(),
                                self.tree.root_span(),
                                self.diagnostics,
                                out.reborrow(),
                            )?;
                            let Some(cycle_start) =
                                frames.iter().position(|frame| frame.symbol == edge.target)
                            else {
                                self.diagnostics.push(schema_internal!(edge.span));
                                return Err(out.fail(BuildFailure::empty()));
                            };
                            let mut cycle = self
                                .diagnostics
                                .new_vec(self.context.execution(), out.reborrow())?;
                            cycle.try_reserve(
                                frames.len().saturating_sub(cycle_start),
                                out.reborrow(),
                            )?;
                            self.context.consume(
                                frames.len() - cycle_start,
                                self.tree.root_span(),
                                self.diagnostics,
                                out.reborrow(),
                            )?;
                            let cycle_frames = self.diagnostics.require(
                                frames.get(cycle_start..),
                                edge.span,
                                out.reborrow(),
                            )?;
                            let mut reported = true;
                            for item in cycle_frames {
                                let state = *self.diagnostics.require(
                                    reported_cycles.get(item.symbol.index()),
                                    edge.span,
                                    out.reborrow(),
                                )?;
                                if !state {
                                    reported = false;
                                }
                            }
                            for item in cycle_frames {
                                self.context.consume(
                                    1,
                                    self.tree.root_span(),
                                    self.diagnostics,
                                    out.reborrow(),
                                )?;
                                cycle.try_push(item.symbol, out.reborrow())?;
                                *self.diagnostics.require(
                                    reported_cycles.get_mut(item.symbol.index()),
                                    edge.span,
                                    out.reborrow(),
                                )? = true;
                            }
                            if reported {
                                continue;
                            }
                            self.report(
                                TypeCycleReport {
                                    key: diagnostic_key!(
                                        "diagnostic.schema.recursive_type_no_finite_value"
                                    ),
                                    message: "recursive type has no finite value",
                                    primary: edge.span,
                                    cycle: &cycle,
                                    label_key: diagnostic_key!(
                                        "diagnostic.schema.recursive_type_declared_here"
                                    ),
                                    label_message: "recursive type is declared here",
                                },
                                out.reborrow(),
                            )?;
                        }
                        GraphState::Complete => {}
                    }
                    continue;
                }
                *self.diagnostics.require(
                    states.get_mut(current.index()),
                    root_span,
                    out.reborrow(),
                )? = GraphState::Complete;
                frames.pop();
            }
        }
        InvalidTypePropagator::new(InvalidTypePropagationInput {
            tree: self.tree,
            symbols: self.symbols,
            context: self.context,
            graph: self.graph,
            diagnostics: self.diagnostics,
        })
        .propagate(&edges, out.reborrow())?;
        Ok(())
    }

    pub(crate) fn report<'slot>(
        &mut self,
        input: TypeCycleReport<'_>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ()> {
        let TypeCycleReport {
            key,
            message,
            primary,
            cycle,
            label_key,
            label_message,
        } = input;
        let mut diagnostic = self.context.budget_access(
            out.reborrow(),
            primary,
            self.diagnostics,
            |budget_out| {
                Diagnostic::try_new(
                    DiagnosticCode::SCHEMA_SEMANTIC_ERROR,
                    DiagnosticMessage::localized(message, key),
                    Some(self.context.execution()),
                    budget_out,
                )
            },
        )?;
        diagnostic.set_primary(Some(primary));
        for id in cycle {
            self.context
                .consume(1, self.tree.root_span(), self.diagnostics, out.reborrow())?;
            let Some(symbol) = self.symbols.type_symbol(*id) else {
                self.diagnostics.push(schema_internal!(primary));
                return Err(out.fail(BuildFailure::empty()));
            };
            let Some(declaration) = self.tree.declaration(symbol.declaration_id()) else {
                self.diagnostics.push(schema_internal!(primary));
                return Err(out.fail(BuildFailure::empty()));
            };
            self.context.budget_access(
                out.reborrow(),
                declaration.span(),
                self.diagnostics,
                |budget_out| {
                    diagnostic.try_add_label_with_text(
                        declaration.span(),
                        DiagnosticMessage::localized(label_message, label_key),
                        Some(self.context.execution()),
                        budget_out,
                    )
                },
            )?;
        }
        self.diagnostics.push(diagnostic);
        if self.diagnostics.is_stopped() {
            Err(out.fail(BuildFailure::empty()))
        } else {
            Ok(())
        }
    }
}
