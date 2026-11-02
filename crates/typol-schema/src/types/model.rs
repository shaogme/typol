use crate::{
    context::CompileContext,
    diagnostics::{BuildFailure, DiagnosticCollector},
    expressions::ExpressionType,
    module_tree::TypeDeclarationKind,
    session::ObjectState,
    symbols::{FieldSymbolId, SymbolTable, TypeSymbolId, VariantSymbolId},
};
use core::{
    cmp::Ordering,
    mem,
    num::NonZeroUsize,
    ops::{Deref, DerefMut},
};
use opeo::{OResult, Out};
use typol_core::{BudgetError, ChargedVec, SourceSpan};
use typol_semantic::TypeId;
use typol_syntax::BuiltinType;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct TypeShapeId(NonZeroUsize);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum TypeShapeKind {
    Builtin(BuiltinType),
    Option(TypeShapeId),
    Array {
        element: TypeShapeId,
        length: Option<u32>,
    },
    Map(TypeShapeId),
    Struct(TypeId),
    Enum(TypeId),
}

struct TypeShapeIndexNode {
    kind: TypeShapeKind,
    shape: TypeShapeId,
    left: Option<usize>,
    right: Option<usize>,
    height: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TypeUseId(usize);

impl TypeUseId {
    pub(crate) fn new(index: usize) -> Self {
        Self(index)
    }

