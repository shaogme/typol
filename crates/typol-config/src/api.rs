use crate::{
    ConfigLimits,
    diagnostics::{
        BudgetDiagnostic, ConfigFailureStage, ConfigParseErrors, ConfigValueErrors,
        DiagnosticBuffer, DiagnosticFactory,
    },
    document::{ConfigDocument, ConfigDocumentInput},
    environment::Environment,
    instance::LocationIndex,
    options::{ConfigParseRequest, ConfigValidationRequest, RequestParts},
    parser::Parser,
    secret::SecretPolicy,
    session::{ConfigSession, TrackingInput},
    value::{NormalizedConfigValue, ValidatedConfigValue},
    value_normalization::{NormalizeRequest, ValueNormalizer},
    value_validation::{ValueValidation, ValueValidationInput, ValueValidationRequest},
};
use ::opeo::{ErrSlot, OResult, Out};
use core::cell::RefCell;
use typol_core::{
    BudgetError, ChargedVec, ConfigValue, Diagnostic, DiagnosticCode, DiagnosticContext,
    ExcerptLimits, ExecutionBudget, Read, SourceCapture, SourceId, diagnostic_context,
};
use typol_semantic::CompiledSchema;

/// Parses configuration text against one compiled Schema.
pub struct ConfigParser<'schema> {
    schema: &'schema CompiledSchema,
}

/// Normalizes and validates external values against one compiled Schema.
pub struct ConfigValueProcessor<'schema> {
    schema: &'schema CompiledSchema,
}

/// Redacts Rust conversion diagnostics using one compiled Schema's secret boundaries.
pub struct ConfigDiagnosticRedactor<'schema> {
    schema: &'schema CompiledSchema,
}

struct ParseSourceRequest<'schema, 'input, 'environment, 'budget, R: Read + ?Sized> {
    schema: &'schema CompiledSchema,
    parts: RequestParts<'input, R>,
    environment: &'environment mut dyn Environment,
    budget: &'budget ExecutionBudget,
    source_id: SourceId,
}

struct ParseFailureRequest<'schema, 'budget> {
    errors: ConfigParseErrors,
    schema: &'schema CompiledSchema,
    excerpt_limits: ExcerptLimits,
    budget: &'budget ExecutionBudget,
    capture: Option<SourceCapture>,
    input_failed: bool,
}

struct ParserSetupRequest<'schema, 'source, 'limits, 'input, 'name, R: Read + ?Sized> {
    schema: &'schema CompiledSchema,
    source: &'source mut TrackingInput<'input, R>,
    limits: &'limits ConfigLimits,
    session: ConfigSession<'limits>,
    source_name: &'name str,
}

struct DocumentPartsRequest<'input, 'environment, 'budget, R: Read + ?Sized> {
    request: ConfigParseRequest<'input, R>,
    environment: &'environment mut dyn Environment,
    budget: &'budget ExecutionBudget,
    retain_source: bool,
}

pub(crate) struct DocumentParseOutput<'schema> {
    pub(crate) value: ValidatedConfigValue,
    pub(crate) locations: LocationIndex<'schema>,
    pub(crate) source: Option<ChargedVec<u8>>,
}

static CONFIG_PARSE_BUDGET_MISMATCH: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "config-parse-entry",
    "execution-budget-does-not-match-options",
);

impl<'schema> ConfigParser<'schema> {
    pub const fn new(schema: &'schema CompiledSchema) -> Self {
        Self { schema }
    }

