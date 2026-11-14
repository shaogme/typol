mod processing;

pub use processing::ConfigValueProcessor;

use crate::{
    ConfigLimits,
    diagnostics::{BudgetDiagnostic, ConfigFailureStage, ConfigParseErrors, DiagnosticFactory},
    document::{ConfigDocument, ConfigDocumentInput},
    environment::Environment,
    instance::LocationIndex,
    options::{ConfigParseRequest, RequestParts},
    parser::Parser,
    secret::SecretPolicy,
    session::{ConfigSession, TrackingInput},
    value::ValidatedConfigValue,
};
use ::opeo::{ErrSlot, OResult, Out};
use core::cell::RefCell;
use typol_core::{
    BudgetError, ChargedVec, Diagnostic, DiagnosticContext, ExcerptLimits, ExecutionBudget, Read,
    SourceCapture, SourceId, diagnostic_context,
};
use typol_i18n::Translator;
use typol_semantic::CompiledSchema;

/// Parses configuration text against one compiled Schema.
pub struct ConfigParser<'schema> {
    schema: &'schema CompiledSchema,
}

/// Redacts Rust conversion diagnostics using one compiled Schema's secret boundaries.
pub struct ConfigDiagnosticRedactor<'schema> {
    schema: &'schema CompiledSchema,
    translator: Translator,
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
static CONFIG_MISSING_TERMINAL_DIAGNOSTIC: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "config-document-entry",
    "source-retention-result-missing",
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
        let translator = request.options().translator();
        check_source(self.schema, &request, out.reborrow())?;
        let mut budget_errors = ErrSlot::new();
        let budget = match budget_errors
            .try_call(|budget_out| ExecutionBudget::new(*request.options().execution(), budget_out))
        {
            Ok(budget) => budget,
            Err(caught) => {
                return Err(out.fail(ConfigParseErrors::emergency(
                    BudgetDiagnostic::from_error(caught.take(), translator),
                    self.schema.next_source_id(),
                    translator,
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
        let translator = options.translator();
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
                DiagnosticFactory::internal(
                    CONFIG_MISSING_TERMINAL_DIAGNOSTIC,
                    None,
                    None,
                    None,
                    translator,
                ),
                self.schema.next_source_id(),
                translator,
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
        let translator = request.options().translator();
        if budget.limits() != *request.options().execution() {
            return Err(out.fail(ConfigParseErrors::emergency(
                DiagnosticFactory::internal(
                    CONFIG_PARSE_BUDGET_MISMATCH,
                    None,
                    None,
                    None,
                    translator,
                ),
                self.schema.next_source_id(),
                translator,
            )));
        }
        check_source(self.schema, &request, out.reborrow())?;
        let parts = request.into_parts();
        let Some(source_id) = self.schema.next_source_id() else {
            return Err(out.fail(ConfigParseErrors::emergency(
                BudgetDiagnostic::from_error(BudgetError::Resource, translator),
                None,
                translator,
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

impl<'schema> ConfigDiagnosticRedactor<'schema> {
    pub const fn new(schema: &'schema CompiledSchema, translator: Translator) -> Self {
        Self { schema, translator }
    }

    /// Redact a Rust value conversion diagnostic according to this Schema's `@secret` boundary.
    pub fn redact_conversion<'slot, 'borrow>(
        &self,
        diagnostic: &mut Diagnostic,
        out: Out<'slot, 'borrow, Diagnostic>,
    ) -> OResult<'slot, ()> {
        SecretPolicy::redact_conversion(self.schema, diagnostic, self.translator, out)
    }
}

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
    let mut session_errors = ErrSlot::new();
    let session = match session_errors.try_call(|session_out| {
        ConfigSession::with_budget(environment, options, budget, session_out)
    }) {
        Ok(session) => session,
        Err(caught) => {
            return Err(out.fail(ConfigParseErrors::emergency(
                caught.take(),
                Some(source_id),
                options.translator(),
            )));
        }
    }
    .with_capture(&capture);
    let mut input = TrackingInput::new(
        parts.input,
        options.config().max_source_bytes(),
        options.translator(),
    )
    .with_capture(&capture);
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
        Parser::parse_slot(
            parser_slot.take(),
            source_id,
            options.translator(),
            parse_out,
        )
    });
    drop(parser_slot);
    match parse_result {
        Ok(value) => Ok(ValidatedConfigValue::from_config_value(schema, value)),
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
    let mut session_errors = ErrSlot::new();
    let session = match session_errors.try_call(|session_out| {
        ConfigSession::with_budget(environment, options, budget, session_out)
    }) {
        Ok(session) => session,
        Err(caught) => {
            return Err(out.fail(ConfigParseErrors::emergency(
                caught.take(),
                Some(source_id),
                options.translator(),
            )));
        }
    }
    .with_capture(&capture);
    let input = TrackingInput::new(
        parts.input,
        options.config().max_source_bytes(),
        options.translator(),
    )
    .with_capture(&capture);
    let mut input = input;
    if retain_source {
        let mut retention_errors = ErrSlot::<Diagnostic>::new();
        input = match retention_errors
            .try_call(|diagnostic_out| input.retain_source(budget, diagnostic_out))
        {
            Ok(input) => input,
            Err(caught) => {
                return Err(out.fail(ConfigParseErrors::emergency(
                    caught.take(),
                    Some(source_id),
                    options.translator(),
                )));
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
        Parser::parse_slot_with_locations(
            parser_slot.take(),
            source_id,
            options.translator(),
            parse_out,
        )
    });
    drop(parser_slot);
    match parse_result {
        Ok((value, locations)) => Ok(DocumentParseOutput {
            value: ValidatedConfigValue::from_config_value(schema, value),
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
    let translator = request.options().translator();
    check_source(schema, &request, out.reborrow())?;
    let mut budget_errors = ErrSlot::new();
    let budget = match budget_errors
        .try_call(|budget_out| ExecutionBudget::new(*request.options().execution(), budget_out))
    {
        Ok(budget) => budget,
        Err(caught) => {
            return Err(out.fail(ConfigParseErrors::emergency(
                BudgetDiagnostic::from_error(caught.take(), translator),
                schema.next_source_id(),
                translator,
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
    let translator = request.options().translator();
    if budget.limits() != *request.options().execution() {
        let diagnostic =
            DiagnosticFactory::internal(CONFIG_PARSE_BUDGET_MISMATCH, None, None, None, translator);
        return Err(out.fail(ConfigParseErrors::emergency(
            diagnostic,
            schema.next_source_id(),
            translator,
        )));
    }
    check_source(schema, &request, out.reborrow())?;
    let parts = request.into_parts();
    let Some(source_id) = schema.next_source_id() else {
        return Err(out.fail(ConfigParseErrors::emergency(
            BudgetDiagnostic::from_error(BudgetError::Resource, translator),
            None,
            translator,
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

fn check_source<'slot, 'borrow, R: Read + ?Sized>(
    schema: &CompiledSchema,
    request: &ConfigParseRequest<'_, R>,
    out: Out<'slot, 'borrow, ConfigParseErrors>,
) -> OResult<'slot, ()> {
    let options = request.options();
    if request.display_name().len() > options.config().max_display_name_bytes() {
        return Err(out.fail(ConfigParseErrors::emergency(
            BudgetDiagnostic::from_error(BudgetError::Resource, options.translator()),
            schema.next_source_id(),
            options.translator(),
        )));
    }
    Ok(())
}
