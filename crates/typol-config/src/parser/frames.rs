mod body;

use self::body::{Body, Start, ValueScope};
use crate::parser::{context::ParserContext, literals::ParsedValue, recovery::ParseFailure};
use opeo::{ErrSlot, OResult, Out};
use typol_core::ChargedVec;
use typol_semantic::TypeView;

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
                        return Err(out.fail(Body::invalid_parse_state(
                            Body::current_span(context),
                            context.session.translator(),
                        )));
                    };
                    let Some(frame) = frames.last_mut() else {
                        return match result {
                            Ok(value) => Ok(value),
                            Err(failure) => Err(out.fail(failure)),
                        };
                    };
                    context.target_field = frame.scope.field();
                    let mut failure = None;
                    if capture(
                        errors,
                        |phase_out| frame.body.accept(context, result, item_errors, phase_out),
                        &mut failure,
                    )
                    .is_some()
                    {
                        Self::advance(context, &mut frames, errors, item_errors, &mut state)
                    } else {
                        state.progress = Some(Err(failure_or_invalid(failure, context)));
                        Self::complete_frame(context, &mut frames, errors, item_errors, &mut state)
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
        let mut preparation_failure = None;
        let prepared = capture(
            errors,
            |mut phase_out| {
                let _ = context.cursor.peek(phase_out.reborrow())?;
                frames.try_reserve(1, phase_out.reborrow())?;
                Ok(())
            },
            &mut preparation_failure,
        );
        if prepared.is_none() {
            state.child = Some(Err(failure_or_invalid(preparation_failure, context)));
            return ParsePhase::Child;
        }

        let mut scope_failure = None;
        let scope = match capture(
            errors,
            |phase_out| ValueScope::enter(context, ty, phase_out),
            &mut scope_failure,
        ) {
            Some(scope) => scope,
            None => {
                state.child = Some(Err(failure_or_invalid(scope_failure, context)));
                return ParsePhase::Child;
            }
        };

        let mut start_failure = None;
        let start = capture(
            errors,
            |phase_out| Body::start(context, ty, phase_out),
            &mut start_failure,
        );
        match start {
            Some(Start::Value(parsed)) => {
                let mut complete_failure = None;
                state.child = Some(
                    match capture(
                        errors,
                        |phase_out| scope.complete(context, Ok(parsed), phase_out),
                        &mut complete_failure,
                    ) {
                        Some(parsed) => Ok(parsed),
                        None => Err(failure_or_invalid(complete_failure, context)),
                    },
                );
                ParsePhase::Child
            }
            Some(Start::Frame(body)) => {
                let mut push_failure = None;
                if capture(
                    errors,
                    |phase_out| frames.try_push(ValueFrame { scope, body }, phase_out),
                    &mut push_failure,
                )
                .is_some()
                {
                    ParsePhase::Resume
                } else {
                    state.child = Some(Err(failure_or_invalid(push_failure, context)));
                    ParsePhase::Child
                }
            }
            None => {
                let failure = failure_or_invalid(start_failure, context);
                let mut complete_failure = None;
                state.child = Some(
                    match capture(
                        errors,
                        |phase_out| scope.complete(context, Err(failure), phase_out),
                        &mut complete_failure,
                    ) {
                        Some(parsed) => Ok(parsed),
                        None => Err(failure_or_invalid(complete_failure, context)),
                    },
                );
                ParsePhase::Child
            }
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
            state.progress = Some(Err(Body::invalid_parse_state(
                Body::current_span(context),
                context.session.translator(),
            )));
            return ParsePhase::Child;
        };
        context.target_field = frame.scope.field();
        let mut failure = None;
        match capture(
            errors,
            |phase_out| frame.body.next(context, item_errors, phase_out),
            &mut failure,
        ) {
            Some(Some(expected)) => ParsePhase::Enter(expected),
            Some(None) => {
                state.progress = Some(Ok(()));
                Self::complete_frame(context, frames, errors, item_errors, state)
            }
            None => {
                state.progress = Some(Err(failure_or_invalid(failure, context)));
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
            state.child = Some(Err(Body::invalid_parse_state(
                Body::current_span(context),
                context.session.translator(),
            )));
            state.progress = None;
            return ParsePhase::Child;
        };
        match state.progress.take() {
            Some(Ok(())) => Self::finish_frame(context, frame, errors, item_errors, state),
            Some(Err(failure)) => {
                Self::complete_scope(context, frame.scope, Err(failure), errors, state)
            }
            None => Self::complete_scope(
                context,
                frame.scope,
                Err(Body::invalid_parse_state(
                    Body::current_span(context),
                    context.session.translator(),
                )),
                errors,
                state,
            ),
        }
    }

    fn finish_frame<'schema>(
        context: &mut ParserContext<'schema, '_, '_>,
        frame: ValueFrame<'schema>,
        errors: &mut ErrSlot<ParseFailure>,
        item_errors: &mut ErrSlot<ParseFailure>,
        state: &mut ParseState,
    ) -> ParsePhase<'schema> {
        let ValueFrame { scope, body } = frame;
        let mut failure = None;
        let parsed = match capture(
            errors,
            |phase_out| body.finish(context, item_errors, phase_out),
            &mut failure,
        ) {
            Some(parsed) => Ok(parsed),
            None => Err(failure_or_invalid(failure, context)),
        };
        Self::complete_scope(context, scope, parsed, errors, state)
    }

    fn complete_scope<'schema>(
        context: &mut ParserContext<'schema, '_, '_>,
        scope: ValueScope<'schema>,
        parsed: Result<ParsedValue, ParseFailure>,
        errors: &mut ErrSlot<ParseFailure>,
        state: &mut ParseState,
    ) -> ParsePhase<'schema> {
        let mut failure = None;
        state.child = Some(
            match capture(
                errors,
                |phase_out| scope.complete(context, parsed, phase_out),
                &mut failure,
            ) {
                Some(parsed) => Ok(parsed),
                None => Err(failure_or_invalid(failure, context)),
            },
        );
        ParsePhase::Child
    }
}

fn failure_or_invalid(
    failure: Option<ParseFailure>,
    context: &ParserContext<'_, '_, '_>,
) -> ParseFailure {
    match failure {
        Some(failure) => failure,
        None => {
            Body::invalid_parse_state(Body::current_span(context), context.session.translator())
        }
    }
}

fn capture<T>(
    errors: &mut ErrSlot<ParseFailure>,
    operation: impl for<'call> FnOnce(Out<'call, 'call, ParseFailure>) -> OResult<'call, T>,
    failure: &mut Option<ParseFailure>,
) -> Option<T> {
    *failure = None;
    match errors.try_call(operation) {
        Ok(value) => Some(value),
        Err(caught) => {
            *failure = Some(caught.take());
            None
        }
    }
}
