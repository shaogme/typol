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
    context::{ParserContext, ParserControlRequest},
    frames::ValueParser,
    literals::{ContextDiagnosticRequest, LiteralParser},
    recovery::{ParseFailure, Recovery},
    semantics::SemanticParser,
};
use crate::{
    ConfigLimits,
    diagnostics::{BudgetDiagnostic, ConfigParseErrors, DiagnosticFactory},
    instance::LocationIndex,
    parser::context::ParserControl,
    session::ConfigSession,
};
use opeo::{ErrSlot, OResult, Out};
use typol_core::{
    BudgetError, ChargedString, ConfigValue, Diagnostic, DiagnosticCode, DiagnosticContext, Read,
    SourceId, diagnostic_context,
};
use typol_i18n::diagnostic_key;
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
    context: ParserContext<'schema, 'source, 'limits>,
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
        let mut initialization_errors = ErrSlot::new();
        match initialization_errors
            .try_call(|out| Self::initialize(schema, source, limits, session, out))
        {
            Ok(parser) => Ok(parser),
            Err(caught) => Err(out.fail(ConfigParseErrors::emergency(
                caught.take(),
                schema.next_source_id(),
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
        let Some(source_id) = schema.next_source_id() else {
            return Err(out.fail(DiagnosticFactory::make_diagnostic(
                DiagnosticCode::RESOURCE_LIMIT_EXCEEDED,
                "configuration source identifier exceeds the representable range",
                None,
                None,
                None,
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
                return Err(out.fail(DiagnosticFactory::internal(
                    PARSER_ROOT_SCHEMA_ACCESS.with_object_id(caught.take().index() as u64),
                    None,
                    None,
                    None,
                )));
            }
        };
        if !matches!(root_kind, TypeKind::Struct { .. }) {
            return Err(out.fail(DiagnosticFactory::internal(
                PARSER_ROOT_TYPE_INVARIANT,
                None,
                None,
                None,
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
                BudgetDiagnostic::from_error(BudgetError::Resource),
                Some(self.context.source_id),
            )));
        }
        let budget = &self.context.budget;
        let copy = match self
            .context
            .session
            .validation()
            .budget_errors
            .try_call(|budget_out| ChargedString::copy(name, Some(budget), budget_out))
        {
            Ok(copy) => copy,
            Err(caught) => {
                return Err(out.fail(ConfigParseErrors::emergency(
                    BudgetDiagnostic::from_error(caught.take()),
                    Some(self.context.source_id),
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
        self.context
            .session
            .begin_secret(self.context.cursor.consumed_end());
        match result {
            Ok(value) if self.context.diagnostics.is_empty() => Ok((value, self.context.locations)),
            Ok(_) => Err(out.fail(ConfigParseErrors::from_buffer(
                self.context.diagnostics,
                self.context.schema,
                self.context.source_id,
                self.source_name,
            ))),
            Err(caught) => {
                Recovery::record_failure(&mut self.context, caught.take());
                Err(out.fail(ConfigParseErrors::from_buffer(
                    self.context.diagnostics,
                    self.context.schema,
                    self.context.source_id,
                    self.source_name,
                )))
            }
        }
    }

    pub(super) fn parse_slot<'slot, 'borrow>(
        parser: Option<Self>,
        source_id: SourceId,
        out: Out<'slot, 'borrow, ConfigParseErrors>,
    ) -> OResult<'slot, ConfigValue> {
        let Some(parser) = parser else {
            return Err(out.fail(ConfigParseErrors::emergency(
                DiagnosticFactory::internal(PARSER_SETUP_RESULT_MISSING, None, None, None),
                Some(source_id),
            )));
        };
        parser.parse(out)
    }

    pub(super) fn parse_slot_with_locations<'slot, 'borrow>(
        parser: Option<Self>,
        source_id: SourceId,
        out: Out<'slot, 'borrow, ConfigParseErrors>,
    ) -> OResult<'slot, (ConfigValue, LocationIndex<'schema>)> {
        let Some(parser) = parser else {
            return Err(out.fail(ConfigParseErrors::emergency(
                DiagnosticFactory::internal(PARSER_SETUP_RESULT_MISSING, None, None, None),
                Some(source_id),
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
        let (kind, span) = {
            let token = self.context.cursor.peek(out.reborrow())?;
            (token.kind(), token.span())
        };
        if kind != TokenKind::Eof {
            let diagnostic = ParserControl::diagnostic_call(
                &self.context.diagnostic_errors,
                |diagnostic_out| {
                    LiteralParser::context_diagnostic(
                        ContextDiagnosticRequest {
                            context: &self.context,
                            code: DiagnosticCode::CONFIG_SYNTAX_ERROR,
                            message: "unexpected trailing content after the root object",
                            key: Some(diagnostic_key!("diagnostic.config.trailing_content")),
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
        if self.context.diagnostics.is_empty() {
            SemanticParser::check_assertions(&mut self.context, &parsed.value, out.reborrow())?;
        }
        Ok(parsed.value)
    }
}
