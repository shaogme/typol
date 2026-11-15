use crate::{
    diagnostics::{DetailedDiagnosticRequest, DiagnosticFactory, WorkspaceDiagnosticRequest},
    parser::context::ParserContext,
    secret::{SecretPathContext, SecretPolicy},
};
use core::{
    convert::TryFrom,
    fmt::{self, Write as FmtWrite},
};
use opeo::{ErrSlot, OResult, Out};
use typol_core::{
    BudgetError, ChargedString, Diagnostic, DiagnosticCode, DiagnosticMessage, DiagnosticParameter,
    DiagnosticParameters, ExecutionBudget, InstancePath, LocatedSpan, ValueSource,
};
use typol_i18n::{Translator, t};
use u18n::RenderError;

#[cfg(test)]
use typol_i18n::Language;

#[cfg(test)]
use typol_core::{ExecutionLimits, ResourceLimit};

#[cfg(test)]
use typol_test_support::{TestFailure, TestOpeo, TestResult, test_assert, test_eq};

struct WorkspaceStringSink {
    output: ChargedString,
    errors: ErrSlot<BudgetError>,
    budget_error: Option<BudgetError>,
}

impl WorkspaceStringSink {
    fn new<'slot>(
        budget: Option<&ExecutionBudget>,
        out: Out<'slot, '_, BudgetError>,
    ) -> OResult<'slot, Self> {
        let output = ChargedString::with_capacity(0, budget, out)?;
        Ok(Self {
            output,
            errors: ErrSlot::new(),
            budget_error: None,
        })
    }
}

impl FmtWrite for WorkspaceStringSink {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        if self.budget_error.is_some() {
            return Err(fmt::Error);
        }
        let (errors, output) = (&mut self.errors, &mut self.output);
        match errors.try_call(|out| output.try_push_str(value, out)) {
            Ok(()) => Ok(()),
            Err(caught) => {
                self.budget_error = Some(caught.take());
                Err(fmt::Error)
            }
        }
    }
}

pub(crate) struct FixedArrayLengthMismatchRequest<'context, 'schema, 'source, 'limits> {
    pub(crate) context: &'context ParserContext<'schema, 'source, 'limits>,
    pub(crate) code: DiagnosticCode,
    pub(crate) expected: u64,
    pub(crate) actual: usize,
    pub(crate) span: LocatedSpan,
    pub(crate) value_source: Option<ValueSource>,
}

pub(crate) struct ContextDiagnosticRequest<'context, 'schema, 'source, 'limits> {
    pub(crate) context: &'context ParserContext<'schema, 'source, 'limits>,
    pub(crate) code: DiagnosticCode,
    pub(crate) message: DiagnosticMessage<'static>,
    pub(crate) span: LocatedSpan,
    pub(crate) value_source: Option<ValueSource>,
}

pub(crate) struct LiteralDiagnostic;

impl LiteralDiagnostic {
    pub(super) fn context_diagnostic<'slot>(
        request: ContextDiagnosticRequest<'_, '_, '_, '_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Diagnostic> {
        let ContextDiagnosticRequest {
            context,
            code,
            message,
            span,
            value_source,
        } = request;
        let path = Self::copy_path_with_source(context, Some(span), value_source, out.reborrow())?;
        let translator = context.session.translator();
        let hidden = SecretPolicy::hides(
            context.schema,
            &context.path,
            SecretPathContext {
                primary: Some(span),
                value_source,
                translator,
            },
            out.reborrow(),
        )?;
        let message = if hidden {
            t! {
                name: "diagnostic.validation.value_hidden",
                locale: translator,
            }
            .into()
        } else {
            message
        };
        Ok(DiagnosticFactory::make_diagnostic(
            DetailedDiagnosticRequest {
                code,
                message,
                primary: Some(span),
                path: Some(path),
                value_source,
                context: None,
                translator,
            },
        ))
    }