    /// Parse and fully validate configuration against this compiled Schema.
    pub fn parse<'slot, R: Read + ?Sized>(
        &self,
        request: ConfigParseRequest<'_, R>,
        environment: &mut dyn Environment,
        out: Out<'slot, '_, ConfigParseErrors>,
    ) -> OResult<'slot, ValidatedConfigValue> {
        parse_for_schema(self.schema, request, environment, out)
    }

    /// Parse configuration while sharing the caller's execution budget.
    pub fn parse_with_budget<'slot, R: Read + ?Sized>(
        &self,
        request: ConfigParseRequest<'_, R>,
        environment: &mut dyn Environment,
        budget: &ExecutionBudget,
        out: Out<'slot, '_, ConfigParseErrors>,
    ) -> OResult<'slot, ValidatedConfigValue> {
        parse_for_schema_with_budget(self.schema, request, environment, budget, out)
    }

    /// Parse and validate a complete source while retaining its text and source locations.
    pub fn parse_document<'slot, R: Read + ?Sized>(
        &self,
        request: ConfigParseRequest<'_, R>,
        environment: &mut dyn Environment,
        mut out: Out<'slot, '_, ConfigParseErrors>,
    ) -> OResult<'slot, ConfigDocument<'schema>> {
        check_source(self.schema, &request, out.reborrow())?;
        let mut budget_errors = ErrSlot::new();
        let budget = match budget_errors
            .try_call(|budget_out| ExecutionBudget::new(*request.options().execution(), budget_out))
        {
            Ok(budget) => budget,
            Err(caught) => {
                return Err(out.fail(ConfigParseErrors::emergency(
                    BudgetDiagnostic::from_error(caught.take()),
                    self.schema.next_source_id(),
                )));
            }
        };
        self.parse_document_with_budget(request, environment, &budget, out.reborrow())
    }

    /// Parse a retained source while sharing the caller's execution budget.
    pub fn parse_document_with_budget<'slot, 'borrow, R: Read + ?Sized>(
        &self,
        request: ConfigParseRequest<'_, R>,
        environment: &mut dyn Environment,
        budget: &ExecutionBudget,
        mut out: Out<'slot, 'borrow, ConfigParseErrors>,
    ) -> OResult<'slot, ConfigDocument<'schema>> {
        let options = *request.options();
        let display_name = request.display_name();
        let parsed = self.parse_document_parts_with_budget(
            DocumentPartsRequest {
                request,
                environment,
                budget,
                retain_source: true,
            },
            out.reborrow(),
        )?;
        let source = parsed.source.ok_or_else(|| {
            out.reborrow().fail(ConfigParseErrors::emergency(
                Diagnostic::emergency(DiagnosticCode::INTERNAL_ERROR),
                self.schema.next_source_id(),
            ))
        })?;
        ConfigDocument::from_parse(
            ConfigDocumentInput {
                schema: self.schema,
                options,
                display_name,
                value: parsed.value,
                locations: parsed.locations,
                source,
                budget,
            },
            out,
        )
    }

    pub(crate) fn parse_document_without_source_retention_with_budget<
        'slot,
        'borrow,
        R: Read + ?Sized,
    >(
        &self,
        request: ConfigParseRequest<'_, R>,
        environment: &mut dyn Environment,
        budget: &ExecutionBudget,
        out: Out<'slot, 'borrow, ConfigParseErrors>,
    ) -> OResult<'slot, DocumentParseOutput<'schema>> {
        self.parse_document_parts_with_budget(
            DocumentPartsRequest {
                request,
                environment,
                budget,
                retain_source: false,
            },
            out,
        )
    }

    fn parse_document_parts_with_budget<'slot, 'borrow, 'input, 'environment, 'budget, R>(
        &self,
        request: DocumentPartsRequest<'input, 'environment, 'budget, R>,
        mut out: Out<'slot, 'borrow, ConfigParseErrors>,
    ) -> OResult<'slot, DocumentParseOutput<'schema>>
    where
        R: Read + ?Sized,
    {
        let DocumentPartsRequest {
            request,
            environment,
            budget,
            retain_source,
        } = request;
        if budget.limits() != *request.options().execution() {
            return Err(out.fail(ConfigParseErrors::emergency(
                DiagnosticFactory::internal(CONFIG_PARSE_BUDGET_MISMATCH, None, None, None),
                self.schema.next_source_id(),
            )));
        }
        check_source(self.schema, &request, out.reborrow())?;
        let parts = request.into_parts();
        let Some(source_id) = self.schema.next_source_id() else {
            return Err(out.fail(ConfigParseErrors::emergency(
                BudgetDiagnostic::from_error(BudgetError::Resource),
                None,
            )));
        };
        let parsed = parse_document_source(
            ParseSourceRequest {
                schema: self.schema,
                parts,
                environment,
                budget,
                source_id,
            },
            retain_source,
            out.reborrow(),
        )?;
        Ok(parsed)
    }
}

impl<'schema> ConfigValueProcessor<'schema> {
    pub const fn new(schema: &'schema CompiledSchema) -> Self {
        Self { schema }
    }

