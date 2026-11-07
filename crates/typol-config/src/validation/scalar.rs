use crate::{
    context::{AssignmentContext, SemanticStatus},
    diagnostics::{
        DetailedDiagnosticRequest, DiagnosticBuffer, DiagnosticFactory, ExecutionDiagnostic,
        RuleFailureLocation, RuleFailureMessage,
    },
    secret::SecretPolicy,
    session::ValidationResources,
    validation::Validator,
};
use opeo::{ErrSlot, OResult, Out};
use typol_core::{
    Diagnostic, DiagnosticCode, DiagnosticContext, DiagnosticMessage, DiagnosticMessageKey,
    InstancePath, LocatedSpan, ValueKind, ValueView, diagnostic_context,
};
use typol_i18n::{diagnostic_key, diagnostic_message};
use typol_regex::{RegexMatchError, RegexScratchError};
use typol_semantic::{
    Constraint, ConstraintData, EvalError, RegexExecutionError, RegexExecutionFailure,
    RegexMatchOptions, SchemaRules,
};

static SCALAR_VALUE_SHAPE: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "scalar-validation",
    "constraint-value-type-mismatch",
);
static SCALAR_REGEX_INTERNAL: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "regex-validation",
    "regex-failure-without-context",
);
static SCALAR_REGEX_ALLOCATION: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "regex-validation",
    "scratch-or-match-allocation-failed",
);

struct RegexLabelRequest<'work, 'schema> {
    diagnostic: &'work mut Diagnostic,
    span: LocatedSpan,
    text: &'work str,
    key: DiagnosticMessageKey,
    context: Option<DiagnosticContext>,
    metadata: &'work AssignmentContext<'schema>,
}

fn string<'a, 'slot, 'borrow>(
    value: ValueView<'a>,
    metadata: &AssignmentContext<'_>,
    out: Out<'slot, 'borrow, Diagnostic>,
) -> OResult<'slot, &'a str> {
    match value.as_ref() {
        ValueKind::String(value) => Ok(value),
        _ => Err(out.fail(internal(metadata))),
    }
}

fn internal(metadata: &AssignmentContext<'_>) -> Diagnostic {
    DiagnosticFactory::internal(SCALAR_VALUE_SHAPE, metadata.span, None, metadata.source)
}

fn allocation(metadata: &AssignmentContext<'_>) -> Diagnostic {
    DiagnosticFactory::allocation_diagnostic(metadata.span, None, metadata.source)
}

fn failure<'slot>(
    metadata: &AssignmentContext<'_>,
    constraint: Constraint<'_>,
    resources: &mut ValidationResources<'_>,
    mut out: Out<'slot, '_, Diagnostic>,
) -> OResult<'slot, Diagnostic> {
    let data = Validator::schema_call(
        &mut resources.schema_access_errors,
        |access_out| constraint.data(access_out),
        out.reborrow(),
    )?;
    let (key, message) = match data {
        ConstraintData::NumberBounds { .. } => (
            diagnostic_key!("diagnostic.validation.numeric_boundary_failed"),
            "value does not satisfy the numeric boundary constraint",
        ),
        ConstraintData::LengthBounds { .. } => (
            diagnostic_key!("diagnostic.validation.length_constraint_failed"),
            "string does not satisfy the length constraint",
        ),
        ConstraintData::Regex { .. } => (
            diagnostic_key!("diagnostic.validation.regex_constraint_failed"),
            "string does not satisfy the regex constraint",
        ),
        ConstraintData::Host => (
            diagnostic_key!("diagnostic.validation.host_format_constraint_failed"),
            "string does not satisfy the host format constraint",
        ),
        ConstraintData::Email => (
            diagnostic_key!("diagnostic.validation.email_format_constraint_failed"),
            "string does not satisfy the email format constraint",
        ),
        ConstraintData::Cidr { .. } => (
            diagnostic_key!("diagnostic.validation.cidr_format_constraint_failed"),
            "string does not satisfy the CIDR format constraint",
        ),
        _ => return Err(out.fail(internal(metadata))),
    };
    let hidden = SecretPolicy::hides(
        metadata.schema,
        metadata.path,
        metadata.span,
        metadata.source,
        out.reborrow(),
    )?;
    let message = if hidden {
        diagnostic_message!("diagnostic.validation.value_hidden", "value is hidden")
    } else {
        DiagnosticMessage::localized(message, key)
    };
    let mut diagnostic = DiagnosticFactory::make_diagnostic(
        DiagnosticCode::CONFIG_VALIDATION_ERROR,
        message,
        metadata.span,
        Some(ScalarValidator::copy_path(metadata, out.reborrow())?),
        metadata.source,
    );
    if diagnostic.code() == DiagnosticCode::ALLOCATION_FAILED {
        return Err(out.fail(diagnostic));
    }
    DiagnosticFactory::add_label_message(
        &mut diagnostic,
        constraint.span(),
        diagnostic_message!(
            "diagnostic.label.constraint_defined_here",
            "constraint defined here"
        ),
        out.reborrow(),
    )?;
    Ok(diagnostic)
}

