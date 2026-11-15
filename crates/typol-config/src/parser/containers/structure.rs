use crate::{
    diagnostics::DiagnosticFactory,
    parser::{
        containers::{ContainerSupport, StructureSpans},
        context::{ParserContext, ParserControl, ValueCallContext},
        literals::{
            ContextDiagnosticRequest, ExpectedSymbol, ExpectedValueSymbol, ParsedValue, Syntax,
        },
        recovery::{ParseFailure, Recovery},
    },
    value::{FieldValue, ValueFactory},
    value_normalization::{MissingFieldDecision, MissingFieldValue},
};
use core::mem::replace;
use opeo::{ErrSlot, OResult, Out};
use typol_core::{ChargedVec, DiagnosticCode, LocatedSpan};
use typol_i18n::t;
use typol_semantic::{FieldList, FieldView, TypeView};
use typol_syntax::{Symbol, TokenKind};

#[derive(Clone, Copy, Eq, PartialEq)]
enum FieldSlotState {
    Unseen,
    Parsing,
    Present,
    Failed,
    Deferred,
}

struct FieldSlot {
    state: FieldSlotState,
    value: Option<FieldValue>,
}

impl FieldSlot {
    fn unseen() -> Self {
        Self {
            state: FieldSlotState::Unseen,
            value: None,
        }
    }
}

pub(crate) struct StructureFrame<'schema> {
    fields: FieldList<'schema>,
    slots: ChargedVec<FieldSlot>,
    pending: Option<FieldView<'schema>>,
    failed: bool,
    depth: usize,
}