    /// Normalize an owned external value with Schema defaults, without evaluating constraints.
    pub fn normalize<'slot>(
        &self,
        value: ConfigValue,
        request: ConfigValidationRequest,
        out: Out<'slot, '_, ConfigValueErrors>,
    ) -> OResult<'slot, NormalizedConfigValue> {
        normalize_for_schema(self.schema, value, request, out)
    }

    /// Normalize an owned external value while sharing the caller's execution budget.
    pub fn normalize_with_budget<'slot, 'borrow>(
        &self,
        value: ConfigValue,
        request: ConfigValidationRequest,
        budget: &ExecutionBudget,
        out: Out<'slot, 'borrow, ConfigValueErrors>,
    ) -> OResult<'slot, NormalizedConfigValue> {
        normalize_for_schema_with_budget(self.schema, value, request, budget, out)
    }

    /// Normalize an owned external value and apply complete Schema validation.
    pub fn validate<'slot>(
        &self,
        value: ConfigValue,
        request: ConfigValidationRequest,
        out: Out<'slot, '_, ConfigValueErrors>,
    ) -> OResult<'slot, ValidatedConfigValue> {
        validate_for_schema(self.schema, value, request, out)
    }

    /// Validate an owned external value while sharing the caller's execution budget.
    pub fn validate_with_budget<'slot, 'borrow>(
        &self,
        value: ConfigValue,
        request: ConfigValidationRequest,
        budget: &ExecutionBudget,
        out: Out<'slot, 'borrow, ConfigValueErrors>,
    ) -> OResult<'slot, ValidatedConfigValue> {
        validate_for_schema_with_budget(self.schema, value, request, budget, out)
    }

    /// Fully validate a Schema-complete value without rebuilding its value tree.
    #[doc(hidden)]
    pub fn validate_schema_complete_with_budget<'slot, 'borrow>(
        &self,
        value: ConfigValue,
        request: ConfigValidationRequest,
        budget: &ExecutionBudget,
        out: Out<'slot, 'borrow, ConfigValueErrors>,
    ) -> OResult<'slot, ValidatedConfigValue> {
        validate_complete_for_schema_with_budget(self.schema, value, request, budget, out)
    }
}

impl<'schema> ConfigDiagnosticRedactor<'schema> {
    pub const fn new(schema: &'schema CompiledSchema) -> Self {
        Self { schema }
    }

    /// Redact a Rust value conversion diagnostic according to this Schema's `@secret` boundary.
    pub fn redact_conversion<'slot, 'borrow>(
        &self,
        diagnostic: &mut Diagnostic,
        out: Out<'slot, 'borrow, Diagnostic>,
    ) -> OResult<'slot, ()> {
        SecretPolicy::redact_conversion(self.schema, diagnostic, out)
    }
}

static CONFIG_VALUE_BUDGET_MISMATCH: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "config-value-validation-entry",
    "execution-budget-does-not-match-options",
);
static CONFIG_VALUE_NORMALIZATION_BUDGET_MISMATCH: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "config-value-normalization-entry",
    "execution-budget-does-not-match-options",
);

fn parse_source<'slot, R: Read + ?Sized>(
    request: ParseSourceRequest<'_, '_, '_, '_, R>,
    mut out: Out<'slot, '_, ConfigParseErrors>,
) -> OResult<'slot, ValidatedConfigValue> {
    let ParseSourceRequest {
        schema,
        parts,
        environment,
        budget,
        source_id,
    } = request;
    let options = &parts.options;
    let capture = RefCell::new(Some(SourceCapture::new(
        source_id,
        *options.excerpts(),
        Some(budget),
    )));
    let session = ConfigSession::with_budget(environment, options, budget, out.reborrow())?
        .with_capture(&capture);
    let mut input =
        TrackingInput::new(parts.input, options.config().max_source_bytes()).with_capture(&capture);
    let mut parser_slot = None;
    prepare_parser(
        ParserSetupRequest {
            schema,
            source: &mut input,
            limits: options.config(),
            session,
            source_name: parts.display_name,
        },
        &mut parser_slot,
        out.reborrow(),
    )?;
    let mut parse_errors = ErrSlot::new();
    let parse_result = parse_errors
        .try_call(|parse_out| Parser::parse_slot(parser_slot.take(), source_id, parse_out));
    match parse_result {
        Ok(value) => Ok(ValidatedConfigValue::from_config_value(value)),
        Err(caught) => finish_parse_failure(
            ParseFailureRequest {
                errors: caught.take(),
                schema,
                excerpt_limits: *options.excerpts(),
                budget,
                capture: capture.borrow_mut().take(),
                input_failed: input.failed(),
            },
            out,
        ),
    }
}

