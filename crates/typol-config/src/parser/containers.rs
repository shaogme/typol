mod structure;

use crate::{
    context::CompletionState,
    diagnostics::{BudgetDiagnostic, DiagnosticFactory},
    instance::Member,
    parser::{
        context::{ParserContext, ParserControl},
        literals::{ContextDiagnosticRequest, LiteralParser, ParsedValue, Syntax},
        map_index::{EntryId, MapIndex},
        recovery::{ParseFailure, Recovery},
    },
    value::ValueFactory,
};
use opeo::{ErrSlot, OResult, Out};
use typol_core::{
    BudgetError, ChargedString, ChargedVec, ConfigValue, Diagnostic, DiagnosticCode,
    DiagnosticContext, LocatedSpan, ValueError, ValueErrorKind, ValueSource, diagnostic_context,
};
use typol_i18n::{diagnostic_key, diagnostic_message};
use typol_semantic::{FieldView, TypeKind, TypeView};
use typol_syntax::{Symbol, TokenKind};

pub(super) use structure::StructureFrame;

pub(super) struct StructureSpans {
    pub(super) initial: LocatedSpan,
    pub(super) close: LocatedSpan,
}

static PARSER_CONTAINER_METADATA: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "parser-container",
    "compiled-field-or-token-unavailable",
);
static PARSER_CONTAINER_STATE: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "parser-container",
    "field-or-item-state-invalid",
);
static PARSER_CONTAINER_VALUE_INVARIANT: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "parser-container",
    "container-value-invariant-failed",
);

enum CollectionKind<'schema> {
    Array {
        element: TypeView<'schema>,
        length: Option<u32>,
        values: ChargedVec<ConfigValue>,
        index: usize,
    },
    Map {
        value: TypeView<'schema>,
        seen: MapIndex,
        key: Option<EntryId>,
    },
}

pub(super) struct CollectionFrame<'schema> {
    kind: CollectionKind<'schema>,
    initial_span: LocatedSpan,
    depth: usize,
    failed: bool,
}

impl<'schema> CollectionFrame<'schema> {
    pub(super) fn new<'slot>(
        context: &mut ParserContext<'schema, '_, '_>,
        expected: TypeView<'schema>,
        initial_span: LocatedSpan,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, Self> {
        let expected_kind = ParserControl::schema_call(
            &mut context.session.validation().schema_access_errors,
            |schema_out| expected.kind(schema_out),
            out.reborrow(),
        )?;
        let kind = match expected_kind {
            TypeKind::Array { element, length } => {
                Syntax::expect_value_symbol(
                    context,
                    Symbol::LeftBracket,
                    "expected an array here",
                    out.reborrow(),
                )?;
                CollectionKind::Array {
                    element,
                    length,
                    values: ChargedVec::new(Some(&context.budget), out.reborrow())?,
                    index: 0,
                }
            }
            TypeKind::Map { value } => {
                Syntax::expect_value_symbol(
                    context,
                    Symbol::LeftBrace,
                    "expected a map object here",
                    out.reborrow(),
                )?;
                CollectionKind::Map {
                    value,
                    seen: MapIndex::new_budgeted(&context.budget, out.reborrow())?,
                    key: None,
                }
            }
            _ => return Err(out.fail(internal_field(initial_span).into())),
        };
        Ok(Self {
            kind,
            initial_span,
            depth: context.cursor.depth(),
            failed: false,
        })
    }

    fn close(&self) -> Symbol {
        match self.kind {
            CollectionKind::Array { .. } => Symbol::RightBracket,
            CollectionKind::Map { .. } => Symbol::RightBrace,
        }
    }

