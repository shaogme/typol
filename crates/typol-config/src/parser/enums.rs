use crate::{
    context::CompletionState,
    diagnostics::{BudgetDiagnostic, DetailedDiagnosticRequest, DiagnosticFactory},
    parser::{
        context::{ParserContext, ParserControl, ValueCallContext},
        literals::{
            ContextDiagnosticRequest, ExpectedSymbol, ExpectedValueSymbol, LiteralParser,
            ParsedValue, Syntax,
        },
        recovery::ParseFailure,
    },
    value::{EnumPayload, FieldValue, ValueFactory},
};
use opeo::{ErrSlot, OResult, Out};
use typol_core::{
    BudgetError, ChargedVec, Diagnostic, DiagnosticCode, DiagnosticContext, LocatedSpan,
    SecretRange, ValueError, ValueErrorKind, ValueSource, diagnostic_context,
};
use typol_i18n::{Translator, t};
use typol_semantic::{SchemaAccessError, TypeView, VariantPayloadInfo, VariantView};
use typol_syntax::{Symbol, TokenKind};

static PARSER_ENUM_PAYLOAD_INVARIANT: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "parser-enum-payload",
    "compiled-payload-shape-invalid",
);
static PARSER_ENUM_FIELDS_DUPLICATE: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "parser-enum-payload",
    "compiled-variant-fields-duplicate",
);

struct EnumVariantToken {
    kind: TokenKind,
    span: LocatedSpan,
}

pub(super) struct EnumFrame<'schema> {
    variant: VariantView<'schema>,
    initial_span: LocatedSpan,
    variant_span: LocatedSpan,
    secret_range: Option<SecretRange>,
}

impl<'schema> EnumFrame<'schema> {
    pub(super) fn new<'slot>(
        context: &mut ParserContext<'schema, '_, '_>,
        expected: TypeView<'schema>,
        initial_span: LocatedSpan,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, Self> {
        Syntax::expect_value_symbol(
            context,
            ExpectedValueSymbol {
                expected: Symbol::Dot,
                syntax_message: t! {
                    name: "diagnostic.config.expected_enum_variant_prefix",
                    locale: context.session.translator(),
                }
                .into(),
                value_message: t! {
                    name: "diagnostic.config.expected_enum_variant_prefix",
                    locale: context.session.translator(),
                }
                .into(),
            },
            out.reborrow(),
        )?;
        let token = {
            let token = context.cursor.peek(out.reborrow())?;
            EnumVariantToken {
                kind: token.kind(),
                span: token.span(),
            }
        };
        if token.kind != TokenKind::Identifier {
            let diagnostic = context_diagnostic(
                ContextDiagnosticRequest {
                    context,
                    code: DiagnosticCode::CONFIG_SYNTAX_ERROR,
                    message: t! {
                        name: "diagnostic.config.enum_variant_name_expected",
                        locale: context.session.translator(),
                    }
                    .into(),
                    span: token.span,
                    value_source: None,
                },
                out.reborrow(),
            )?;
            return Err(out.fail(diagnostic.into()));
        }

        let (too_long, variant_name) = {
            let token_text = context.cursor.peek(out.reborrow())?;
            let variant_name = token_text.text().unwrap_or("");
            let too_long = variant_name.len() > context.limits.max_string_bytes();
            (too_long, variant_name)
        };
        if too_long {
            return Err(out.fail(
                DiagnosticFactory::make_diagnostic(DetailedDiagnosticRequest {
                    code: DiagnosticCode::RESOURCE_LIMIT_EXCEEDED,
                    message: t! {
                        name: "diagnostic.config.enum_variant_name_byte_limit",
                        locale: context.session.translator(),
                    }
                    .into(),
                    primary: Some(token.span),
                    path: None,
                    value_source: None,
                    context: None,
                    translator: context.session.translator(),
                })
                .into(),
            ));
        }

        let budget = &context.budget;
        let selected = budget_call(
            &mut context.session.validation().budget_errors,
            |budget_out| expected.variant_named_budgeted(variant_name, budget, budget_out),
            out.reborrow(),
        )?;
        let Some(variant_id) = selected else {
            let diagnostic = context_diagnostic(
                ContextDiagnosticRequest {
                    context,
                    code: DiagnosticCode::CONFIG_VALUE_ERROR,
                    message: t! {
                        name: "diagnostic.config.enum_variant_undeclared",
                        locale: context.session.translator(),
                    }
                    .into(),
                    span: token.span,
                    value_source: None,
                },
                out.reborrow(),
            )?;
            return Err(out.fail(diagnostic.into()));
        };
        let secret = schema_call(
            &mut context.session.validation().schema_access_errors,
            |schema_out| variant_id.is_secret(schema_out),
            out.reborrow(),
        )?;
        let secret_range = if secret {
            context.session.begin_secret(initial_span.span().start())
        } else {
            None
        };
        context.cursor.bump(out.reborrow())?;
        if let Some(node) = context.current_node {
            ParserControl::diagnostic_call(
                &context.diagnostic_errors,
                |diagnostic_out| {
                    context
                        .locations
                        .set_variant(node, variant_id, token.span, diagnostic_out)
                },
                out.reborrow(),
            )?;
        }

        Ok(Self {
            variant: variant_id,
            initial_span,
            variant_span: token.span,
            secret_range,
        })
    }

