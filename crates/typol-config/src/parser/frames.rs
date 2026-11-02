use crate::{
    context::{AssignmentTarget, CompletionState, RuleContext},
    diagnostics::{BudgetDiagnostic, DiagnosticFactory},
    instance::{Member, NodeId},
    parser::{
        containers::{CollectionFrame, ContainerSupport, StructureFrame, StructureSpans},
        context::{ParserContext, ParserControl},
        enums::EnumFrame,
        literals::{ContextDiagnosticRequest, LiteralParser, ParsedValue},
        recovery::ParseFailure,
        semantics::{SemanticParser, SubtreeValidator},
    },
    value::ValueFactory,
};
use core::mem;
use opeo::{ErrSlot, OResult, Out, ResultOutExt};
use typol_core::{
    BudgetError, ChargedVec, DiagnosticCode, DiagnosticContext, LocatedSpan, SecretRange,
    diagnostic_context,
};
use typol_i18n::diagnostic_key;
use typol_semantic::{
    ConstraintList, FieldView, SchemaAccessError, TypeKind, TypeView, VariantPayloadInfo,
    VariantView,
};
use typol_syntax::{Keyword, TokenKind};

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

struct ChildPhase<'schema> {
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

enum Body<'schema> {
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

enum Start<'schema> {
    Value(ParsedValue),
    Frame(Body<'schema>),
}

struct ValueScope<'schema> {
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

struct ValueFrame<'schema> {
    scope: ValueScope<'schema>,
    body: Body<'schema>,
}

pub(super) struct ValueParser;

enum ParsePhase<'schema> {
    Enter(TypeView<'schema>),
    Resume,
    Child,
}

struct ParseState {
    child: Option<Result<ParsedValue, ParseFailure>>,
    progress: Option<Result<(), ParseFailure>>,
}

