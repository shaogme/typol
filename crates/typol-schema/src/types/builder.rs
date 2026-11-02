mod alias_resolution;
mod support;

use self::{
    alias_resolution::{AliasResolver, AliasResolverInput},
    support::TypeGraphSupport,
};
use crate::{
    context::CompileContext,
    diagnostics::{BuildFailure, DiagnosticCollector, EmergencyDiagnostics},
    expressions::ExpressionType,
    module_tree::{ModuleId, ModuleTree, TypeDeclarationKind},
    session::ObjectState,
    symbols::{FieldOwner, SymbolState, SymbolTable},
    types::{
        PathResolution, TypePathResolver,
        cycles::TypeCycles,
        model::{
            AliasLinkId, GraphData, ResolvedTypeGraph, ResolvedTypeKind, ResolvedTypeUse,
            ShapeCompilationInput, TypeSlot, TypeUseId,
        },
    },
};
use opeo::{OResult, Out};
use typol_core::SourceSpan;
use typol_i18n::diagnostic_message;
use typol_semantic::TypeId;
use typol_syntax::{AstAccessError, BuiltinType, TypeExpr, TypeExprKind, VariantPayloadKind};
pub(crate) struct TypeGraphBuilder<'a, 'd> {
    tree: &'a ModuleTree<'a>,
    symbols: &'a SymbolTable<'a>,
    diagnostics: &'d mut DiagnosticCollector,
    context: &'a CompileContext,
    graph: ResolvedTypeGraph,
}