    pub(super) fn variant(&self) -> VariantView<'schema> {
        self.variant
    }

    pub(super) fn enter_payload<'slot>(
        &self,
        context: &mut ParserContext<'schema, '_, '_>,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, ()> {
        let payload = schema_call(
            &mut context.session.validation().schema_access_errors,
            |schema_out| self.variant.payload(schema_out),
            out.reborrow(),
        )?;
        match payload {
            VariantPayloadInfo::Single { .. } => {
                Syntax::expect_value_symbol(
                    context,
                    ExpectedValueSymbol {
                        expected: Symbol::LeftParen,
                        syntax_message: t! {
                            name: "diagnostic.config.expected_enum_payload",
                            locale: context.session.translator(),
                        }
                        .into(),
                        value_message: t! {
                            name: "diagnostic.config.expected_enum_payload",
                            locale: context.session.translator(),
                        }
                        .into(),
                    },
                    out.reborrow(),
                )?;
                let name = schema_call(
                    &mut context.session.validation().schema_access_errors,
                    |schema_out| self.variant.name(schema_out),
                    out.reborrow(),
                )?;
                push_variant_path(context, name, self.variant_span, out.reborrow())?;
                context.target_field = None;
                context.target_variant = Some(self.variant);
            }
            VariantPayloadInfo::Structure { .. } => {
                let token = context.cursor.peek(out.reborrow())?;
                let span = token.span();
                if token.kind() != TokenKind::Symbol(Symbol::LeftBrace) {
                    let diagnostic = context_diagnostic(
                        ContextDiagnosticRequest {
                            context,
                            code: DiagnosticCode::CONFIG_VALUE_ERROR,
                            message: t! {
                                name: "diagnostic.config.enum_payload_shape_mismatch",
                                locale: context.session.translator(),
                            }
                            .into(),
                            span,
                            value_source: Some(context.value_source),
                        },
                        out.reborrow(),
                    )?;
                    return Err(out.fail(diagnostic.into()));
                }
                ParserControl::check_depth(
                    context,
                    context.nesting_depth,
                    span,
                    Some(context.value_source),
                    out.reborrow(),
                )?;
                ParserControl::enter_nesting(
                    context,
                    span,
                    Some(context.value_source),
                    out.reborrow(),
                )?;
                let name = schema_call(
                    &mut context.session.validation().schema_access_errors,
                    |schema_out| self.variant.name(schema_out),
                    out.reborrow(),
                )?;
                push_variant_path(context, name, self.variant_span, out.reborrow())?;
            }
            VariantPayloadInfo::Unit => {}
        }
        Ok(())
    }

    pub(super) fn finish_unit<'slot>(
        self,
        context: &mut ParserContext<'schema, '_, '_>,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, ParsedValue> {
        let token = context.cursor.peek(out.reborrow())?;
        let span = token.span();
        if matches!(
            token.kind(),
            TokenKind::Symbol(Symbol::LeftParen | Symbol::LeftBrace)
        ) {
            let diagnostic = context_diagnostic(
                ContextDiagnosticRequest {
                    context,
                    code: DiagnosticCode::CONFIG_VALUE_ERROR,
                    message: t! {
                        name: "diagnostic.config.enum_payload_shape_mismatch",
                        locale: context.session.translator(),
                    }
                    .into(),
                    span,
                    value_source: Some(context.value_source),
                },
                out.reborrow(),
            )?;
            return Err(out.fail(diagnostic.into()));
        }
        let end = self.variant_span;
        self.finish(context, EnumPayload::unit(), end, out.reborrow())
    }

    pub(super) fn finish_single<'slot>(
        self,
        context: &mut ParserContext<'schema, '_, '_>,
        parsed: ParsedValue,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, ParsedValue> {
        let token = context.cursor.peek(out.reborrow())?;
        let span = token.span();
        if token.kind() == TokenKind::Symbol(Symbol::Comma) {
            let diagnostic = context_diagnostic(
                ContextDiagnosticRequest {
                    context,
                    code: DiagnosticCode::CONFIG_VALUE_ERROR,
                    message: t! {
                        name: "diagnostic.config.enum_payload_shape_mismatch",
                        locale: context.session.translator(),
                    }
                    .into(),
                    span,
                    value_source: Some(context.value_source),
                },
                out.reborrow(),
            )?;
            return Err(out.fail(diagnostic.into()));
        }
        let close = Syntax::expect_symbol(
            context,
            ExpectedSymbol {
                expected: Symbol::RightParen,
                code: DiagnosticCode::CONFIG_SYNTAX_ERROR,
                message: t! {
                    name: "diagnostic.config.expected_enum_payload_close_parenthesis",
                    locale: context.session.translator(),
                }
                .into(),
            },
            out.reborrow(),
        )?;
        let payload = EnumPayload::single(parsed.value, Some(&context.budget), out.reborrow())?;
        self.finish(context, payload, close, out.reborrow())
    }

    pub(super) fn finish_structure<'slot>(
        self,
        context: &mut ParserContext<'schema, '_, '_>,
        fields: ChargedVec<FieldValue>,
        close: LocatedSpan,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, ParsedValue> {
        let payload = EnumPayload::structure(fields);
        self.finish(context, payload, close, out.reborrow())
    }

    fn finish<'slot>(
        self,
        context: &mut ParserContext<'schema, '_, '_>,
        payload: EnumPayload,
        end: LocatedSpan,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, ParsedValue> {
        if let Some(index) = self.secret_range {
            context.session.end_secret(index, end.span().end());
        }
        let name = schema_call(
            &mut context.session.validation().schema_access_errors,
            |schema_out| self.variant.name(schema_out),
            out.reborrow(),
        )?;
        ParserControl::construct_text(context, name.len(), out.reborrow())?;
        let value = value_call(
            &mut context.value_errors,
            |value_out| {
                ValueFactory::enumeration(
                    name,
                    payload,
                    context.value_source,
                    Some(&context.budget),
                    value_out,
                )
            },
            ValueCallContext {
                span: end,
                source: context.value_source,
                translator: context.session.translator(),
            },
            out.reborrow(),
        )?;
        Ok(ParsedValue {
            completion: CompletionState::ChildrenComplete,
            value,
            span: ParserControl::diagnostic_call(
                &context.diagnostic_errors,
                |diagnostic_out| {
                    DiagnosticFactory::span_through(
                        self.initial_span,
                        end,
                        context.session.translator(),
                        diagnostic_out,
                    )
                },
                out.reborrow(),
            )?,
        })
    }
}

