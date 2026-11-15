use crate::context::SemanticStatus;
use opeo::{ErrSlot, OResult, Out};
use typol_core::{
    BudgetError, CapacityExceeded, ChargedString, ChargedVec, Diagnostic, DiagnosticCode,
    DiagnosticContext, DiagnosticFieldStatus, DiagnosticLabelInput, DiagnosticMessage,
    DiagnosticParameters, ExecutionBudget, InstancePath, InternalError, LocatedSpan,
    RelatedLocationRole, SourceSpan, ValueSource, diagnostic_context,
};
use typol_i18n::{Translator, t};
use typol_semantic::{EvalError, EvalFailure, SchemaAccessError};
use u18n::RenderError;

#[cfg(test)]
use typol_i18n::Language;

#[cfg(test)]
use typol_core::{SourceId, Span};

static CONFIG_SPAN_COMBINATION: DiagnosticContext =
    diagnostic_context!("typol-config", "source-location", "span-combination-failed",);
static CONFIG_EVALUATION_UNSPECIFIED: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "value-validation",
    "evaluation-error-without-context",
);
static CONFIG_DIAGNOSTIC_LIMIT: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "diagnostic-buffer",
    "diagnostic-limit-reached",
);

pub(crate) struct DetailedDiagnosticRequest<'message> {
    pub(crate) code: DiagnosticCode,
    pub(crate) message: DiagnosticMessage<'message>,
    pub(crate) primary: Option<LocatedSpan>,
    pub(crate) path: Option<InstancePath>,
    pub(crate) value_source: Option<ValueSource>,
    pub(crate) context: Option<DiagnosticContext>,
    pub(crate) translator: Translator,
}

pub(crate) struct WorkspaceDiagnosticRequest {
    pub(crate) code: DiagnosticCode,
    pub(crate) message: Result<ChargedString, RenderError>,
    pub(crate) parameters: Option<DiagnosticParameters>,
    pub(crate) primary: Option<LocatedSpan>,
    pub(crate) path: Option<InstancePath>,
    pub(crate) value_source: Option<ValueSource>,
    pub(crate) context: Option<DiagnosticContext>,
    pub(crate) translator: Translator,
}

pub(crate) struct LabelMessageRequest<S, M> {
    pub(crate) span: S,
    pub(crate) message: M,
    pub(crate) role: RelatedLocationRole,
    pub(crate) translator: Translator,
}

pub(crate) struct ExecutionDiagnostic;

pub(crate) struct RuleFailureLocation {
    pub(crate) span: Option<LocatedSpan>,
    pub(crate) constraint: SourceSpan,
    pub(crate) path: InstancePath,
    pub(crate) source: Option<ValueSource>,
}

pub(crate) struct RuleFailureMessage<'message> {
    pub(crate) message: DiagnosticMessage<'message>,
    pub(crate) redacted: bool,
    pub(crate) translator: Translator,
}

