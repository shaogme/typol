use crate::{
    context::CompileContext,
    diagnostics::{BuildFailure, DiagnosticCollector},
    module_tree::TypeDeclarationKind,
    session::ObjectState,
    symbols::{FieldSymbolId, SymbolState, SymbolTable, TypeSymbol, TypeSymbolId},
    types::model::{ResolvedTypeGraph, ResolvedTypeKind, TypeUseId},
};

use opeo::{OResult, Out};
use typol_core::{ChargedVec, SourceSpan};
use typol_syntax::VariantPayloadKind;

pub(super) struct TypeConstructibilityInput<'a, 'src, 'd> {
    pub(super) symbols: &'a SymbolTable<'src>,
    pub(super) context: &'a CompileContext,
    pub(super) graph: &'d mut ResolvedTypeGraph,
    pub(super) diagnostics: &'d mut DiagnosticCollector,
    pub(super) root_span: SourceSpan,
}

pub(super) struct TypeConstructibilityResult {
    pub(super) eligible: ChargedVec<bool>,
    pub(super) constructible: ChargedVec<bool>,
}

struct ConstructibilityClause {
    owner: TypeSymbolId,
    unresolved: usize,
}

struct ConstructibilityPlan {
    clauses: ChargedVec<ConstructibilityClause>,
    waiting: ChargedVec<ChargedVec<usize>>,
}

struct ConstructibilityPlanInput<'a, 'src, 'd> {
    symbols: &'a SymbolTable<'src>,
    context: &'a CompileContext,
    graph: &'a ResolvedTypeGraph,
    diagnostics: &'d mut DiagnosticCollector,
    root_span: SourceSpan,
}

struct ConstructibilityPlanBuilder<'a, 'src, 'd> {
    symbols: &'a SymbolTable<'src>,
    context: &'a CompileContext,
    graph: &'a ResolvedTypeGraph,
    diagnostics: &'d mut DiagnosticCollector,
    root_span: SourceSpan,
    clauses: ChargedVec<ConstructibilityClause>,
    waiting: ChargedVec<ChargedVec<usize>>,
}

pub(super) struct TypeConstructibility<'a, 'src, 'd> {
    symbols: &'a SymbolTable<'src>,
    context: &'a CompileContext,
    graph: &'d mut ResolvedTypeGraph,
    diagnostics: &'d mut DiagnosticCollector,
    root_span: SourceSpan,
}

impl<'a, 'src, 'd> TypeConstructibility<'a, 'src, 'd> {
    pub(super) fn new(input: TypeConstructibilityInput<'a, 'src, 'd>) -> Self {
        Self {
            symbols: input.symbols,
            context: input.context,
            graph: input.graph,
            diagnostics: input.diagnostics,
            root_span: input.root_span,
        }
    }

