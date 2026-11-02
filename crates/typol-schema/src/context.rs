use core::{cell::RefCell, cmp::Ordering};
use typol_i18n::diagnostic_message;

use opeo::{ErrSlot, OResult, Out};
use typol_core::{BudgetError, Diagnostic, ExecutionBudget, SourceSpan};
use typol_semantic::{BuildError, EvalFailure};
use typol_syntax::AstAccessError;

use crate::{
    SchemaLimits,
    diagnostics::{BudgetDiagnostics, BuildFailure, DiagnosticCollector, EmergencyDiagnostics},
};

#[cfg(test)]
use crate::test_support::TestOperation;
#[cfg(test)]
use typol_core::{DiagnosticCode, ExecutionLimits, SourceId, Span};
#[cfg(test)]
use typol_test_support::*;

/// All compilation phases explicitly share one execution budget; the diagnostic collector only receives failures.
pub(crate) struct CompileContext {
    limits: SchemaLimits,
    execution: ExecutionBudget,
    ast_access_errors: RefCell<ErrSlot<AstAccessError>>,
    budget_errors: RefCell<ErrSlot<BudgetError>>,
    builder_errors: RefCell<ErrSlot<BuildError>>,
}

impl CompileContext {
    pub(crate) fn new<'slot, E>(
        limits: SchemaLimits,
        execution: &ExecutionBudget,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, Self>
    where
        E: From<BudgetError>,
    {
        let execution = execution.try_clone(out.reborrow())?;
        Ok(Self {
            limits,
            execution,
            ast_access_errors: RefCell::new(ErrSlot::new()),
            budget_errors: RefCell::new(ErrSlot::new()),
            builder_errors: RefCell::new(ErrSlot::new()),
        })
    }

    pub(crate) fn limits(&self) -> &SchemaLimits {
        &self.limits
    }

    pub(crate) fn execution(&self) -> &ExecutionBudget {
        &self.execution
    }