    pub(super) fn next<'slot>(
        &mut self,
        context: &mut ParserContext<'schema, '_, '_>,
        item_errors: &mut ErrSlot<ParseFailure>,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, Option<TypeView<'schema>>> {
        loop {
            if (context.cursor.peek(out.reborrow()))?.kind() == TokenKind::Symbol(self.close()) {
                break Ok(None);
            }
            let path_depth = context.path.len();
            match item_errors.try_call(|item_out| self.start_item(context, item_out)) {
                Ok(ty) => break Ok(Some(ty)),
                Err(caught) => {
                    let failure = caught.take();
                    while context.path.len() > path_depth {
                        context.path.pop();
                    }
                    self.failed = true;
                    Recovery::recover_item(
                        context,
                        self.depth,
                        self.close(),
                        failure,
                        out.reborrow(),
                    )?;
                }
            }
        }
    }

    fn start_item<'slot>(
        &mut self,
        context: &mut ParserContext<'schema, '_, '_>,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, TypeView<'schema>> {
        let budget = ParserControl::budget_call(
            &mut context.session.validation().budget_errors,
            |budget_out| context.budget.try_clone(budget_out),
            out.reborrow(),
        )?;
        let text_limit = context.config_limits.max_constructed_text_bytes();
        let token = context.cursor.peek(out.reborrow())?;
        let span = token.span();
        let ty = match &mut self.kind {
            CollectionKind::Array { element, index, .. } => {
                context.next_member = Member::Index(*index);
                ParserControl::budget_call(
                    &mut context.session.validation().budget_errors,
                    |budget_out| context.path.push_index(*index, budget_out),
                    out.reborrow(),
                )?;
                *element
            }
            CollectionKind::Map {
                value, seen, key, ..
            } => {
                if token.kind() != TokenKind::String {
                    let code = if token.kind() == TokenKind::Identifier {
                        DiagnosticCode::CONFIG_VALUE_ERROR
                    } else {
                        DiagnosticCode::CONFIG_SYNTAX_ERROR
                    };
                    let diagnostic = context_diagnostic(
                        ContextDiagnosticRequest {
                            context,
                            code,
                            message: "expected a string key or closing brace here in the map object",
                            key: Some(diagnostic_key!("diagnostic.syntax.braced_literal_keys")),
                            span,
                            value_source: None,
                        },
                        out.reborrow(),
                    )?;
                    return Err(out.fail(diagnostic.into()));
                }
                let Some(decoded) = token.decoded_string() else {
                    return Err(out.fail(internal_field(span).into()));
                };
                budget.construct_text(decoded.len(), text_limit, out.reborrow())?;
                let owned = ChargedString::copy(decoded, Some(&budget), out.reborrow())?;
                check_name(context, &owned, span, out.reborrow())?;
                ParserControl::count_value_node(
                    context,
                    span,
                    Some(context.value_source),
                    out.reborrow(),
                )?;
                push_map_path(context, &owned, span, out.reborrow())?;
                let Some(index) = ParserControl::budget_call(
                    &mut context.session.validation().budget_errors,
                    |budget_out| seen.insert_budgeted(owned, budget_out),
                    out.reborrow(),
                )?
                else {
                    let diagnostic = context_diagnostic(
                        ContextDiagnosticRequest {
                            context,
                            code: DiagnosticCode::CONFIG_VALUE_ERROR,
                            message: "map contains a duplicate key",
                            key: Some(diagnostic_key!(
                                "diagnostic.config.duplicate_object_field_or_map_key"
                            )),
                            span,
                            value_source: None,
                        },
                        out.reborrow(),
                    )?;
                    return Err(out.fail(diagnostic.into()));
                };
                context.cursor.bump(out.reborrow())?;
                Syntax::expect_symbol(
                    context,
                    Symbol::Colon,
                    DiagnosticCode::CONFIG_SYNTAX_ERROR,
                    "expected a colon after the map key",
                    out.reborrow(),
                )?;
                *key = Some(index);
                context.next_member = Member::MapItem(index.slot());
                *value
            }
        };
        context.target_field = None;
        Ok(ty)
    }

    pub(super) fn accept<'slot>(
        &mut self,
        context: &mut ParserContext<'schema, '_, '_>,
        result: Result<ParsedValue, ParseFailure>,
        item_errors: &mut ErrSlot<ParseFailure>,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, ()> {
        let item = item_errors.try_call(|mut item_out| {
            let parsed = match result {
                Ok(parsed) => parsed,
                Err(failure) => return Err(item_out.fail(failure)),
            };
            match &mut self.kind {
                CollectionKind::Array { values, .. } => {
                    ParserControl::budget_call(
                        &mut context.session.validation().budget_errors,
                        |budget_out| values.try_push(parsed.value, budget_out),
                        item_out.reborrow(),
                    )?;
                }
                CollectionKind::Map { seen, key, .. } => {
                    let Some(index) = key.take() else {
                        return Err(item_out.fail(invalid_parser_state(Some(self.initial_span))));
                    };
                    ParserControl::budget_call(
                        &mut context.session.validation().budget_errors,
                        |budget_out| seen.set_value(index, parsed.value, budget_out),
                        item_out.reborrow(),
                    )?;
                }
            }
            let message = if self.close() == Symbol::RightBracket {
                "expected a comma or closing bracket after the array element"
            } else {
                "expected a comma or closing brace after the map entry"
            };
            item_boundary(context, self.close(), message, item_out.reborrow())
        });
        context.path.pop();
        if let CollectionKind::Array { index, .. } = &mut self.kind {
            let Some(next_index) = index.checked_add(1) else {
                return Err(out.fail(
                    DiagnosticFactory::make_diagnostic(
                        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED,
                        "array element count exceeded the limit",
                        Some(self.initial_span),
                        None,
                        None,
                    )
                    .into(),
                ));
            };
            *index = next_index;
        }
        let item = match item {
            Ok(()) => Ok(()),
            Err(caught) => Err(caught.take()),
        };
        if let Err(failure) = item {
            self.failed = true;
            Recovery::recover_item(context, self.depth, self.close(), failure, out.reborrow())?;
        }
        Ok(())
    }

    pub(super) fn finish<'slot>(
        self,
        context: &mut ParserContext<'schema, '_, '_>,
        close_span: LocatedSpan,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, ParsedValue> {
        let value_source = context.value_source;
        let budget = &context.budget;
        if self.failed {
            return Err(out.fail(ParseFailure::Reported));
        }
        let value = match self.kind {
            CollectionKind::Array { values, length, .. } => {
                if length.is_some_and(|length| values.len() as u128 != u128::from(length)) {
                    let diagnostic = context_diagnostic(
                        ContextDiagnosticRequest {
                            context,
                            code: DiagnosticCode::CONFIG_VALUE_ERROR,
                            message: "fixed-size array element count does not match the schema declaration",
                            key: Some(diagnostic_key!(
                                "diagnostic.config.fixed_array_length_mismatch"
                            )),
                            span: close_span,
                            value_source: Some(context.value_source),
                        },
                        out.reborrow(),
                    )?;
                    return Err(out.fail(diagnostic.into()));
                }
                value_call(
                    &mut context.value_errors,
                    move |value_out| {
                        ValueFactory::array_owned(values, value_source, Some(budget), value_out)
                    },
                    close_span,
                    value_source,
                    out.reborrow(),
                )?
            }
            CollectionKind::Map { seen, .. } => {
                let entries = seen.into_entries(out.reborrow())?;
                value_call(
                    &mut context.value_errors,
                    move |value_out| {
                        ValueFactory::map_owned(entries, value_source, Some(budget), value_out)
                    },
                    close_span,
                    value_source,
                    out.reborrow(),
                )?
            }
        };
        Ok(ParsedValue {
            completion: CompletionState::ChildrenComplete,
            value,
            span: ParserControl::diagnostic_call(
                &context.diagnostic_errors,
                |diagnostic_out| {
                    DiagnosticFactory::span_through(self.initial_span, close_span, diagnostic_out)
                },
                out.reborrow(),
            )?,
        })
    }
}