fn push_variant_path<'slot>(
    context: &mut ParserContext<'_, '_, '_>,
    name: &str,
    span: LocatedSpan,
    mut out: Out<'slot, '_, ParseFailure>,
) -> OResult<'slot, ()> {
    if name.len() > context.limits.max_string_bytes() {
        return Err(out.fail(
            DiagnosticFactory::make_diagnostic(DetailedDiagnosticRequest {
                code: DiagnosticCode::RESOURCE_LIMIT_EXCEEDED,
                message: t! {
                    name: "diagnostic.config.enum_variant_name_byte_limit",
                    locale: context.session.translator(),
                }
                .into(),
                primary: Some(span),
                path: None,
                value_source: None,
                context: None,
                translator: context.session.translator(),
            })
            .into(),
        ));
    }
    budget_call(
        &mut context.session.validation().budget_errors,
        |budget_out| context.path.push_payload(name, budget_out),
        out.reborrow(),
    )?;
    Ok(())
}

fn context_diagnostic<'slot, 'borrow>(
    request: ContextDiagnosticRequest<'_, '_, '_, '_>,
    out: Out<'slot, 'borrow, ParseFailure>,
) -> OResult<'slot, Diagnostic> {
    let context = request.context;
    ParserControl::diagnostic_call(
        &context.diagnostic_errors,
        |diagnostic_out| LiteralParser::context_diagnostic(request, diagnostic_out),
        out,
    )
}