fn parse_document_source<'slot, 'schema, R: Read + ?Sized>(
    request: ParseSourceRequest<'schema, '_, '_, '_, R>,
    retain_source: bool,
    mut out: Out<'slot, '_, ConfigParseErrors>,
) -> OResult<'slot, DocumentParseOutput<'schema>> {
    let ParseSourceRequest {
        schema,
        parts,
        environment,
        budget,
        source_id,
    } = request;
    let options = &parts.options;
    let capture = RefCell::new(Some(SourceCapture::new(
        source_id,
        *options.excerpts(),
        Some(budget),
    )));
    let session = ConfigSession::with_budget(environment, options, budget, out.reborrow())?
        .with_capture(&capture);
    let input =
        TrackingInput::new(parts.input, options.config().max_source_bytes()).with_capture(&capture);
    let mut input = input;
    if retain_source {
        let mut retention_errors = ErrSlot::<Diagnostic>::new();
        input = match retention_errors
            .try_call(|diagnostic_out| input.retain_source(budget, diagnostic_out))
        {
            Ok(input) => input,
            Err(caught) => {
                return Err(out.fail(ConfigParseErrors::emergency(caught.take(), Some(source_id))));
            }
        };
    }
    let mut parser_slot = None;
    prepare_parser(
        ParserSetupRequest {
            schema,
            source: &mut input,
            limits: options.config(),
            session,
            source_name: parts.display_name,
        },
        &mut parser_slot,
        out.reborrow(),
    )?;
    let mut parse_errors = ErrSlot::new();
    let parse_result = parse_errors.try_call(|parse_out| {
        Parser::parse_slot_with_locations(parser_slot.take(), source_id, parse_out)
    });
    match parse_result {
        Ok((value, locations)) => Ok(DocumentParseOutput {
            value: ValidatedConfigValue::from_config_value(value),
            locations,
            source: input.take_retained_source(),
        }),
        Err(caught) => finish_parse_failure(
            ParseFailureRequest {
                errors: caught.take(),
                schema,
                excerpt_limits: *options.excerpts(),
                budget,
                capture: capture.borrow_mut().take(),
                input_failed: input.failed(),
            },
            out,
        ),
    }
}

fn prepare_parser<'schema, 'source, 'limits, 'input, 'name, 'slot, R: Read + ?Sized>(
    request: ParserSetupRequest<'schema, 'source, 'limits, 'input, 'name, R>,
    parser_slot: &mut Option<Parser<'schema, 'source, 'limits>>,
    mut out: Out<'slot, '_, ConfigParseErrors>,
) -> OResult<'slot, ()> {
    let ParserSetupRequest {
        schema,
        source,
        limits,
        session,
        source_name,
    } = request;
    let mut parser = Parser::new(schema, source, limits, session, out.reborrow())?;
    parser.set_source_name(source_name, out.reborrow())?;
    *parser_slot = Some(parser);
    Ok(())
}

fn finish_parse_failure<'slot, T>(
    request: ParseFailureRequest<'_, '_>,
    out: Out<'slot, '_, ConfigParseErrors>,
) -> OResult<'slot, T> {
    let ParseFailureRequest {
        errors,
        schema,
        excerpt_limits,
        budget,
        capture,
        input_failed,
    } = request;
    let mut excerpts = errors.schema_excerpts(schema, excerpt_limits, budget);
    if let Some(mut capture) = capture {
        capture.seal();
        capture.finish(&mut excerpts, errors.diagnostics());
    }
    let errors = errors.with_excerpts(excerpts);
    Err(out.fail(if input_failed {
        errors.with_stage(ConfigFailureStage::Input)
    } else {
        errors
    }))
}

/// Parse and fully validate configuration against a compiled Schema.
///
/// The result carries a validation marker; call `into_config_value()` to consume it.
fn parse_for_schema<'slot, R: Read + ?Sized>(
    schema: &CompiledSchema,
    request: ConfigParseRequest<'_, R>,
    environment: &mut dyn Environment,
    mut out: Out<'slot, '_, ConfigParseErrors>,
) -> OResult<'slot, ValidatedConfigValue> {
    check_source(schema, &request, out.reborrow())?;
    let mut budget_errors = ErrSlot::new();
    let budget = match budget_errors
        .try_call(|budget_out| ExecutionBudget::new(*request.options().execution(), budget_out))
    {
        Ok(budget) => budget,
        Err(caught) => {
            return Err(out.fail(ConfigParseErrors::emergency(
                BudgetDiagnostic::from_error(caught.take()),
                schema.next_source_id(),
            )));
        }
    };
    parse_for_schema_with_budget(schema, request, environment, &budget, out.reborrow())
}