fn item_boundary<'slot>(
    context: &mut ParserContext<'_, '_, '_>,
    close: Symbol,
    message: &str,
    mut out: Out<'slot, '_, ParseFailure>,
) -> OResult<'slot, ()> {
    let (kind, span) = {
        let token = context.cursor.peek(out.reborrow())?;
        (token.kind(), token.span())
    };
    if kind == TokenKind::Symbol(Symbol::Comma) {
        context.cursor.bump(out.reborrow())?;
        return Ok(());
    }
    if kind == TokenKind::Symbol(close) {
        return Ok(());
    }
    let diagnostic = context_diagnostic(
        ContextDiagnosticRequest {
            context,
            code: DiagnosticCode::CONFIG_SYNTAX_ERROR,
            message,
            key: Some(diagnostic_key!("diagnostic.syntax.expected_punctuation")),
            span,
            value_source: None,
        },
        out.reborrow(),
    )?;
    Err(out.fail(diagnostic.into()))
}

fn check_name<'slot, 'borrow>(
    context: &ParserContext<'_, '_, '_>,
    name: &str,
    span: LocatedSpan,
    out: Out<'slot, 'borrow, ParseFailure>,
) -> OResult<'slot, ()> {
    if name.len() > context.limits.max_string_bytes() {
        return Err(out.fail(
            DiagnosticFactory::make_diagnostic(
                DiagnosticCode::RESOURCE_LIMIT_EXCEEDED,
                "name or map key exceeded the string byte limit",
                Some(span),
                None,
                None,
            )
            .into(),
        ));
    }
    Ok(())
}

