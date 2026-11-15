use crate::{
    context::{AssignmentTarget, RuleContext},
    diagnostics::{BudgetDiagnostic, DiagnosticFactory},
    instance::{Member, NodeId},
    parser::{
        containers::{CollectionFrame, ContainerSupport, StructureFrame, StructureSpans},
        context::{ParserContext, ParserControl, ValueCallContext},
        enums::EnumFrame,
        literals::{ContextDiagnosticRequest, LiteralParser, ParsedValue},
        recovery::ParseFailure,
        semantics::SemanticParser,
    },
    value::ValueFactory,
};
use core::mem;
use opeo::{ErrSlot, OResult, Out, ResultOutExt};
use typol_core::{
    BudgetError, ChargedVec, DiagnosticCode, DiagnosticContext, LocatedSpan, SecretRange,
    diagnostic_context,
};
use typol_i18n::{Translator, t};
use typol_semantic::{
    ConstraintList, FieldList, FieldView, SchemaAccessError, TypeKind, TypeView,
    VariantPayloadInfo, VariantView,
};
use typol_syntax::{BuiltinType, Keyword, TokenKind};

static PARSER_FRAME_STATE: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "parser-value-frames",
    "frame-stack-transition-invalid",
);

#[derive(Clone, Copy)]
enum ChildState<'schema> {
    Pending(TypeView<'schema>),
    Awaiting,
    Ready,
}

pub(super) struct ChildPhase<'schema> {
    state: ChildState<'schema>,
    parsed: Option<ParsedValue>,
}

impl<'schema> ChildPhase<'schema> {
    fn pending(expected: TypeView<'schema>) -> Self {
        Self {
            state: ChildState::Pending(expected),
            parsed: None,
        }
    }
}

pub(super) enum Body<'schema> {
    Collection(CollectionFrame<'schema>),
    Structure(StructureFrame<'schema>, LocatedSpan),
    Option {
        child: ChildPhase<'schema>,
        span: LocatedSpan,
    },
    Single {
        frame: EnumFrame<'schema>,
        child: ChildPhase<'schema>,
    },
    EnumStructure(StructureFrame<'schema>, EnumFrame<'schema>),
}

pub(super) enum Start<'schema> {
    Value(ParsedValue),
    Frame(Body<'schema>),
}

pub(super) struct ValueScope<'schema> {
    expected: TypeView<'schema>,
    previous_type: TypeView<'schema>,
    field: Option<FieldView<'schema>>,
    variant: Option<VariantView<'schema>>,
    previous_variant: Option<VariantView<'schema>>,
    previous_node: Option<NodeId>,
    inherited: Option<ChargedVec<ConstraintList<'schema>>>,
    inherited_depth: usize,
    path_depth: usize,
    nesting: usize,
    secret_range: Option<SecretRange>,
}