    pub(crate) fn ast_access<'slot, 'borrow, T, E>(
        &self,
        out: Out<'slot, 'borrow, E>,
        operation: impl for<'call> FnOnce(Out<'call, 'call, AstAccessError>) -> OResult<'call, T>,
        convert: impl FnOnce(AstAccessError) -> E,
    ) -> OResult<'slot, T> {
        match self.ast_access_errors.borrow_mut().try_call(operation) {
            Ok(value) => Ok(value),
            Err(caught) => Err(out.fail(convert(caught.take()))),
        }
    }

    pub(crate) fn budget_access<'slot, 'borrow, T>(
        &self,
        out: Out<'slot, 'borrow, BuildFailure>,
        span: SourceSpan,
        diagnostics: &mut DiagnosticCollector,
        operation: impl for<'call> FnOnce(Out<'call, 'call, BudgetError>) -> OResult<'call, T>,
    ) -> OResult<'slot, T> {
        match self.budget_errors.borrow_mut().try_call(operation) {
            Ok(value) => Ok(value),
            Err(caught) => {
                diagnostics.abort(BudgetDiagnostics::make(caught.take(), span));
                Err(out.fail(BuildFailure::empty()))
            }
        }
    }

    pub(crate) fn builder_access<'slot, 'borrow, T>(
        &self,
        out: Out<'slot, 'borrow, BuildFailure>,
        span: SourceSpan,
        diagnostics: &mut DiagnosticCollector,
        operation: impl for<'call> FnOnce(Out<'call, 'call, BuildError>) -> OResult<'call, T>,
    ) -> OResult<'slot, T> {
        match self.builder_errors.borrow_mut().try_call(operation) {
            Ok(value) => Ok(value),
            Err(caught) => {
                let diagnostic = builder_diagnostic(caught.take(), span, diagnostics);
                diagnostics.push(diagnostic);
                Err(out.fail(BuildFailure::empty()))
            }
        }
    }

    pub(crate) fn compare_text<'slot, E>(
        &self,
        left: &str,
        right: &str,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, Ordering>
    where
        E: From<BudgetError>,
    {
        self.execution.consume(1, out.reborrow())?;
        self.execution.compare_text(left, right, out.reborrow())
    }

    pub(crate) fn consume<'slot>(
        &self,
        steps: usize,
        span: SourceSpan,
        diagnostics: &mut DiagnosticCollector,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ()> {
        if diagnostics.is_stopped() {
            return Err(out.fail(BuildFailure::empty()));
        }
        self.execution.consume_with(steps, out.reborrow(), |error| {
            diagnostics.abort(BudgetDiagnostics::make(error, span));
            BuildFailure::empty()
        })
    }

    /// In-place heap sorting allocates no additional payload, and each comparison and swap can propagate a budget failure.
    pub(crate) fn sort<'slot, T, E>(
        &self,
        values: &mut [T],
        mut compare: impl for<'call> FnMut(&T, &T, Out<'slot, 'call, E>) -> OResult<'slot, Ordering>,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, ()>
    where
        E: From<BudgetError>,
    {
        self.execution.consume(1, out.reborrow())?;
        for root in (0..values.len() / 2).rev() {
            self.sift(values, root, &mut compare, out.reborrow())?;
        }
        for end in (1..values.len()).rev() {
            self.execution.consume(2, out.reborrow())?;
            if values.is_empty() {
                return Err(out.fail(E::from(BudgetError::invariant(schema_context_ref!(
                    "sorting",
                    "first-slot-missing"
                )))));
            }
            if values.get(end).is_none() {
                return Err(out.fail(E::from(BudgetError::invariant(schema_context_ref!(
                    "sorting",
                    "end-slot-missing"
                )))));
            }
            values.swap(0, end);
            let Some(prefix) = values.get_mut(..end) else {
                return Err(out.fail(E::from(BudgetError::invariant(schema_context_ref!(
                    "sorting",
                    "prefix-range-missing"
                )))));
            };
            self.sift(prefix, 0, &mut compare, out.reborrow())?;
        }
        Ok(())
    }

    fn sift<'slot, T, E>(
        &self,
        values: &mut [T],
        mut root: usize,
        compare: &mut impl for<'call> FnMut(&T, &T, Out<'slot, 'call, E>) -> OResult<'slot, Ordering>,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, ()>
    where
        E: From<BudgetError>,
    {
        while root < values.len() / 2 {
            self.execution.consume(1, out.reborrow())?;
            let Some(mut child) = root.checked_mul(2).and_then(|index| index.checked_add(1)) else {
                return Err(out.fail(E::from(BudgetError::invariant(schema_context_ref!(
                    "sorting",
                    "left-child-overflow"
                )))));
            };
            let Some(right) = child.checked_add(1) else {
                return Err(out.fail(E::from(BudgetError::invariant(schema_context_ref!(
                    "sorting",
                    "right-child-overflow"
                )))));
            };
            if values.get(right).is_some() {
                self.execution.consume(1, out.reborrow())?;
                let Some(left_value) = values.get(child) else {
                    return Err(out.fail(E::from(BudgetError::invariant(schema_context_ref!(
                        "sorting",
                        "left-child-slot-missing"
                    )))));
                };
                let Some(right_value) = values.get(right) else {
                    return Err(out.fail(E::from(BudgetError::invariant(schema_context_ref!(
                        "sorting",
                        "right-child-slot-missing"
                    )))));
                };
                if compare(left_value, right_value, out.reborrow())? == Ordering::Less {
                    child += 1;
                }
            }
            self.execution.consume(1, out.reborrow())?;
            let Some(root_value) = values.get(root) else {
                return Err(out.fail(E::from(BudgetError::invariant(schema_context_ref!(
                    "sorting",
                    "root-slot-missing"
                )))));
            };
            let Some(child_value) = values.get(child) else {
                return Err(out.fail(E::from(BudgetError::invariant(schema_context_ref!(
                    "sorting",
                    "child-slot-missing"
                )))));
            };
            if compare(root_value, child_value, out.reborrow())? != Ordering::Less {
                break;
            }
            self.execution.consume(2, out.reborrow())?;
            if values.get(root).is_none() {
                return Err(out.fail(E::from(BudgetError::invariant(schema_context_ref!(
                    "sorting",
                    "swap-root-slot-missing"
                )))));
            }
            if values.get(child).is_none() {
                return Err(out.fail(E::from(BudgetError::invariant(schema_context_ref!(
                    "sorting",
                    "swap-child-slot-missing"
                )))));
            }
            values.swap(root, child);
            root = child;
        }
        Ok(())
    }
}