impl ExecutionDiagnostic {
    fn display_message<'message>(
        message: RuleFailureMessage<'message>,
        error: Option<EvalError>,
    ) -> DiagnosticMessage<'message> {
        let translator = message.translator;
        if message.redacted {
            return t! {
                name: "diagnostic.validation.value_hidden",
                locale: translator,
            }
            .into();
        }
        match error.map(EvalError::kind) {
            Some(
                EvalFailure::Internal(_)
                | EvalFailure::RegexExecution(_)
                | EvalFailure::SchemaAccess(_)
                | EvalFailure::CapacityExceeded(_),
            ) => t! {
                name: "diagnostic.validation.runtime_internal",
                locale: translator,
            }
            .into(),
            Some(EvalFailure::ResourceLimit(_)) => t! {
                name: "diagnostic.validation.runtime_limit",
                locale: translator,
            }
            .into(),
            Some(EvalFailure::Allocation) => t! {
                name: "diagnostic.validation.runtime_allocation",
                locale: translator,
            }
            .into(),
            Some(EvalFailure::Arithmetic) => t! {
                name: "diagnostic.validation.constraint_numeric_evaluation_failed",
                locale: translator,
            }
            .into(),
            _ => message.message,
        }
    }

    pub(crate) fn failure<'slot>(
        result: Result<bool, EvalError>,
        message: RuleFailureMessage<'_>,
        location: RuleFailureLocation,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Option<Diagnostic>> {
        let RuleFailureLocation {
            span,
            constraint,
            path,
            source,
        } = location;
        let code = match result {
            Ok(true) => return Ok(None),
            Ok(false) => DiagnosticCode::CONFIG_VALIDATION_ERROR,
            Err(error) => ExecutionDiagnostic::code(error),
        };
        let error = result.err();
        let translator = message.translator;
        let message = Self::display_message(message, error);
        let context = error.map(|error| error.context().unwrap_or(CONFIG_EVALUATION_UNSPECIFIED));
        let mut diagnostic = DiagnosticFactory::detailed_diagnostic(DetailedDiagnosticRequest {
            code,
            message,
            primary: span,
            path: Some(path),
            value_source: source,
            context,
            translator,
        });
        if let Some(error) = error {
            if diagnostic.code() == DiagnosticCode::ALLOCATION_FAILED {
                return Err(out.fail(diagnostic));
            }
            if diagnostic.is_emergency() {
                return Ok(Some(diagnostic));
            }
            if let Some(error_span) = error.source_span() {
                let error_span = error_span.into();
                if Some(error_span) != span && error_span.source_span() != constraint {
                    DiagnosticFactory::add_label_message(
                        &mut diagnostic,
                        LabelMessageRequest {
                            span: error_span,
                            message: t! {
                                name: "diagnostic.label.evaluation_failed_here",
                                locale: translator,
                            },
                            role: RelatedLocationRole::PreviousDeclaration,
                            translator,
                        },
                        out.reborrow(),
                    )?;
                }
            }
        }
        if diagnostic.code() == DiagnosticCode::ALLOCATION_FAILED {
            return Err(out.fail(diagnostic));
        }
        if diagnostic.is_emergency() {
            return Ok(Some(diagnostic));
        }
        DiagnosticFactory::add_label_message(
            &mut diagnostic,
            LabelMessageRequest {
                span: constraint,
                message: t! {
                    name: "diagnostic.label.constraint_defined_here",
                    locale: translator,
                },
                role: RelatedLocationRole::ConstraintDefinition,
                translator,
            },
            out.reborrow(),
        )?;
        Ok(Some(diagnostic))
    }

    pub(crate) fn status(reported: bool) -> SemanticStatus {
        if reported {
            SemanticStatus::Reported
        } else {
            SemanticStatus::Success
        }
    }

    pub(crate) fn code(error: EvalError) -> DiagnosticCode {
        match error.kind() {
            EvalFailure::Internal(internal) => internal.diagnostic_code(),
            EvalFailure::RegexExecution(error) => error.diagnostic_code(),
            EvalFailure::SchemaAccess(error) => {
                InternalError::SchemaAccessInvariantFailure(error.context()).diagnostic_code()
            }
            EvalFailure::ResourceLimit(limit) => limit.diagnostic_code(),
            EvalFailure::Arithmetic => DiagnosticCode::CONFIG_VALIDATION_ERROR,
            EvalFailure::Allocation => DiagnosticCode::ALLOCATION_FAILED,
            EvalFailure::CapacityExceeded(capacity) => capacity.diagnostic_code(),
        }
    }
}

pub(crate) struct BudgetDiagnostic;

impl BudgetDiagnostic {
    pub(crate) fn workspace_vec<'slot, 'borrow, T, E>(
        budget: Option<&ExecutionBudget>,
        out: Out<'slot, 'borrow, E>,
    ) -> OResult<'slot, ChargedVec<T>>
    where
        E: From<BudgetError>,
    {
        ChargedVec::new(budget, out)
    }

    pub(crate) fn from_error(error: impl Into<BudgetError>, translator: Translator) -> Diagnostic {
        let error = error.into();
        let code = error.diagnostic_code();
        let context = error.context();
        let message = match error {
            BudgetError::Allocation => t! {
                name: "diagnostic.emergency.allocation_failed",
                locale: translator,
            },
            BudgetError::CapacityExceeded(capacity) => capacity.emergency_message(translator),
            BudgetError::Internal(error) => error.emergency_message(translator),
            BudgetError::ResourceLimit(limit) => limit.emergency_message(translator),
        };
        let mut diagnostic = match message {
            Ok(message) => Diagnostic::emergency(code, message),
            Err(error) => Diagnostic::translation_failure(code, error),
        };
        diagnostic.set_context(context);
        diagnostic
    }
}