/// Internal session seam that reuses the call budget to fully parse and validate configuration.
fn parse_for_schema_with_budget<'slot, R: Read + ?Sized>(
    schema: &CompiledSchema,
    request: ConfigParseRequest<'_, R>,
    environment: &mut dyn Environment,
    budget: &ExecutionBudget,
    mut out: Out<'slot, '_, ConfigParseErrors>,
) -> OResult<'slot, ValidatedConfigValue> {
    if budget.limits() != *request.options().execution() {
        let diagnostic =
            DiagnosticFactory::internal(CONFIG_PARSE_BUDGET_MISMATCH, None, None, None);
        return Err(out.fail(ConfigParseErrors::emergency(
            diagnostic,
            schema.next_source_id(),
        )));
    }
    check_source(schema, &request, out.reborrow())?;
    let parts = request.into_parts();
    let Some(source_id) = schema.next_source_id() else {
        return Err(out.fail(ConfigParseErrors::emergency(
            BudgetDiagnostic::from_error(BudgetError::Resource),
            None,
        )));
    };
    parse_source(
        ParseSourceRequest {
            schema,
            parts,
            environment,
            budget,
            source_id,
        },
        out,
    )
}

/// Normalize an owned external value and apply complete Schema validation.
fn validate_for_schema<'slot>(
    schema: &CompiledSchema,
    value: ConfigValue,
    request: ConfigValidationRequest,
    mut out: Out<'slot, '_, ConfigValueErrors>,
) -> OResult<'slot, ValidatedConfigValue> {
    let mut budget_errors = ErrSlot::new();
    let budget = match budget_errors
        .try_call(|budget_out| ExecutionBudget::new(*request.options().execution(), budget_out))
    {
        Ok(budget) => budget,
        Err(caught) => {
            return Err(out.fail(ConfigValueErrors::from_errors(
                ConfigParseErrors::emergency(BudgetDiagnostic::from_error(caught.take()), None),
            )));
        }
    };
    validate_for_schema_with_budget(schema, value, request, &budget, out.reborrow())
}

/// Normalize an owned external value with Schema defaults, without evaluating constraints.
fn normalize_for_schema<'slot>(
    schema: &CompiledSchema,
    value: ConfigValue,
    request: ConfigValidationRequest,
    mut out: Out<'slot, '_, ConfigValueErrors>,
) -> OResult<'slot, NormalizedConfigValue> {
    let mut budget_errors = ErrSlot::new();
    let budget = match budget_errors
        .try_call(|budget_out| ExecutionBudget::new(*request.options().execution(), budget_out))
    {
        Ok(budget) => budget,
        Err(caught) => {
            return Err(out.fail(ConfigValueErrors::from_errors(
                ConfigParseErrors::emergency(BudgetDiagnostic::from_error(caught.take()), None),
            )));
        }
    };
    normalize_for_schema_with_budget(schema, value, request, &budget, out.reborrow())
}

/// Shared-budget entry point that normalizes an owned value without validation.
fn normalize_for_schema_with_budget<'slot, 'borrow>(
    schema: &CompiledSchema,
    value: ConfigValue,
    request: ConfigValidationRequest,
    budget: &ExecutionBudget,
    out: Out<'slot, 'borrow, ConfigValueErrors>,
) -> OResult<'slot, NormalizedConfigValue> {
    if budget.limits() != *request.options().execution() {
        return Err(out.fail(ConfigValueErrors::from_errors(
            ConfigParseErrors::emergency(
                DiagnosticFactory::internal(
                    CONFIG_VALUE_NORMALIZATION_BUDGET_MISMATCH,
                    None,
                    None,
                    None,
                ),
                None,
            ),
        )));
    }
    let mut operation_errors = ErrSlot::new();
    let mut diagnostics = match operation_errors.try_call(|diagnostic_out| {
        DiagnosticBuffer::new_budgeted(
            schema,
            request.options().config().max_diagnostics(),
            budget,
            diagnostic_out,
        )
    }) {
        Ok(diagnostics) => diagnostics,
        Err(caught) => {
            return Err(out.fail(ConfigValueErrors::from_errors(
                ConfigParseErrors::emergency(caught.take(), None),
            )));
        }
    };
    let mut nodes = 0;
    let result = operation_errors.try_call(|diagnostic_out| {
        ValueNormalizer::copy(
            NormalizeRequest {
                expected: schema.entry(),
                value: value.root(),
                depth: 0,
                primary: None,
                text_limit: request.options().config().max_constructed_text_bytes(),
                schema_default: false,
            },
            &mut nodes,
            request.options().config().values(),
            budget,
            diagnostic_out,
        )
    });
    match result {
        Ok(value) => Ok(NormalizedConfigValue::from_config_value(
            value.into_config_value(),
        )),
        Err(caught) => {
            diagnostics.push(caught.take());
            let errors = ConfigParseErrors::finish(diagnostics, schema, None, None);
            let excerpts = errors.schema_excerpts(schema, *request.options().excerpts(), budget);
            Err(out.fail(ConfigValueErrors::from_errors(
                errors.with_excerpts(excerpts),
            )))
        }
    }
}

