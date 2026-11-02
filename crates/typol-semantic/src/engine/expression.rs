use crate::{
    ExpressionMethodInfo, StoredExpressionArgument, StoredExpressionKind, StoredTypeKind,
    TypedExpression,
    engine::{
        Bindings, EVALUATOR_INTERNAL, EvalError, EvalKind, EvalValue, Evaluator,
        model::{EvaluationStack, FinishInputs, Local, MethodCall, Task},
    },
};
use opeo::{OResult, Out, ResultOutExt};
use typol_core::{ChargedVec, ValueKind};
use typol_model::BinaryOperator;

struct EvaluationState<'a> {
    expression: &'a TypedExpression,
    bindings: Bindings<'a>,
    tasks: ChargedVec<Task<'a>>,
    locals: ChargedVec<Local<'a>>,
    result: Option<EvalValue<'a>>,
}

pub(super) struct ExpressionExecutor<'e, 'model> {
    evaluator: &'e Evaluator<'model>,
}

impl<'e, 'model> ExpressionExecutor<'e, 'model> {
    pub(super) fn new(evaluator: &'e Evaluator<'model>) -> Self {
        Self { evaluator }
    }

    pub(super) fn evaluate_inner<'a, 'slot>(
        &self,
        expression: &'a TypedExpression,
        bindings: Bindings<'a>,
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, EvalValue<'a>> {
        let mut state = EvaluationState {
            expression,
            bindings,
            tasks: ChargedVec::new(self.evaluator.budget(), out.reborrow())?,
            locals: ChargedVec::new(self.evaluator.budget(), out.reborrow())?,
            result: None,
        };
        EvaluationStack::push(
            &mut state.tasks,
            Task::Enter(expression.root(), 0),
            out.reborrow(),
        )?;
        while let Some(task) = state.tasks.pop() {
            match task {
                Task::Enter(index, depth) => {
                    self.evaluate_enter(&mut state, (index, depth), out.reborrow())?
                }
                Task::Child(index) => self.evaluate_child(&mut state, index, out.reborrow())?,
                Task::BinaryLeft(index, depth) => {
                    self.evaluate_binary_left(&mut state, (index, depth), out.reborrow())?
                }
                Task::BinaryRight(index, left) => {
                    self.evaluate_binary_right(&mut state, (index, left), out.reborrow())?
                }
                Task::MethodReceiver(index, depth) => {
                    self.evaluate_method_receiver(&mut state, (index, depth), out.reborrow())?
                }
                Task::MethodArgument(index, receiver) => {
                    self.evaluate_method_argument(&mut state, index, receiver, out.reborrow())?
                }
                task @ Task::Quantifier { .. } => {
                    self.evaluate_quantifier(&mut state, task, out.reborrow())?
                }
            }
        }
        let result = state
            .result
            .ok_or(EvalError::internal(EVALUATOR_INTERNAL))
            .store_err_into(out.reborrow())?;
        let same_type =
            self.evaluator
                .schema()
                .same_type(result.ty(), expression.ty(), out.reborrow())?;
        if !same_type || matches!(result.kind(), EvalKind::Magnitude(_)) {
            return Err(out.fail(EvalError::internal(EVALUATOR_INTERNAL)));
        }
        self.evaluator.checked(result, false, out.reborrow())
    }