impl<'a, 'd> TypeGraphBuilder<'a, 'd> {
    fn ast_access<'slot, 'borrow, T>(
        &self,
        out: Out<'slot, 'borrow, BuildFailure>,
        operation: impl for<'call> FnOnce(Out<'call, 'call, AstAccessError>) -> OResult<'call, T>,
    ) -> OResult<'slot, T> {
        self.context.ast_access(out, operation, BuildFailure::from)
    }

    pub(crate) fn build<'slot>(
        tree: &'a ModuleTree<'a>,
        symbols: &'a SymbolTable<'a>,
        context: &'a CompileContext,
        diagnostics: &'d mut DiagnosticCollector,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ResolvedTypeGraph> {
        let mut builder = Self::new(tree, symbols, context, diagnostics, out.reborrow())?;
        {
            let mut alias_resolver = AliasResolver::new(AliasResolverInput {
                tree,
                symbols,
                diagnostics: &mut *builder.diagnostics,
                context,
                graph: &mut builder.graph,
            });
            let (dependencies, mut alias_states) =
                alias_resolver.collect_alias_dependencies(out.reborrow())?;
            alias_resolver.resolve_aliases(&dependencies, &mut alias_states, out.reborrow())?;
        }
        builder.resolve_fields_and_payloads(out.reborrow())?;
        TypeCycles::new(
            tree,
            symbols,
            context,
            &mut builder.graph,
            builder.diagnostics,
        )
        .check(out.reborrow())?;
        if !builder.diagnostics.has_diagnostics() {
            builder.graph.set_shapes(
                ShapeCompilationInput {
                    symbols,
                    context,
                    span: tree.root_span(),
                    diagnostics: builder.diagnostics,
                },
                out.reborrow(),
            )?;
        }
        Ok(builder.graph)
    }

    pub(crate) fn new<'slot>(
        tree: &'a ModuleTree<'a>,
        symbols: &'a SymbolTable<'a>,
        context: &'a CompileContext,
        diagnostics: &'d mut DiagnosticCollector,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, Self> {
        let span = tree.root_span();
        let slots =
            TypeGraphSupport::empty_graph_slots(symbols, context, diagnostics, out.reborrow())?;
        let mut graph = ResolvedTypeGraph::new(GraphData {
            uses: diagnostics.new_vec(context.execution(), out.reborrow())?,
            semantic_types: diagnostics.new_vec(context.execution(), out.reborrow())?,
            shapes: diagnostics.new_vec(context.execution(), out.reborrow())?,
            named_ids: diagnostics.new_vec(context.execution(), out.reborrow())?,
            builtin_ids: diagnostics.new_vec(context.execution(), out.reborrow())?,
            variant_ids: diagnostics.new_vec(context.execution(), out.reborrow())?,
            declarations: diagnostics.new_vec(context.execution(), out.reborrow())?,
            alias_links: diagnostics.new_vec(context.execution(), out.reborrow())?,
            aliases: slots.aliases,
            fields: slots.fields,
            variant_payloads: slots.variant_payloads,
        });
        let builtins = [
            BuiltinType::I8,
            BuiltinType::I16,
            BuiltinType::I32,
            BuiltinType::I64,
            BuiltinType::I128,
            BuiltinType::U8,
            BuiltinType::U16,
            BuiltinType::U32,
            BuiltinType::U64,
            BuiltinType::U128,
            BuiltinType::F32,
            BuiltinType::F64,
            BuiltinType::String,
            BuiltinType::Bool,
        ];
        let count = symbols
            .types()
            .len()
            .checked_add(builtins.len())
            .and_then(|n| n.checked_add(symbols.variants().len()));
        let Some(count) = count else {
            diagnostics.push(EmergencyDiagnostics::resource(span));
            return Err(out.fail(BuildFailure::empty()));
        };
        graph
            .semantic_types
            .try_reserve_exact(count, out.reborrow())?;
        graph
            .named_ids
            .try_reserve_exact(symbols.types().len(), out.reborrow())?;
        graph
            .builtin_ids
            .try_reserve_exact(builtins.len(), out.reborrow())?;
        graph
            .variant_ids
            .try_reserve_exact(symbols.variants().len(), out.reborrow())?;
        for symbol in symbols.types() {
            context.consume(1, span, diagnostics, out.reborrow())?;
            let id = TypeId::new(graph.semantic_types.len());
            graph
                .semantic_types
                .try_push(ExpressionType::User(symbol.id()), out.reborrow())?;
            graph.named_ids.try_push(id, out.reborrow())?;
        }
        for builtin in builtins {
            context.consume(1, span, diagnostics, out.reborrow())?;
            let id = TypeId::new(graph.semantic_types.len());
            graph
                .semantic_types
                .try_push(ExpressionType::Builtin(builtin), out.reborrow())?;
            graph.builtin_ids.try_push(id, out.reborrow())?;
        }
        for variant in symbols.variants() {
            context.consume(1, span, diagnostics, out.reborrow())?;
            let id = TypeId::new(graph.semantic_types.len());
            graph.semantic_types.try_push(
                ExpressionType::VariantStructure(variant.id()),
                out.reborrow(),
            )?;
            graph.variant_ids.try_push(id, out.reborrow())?;
        }
        graph
            .declarations
            .try_reserve_exact(symbols.types().len(), out.reborrow())?;
        for _ in 0..symbols.types().len() {
            context.consume(1, span, diagnostics, out.reborrow())?;
            graph
                .declarations
                .try_push(ObjectState::Pending, out.reborrow())?;
        }
        graph
            .uses
            .try_reserve(symbols.types().len(), out.reborrow())?;
        Ok(Self {
            tree,
            symbols,
            diagnostics,
            context,
            graph,
        })
    }

    fn resolve_fields_and_payloads<'slot>(
        &mut self,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ()> {
        for field in self.symbols.fields() {
            self.context
                .consume(1, self.tree.root_span(), self.diagnostics, out.reborrow())?;
            if self.diagnostics.is_stopped() {
                return Err(out.fail(BuildFailure::empty()));
            }
            if field.state() != SymbolState::Valid {
                continue;
            }
            let owner = match field.owner() {
                FieldOwner::Type(id) => id,
                FieldOwner::Variant(id) => {
                    let Some(variant) = self.symbols.variant_symbol(id) else {
                        self.diagnostics
                            .push(schema_internal!(field.declaration().span()));
                        return Err(out.fail(BuildFailure::empty()));
                    };
                    variant.owner()
                }
            };
            let Some(owner) = self.symbols.type_symbol(owner) else {
                self.diagnostics
                    .push(schema_internal!(field.declaration().span()));
                return Err(out.fail(BuildFailure::empty()));
            };
            if owner.state() != SymbolState::Valid {
                continue;
            }
            let expression =
                self.ast_access(out.reborrow(), |ast_out| field.declaration().ty(ast_out))?;
            let resolved = self.resolve_expression(expression, owner.module(), out.reborrow())?;
            *self.diagnostics.require(
                self.graph.fields.get_mut(field.id().index()),
                field.declaration().span(),
                out.reborrow(),
            )? = TypeSlot::resolved(resolved);
        }

        for variant in self.symbols.variants() {
            self.context
                .consume(1, self.tree.root_span(), self.diagnostics, out.reborrow())?;
            if self.diagnostics.is_stopped() {
                return Err(out.fail(BuildFailure::empty()));
            }
            if variant.state() != SymbolState::Valid {
                continue;
            }
            let Some(owner) = self.symbols.type_symbol(variant.owner()) else {
                self.diagnostics
                    .push(schema_internal!(variant.declaration().span()));
                return Err(out.fail(BuildFailure::empty()));
            };
            if owner.state() != SymbolState::Valid {
                continue;
            }
            let payload = self.ast_access(out.reborrow(), |ast_out| {
                variant.declaration().payload(ast_out)
            })?;
            let payload_kind = self.ast_access(out.reborrow(), |ast_out| payload.kind(ast_out))?;
            if let VariantPayloadKind::Single(payload) = payload_kind {
                let expression = self.ast_access(out.reborrow(), |ast_out| payload.ty(ast_out))?;
                let resolved =
                    self.resolve_expression(expression, owner.module(), out.reborrow())?;
                *self.diagnostics.require(
                    self.graph.variant_payloads.get_mut(variant.id().index()),
                    variant.declaration().span(),
                    out.reborrow(),
                )? = TypeSlot::resolved(resolved);
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
}