fn regex_failure<'slot>(
    metadata: &AssignmentContext<'_>,
    constraint: Constraint<'_>,
    error: RegexExecutionError,
    mut out: Out<'slot, '_, Diagnostic>,
) -> OResult<'slot, Diagnostic> {
    let code = match error {
        RegexExecutionError::Scratch(error) => scratch_error_code(error),
        RegexExecutionError::Match(error) => match_error_code(error),
        RegexExecutionError::Budget(error) => error.diagnostic_code(),
        RegexExecutionError::Internal | RegexExecutionError::InternalAt { .. } => {
            DiagnosticCode::INTERNAL_ERROR
        }
        RegexExecutionError::Located { error, .. } => match error {
            RegexExecutionFailure::Scratch(error) => scratch_error_code(error),
            RegexExecutionFailure::Match(error) => match_error_code(error),
            RegexExecutionFailure::Budget(error) => error.diagnostic_code(),
            RegexExecutionFailure::Internal(_) => DiagnosticCode::INTERNAL_ERROR,
        },
    };
    if code == DiagnosticCode::ALLOCATION_FAILED {
        let mut diagnostic = allocation(metadata);
        diagnostic.set_context(Some(error.context().unwrap_or(SCALAR_REGEX_ALLOCATION)));
        return Ok(diagnostic);
    }
    let path = ScalarValidator::copy_path(metadata, out.reborrow())?;
    let hidden = SecretPolicy::hides(
        metadata.schema,
        metadata.path,
        metadata.span,
        metadata.source,
        out.reborrow(),
    )?;
    let context = if code == DiagnosticCode::INTERNAL_ERROR {
        Some(error.context().unwrap_or(SCALAR_REGEX_INTERNAL))
    } else {
        None
    };
    let mut diagnostic = DiagnosticFactory::detailed_diagnostic(DetailedDiagnosticRequest {
        code,
        key: Some(if hidden {
            diagnostic_key!("diagnostic.validation.value_hidden")
        } else {
            diagnostic_key!("diagnostic.validation.regex_execution_failed")
        }),
        message: if hidden {
            "value is hidden"
        } else {
            "regex execution failed"
        },
        primary: metadata.span,
        path: Some(path),
        value_source: metadata.source,
        context,
    });
    if diagnostic.code() == DiagnosticCode::ALLOCATION_FAILED || diagnostic.message().is_empty() {
        return Ok(diagnostic);
    }
    if code == DiagnosticCode::INTERNAL_ERROR {
        diagnostic.set_context(context);
    }
    if let Some(source_span) = error.source_span() {
        let source_span = source_span.into();
        if Some(source_span) != metadata.span && source_span.source_span() != constraint.span() {
            add_regex_label(
                RegexLabelRequest {
                    diagnostic: &mut diagnostic,
                    span: source_span,
                    text: "regex execution failed here",
                    key: diagnostic_key!("diagnostic.validation.regex_execution_failed_here"),
                    context,
                    metadata,
                },
                out.reborrow(),
            )?;
        }
    }
    if diagnostic.code() == DiagnosticCode::ALLOCATION_FAILED {
        return Ok(diagnostic);
    }
    add_regex_label(
        RegexLabelRequest {
            diagnostic: &mut diagnostic,
            span: constraint.span().into(),
            text: "constraint defined here",
            key: diagnostic_key!("diagnostic.label.constraint_defined_here"),
            context,
            metadata,
        },
        out.reborrow(),
    )?;
    Ok(diagnostic)
}

