mod containers;
mod context;
mod enums;
mod frames;
mod literals;
mod map_index;
mod recovery;
mod semantics;

#[cfg(test)]
mod tests;

use self::{
    context::{ParserContextStorage, ParserControlRequest},
    frames::ValueParser,
    literals::{ContextDiagnosticRequest, LiteralParser},
    recovery::{ParseFailure, Recovery},
    semantics::{SemanticParser, SubtreeValidator},
};
use crate::{
    ConfigLimits,
    diagnostics::{
        BudgetDiagnostic, ConfigParseErrors, DetailedDiagnosticRequest, DiagnosticFactory,
    },
    instance::LocationIndex,
    parser::context::ParserControl,
    session::ConfigSession,
};
use opeo::{ErrSlot, OResult, Out};
use typol_core::{
    BudgetError, CapacityExceeded, ChargedString, ConfigValue, Diagnostic, DiagnosticCode,
    DiagnosticContext, Read, ResourceLimit, SourceId, diagnostic_context,
};
use typol_i18n::{Translator, t};
use typol_semantic::{CompiledSchema, TypeKind, TypeView};
use typol_syntax::TokenKind;

static PARSER_ROOT_TYPE_INVARIANT: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "parser-initialization",
    "compiled-entry-type-is-not-structure",
);
static PARSER_ROOT_SCHEMA_ACCESS: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "parser-initialization",
    "compiled-entry-type-is-unavailable",
);
static PARSER_SETUP_RESULT_MISSING: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "parser-initialization",
    "parser-setup-result-missing",
);

pub(super) struct Parser<'schema, 'source, 'limits> {
    context: ParserContextStorage<'schema, 'source, 'limits>,
    frame_errors: ErrSlot<ParseFailure>,
    item_errors: ErrSlot<ParseFailure>,
    root_type: TypeView<'schema>,
    source_name: Option<ChargedString>,
}