fn internal_field(span: LocatedSpan) -> Diagnostic {
    DiagnosticFactory::internal(PARSER_CONTAINER_METADATA, Some(span), None, None)
}

fn container_error(error: ValueError, span: LocatedSpan, source: ValueSource) -> Diagnostic {
    match error.kind() {
        ValueErrorKind::ResourceLimitExceeded => {
            BudgetDiagnostic::from_error(BudgetError::Resource)
        }
        ValueErrorKind::AllocationFailed => {
            DiagnosticFactory::allocation_diagnostic(Some(span), None, Some(source))
        }
        ValueErrorKind::CapacityExceeded => {
            BudgetDiagnostic::from_error(BudgetError::CapacityExceeded)
        }
        ValueErrorKind::InternalInvariantFailure => DiagnosticFactory::internal(
            error.context().unwrap_or(PARSER_CONTAINER_VALUE_INVARIANT),
            Some(span),
            None,
            Some(source),
        ),
        ValueErrorKind::DuplicateFieldName | ValueErrorKind::DuplicateMapKey => {
            DiagnosticFactory::internal(
                error
                    .duplicate_indices()
                    .map_or(PARSER_CONTAINER_VALUE_INVARIANT, |(index, _)| {
                        PARSER_CONTAINER_VALUE_INVARIANT.with_object_id(index as u64)
                    }),
                Some(span),
                None,
                Some(source),
            )
        }
    }
}

fn invalid_parser_state(span: Option<LocatedSpan>) -> ParseFailure {
    DiagnosticFactory::internal(PARSER_CONTAINER_STATE, span, None, None).into()
}

fn current_span(context: &ParserContext<'_, '_, '_>) -> Option<LocatedSpan> {
    context
        .current_node
        .and_then(|node| context.locations.get(node))
        .and_then(|node| node.span())
}

fn push_map_path<'slot>(
    context: &mut ParserContext<'_, '_, '_>,
    key: &str,
    _span: LocatedSpan,
    mut out: Out<'slot, '_, ParseFailure>,
) -> OResult<'slot, ()> {
    ParserControl::budget_call(
        &mut context.session.validation().budget_errors,
        |budget_out| context.path.push_map_key(key, budget_out),
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

fn value_call<'slot, 'borrow, T>(
    errors: &mut ErrSlot<ValueError>,
    operation: impl for<'call> FnOnce(Out<'call, 'call, ValueError>) -> OResult<'call, T>,
    span: LocatedSpan,
    source: ValueSource,
    out: Out<'slot, 'borrow, ParseFailure>,
) -> OResult<'slot, T> {
    match errors.try_call(operation) {
        Ok(value) => Ok(value),
        Err(caught) => Err(out.fail(container_error(caught.take(), span, source).into())),
    }
}