    pub(crate) const fn index(self) -> usize {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AliasLinkId(usize);

impl AliasLinkId {
    pub(crate) fn new(index: usize) -> Self {
        Self(index)
    }

    pub(crate) const fn index(self) -> usize {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ResolvedTypeKind {
    Builtin(BuiltinType),
    Option(TypeUseId),
    Array {
        element: TypeUseId,
        length: Option<u32>,
    },
    Map(TypeUseId),
    User(TypeSymbolId),
}

pub(crate) struct ResolvedTypeUse {
    kind: ResolvedTypeKind,
    semantic_id: TypeId,
    state: ObjectState,
    alias_chain: Option<AliasLinkId>,
    span: SourceSpan,
}

pub(crate) struct ResolvedAliasLink {
    symbol: TypeSymbolId,
    next: Option<AliasLinkId>,
}

#[derive(Clone, Copy)]
pub(crate) struct TypeSlot {
    state: ObjectState,
    target: Option<TypeUseId>,
}

pub(crate) struct GraphData {
    pub(crate) uses: ChargedVec<ResolvedTypeUse>,
    pub(crate) semantic_types: ChargedVec<ExpressionType>,
    pub(crate) shapes: ChargedVec<Option<TypeShapeId>>,
    pub(crate) named_ids: ChargedVec<TypeId>,
    pub(crate) builtin_ids: ChargedVec<TypeId>,
    pub(crate) variant_ids: ChargedVec<TypeId>,
    pub(crate) declarations: ChargedVec<ObjectState>,
    pub(crate) alias_links: ChargedVec<ResolvedAliasLink>,
    pub(crate) aliases: ChargedVec<TypeSlot>,
    pub(crate) fields: ChargedVec<TypeSlot>,
    pub(crate) variant_payloads: ChargedVec<TypeSlot>,
}

pub(crate) struct ResolvedTypeGraph {
    data: GraphData,
}

pub(crate) struct ShapeCompilationInput<'symbols, 'src, 'context, 'diagnostics> {
    pub(crate) symbols: &'symbols SymbolTable<'src>,
    pub(crate) context: &'context CompileContext,
    pub(crate) span: SourceSpan,
    pub(crate) diagnostics: &'diagnostics mut DiagnosticCollector,
}

struct TypeShapeBuilder<'graph, 'symbols, 'src, 'context, 'diagnostics> {
    graph: &'graph ResolvedTypeGraph,
    symbols: &'symbols SymbolTable<'src>,
    context: &'context CompileContext,
    span: SourceSpan,
    diagnostics: &'diagnostics mut DiagnosticCollector,
    shapes: ChargedVec<Option<TypeShapeId>>,
    index: ChargedVec<TypeShapeIndexNode>,
    index_root: Option<usize>,
}

impl Deref for ResolvedTypeGraph {
    type Target = GraphData;

    fn deref(&self) -> &GraphData {
        &self.data
    }
}

impl DerefMut for ResolvedTypeGraph {
    fn deref_mut(&mut self) -> &mut GraphData {
        &mut self.data
    }
}

impl TypeSlot {
    pub(crate) fn pending() -> Self {
        Self {
            state: ObjectState::Pending,
            target: None,
        }
    }

    pub(crate) fn state_mut(&mut self) -> &mut ObjectState {
        &mut self.state
    }

    pub(crate) fn resolved(target: Option<TypeUseId>) -> Self {
        Self {
            state: if target.is_some() {
                ObjectState::Valid
            } else {
                ObjectState::Invalid
            },
            target,
        }
    }

    pub(crate) fn target(self) -> Option<TypeUseId> {
        if self.state == ObjectState::Valid {
            self.target
        } else {
            None
        }
    }
}

impl ResolvedTypeGraph {
    pub(crate) fn new(data: GraphData) -> Self {
        Self { data }
    }

    pub(crate) fn alias_links(&self) -> &[ResolvedAliasLink] {
        &self.alias_links
    }

    pub(crate) fn set_shapes<'slot>(
        &mut self,
        input: ShapeCompilationInput<'_, '_, '_, '_>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ()> {
        let shapes = TypeShapeBuilder::new(self, input, out.reborrow())?.build(out.reborrow())?;
        self.shapes = shapes;
        Ok(())
    }

    pub(crate) fn shape_id(&self, ty: ExpressionType) -> Option<TypeShapeId> {
        self.semantic_id(ty)
            .and_then(|id| self.shapes.get(id.index()).copied())
            .flatten()
    }

    pub(crate) fn release_shape_ids(&mut self) {
        drop(mem::replace(&mut self.shapes, ChargedVec::unmetered()));
    }

    pub(crate) fn semantic_types(&self) -> &[ExpressionType] {
        &self.semantic_types
    }

    pub(crate) fn expression_type(&self, id: TypeId) -> Option<ExpressionType> {
        self.semantic_types.get(id.index()).copied()
    }

    pub(crate) fn semantic_id(&self, ty: ExpressionType) -> Option<TypeId> {
        match ty {
            ExpressionType::Resolved(id) => self.type_use(id).map(|u| u.semantic_id),
            ExpressionType::User(id) => self.named_ids.get(id.index()).copied(),
            ExpressionType::Builtin(builtin) => self.builtin_ids.get(builtin as usize).copied(),
            ExpressionType::VariantStructure(id) => self.variant_ids.get(id.index()).copied(),
        }
    }

    pub(crate) fn declaration_state(&self, id: TypeSymbolId) -> Option<ObjectState> {
        self.declarations.get(id.index()).copied()
    }

    pub(crate) fn alias_state(&self, id: TypeSymbolId) -> Option<ObjectState> {
        self.aliases.get(id.index()).map(|slot| slot.state)
    }

    pub(crate) fn field_state(&self, id: FieldSymbolId) -> Option<ObjectState> {
        self.fields.get(id.index()).map(|slot| slot.state)
    }

    pub(crate) fn variant_payload_state(&self, id: VariantSymbolId) -> Option<ObjectState> {
        self.variant_payloads.get(id.index()).map(|slot| slot.state)
    }

    pub(crate) fn type_use(&self, id: TypeUseId) -> Option<&ResolvedTypeUse> {
        self.uses
            .get(id.index())
            .filter(|item| item.state == ObjectState::Valid)
    }

    pub(crate) fn alias_target(&self, id: TypeSymbolId) -> Option<TypeUseId> {
        self.aliases
            .get(id.index())
            .copied()
            .and_then(TypeSlot::target)
            .filter(|id| self.type_use(*id).is_some())
    }

    pub(crate) fn field_type(&self, id: FieldSymbolId) -> Option<TypeUseId> {
        self.fields
            .get(id.index())
            .copied()
            .and_then(TypeSlot::target)
            .filter(|id| self.type_use(*id).is_some())
    }

    pub(crate) fn variant_payload_type(&self, id: VariantSymbolId) -> Option<TypeUseId> {
        self.variant_payloads
            .get(id.index())
            .copied()
            .and_then(TypeSlot::target)
            .filter(|id| self.type_use(*id).is_some())
    }

    pub(crate) fn alias_link(&self, id: AliasLinkId) -> Option<&ResolvedAliasLink> {
        self.alias_links.get(id.0)
    }
}

impl<'graph, 'symbols, 'src, 'context, 'diagnostics>
    TypeShapeBuilder<'graph, 'symbols, 'src, 'context, 'diagnostics>
{
    fn new<'slot>(
        graph: &'graph ResolvedTypeGraph,
        input: ShapeCompilationInput<'symbols, 'src, 'context, 'diagnostics>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, Self> {
        let ShapeCompilationInput {
            symbols,
            context,
            span,
            diagnostics,
        } = input;
        let mut shapes = diagnostics.new_vec(context.execution(), out.reborrow())?;
        let index = diagnostics.new_vec(context.execution(), out.reborrow())?;
        let count = graph.semantic_types.len();
        shapes.try_reserve_exact(count, out.reborrow())?;
        for _ in 0..count {
            context.consume(1, span, diagnostics, out.reborrow())?;
            shapes.try_push(None, out.reborrow())?;
        }
        Ok(Self {
            graph,
            symbols,
            context,
            span,
            diagnostics,
            shapes,
            index,
            index_root: None,
        })
    }

    fn build<'slot>(
        mut self,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ChargedVec<Option<TypeShapeId>>> {
        for (index, ty) in self.graph.semantic_types.iter().copied().enumerate() {
            self.context
                .consume(1, self.span, self.diagnostics, out.reborrow())?;
            let type_id = TypeId::new(index);
            match ty {
                ExpressionType::User(symbol_id) => {
                    let Some(symbol) = self.symbols.type_symbol(symbol_id) else {
                        return self.internal(out.reborrow());
                    };
                    let kind = match symbol.declaration() {
                        TypeDeclarationKind::Struct(_) => Some(TypeShapeKind::Struct(type_id)),
                        TypeDeclarationKind::Enum(_) => Some(TypeShapeKind::Enum(type_id)),
                        TypeDeclarationKind::Alias(_) => None,
                    };
                    if let Some(kind) = kind {
                        self.set_shape(type_id, kind, out.reborrow())?;
                    }
                }
                ExpressionType::Builtin(builtin) => {
                    self.set_shape(type_id, TypeShapeKind::Builtin(builtin), out.reborrow())?;
                }
                ExpressionType::VariantStructure(_) => {
                    self.set_shape(type_id, TypeShapeKind::Struct(type_id), out.reborrow())?;
                }
                ExpressionType::Resolved(use_id) => {
                    self.set_resolved_shape(type_id, use_id, out.reborrow())?;
                }
            }
        }
        for symbol in self.symbols.types() {
            self.context
                .consume(1, symbol.name_span(), self.diagnostics, out.reborrow())?;
            if matches!(symbol.declaration(), TypeDeclarationKind::Alias(_)) {
                let Some(target) = self.graph.alias_target(symbol.id()) else {
                    return self.internal(out.reborrow());
                };
                let Some(target_id) = self.graph.semantic_id(ExpressionType::Resolved(target))
                else {
                    return self.internal(out.reborrow());
                };
                let shape = self.shape_at(target_id, out.reborrow())?;
                let Some(alias_id) = self.graph.semantic_id(ExpressionType::User(symbol.id()))
                else {
                    return self.internal(out.reborrow());
                };
                self.set_shape_id(alias_id, shape, out.reborrow())?;
            }
        }
        Ok(self.shapes)
    }

    fn set_resolved_shape<'slot>(
        &mut self,
        id: TypeId,
        use_id: TypeUseId,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ()> {
        let Some(use_) = self.graph.type_use(use_id) else {
            return self.internal(out.reborrow());
        };
        let kind = match use_.kind() {
            ResolvedTypeKind::Builtin(builtin) => TypeShapeKind::Builtin(builtin),
            ResolvedTypeKind::User(symbol) => {
                let Some(target_id) = self.graph.semantic_id(ExpressionType::User(symbol)) else {
                    return self.internal(out.reborrow());
                };
                return self.copy_shape(id, target_id, out.reborrow());
            }
            ResolvedTypeKind::Option(child) => {
                let Some(child_id) = self.graph.type_use(child).map(ResolvedTypeUse::semantic_id)
                else {
                    return self.internal(out.reborrow());
                };
                TypeShapeKind::Option(self.shape_at(child_id, out.reborrow())?)
            }
            ResolvedTypeKind::Array { element, length } => {
                let Some(child_id) = self
                    .graph
                    .type_use(element)
                    .map(ResolvedTypeUse::semantic_id)
                else {
                    return self.internal(out.reborrow());
                };
                TypeShapeKind::Array {
                    element: self.shape_at(child_id, out.reborrow())?,
                    length,
                }
            }
            ResolvedTypeKind::Map(child) => {
                let Some(child_id) = self.graph.type_use(child).map(ResolvedTypeUse::semantic_id)
                else {
                    return self.internal(out.reborrow());
                };
                TypeShapeKind::Map(self.shape_at(child_id, out.reborrow())?)
            }
        };
        self.set_shape(id, kind, out.reborrow())
    }

    fn set_shape<'slot>(
        &mut self,
        id: TypeId,
        kind: TypeShapeKind,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ()> {
        let shape = self.intern(kind, out.reborrow())?;
        self.set_shape_id(id, shape, out.reborrow())
    }

    fn set_shape_id<'slot>(
        &mut self,
        id: TypeId,
        shape: TypeShapeId,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ()> {
        let Some(slot) = self.shapes.get_mut(id.index()) else {
            return self.internal(out.reborrow());
        };
        *slot = Some(shape);
        Ok(())
    }

    fn copy_shape<'slot>(
        &mut self,
        id: TypeId,
        target: TypeId,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ()> {
        let shape = self.shape_at(target, out.reborrow())?;
        self.set_shape_id(id, shape, out.reborrow())
    }

    fn shape_at<'slot>(
        &mut self,
        id: TypeId,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, TypeShapeId> {
        self.context
            .consume(1, self.span, self.diagnostics, out.reborrow())?;
        match self.shapes.get(id.index()).copied().flatten() {
            Some(shape) => Ok(shape),
            None => self.internal(out.reborrow()),
        }
    }

    fn intern<'slot>(
        &mut self,
        kind: TypeShapeKind,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, TypeShapeId> {
        let (root, shape) = self.insert(self.index_root, kind, out.reborrow())?;
        self.index_root = root;
        Ok(shape)
    }

    fn insert<'slot>(
        &mut self,
        root: Option<usize>,
        kind: TypeShapeKind,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, (Option<usize>, TypeShapeId)> {
        let Some(node_id) = root else {
            let Some(encoded) = self.index.len().checked_add(1).and_then(NonZeroUsize::new) else {
                return Err(out.fail(BuildFailure::from(BudgetError::CapacityExceeded)));
            };
            let node_id = self.index.len();
            let shape = TypeShapeId(encoded);
            self.index.try_push(
                TypeShapeIndexNode {
                    kind,
                    shape,
                    left: None,
                    right: None,
                    height: 1,
                },
                out.reborrow(),
            )?;
            return Ok((Some(node_id), shape));
        };
        self.context
            .consume(1, self.span, self.diagnostics, out.reborrow())?;
        let Some(node) = self.index.get(node_id) else {
            return self.internal(out.reborrow());
        };
        let node_kind = node.kind;
        let node_shape = node.shape;
        let left = node.left;
        let right = node.right;
        match node_kind.cmp(&kind) {
            Ordering::Equal => Ok((Some(node_id), node_shape)),
            Ordering::Less => {
                let (right, shape) = self.insert(right, kind, out.reborrow())?;
                let Some(node) = self.index.get_mut(node_id) else {
                    return self.internal(out.reborrow());
                };
                node.right = right;
                let root = self.rebalance(node_id, out.reborrow())?;
                Ok((Some(root), shape))
            }
            Ordering::Greater => {
                let (left, shape) = self.insert(left, kind, out.reborrow())?;
                let Some(node) = self.index.get_mut(node_id) else {
                    return self.internal(out.reborrow());
                };
                node.left = left;
                let root = self.rebalance(node_id, out.reborrow())?;
                Ok((Some(root), shape))
            }
        }
    }

    fn rebalance<'slot>(
        &mut self,
        node_id: usize,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, usize> {
        self.update_height(node_id, out.reborrow())?;
        let Some(node) = self.index.get(node_id) else {
            return self.internal(out.reborrow());
        };
        let left = node.left;
        let right = node.right;
        let left_height = self.node_height(left, out.reborrow())?;
        let right_height = self.node_height(right, out.reborrow())?;
        if left_height > right_height.saturating_add(1) {
            let Some(left_id) = left else {
                return self.internal(out.reborrow());
            };
            let Some((left_left, left_right)) =
                self.index.get(left_id).map(|node| (node.left, node.right))
            else {
                return self.internal(out.reborrow());
            };
            let left_left_height = self.node_height(left_left, out.reborrow())?;
            let left_right_height = self.node_height(left_right, out.reborrow())?;
            if left_right_height > left_left_height {
                let new_left = self.rotate_left(left_id, out.reborrow())?;
                let Some(node) = self.index.get_mut(node_id) else {
                    return self.internal(out.reborrow());
                };
                node.left = Some(new_left);
            }
            self.rotate_right(node_id, out.reborrow())
        } else if right_height > left_height.saturating_add(1) {
            let Some(right_id) = right else {
                return self.internal(out.reborrow());
            };
            let Some((right_left, right_right)) =
                self.index.get(right_id).map(|node| (node.left, node.right))
            else {
                return self.internal(out.reborrow());
            };
            let right_left_height = self.node_height(right_left, out.reborrow())?;
            let right_right_height = self.node_height(right_right, out.reborrow())?;
            if right_left_height > right_right_height {
                let new_right = self.rotate_right(right_id, out.reborrow())?;
                let Some(node) = self.index.get_mut(node_id) else {
                    return self.internal(out.reborrow());
                };
                node.right = Some(new_right);
            }
            self.rotate_left(node_id, out.reborrow())
        } else {
            Ok(node_id)
        }
    }

    fn rotate_left<'slot>(
        &mut self,
        node_id: usize,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, usize> {
        self.context
            .consume(1, self.span, self.diagnostics, out.reborrow())?;
        let Some(node) = self.index.get(node_id) else {
            return self.internal(out.reborrow());
        };
        let Some(pivot_id) = node.right else {
            return self.internal(out.reborrow());
        };
        let Some(pivot) = self.index.get(pivot_id) else {
            return self.internal(out.reborrow());
        };
        let middle = pivot.left;
        let Some(node) = self.index.get_mut(node_id) else {
            return self.internal(out.reborrow());
        };
        node.right = middle;
        let Some(pivot) = self.index.get_mut(pivot_id) else {
            return self.internal(out.reborrow());
        };
        pivot.left = Some(node_id);
        self.update_height(node_id, out.reborrow())?;
        self.update_height(pivot_id, out.reborrow())?;
        Ok(pivot_id)
    }

    fn rotate_right<'slot>(
        &mut self,
        node_id: usize,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, usize> {
        self.context
            .consume(1, self.span, self.diagnostics, out.reborrow())?;
        let Some(node) = self.index.get(node_id) else {
            return self.internal(out.reborrow());
        };
        let Some(pivot_id) = node.left else {
            return self.internal(out.reborrow());
        };
        let Some(pivot) = self.index.get(pivot_id) else {
            return self.internal(out.reborrow());
        };
        let middle = pivot.right;
        let Some(node) = self.index.get_mut(node_id) else {
            return self.internal(out.reborrow());
        };
        node.left = middle;
        let Some(pivot) = self.index.get_mut(pivot_id) else {
            return self.internal(out.reborrow());
        };
        pivot.right = Some(node_id);
        self.update_height(node_id, out.reborrow())?;
        self.update_height(pivot_id, out.reborrow())?;
        Ok(pivot_id)
    }

    fn update_height<'slot>(
        &mut self,
        node_id: usize,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ()> {
        self.context
            .consume(1, self.span, self.diagnostics, out.reborrow())?;
        let Some(node) = self.index.get(node_id) else {
            return self.internal(out.reborrow());
        };
        let left = node.left;
        let right = node.right;
        let height = self
            .node_height(left, out.reborrow())?
            .max(self.node_height(right, out.reborrow())?)
            .saturating_add(1);
        let Some(node) = self.index.get_mut(node_id) else {
            return self.internal(out.reborrow());
        };
        node.height = height;
        Ok(())
    }

    fn node_height<'slot>(
        &mut self,
        node_id: Option<usize>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, u16> {
        self.context
            .consume(1, self.span, self.diagnostics, out.reborrow())?;
        let Some(node_id) = node_id else {
            return Ok(0);
        };
        let Some(node) = self.index.get(node_id) else {
            return self.internal(out.reborrow());
        };
        Ok(node.height)
    }

    fn internal<'slot, T>(&mut self, out: Out<'slot, '_, BuildFailure>) -> OResult<'slot, T> {
        self.diagnostics.push(schema_internal!(self.span));
        Err(out.fail(BuildFailure::empty()))
    }
}

impl ResolvedTypeUse {
    pub(crate) fn new(
        kind: ResolvedTypeKind,
        semantic_id: TypeId,
        alias_chain: Option<AliasLinkId>,
        span: SourceSpan,
    ) -> Self {
        Self {
            kind,
            semantic_id,
            alias_chain,
            span,
            state: ObjectState::Valid,
        }
    }

    pub(crate) fn state(&self) -> ObjectState {
        self.state
    }

    pub(crate) fn invalidate(&mut self) {
        self.state = ObjectState::Invalid;
    }

    pub(crate) const fn kind(&self) -> ResolvedTypeKind {
        self.kind
    }

    pub(crate) const fn semantic_id(&self) -> TypeId {
        self.semantic_id
    }

    pub(crate) const fn alias_chain(&self) -> Option<AliasLinkId> {
        self.alias_chain
    }

    pub(crate) const fn span(&self) -> SourceSpan {
        self.span
    }
}

impl ResolvedAliasLink {
    pub(crate) fn new(symbol: TypeSymbolId, next: Option<AliasLinkId>) -> Self {
        Self { symbol, next }
    }

    pub(crate) const fn symbol(&self) -> TypeSymbolId {
        self.symbol
    }

    pub(crate) const fn next(&self) -> Option<AliasLinkId> {
        self.next
    }
}
