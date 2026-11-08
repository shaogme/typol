use crate::{
    context::SemanticStatus,
    diagnostics::{
        DiagnosticFactory, ExecutionDiagnostic, RuleFailureLocation, RuleFailureMessage,
    },
    instance::InstanceWalk,
    secret::SecretPolicy,
    validation::AssertionsRequest,
    value::ValueFactory,
};
use opeo::{ErrSlot, OResult, Out};
use typol_core::{
    ConfigValue, Diagnostic, DiagnosticContext, ValueError, ValueErrorKind, ValueKind,
    ValuePayloadView, ValueView, diagnostic_context,
};
use typol_semantic::{
    AssertionView, EvalError, ExpressionBindingInfo, ExpressionKind, SchemaAccessError,
    SchemaBindings as Bindings, SchemaEvaluator as Evaluator, SchemaValue as EvalValue, TypeKind,
    VariantPayloadInfo,
};

static ASSERTION_UNIT_SELF_VALUE: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "assertion-evaluation",
    "self-binding-used-with-unit-variant",
);
static ASSERTION_SCHEMA_ACCESS: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "assertion-evaluation",
    "compiled-schema-view-unavailable",
);

struct AssertionEvaluation<'evaluator, 'schema, 'value> {
    evaluator: &'evaluator Evaluator<'schema>,
    assertion: AssertionView<'schema>,
    value: ValueView<'value>,
    root: &'value ConfigValue,
    payload_host: bool,
    schema_access_errors: &'evaluator mut ErrSlot<SchemaAccessError>,
    schema_iteration_errors: &'evaluator mut ErrSlot<SchemaAccessError>,
}

fn schema_call<'slot, 'borrow, T>(
    errors: &mut ErrSlot<SchemaAccessError>,
    operation: impl for<'call> FnOnce(Out<'call, 'call, SchemaAccessError>) -> OResult<'call, T>,
    out: Out<'slot, 'borrow, Diagnostic>,
) -> OResult<'slot, T> {
    match errors.try_call(operation) {
        Ok(value) => Ok(value),
        Err(caught) => Err(out.fail(DiagnosticFactory::internal(
            ASSERTION_SCHEMA_ACCESS.with_object_id(caught.take().index() as u64),
            None,
            None,
            None,
        ))),
    }
}

fn assertion_result<'schema, 'value, 'slot>(
    request: AssertionEvaluation<'_, 'schema, 'value>,
    mut out: Out<'slot, '_, EvalError>,
) -> OResult<'slot, bool> {
    let AssertionEvaluation {
        evaluator,
        assertion,
        value,
        root,
        payload_host,
        schema_access_errors,
        schema_iteration_errors,
    } = request;
    let expression = assertion.expression();
    let self_type = {
        let mut self_type = None;
        let mut visited = 0;
        let result = schema_iteration_errors.try_call(|mut iteration_out| {
            let mut access_failure = None;
            {
                let nodes = expression.nodes(iteration_out.reborrow());
                for node_result in nodes {
                    visited += 1;
                    let node = match node_result {
                        Ok(node) => node,
                        Err(failure) => return Err(failure),
                    };
                    let kind =
                        match schema_access_errors.try_call(|access_out| node.kind(access_out)) {
                            Ok(kind) => kind,
                            Err(caught) => {
                                access_failure = Some(caught.take());
                                break;
                            }
                        };
                    if matches!(
                        kind,
                        ExpressionKind::Binding(ExpressionBindingInfo::SelfValue)
                    ) {
                        match schema_access_errors.try_call(|access_out| node.ty(access_out)) {
                            Ok(ty) => self_type = Some(ty),
                            Err(caught) => access_failure = Some(caught.take()),
                        }
                        break;
                    }
                }
            }
            if let Some(error) = access_failure {
                return Err(iteration_out.fail(error));
            }
            Ok(())
        });
        if let Some(budget) = evaluator.budget() {
            budget.consume(visited, out.reborrow())?;
        }
        if let Err(caught) = result {
            return Err(out.fail(EvalError::from(caught.take())));
        }
        self_type
    };
    let payload_owner = if self_type.is_some() {
        match value.as_ref() {
            ValueKind::Enumeration {
                payload: ValuePayloadView::Structure(fields),
                ..
            } => {
                let mut value_errors = ErrSlot::new();
                Some(
                    value_errors
                        .try_call(|value_out| {
                            ValueFactory::structure_view(
                                fields,
                                value.source(),
                                evaluator.budget(),
                                value_out,
                            )
                        })
                        .map_err(|caught| out.reborrow().fail(map_value_error(caught.take())))?,
                )
            }
            _ => None,
        }
    } else {
        None
    };
    let mut bindings = Bindings {
        root: Some(root),
        ..Bindings::default()
    };
    if let Some(ty) = self_type {
        let host = payload_host || evaluator.host(ty, out.reborrow())?;
        let self_value = match payload_owner.as_ref() {
            Some(payload) => EvalValue::config(payload.root(), ty, host),
            None => match value.as_ref() {
                ValueKind::Enumeration {
                    payload: ValuePayloadView::Single(value),
                    ..
                } => EvalValue::config(value, ty, host),
                ValueKind::Enumeration {
                    payload: ValuePayloadView::Unit,
                    ..
                } => {
                    return Err(out.fail(
                        EvalError::internal(ASSERTION_UNIT_SELF_VALUE)
                            .with_source_span(assertion.span()),
                    ));
                }
                _ => EvalValue::config(value, ty, host),
            },
        };
        bindings.self_value = Some(self_value);
    }
    let value = evaluator.evaluate(expression, bindings, out.reborrow())?;
    value.boolean(out.reborrow())
}