    fn evaluate_enter<'a, 'slot>(
        &self,
        state: &mut EvaluationState<'a>,
        location: (usize, usize),
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, ()> {
        let expression = state.expression;
        let bindings = state.bindings;
        let (index, depth) = location;
        self.evaluator.step(out.reborrow())?;
        if depth >= self.evaluator.limits().max_nesting_depth() {
            return Err(out.fail(EvalError::Resource));
        }
        let node = expression
            .node(index)
            .ok_or(EvalError::internal(EVALUATOR_INTERNAL))
            .store_err_into(out.reborrow())?;
        let next = depth
            .checked_add(1)
            .ok_or(EvalError::Resource)
            .store_err_into(out.reborrow())?;
        match node.kind() {
            StoredExpressionKind::Field { receiver, .. }
            | StoredExpressionKind::Unary {
                operand: receiver, ..
            } => {
                EvaluationStack::push(&mut state.tasks, Task::Child(index), out.reborrow())?;
                EvaluationStack::push(
                    &mut state.tasks,
                    Task::Enter(receiver, next),
                    out.reborrow(),
                )?;
            }
            StoredExpressionKind::Binary { left, .. } => {
                EvaluationStack::push(
                    &mut state.tasks,
                    Task::BinaryLeft(index, next),
                    out.reborrow(),
                )?;
                EvaluationStack::push(&mut state.tasks, Task::Enter(left, next), out.reborrow())?;
            }
            StoredExpressionKind::Method { receiver, .. } => {
                EvaluationStack::push(
                    &mut state.tasks,
                    Task::MethodReceiver(index, next),
                    out.reborrow(),
                )?;
                EvaluationStack::push(
                    &mut state.tasks,
                    Task::Enter(receiver, next),
                    out.reborrow(),
                )?;
            }
            _ => {
                let result = self.evaluator.finish(
                    expression,
                    FinishInputs {
                        index,
                        bindings,
                        locals: &state.locals,
                        first: None,
                        second: None,
                    },
                    out.reborrow(),
                )?;
                state.result = Some(result);
            }
        }
        Ok(())
    }

    fn evaluate_child<'a, 'slot>(
        &self,
        state: &mut EvaluationState<'a>,
        index: usize,
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, ()> {
        let result = self.evaluator.finish(
            state.expression,
            FinishInputs {
                index,
                bindings: state.bindings,
                locals: &state.locals,
                first: state.result.take(),
                second: None,
            },
            out.reborrow(),
        )?;
        state.result = Some(result);
        Ok(())
    }

    fn evaluate_binary_left<'a, 'slot>(
        &self,
        state: &mut EvaluationState<'a>,
        location: (usize, usize),
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, ()> {
        let expression = state.expression;
        let (index, depth) = location;
        let left = state
            .result
            .take()
            .ok_or(EvalError::internal(EVALUATOR_INTERNAL))
            .store_err_into(out.reborrow())?;
        let StoredExpressionKind::Binary {
            operator, right, ..
        } = expression
            .node(index)
            .ok_or(EvalError::internal(EVALUATOR_INTERNAL))
            .store_err_into(out.reborrow())?
            .kind()
        else {
            return Err(out.fail(EvalError::internal(EVALUATOR_INTERNAL)));
        };
        let short = match operator {
            BinaryOperator::And => !left.boolean(out.reborrow())?,
            BinaryOperator::Or => left.boolean(out.reborrow())?,
            _ => false,
        };
        if short {
            state.result = Some(left);
        } else {
            EvaluationStack::push(
                &mut state.tasks,
                Task::BinaryRight(index, left),
                out.reborrow(),
            )?;
            EvaluationStack::push(&mut state.tasks, Task::Enter(right, depth), out.reborrow())?;
        }
        Ok(())
    }

    fn evaluate_binary_right<'a, 'slot>(
        &self,
        state: &mut EvaluationState<'a>,
        inputs: (usize, EvalValue<'a>),
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, ()> {
        let expression = state.expression;
        let (index, left) = inputs;
        let result = self.evaluator.finish(
            expression,
            FinishInputs {
                index,
                bindings: state.bindings,
                locals: &state.locals,
                first: Some(left),
                second: state.result.take(),
            },
            out.reborrow(),
        )?;
        state.result = Some(result);
        Ok(())
    }

    fn evaluate_method_receiver<'a, 'slot>(
        &self,
        state: &mut EvaluationState<'a>,
        location: (usize, usize),
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, ()> {
        let expression = state.expression;
        let (index, depth) = location;
        let receiver = state
            .result
            .take()
            .ok_or(EvalError::internal(EVALUATOR_INTERNAL))
            .store_err_into(out.reborrow())?;
        let node = expression
            .node(index)
            .ok_or(EvalError::internal(EVALUATOR_INTERNAL))
            .store_err_into(out.reborrow())?;
        let StoredExpressionKind::Method {
            method, argument, ..
        } = node.kind()
        else {
            return Err(out.fail(EvalError::internal(EVALUATOR_INTERNAL)));
        };
        match argument {
            Some(StoredExpressionArgument::Expression(child)) => {
                EvaluationStack::push(
                    &mut state.tasks,
                    Task::MethodArgument(index, receiver),
                    out.reborrow(),
                )?;
                EvaluationStack::push(&mut state.tasks, Task::Enter(child, depth), out.reborrow())?;
            }
            Some(StoredExpressionArgument::Closure { parameter, body }) => {
                if !matches!(
                    method,
                    ExpressionMethodInfo::Any | ExpressionMethodInfo::All
                ) {
                    return Err(out.fail(EvalError::internal(EVALUATOR_INTERNAL)));
                }
                let type_info = self
                    .evaluator
                    .schema()
                    .type_info(receiver.ty(), out.reborrow())?;
                let StoredTypeKind::Array { element, .. } = type_info
                    .ok_or(EvalError::internal(EVALUATOR_INTERNAL))
                    .store_err_into(out.reborrow())?
                    .kind()
                else {
                    return Err(out.fail(EvalError::internal(EVALUATOR_INTERNAL)));
                };
                let value = receiver.value(out.reborrow())?;
                let ValueKind::Array(values) = value.as_ref() else {
                    return Err(out.fail(EvalError::internal(EVALUATOR_INTERNAL)));
                };
                EvaluationStack::push(
                    &mut state.tasks,
                    Task::Quantifier {
                        ty: node.ty(),
                        values,
                        completed: false,
                        depth,
                        parameter,
                        body,
                        element,
                        any: method == ExpressionMethodInfo::Any,
                    },
                    out.reborrow(),
                )?;
            }
            None => {
                let result = self.evaluator.method(
                    receiver,
                    MethodCall {
                        method,
                        argument,
                        ty: node.ty(),
                    },
                    None,
                    out.reborrow(),
                )?;
                state.result = Some(result);
            }
        }
        Ok(())
    }

    fn evaluate_method_argument<'a, 'slot>(
        &self,
        state: &mut EvaluationState<'a>,
        index: usize,
        receiver: EvalValue<'a>,
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, ()> {
        let expression = state.expression;
        let node = expression
            .node(index)
            .ok_or(EvalError::internal(EVALUATOR_INTERNAL))
            .store_err_into(out.reborrow())?;
        let StoredExpressionKind::Method {
            method, argument, ..
        } = node.kind()
        else {
            return Err(out.fail(EvalError::internal(EVALUATOR_INTERNAL)));
        };
        let result = self.evaluator.method(
            receiver,
            MethodCall {
                method,
                argument,
                ty: node.ty(),
            },
            state.result.take(),
            out.reborrow(),
        )?;
        state.result = Some(result);
        Ok(())
    }

    fn evaluate_quantifier<'a, 'slot>(
        &self,
        state: &mut EvaluationState<'a>,
        task: Task<'a>,
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, ()> {
        let Task::Quantifier {
            ty,
            mut values,
            completed,
            depth,
            parameter,
            body,
            element,
            any,
        } = task
        else {
            return Err(out.fail(EvalError::internal(EVALUATOR_INTERNAL)));
        };
        if completed {
            let _local = state
                .locals
                .pop()
                .ok_or(EvalError::internal(EVALUATOR_INTERNAL))
                .store_err_into(out.reborrow())?;
            let matched = state
                .result
                .take()
                .ok_or(EvalError::internal(EVALUATOR_INTERNAL))
                .store_err_into(out.reborrow())?;
            if matched.boolean(out.reborrow())? == any {
                state.result = Some(EvalValue::new(EvalKind::Boolean(any), ty, false));
                return Ok(());
            }
        }
        if let Some(value) = values.next() {
            self.evaluator.step(out.reborrow())?;
            EvaluationStack::push(
                &mut state.locals,
                Local {
                    parameter,
                    value: EvalValue::config(
                        value.value(),
                        element,
                        self.evaluator.host(element, out.reborrow())?,
                    ),
                },
                out.reborrow(),
            )?;
            EvaluationStack::push(
                &mut state.tasks,
                Task::Quantifier {
                    ty,
                    values,
                    completed: true,
                    depth,
                    parameter,
                    body,
                    element,
                    any,
                },
                out.reborrow(),
            )?;
            EvaluationStack::push(&mut state.tasks, Task::Enter(body, depth), out.reborrow())?;
        } else {
            state.result = Some(EvalValue::new(EvalKind::Boolean(!any), ty, false));
        }
        Ok(())
    }
}