fn schema_call<'slot, 'borrow, T>(
    errors: &mut ErrSlot<SchemaAccessError>,
    operation: impl for<'call> FnOnce(Out<'call, 'call, SchemaAccessError>) -> OResult<'call, T>,
    out: Out<'slot, 'borrow, ParseFailure>,
) -> OResult<'slot, T> {
    match errors.try_call(operation) {
        Ok(value) => Ok(value),
        Err(caught) => Err(out.fail(ParseFailure::from(caught.take()))),
    }
}

fn budget_call<'slot, 'borrow, T>(
    errors: &mut ErrSlot<BudgetError>,
    operation: impl for<'call> FnOnce(Out<'call, 'call, BudgetError>) -> OResult<'call, T>,
    out: Out<'slot, 'borrow, ParseFailure>,
) -> OResult<'slot, T> {
    match errors.try_call(operation) {
        Ok(value) => Ok(value),
        Err(caught) => Err(out.fail(ParseFailure::from(caught.take()))),
    }
}

fn value_call<'slot, 'borrow, T>(
    errors: &mut ErrSlot<ValueError>,
    operation: impl for<'call> FnOnce(Out<'call, 'call, ValueError>) -> OResult<'call, T>,
    context: ValueCallContext,
    out: Out<'slot, 'borrow, ParseFailure>,
) -> OResult<'slot, T> {
    let ValueCallContext {
        span,
        source,
        translator,
    } = context;
    match errors.try_call(operation) {
        Ok(value) => Ok(value),
        Err(caught) => Err(out.fail(value_error(caught.take(), span, source, translator).into())),
    }
}

fn value_error(
    error: ValueError,
    span: LocatedSpan,
    source: ValueSource,
    translator: Translator,
) -> Diagnostic {
    match error.kind() {
        ValueErrorKind::ResourceLimitExceeded => {
            DiagnosticFactory::make_diagnostic(DetailedDiagnosticRequest {
                code: DiagnosticCode::RESOURCE_LIMIT_EXCEEDED,
                message: t! {
                    name: "diagnostic.config.enum_payload_resource_limit",
                    locale: translator,
                }
                .into(),
                primary: Some(span),
                path: None,
                value_source: Some(source),
                context: None,
                translator,
            })
        }
        ValueErrorKind::AllocationFailed => {
            DiagnosticFactory::allocation_diagnostic(Some(span), None, Some(source), translator)
        }
        ValueErrorKind::CapacityExceeded => {
            BudgetDiagnostic::from_error(BudgetError::CapacityExceeded, translator)
        }
        ValueErrorKind::InternalInvariantFailure => DiagnosticFactory::internal(
            error.context().unwrap_or(PARSER_ENUM_PAYLOAD_INVARIANT),
            Some(span),
            None,
            Some(source),
            translator,
        ),
        ValueErrorKind::EnvironmentReferenceRequired => {
            DiagnosticFactory::make_diagnostic(DetailedDiagnosticRequest {
                code: DiagnosticCode::CONFIG_VALUE_ERROR,
                message: t! {
                    name: "diagnostic.config.environment_reference_metadata_required",
                    locale: translator,
                }
                .into(),
                primary: Some(span),
                path: None,
                value_source: Some(source),
                context: None,
                translator,
            })
        }
        ValueErrorKind::DuplicateFieldName | ValueErrorKind::DuplicateMapKey => {
            DiagnosticFactory::internal(
                error
                    .duplicate_indices()
                    .map_or(PARSER_ENUM_FIELDS_DUPLICATE, |(index, _)| {
                        PARSER_ENUM_FIELDS_DUPLICATE.with_object_id(index as u64)
                    }),
                Some(span),
                None,
                Some(source),
                translator,
            )
        }
    }
}
