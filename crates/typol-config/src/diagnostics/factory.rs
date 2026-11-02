use crate::context::SemanticStatus;
use opeo::{ErrSlot, OResult, Out};
use typol_core::{
    BudgetError, ChargedVec, Diagnostic, DiagnosticCode, DiagnosticContext, DiagnosticMessage,
    DiagnosticMessageKey, ExecutionBudget, InstancePath, LocatedSpan, SourceSpan, ValueSource,
    diagnostic_context,
};
use typol_i18n::{diagnostic_key, diagnostic_message};
use typol_semantic::{EvalError, EvalFailure, SchemaAccessError};

#[cfg(test)]
use typol_core::{SourceId, Span};

static CONFIG_SPAN_COMBINATION: DiagnosticContext =
    diagnostic_context!("typol-config", "source-location", "span-combination-failed",);
static CONFIG_EVALUATION_UNSPECIFIED: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "value-validation",
    "evaluation-error-without-context",
);
static CONFIG_UNCLASSIFIED_INTERNAL: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "diagnostic-construction",
    "unclassified-internal-error",
);
static CONFIG_DIAGNOSTIC_LIMIT: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "diagnostic-buffer",
    "diagnostic-limit-reached",
);

pub(crate) struct DetailedDiagnosticRequest<'message> {
    pub(crate) code: DiagnosticCode,
    pub(crate) key: Option<DiagnosticMessageKey>,
    pub(crate) message: &'message str,
    pub(crate) primary: Option<LocatedSpan>,
    pub(crate) path: Option<InstancePath>,
    pub(crate) value_source: Option<ValueSource>,
    pub(crate) context: Option<DiagnosticContext>,
}

pub(crate) struct ExecutionDiagnostic;

pub(crate) struct RuleFailureLocation {
    pub(crate) span: Option<LocatedSpan>,
    pub(crate) constraint: SourceSpan,
    pub(crate) path: InstancePath,
    pub(crate) source: Option<ValueSource>,
}

pub(crate) struct RuleFailureMessage<'message> {
    pub(crate) text: &'message str,
    pub(crate) redacted: bool,
    pub(crate) key: Option<DiagnosticMessageKey>,
}

