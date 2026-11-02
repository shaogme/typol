use crate::{
    diagnostics::{BudgetDiagnostic, DiagnosticFactory},
    value_validation::TreeContext,
};
use opeo::{ErrSlot, OResult, Out};
use typol_core::{
    BudgetError, ChargedVec, Diagnostic, DiagnosticCode, DiagnosticContext, LocatedSpan,
    ValueSource, ValueView, diagnostic_context,
};
use typol_semantic::SchemaAccessError;

pub(super) static VALUE_TREE_SCHEMA_ACCESS: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "value-tree-validation",
    "compiled-schema-view-unavailable",
);

pub(super) struct TreeSupport;

impl TreeSupport {
    pub(super) fn push_task<'slot, 'borrow, T>(
        tasks: &mut ChargedVec<T>,
        errors: &mut ErrSlot<BudgetError>,
        task: T,
        out: Out<'slot, 'borrow, Diagnostic>,
    ) -> OResult<'slot, ()> {
        Self::budget_call(errors, |budget_out| tasks.try_push(task, budget_out), out)
    }

    pub(super) fn budget_call<'slot, 'borrow, T>(
        errors: &mut ErrSlot<BudgetError>,
        operation: impl for<'call> FnOnce(Out<'call, 'call, BudgetError>) -> OResult<'call, T>,
        out: Out<'slot, 'borrow, Diagnostic>,
    ) -> OResult<'slot, T> {
        match errors.try_call(operation) {
            Ok(value) => Ok(value),
            Err(caught) => Err(out.fail(BudgetDiagnostic::from_error(caught.take()))),
        }
    }

    pub(super) fn schema_call<'slot, 'borrow, T>(
        errors: &mut ErrSlot<SchemaAccessError>,
        operation: impl for<'call> FnOnce(Out<'call, 'call, SchemaAccessError>) -> OResult<'call, T>,
        out: Out<'slot, 'borrow, Diagnostic>,
    ) -> OResult<'slot, T> {
        match errors.try_call(operation) {
            Ok(value) => Ok(value),
            Err(caught) => Err(out.fail(DiagnosticFactory::internal(
                VALUE_TREE_SCHEMA_ACCESS.with_object_id(caught.take().index() as u64),
                None,
                None,
                None,
            ))),
        }
    }

    pub(super) fn source(
        context: &TreeContext<'_, '_, '_>,
        value: ValueView<'_>,
    ) -> Option<ValueSource> {
        context.current_node.map(|_| value.source())
    }

    pub(super) fn enter_nesting<'slot, 'borrow>(
        context: &mut TreeContext<'_, '_, '_>,
        span: Option<LocatedSpan>,
        source: Option<ValueSource>,
        out: Out<'slot, 'borrow, Diagnostic>,
    ) -> OResult<'slot, ()> {
        let Some(depth) = context.nesting_depth.checked_add(1) else {
            return Err(out.fail(DiagnosticFactory::make_diagnostic(
                DiagnosticCode::RESOURCE_LIMIT_EXCEEDED,
                "value tree nesting depth exceeded the limit",
                span,
                None,
                source,
            )));
        };
        context.nesting_depth = depth;
        Ok(())
    }

    pub(super) fn check_depth<'slot, 'borrow>(
        context: &TreeContext<'_, '_, '_>,
        depth: usize,
        span: Option<LocatedSpan>,
        source: Option<ValueSource>,
        out: Out<'slot, 'borrow, Diagnostic>,
    ) -> OResult<'slot, ()> {
        if context.resources.value_limits.allows_depth(depth) {
            return Ok(());
        }
        Err(out.fail(DiagnosticFactory::make_diagnostic(
            DiagnosticCode::RESOURCE_LIMIT_EXCEEDED,
            "value tree nesting depth exceeded the limit",
            span,
            None,
            source,
        )))
    }

    pub(super) fn leave_nesting(context: &mut TreeContext<'_, '_, '_>) {
        context.nesting_depth -= 1;
    }

    pub(super) fn reserve_tasks<'slot, T>(
        tasks: &mut ChargedVec<T>,
        errors: &mut ErrSlot<BudgetError>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ()> {
        Self::budget_call(
            errors,
            |budget_out| tasks.try_reserve(5, budget_out),
            out.reborrow(),
        )
    }
}