impl ValueParser {
    pub(super) fn parse<'schema, 'slot>(
        context: &mut ParserContext<'schema, '_, '_>,
        expected: TypeView<'schema>,
        errors: &mut ErrSlot<ParseFailure>,
        item_errors: &mut ErrSlot<ParseFailure>,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, ParsedValue> {
        let mut frames: ChargedVec<ValueFrame<'schema>> =
            ChargedVec::new(Some(&context.budget), out.reborrow())?;
        let mut state = ParseState {
            child: None,
            progress: None,
        };
        let mut phase = ParsePhase::Enter(expected);
        loop {
            phase = match phase {
                ParsePhase::Enter(ty) => Self::enter(context, &mut frames, errors, &mut state, ty),
                ParsePhase::Resume => {
                    Self::advance(context, &mut frames, errors, item_errors, &mut state)
                }
                ParsePhase::Child => {
                    let Some(result) = state.child.take() else {
                        return Err(out.fail(invalid_parse_state(current_span(context))));
                    };
                    let Some(frame) = frames.last_mut() else {
                        return match result {
                            Ok(value) => Ok(value),
                            Err(failure) => Err(out.fail(failure)),
                        };
                    };
                    context.target_field = frame.scope.field;
                    match capture(errors, |phase_out| {
                        frame.body.accept(context, result, item_errors, phase_out)
                    }) {
                        Ok(()) => {
                            Self::advance(context, &mut frames, errors, item_errors, &mut state)
                        }
                        Err(failure) => {
                            state.progress = Some(Err(failure));
                            Self::complete_frame(
                                context,
                                &mut frames,
                                errors,
                                item_errors,
                                &mut state,
                            )
                        }
                    }
                }
            };
        }
    }

    fn enter<'schema>(
        context: &mut ParserContext<'schema, '_, '_>,
        frames: &mut ChargedVec<ValueFrame<'schema>>,
        errors: &mut ErrSlot<ParseFailure>,
        state: &mut ParseState,
        ty: TypeView<'schema>,
    ) -> ParsePhase<'schema> {
        let preparation = capture(errors, |mut phase_out| {
            let _ = context.cursor.peek(phase_out.reborrow())?;
            frames.try_reserve(1, phase_out.reborrow())?;
            Ok(())
        });
        match preparation {
            Err(failure) => {
                state.child = Some(Err(failure));
                ParsePhase::Child
            }
            Ok(()) => match capture(errors, |phase_out| {
                ValueScope::enter(context, ty, phase_out)
            }) {
                Err(failure) => {
                    state.child = Some(Err(failure));
                    ParsePhase::Child
                }
                Ok(scope) => match capture(errors, |phase_out| Body::start(context, ty, phase_out))
                {
                    Ok(Start::Value(parsed)) => {
                        state.child = Some(capture(errors, |phase_out| {
                            scope.complete(context, Ok(parsed), phase_out)
                        }));
                        ParsePhase::Child
                    }
                    Ok(Start::Frame(body)) => {
                        match capture(errors, |phase_out| {
                            frames.try_push(ValueFrame { scope, body }, phase_out)
                        }) {
                            Ok(()) => ParsePhase::Resume,
                            Err(failure) => {
                                state.child = Some(Err(failure));
                                ParsePhase::Child
                            }
                        }
                    }
                    Err(failure) => {
                        state.child = Some(capture(errors, |phase_out| {
                            scope.complete(context, Err(failure), phase_out)
                        }));
                        ParsePhase::Child
                    }
                },
            },
        }
    }

    fn advance<'schema>(
        context: &mut ParserContext<'schema, '_, '_>,
        frames: &mut ChargedVec<ValueFrame<'schema>>,
        errors: &mut ErrSlot<ParseFailure>,
        item_errors: &mut ErrSlot<ParseFailure>,
        state: &mut ParseState,
    ) -> ParsePhase<'schema> {
        let Some(frame) = frames.last_mut() else {
            state.child = Some(Err(invalid_parse_state(current_span(context))));
            return ParsePhase::Child;
        };
        context.target_field = frame.scope.field;
        match capture(errors, |phase_out| {
            frame.body.next(context, item_errors, phase_out)
        }) {
            Ok(Some(expected)) => ParsePhase::Enter(expected),
            Ok(None) => {
                state.progress = Some(Ok(()));
                Self::complete_frame(context, frames, errors, item_errors, state)
            }
            Err(failure) => {
                state.progress = Some(Err(failure));
                Self::complete_frame(context, frames, errors, item_errors, state)
            }
        }
    }

    fn complete_frame<'schema>(
        context: &mut ParserContext<'schema, '_, '_>,
        frames: &mut ChargedVec<ValueFrame<'schema>>,
        errors: &mut ErrSlot<ParseFailure>,
        item_errors: &mut ErrSlot<ParseFailure>,
        state: &mut ParseState,
    ) -> ParsePhase<'schema> {
        let Some(frame) = frames.pop() else {
            state.child = Some(Err(invalid_parse_state(current_span(context))));
            state.progress = None;
            return ParsePhase::Child;
        };
        let progress = state
            .progress
            .take()
            .unwrap_or_else(|| Err(invalid_parse_state(current_span(context))));
        let parsed = match progress {
            Ok(()) => capture(errors, |phase_out| {
                frame.body.finish(context, item_errors, phase_out)
            }),
            Err(failure) => Err(failure),
        };
        state.child = Some(capture(errors, |phase_out| {
            frame.scope.complete(context, parsed, phase_out)
        }));
        ParsePhase::Child
    }
}

fn invalid_parse_state(span: Option<LocatedSpan>) -> ParseFailure {
    let mut diagnostic = BudgetDiagnostic::from_error(BudgetError::invariant(&PARSER_FRAME_STATE));
    diagnostic.set_located_primary(span);
    diagnostic.into()
}

fn current_span(context: &ParserContext<'_, '_, '_>) -> Option<LocatedSpan> {
    context
        .current_node
        .and_then(|node| context.locations.get(node))
        .and_then(|node| node.span())
}

impl<'schema> ValueScope<'schema> {
    fn enter<'slot>(
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

    fn complete<'slot>(
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
        if !matches!(parsed.completion, CompletionState::ChildrenComplete) {
            SubtreeValidator::validate(
                context,
                target,
                &parsed.value,
                parsed.completion,
                out.reborrow(),
            )?;
        } else {
            SemanticParser::check_completed(context, target, &parsed.value, out.reborrow())?;
            if let Some(id) = context.current_node {
                ParserControl::diagnostic_call(
                    &context.diagnostic_errors,
                    |diagnostic_out| context.locations.complete(id, parsed.span, diagnostic_out),
                    out.reborrow(),
                )?;
            }
        }
        Ok(ParsedValue {
            completion: CompletionState::ChildrenComplete,
            ..parsed
        })
    }
}