impl ExecutionDiagnostic {
    fn display_message<'message>(
        message: RuleFailureMessage<'message>,
        error: Option<EvalError>,
    ) -> &'message str {
        if message.redacted {
            return "value is hidden";
        }
        match error.map(EvalError::kind) {
            Some(EvalFailure::Internal | EvalFailure::InternalAt(_)) => {
                "runtime validation type or location relationship is invalid"
            }
            Some(EvalFailure::Resource) => "runtime validation depth exceeded the limit",
            Some(EvalFailure::Allocation) => "runtime validation allocation failed",
            Some(EvalFailure::Arithmetic) => "constraint expression numeric evaluation failed",
            _ => message.text,
        }
    }

    fn key(
        message: &RuleFailureMessage<'_>,
        error: Option<EvalError>,
    ) -> Option<DiagnosticMessageKey> {
        if message.redacted {
            return Some(diagnostic_key!("diagnostic.validation.value_hidden"));
        }
        match error.map(EvalError::kind) {
            Some(EvalFailure::Internal | EvalFailure::InternalAt(_)) => {
                Some(diagnostic_key!("diagnostic.validation.runtime_internal"))
            }
            Some(EvalFailure::Resource) => {
                Some(diagnostic_key!("diagnostic.validation.runtime_limit"))
            }
            Some(EvalFailure::Allocation) => {
                Some(diagnostic_key!("diagnostic.validation.runtime_allocation"))
            }
            Some(EvalFailure::Arithmetic) => Some(diagnostic_key!(
                "diagnostic.validation.constraint_numeric_evaluation_failed"
            )),
            _ => message.key,
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
        let key = Self::key(&message, error);
        let message = Self::display_message(message, error);
        let context = error.map(|error| error.context().unwrap_or(CONFIG_EVALUATION_UNSPECIFIED));
        let mut diagnostic = DiagnosticFactory::detailed_diagnostic(DetailedDiagnosticRequest {
            code,
            key,
            message,
            primary: span,
            path: Some(path),
            value_source: source,
            context,
        });
        if let Some(error) = error {
            if diagnostic.code() == DiagnosticCode::ALLOCATION_FAILED {
                return Err(out.fail(diagnostic));
            }
            if diagnostic.message().is_empty() {
                return Ok(Some(diagnostic));
            }
            if let Some(error_span) = error.source_span() {
                let error_span = error_span.into();
                if Some(error_span) != span && error_span.source_span() != constraint {
                    DiagnosticFactory::add_label_message(
                        &mut diagnostic,
                        error_span,
                        diagnostic_message!(
                            "diagnostic.label.evaluation_failed_here",
                            "evaluation failed here"
                        ),
                        out.reborrow(),
                    )?;
                }
            }
        }
        if diagnostic.code() == DiagnosticCode::ALLOCATION_FAILED {
            return Err(out.fail(diagnostic));
        }
        if diagnostic.message().is_empty() {
            return Ok(Some(diagnostic));
        }
        DiagnosticFactory::add_label_message(
            &mut diagnostic,
            constraint,
            diagnostic_message!(
                "diagnostic.label.constraint_defined_here",
                "constraint defined here"
            ),
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
            EvalFailure::Internal | EvalFailure::InternalAt(_) | EvalFailure::SchemaAccess(_) => {
                DiagnosticCode::INTERNAL_ERROR
            }
            EvalFailure::Resource => DiagnosticCode::RESOURCE_LIMIT_EXCEEDED,
            EvalFailure::Arithmetic => DiagnosticCode::CONFIG_VALIDATION_ERROR,
            EvalFailure::Allocation => DiagnosticCode::ALLOCATION_FAILED,
            EvalFailure::Capacity => DiagnosticCode::CAPACITY_EXCEEDED,
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

    pub(crate) fn from_error(error: impl Into<BudgetError>) -> Diagnostic {
        let error = error.into();
        let mut diagnostic = Diagnostic::emergency(error.diagnostic_code());
        diagnostic.set_context(error.context());
        diagnostic
    }
}

pub(crate) struct DiagnosticFactory;

impl DiagnosticFactory {
    pub(crate) fn clear_path(diagnostic: &mut Diagnostic) {
        Self::bind_path(diagnostic, None);
    }

    pub(crate) fn add_label_message<'slot>(
        diagnostic: &mut Diagnostic,
        span: impl Into<LocatedSpan>,
        message: DiagnosticMessage<'_>,
        out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ()> {
        let mut errors = ErrSlot::new();
        match errors.try_call(|mut budget_out| {
            let budget = match diagnostic.budget() {
                Some(budget) => Some(budget.try_clone(budget_out.reborrow())?),
                None => None,
            };
            diagnostic.try_add_label_with_text(
                span,
                message,
                budget.as_ref(),
                budget_out.reborrow(),
            )
        }) {
            Ok(()) => Ok(()),
            Err(caught) => Err(out.fail(BudgetDiagnostic::from_error(caught.take()))),
        }
    }

    pub(crate) fn schema_access(error: SchemaAccessError) -> Diagnostic {
        Self::internal(error.context(), None, None, None)
    }

    pub(crate) fn make_diagnostic<'message>(
        code: DiagnosticCode,
        message: impl Into<DiagnosticMessage<'message>>,
        primary: Option<LocatedSpan>,
        path: Option<InstancePath>,
        value_source: Option<ValueSource>,
    ) -> Diagnostic {
        let message = message.into();
        let budget = path.as_ref().and_then(InstancePath::budget);
        if code == DiagnosticCode::INTERNAL_ERROR {
            return Self::internal(CONFIG_UNCLASSIFIED_INTERNAL, primary, path, value_source);
        }
        if budget.is_none()
            && matches!(
                code,
                DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
                    | DiagnosticCode::ALLOCATION_FAILED
                    | DiagnosticCode::SOURCE_INPUT_ERROR
            )
        {
            let mut diagnostic = Diagnostic::emergency(code);
            diagnostic.set_located_primary(primary);
            Self::bind_path(&mut diagnostic, path);
            diagnostic.set_value_source(value_source);
            return diagnostic;
        }
        let mut errors = ErrSlot::new();
        let mut diagnostic =
            match errors.try_call(|out| Diagnostic::try_new(code, message, budget, out)) {
                Ok(diagnostic) => diagnostic,
                Err(caught) => {
                    let mut diagnostic = BudgetDiagnostic::from_error(caught.take());
                    diagnostic.set_located_primary(primary);
                    Self::bind_path(&mut diagnostic, path);
                    diagnostic.set_value_source(value_source);
                    return diagnostic;
                }
            };
        if let Some(key) = message.key() {
            diagnostic.set_message_key(Some(key));
        }
        diagnostic.set_located_primary(primary);
        Self::bind_path(&mut diagnostic, path);
        diagnostic.set_value_source(value_source);
        diagnostic
    }

    pub(crate) fn detailed_diagnostic(request: DetailedDiagnosticRequest<'_>) -> Diagnostic {
        let DetailedDiagnosticRequest {
            code,
            key,
            message,
            primary,
            path,
            value_source,
            context,
        } = request;
        if matches!(
            code,
            DiagnosticCode::ALLOCATION_FAILED
                | DiagnosticCode::SOURCE_INPUT_ERROR
                | DiagnosticCode::DIAGNOSTIC_LIMIT_REACHED
        ) {
            let mut diagnostic = Diagnostic::emergency(code);
            diagnostic.set_context(context);
            diagnostic.set_located_primary(primary);
            diagnostic.set_value_source(value_source);
            return diagnostic;
        }
        let budget = path.as_ref().and_then(InstancePath::budget);
        let mut errors = ErrSlot::new();
        let mut diagnostic = match errors.try_call(|out| match key {
            Some(key) => Diagnostic::try_new(
                code,
                DiagnosticMessage::localized(message, key),
                budget,
                out,
            ),
            None => Diagnostic::try_new(code, message, budget, out),
        }) {
            Ok(diagnostic) => diagnostic,
            Err(caught) => BudgetDiagnostic::from_error(caught.take()),
        };
        diagnostic.set_context(context);
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
    ) -> Diagnostic {
        let mut diagnostic = Diagnostic::emergency(DiagnosticCode::INTERNAL_ERROR);
        diagnostic.set_context(Some(context));
        diagnostic.set_located_primary(primary);
        Self::bind_path(&mut diagnostic, path);
        diagnostic.set_value_source(value_source);
        diagnostic
    }

    /// Bounded diagnostics avoid message allocation so failures preserve the terminal code.
    pub(crate) fn limit_diagnostic(primary: Option<LocatedSpan>) -> Diagnostic {
        let mut diagnostic = Diagnostic::emergency(DiagnosticCode::DIAGNOSTIC_LIMIT_REACHED);
        diagnostic.set_context(Some(CONFIG_DIAGNOSTIC_LIMIT));
        diagnostic.set_located_primary(primary);
        diagnostic
    }

    pub(crate) fn allocation_diagnostic(
        primary: Option<LocatedSpan>,
        _path: Option<InstancePath>,
        value_source: Option<ValueSource>,
    ) -> Diagnostic {
        let mut diagnostic = Diagnostic::emergency(DiagnosticCode::ALLOCATION_FAILED);
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
            let error = BudgetDiagnostic::from_error(caught.take());
            let primary = diagnostic.located_primary();
            let value_source = diagnostic.value_source();
            let context = diagnostic.context();
            *diagnostic = error;
            diagnostic.set_located_primary(primary);
            diagnostic.set_value_source(value_source);
            diagnostic.set_context(context);
        }
    }

    #[cfg(test)]
    pub(crate) fn source_span(source_id: SourceId, span: Span) -> LocatedSpan {
        SourceSpan::new(source_id, span).into()
    }

    pub(crate) fn span_through<'slot, 'borrow>(
        start: LocatedSpan,
        end: LocatedSpan,
        out: Out<'slot, 'borrow, Diagnostic>,
    ) -> OResult<'slot, LocatedSpan> {
        match start.through(end) {
            Some(span) => Ok(span),
            None => {
                let mut diagnostic =
                    BudgetDiagnostic::from_error(BudgetError::invariant(&CONFIG_SPAN_COMBINATION));
                diagnostic.set_located_primary(Some(start));
                Err(out.fail(diagnostic))
            }
        }
    }
}

#[cfg(test)]
#[test]
fn execution_redaction_uses_explicit_state() {
    let hidden = ExecutionDiagnostic::display_message(
        RuleFailureMessage {
            text: "arbitrary non-secret text",
            redacted: true,
            key: None,
        },
        Some(EvalError::Internal),
    );
    assert_eq!(hidden, "value is hidden");

    let visible = ExecutionDiagnostic::display_message(
        RuleFailureMessage {
            text: "value is hidden",
            redacted: false,
            key: None,
        },
        Some(EvalError::Internal),
    );
    assert_eq!(
        visible,
        "runtime validation type or location relationship is invalid"
    );
}
