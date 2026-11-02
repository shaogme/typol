use crate::{
    annotations::{CompiledConstraint, TypedNumber},
    defaults::{
        ConstraintLabelInput, DefaultDiagnostic,
        compiler::{BuiltValue, DefaultData},
    },
    diagnostics::BuildFailure,
    execution::CompileModel,
    symbols::FieldSymbolId,
    types::{ResolvedTypeKind, TypeUseId},
};
use core::{cell::RefCell, ops::Deref};
use opeo::{ErrSlot, OResult, Out};
use typol_core::{DiagnosticMessage, SourceSpan, ValueKind};
use typol_i18n::diagnostic_message;
use typol_semantic::{
    AllItemRequest, AllItemValue, EvalError, Evaluator, SumBoundsRequest, ValueRules,
};

pub(super) struct UniqueRuleInput<'a> {
    pub(super) value: &'a BuiltValue,
    pub(super) target: TypeUseId,
    pub(super) by: Option<&'a [FieldSymbolId]>,
    pub(super) constraint_span: SourceSpan,
}

pub(super) struct SumRuleInput<'a> {
    pub(super) value: &'a BuiltValue,
    pub(super) target: TypeUseId,
    pub(super) bounds: (Option<TypedNumber>, Option<TypedNumber>),
    pub(super) constraint_span: SourceSpan,
}

pub(super) struct DefaultRuleChecker<'borrow, 'data, 'diagnostics> {
    data: &'borrow mut DefaultData<'data, 'diagnostics>,
    evaluation_errors: &'borrow RefCell<ErrSlot<EvalError>>,
}

impl<'borrow, 'data, 'diagnostics> DefaultRuleChecker<'borrow, 'data, 'diagnostics> {
    pub(super) fn new(
        data: &'borrow mut DefaultData<'data, 'diagnostics>,
        evaluation_errors: &'borrow RefCell<ErrSlot<EvalError>>,
    ) -> Self {
        Self {
            data,
            evaluation_errors,
        }
    }

