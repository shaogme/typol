use super::{
    containers::ContainerSupport,
    context::ParserContext,
    literals::{ContextDiagnosticRequest, LiteralParser, ParsedValue, Syntax},
    recovery::ParseFailure,
};
use crate::{
    context::{
        AssignmentContext, AssignmentTarget, CompletionState, EnvironmentSpans, FallbackValue,
        SemanticStatus,
    },
    diagnostics::DiagnosticFactory,
    environment::{EnvironmentAuthorizationRequest, EnvironmentResolver},
    parser::context::ParserControl,
    session::{AssertionRequest, EnvironmentRequest},
    value::ValueFactory,
    value_normalization::{NormalizeRequest, ValueNormalizer},
    value_validation::{TreeContext, TreeTarget, ValueValidation},
};
use core::cell::RefCell;
use opeo::{ErrSlot, OResult, Out};
use typol_core::{
    ConfigValue, Diagnostic, DiagnosticCode, DiagnosticContext, LocatedSpan, ValueKind,
    ValueSource, diagnostic_context,
};
use typol_i18n::diagnostic_key;
use typol_semantic::{FieldView, SchemaAccessError, TypeKind, TypeView};
use typol_syntax::{BuiltinType, Symbol, TokenKind};

static PARSER_ENVIRONMENT_VALUE_SHAPE: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "environment-resolution",
    "environment-name-is-not-string",
);
static PARSER_DEFAULT_SCHEMA_ACCESS: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "value-normalization",
    "default-field-schema-view-unavailable",
);

fn enrich_diagnostic(
    context: &ParserContext<'_, '_, '_>,
    mut diagnostic: Diagnostic,
    span: LocatedSpan,
    source: ValueSource,
) -> ParseFailure {
    if diagnostic.primary().is_none() {
        diagnostic.set_located_primary(Some(span));
    }
    if diagnostic.value_source().is_none() {
        diagnostic.set_value_source(Some(source));
    }
    if diagnostic.path().is_none() {
        match context
            .diagnostic_errors
            .borrow_mut()
            .try_call(|diagnostic_out| {
                LiteralParser::copy_path_with_source(
                    context,
                    diagnostic.located_primary(),
                    diagnostic.value_source(),
                    diagnostic_out,
                )
            }) {
            Ok(path) => {
                DiagnosticFactory::bind_path(&mut diagnostic, Some(path));
            }
            Err(caught) => return caught.take().into(),
        }
    }
    diagnostic.into()
}

fn status_result<'slot, 'borrow>(
    status: SemanticStatus,
    out: Out<'slot, 'borrow, ParseFailure>,
) -> OResult<'slot, ()> {
    match status {
        SemanticStatus::Success => Ok(()),
        SemanticStatus::Reported => Err(out.fail(ParseFailure::Reported)),
        SemanticStatus::Stopped => Err(out.fail(ParseFailure::Stopped)),
    }
}

fn diagnostic_call<'slot, 'borrow, T>(
    errors: &RefCell<ErrSlot<Diagnostic>>,
    operation: impl for<'call> FnOnce(Out<'call, 'call, Diagnostic>) -> OResult<'call, T>,
    out: Out<'slot, 'borrow, ParseFailure>,
) -> OResult<'slot, T> {
    match errors.borrow_mut().try_call(operation) {
        Ok(value) => Ok(value),
        Err(caught) => Err(out.fail(caught.take().into())),
    }
}

fn schema_call<'slot, 'borrow, T>(
    errors: &mut ErrSlot<SchemaAccessError>,
    operation: impl for<'call> FnOnce(Out<'call, 'call, SchemaAccessError>) -> OResult<'call, T>,
    out: Out<'slot, 'borrow, ParseFailure>,
) -> OResult<'slot, T> {
    match errors.try_call(operation) {
        Ok(value) => Ok(value),
        Err(caught) => Err(out.fail(caught.take().into())),
    }
}

pub(crate) struct SemanticParser;