    pub(crate) fn fixed_array_length_mismatch<'slot>(
        request: FixedArrayLengthMismatchRequest<'_, '_, '_, '_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Diagnostic> {
        let FixedArrayLengthMismatchRequest {
            context,
            code,
            expected,
            actual,
            span,
            value_source,
        } = request;
        let path = Self::copy_path_with_source(context, Some(span), value_source, out.reborrow())?;
        let translator = context.session.translator();
        let hidden = SecretPolicy::hides(
            context.schema,
            &context.path,
            SecretPathContext {
                primary: Some(span),
                value_source,
                translator,
            },
            out.reborrow(),
        )?;
        if hidden {
            return Ok(DiagnosticFactory::make_diagnostic(
                DetailedDiagnosticRequest {
                    code,
                    message: t! {
                        name: "diagnostic.validation.value_hidden",
                        locale: translator,
                    }
                    .into(),
                    primary: Some(span),
                    path: Some(path),
                    value_source,
                    context: None,
                    translator,
                },
            ));
        }
        let budget = path.budget();
        let mut errors = ErrSlot::new();
        let message = match errors.try_call(|budget_out| {
            render_fixed_array_length_mismatch(expected, actual, translator, budget, budget_out)
        }) {
            Ok(message) => message,
            Err(caught) => {
                return Ok(DiagnosticFactory::budget_failure(
                    caught.take(),
                    Some(span),
                    Some(path),
                    value_source,
                    translator,
                ));
            }
        };
        let parameters = fixed_array_length_parameters(expected, actual);
        Ok(DiagnosticFactory::workspace_diagnostic(
            WorkspaceDiagnosticRequest {
                code,
                message,
                parameters: Some(parameters),
                primary: Some(span),
                path: Some(path),
                value_source,
                context: None,
                translator,
            },
        ))
    }

    pub(super) fn copy_path_with_source<'slot, 'borrow>(
        context: &ParserContext<'_, '_, '_>,
        primary: Option<LocatedSpan>,
        value_source: Option<ValueSource>,
        out: Out<'slot, 'borrow, Diagnostic>,
    ) -> OResult<'slot, InstancePath> {
        SecretPolicy::copy_path(
            context.schema,
            &context.path,
            SecretPathContext {
                primary,
                value_source,
                translator: context.session.translator(),
            },
            out,
        )
    }
}

fn render_fixed_array_length_mismatch<'slot>(
    expected: u64,
    actual: usize,
    translator: Translator,
    budget: Option<&ExecutionBudget>,
    mut out: Out<'slot, '_, BudgetError>,
) -> OResult<'slot, Result<ChargedString, RenderError>> {
    let mut sink = WorkspaceStringSink::new(budget, out.reborrow())?;
    let result = t! {
        name: "diagnostic.config.fixed_array_length_mismatch",
        locale: translator,
        args: { expected, actual },
        sink: &mut sink,
    };
    if let Some(error) = sink.budget_error.take() {
        return Err(out.fail(error));
    }
    let WorkspaceStringSink { output, .. } = sink;
    Ok(result.map(|()| output))
}

fn fixed_array_length_parameters(expected: u64, actual: usize) -> DiagnosticParameters {
    let Ok(expected) = usize::try_from(expected) else {
        return DiagnosticParameters::unavailable();
    };
    DiagnosticParameters::new([
        Some(DiagnosticParameter::unsigned("expected", expected)),
        Some(DiagnosticParameter::unsigned("actual", actual)),
        None,
        None,
    ])
}

#[cfg(test)]
#[test]
fn fixed_array_length_message_writes_localized_text_to_the_sink() -> TestResult<()> {
    let languages = [
        (
            Language::English,
            "fixed-size array expects 4 elements, but the input contains 3",
        ),
        (Language::Chinese, "定长数组需要 4 个元素，输入中有 3 个"),
    ];
    for (language, expected_text) in languages {
        let result = TestOpeo::call(|out| {
            render_fixed_array_length_mismatch(4, 3, Translator::new(language), None, out)
        })
        .map_err(TestFailure::from_debug)?;
        let text = result.map_err(TestFailure::from_debug)?;
        test_eq!(text.as_str(), expected_text)?;
    }
    Ok(())
}

#[cfg(test)]
#[test]
fn fixed_array_length_sink_preserves_budget_errors() -> TestResult<()> {
    let budget =
        TestOpeo::call(|out| ExecutionBudget::new(ExecutionLimits::new(1_000_000, 4096), out))
            .map_err(TestFailure::from_debug)?;
    let remaining = budget
        .limits()
        .max_workspace_bytes()
        .saturating_sub(budget.workspace_bytes());
    let _occupied = TestOpeo::call::<BudgetError, _>(|out| {
        ChargedString::with_capacity(remaining, Some(&budget), out)
    })
    .map_err(TestFailure::from_debug)?;

    let result = TestOpeo::call(|out| {
        render_fixed_array_length_mismatch(
            4,
            3,
            Translator::new(Language::English),
            Some(&budget),
            out,
        )
    });
    test_assert!(matches!(
        result,
        Err(BudgetError::ResourceLimit(ResourceLimit::WorkspaceBytes))
    ))?;
    Ok(())
}
