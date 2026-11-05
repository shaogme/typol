use crate::{
    ConfigLimits, ConfigParseOptions,
    context::{
        AssignmentContext, EnvironmentSpans, EnvironmentValue, FallbackValue, SemanticStatus,
    },
    diagnostics::DiagnosticBuffer,
    environment::{
        Environment, EnvironmentCache, EnvironmentResolveRequest, EnvironmentResolver,
        EnvironmentTextError,
    },
    instance::LocationIndex,
    validation::{AssertionsRequest, ValidatedFallback, Validator},
};
use opeo::{ErrSlot, OResult, Out};

use core::cell::RefCell;

use typol_core::{
    BudgetError, ConfigValue, Diagnostic, ExecutionBudget, Read, SecretRange, SourceCapture,
    SourceId, SourceReadError, SourceSpan, SpanLocation,
};
use typol_policy::{RegexLimits, ValueLimits};
use typol_semantic::{
    CompiledSchema, EnvironmentAuthorization, EvalError, RegexExecutionError, RegexExecutor,
    RuleOwnerError, SchemaAccessError,
};
use typol_syntax::StreamLimits;

#[cfg(test)]
use crate::diagnostics::BudgetDiagnostic;

pub(crate) struct ValidationResources<'a> {
    pub(crate) scratch: RegexExecutor,
    pub(crate) value_limits: &'a ValueLimits,
    pub(crate) regex_limits: &'a RegexLimits,
    pub(crate) budget: ExecutionBudget,
    pub(crate) budget_errors: ErrSlot<BudgetError>,
    pub(crate) schema_access_errors: ErrSlot<SchemaAccessError>,
    pub(crate) schema_iteration_errors: ErrSlot<SchemaAccessError>,
    pub(crate) environment_text_errors: ErrSlot<EnvironmentTextError>,
    pub(crate) rule_owner_errors: ErrSlot<RuleOwnerError>,
    pub(crate) eval_errors: ErrSlot<EvalError>,
    pub(crate) regex_errors: ErrSlot<RegexExecutionError>,
}

pub(crate) struct SessionResources<'a> {
    pub(crate) environment: &'a mut dyn Environment,
    pub(crate) cache: EnvironmentCache,
    pub(crate) validation: ValidationResources<'a>,
    pub(crate) budget: ExecutionBudget,
    pub(crate) config_limits: ConfigLimits,
}

pub(crate) struct ConfigSession<'a> {
    stream: StreamLimits,
    resources: SessionResources<'a>,
    capture: Option<&'a RefCell<Option<SourceCapture>>>,
}

pub(crate) struct EnvironmentRequest<'schema, 'auth> {
    pub(crate) metadata: AssignmentContext<'schema>,
    pub(crate) authorization: EnvironmentAuthorization<'auth>,
    pub(crate) fallback: Option<FallbackValue>,
    pub(crate) spans: EnvironmentSpans,
}

pub(crate) struct AssertionRequest<'schema, 'value, 'index> {
    pub(crate) schema: &'schema CompiledSchema,
    pub(crate) root: &'value ConfigValue,
    pub(crate) locations: &'index LocationIndex<'schema>,
    pub(crate) diagnostics: &'index mut DiagnosticBuffer<'schema>,
}

impl<'a> ConfigSession<'a> {
    pub(crate) fn with_capture(mut self, capture: &'a RefCell<Option<SourceCapture>>) -> Self {
        self.capture = Some(capture);
        self
    }

    pub(crate) fn begin_secret(&self, start: usize) -> Option<SecretRange> {
        self.capture.and_then(|capture| {
            capture
                .borrow_mut()
                .as_mut()
                .and_then(|capture| capture.begin_secret(start))
        })
    }

    pub(crate) fn end_secret(&self, secret: SecretRange, end: usize) {
        if let Some(capture) = self.capture {
            if let Some(capture) = capture.borrow_mut().as_mut() {
                capture.end_secret(secret, end);
            }
        }
    }

    pub(crate) fn pin(&self, span: SourceSpan, location: Option<SpanLocation>) {
        if let Some(capture) = self.capture {
            if let Some(capture) = capture.borrow_mut().as_mut() {
                capture.pin(span, location);
            }
        }
    }

    pub(crate) fn invalidate_capture(&self) {
        if let Some(capture) = self.capture {
            if let Some(capture) = capture.borrow_mut().as_mut() {
                capture.invalidate();
            }
        }
    }