    pub(super) fn check_unique<'slot>(
        &mut self,
        input: UniqueRuleInput<'_>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, bool> {
        let UniqueRuleInput {
            value,
            target,
            by,
            constraint_span,
        } = input;
        let ValueKind::Array(_) = value.value.as_ref() else {
            return self.constraint_failure(
                diagnostic_message!(
                    "diagnostic.schema.default_unique_constraint_failed",
                    "schema default does not satisfy the @unique constraint"
                ),
                value.span,
                constraint_span,
                out.reborrow(),
            );
        };
        let Some(ResolvedTypeKind::Array { element, .. }) =
            self.data.types.type_use(target).map(|item| item.kind())
        else {
            self.data.diagnostics.push(schema_internal!(value.span));
            return Err(out.fail(BuildFailure::empty()));
        };
        let model = CompileModel::new(self.data.types, self.data.symbols, self.data.annotations);
        let execution = self.data.context.execution();
        let evaluator = Evaluator::new(
            &model,
            self.data.limits.values(),
            self.data.limits.regex(),
            Some(execution),
        );
        let element = match self
            .evaluation_errors
            .borrow_mut()
            .try_call(|eval_out| model.use_id(element, eval_out))
        {
            Ok(Some(element)) => element,
            Ok(None) => {
                self.data.diagnostics.push(schema_internal!(value.span));
                return Err(out.fail(BuildFailure::empty()));
            }
            Err(caught) => {
                self.data.diagnostics.push(DefaultDiagnostic::execution(
                    caught.take(),
                    value.span,
                    self.data.diagnostics,
                ));
                return Err(out.fail(BuildFailure::empty()));
            }
        };
        let duplicates = match self.evaluation_errors.borrow_mut().try_call(|eval_out| {
            ValueRules::unique(&evaluator, value.value.root(), element, by, eval_out)
        }) {
            Ok(duplicates) => duplicates,
            Err(caught) => {
                self.data.diagnostics.push(DefaultDiagnostic::execution(
                    caught.take(),
                    value.span,
                    self.data.diagnostics,
                ));
                return Err(out.fail(BuildFailure::empty()));
            }
        };
        let mut valid = true;
        for (index, duplicate) in duplicates.into_iter().enumerate() {
            self.data
                .context
                .consume(1, value.span, self.data.diagnostics, out.reborrow())?;
            if duplicate {
                valid = false;
                let span = value.member_spans.get(index).copied().unwrap_or(value.span);
                let context = self.data.context;
                let diagnostics = &mut self.data.diagnostics;
                let mut diagnostic = DefaultDiagnostic::semantic(
                    diagnostic_message!(
                        "diagnostic.schema.default_duplicate_elements",
                        "schema default contains duplicate elements"
                    ),
                    span,
                    context,
                    diagnostics,
                    out.reborrow(),
                )?;
                DefaultDiagnostic::constraint_label(
                    ConstraintLabelInput {
                        diagnostic: &mut diagnostic,
                        constraint: constraint_span,
                        fallback: span,
                        context,
                        sink: diagnostics,
                    },
                    out.reborrow(),
                )?;
                self.data.diagnostics.push(diagnostic);
                if self.data.diagnostics.is_stopped() {
                    return Err(out.fail(BuildFailure::empty()));
                }
            }
        }
        Ok(valid)
    }

    pub(super) fn check_all<'slot>(
        &mut self,
        value: &BuiltValue,
        target: TypeUseId,
        constraint: &CompiledConstraint,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, bool> {
        let Some(expression) = self
            .data
            .expressions
            .all_constraint(constraint.span(), out.reborrow())?
        else {
            if self
                .data
                .expressions
                .all_constraint_is_invalid(constraint.span(), out.reborrow())?
            {
                return Ok(true);
            }
            self.data.diagnostics.push(schema_internal!(value.span));
            return Err(out.fail(BuildFailure::empty()));
        };
        let model = CompileModel::new(self.data.types, self.data.symbols, self.data.annotations);
        let execution = self.data.context.execution();
        let evaluator = Evaluator::new(
            &model,
            self.data.limits.values(),
            self.data.limits.regex(),
            Some(execution),
        );
        let element = match self.data.types.type_use(target).map(|use_| use_.kind()) {
            Some(ResolvedTypeKind::Array { element, .. })
            | Some(ResolvedTypeKind::Map(element)) => element,
            _ => {
                self.data.diagnostics.push(schema_internal!(value.span));
                return Err(out.fail(BuildFailure::empty()));
            }
        };
        let element = match self
            .evaluation_errors
            .borrow_mut()
            .try_call(|eval_out| model.use_id(element, eval_out))
        {
            Ok(Some(element)) => element,
            Ok(None) => {
                self.data.diagnostics.push(schema_internal!(value.span));
                return Err(out.fail(BuildFailure::empty()));
            }
            Err(caught) => {
                self.data.diagnostics.push(DefaultDiagnostic::execution(
                    caught.take(),
                    value.span,
                    self.data.diagnostics,
                ));
                return Err(out.fail(BuildFailure::empty()));
            }
        };
        let (entries, is_map) = match value.value.as_ref() {
            ValueKind::Array(values) => (values, false),
            ValueKind::Map(values) => (values, true),
            _ => {
                self.data.diagnostics.push(schema_internal!(value.span));
                return Err(out.fail(BuildFailure::empty()));
            }
        };
        let mut valid = true;
        for (index, entry) in entries.enumerate() {
            if self.data.diagnostics.is_stopped() {
                return Err(out.fail(BuildFailure::empty()));
            }
            let item = match (is_map, entry.name()) {
                (false, None) => AllItemValue::ArrayItem(entry.value()),
                (true, Some(key)) => AllItemValue::MapEntry {
                    key,
                    value: entry.value(),
                },
                _ => {
                    self.data.diagnostics.push(schema_internal!(value.span));
                    return Err(out.fail(BuildFailure::empty()));
                }
            };
            let result = {
                let mut errors = self.evaluation_errors.borrow_mut();
                match errors.try_call(|eval_out| {
                    ValueRules::all_item(
                        &evaluator,
                        AllItemRequest {
                            expression: expression.expression(),
                            element,
                            item,
                        },
                        eval_out,
                    )
                }) {
                    Ok(value) => Ok(value),
                    Err(caught) => Err(caught.take()),
                }
            };
            match result {
                Ok(true) => continue,
                Ok(false) => {}
                Err(error) => {
                    self.data.diagnostics.push(DefaultDiagnostic::execution(
                        error,
                        value.span,
                        self.data.diagnostics,
                    ));
                    return Err(out.fail(BuildFailure::empty()));
                }
            }
            let span = value.member_spans.get(index).copied().unwrap_or(value.span);
            let diagnostic = DefaultDiagnostic::all_failure(
                span,
                constraint.span(),
                self.data.context,
                self.data.diagnostics,
                out.reborrow(),
            )?;
            self.data.diagnostics.push(diagnostic);
            valid = false;
        }
        Ok(valid)
    }

    pub(super) fn check_sum<'slot>(
        &mut self,
        input: SumRuleInput<'_>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, bool> {
        let SumRuleInput {
            value,
            target,
            bounds,
            constraint_span,
        } = input;
        let ValueKind::Array(_) = value.value.as_ref() else {
            return self.constraint_failure(
                diagnostic_message!(
                    "diagnostic.schema.default_sum_constraint_failed",
                    "schema default does not satisfy the @sum constraint"
                ),
                value.span,
                constraint_span,
                out.reborrow(),
            );
        };
        let Some(ResolvedTypeKind::Array { element, .. }) =
            self.data.types.type_use(target).map(|item| item.kind())
        else {
            self.data.diagnostics.push(schema_internal!(value.span));
            return Err(out.fail(BuildFailure::empty()));
        };
        let model = CompileModel::new(self.data.types, self.data.symbols, self.data.annotations);
        let execution = self.data.context.execution();
        let evaluator = Evaluator::new(
            &model,
            self.data.limits.values(),
            self.data.limits.regex(),
            Some(execution),
        );
        let element = match self
            .evaluation_errors
            .borrow_mut()
            .try_call(|eval_out| model.use_id(element, eval_out))
        {
            Ok(Some(element)) => element,
            Ok(None) => {
                self.data.diagnostics.push(schema_internal!(value.span));
                return Err(out.fail(BuildFailure::empty()));
            }
            Err(caught) => {
                self.data.diagnostics.push(DefaultDiagnostic::execution(
                    caught.take(),
                    value.span,
                    self.data.diagnostics,
                ));
                return Err(out.fail(BuildFailure::empty()));
            }
        };
        let result = {
            let mut errors = self.evaluation_errors.borrow_mut();
            match errors.try_call(|eval_out| {
                ValueRules::sum_bounds(
                    &evaluator,
                    SumBoundsRequest {
                        element,
                        values: value.value.root(),
                        min: bounds.0,
                        max: bounds.1,
                    },
                    eval_out,
                )
            }) {
                Ok(value) => Ok(value),
                Err(caught) => Err(caught.take()),
            }
        };
        match result {
            Ok(true) => Ok(true),
            Ok(false) => self.constraint_failure(
                diagnostic_message!(
                    "diagnostic.schema.default_sum_constraint_failed",
                    "schema default does not satisfy the @sum constraint"
                ),
                value.span,
                constraint_span,
                out.reborrow(),
            ),
            Err(error) => {
                self.data.diagnostics.push(DefaultDiagnostic::execution(
                    error,
                    value.span,
                    self.data.diagnostics,
                ));
                Err(out.fail(BuildFailure::empty()))
            }
        }
    }

    pub(super) fn constraint_failure<'slot>(
        &mut self,
        message: DiagnosticMessage<'static>,
        value_span: SourceSpan,
        constraint_span: SourceSpan,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, bool> {
        let context = self.data.context;
        let diagnostics = &mut self.data.diagnostics;
        let mut diagnostic =
            DefaultDiagnostic::semantic(message, value_span, context, diagnostics, out.reborrow())?;
        DefaultDiagnostic::constraint_label(
            ConstraintLabelInput {
                diagnostic: &mut diagnostic,
                constraint: constraint_span,
                fallback: value_span,
                context,
                sink: diagnostics,
            },
            out.reborrow(),
        )?;
        self.data.diagnostics.push(diagnostic);
        Ok(false)
    }
}

impl<'borrow, 'data, 'diagnostics> Deref for DefaultRuleChecker<'borrow, 'data, 'diagnostics> {
    type Target = DefaultData<'data, 'diagnostics>;

    fn deref(&self) -> &Self::Target {
        self.data
    }
}