impl<'schema> ValueScope<'schema> {
    pub(super) const fn field(&self) -> Option<FieldView<'schema>> {
        self.field
    }

    pub(super) fn enter<'slot>(
        context: &mut ParserContext<'schema, '_, '_>,
        expected: TypeView<'schema>,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, Self> {
        let start = context.cursor.peek(out.reborrow())?.span();
        let field_secret = match context.target_field {
            Some(field) => schema_call(
                &mut context.session.validation().schema_access_errors,
                |schema_out| field.is_secret(schema_out),
                out.reborrow(),
            )?,
            None => false,
        };
        let type_secret = secret_type(context, expected, out.reborrow())?;
        let child_secret = match (schema_kind(context, expected, out.reborrow()))? {
            TypeKind::Map { value } => (secret_type(context, value, out.reborrow()))?,
            TypeKind::Array { element, .. } => (secret_type(context, element, out.reborrow()))?,
            _ => false,
        };
        let secret = field_secret || type_secret || child_secret;
        let secret_range = if secret {
            context.session.begin_secret(start.span().start())
        } else {
            None
        };
        let previous_type = mem::replace(&mut context.expected_type, expected);
        let variant = context.target_variant.take();
        let previous_variant = mem::replace(&mut context.assignment_variant, variant);
        let inherited = if context
            .current_node
            .and_then(|id| context.locations.get(id))
            .is_some_and(|node| node.path_depth() != context.path.len())
        {
            Some(ParserControl::take_inherited(context, out.reborrow())?)
        } else {
            None
        };
        Ok(Self {
            expected,
            previous_type,
            field: context.target_field,
            variant,
            previous_variant,
            previous_node: context.current_node,
            inherited,
            inherited_depth: context.inherited_constraints.len(),
            path_depth: context.path.len(),
            nesting: context.nesting_depth,
            secret_range,
        })
    }

    pub(super) fn complete<'slot>(
        self,
        context: &mut ParserContext<'schema, '_, '_>,
        parsed: Result<ParsedValue, ParseFailure>,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, ParsedValue> {
        if let (Some(index), Ok(value)) = (self.secret_range, &parsed) {
            context.session.end_secret(index, value.span.span().end());
        }
        // After container and wrapper continuations finish, the parent value callback uses the assignment context from when that value was entered.
        while context.path.len() > self.path_depth {
            context.path.pop();
        }
        context.nesting_depth = self.nesting;
        context.target_field = self.field;
        context.inherited_constraints.truncate(self.inherited_depth);
        let result = match parsed {
            Err(failure) => Err(out.reborrow().fail(failure)),
            Ok(parsed) => Self::complete_parsed(
                context,
                AssignmentTarget {
                    expected: self.expected,
                    field: self.field,
                    variant: self.variant,
                    span: parsed.span,
                },
                parsed,
                out.reborrow(),
            ),
        };
        context.current_node = self.previous_node;
        context.assignment_variant = self.previous_variant;
        if let Some(inherited) = self.inherited {
            context.inherited_constraints = inherited;
        }
        context.expected_type = self.previous_type;
        result
    }

    fn complete_parsed<'slot>(
        context: &mut ParserContext<'schema, '_, '_>,
        target: AssignmentTarget<'schema>,
        parsed: ParsedValue,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, ParsedValue> {
        SemanticParser::check_completed(context, target, &parsed.value, out.reborrow())?;
        if let Some(id) = context.current_node {
            ParserControl::diagnostic_call(
                &context.diagnostic_errors,
                |diagnostic_out| context.locations.complete(id, parsed.span, diagnostic_out),
                out.reborrow(),
            )?;
        }
        Ok(parsed)
    }
}

impl<'schema> Body<'schema> {
    pub(super) fn invalid_parse_state(
        span: Option<LocatedSpan>,
        translator: Translator,
    ) -> ParseFailure {
        let mut diagnostic =
            BudgetDiagnostic::from_error(BudgetError::invariant(&PARSER_FRAME_STATE), translator);
        diagnostic.set_located_primary(span);
        diagnostic.into()
    }

    pub(super) fn current_span(context: &ParserContext<'_, '_, '_>) -> Option<LocatedSpan> {
        context
            .current_node
            .and_then(|node| context.locations.get(node))
            .and_then(|node| node.span())
    }

    pub(super) fn start<'slot>(
        context: &mut ParserContext<'schema, '_, '_>,
        expected: TypeView<'schema>,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, Start<'schema>> {
        let token = context.cursor.peek(out.reborrow())?;
        let span = token.span();
        if token.kind() == TokenKind::Identifier && token.text().unwrap_or("") == "env" {
            let parsed =
                SemanticParser::parse_environment(context, expected, span, out.reborrow())?;
            return Ok(Start::Value(parsed));
        }
        ParserControl::count_value_node(context, span, Some(context.value_source), out.reborrow())?;
        context.current_node = Some(ParserControl::begin_location(
            context,
            expected,
            span,
            out.reborrow(),
        )?);
        let expected_kind = schema_call(
            &mut context.session.validation().schema_access_errors,
            |schema_out| expected.kind(schema_out),
            out.reborrow(),
        )?;
        if (token_is_none(context, out.reborrow()))?
            && !matches!(expected_kind, TypeKind::Option { .. })
        {
            let diagnostic = ParserControl::diagnostic_call(
                &context.diagnostic_errors,
                |diagnostic_out| {
                    LiteralParser::context_diagnostic(
                        ContextDiagnosticRequest {
                            context,
                            code: DiagnosticCode::CONFIG_VALUE_ERROR,
                            message: t! {
                                name: "diagnostic.config.none_requires_option",
                                locale: context.session.translator(),
                            }
                            .into(),
                            span,
                            value_source: Some(context.value_source),
                        },
                        diagnostic_out,
                    )
                },
                out.reborrow(),
            )?;
            return Err(out.fail(diagnostic.into()));
        }
        Self::start_expected(context, expected, expected_kind, span, out.reborrow())
    }

