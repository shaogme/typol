mod builder;
mod cycles;
mod model;
#[cfg(test)]
mod tests;

use crate::{
    module_tree::{ModuleId, ModuleTree},
    session::ObjectState,
    symbols::{LookupResult, NamespaceTarget, SymbolState, SymbolTable, TypeSymbolId},
};
pub(crate) use builder::TypeGraphBuilder as TypeGraphCompiler;
pub(crate) use model::{ResolvedTypeGraph, ResolvedTypeKind, TypeShapeId, TypeUseId};
use opeo::{OResult, Out};
use typol_core::BudgetError;
use typol_syntax::{AstAccessError, TypePath};

#[derive(Clone, Copy)]
pub(crate) enum PathResolution {
    NotFound,
    WrongKind,
    Ambiguous,
    Invalid,
    Resolved(TypeSymbolId),
}

pub(crate) struct TypePathResolver;

impl TypePathResolver {
    pub(crate) fn lookup<'slot, E>(
        tree: &ModuleTree<'_>,
        symbols: &SymbolTable<'_>,
        start: ModuleId,
        path: TypePath<'_>,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, PathResolution>
    where
        E: From<BudgetError> + From<AstAccessError>,
    {
        let execution = symbols.context().execution();
        execution.consume::<E>(1, out.reborrow())?;
        let segments = symbols.context().ast_access(
            out.reborrow(),
            |ast_out| path.segments(ast_out),
            E::from,
        )?;
        let first = symbols.context().ast_access(
            out.reborrow(),
            |ast_out| segments.first(ast_out),
            E::from,
        )?;
        let Some(first) = first else {
            return Ok(PathResolution::NotFound);
        };
        let first_text =
            symbols
                .context()
                .ast_access(out.reborrow(), |ast_out| first.text(ast_out), E::from)?;

        let mut ancestor = Some(start);
        let mut first_target = None;
        while let Some(module) = ancestor {
            match symbols.lookup_namespace::<E>(module, first_text, out.reborrow())? {
                LookupResult::Found(target) => {
                    first_target = Some(target);
                    break;
                }
                LookupResult::Ambiguous => {
                    return Ok(PathResolution::Ambiguous);
                }
                LookupResult::Missing => {
                    execution.consume::<E>(1, out.reborrow())?;
                    ancestor = tree.module(module).and_then(|node| node.parent());
                }
            }
        }
        let Some(mut target) = first_target else {
            return Ok(PathResolution::NotFound);
        };

        if let NamespaceTarget::Module(id) = target {
            if tree
                .module(id)
                .is_some_and(|node| node.state() == ObjectState::Invalid)
            {
                return Ok(PathResolution::Invalid);
            }
        }
        if segments.len() == 1 {
            return Self::finish_type_target(target, symbols, out.reborrow());
        }
        let NamespaceTarget::Module(mut module) = target else {
            return Ok(PathResolution::WrongKind);
        };

        for index in 1..segments.len() {
            let segment = symbols.context().ast_access(
                out.reborrow(),
                |ast_out| segments.get(index, ast_out),
                E::from,
            )?;
            let Some(segment) = segment else {
                return Ok(PathResolution::NotFound);
            };
            execution.consume::<E>(1, out.reborrow())?;
            let segment_text = symbols.context().ast_access(
                out.reborrow(),
                |ast_out| segment.text(ast_out),
                E::from,
            )?;
            target = match symbols.lookup_namespace::<E>(module, segment_text, out.reborrow())? {
                LookupResult::Missing => {
                    return Ok(PathResolution::NotFound);
                }
                LookupResult::Ambiguous => {
                    return Ok(PathResolution::Ambiguous);
                }
                LookupResult::Found(target) => target,
            };
            if let NamespaceTarget::Module(id) = target {
                if tree
                    .module(id)
                    .is_some_and(|node| node.state() == ObjectState::Invalid)
                {
                    return Ok(PathResolution::Invalid);
                }
            }
            if index + 1 < segments.len() {
                module = match target {
                    NamespaceTarget::Module(module) => module,
                    NamespaceTarget::Type(_) => {
                        return Ok(PathResolution::WrongKind);
                    }
                };
            }
        }
        Self::finish_type_target(target, symbols, out.reborrow())
    }

    fn finish_type_target<'slot, E>(
        target: NamespaceTarget,
        symbols: &SymbolTable<'_>,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, PathResolution>
    where
        E: From<BudgetError>,
    {
        symbols
            .context()
            .execution()
            .consume::<E>(1, out.reborrow())?;
        let NamespaceTarget::Type(id) = target else {
            return Ok(PathResolution::WrongKind);
        };
        let Some(symbol) = symbols.type_symbol(id) else {
            return Ok(PathResolution::NotFound);
        };
        if symbol.state() == SymbolState::Ambiguous {
            Ok(PathResolution::Ambiguous)
        } else {
            Ok(PathResolution::Resolved(id))
        }
    }
}