    pub(super) fn evaluate<'slot>(
        &mut self,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, TypeConstructibilityResult> {
        let count = self.symbols.types().len();
        let mut eligible = self
            .diagnostics
            .new_vec(self.context.execution(), out.reborrow())?;
        eligible.try_reserve_exact(count, out.reborrow())?;
        let mut constructible = self
            .diagnostics
            .new_vec(self.context.execution(), out.reborrow())?;
        constructible.try_reserve_exact(count, out.reborrow())?;
        for symbol in self.symbols.types() {
            self.context
                .consume(1, self.root_span, self.diagnostics, out.reborrow())?;
            let valid = symbol.state() == SymbolState::Valid
                && *self.diagnostics.require(
                    self.graph.declarations.get(symbol.id().index()),
                    symbol.name_span(),
                    out.reborrow(),
                )? == ObjectState::Valid;
            eligible.try_push(valid, out.reborrow())?;
            constructible.try_push(false, out.reborrow())?;
        }

        let mut plan = ConstructibilityPlanBuilder::new(
            ConstructibilityPlanInput {
                symbols: self.symbols,
                context: self.context,
                graph: self.graph,
                diagnostics: self.diagnostics,
                root_span: self.root_span,
            },
            out.reborrow(),
        )?
        .build(&eligible, out.reborrow())?;
        let mut pending = self
            .diagnostics
            .new_vec(self.context.execution(), out.reborrow())?;
        pending.try_reserve_exact(count, out.reborrow())?;
        for clause in plan.clauses.iter() {
            self.context
                .consume(1, self.root_span, self.diagnostics, out.reborrow())?;
            if clause.unresolved != 0 {
                continue;
            }
            let owner = clause.owner;
            let is_eligible = *self.diagnostics.require(
                eligible.get(owner.index()),
                self.root_span,
                out.reborrow(),
            )?;
            let is_constructible = self.diagnostics.require(
                constructible.get_mut(owner.index()),
                self.root_span,
                out.reborrow(),
            )?;
            if is_eligible && !*is_constructible {
                *is_constructible = true;
                pending.try_push(owner, out.reborrow())?;
            }
        }

        while let Some(id) = pending.pop() {
            self.context
                .consume(1, self.root_span, self.diagnostics, out.reborrow())?;
            for clause_id in self.diagnostics.require(
                plan.waiting.get(id.index()),
                self.root_span,
                out.reborrow(),
            )? {
                self.context
                    .consume(1, self.root_span, self.diagnostics, out.reborrow())?;
                let clause = self.diagnostics.require(
                    plan.clauses.get_mut(*clause_id),
                    self.root_span,
                    out.reborrow(),
                )?;
                let Some(remaining) = clause.unresolved.checked_sub(1) else {
                    self.diagnostics.push(schema_internal!(self.root_span));
                    return Err(out.fail(BuildFailure::empty()));
                };
                clause.unresolved = remaining;
                if remaining != 0 {
                    continue;
                }
                let owner = clause.owner;
                let is_eligible = *self.diagnostics.require(
                    eligible.get(owner.index()),
                    self.root_span,
                    out.reborrow(),
                )?;
                let is_constructible = self.diagnostics.require(
                    constructible.get_mut(owner.index()),
                    self.root_span,
                    out.reborrow(),
                )?;
                if is_eligible && !*is_constructible {
                    *is_constructible = true;
                    pending.try_push(owner, out.reborrow())?;
                }
            }
        }

        for symbol in self.symbols.types() {
            self.context
                .consume(1, self.root_span, self.diagnostics, out.reborrow())?;
            let id = symbol.id().index();
            let is_constructible = *self.diagnostics.require(
                constructible.get(id),
                symbol.name_span(),
                out.reborrow(),
            )?;
            let is_eligible =
                *self
                    .diagnostics
                    .require(eligible.get(id), symbol.name_span(), out.reborrow())?;
            if is_eligible && !is_constructible {
                *self.diagnostics.require(
                    self.graph.declarations.get_mut(id),
                    symbol.name_span(),
                    out.reborrow(),
                )? = ObjectState::Invalid;
            }
        }

        Ok(TypeConstructibilityResult {
            eligible,
            constructible,
        })
    }
}