fn add_regex_label<'slot, 'borrow>(
    request: RegexLabelRequest<'_, '_>,
    out: Out<'slot, 'borrow, Diagnostic>,
) -> OResult<'slot, ()> {
    let RegexLabelRequest {
        diagnostic,
        span,
        text,
        key,
        context,
        metadata,
    } = request;
    let mut errors = ErrSlot::new();
    match errors.try_call(|diagnostic_out| {
        DiagnosticFactory::add_label_message(
            diagnostic,
            span,
            DiagnosticMessage::localized(text, key),
            diagnostic_out,
        )
    }) {
        Ok(()) => Ok(()),
        Err(caught) => Err(out.fail(diagnostic_label_error(caught.take(), context, metadata))),
    }
}

fn diagnostic_label_error(
    mut diagnostic: Diagnostic,
    context: Option<DiagnosticContext>,
    metadata: &AssignmentContext<'_>,
) -> Diagnostic {
    if diagnostic.context().is_none() {
        diagnostic.set_context(context);
    }
    if diagnostic.located_primary().is_none() {
        diagnostic.set_located_primary(metadata.span);
    }
    if diagnostic.value_source().is_none() {
        diagnostic.set_value_source(metadata.source);
    }
    diagnostic
}

fn scratch_error_code(error: RegexScratchError) -> DiagnosticCode {
    match error {
        RegexScratchError::ResourceLimitExceeded(_) => DiagnosticCode::RESOURCE_LIMIT_EXCEEDED,
        RegexScratchError::AllocationFailed => DiagnosticCode::ALLOCATION_FAILED,
        RegexScratchError::CapacityExceeded => DiagnosticCode::CAPACITY_EXCEEDED,
        RegexScratchError::InternalInvariantFailure(_) => DiagnosticCode::INTERNAL_ERROR,
    }
}

fn match_error_code(error: RegexMatchError) -> DiagnosticCode {
    match error {
        RegexMatchError::ResourceLimitExceeded(_) => DiagnosticCode::RESOURCE_LIMIT_EXCEEDED,
        RegexMatchError::AllocationFailed => DiagnosticCode::ALLOCATION_FAILED,
        RegexMatchError::CapacityExceeded => DiagnosticCode::CAPACITY_EXCEEDED,
        RegexMatchError::InternalInvariantFailure(_) => DiagnosticCode::INTERNAL_ERROR,
    }
}

struct EvaluationFailureRequest<'work, 'schema, 'diagnostics> {
    metadata: &'work AssignmentContext<'schema>,
    constraint: Constraint<'schema>,
    error: EvalError,
    diagnostics: &'work mut DiagnosticBuffer<'diagnostics>,
    reported: &'work mut bool,
}

fn report_evaluation_error<'slot>(
    request: EvaluationFailureRequest<'_, '_, '_>,
    mut out: Out<'slot, '_, Diagnostic>,
) -> OResult<'slot, bool> {
    let EvaluationFailureRequest {
        metadata,
        constraint,
        error,
        diagnostics,
        reported,
    } = request;
    let diagnostic = ExecutionDiagnostic::failure(
        Err(error),
        RuleFailureMessage {
            text: "scalar constraint evaluation failed",
            redacted: false,
            key: None,
        },
        RuleFailureLocation {
            span: metadata.span,
            constraint: constraint.span(),
            path: ScalarValidator::copy_path(metadata, out.reborrow())?,
            source: metadata.source,
        },
        out.reborrow(),
    );
    let diagnostic = diagnostic?;
    let Some(diagnostic) = diagnostic else {
        return Ok(true);
    };
    *reported = true;
    Ok(diagnostics.push(diagnostic))
}