fn map_value_error(error: ValueError) -> EvalError {
    match error.kind() {
        ValueErrorKind::ResourceLimitExceeded => EvalError::Resource,
        ValueErrorKind::AllocationFailed => EvalError::Allocation,
        ValueErrorKind::CapacityExceeded => EvalError::Capacity,
        ValueErrorKind::DuplicateMapKey
        | ValueErrorKind::DuplicateFieldName
        | ValueErrorKind::EnvironmentReferenceRequired
        | ValueErrorKind::InternalInvariantFailure => EvalError::Internal,
    }
}

pub(crate) struct AssertionValidator;

impl AssertionValidator {
    pub(super) fn validate<'slot>(
        request: AssertionsRequest<'_, '_, '_, '_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, SemanticStatus> {
        let AssertionsRequest {
            schema,
            root,
            locations,
            resources,
            diagnostics,
        } = request;
        let evaluator = Evaluator::new(
            schema,
            resources.value_limits,
            resources.regex_limits,
            Some(&resources.budget),
        );
        let mut reported = false;
        let mut walk = InstanceWalk::new(
            locations,
            schema,
            root,
            resources.value_limits.max_nesting_depth(),
            out.reborrow(),
        )?;
        while let Some((node, value)) = walk.next(out.reborrow())? {
            let info = node.expected();
            let variant = node.variant();
            let kind = schema_call(
                &mut resources.schema_access_errors,
                |access_out| info.kind(access_out),
                out.reborrow(),
            )?;
            let assertions = match kind {
                TypeKind::Struct { .. } => Some(schema_call(
                    &mut resources.schema_access_errors,
                    |access_out| info.assertions(access_out),
                    out.reborrow(),
                )?),
                TypeKind::Enum { .. } => match variant {
                    Some(variant) => Some(schema_call(
                        &mut resources.schema_access_errors,
                        |access_out| variant.assertions(access_out),
                        out.reborrow(),
                    )?),
                    None => None,
                },
                _ => None,
            };
            let Some(assertions) = assertions.filter(|items| !items.is_empty()) else {
                continue;
            };
            let payload_host = if let Some(variant) = variant {
                let constraints = schema_call(
                    &mut resources.schema_access_errors,
                    |access_out| variant.constraints(access_out),
                    out.reborrow(),
                )?;
                schema_call(
                    &mut resources.schema_access_errors,
                    |access_out| constraints.has_host(access_out),
                    out.reborrow(),
                )?
            } else {
                false
            };
            for index in 0..assertions.len() {
                let Some(assertion) = schema_call(
                    &mut resources.schema_access_errors,
                    |access_out| assertions.get(index, access_out),
                    out.reborrow(),
                )?
                else {
                    continue;
                };
                resources.budget.consume(1, out.reborrow())?;
                let result = resources
                    .eval_errors
                    .try_call(|eval_out| {
                        assertion_result(
                            AssertionEvaluation {
                                evaluator: &evaluator,
                                assertion,
                                value,
                                root,
                                payload_host,
                                schema_access_errors: &mut resources.schema_access_errors,
                                schema_iteration_errors: &mut resources.schema_iteration_errors,
                            },
                            eval_out,
                        )
                    })
                    .map_err(|caught| caught.take());
                if matches!(result, Ok(true)) {
                    continue;
                }
                let source = Some(value.source());
                let mut secret = walk.hidden(node.span(), source, out.reborrow())?;
                if let Some(variant) = variant {
                    if (schema_call(
                        &mut resources.schema_access_errors,
                        |access_out| variant.is_secret(access_out),
                        out.reborrow(),
                    ))? {
                        secret = true;
                    } else {
                        let payload = schema_call(
                            &mut resources.schema_access_errors,
                            |access_out| variant.payload(access_out),
                            out.reborrow(),
                        )?;
                        if let VariantPayloadInfo::Single { ty } = payload {
                            secret |= SecretPolicy::secret_type(
                                ty,
                                Some(&resources.budget),
                                &mut resources.schema_access_errors,
                                &mut resources.budget_errors,
                                out.reborrow(),
                            )?;
                        }
                    }
                }
                let path = walk.copy_path(node.span(), source, out.reborrow())?;
                if let Some(error) = ExecutionDiagnostic::failure(
                    result,
                    RuleFailureMessage {
                        text: assertion.message(),
                        redacted: secret,
                        key: None,
                    },
                    RuleFailureLocation {
                        span: node.span(),
                        constraint: assertion.span(),
                        path,
                        source,
                    },
                    out.reborrow(),
                )? {
                    reported = true;
                    if !diagnostics.push(error) {
                        return Ok(SemanticStatus::Stopped);
                    }
                }
            }
        }
        Ok(ExecutionDiagnostic::status(reported))
    }
}