/// Internal session seam that reuses the call budget to normalize and fully validate an owned tree.
fn validate_for_schema_with_budget<'slot, 'borrow>(
    schema: &CompiledSchema,
    value: ConfigValue,
    request: ConfigValidationRequest,
    budget: &ExecutionBudget,
    out: Out<'slot, 'borrow, ConfigValueErrors>,
) -> OResult<'slot, ValidatedConfigValue> {
    if budget.limits() != *request.options().execution() {
        return Err(out.fail(ConfigValueErrors::from_errors(
            ConfigParseErrors::emergency(
                DiagnosticFactory::internal(CONFIG_VALUE_BUDGET_MISMATCH, None, None, None),
                None,
            ),
        )));
    }
    let mut validation_errors = ErrSlot::new();
    match validation_errors.try_call(|validation_out| {
        ValueValidation::value(
            ValueValidationRequest {
                schema,
                value: ValueValidationInput::External(&value),
                options: request.options(),
                budget,
            },
            validation_out,
        )
    }) {
        Ok(value) => Ok(ValidatedConfigValue::from_config_value(value)),
        Err(caught) => {
            let errors = caught.take();
            let excerpts = errors.schema_excerpts(schema, *request.options().excerpts(), budget);
            Err(out.fail(ConfigValueErrors::from_errors(
                errors.with_excerpts(excerpts),
            )))
        }
    }
}

/// Fully validate a schema-complete value without rebuilding its value tree.
///
/// Binding encoders use this entry point; external values should use
/// the budgeted validation method instead.
#[doc(hidden)]
fn validate_complete_for_schema_with_budget<'slot, 'borrow>(
    schema: &CompiledSchema,
    value: ConfigValue,
    request: ConfigValidationRequest,
    budget: &ExecutionBudget,
    out: Out<'slot, 'borrow, ConfigValueErrors>,
) -> OResult<'slot, ValidatedConfigValue> {
    if budget.limits() != *request.options().execution() {
        return Err(out.fail(ConfigValueErrors::from_errors(
            ConfigParseErrors::emergency(
                DiagnosticFactory::internal(CONFIG_VALUE_BUDGET_MISMATCH, None, None, None),
                None,
            ),
        )));
    }
    let mut validation_errors = ErrSlot::new();
    match validation_errors.try_call(|validation_out| {
        ValueValidation::value(
            ValueValidationRequest {
                schema,
                value: ValueValidationInput::SchemaComplete(value),
                options: request.options(),
                budget,
            },
            validation_out,
        )
    }) {
        Ok(value) => Ok(ValidatedConfigValue::from_config_value(value)),
        Err(caught) => {
            let errors = caught.take();
            let excerpts = errors.schema_excerpts(schema, *request.options().excerpts(), budget);
            Err(out.fail(ConfigValueErrors::from_errors(
                errors.with_excerpts(excerpts),
            )))
        }
    }
}

fn check_source<'slot, 'borrow, R: Read + ?Sized>(
    schema: &CompiledSchema,
    request: &ConfigParseRequest<'_, R>,
    out: Out<'slot, 'borrow, ConfigParseErrors>,
) -> OResult<'slot, ()> {
    let options = request.options();
    if request.display_name().len() > options.config().max_display_name_bytes() {
        return Err(out.fail(ConfigParseErrors::emergency(
            BudgetDiagnostic::from_error(BudgetError::Resource),
            schema.next_source_id(),
        )));
    }
    Ok(())
}