pub(super) struct ScalarValidationRequest<'work, 'schema, 'diagnostics, 'limits> {
    pub(super) metadata: &'work AssignmentContext<'schema>,
    pub(super) value: ValueView<'work>,
    pub(super) constraints: &'work [Constraint<'schema>],
    pub(super) resources: &'work mut ValidationResources<'limits>,
    pub(super) diagnostics: &'work mut DiagnosticBuffer<'diagnostics>,
}

pub(crate) struct ScalarValidator;

impl ScalarValidator {
    /// Container children are already complete; optional-value rules are applied together at the scalar leaf.
    pub(super) fn validate<'slot>(
        request: ScalarValidationRequest<'_, '_, '_, '_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, SemanticStatus> {
        let ScalarValidationRequest {
            metadata,
            value,
            constraints,
            resources,
            diagnostics,
        } = request;
        if matches!(value.as_ref(), ValueKind::Optional(_)) {
            return Ok(SemanticStatus::Success);
        }
        let mut reported = false;
        for &constraint in constraints {
            Validator::budget_call(
                &mut resources.budget_errors,
                |budget_out| resources.budget.consume(1, budget_out),
                out.reborrow(),
            )?;
            let data = Validator::schema_call(
                &mut resources.schema_access_errors,
                |access_out| constraint.data(access_out),
                out.reborrow(),
            )?;
            let matched = match data {
                ConstraintData::Regex { program } => {
                    let text = string(value, metadata, out.reborrow())?;
                    let result = resources
                        .regex_errors
                        .try_call(|regex_out| {
                            resources.scratch.is_match_budgeted(
                                program.regex(),
                                RegexMatchOptions {
                                    value: Some(text),
                                    limits: resources.regex_limits,
                                    budget: &resources.regex_execution,
                                },
                                regex_out,
                            )
                        })
                        .map_err(|caught| caught.take());
                    match result {
                        Ok(matched) => matched,
                        Err(error) => {
                            let diagnostic =
                                regex_failure(metadata, constraint, error, out.reborrow())?;
                            if !diagnostics.push(diagnostic) {
                                return Ok(SemanticStatus::Stopped);
                            }
                            reported = true;
                            continue;
                        }
                    }
                }
                ConstraintData::SizeBounds { .. } => continue,
                _ => match resources
                    .eval_errors
                    .try_call(|eval_out| {
                        SchemaRules::scalar(value, data, Some(&resources.budget), eval_out)
                    })
                    .map_err(|caught| caught.take())
                {
                    Ok(Some(matches)) => matches,
                    Ok(None) => continue,
                    Err(error) => {
                        if !(report_evaluation_error(
                            EvaluationFailureRequest {
                                metadata,
                                constraint,
                                error,
                                diagnostics,
                                reported: &mut reported,
                            },
                            out.reborrow(),
                        ))? {
                            return Ok(SemanticStatus::Stopped);
                        }
                        continue;
                    }
                },
            };
            if !matched {
                let diagnostic = failure(metadata, constraint, resources, out.reborrow())?;
                if !diagnostics.push(diagnostic) {
                    return Ok(SemanticStatus::Stopped);
                }
                reported = true;
            }
        }
        Ok(if reported {
            SemanticStatus::Reported
        } else {
            SemanticStatus::Success
        })
    }

    pub(super) fn copy_path<'slot, 'borrow>(
        metadata: &AssignmentContext<'_>,
        out: Out<'slot, 'borrow, Diagnostic>,
    ) -> OResult<'slot, InstancePath> {
        SecretPolicy::copy_path(
            metadata.schema,
            metadata.path,
            metadata.span,
            metadata.source,
            out,
        )
    }
}