pub(crate) struct DiagnosticFactory;

impl DiagnosticFactory {
    pub(crate) fn clear_path(diagnostic: &mut Diagnostic) {
        Self::bind_path(diagnostic, None);
        diagnostic.set_path_status(DiagnosticFieldStatus::Redacted);
    }

    pub(crate) fn add_label_message<'slot, 'message, S, M>(
        diagnostic: &mut Diagnostic,
        request: LabelMessageRequest<S, M>,
        out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ()>
    where
        S: Into<LocatedSpan>,
        M: Into<DiagnosticMessage<'message>>,
    {
        let LabelMessageRequest {
            span,
            message,
            role,
            translator,
        } = request;
        let message = message.into();
        if let Some(error) = message.failure() {
            return Err(out.fail(Diagnostic::translation_failure(diagnostic.code(), error)));
        }
        let mut errors = ErrSlot::new();
        match errors.try_call(|mut budget_out| {
            let budget = match diagnostic.budget() {
                Some(budget) => Some(budget.try_clone(budget_out.reborrow())?),
                None => None,
            };
            diagnostic.try_add_label_with_text(
                DiagnosticLabelInput {
                    span,
                    text: message.text(),
                    role,
                },
                budget.as_ref(),
                budget_out.reborrow(),
            )
        }) {
            Ok(()) => Ok(()),
            Err(caught) => Err(out.fail(BudgetDiagnostic::from_error(caught.take(), translator))),
        }
    }

    pub(crate) fn schema_access(error: SchemaAccessError, translator: Translator) -> Diagnostic {
        Self::internal_error(
            InternalError::SchemaAccessInvariantFailure(error.context()),
            None,
            None,
            None,
            translator,
        )
    }

    pub(crate) fn parser_internal(
        context: DiagnosticContext,
        primary: Option<LocatedSpan>,
        path: Option<InstancePath>,
        value_source: Option<ValueSource>,
        translator: Translator,
    ) -> Diagnostic {
        Self::internal_error(
            InternalError::ConfigParserInvariantFailure(context),
            primary,
            path,
            value_source,
            translator,
        )
    }

    pub(crate) fn value_internal(
        context: DiagnosticContext,
        primary: Option<LocatedSpan>,
        path: Option<InstancePath>,
        value_source: Option<ValueSource>,
        translator: Translator,
    ) -> Diagnostic {
        Self::internal_error(
            InternalError::ConfigValueInvariantFailure(context),
            primary,
            path,
            value_source,
            translator,
        )
    }

    pub(crate) fn environment_internal(
        context: DiagnosticContext,
        primary: Option<LocatedSpan>,
        path: Option<InstancePath>,
        value_source: Option<ValueSource>,
        translator: Translator,
    ) -> Diagnostic {
        Self::internal_error(
            InternalError::EnvironmentInvariantFailure(context),
            primary,
            path,
            value_source,
            translator,
        )
    }

    pub(crate) fn secret_policy_internal(
        context: DiagnosticContext,
        primary: Option<LocatedSpan>,
        path: Option<InstancePath>,
        value_source: Option<ValueSource>,
        translator: Translator,
    ) -> Diagnostic {
        Self::internal_error(
            InternalError::SecretPolicyInvariantFailure(context),
            primary,
            path,
            value_source,
            translator,
        )
    }

    pub(crate) fn edit_internal(
        context: DiagnosticContext,
        primary: Option<LocatedSpan>,
        path: Option<InstancePath>,
        value_source: Option<ValueSource>,
        translator: Translator,
    ) -> Diagnostic {
        Self::internal_error(
            InternalError::ConfigEditInvariantFailure(context),
            primary,
            path,
            value_source,
            translator,
        )
    }

    pub(crate) fn collection_internal(
        context: DiagnosticContext,
        primary: Option<LocatedSpan>,
        path: Option<InstancePath>,
        value_source: Option<ValueSource>,
        translator: Translator,
    ) -> Diagnostic {
        Self::internal_error(
            InternalError::DiagnosticCollectionInvariantFailure(context),
            primary,
            path,
            value_source,
            translator,
        )
    }

    pub(crate) fn make_diagnostic(request: DetailedDiagnosticRequest<'_>) -> Diagnostic {
        Self::detailed_diagnostic(request)
    }

    pub(crate) fn detailed_diagnostic(request: DetailedDiagnosticRequest<'_>) -> Diagnostic {
        let DetailedDiagnosticRequest {
            code,
            message,
            primary,
            path,
            value_source,
            context,
            translator,
        } = request;
        let budget = path.as_ref().and_then(InstancePath::budget);
        let mut errors = ErrSlot::new();
        let mut diagnostic =
            match errors.try_call(|out| Diagnostic::try_new(code, message, budget, out)) {
                Ok(diagnostic) => diagnostic,
                Err(caught) => BudgetDiagnostic::from_error(caught.take(), translator),
            };
        diagnostic.set_context(context);
        diagnostic.set_located_primary(primary);
        Self::bind_path(&mut diagnostic, path);
        diagnostic.set_value_source(value_source);
        diagnostic
    }

    pub(crate) fn workspace_diagnostic(request: WorkspaceDiagnosticRequest) -> Diagnostic {
        let WorkspaceDiagnosticRequest {
            code,
            message,
            parameters,
            primary,
            path,
            value_source,
            context,
            translator,
        } = request;
        let budget = path.as_ref().and_then(InstancePath::budget);
        let mut errors = ErrSlot::new();
        let mut diagnostic = match message {
            Ok(text) => match errors
                .try_call(|out| Diagnostic::try_new_with_text(code, text, parameters, budget, out))
            {
                Ok(diagnostic) => diagnostic,
                Err(caught) => BudgetDiagnostic::from_error(caught.take(), translator),
            },
            Err(error) => Diagnostic::translation_failure(code, error),
        };
        diagnostic.set_context(context);
        diagnostic.set_located_primary(primary);
        Self::bind_path(&mut diagnostic, path);
        diagnostic.set_value_source(value_source);
        diagnostic
    }

    pub(crate) fn budget_failure(
        error: impl Into<BudgetError>,
        primary: Option<LocatedSpan>,
        path: Option<InstancePath>,
        value_source: Option<ValueSource>,
        translator: Translator,
    ) -> Diagnostic {
        let mut diagnostic = BudgetDiagnostic::from_error(error, translator);
        diagnostic.set_located_primary(primary);
        Self::bind_path(&mut diagnostic, path);
        diagnostic.set_value_source(value_source);
        diagnostic
    }

    pub(crate) fn internal(
        context: DiagnosticContext,
        primary: Option<LocatedSpan>,
        path: Option<InstancePath>,
        value_source: Option<ValueSource>,
        translator: Translator,
    ) -> Diagnostic {
        Self::internal_error(
            InternalError::ConfigValidationInvariantFailure(context),
            primary,
            path,
            value_source,
            translator,
        )
    }

    pub(crate) fn internal_error(
        error: InternalError,
        primary: Option<LocatedSpan>,
        path: Option<InstancePath>,
        value_source: Option<ValueSource>,
        translator: Translator,
    ) -> Diagnostic {
        let code = error.diagnostic_code();
        let mut diagnostic = match error.emergency_message(translator) {
            Ok(message) => Diagnostic::emergency(error.diagnostic_code(), message),
            Err(render_error) => Diagnostic::translation_failure(code, render_error),
        };
        diagnostic.set_context(Some(error.context()));
        diagnostic.set_located_primary(primary);
        Self::bind_path(&mut diagnostic, path);
        diagnostic.set_value_source(value_source);
        diagnostic
    }

    pub(crate) fn capacity_error(
        capacity: CapacityExceeded,
        primary: Option<LocatedSpan>,
        path: Option<InstancePath>,
        value_source: Option<ValueSource>,
        translator: Translator,
    ) -> Diagnostic {
        let code = capacity.diagnostic_code();
        let mut diagnostic =
            Diagnostic::translated_emergency(code, capacity.emergency_message(translator));
        diagnostic.set_located_primary(primary);
        Self::bind_path(&mut diagnostic, path);
        diagnostic.set_value_source(value_source);
        diagnostic
    }

    /// Bounded diagnostics avoid message allocation so failures preserve the terminal code.
    pub(crate) fn limit_diagnostic(
        primary: Option<LocatedSpan>,
        translator: Translator,
    ) -> Diagnostic {
        let mut diagnostic = match t! {
            name: "diagnostic.emergency.diagnostic_limit_reached",
            locale: translator,
        } {
            Ok(message) => Diagnostic::emergency(DiagnosticCode::DIAGNOSTIC_LIMIT_REACHED, message),
            Err(error) => {
                Diagnostic::translation_failure(DiagnosticCode::DIAGNOSTIC_LIMIT_REACHED, error)
            }
        };
        diagnostic.set_context(Some(CONFIG_DIAGNOSTIC_LIMIT));
        diagnostic.set_located_primary(primary);
        diagnostic
    }

    pub(crate) fn allocation_diagnostic(
        primary: Option<LocatedSpan>,
        _path: Option<InstancePath>,
        value_source: Option<ValueSource>,
        translator: Translator,
    ) -> Diagnostic {
        let mut diagnostic = BudgetDiagnostic::from_error(BudgetError::Allocation, translator);
        diagnostic.set_located_primary(primary);
        diagnostic.set_value_source(value_source);
        diagnostic
    }

    pub(crate) fn bind_path(diagnostic: &mut Diagnostic, path: Option<InstancePath>) {
        let mut errors = ErrSlot::new();
        if let Err(caught) = errors.try_call(|mut out| {
            let budget = match path.as_ref().and_then(InstancePath::budget) {
                Some(budget) => Some(budget.try_clone(out.reborrow())?),
                None => None,
            };
            diagnostic.try_set_path(path, budget.as_ref(), out.reborrow())
        }) {
            let _ = caught.take();
            diagnostic.set_path_status(DiagnosticFieldStatus::Unavailable);
        }
    }

    #[cfg(test)]
    pub(crate) fn source_span(source_id: SourceId, span: Span) -> LocatedSpan {
        SourceSpan::new(source_id, span).into()
    }

    pub(crate) fn span_through<'slot, 'borrow>(
        start: LocatedSpan,
        end: LocatedSpan,
        translator: Translator,
        out: Out<'slot, 'borrow, Diagnostic>,
    ) -> OResult<'slot, LocatedSpan> {
        match start.through(end) {
            Some(span) => Ok(span),
            None => {
                let mut diagnostic = BudgetDiagnostic::from_error(
                    BudgetError::invariant(&CONFIG_SPAN_COMBINATION),
                    translator,
                );
                diagnostic.set_located_primary(Some(start));
                Err(out.fail(diagnostic))
            }
        }
    }
}

#[cfg(test)]
#[test]
fn execution_redaction_uses_explicit_state() {
    let translator = Translator::new(Language::English);
    let hidden = ExecutionDiagnostic::display_message(
        RuleFailureMessage {
            message: DiagnosticMessage::custom("arbitrary non-secret text"),
            redacted: true,
            translator,
        },
        Some(EvalError::Internal {
            error: InternalError::SemanticEvaluationInvariantFailure(CONFIG_EVALUATION_UNSPECIFIED),
            source_span: None,
        }),
    );
    assert!(hidden.failure().is_none());
    assert_eq!(hidden.text(), "value is hidden");

    let visible = ExecutionDiagnostic::display_message(
        RuleFailureMessage {
            message: DiagnosticMessage::custom("value is hidden"),
            redacted: false,
            translator,
        },
        Some(EvalError::Internal {
            error: InternalError::SemanticEvaluationInvariantFailure(CONFIG_EVALUATION_UNSPECIFIED),
            source_span: None,
        }),
    );
    assert!(visible.failure().is_none());
    assert_eq!(
        visible.text(),
        "runtime validation type or location relationship is invalid"
    );
}