fn field_member<'schema>(
    context: &ParserContext<'schema, '_, '_>,
    field: FieldView<'schema>,
) -> Member<'schema> {
    match context
        .current_node
        .and_then(|id| context.locations.get(id))
        .and_then(|node| node.variant())
    {
        Some(variant) => Member::VariantField { variant, field },
        None => Member::Field(field),
    }
}

pub(super) struct ContainerSupport;

impl ContainerSupport {
    pub(super) fn item_boundary<'slot, 'borrow>(
        context: &mut ParserContext<'_, '_, '_>,
        close: Symbol,
        message: &str,
        out: Out<'slot, 'borrow, ParseFailure>,
    ) -> OResult<'slot, ()> {
        item_boundary(context, close, message, out)
    }

    pub(super) fn context_diagnostic<'slot, 'borrow>(
        request: ContextDiagnosticRequest<'_, '_, '_, '_>,
        out: Out<'slot, 'borrow, ParseFailure>,
    ) -> OResult<'slot, Diagnostic> {
        context_diagnostic(request, out)
    }

    pub(super) fn value_call<'slot, 'borrow, T>(
        errors: &mut ErrSlot<ValueError>,
        operation: impl for<'call> FnOnce(Out<'call, 'call, ValueError>) -> OResult<'call, T>,
        span: LocatedSpan,
        source: ValueSource,
        out: Out<'slot, 'borrow, ParseFailure>,
    ) -> OResult<'slot, T> {
        value_call(errors, operation, span, source, out)
    }

    pub(super) fn invalid_parser_state(span: Option<LocatedSpan>) -> ParseFailure {
        invalid_parser_state(span)
    }

    pub(super) fn current_span(context: &ParserContext<'_, '_, '_>) -> Option<LocatedSpan> {
        current_span(context)
    }

    pub(super) fn field_member<'schema>(
        context: &ParserContext<'schema, '_, '_>,
        field: FieldView<'schema>,
    ) -> Member<'schema> {
        field_member(context, field)
    }

    pub(super) fn push_field_path<'slot>(
        context: &mut ParserContext<'_, '_, '_>,
        name: &str,
        span: LocatedSpan,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, ()> {
        if name.len() > context.limits.max_string_bytes() {
            return Err(out.fail(
                DiagnosticFactory::make_diagnostic(
                    DiagnosticCode::RESOURCE_LIMIT_EXCEEDED,
                    "field name exceeded the string byte limit",
                    Some(span),
                    None,
                    None,
                )
                .into(),
            ));
        }
        ParserControl::budget_call(
            &mut context.session.validation().budget_errors,
            |budget_out| context.path.push_field(name, budget_out),
            out.reborrow(),
        )?;
        Ok(())
    }

    pub(super) fn missing_field_diagnostic<'slot>(
        context: &ParserContext<'_, '_, '_>,
        close_span: LocatedSpan,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, Diagnostic> {
        let path = ParserControl::diagnostic_call(
            &context.diagnostic_errors,
            |diagnostic_out| {
                LiteralParser::copy_path_with_source(
                    context,
                    Some(close_span),
                    None,
                    diagnostic_out,
                )
            },
            out.reborrow(),
        )?;
        Ok(DiagnosticFactory::make_diagnostic(
            DiagnosticCode::CONFIG_VALUE_ERROR,
            diagnostic_message!(
                "diagnostic.config.required_field_missing",
                "required field is missing"
            ),
            Some(close_span),
            Some(path),
            None,
        ))
    }
}