impl<'schema> Body<'schema> {
    fn start<'slot>(
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
                            message: "none can only be assigned to an optional type",
                            key: Some(diagnostic_key!("diagnostic.config.none_requires_option")),
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
        let body = match expected_kind {
            TypeKind::Builtin(builtin) => {
                let parsed = LiteralParser::parse_scalar(context, builtin, out.reborrow())?;
                return Ok(Start::Value(parsed));
            }
            TypeKind::Option { inner } => {
                if let Some(none) = LiteralParser::take_none(context, out.reborrow())? {
                    return Ok(Start::Value(none));
                }
                ParserControl::enter_nesting(
                    context,
                    span,
                    Some(context.value_source),
                    out.reborrow(),
                )?;
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
                Body::Option {
                    child: ChildPhase::pending(inner),
                    span,
                }
            }
            TypeKind::Array { .. } | TypeKind::Map { .. } => {
                ParserControl::enter_nesting(
                    context,
                    span,
                    Some(context.value_source),
                    out.reborrow(),
                )?;
                Body::Collection(CollectionFrame::new(
                    context,
                    expected,
                    span,
                    out.reborrow(),
                )?)
            }
            TypeKind::Struct { fields } => {
                ParserControl::enter_nesting(
                    context,
                    span,
                    Some(context.value_source),
                    out.reborrow(),
                )?;
                Body::Structure(StructureFrame::new(context, fields, out.reborrow())?, span)
            }
            TypeKind::Enum { .. } => {
                ParserControl::enter_nesting(
                    context,
                    span,
                    Some(context.value_source),
                    out.reborrow(),
                )?;
                let frame = EnumFrame::new(context, expected, span, out.reborrow())?;
                frame.enter_payload(context, out.reborrow())?;
                let payload = schema_call(
                    &mut context.session.validation().schema_access_errors,
                    |schema_out| frame.variant().payload(schema_out),
                    out.reborrow(),
                )?;
                match payload {
                    VariantPayloadInfo::Unit => {
                        let parsed = frame.finish_unit(context, out.reborrow())?;
                        return Ok(Start::Value(parsed));
                    }
                    VariantPayloadInfo::Single { ty } => Body::Single {
                        frame,
                        child: ChildPhase::pending(ty),
                    },
                    VariantPayloadInfo::Structure { fields } => Body::EnumStructure(
                        StructureFrame::new(context, fields, out.reborrow())?,
                        frame,
                    ),
                }
            }
        };
        Ok(Start::Frame(body))
    }

    fn next<'slot>(
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

    fn accept<'slot>(
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
                    Err(out.fail(invalid_parse_state(current_span(context))))
                }
            },
        }
    }

    fn finish<'slot>(
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
        let parsed = ready_child(child, current_span(context), out.reborrow())?;
        let parsed_span = parsed.span;
        let parsed_value = parsed.value;
        let value_source = context.value_source;
        let budget = &context.budget;
        let value = ContainerSupport::value_call(
            &mut context.value_errors,
            move |value_out| {
                ValueFactory::optional_owned(parsed_value, value_source, Some(budget), value_out)
            },
            parsed_span,
            value_source,
            out.reborrow(),
        )?;
        Ok(ParsedValue {
            completion: CompletionState::ChildrenComplete,
            value,
            span: ParserControl::diagnostic_call(
                &context.diagnostic_errors,
                |diagnostic_out| DiagnosticFactory::span_through(span, parsed_span, diagnostic_out),
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
        let parsed = ready_child(child, current_span(context), out.reborrow())?;
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
    out: Out<'slot, 'borrow, ParseFailure>,
) -> OResult<'slot, ParsedValue> {
    match child.state {
        ChildState::Ready => child
            .parsed
            .take()
            .ok_or_else(|| out.fail(invalid_parse_state(span))),
        ChildState::Pending(_) | ChildState::Awaiting => Err(out.fail(invalid_parse_state(span))),
    }
}

fn token_is_none<'slot>(
    context: &mut ParserContext<'_, '_, '_>,
    mut out: Out<'slot, '_, ParseFailure>,
) -> OResult<'slot, bool> {
    Ok(context.cursor.peek(out.reborrow())?.kind() == TokenKind::Keyword(Keyword::None))
}

fn capture<T>(
    errors: &mut ErrSlot<ParseFailure>,
    operation: impl for<'call> FnOnce(Out<'call, 'call, ParseFailure>) -> OResult<'call, T>,
) -> Result<T, ParseFailure> {
    match errors.try_call(operation) {
        Ok(value) => Ok(value),
        Err(caught) => Err(caught.take()),
    }
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