impl<'a, 'src, 'd> ConstructibilityPlanBuilder<'a, 'src, 'd> {
    fn new<'slot>(
        input: ConstructibilityPlanInput<'a, 'src, 'd>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, Self> {
        let ConstructibilityPlanInput {
            symbols,
            context,
            graph,
            diagnostics,
            root_span,
        } = input;
        let count = symbols.types().len();
        let mut waiting = diagnostics.new_vec(context.execution(), out.reborrow())?;
        waiting.try_reserve_exact(count, out.reborrow())?;
        for _ in 0..count {
            context.consume(1, root_span, diagnostics, out.reborrow())?;
            waiting.try_push(
                diagnostics.new_vec(context.execution(), out.reborrow())?,
                out.reborrow(),
            )?;
        }
        let clauses = diagnostics.new_vec(context.execution(), out.reborrow())?;
        Ok(Self {
            symbols,
            context,
            graph,
            diagnostics,
            root_span,
            clauses,
            waiting,
        })
    }

    fn build<'slot>(
        mut self,
        eligible: &[bool],
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ConstructibilityPlan> {
        for symbol in self.symbols.types() {
            self.context
                .consume(1, self.root_span, self.diagnostics, out.reborrow())?;
            if !*self.diagnostics.require(
                eligible.get(symbol.id().index()),
                symbol.name_span(),
                out.reborrow(),
            )? {
                continue;
            }
            match symbol.declaration() {
                TypeDeclarationKind::Alias(_) => {
                    let clause = self.add_clause(symbol.id(), out.reborrow())?;
                    let Some(target) = self.graph.alias_target(symbol.id()) else {
                        self.diagnostics.push(schema_internal!(symbol.name_span()));
                        return Err(out.fail(BuildFailure::empty()));
                    };
                    self.add_type_use_dependency(clause, target, out.reborrow())?;
                }
                TypeDeclarationKind::Struct(_) => {
                    let clause = self.add_clause(symbol.id(), out.reborrow())?;
                    self.add_fields_dependency(symbol, symbol.fields(), clause, out.reborrow())?;
                }
                TypeDeclarationKind::Enum(_) => {
                    for variant_id in symbol.variants() {
                        self.context.consume(
                            1,
                            symbol.name_span(),
                            self.diagnostics,
                            out.reborrow(),
                        )?;
                        let Some(variant) = self.symbols.variant_symbol(*variant_id) else {
                            self.diagnostics.push(schema_internal!(symbol.name_span()));
                            return Err(out.fail(BuildFailure::empty()));
                        };
                        let clause = self.add_clause(symbol.id(), out.reborrow())?;
                        let payload = self.context.ast_access(
                            out.reborrow(),
                            |ast_out| variant.declaration().payload(ast_out),
                            BuildFailure::from,
                        )?;
                        match self.context.ast_access(
                            out.reborrow(),
                            |ast_out| payload.kind(ast_out),
                            BuildFailure::from,
                        )? {
                            VariantPayloadKind::Unit => {}
                            VariantPayloadKind::Single(_) => {
                                let Some(payload_type) =
                                    self.graph.variant_payload_type(variant.id())
                                else {
                                    self.diagnostics
                                        .push(schema_internal!(variant.declaration().span()));
                                    return Err(out.fail(BuildFailure::empty()));
                                };
                                self.add_type_use_dependency(clause, payload_type, out.reborrow())?;
                            }
                            VariantPayloadKind::Structure(_) => self.add_fields_dependency(
                                symbol,
                                variant.fields(),
                                clause,
                                out.reborrow(),
                            )?,
                        }
                    }
                }
            }
        }
        Ok(ConstructibilityPlan {
            clauses: self.clauses,
            waiting: self.waiting,
        })
    }

    fn add_clause<'slot>(
        &mut self,
        owner: TypeSymbolId,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, usize> {
        let id = self.clauses.len();
        self.clauses.try_push(
            ConstructibilityClause {
                owner,
                unresolved: 0,
            },
            out.reborrow(),
        )?;
        Ok(id)
    }

    fn add_fields_dependency<'slot>(
        &mut self,
        owner: &TypeSymbol<'src>,
        fields: &[FieldSymbolId],
        clause: usize,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ()> {
        for field_id in fields {
            self.context
                .consume(1, owner.name_span(), self.diagnostics, out.reborrow())?;
            let Some(field_type) = self.graph.field_type(*field_id) else {
                self.diagnostics.push(schema_internal!(owner.name_span()));
                return Err(out.fail(BuildFailure::empty()));
            };
            self.add_type_use_dependency(clause, field_type, out.reborrow())?;
        }
        Ok(())
    }

    fn add_type_use_dependency<'slot>(
        &mut self,
        clause_id: usize,
        mut type_use_id: TypeUseId,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ()> {
        loop {
            self.context
                .consume(1, self.root_span, self.diagnostics, out.reborrow())?;
            let Some(type_use) = self.graph.type_use(type_use_id) else {
                self.diagnostics.push(schema_internal!(self.root_span));
                return Err(out.fail(BuildFailure::empty()));
            };
            match type_use.kind() {
                ResolvedTypeKind::Builtin(_)
                | ResolvedTypeKind::Option(_)
                | ResolvedTypeKind::Map(_)
                | ResolvedTypeKind::Array {
                    length: None | Some(0),
                    ..
                } => return Ok(()),
                ResolvedTypeKind::Array { element, .. } => type_use_id = element,
                ResolvedTypeKind::User(target) => {
                    let clause = self.diagnostics.require(
                        self.clauses.get_mut(clause_id),
                        self.root_span,
                        out.reborrow(),
                    )?;
                    let Some(unresolved) = clause.unresolved.checked_add(1) else {
                        self.diagnostics.push(schema_internal!(self.root_span));
                        return Err(out.fail(BuildFailure::empty()));
                    };
                    clause.unresolved = unresolved;
                    self.diagnostics
                        .require(
                            self.waiting.get_mut(target.index()),
                            self.root_span,
                            out.reborrow(),
                        )?
                        .try_push(clause_id, out.reborrow())?;
                    return Ok(());
                }
            }
        }
    }
}