    fn pin_diagnostics(&self, diagnostics: &DiagnosticBuffer<'_>) {
        for diagnostic in diagnostics.iter() {
            for span in diagnostic
                .located_primary()
                .into_iter()
                .chain(diagnostic.labels().iter().map(|label| label.located_span()))
            {
                self.pin(span.source_span(), span.location());
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn new<'slot>(
        environment: &'a mut dyn Environment,
        options: &'a ConfigParseOptions,
        budget_errors: &mut ErrSlot<BudgetError>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Self> {
        let budget = match budget_errors
            .try_call(|budget_out| ExecutionBudget::new(*options.execution(), budget_out))
        {
            Ok(budget) => budget,
            Err(caught) => {
                return Err(out.fail(BudgetDiagnostic::from_error(caught.take())));
            }
        };
        Self::with_budget(environment, options, &budget, out.reborrow())
    }

    pub(crate) fn with_budget<'slot, E>(
        environment: &'a mut dyn Environment,
        options: &'a ConfigParseOptions,
        budget: &ExecutionBudget,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, Self>
    where
        E: From<BudgetError>,
    {
        let value_limits = options.config().values();
        let regex_limits = options.resources().regex();
        let cache = EnvironmentCache::new(budget, *options.config(), out.reborrow())?;
        let validation_budget = budget.try_clone(out.reborrow())?;
        let session_budget = budget.try_clone(out.reborrow())?;
        Ok(Self {
            stream: options.stream(),
            capture: None,
            resources: SessionResources {
                environment,
                cache,
                validation: ValidationResources {
                    scratch: RegexExecutor::new(),
                    value_limits,
                    regex_limits,
                    budget: validation_budget,
                    budget_errors: ErrSlot::new(),
                    schema_access_errors: ErrSlot::new(),
                    schema_iteration_errors: ErrSlot::new(),
                    environment_text_errors: ErrSlot::new(),
                    rule_owner_errors: ErrSlot::new(),
                    eval_errors: ErrSlot::new(),
                    regex_errors: ErrSlot::new(),
                },
                budget: session_budget,
                config_limits: *options.config(),
            },
        })
    }

    pub(crate) fn stream(&self) -> StreamLimits {
        self.stream
    }

    pub(crate) fn validation(&mut self) -> &mut ValidationResources<'a> {
        &mut self.resources.validation
    }

    pub(crate) fn budget(&self) -> &ExecutionBudget {
        &self.resources.budget
    }

    pub(crate) fn config_limits(&self) -> ConfigLimits {
        self.resources.config_limits
    }

    pub(crate) fn environment<'slot>(
        &mut self,
        request: EnvironmentRequest<'_, '_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Option<EnvironmentValue>> {
        EnvironmentResolver::resolve_environment(
            EnvironmentResolveRequest {
                metadata: request.metadata,
                authorization: request.authorization,
                fallback: request.fallback,
                spans: request.spans,
                resources: &mut self.resources,
            },
            out.reborrow(),
        )
    }

    pub(crate) fn validate_fallback<'slot>(
        &mut self,
        metadata: AssignmentContext<'_>,
        value: &ConfigValue,
        diagnostics: &mut DiagnosticBuffer,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Option<ValidatedFallback>> {
        let result = Validator::fallback(
            metadata,
            value,
            &mut self.resources.validation,
            diagnostics,
            out.reborrow(),
        );
        self.pin_diagnostics(diagnostics);
        result
    }

    pub(crate) fn completed<'slot>(
        &mut self,
        metadata: AssignmentContext<'_>,
        value: &ConfigValue,
        diagnostics: &mut DiagnosticBuffer,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, SemanticStatus> {
        let result = Validator::completed(
            metadata,
            value.root(),
            &mut self.resources.validation,
            diagnostics,
            out.reborrow(),
        );
        self.pin_diagnostics(diagnostics);
        result
    }

    pub(crate) fn assertions<'slot>(
        &mut self,
        request: AssertionRequest<'_, '_, '_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, SemanticStatus> {
        let result = Validator::assertions(
            AssertionsRequest {
                schema: request.schema,
                root: request.root,
                locations: request.locations,
                resources: &mut self.resources.validation,
                diagnostics: request.diagnostics,
            },
            out.reborrow(),
        );
        self.pin_diagnostics(request.diagnostics);
        result
    }
}

pub(crate) struct TrackingInput<'input, R: Read + ?Sized = dyn Read> {
    source: &'input mut R,
    failed: bool,
    fetched: usize,
    limit: usize,
    capture: Option<&'input RefCell<Option<SourceCapture>>>,
    source_errors: ErrSlot<SourceReadError>,
}

impl<'input, R: Read + ?Sized> TrackingInput<'input, R> {
    pub(crate) fn new(source: &'input mut R, limit: usize) -> Self {
        Self {
            source,
            failed: false,
            fetched: 0,
            limit,
            capture: None,
            source_errors: ErrSlot::new(),
        }
    }

    pub(crate) fn with_capture(mut self, capture: &'input RefCell<Option<SourceCapture>>) -> Self {
        self.capture = Some(capture);
        self
    }

    pub(crate) fn failed(&self) -> bool {
        self.failed
    }
}

impl<R: Read + ?Sized> Read<Diagnostic> for TrackingInput<'_, R> {
    fn read<'slot, 'borrow>(
        &mut self,
        output: &mut [u8],
        source_id: SourceId,
        offset: usize,
        out: Out<'slot, 'borrow, Diagnostic>,
    ) -> OResult<'slot, usize> {
        let count = match self
            .source_errors
            .try_call(|source_out| self.source.read(output, source_id, offset, source_out))
        {
            Ok(count) => count,
            Err(caught) => {
                self.failed = true;
                return Err(out.fail(Diagnostic::from(caught.take())));
            }
        };
        self.failed |= count > output.len();
        if let Some(capture) = self.capture {
            if count == 0 {
                if let Some(capture) = capture.borrow_mut().as_mut() {
                    capture.end_input();
                }
            } else if let Some(bytes) = output.get(..count) {
                if let Some(capture) = capture.borrow_mut().as_mut() {
                    capture.feed(bytes);
                }
            } else {
                if let Some(capture) = capture.borrow_mut().as_mut() {
                    capture.invalidate();
                }
            }
        }
        match self.fetched.checked_add(count) {
            Some(fetched) => {
                self.fetched = fetched;
                self.failed |= fetched > self.limit;
            }
            None => self.failed = true,
        }
        Ok(count)
    }
}