impl SemanticParser {
    pub(super) fn check_completed<'slot>(
        context: &mut ParserContext<'_, '_, '_>,
        target: AssignmentTarget<'_>,
        value: &ConfigValue,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, ()> {
        let AssignmentTarget {
            expected,
            field,
            variant,
            span,
        } = target;
        let before = context.diagnostics.len();
        let result = {
            let mut diagnostic_errors = context.diagnostic_errors.borrow_mut();
            let result = diagnostic_errors.try_call(|diagnostic_out| {
                context.session.completed(
                    AssignmentContext {
                        schema: context.schema,
                        node: context.current_node,
                        locations: &context.locations,
                        inherited_constraints: &context.inherited_constraints,
                        expected,
                        field,
                        variant,
                        span: Some(span),
                        path: &context.path,
                        source: Some(value.source()),
                        shape_validated: false,
                    },
                    value,
                    &mut context.diagnostics,
                    diagnostic_out,
                )
            });
            match result {
                Ok(status) => Ok(status),
                Err(caught) => Err(caught.take()),
            }
        };
        let status = match result {
            Ok(status) => status,
            Err(diagnostic) => {
                let failure = enrich_diagnostic(context, diagnostic, span, value.source());
                let ParseFailure::Diagnostic(diagnostic) = failure else {
                    return Err(out.fail(failure));
                };
                if context.diagnostics.push(diagnostic) {
                    SemanticStatus::Reported
                } else {
                    SemanticStatus::Stopped
                }
            }
        };
        if context.diagnostics.stopped() {
            return Err(out.fail(ParseFailure::Stopped));
        }
        if context.diagnostics.len() > before {
            return Err(out.fail(ParseFailure::Reported));
        }
        status_result(status, out.reborrow())
    }

    pub(super) fn check_assertions<'slot>(
        context: &mut ParserContext<'_, '_, '_>,
        root: &ConfigValue,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, ()> {
        let result = {
            let mut diagnostic_errors = context.diagnostic_errors.borrow_mut();
            let result = diagnostic_errors.try_call(|diagnostic_out| {
                context.session.assertions(
                    AssertionRequest {
                        schema: context.schema,
                        root,
                        locations: &context.locations,
                        diagnostics: &mut context.diagnostics,
                    },
                    diagnostic_out,
                )
            });
            match result {
                Ok(status) => Ok(status),
                Err(caught) => Err(caught.take()),
            }
        };
        let result = match result {
            Ok(result) => result,
            Err(diagnostic) => return Err(out.fail(diagnostic.into())),
        };
        if context.diagnostics.stopped() {
            return Err(out.fail(ParseFailure::Stopped));
        }
        status_result(result, out.reborrow())
    }

