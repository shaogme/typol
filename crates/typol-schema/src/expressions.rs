mod checker;
mod compile_support;
mod compiler;
mod methods;
mod numeric;
mod type_analysis;

pub(crate) use compiler::{ExpressionCompileInput, ExpressionCompiler};

pub(crate) use typol_semantic::{
    AssertionInfo as CompiledAll, AssertionInfo as CompiledAssertion, ClosureLocalId,
    ExpressionBinding, ExpressionMethod, ExpressionNodeId, TypedExpression as CompiledExpression,
    TypedExpressionKind, TypedExpressionNode, TypedLiteral, TypedMethodArgument,
};

use core::cmp::Ordering;

use opeo::{OResult, Out, ResultOutExt};
use typol_core::{ChargedVec, SourceSpan};
use typol_model::BuiltinType;

use crate::{
    diagnostics::BuildFailure,
    symbols::{TypeSymbolId, VariantSymbolId},
    types::TypeUseId,
};

#[derive(Clone, Copy)]
pub(crate) enum ExpressionType {
    Resolved(TypeUseId),
    Builtin(BuiltinType),
    User(TypeSymbolId),
    VariantStructure(VariantSymbolId),
}

pub(crate) struct ExpressionParts {
    pub(crate) type_assertions: ChargedVec<ChargedVec<CompiledAssertion>>,
    pub(crate) variant_assertions: ChargedVec<ChargedVec<CompiledAssertion>>,
    pub(crate) all_programs: ChargedVec<CompiledAll>,
}

pub(crate) struct ExpressionCompilation {
    type_assertions: ChargedVec<ChargedVec<CompiledAssertion>>,
    variant_assertions: ChargedVec<ChargedVec<CompiledAssertion>>,
    all_constraints: ChargedVec<CompiledAll>,
    invalid_all_constraints: ChargedVec<SourceSpan>,
}

impl ExpressionCompilation {
    pub(crate) fn all_index<'slot>(
        &self,
        span: SourceSpan,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, Option<usize>> {
        let mut low = 0;
        let mut high = self.all_constraints.len();
        while low < high {
            if let Some(budget) = self.all_constraints.budget() {
                budget.consume(1, out.reborrow())?;
            }
            let mid = low + (high - low) / 2;
            let constraint = self
                .all_constraints
                .get(mid)
                .ok_or_else(|| schema_invariant!("expression-index", "all-constraint-slot-missing"))
                .store_err_into(out.reborrow())?;
            match span_key(constraint.span()).cmp(&span_key(span)) {
                Ordering::Less => low = mid + 1,
                Ordering::Greater => high = mid,
                Ordering::Equal => return Ok(Some(mid)),
            }
        }
        Ok(None)
    }

    pub(crate) fn into_parts(self) -> ExpressionParts {
        ExpressionParts {
            type_assertions: self.type_assertions,
            variant_assertions: self.variant_assertions,
            all_programs: self.all_constraints,
        }
    }

    pub(crate) fn new(
        type_assertions: ChargedVec<ChargedVec<CompiledAssertion>>,
        variant_assertions: ChargedVec<ChargedVec<CompiledAssertion>>,
        all_constraints: ChargedVec<CompiledAll>,
        invalid_all_constraints: ChargedVec<SourceSpan>,
    ) -> Self {
        Self {
            type_assertions,
            variant_assertions,
            all_constraints,
            invalid_all_constraints,
        }
    }

    pub(crate) fn type_assertions_mut(
        &mut self,
        id: TypeSymbolId,
    ) -> Option<&mut ChargedVec<CompiledAssertion>> {
        self.type_assertions.get_mut(id.index())
    }

    pub(crate) fn variant_assertions_mut(
        &mut self,
        id: VariantSymbolId,
    ) -> Option<&mut ChargedVec<CompiledAssertion>> {
        self.variant_assertions.get_mut(id.index())
    }

    pub(crate) fn all_constraints_mut(&mut self) -> &mut ChargedVec<CompiledAll> {
        &mut self.all_constraints
    }

    pub(crate) fn invalid_all_constraints_mut(&mut self) -> &mut ChargedVec<SourceSpan> {
        &mut self.invalid_all_constraints
    }

    pub(crate) fn all_constraint<'slot>(
        &self,
        span: SourceSpan,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, Option<&CompiledAll>> {
        match self.all_index(span, out.reborrow())? {
            Some(index) => {
                let constraint = self
                    .all_constraints
                    .get(index)
                    .map(Some)
                    .ok_or_else(|| {
                        schema_invariant!("expression-index", "all-constraint-result-missing")
                    })
                    .store_err_into(out.reborrow())?;
                Ok(constraint)
            }
            None => Ok(None),
        }
    }

    pub(crate) fn all_constraint_is_invalid<'slot>(
        &self,
        span: SourceSpan,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, bool> {
        let key = span_key(span);
        let mut low = 0;
        let mut high = self.invalid_all_constraints.len();
        while low < high {
            if let Some(budget) = self.invalid_all_constraints.budget() {
                budget.consume(1, out.reborrow())?;
            }
            let mid = low + (high - low) / 2;
            let invalid_span = self
                .invalid_all_constraints
                .get(mid)
                .copied()
                .ok_or_else(|| {
                    schema_invariant!("expression-index", "invalid-constraint-slot-missing")
                })
                .store_err_into(out.reborrow())?;
            match span_key(invalid_span).cmp(&key) {
                Ordering::Less => low = mid + 1,
                Ordering::Greater => high = mid,
                Ordering::Equal => return Ok(true),
            }
        }
        Ok(false)
    }
}

fn span_key(span: SourceSpan) -> (u32, usize) {
    (span.source_id().get(), span.span().start())
}
