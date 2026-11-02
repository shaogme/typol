use crate::{
    context::CompileContext,
    diagnostics::{BuildFailure, DiagnosticCollector},
    expressions::ExpressionType,
    module_tree::{ModuleId, ModuleTree, TypeDeclarationKind},
    symbols::{SymbolState, SymbolTable, TypeSymbolId},
    types::{
        PathResolution, TypePathResolver,
        cycles::{TypeCycleReport, TypeCycles},
        model::{
            AliasLinkId, ResolvedAliasLink, ResolvedTypeGraph, ResolvedTypeKind, ResolvedTypeUse,
            TypeSlot, TypeUseId,
        },
    },
};
use opeo::{OResult, Out};
use typol_core::{ChargedVec, SourceSpan};
use typol_i18n::{diagnostic_key, diagnostic_message};
use typol_semantic::TypeId;
use typol_syntax::{AstAccessError, TypeExpr, TypeExprKind};

use super::support::{AliasDependency, AliasFrame, AliasState, TypeGraphSupport};

pub(super) struct AliasResolverInput<'a, 'src, 'd> {
    pub(super) tree: &'a ModuleTree<'src>,
    pub(super) symbols: &'a SymbolTable<'src>,
    pub(super) diagnostics: &'d mut DiagnosticCollector,
    pub(super) context: &'a CompileContext,
    pub(super) graph: &'d mut ResolvedTypeGraph,
}

pub(super) struct AliasResolver<'a, 'src, 'd> {
    tree: &'a ModuleTree<'src>,
    symbols: &'a SymbolTable<'src>,
    diagnostics: &'d mut DiagnosticCollector,
    context: &'a CompileContext,
    graph: &'d mut ResolvedTypeGraph,
}

impl<'a, 'src, 'd> AliasResolver<'a, 'src, 'd> {
    pub(super) fn new(input: AliasResolverInput<'a, 'src, 'd>) -> Self {
        Self {
            tree: input.tree,
            symbols: input.symbols,
            diagnostics: input.diagnostics,
            context: input.context,
            graph: input.graph,
        }
    }

