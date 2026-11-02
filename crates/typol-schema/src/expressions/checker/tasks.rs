use crate::{
    context::CompileContext,
    diagnostics::{BuildFailure, DiagnosticCollector},
    expressions::{
        ExpressionNodeId, ExpressionType, compile_support::Context, methods::MethodPlan,
    },
    symbols::SymbolTable,
    types::ResolvedTypeGraph,
};
use opeo::{OResult, Out};
use typol_core::ChargedVec;
use typol_syntax::Expr;

pub(super) struct EnterTask<'a> {
    pub(super) expression: Expr<'a>,
    pub(super) expected: Option<ExpressionType>,
    pub(super) allow_minimum: bool,
}

pub(super) enum CheckTask<'a> {
    Enter(EnterTask<'a>),
    Finish(Expr<'a>, Option<ExpressionType>),
    BinaryFirst {
        expression: Expr<'a>,
        expected: Option<ExpressionType>,
        reversed: bool,
    },
    BinaryLast {
        expression: Expr<'a>,
        expected: Option<ExpressionType>,
        first: ExpressionNodeId,
        reversed: bool,
    },
    MethodReceiver(Expr<'a>, Option<ExpressionType>),
    MethodLast {
        expression: Expr<'a>,
        expected: Option<ExpressionType>,
        plan: MethodPlan<'a>,
    },
}

pub(crate) struct ExpressionCompileInput<'source, 'context, 'diagnostics> {
    pub(crate) expression: Expr<'source>,
    pub(crate) context: Context,
    pub(crate) symbols: &'context SymbolTable<'source>,
    pub(crate) types: &'context ResolvedTypeGraph,
    pub(crate) diagnostics: &'diagnostics mut DiagnosticCollector,
}

pub(super) struct CheckTaskStack;

impl<'a> CheckTask<'a> {
    pub(super) fn enter(
        expression: Expr<'a>,
        expected: Option<ExpressionType>,
        allow_minimum: bool,
    ) -> Self {
        Self::Enter(EnterTask {
            expression,
            expected,
            allow_minimum,
        })
    }
}

impl CheckTaskStack {
    pub(super) fn allocate_tasks<'slot, 'a>(
        context: &CompileContext,
        diagnostics: &mut DiagnosticCollector,
        out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ChargedVec<CheckTask<'a>>> {
        diagnostics.new_vec(context.execution(), out)
    }

    pub(super) fn push<'slot, 'a>(
        tasks: &mut ChargedVec<CheckTask<'a>>,
        task: CheckTask<'a>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ()> {
        tasks.try_reserve(1, out.reborrow())?;
        tasks.try_push(task, out.reborrow())?;
        Ok(())
    }
}