    fn start_expected<'slot>(
        context: &mut ParserContext<'schema, '_, '_>,
        expected: TypeView<'schema>,
        kind: TypeKind<'schema>,
        span: LocatedSpan,
        out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, Start<'schema>> {
        match kind {
            TypeKind::Builtin(builtin) => Self::start_scalar(context, builtin, out),
            TypeKind::Option { inner } => Self::start_option(context, expected, inner, span, out),
            TypeKind::Array { .. } | TypeKind::Map { .. } => {
                Self::start_collection(context, expected, span, out)
            }
            TypeKind::Struct { fields } => Self::start_structure(context, fields, span, out),
            TypeKind::Enum { .. } => Self::start_enum(context, expected, span, out),
        }
    }

    fn start_scalar<'slot>(
        context: &mut ParserContext<'schema, '_, '_>,
        builtin: BuiltinType,
        out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, Start<'schema>> {
        LiteralParser::parse_scalar(context, builtin, out).map(Start::Value)
    }

    fn start_option<'slot>(
        context: &mut ParserContext<'schema, '_, '_>,
        expected: TypeView<'schema>,
        inner: TypeView<'schema>,
        span: LocatedSpan,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, Start<'schema>> {
        if let Some(none) = LiteralParser::take_none(context, out.reborrow())? {
            return Ok(Start::Value(none));
        }
        ParserControl::enter_nesting(context, span, Some(context.value_source), out.reborrow())?;
        let constraints = schema_call(
            &mut context.session.validation().schema_access_errors,
            |schema_out| {
                RuleContext::constraints(
                    expected,
                    context.target_field,
                    context.assignment_variant,
                    schema_out,
                )
            },
            out.reborrow(),
        )?;
        ParserControl::budget_call(
            &mut context.session.validation().budget_errors,
            |budget_out| {
                context
                    .inherited_constraints
                    .try_push(constraints, budget_out)
            },
            out.reborrow(),
        )?;
        Ok(Start::Frame(Body::Option {
            child: ChildPhase::pending(inner),
            span,
        }))
    }

    fn start_collection<'slot>(
        context: &mut ParserContext<'schema, '_, '_>,
        expected: TypeView<'schema>,
        span: LocatedSpan,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, Start<'schema>> {
        ParserControl::enter_nesting(context, span, Some(context.value_source), out.reborrow())?;
        let frame = CollectionFrame::new(context, expected, span, out.reborrow())?;
        Ok(Start::Frame(Body::Collection(frame)))
    }

    fn start_structure<'slot>(
        context: &mut ParserContext<'schema, '_, '_>,
        fields: FieldList<'schema>,
        span: LocatedSpan,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, Start<'schema>> {
        ParserControl::enter_nesting(context, span, Some(context.value_source), out.reborrow())?;
        let frame = StructureFrame::new(context, fields, out.reborrow())?;
        Ok(Start::Frame(Body::Structure(frame, span)))
    }

    fn start_enum<'slot>(
        context: &mut ParserContext<'schema, '_, '_>,
        expected: TypeView<'schema>,
        span: LocatedSpan,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, Start<'schema>> {
        ParserControl::enter_nesting(context, span, Some(context.value_source), out.reborrow())?;
        let frame = EnumFrame::new(context, expected, span, out.reborrow())?;
        frame.enter_payload(context, out.reborrow())?;
        Self::start_enum_payload(context, frame, out.reborrow())
    }

    fn start_enum_payload<'slot>(
        context: &mut ParserContext<'schema, '_, '_>,
        frame: EnumFrame<'schema>,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, Start<'schema>> {
        let payload = schema_call(
            &mut context.session.validation().schema_access_errors,
            |schema_out| frame.variant().payload(schema_out),
            out.reborrow(),
        )?;
        match payload {
            VariantPayloadInfo::Unit => {
                let parsed = frame.finish_unit(context, out.reborrow())?;
                Ok(Start::Value(parsed))
            }
            VariantPayloadInfo::Single { ty } => Ok(Start::Frame(Body::Single {
                frame,
                child: ChildPhase::pending(ty),
            })),
            VariantPayloadInfo::Structure { fields } => {
                let fields = StructureFrame::new(context, fields, out.reborrow())?;
                Ok(Start::Frame(Body::EnumStructure(fields, frame)))
            }
        }
    }

    pub(super) fn next<'slot>(
        &mut self,
        context: &mut ParserContext<'schema, '_, '_>,
        item_errors: &mut ErrSlot<ParseFailure>,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, Option<TypeView<'schema>>> {
        match self {
            Self::Collection(frame) => frame.next(context, item_errors, out.reborrow()),
            Self::Structure(frame, _) | Self::EnumStructure(frame, _) => {
                frame.next(context, item_errors, out.reborrow())
            }
            Self::Option { child, .. } => match child.state {
                ChildState::Pending(inner) => {
                    child.state = ChildState::Awaiting;
                    context.next_member = Member::Optional;
                    context.target_field = None;
                    Ok(Some(inner))
                }
                ChildState::Awaiting | ChildState::Ready => Ok(None),
            },
            Self::Single { child, frame } => match child.state {
                ChildState::Pending(inner) => {
                    child.state = ChildState::Awaiting;
                    context.target_field = None;
                    context.target_variant = Some(frame.variant());
                    context.next_member = Member::Payload(frame.variant());
                    Ok(Some(inner))
                }
                ChildState::Awaiting | ChildState::Ready => Ok(None),
            },
        }
    }

    pub(super) fn accept<'slot>(
        &mut self,
        context: &mut ParserContext<'schema, '_, '_>,
        result: Result<ParsedValue, ParseFailure>,
        item_errors: &mut ErrSlot<ParseFailure>,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, ()> {
        match self {
            Self::Collection(frame) => frame.accept(context, result, item_errors, out.reborrow()),
            Self::Structure(frame, _) | Self::EnumStructure(frame, _) => {
                frame.accept(context, result, item_errors, out.reborrow())
            }
            Self::Option { child, .. } | Self::Single { child, .. } => match child.state {
                ChildState::Awaiting => {
                    child.parsed = Some(result.store_err_into(out.reborrow())?);
                    child.state = ChildState::Ready;
                    Ok(())
                }
                ChildState::Pending(_) | ChildState::Ready => {
                    Err(out.fail(Body::invalid_parse_state(
                        Body::current_span(context),
                        context.session.translator(),
                    )))
                }
            },
        }
    }

    pub(super) fn finish<'slot>(
        self,
        context: &mut ParserContext<'schema, '_, '_>,
        item_errors: &mut ErrSlot<ParseFailure>,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, ParsedValue> {
        match self {
            Self::Option { child, span } => {
                Self::finish_option(child, span, context, out.reborrow())
            }
            Self::Single { frame, child } => {
                Self::finish_single(frame, child, context, out.reborrow())
            }
            Self::Collection(frame) => Self::finish_collection(frame, context, out.reborrow()),
            Self::Structure(frame, span) => {
                Self::finish_structure(frame, span, context, item_errors, out.reborrow())
            }
            Self::EnumStructure(fields, frame) => {
                Self::finish_enum_structure(fields, frame, context, item_errors, out.reborrow())
            }
        }
    }

    fn finish_option<'slot>(
        child: ChildPhase<'schema>,
        span: LocatedSpan,
        context: &mut ParserContext<'schema, '_, '_>,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, ParsedValue> {
        let parsed = ready_child(
            child,
            Body::current_span(context),
            context.session.translator(),
            out.reborrow(),
        )?;
        let parsed_span = parsed.span;
        let parsed_value = parsed.value;
        let value_source = context.value_source;
        let budget = &context.budget;
        let value = ContainerSupport::value_call(
            &mut context.value_errors,
            move |value_out| {
                ValueFactory::optional_owned(parsed_value, value_source, Some(budget), value_out)
            },
            ValueCallContext {
                span: parsed_span,
                source: value_source,
                translator: context.session.translator(),
            },
            out.reborrow(),
        )?;
        Ok(ParsedValue {
            value,
            span: ParserControl::diagnostic_call(
                &context.diagnostic_errors,
                |diagnostic_out| {
                    DiagnosticFactory::span_through(
                        span,
                        parsed_span,
                        context.session.translator(),
                        diagnostic_out,
                    )
                },
                out.reborrow(),
            )?,
        })
    }

    fn finish_single<'slot>(
        frame: EnumFrame<'schema>,
        child: ChildPhase<'schema>,
        context: &mut ParserContext<'schema, '_, '_>,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, ParsedValue> {
        let parsed = ready_child(
            child,
            Body::current_span(context),
            context.session.translator(),
            out.reborrow(),
        )?;
        context.path.pop();
        frame.finish_single(context, parsed, out)
    }

    fn finish_collection<'slot>(
        frame: CollectionFrame<'schema>,
        context: &mut ParserContext<'schema, '_, '_>,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, ParsedValue> {
        let close = context.cursor.peek(out.reborrow())?.span();
        context.cursor.bump(out.reborrow())?;
        frame.finish(context, close, out)
    }

    fn finish_structure<'slot>(
        frame: StructureFrame<'schema>,
        initial: LocatedSpan,
        context: &mut ParserContext<'schema, '_, '_>,
        item_errors: &mut ErrSlot<ParseFailure>,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, ParsedValue> {
        let close = context.cursor.peek(out.reborrow())?.span();
        context.cursor.bump(out.reborrow())?;
        frame.finish_value(context, StructureSpans { initial, close }, item_errors, out)
    }

    fn finish_enum_structure<'slot>(
        fields: StructureFrame<'schema>,
        frame: EnumFrame<'schema>,
        context: &mut ParserContext<'schema, '_, '_>,
        item_errors: &mut ErrSlot<ParseFailure>,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, ParsedValue> {
        let close = context.cursor.peek(out.reborrow())?.span();
        context.cursor.bump(out.reborrow())?;
        let fields = fields.finish(context, close, item_errors, out.reborrow())?;
        context.path.pop();
        frame.finish_structure(context, fields, close, out)
    }
}