    fn ast_access<'slot, 'borrow, T>(
        &self,
        out: Out<'slot, 'borrow, BuildFailure>,
        operation: impl for<'call> FnOnce(Out<'call, 'call, AstAccessError>) -> OResult<'call, T>,
    ) -> OResult<'slot, T> {
        self.context.ast_access(out, operation, BuildFailure::from)
    }

    pub(super) fn collect_alias_dependencies<'slot>(
        &mut self,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<
        'slot,
        (
            ChargedVec<ChargedVec<AliasDependency>>,
            ChargedVec<AliasState>,
        ),
    > {
        let count = self.symbols.types().len();
        let mut dependencies = self
            .diagnostics
            .new_vec(self.context.execution(), out.reborrow())?;
        dependencies.try_reserve_exact(count, out.reborrow())?;
        for _ in 0..count {
            self.context
                .consume(1, self.tree.root_span(), self.diagnostics, out.reborrow())?;
            let dependencies_for_symbol = self
                .diagnostics
                .new_vec(self.context.execution(), out.reborrow())?;
            dependencies.try_push(dependencies_for_symbol, out.reborrow())?;
        }

        let mut states = self
            .diagnostics
            .new_vec(self.context.execution(), out.reborrow())?;
        states.try_reserve_exact(count, out.reborrow())?;
        for _ in 0..count {
            self.context
                .consume(1, self.tree.root_span(), self.diagnostics, out.reborrow())?;
            states.try_push(AliasState::NotAlias, out.reborrow())?;
        }

        for symbol in self.symbols.types() {
            self.context
                .consume(1, self.tree.root_span(), self.diagnostics, out.reborrow())?;
            if symbol.state() != SymbolState::Valid {
                continue;
            }
            let TypeDeclarationKind::Alias(alias) = symbol.declaration() else {
                continue;
            };
            let target = self.ast_access(out.reborrow(), |ast_out| alias.target(ast_out))?;
            let state = self.diagnostics.require(
                states.get_mut(symbol.id().index()),
                symbol.name_span(),
                out.reborrow(),
            )?;
            *state = AliasState::Unvisited;
            let mut expressions = self
                .diagnostics
                .new_vec(self.context.execution(), out.reborrow())?;
            expressions.try_reserve(1, out.reborrow())?;
            expressions.try_push(target, out.reborrow())?;
            let mut invalid = false;

            while let Some(expression) = expressions.pop() {
                self.context
                    .consume(1, self.tree.root_span(), self.diagnostics, out.reborrow())?;
                let expression_kind =
                    self.ast_access(out.reborrow(), |ast_out| expression.kind(ast_out))?;
                match expression_kind {
                    TypeExprKind::Builtin(_) => {}
                    TypeExprKind::Option(inner) | TypeExprKind::Map { value: inner } => {
                        TypeGraphSupport::push_expr(&mut expressions, inner, out.reborrow())?;
                    }
                    TypeExprKind::Array { element, .. } => {
                        TypeGraphSupport::push_expr(&mut expressions, element, out.reborrow())?;
                    }
                    TypeExprKind::Path(path) => {
                        let resolution = TypePathResolver::lookup::<BuildFailure>(
                            self.tree,
                            self.symbols,
                            symbol.module(),
                            path,
                            out.reborrow(),
                        )?;
                        match resolution {
                            PathResolution::Resolved(target) => {
                                let Some(target_symbol) = self.symbols.type_symbol(target) else {
                                    self.diagnostics.push(schema_internal!(path.span()));
                                    return Err(out.fail(BuildFailure::empty()));
                                };
                                if target_symbol.state() != SymbolState::Valid {
                                    invalid = true;
                                    continue;
                                }
                                if matches!(
                                    target_symbol.declaration(),
                                    TypeDeclarationKind::Alias(_)
                                ) {
                                    let edges = self.diagnostics.require(
                                        dependencies.get_mut(symbol.id().index()),
                                        path.span(),
                                        out.reborrow(),
                                    )?;
                                    edges.try_reserve(1, out.reborrow())?;
                                    edges.try_push(
                                        AliasDependency {
                                            target,
                                            reference_span: path.span(),
                                        },
                                        out.reborrow(),
                                    )?;
                                }
                            }
                            PathResolution::Ambiguous => invalid = true,
                            problem => {
                                TypeGraphSupport::report(problem, path.span(), self.diagnostics);
                                invalid = true;
                            }
                        }
                    }
                }
                if self.diagnostics.is_stopped() {
                    return Err(out.fail(BuildFailure::empty()));
                }
            }
            if invalid {
                *self.diagnostics.require(
                    states.get_mut(symbol.id().index()),
                    symbol.name_span(),
                    out.reborrow(),
                )? = AliasState::Invalid;
            }
        }
        Ok((dependencies, states))
    }

    pub(super) fn resolve_aliases<'slot>(
        &mut self,
        dependencies: &[ChargedVec<AliasDependency>],
        states: &mut [AliasState],
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ()> {
        let mut frames = self
            .diagnostics
            .new_vec(self.context.execution(), out.reborrow())?;
        frames.try_reserve(self.symbols.types().len(), out.reborrow())?;

        for symbol in self.symbols.types() {
            self.context
                .consume(1, self.tree.root_span(), self.diagnostics, out.reborrow())?;
            if self.diagnostics.is_stopped() {
                return Err(out.fail(BuildFailure::empty()));
            }
            let id = symbol.id();
            if *self.diagnostics.require(
                states.get(id.index()),
                symbol.name_span(),
                out.reborrow(),
            )? != AliasState::Unvisited
            {
                continue;
            }
            *self.diagnostics.require(
                states.get_mut(id.index()),
                symbol.name_span(),
                out.reborrow(),
            )? = AliasState::Visiting;
            frames.try_push(
                AliasFrame {
                    symbol: id,
                    next_dependency: 0,
                },
                out.reborrow(),
            )?;

            while let Some(frame) = frames.last() {
                self.context
                    .consume(1, self.tree.root_span(), self.diagnostics, out.reborrow())?;
                let current = frame.symbol;
                if *self.diagnostics.require(
                    states.get(current.index()),
                    self.tree.root_span(),
                    out.reborrow(),
                )? == AliasState::Invalid
                {
                    frames.pop();
                    continue;
                }
                let current_dependencies = self.diagnostics.require(
                    dependencies.get(current.index()),
                    self.tree.root_span(),
                    out.reborrow(),
                )?;
                if let Some(dependency) =
                    current_dependencies.get(frames.last().map_or(0, |frame| frame.next_dependency))
                {
                    let dependency = AliasDependency {
                        target: dependency.target,
                        reference_span: dependency.reference_span,
                    };
                    if let Some(frame) = frames.last_mut() {
                        frame.next_dependency += 1;
                    }
                    let target_state = *self.diagnostics.require(
                        states.get(dependency.target.index()),
                        dependency.reference_span,
                        out.reborrow(),
                    )?;
                    match target_state {
                        AliasState::Unvisited => {
                            *self.diagnostics.require(
                                states.get_mut(dependency.target.index()),
                                dependency.reference_span,
                                out.reborrow(),
                            )? = AliasState::Visiting;
                            frames.try_push(
                                AliasFrame {
                                    symbol: dependency.target,
                                    next_dependency: 0,
                                },
                                out.reborrow(),
                            )?;
                        }
                        AliasState::Visiting => {
                            self.context.consume(
                                frames.len(),
                                self.tree.root_span(),
                                self.diagnostics,
                                out.reborrow(),
                            )?;
                            let Some(cycle_start) = frames
                                .iter()
                                .position(|frame| frame.symbol == dependency.target)
                            else {
                                self.diagnostics
                                    .push(schema_internal!(dependency.reference_span,));
                                return Err(out.fail(BuildFailure::empty()));
                            };
                            let mut cycle = self
                                .diagnostics
                                .new_vec(self.context.execution(), out.reborrow())?;
                            cycle.try_reserve(
                                frames.len().saturating_sub(cycle_start),
                                out.reborrow(),
                            )?;
                            let cycle_frames = self.diagnostics.require(
                                frames.get(cycle_start..),
                                dependency.reference_span,
                                out.reborrow(),
                            )?;
                            for item in cycle_frames {
                                self.context.consume(
                                    1,
                                    self.tree.root_span(),
                                    self.diagnostics,
                                    out.reborrow(),
                                )?;
                                cycle.try_push(item.symbol, out.reborrow())?;
                                *self.diagnostics.require(
                                    states.get_mut(item.symbol.index()),
                                    dependency.reference_span,
                                    out.reborrow(),
                                )? = AliasState::Invalid;
                            }
                            TypeCycles::new(
                                self.tree,
                                self.symbols,
                                self.context,
                                self.graph,
                                self.diagnostics,
                            )
                            .report(
                                TypeCycleReport {
                                    key: diagnostic_key!("diagnostic.schema.recursive_alias"),
                                    message: "recursive type alias detected",
                                    primary: dependency.reference_span,
                                    cycle: &cycle,
                                    label_key: diagnostic_key!(
                                        "diagnostic.schema.recursive_alias_declared_here"
                                    ),
                                    label_message: "declaration in the alias cycle is here",
                                },
                                out.reborrow(),
                            )?;
                        }
                        AliasState::Invalid => {
                            *self.diagnostics.require(
                                states.get_mut(current.index()),
                                dependency.reference_span,
                                out.reborrow(),
                            )? = AliasState::Invalid;
                        }
                        AliasState::Resolved | AliasState::NotAlias => {}
                    }
                    continue;
                }

                if *self.diagnostics.require(
                    states.get(current.index()),
                    self.tree.root_span(),
                    out.reborrow(),
                )? == AliasState::Visiting
                {
                    let Some(symbol) = self.symbols.type_symbol(current) else {
                        self.diagnostics
                            .push(schema_internal!(self.tree.root_span()));
                        return Err(out.fail(BuildFailure::empty()));
                    };
                    let TypeDeclarationKind::Alias(alias) = symbol.declaration() else {
                        *self.diagnostics.require(
                            states.get_mut(current.index()),
                            symbol.name_span(),
                            out.reborrow(),
                        )? = AliasState::Invalid;
                        frames.pop();
                        continue;
                    };
                    let target_expression =
                        self.ast_access(out.reborrow(), |ast_out| alias.target(ast_out))?;
                    if let Some(target) =
                        self.resolve_expression(target_expression, symbol.module(), out.reborrow())?
                    {
                        let Some(target_use) = self.graph.type_use(target) else {
                            self.diagnostics
                                .push(schema_internal!(target_expression.span()));
                            return Err(out.fail(BuildFailure::empty()));
                        };
                        let next = target_use.alias_chain();
                        let kind = target_use.kind();
                        let chain = self.push_alias_link(current, next, out.reborrow())?;
                        let resolved = self.push_type_use(
                            kind,
                            Some(chain),
                            target_expression.span(),
                            out.reborrow(),
                        )?;
                        *self.diagnostics.require(
                            self.graph.aliases.get_mut(current.index()),
                            target_expression.span(),
                            out.reborrow(),
                        )? = TypeSlot::resolved(Some(resolved));
                        *self.diagnostics.require(
                            states.get_mut(current.index()),
                            target_expression.span(),
                            out.reborrow(),
                        )? = AliasState::Resolved;
                    } else {
                        *self.diagnostics.require(
                            states.get_mut(current.index()),
                            target_expression.span(),
                            out.reborrow(),
                        )? = AliasState::Invalid;
                    }
                }
                frames.pop();
            }
        }
        Ok(())
    }

    fn resolve_expression<'slot>(
        &mut self,
        mut expression: TypeExpr<'_>,
        module: ModuleId,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, Option<TypeUseId>> {
        let mut frames = self
            .diagnostics
            .new_vec(self.context.execution(), out.reborrow())?;
        loop {
            self.context
                .consume(1, self.tree.root_span(), self.diagnostics, out.reborrow())?;
            let kind = self.ast_access(out.reborrow(), |ast_out| expression.kind(ast_out))?;
            let child = match kind {
                TypeExprKind::Option(inner) | TypeExprKind::Map { value: inner } => inner,
                TypeExprKind::Array { element, .. } => element,
                TypeExprKind::Builtin(_) | TypeExprKind::Path(_) => break,
            };
            frames.try_reserve(1, out.reborrow())?;
            frames.try_push(expression, out.reborrow())?;
            expression = child;
        }
        let mut resolved = self.finish_expression(expression, module, None, out.reborrow())?;
        while let Some(frame) = frames.pop() {
            self.context
                .consume(1, self.tree.root_span(), self.diagnostics, out.reborrow())?;
            if resolved.is_none() {
                break;
            }
            resolved = self.finish_expression(frame, module, resolved, out.reborrow())?;
        }
        Ok(resolved)
    }

    fn finish_expression<'slot>(
        &mut self,
        expression: TypeExpr<'_>,
        module: ModuleId,
        child: Option<TypeUseId>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, Option<TypeUseId>> {
        let span = expression.span();
        let kind = self.ast_access(out.reborrow(), |ast_out| expression.kind(ast_out))?;
        let resolved = match kind {
            TypeExprKind::Builtin(kind) => Some(self.push_type_use(
                ResolvedTypeKind::Builtin(kind),
                None,
                span,
                out.reborrow(),
            )?),
            TypeExprKind::Option(_) => {
                let Some(inner) = child else {
                    return Ok(None);
                };
                if matches!(
                    self.graph.type_use(inner).map(ResolvedTypeUse::kind),
                    Some(ResolvedTypeKind::Option(_))
                ) {
                    self.diagnostics.push(self.diagnostics.make_semantic(
                        diagnostic_message!(
                            "diagnostic.schema.nested_options_unsupported",
                            "nested options are unsupported after full expansion"
                        ),
                        span,
                    ));
                    None
                } else {
                    Some(self.push_type_use(
                        ResolvedTypeKind::Option(inner),
                        None,
                        span,
                        out.reborrow(),
                    )?)
                }
            }
            TypeExprKind::Array { length, .. } => {
                let Some(element) = child else {
                    return Ok(None);
                };
                let length = if let Some(length) = length {
                    let raw = self.ast_access(out.reborrow(), |ast_out| length.raw(ast_out))?;
                    self.context.consume(
                        raw.len(),
                        length.span(),
                        self.diagnostics,
                        out.reborrow(),
                    )?;
                    match raw.parse::<u32>() {
                        Ok(length) => Some(length),
                        Err(_) => {
                            self.diagnostics.push(self.diagnostics.make_semantic(
                                diagnostic_message!(
                                    "diagnostic.schema.fixed_array_length_out_of_range",
                                    "fixed-size array length exceeds the u32 range"
                                ),
                                length.span(),
                            ));
                            return Ok(None);
                        }
                    }
                } else {
                    None
                };
                Some(self.push_type_use(
                    ResolvedTypeKind::Array { element, length },
                    None,
                    span,
                    out.reborrow(),
                )?)
            }
            TypeExprKind::Map { .. } => {
                let Some(value) = child else {
                    return Ok(None);
                };
                Some(self.push_type_use(
                    ResolvedTypeKind::Map(value),
                    None,
                    span,
                    out.reborrow(),
                )?)
            }
            TypeExprKind::Path(path) => {
                let resolution = TypePathResolver::lookup::<BuildFailure>(
                    self.tree,
                    self.symbols,
                    module,
                    path,
                    out.reborrow(),
                )?;
                match resolution {
                    PathResolution::Resolved(target) => {
                        let Some(symbol) = self.symbols.type_symbol(target) else {
                            self.diagnostics.push(schema_internal!(path.span()));
                            return Err(out.fail(BuildFailure::empty()));
                        };
                        if symbol.state() != SymbolState::Valid {
                            None
                        } else {
                            match symbol.declaration() {
                                TypeDeclarationKind::Alias(_) => {
                                    let Some(target_use) = self.graph.alias_target(target) else {
                                        return Ok(None);
                                    };
                                    let Some(target_type) = self.graph.type_use(target_use) else {
                                        self.diagnostics.push(schema_internal!(path.span()));
                                        return Err(out.fail(BuildFailure::empty()));
                                    };
                                    Some(self.push_type_use(
                                        target_type.kind(),
                                        target_type.alias_chain(),
                                        path.span(),
                                        out.reborrow(),
                                    )?)
                                }
                                TypeDeclarationKind::Struct(_) | TypeDeclarationKind::Enum(_) => {
                                    Some(self.push_type_use(
                                        ResolvedTypeKind::User(target),
                                        None,
                                        path.span(),
                                        out.reborrow(),
                                    )?)
                                }
                            }
                        }
                    }
                    PathResolution::Ambiguous | PathResolution::Invalid => None,
                    problem => {
                        TypeGraphSupport::report(problem, path.span(), self.diagnostics);
                        None
                    }
                }
            }
        };
        if self.diagnostics.is_stopped() {
            return Err(out.fail(BuildFailure::empty()));
        }
        Ok(resolved)
    }

    fn push_type_use<'slot>(
        &mut self,
        kind: ResolvedTypeKind,
        alias_chain: Option<AliasLinkId>,
        span: SourceSpan,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, TypeUseId> {
        self.graph.uses.try_reserve(1, out.reborrow())?;
        let id = TypeUseId::new(self.graph.uses.len());
        self.graph.semantic_types.try_reserve(1, out.reborrow())?;
        let semantic_id = TypeId::new(self.graph.semantic_types.len());
        self.graph
            .semantic_types
            .try_push(ExpressionType::Resolved(id), out.reborrow())?;
        self.graph.uses.try_push(
            ResolvedTypeUse::new(kind, semantic_id, alias_chain, span),
            out.reborrow(),
        )?;
        Ok(id)
    }

    fn push_alias_link<'slot>(
        &mut self,
        symbol: TypeSymbolId,
        next: Option<AliasLinkId>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, AliasLinkId> {
        self.graph.alias_links.try_reserve(1, out.reborrow())?;
        let id = AliasLinkId::new(self.graph.alias_links.len());
        self.graph
            .alias_links
            .try_push(ResolvedAliasLink::new(symbol, next), out.reborrow())?;
        Ok(id)
    }
}