fn builder_diagnostic(
    error: BuildError,
    span: SourceSpan,
    diagnostics: &DiagnosticCollector,
) -> Diagnostic {
    let span = error.source_span().unwrap_or(span);
    if let Some(access) = error.schema_access() {
        EmergencyDiagnostics::internal(span, EmergencyDiagnostics::schema_access_context(access))
    } else if let Some(context) = error.context() {
        EmergencyDiagnostics::internal(span, context)
    } else {
        match error {
            BuildError::Allocation => EmergencyDiagnostics::allocation(span),
            BuildError::Resource => EmergencyDiagnostics::resource(span),
            BuildError::Capacity => EmergencyDiagnostics::capacity(span),
            BuildError::Internal
            | BuildError::InternalAt { .. }
            | BuildError::SchemaAccess(_)
            | BuildError::SchemaAccessAt { .. } => schema_internal!(span),
            BuildError::DefaultConstraintAt { .. } => diagnostics.make_semantic(
                diagnostic_message!(
                    "diagnostic.schema.default_constraint_failed",
                    "schema default does not satisfy the constraint"
                ),
                span,
            ),
            BuildError::DefaultEvaluationAt { failure, .. } => match failure {
                EvalFailure::Arithmetic => diagnostics.make_semantic(
                    diagnostic_message!(
                        "diagnostic.schema.default_numeric_evaluation_failed",
                        "schema default numeric evaluation failed"
                    ),
                    span,
                ),
                EvalFailure::Resource => EmergencyDiagnostics::resource(span),
                EvalFailure::Allocation => EmergencyDiagnostics::allocation(span),
                EvalFailure::Capacity => EmergencyDiagnostics::capacity(span),
                EvalFailure::Internal
                | EvalFailure::InternalAt(_)
                | EvalFailure::SchemaAccess(_) => {
                    schema_internal!(span)
                }
            },
        }
    }
}

#[cfg(test)]
#[test]
fn freeze_default_failures_map_to_semantic_diagnostics() -> TestResult {
    let mut operations = TestOperation::new();
    let execution = operations
        .budget(|out| ExecutionBudget::new(ExecutionLimits::new(10_000, 10_000), out))
        .into_test_value()?;
    let span = operations
        .span(|out| Span::new(0, 1, out))
        .into_test_value()?;
    let span = SourceSpan::new(SourceId::new(0), span);
    let diagnostics = operations
        .diagnostic(|out| DiagnosticCollector::new(4, span, &execution, out))
        .into_test_value()?;

    let constraint = builder_diagnostic(
        BuildError::DefaultConstraintAt { source_span: span },
        span,
        &diagnostics,
    );
    test_verify_eq!(constraint.code(), DiagnosticCode::SCHEMA_SEMANTIC_ERROR);
    test_verify_eq!(
        constraint.message(),
        "schema default does not satisfy the constraint"
    );
    test_verify_eq!(constraint.primary(), Some(span));

    let evaluation = builder_diagnostic(
        BuildError::DefaultEvaluationAt {
            failure: EvalFailure::Arithmetic,
            source_span: span,
        },
        span,
        &diagnostics,
    );
    test_verify_eq!(evaluation.code(), DiagnosticCode::SCHEMA_SEMANTIC_ERROR);
    test_verify_eq!(
        evaluation.message(),
        "schema default numeric evaluation failed"
    );
    test_verify_eq!(evaluation.primary(), Some(span));

    Ok(())
}