impl<'schema, 'source, 'limits> Parser<'schema, 'source, 'limits> {
    pub(super) fn new<'slot, 'borrow>(
        schema: &'schema CompiledSchema,
        source: &'source mut dyn Read<Diagnostic>,
        limits: &'limits ConfigLimits,
        session: ConfigSession<'limits>,
        out: Out<'slot, 'borrow, ConfigParseErrors>,
    ) -> OResult<'slot, Self> {
        let translator = session.translator();
        let mut initialization_errors = ErrSlot::new();
        match initialization_errors
            .try_call(|out| Self::initialize(schema, source, limits, session, out))
        {
            Ok(parser) => Ok(parser),
            Err(caught) => Err(out.fail(ConfigParseErrors::emergency(
                caught.take(),
                schema.next_source_id(),
                translator,
            ))),
        }
    }

    fn initialize<'slot>(
        schema: &'schema CompiledSchema,
        source: &'source mut dyn Read<Diagnostic>,
        limits: &'limits ConfigLimits,
        mut session: ConfigSession<'limits>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Self> {
        let translator = session.translator();
        let Some(source_id) = schema.next_source_id() else {
            return Err(out.fail(DiagnosticFactory::make_diagnostic(
                DetailedDiagnosticRequest {
                    code: CapacityExceeded::SourceInput.diagnostic_code(),
                    message: t! {
                        name: "diagnostic.config.source_id_unrepresentable",
                        locale: translator,
                    }
                    .into(),
                    primary: None,
                    path: None,
                    value_source: None,
                    context: None,
                    translator,
                },
            )));
        };
        let root_type = schema.entry();
        let root_info = root_type;
        let root_kind = match session
            .validation()
            .schema_access_errors
            .try_call(|access_out| root_info.kind(access_out))
        {
            Ok(kind) => kind,
            Err(caught) => {
                return Err(out.fail(DiagnosticFactory::parser_internal(
                    PARSER_ROOT_SCHEMA_ACCESS.with_object_id(caught.take().index() as u64),
                    None,
                    None,
                    None,
                    translator,
                )));
            }
        };
        if !matches!(root_kind, TypeKind::Struct { .. }) {
            return Err(out.fail(DiagnosticFactory::parser_internal(
                PARSER_ROOT_TYPE_INVARIANT,
                None,
                None,
                None,
                translator,
            )));
        }

        let context = ParserControl::create(
            ParserControlRequest {
                schema,
                source,
                source_id,
                limits,
                session,
            },
            out.reborrow(),
        )?;
        Ok(Self {
            context,
            frame_errors: ErrSlot::new(),
            item_errors: ErrSlot::new(),
            root_type,
            source_name: None,
        })
    }

    pub(super) fn set_source_name<'slot, 'borrow>(
        &mut self,
        name: &str,
        out: Out<'slot, 'borrow, ConfigParseErrors>,
    ) -> OResult<'slot, ()> {
        if name.len() > self.context.config_limits.max_display_name_bytes() {
            return Err(out.fail(ConfigParseErrors::emergency(
                BudgetDiagnostic::from_error(
                    BudgetError::ResourceLimit(ResourceLimit::StringBytes),
                    self.context.session.translator(),
                ),
                Some(self.context.source_id),
                self.context.session.translator(),
            )));
        }
        let source_id = self.context.source_id;
        let mut budget_errors = ErrSlot::<BudgetError>::new();
        let budget = match budget_errors
            .try_call(|budget_out| self.context.budget.try_clone(budget_out))
        {
            Ok(budget) => budget,
            Err(caught) => {
                return Err(out.fail(ConfigParseErrors::emergency(
                    BudgetDiagnostic::from_error(caught.take(), self.context.session.translator()),
                    Some(source_id),
                    self.context.session.translator(),
                )));
            }
        };
        let translator = self.context.session.translator();
        let copy = match self
            .context
            .session
            .validation()
            .budget_errors
            .try_call(|budget_out| ChargedString::copy(name, Some(&budget), budget_out))
        {
            Ok(copy) => copy,
            Err(caught) => {
                return Err(out.fail(ConfigParseErrors::emergency(
                    BudgetDiagnostic::from_error(caught.take(), translator),
                    Some(source_id),
                    translator,
                )));
            }
        };
        self.source_name = Some(copy);
        Ok(())
    }

    pub(super) fn parse<'slot, 'borrow>(
        self,
        out: Out<'slot, 'borrow, ConfigParseErrors>,
    ) -> OResult<'slot, ConfigValue> {
        self.parse_with_locations(out)
            .map(|(value, _locations)| value)
    }

    pub(super) fn parse_with_locations<'slot, 'borrow>(
        mut self,
        out: Out<'slot, 'borrow, ConfigParseErrors>,
    ) -> OResult<'slot, (ConfigValue, LocationIndex<'schema>)> {
        let mut parse_errors = ErrSlot::new();
        let result = parse_errors.try_call(|parse_out| self.parse_value(parse_out));
        let mut context = self.context.into_inner();
        context.session.begin_secret(context.cursor.consumed_end());
        match result {
            Ok(value) if context.diagnostics.is_empty() => Ok((value, context.locations)),
            Ok(_) => Err(out.fail(ConfigParseErrors::from_buffer(
                context.diagnostics,
                context.schema,
                context.source_id,
                self.source_name,
            ))),
            Err(caught) => {
                Recovery::record_failure(&mut context, caught.take());
                Err(out.fail(ConfigParseErrors::from_buffer(
                    context.diagnostics,
                    context.schema,
                    context.source_id,
                    self.source_name,
                )))
            }
        }
    }

    pub(super) fn parse_slot<'slot, 'borrow>(
        parser: Option<Self>,
        source_id: SourceId,
        translator: Translator,
        out: Out<'slot, 'borrow, ConfigParseErrors>,
    ) -> OResult<'slot, ConfigValue> {
        let Some(parser) = parser else {
            return Err(out.fail(ConfigParseErrors::emergency(
                DiagnosticFactory::parser_internal(
                    PARSER_SETUP_RESULT_MISSING,
                    None,
                    None,
                    None,
                    translator,
                ),
                Some(source_id),
                translator,
            )));
        };
        parser.parse(out)
    }

    pub(super) fn parse_slot_with_locations<'slot, 'borrow>(
        parser: Option<Self>,
        source_id: SourceId,
        translator: Translator,
        out: Out<'slot, 'borrow, ConfigParseErrors>,
    ) -> OResult<'slot, (ConfigValue, LocationIndex<'schema>)> {
        let Some(parser) = parser else {
            return Err(out.fail(ConfigParseErrors::emergency(
                DiagnosticFactory::parser_internal(
                    PARSER_SETUP_RESULT_MISSING,
                    None,
                    None,
                    None,
                    translator,
                ),
                Some(source_id),
                translator,
            )));
        };
        parser.parse_with_locations(out)
    }

    fn parse_value<'slot>(
        &mut self,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, ConfigValue> {
        let parsed = ValueParser::parse(
            &mut self.context,
            self.root_type,
            &mut self.frame_errors,
            &mut self.item_errors,
            out.reborrow(),
        )?;
        let mut peek_errors = ErrSlot::new();
        let token = match peek_errors.try_call(|peek_out| self.context.cursor.peek(peek_out)) {
            Ok(token) => token,
            Err(caught) => {
                let failure = caught.take();
                if failure.is_input_failure() && self.context.diagnostics.is_empty() {
                    let normalized = if self.context.needs_normalization {
                        let mut normalization_errors = ErrSlot::new();
                        match normalization_errors.try_call(|normalization_out| {
                            SubtreeValidator::normalize_complete_root(
                                &mut self.context,
                                self.root_type,
                                parsed.span,
                                &parsed.value,
                                normalization_out,
                            )
                        }) {
                            Ok(value) => Some(value),
                            Err(caught) => {
                                if let Some(diagnostic) = caught.take().into_diagnostic() {
                                    self.context.diagnostics.push(diagnostic);
                                }
                                None
                            }
                        }
                    } else {
                        None
                    };
                    let value = normalized.as_ref().unwrap_or(&parsed.value);
                    if !self.context.needs_normalization || normalized.is_some() {
                        let mut validation_errors = ErrSlot::new();
                        if let Err(caught) = validation_errors.try_call(|validation_out| {
                            SubtreeValidator::validate_complete_root(
                                &mut self.context,
                                self.root_type,
                                parsed.span,
                                value,
                                validation_out,
                            )
                        }) {
                            let validation_failure = caught.take();
                            if !validation_failure.is_reported()
                                && let Some(diagnostic) = validation_failure.into_diagnostic()
                            {
                                self.context.diagnostics.push(diagnostic);
                            }
                        }
                        self.context
                            .session
                            .pin_diagnostics(&self.context.diagnostics);
                    }
                }
                return Err(out.fail(failure));
            }
        };
        let (kind, span) = (token.kind(), token.span());
        if kind != TokenKind::Eof {
            let diagnostic = ParserControl::diagnostic_call(
                &self.context.diagnostic_errors,
                |diagnostic_out| {
                    LiteralParser::context_diagnostic(
                        ContextDiagnosticRequest {
                            context: &self.context,
                            code: DiagnosticCode::CONFIG_SYNTAX_ERROR,
                            message: t! {
                                name: "diagnostic.config.trailing_content",
                                locale: self.context.session.translator(),
                            }
                            .into(),
                            span,
                            value_source: None,
                        },
                        diagnostic_out,
                    )
                },
                out.reborrow(),
            )?;
            return Err(out.fail(diagnostic.into()));
        }
        let value = if self.context.diagnostics.is_empty() {
            let value = if self.context.needs_normalization {
                SubtreeValidator::normalize_complete_root(
                    &mut self.context,
                    self.root_type,
                    parsed.span,
                    &parsed.value,
                    out.reborrow(),
                )?
            } else {
                parsed.value
            };
            let validation = SubtreeValidator::validate_complete_root(
                &mut self.context,
                self.root_type,
                parsed.span,
                &value,
                out.reborrow(),
            );
            self.context
                .session
                .pin_diagnostics(&self.context.diagnostics);
            validation?;
            value
        } else {
            parsed.value
        };
        if self.context.diagnostics.is_empty() {
            SemanticParser::check_assertions(&mut self.context, &value, out.reborrow())?;
        }
        Ok(value)
    }
}