    pub(super) fn parse_environment<'schema, 'slot>(
        context: &mut ParserContext<'schema, '_, '_>,
        expected: TypeView<'schema>,
        start: LocatedSpan,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, ParsedValue> {
        Self::parse_environment_inner(context, expected, start, out.reborrow())
    }

    fn parse_environment_inner<'schema, 'slot>(
        context: &mut ParserContext<'schema, '_, '_>,
        expected: TypeView<'schema>,
        start: LocatedSpan,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, ParsedValue> {
        let Some(field_id) = context.target_field else {
            let diagnostic = diagnostic_call(
                &context.diagnostic_errors,
                |diagnostic_out| {
                    LiteralParser::context_diagnostic(
                        ContextDiagnosticRequest {
                            context,
                            code: DiagnosticCode::CONFIG_VALUE_ERROR,
                            message: "environment expressions are only allowed for authorized scalar fields",
                            key: Some(diagnostic_key!("diagnostic.environment.expression_invalid")),
                            span: start,
                            value_source: None,
                        },
                        diagnostic_out,
                    )
                },
                out.reborrow(),
            )?;
            return Err(out.fail(diagnostic.into()));
        };
        let field = field_id;
        let target = expected;
        let kind = schema_call(
            &mut context.session.validation().schema_access_errors,
            |schema_out| target.kind(schema_out),
            out.reborrow(),
        )?;
        let optional = matches!(&kind, TypeKind::Option { .. });
        let scalar = match kind {
            TypeKind::Builtin(builtin) => Some(builtin),
            TypeKind::Option { inner } => {
                let inner_kind = schema_call(
                    &mut context.session.validation().schema_access_errors,
                    |schema_out| inner.kind(schema_out),
                    out.reborrow(),
                )?;
                if let TypeKind::Builtin(builtin) = inner_kind {
                    Some(builtin)
                } else {
                    None
                }
            }
            _ => None,
        };
        let authorization = schema_call(
            &mut context.session.validation().schema_access_errors,
            |schema_out| field.environment(schema_out),
            out.reborrow(),
        )?;
        let Some(builtin) = scalar.filter(|_| authorization.is_some()) else {
            let diagnostic = diagnostic_call(
                &context.diagnostic_errors,
                |diagnostic_out| {
                    LiteralParser::context_diagnostic(
                        ContextDiagnosticRequest {
                            context,
                            code: DiagnosticCode::CONFIG_VALUE_ERROR,
                            message: "environment expressions are only allowed for authorized scalar fields",
                            key: Some(diagnostic_key!("diagnostic.environment.expression_invalid")),
                            span: start,
                            value_source: None,
                        },
                        diagnostic_out,
                    )
                },
                out.reborrow(),
            )?;
            return Err(out.fail(diagnostic.into()));
        };
        context.cursor.bump(out.reborrow())?;
        Syntax::expect_symbol(
            context,
            Symbol::LeftParen,
            DiagnosticCode::CONFIG_SYNTAX_ERROR,
            "expected an opening parenthesis in the environment expression",
            out.reborrow(),
        )?;
        let name = LiteralParser::parse_scalar(context, BuiltinType::String, out.reborrow())?;
        let mut spans = EnvironmentSpans {
            call: start,
            name: name.span,
        };
        let ValueKind::String(name) = name.value.as_ref() else {
            return Err(out.fail(
                DiagnosticFactory::internal(
                    PARSER_ENVIRONMENT_VALUE_SHAPE,
                    Some(name.span),
                    None,
                    Some(ValueSource::EnvironmentVariable),
                )
                .into(),
            ));
        };
        let authorization = schema_call(
            &mut context.session.validation().schema_access_errors,
            |schema_out| field.environment(schema_out),
            out.reborrow(),
        )?;
        let authorization_request = EnvironmentAuthorizationRequest {
            name,
            authorization,
            span: spans.name,
            budget: &context.budget,
        };
        let authorization_result = {
            let mut diagnostic_errors = context.diagnostic_errors.borrow_mut();
            diagnostic_errors
                .try_call(|diagnostic_out| {
                    EnvironmentResolver::authorize(
                        authorization_request,
                        &mut context.session.validation().budget_errors,
                        diagnostic_out,
                    )
                })
                .map_err(|caught| caught.take())
        };
        let authorization = match authorization_result {
            Ok(authorization) => authorization,
            Err(diagnostic) => {
                return Err(out.fail(enrich_diagnostic(
                    context,
                    diagnostic,
                    spans.name,
                    ValueSource::EnvironmentVariable,
                )));
            }
        };
        let token = context.cursor.peek(out.reborrow())?;
        let fallback = if token.kind() == TokenKind::Symbol(Symbol::Comma) {
            context.cursor.bump(out.reborrow())?;
            let span = context.cursor.peek(out.reborrow())?.span();
            if optional {
                ParserControl::enter_nesting(
                    context,
                    span,
                    Some(ValueSource::ExplicitFallback),
                    out.reborrow(),
                )?;
            }
            let count_result = ParserControl::count_value_node(
                context,
                span,
                Some(ValueSource::ExplicitFallback),
                out.reborrow(),
            );
            if optional {
                ParserControl::leave_nesting(context);
            }
            count_result?;
            let previous = context.value_source;
            context.value_source = ValueSource::ExplicitFallback;
            let parsed = LiteralParser::parse_scalar(context, builtin, out.reborrow());
            context.value_source = previous;
            Some(parsed?)
        } else {
            None
        };
        let close = Syntax::expect_symbol(
            context,
            Symbol::RightParen,
            DiagnosticCode::CONFIG_SYNTAX_ERROR,
            "expected a closing parenthesis in the environment expression",
            out.reborrow(),
        )?;
        spans.call = diagnostic_call(
            &context.diagnostic_errors,
            |diagnostic_out| DiagnosticFactory::span_through(start, close, diagnostic_out),
            out.reborrow(),
        )?;
        let fallback = if let Some(parsed) = fallback {
            let value_span = parsed.span;
            let value = parsed.value;
            let target_kind = schema_call(
                &mut context.session.validation().schema_access_errors,
                |schema_out| target.kind(schema_out),
                out.reborrow(),
            )?;
            let value = if matches!(target_kind, TypeKind::Option { .. }) {
                ParserControl::count_value_node(
                    context,
                    value_span,
                    Some(ValueSource::ExplicitFallback),
                    out.reborrow(),
                )?;
                let budget = &context.budget;
                ContainerSupport::value_call(
                    &mut context.value_errors,
                    move |value_out| {
                        ValueFactory::optional_owned(
                            value,
                            ValueSource::ExplicitFallback,
                            Some(budget),
                            value_out,
                        )
                    },
                    value_span,
                    ValueSource::ExplicitFallback,
                    out.reborrow(),
                )?
            } else {
                value
            };
            let proof_result = {
                let mut diagnostic_errors = context.diagnostic_errors.borrow_mut();
                match diagnostic_errors.try_call(|diagnostic_out| {
                    context.session.validate_fallback(
                        AssignmentContext {
                            schema: context.schema,
                            node: None,
                            locations: &context.locations,
                            inherited_constraints: &context.inherited_constraints,
                            expected,
                            field: Some(field_id),
                            variant: None,
                            span: Some(value_span),
                            path: &context.path,
                            source: Some(ValueSource::ExplicitFallback),
                            shape_validated: false,
                        },
                        &value,
                        &mut context.diagnostics,
                        diagnostic_out,
                    )
                }) {
                    Ok(proof) => Ok(proof),
                    Err(caught) => Err(caught.take()),
                }
            };
            let proof = match proof_result {
                Ok(Some(proof)) => proof,
                Ok(None) => {
                    return Err(out.fail(if context.diagnostics.stopped() {
                        ParseFailure::Stopped
                    } else {
                        ParseFailure::Reported
                    }));
                }
                Err(diagnostic) => return Err(out.fail(diagnostic.into())),
            };
            Some(FallbackValue {
                value,
                proof,
                span: value_span,
            })
        } else {
            None
        };
        let call_span = spans.call;
        let environment_request = EnvironmentRequest {
            metadata: AssignmentContext {
                schema: context.schema,
                node: None,
                locations: &context.locations,
                inherited_constraints: &context.inherited_constraints,
                expected,
                field: Some(field_id),
                variant: None,
                span: Some(spans.call),
                path: &context.path,
                source: Some(ValueSource::EnvironmentVariable),
                shape_validated: false,
            },
            authorization,
            fallback,
            spans,
        };
        let resolved_result = {
            let mut diagnostic_errors = context.diagnostic_errors.borrow_mut();
            diagnostic_errors
                .try_call(|diagnostic_out| {
                    context
                        .session
                        .environment(environment_request, diagnostic_out)
                })
                .map_err(|caught| caught.take())
        };
        let resolved = match resolved_result {
            Ok(value) => value,
            Err(diagnostic) => {
                return Err(out.fail(enrich_diagnostic(
                    context,
                    diagnostic,
                    call_span,
                    ValueSource::EnvironmentVariable,
                )));
            }
        };
        let Some(resolved) = resolved else {
            return Err(out.fail(ParseFailure::DeferredField));
        };
        let span = resolved.origin.span();
        let value = resolved.value;
        if !matches!(resolved.completion, CompletionState::Validated(_)) {
            let nested = match value.as_ref() {
                ValueKind::Optional(Some(inner)) => Some(inner),
                _ => None,
            };
            ParserControl::count_value_node(context, span, Some(value.source()), out.reborrow())?;
            if nested.is_some() {
                ParserControl::enter_nesting(context, span, Some(value.source()), out.reborrow())?;
                let count_result = ParserControl::count_value_node(
                    context,
                    span,
                    Some(value.source()),
                    out.reborrow(),
                );
                ParserControl::leave_nesting(context);
                count_result?;
            }
        }
        Ok(ParsedValue {
            value,
            span,
            completion: resolved.completion,
        })
    }
}

pub(super) struct SubtreeValidator;

impl SubtreeValidator {
    pub(super) fn copy_default<'slot>(
        context: &mut ParserContext<'_, '_, '_>,
        value: &ConfigValue,
        field: FieldView<'_>,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, ConfigValue> {
        let metadata =
            context
                .session
                .validation()
                .schema_access_errors
                .try_call(|mut schema_out| {
                    let ty = field.ty(schema_out.reborrow())?;
                    let span = field.span(schema_out.reborrow())?;
                    Ok((ty, span))
                });
        let (expected, primary) = match metadata {
            Ok(metadata) => metadata,
            Err(caught) => {
                return Err(out.fail(
                    DiagnosticFactory::internal(
                        PARSER_DEFAULT_SCHEMA_ACCESS.with_object_id(caught.take().index() as u64),
                        None,
                        None,
                        None,
                    )
                    .into(),
                ));
            }
        };
        let request = NormalizeRequest {
            expected,
            value: value.root(),
            depth: context.nesting_depth,
            primary: Some(primary.into()),
            text_limit: context.config_limits.max_constructed_text_bytes(),
            schema_default: true,
        };
        let value_nodes = &mut context.value_nodes;
        let limits = context.config_limits.values();
        let budget = &context.budget;
        let normalized = ParserControl::diagnostic_call(
            &context.diagnostic_errors,
            |diagnostic_out| {
                ValueNormalizer::copy(request, value_nodes, limits, budget, diagnostic_out)
            },
            out.reborrow(),
        )?;
        Ok(normalized.into_config_value())
    }

    pub(super) fn validate<'schema, 'slot, 'borrow>(
        context: &mut ParserContext<'schema, '_, '_>,
        target: AssignmentTarget<'schema>,
        value: &ConfigValue,
        completion: CompletionState,
        out: Out<'slot, 'borrow, ParseFailure>,
    ) -> OResult<'slot, ()> {
        let mut tree = TreeContext {
            schema: context.schema,
            resources: context.session.validation(),
            diagnostic_errors: &mut context.tree_diagnostic_errors,
            locations: &mut context.locations,
            current_node: context.current_node,
            next_member: context.next_member,
            inherited_constraints: &mut context.inherited_constraints,
            path: &mut context.path,
            diagnostics: &mut context.diagnostics,
            nesting_depth: context.nesting_depth,
            value_nodes: context.value_nodes,
            shape_validated: matches!(&completion, CompletionState::NeedsTraversalWithKnownShape),
            #[cfg(test)]
            work_peak: context.default_work_peak,
            #[cfg(test)]
            shape_skips: context.subtree_shape_skips,
        };
        let result = {
            let mut diagnostic_errors = context.diagnostic_errors.borrow_mut();
            let result = diagnostic_errors.try_call(|diagnostic_out| {
                ValueValidation::subtree(
                    &mut tree,
                    Self::target(target),
                    value,
                    completion,
                    diagnostic_out,
                )
            });
            match result {
                Ok(status) => Ok(status),
                Err(caught) => Err(caught.take()),
            }
        };
        #[cfg(test)]
        {
            context.default_work_peak = tree.work_peak;
            context.subtree_shape_skips = tree.shape_skips;
        }
        let status = match result {
            Ok(status) => status,
            Err(diagnostic) => return Err(out.fail(diagnostic.into())),
        };
        match status {
            SemanticStatus::Success => Ok(()),
            SemanticStatus::Reported => Err(out.fail(ParseFailure::Reported)),
            SemanticStatus::Stopped => Err(out.fail(ParseFailure::Stopped)),
        }
    }

    fn target(target: AssignmentTarget<'_>) -> TreeTarget<'_> {
        TreeTarget {
            expected: target.expected,
            field: target.field,
            variant: target.variant,
            span: Some(target.span),
        }
    }
}