impl<'schema> StructureFrame<'schema> {
    pub(crate) fn new<'slot>(
        context: &mut ParserContext<'schema, '_, '_>,
        fields: FieldList<'schema>,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, Self> {
        Syntax::expect_value_symbol(
            context,
            ExpectedValueSymbol {
                expected: Symbol::LeftBrace,
                syntax_message: t! {
                    name: "diagnostic.config.expected_struct_object",
                    locale: context.session.translator(),
                }
                .into(),
                value_message: t! {
                    name: "diagnostic.config.expected_struct_object",
                    locale: context.session.translator(),
                }
                .into(),
            },
            out.reborrow(),
        )?;
        Ok(Self {
            fields,
            slots: field_slots(fields.len(), context, out.reborrow())?,
            pending: None,
            failed: false,
            depth: context.cursor.depth(),
        })
    }

    pub(crate) fn next<'slot>(
        &mut self,
        context: &mut ParserContext<'schema, '_, '_>,
        item_errors: &mut ErrSlot<ParseFailure>,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, Option<TypeView<'schema>>> {
        loop {
            let token = context.cursor.peek(out.reborrow())?;
            if token.kind() == TokenKind::Symbol(Symbol::RightBrace) {
                break Ok(None);
            }
            let path_depth = context.path.len();
            let item = item_errors.try_call(|item_out| self.start_field(context, item_out));
            match item {
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
                        Symbol::RightBrace,
                        failure,
                        out.reborrow(),
                    )?;
                }
            }
        }
    }

    fn start_field<'slot>(
        &mut self,
        context: &mut ParserContext<'schema, '_, '_>,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, TypeView<'schema>> {
        let token = context.cursor.peek(out.reborrow())?;
        let span = token.span();
        if token.kind() != TokenKind::Identifier {
            let code = if token.kind() == TokenKind::String {
                DiagnosticCode::CONFIG_VALUE_ERROR
            } else {
                DiagnosticCode::CONFIG_SYNTAX_ERROR
            };
            let diagnostic = ContainerSupport::context_diagnostic(
                ContextDiagnosticRequest {
                    context,
                    code,
                    message: t! {
                        name: "diagnostic.config.object_field_name_expected",
                        locale: context.session.translator(),
                    }
                    .into(),
                    span,
                    value_source: None,
                },
                out.reborrow(),
            )?;
            return Err(out.fail(diagnostic.into()));
        }
        let name = token.text().unwrap_or("");

        let matched = ParserControl::budget_call(
            &mut context.session.validation().budget_errors,
            |budget_out| {
                self.fields
                    .named_budgeted(name, &context.budget, budget_out)
            },
            out.reborrow(),
        )?;
        let Some((index, field)) = matched else {
            let diagnostic = ContainerSupport::context_diagnostic(
                ContextDiagnosticRequest {
                    context,
                    code: DiagnosticCode::CONFIG_VALUE_ERROR,
                    message: t! {
                        name: "diagnostic.config.undeclared_object_field",
                        locale: context.session.translator(),
                    }
                    .into(),
                    span,
                    value_source: None,
                },
                out.reborrow(),
            )?;
            return Err(out.fail(diagnostic.into()));
        };
        let field_name = ParserControl::schema_call(
            &mut context.session.validation().schema_access_errors,
            |schema_out| field.name(schema_out),
            out.reborrow(),
        )?;
        ContainerSupport::push_field_path(context, field_name, span, out.reborrow())?;
        let Some(slot) = self.slots.get_mut(index) else {
            return Err(out.fail(ContainerSupport::invalid_parser_state(
                Some(span),
                context.session.translator(),
            )));
        };
        if slot.state != FieldSlotState::Unseen {
            let diagnostic = ContainerSupport::context_diagnostic(
                ContextDiagnosticRequest {
                    context,
                    code: DiagnosticCode::CONFIG_VALUE_ERROR,
                    message: t! {
                        name: "diagnostic.config.duplicate_object_field",
                        locale: context.session.translator(),
                    }
                    .into(),
                    span,
                    value_source: None,
                },
                out.reborrow(),
            )?;
            return Err(out.fail(diagnostic.into()));
        }
        slot.state = FieldSlotState::Failed;
        context.cursor.bump(out.reborrow())?;
        Syntax::expect_symbol(
            context,
            ExpectedSymbol {
                expected: Symbol::Colon,
                code: DiagnosticCode::CONFIG_SYNTAX_ERROR,
                message: t! {
                    name: "diagnostic.config.expected_struct_field_colon",
                    locale: context.session.translator(),
                }
                .into(),
            },
            out.reborrow(),
        )?;
        let Some(slot) = self.slots.get_mut(index) else {
            return Err(out.fail(ContainerSupport::invalid_parser_state(
                Some(span),
                context.session.translator(),
            )));
        };
        slot.state = FieldSlotState::Parsing;
        self.pending = Some(field);
        context.target_field = Some(field);
        context.next_member = ContainerSupport::field_member(context, field);
        ParserControl::schema_call(
            &mut context.session.validation().schema_access_errors,
            |schema_out| field.ty(schema_out),
            out.reborrow(),
        )
    }

    pub(crate) fn accept<'slot>(
        &mut self,
        context: &mut ParserContext<'schema, '_, '_>,
        result: Result<ParsedValue, ParseFailure>,
        item_errors: &mut ErrSlot<ParseFailure>,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, ()> {
        let span = ContainerSupport::current_span(context);
        let Some(field) = self.pending.take() else {
            return Err(out.fail(ContainerSupport::invalid_parser_state(
                span,
                context.session.translator(),
            )));
        };
        let index = ParserControl::schema_call(
            &mut context.session.validation().schema_access_errors,
            |schema_out| field.slot(schema_out),
            out.reborrow(),
        )?;
        let Some(slot) = self.slots.get_mut(index) else {
            return Err(out.fail(ContainerSupport::invalid_parser_state(
                span,
                context.session.translator(),
            )));
        };
        if slot.state != FieldSlotState::Parsing {
            return Err(out.fail(ContainerSupport::invalid_parser_state(
                span,
                context.session.translator(),
            )));
        }
        let item = item_errors.try_call(|mut item_out| {
            match result {
                Ok(parsed) => {
                    let name = ParserControl::schema_call(
                        &mut context.session.validation().schema_access_errors,
                        |schema_out| field.name(schema_out),
                        item_out.reborrow(),
                    )?;
                    ParserControl::construct_text(context, name.len(), item_out.reborrow())?;
                    let value = ParserControl::budget_call(
                        &mut context.session.validation().budget_errors,
                        |budget_out| {
                            FieldValue::try_new(
                                name,
                                parsed.value,
                                Some(&context.budget),
                                budget_out,
                            )
                        },
                        item_out.reborrow(),
                    )?;
                    let Some(slot) = self.slots.get_mut(index) else {
                        return Err(item_out.fail(ContainerSupport::invalid_parser_state(
                            span,
                            context.session.translator(),
                        )));
                    };
                    slot.state = FieldSlotState::Present;
                    slot.value = Some(value);
                }
                Err(failure) if failure.is_deferred_field() => {
                    let Some(slot) = self.slots.get_mut(index) else {
                        return Err(item_out.fail(ContainerSupport::invalid_parser_state(
                            span,
                            context.session.translator(),
                        )));
                    };
                    slot.state = FieldSlotState::Deferred;
                }
                Err(failure) => {
                    if let Some(slot) = self.slots.get_mut(index) {
                        slot.state = FieldSlotState::Failed;
                        slot.value = None;
                    }
                    return Err(item_out.fail(failure));
                }
            }
            ContainerSupport::item_boundary(context, Symbol::RightBrace, item_out.reborrow())
        });
        context.path.pop();
        let item = match item {
            Ok(()) => Ok(()),
            Err(caught) => Err(caught.take()),
        };
        if let Err(failure) = item {
            self.failed = true;
            Recovery::recover_item(
                context,
                self.depth,
                Symbol::RightBrace,
                failure,
                out.reborrow(),
            )?;
        }
        Ok(())
    }

    pub(crate) fn finish<'slot>(
        mut self,
        context: &mut ParserContext<'schema, '_, '_>,
        close_span: LocatedSpan,
        item_errors: &mut ErrSlot<ParseFailure>,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, ChargedVec<FieldValue>> {
        let mut output = ChargedVec::new(Some(&context.budget), out.reborrow())?;
        output.try_reserve_exact(self.fields.len(), out.reborrow())?;
        for (index, field) in self.fields.iter().enumerate() {
            context.budget.consume(1, out.reborrow())?;
            let Some(slot) = self.slots.get_mut(index) else {
                return Err(out.fail(ContainerSupport::invalid_parser_state(
                    Some(close_span),
                    context.session.translator(),
                )));
            };
            let state = replace(slot, FieldSlot::unseen());
            match state.state {
                FieldSlotState::Present => {
                    let Some(value) = state.value else {
                        return Err(out.fail(ContainerSupport::invalid_parser_state(
                            Some(close_span),
                            context.session.translator(),
                        )));
                    };
                    ParserControl::budget_call(
                        &mut context.session.validation().budget_errors,
                        |budget_out| output.try_push(value, budget_out),
                        out.reborrow(),
                    )?;
                    continue;
                }
                FieldSlotState::Failed => continue,
                FieldSlotState::Parsing => {
                    return Err(out.fail(ContainerSupport::invalid_parser_state(
                        Some(close_span),
                        context.session.translator(),
                    )));
                }
                FieldSlotState::Unseen | FieldSlotState::Deferred => {}
            }
            let field_name = ParserControl::schema_call(
                &mut context.session.validation().schema_access_errors,
                |schema_out| field.name(schema_out),
                out.reborrow(),
            )?;
            ContainerSupport::push_field_path(context, field_name, close_span, out.reborrow())?;
            context.next_member = ContainerSupport::field_member(context, field);
            let missing = item_errors.try_call(|mut item_out| {
                let default = ParserControl::schema_call(
                    &mut context.session.validation().schema_access_errors,
                    |schema_out| field.default_value(schema_out),
                    item_out.reborrow(),
                )?;
                let expected = ParserControl::schema_call(
                    &mut context.session.validation().schema_access_errors,
                    |schema_out| field.ty(schema_out),
                    item_out.reborrow(),
                )?;
                let kind = ParserControl::schema_call(
                    &mut context.session.validation().schema_access_errors,
                    |schema_out| expected.kind(schema_out),
                    item_out.reborrow(),
                )?;
                match MissingFieldValue::decide(false, default, kind) {
                    MissingFieldDecision::SchemaDefault(_) | MissingFieldDecision::OptionalNone => {
                        context.needs_normalization = true;
                        Ok(())
                    }
                    MissingFieldDecision::Required => {
                        let diagnostic = ContainerSupport::missing_field_diagnostic(
                            context,
                            close_span,
                            item_out.reborrow(),
                        )?;
                        Err(item_out.fail(diagnostic.into()))
                    }
                    MissingFieldDecision::Provided => {
                        Err(item_out.fail(ContainerSupport::invalid_parser_state(
                            Some(close_span),
                            context.session.translator(),
                        )))
                    }
                }
            });
            context.path.pop();
            if let Err(caught) = missing {
                self.failed = true;
                if !Recovery::record_failure(context, caught.take()) {
                    return Err(out.fail(ParseFailure::reported()));
                }
            }
        }
        if self.failed {
            return Err(out.fail(ParseFailure::reported()));
        }
        Ok(output)
    }

    pub(crate) fn finish_value<'slot>(
        self,
        context: &mut ParserContext<'schema, '_, '_>,
        spans: StructureSpans,
        item_errors: &mut ErrSlot<ParseFailure>,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, ParsedValue> {
        let StructureSpans {
            initial: initial_span,
            close: close_span,
        } = spans;
        let fields = self.finish(context, close_span, item_errors, out.reborrow())?;
        let value_source = context.value_source;
        let budget = &context.budget;
        let value = ContainerSupport::value_call(
            &mut context.value_errors,
            move |value_out| {
                ValueFactory::structure_owned(fields, value_source, Some(budget), value_out)
            },
            ValueCallContext {
                span: close_span,
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
                        initial_span,
                        close_span,
                        context.session.translator(),
                        diagnostic_out,
                    )
                },
                out.reborrow(),
            )?,
        })
    }
}

fn field_slots<'slot>(
    length: usize,
    context: &ParserContext<'_, '_, '_>,
    mut out: Out<'slot, '_, ParseFailure>,
) -> OResult<'slot, ChargedVec<FieldSlot>> {
    let mut slots: ChargedVec<FieldSlot> = ChargedVec::new(Some(&context.budget), out.reborrow())?;
    slots.try_reserve_exact(length, out.reborrow())?;
    slots.resize_with(length, FieldSlot::unseen, out.reborrow())?;
    Ok(slots)
}