fn ready_child<'slot, 'borrow>(
    mut child: ChildPhase<'_>,
    span: Option<LocatedSpan>,
    translator: Translator,
    out: Out<'slot, 'borrow, ParseFailure>,
) -> OResult<'slot, ParsedValue> {
    match child.state {
        ChildState::Ready => child
            .parsed
            .take()
            .ok_or_else(|| out.fail(Body::invalid_parse_state(span, translator))),
        ChildState::Pending(_) | ChildState::Awaiting => {
            Err(out.fail(Body::invalid_parse_state(span, translator)))
        }
    }
}

fn token_is_none<'slot>(
    context: &mut ParserContext<'_, '_, '_>,
    mut out: Out<'slot, '_, ParseFailure>,
) -> OResult<'slot, bool> {
    Ok(context.cursor.peek(out.reborrow())?.kind() == TokenKind::Keyword(Keyword::None))
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

fn schema_kind<'schema, 'slot>(
    context: &mut ParserContext<'schema, '_, '_>,
    ty: TypeView<'schema>,
    mut out: Out<'slot, '_, ParseFailure>,
) -> OResult<'slot, TypeKind<'schema>> {
    let info = ty;
    schema_call(
        &mut context.session.validation().schema_access_errors,
        |schema_out| info.kind(schema_out),
        out.reborrow(),
    )
}

fn secret_type<'schema, 'slot>(
    context: &mut ParserContext<'schema, '_, '_>,
    mut ty: TypeView<'schema>,
    mut out: Out<'slot, '_, ParseFailure>,
) -> OResult<'slot, bool> {
    let mut secret = false;
    loop {
        let info = ty;
        let is_secret = schema_call(
            &mut context.session.validation().schema_access_errors,
            |schema_out| info.is_secret(schema_out),
            out.reborrow(),
        )?;
        if is_secret {
            secret = true;
            break;
        }
        match (schema_kind(context, info, out.reborrow()))? {
            TypeKind::Option { inner } => ty = inner,
            _ => break,
        }
    }
    Ok(secret)
}
