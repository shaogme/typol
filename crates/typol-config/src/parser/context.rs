use super::{
    literals::LiteralParser,
    recovery::{ParseFailure, RecoveryCursor, RecoveryCursorRequest},
};
use crate::{
    ConfigLimits,
    diagnostics::{BudgetDiagnostic, DiagnosticBuffer, DiagnosticFactory},
    instance::{LocationIndex, LocationNode, Member, NodeId},
    session::ConfigSession,
};
use core::{cell::RefCell, mem::replace};
use opeo::{ErrSlot, OResult, Out};
use typol_core::{
    BudgetError, ChargedVec, Diagnostic, DiagnosticCode, ExecutionBudget, InstancePath,
    LocatedSpan, Read, SourceId, ValueError, ValueSource,
};
use typol_semantic::{
    CompiledSchema, ConstraintList, FieldView, SchemaAccessError, TypeView, VariantView,
};

#[cfg(test)]
use typol_semantic::SpanLocation;

pub(super) struct ParserContext<'schema, 'source, 'limits> {
    #[cfg(test)]
    pub(super) default_work_peak: usize,
    #[cfg(test)]
    pub(super) subtree_shape_skips: usize,
    pub(super) budget: ExecutionBudget,
    pub(super) config_limits: ConfigLimits,
    pub(super) session: ConfigSession<'limits>,
    pub(super) locations: LocationIndex<'schema>,
    pub(super) current_node: Option<NodeId>,
    pub(super) next_member: Member<'schema>,
    pub(super) inherited_constraints: ChargedVec<ConstraintList<'schema>>,
    pub(super) target_variant: Option<VariantView<'schema>>,
    pub(super) assignment_variant: Option<VariantView<'schema>>,
    pub(super) target_field: Option<FieldView<'schema>>,
    pub(super) expected_type: TypeView<'schema>,
    pub(super) schema: &'schema CompiledSchema,
    pub(super) source_id: SourceId,
    pub(super) cursor: RecoveryCursor<'source>,
    pub(super) limits: &'limits ConfigLimits,
    pub(super) path: InstancePath,
    pub(super) value_source: ValueSource,
    pub(super) nesting_depth: usize,
    pub(super) value_nodes: usize,
    pub(super) diagnostics: DiagnosticBuffer<'schema>,
    pub(super) diagnostic_errors: RefCell<ErrSlot<Diagnostic>>,
    pub(super) tree_diagnostic_errors: ErrSlot<Diagnostic>,
    pub(super) value_errors: ErrSlot<ValueError>,
}

pub(super) struct ParserControl;

pub(super) struct ParserControlRequest<'schema, 'source, 'limits> {
    pub(super) schema: &'schema CompiledSchema,
    pub(super) source: &'source mut dyn Read<Diagnostic>,
    pub(super) source_id: SourceId,
    pub(super) limits: &'limits ConfigLimits,
    pub(super) session: ConfigSession<'limits>,
}

impl ParserControl {
    pub(super) fn create<'schema, 'source, 'limits, 'slot>(
        request: ParserControlRequest<'schema, 'source, 'limits>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ParserContext<'schema, 'source, 'limits>> {
        let ParserControlRequest {
            schema,
            source,
            source_id,
            limits,
            mut session,
        } = request;
        let budget = session.budget().try_clone(out.reborrow())?;
        let config_limits = session.config_limits();
        let diagnostics = DiagnosticBuffer::new_budgeted(
            schema,
            limits.max_diagnostics(),
            &budget,
            out.reborrow(),
        )?;
        let cursor = RecoveryCursor::new(
            RecoveryCursorRequest {
                source_id,
                source,
                limits,
                stream: session.stream(),
                budget: &budget,
            },
            out.reborrow(),
        )?;

        let owned_budget = budget.try_clone(out.reborrow())?;
        let locations = LocationIndex::new_budgeted(&budget, out.reborrow())?;
        let inherited_constraints = ChargedVec::new(Some(&budget), out.reborrow())?;
        let path = match session
            .validation()
            .budget_errors
            .try_call(|budget_out| InstancePath::new(Some(&budget), budget_out))
        {
            Ok(path) => path,
            Err(caught) => {
                return Err(out.fail(BudgetDiagnostic::from_error(caught.take())));
            }
        };
        Ok(ParserContext {
            #[cfg(test)]
            default_work_peak: 0,
            #[cfg(test)]
            subtree_shape_skips: 0,
            budget: owned_budget,
            config_limits,
            session,
            locations,
            current_node: None,
            next_member: Member::Root,
            inherited_constraints,
            target_variant: None,
            assignment_variant: None,
            target_field: None,
            expected_type: schema.entry(),
            schema,
            source_id,
            cursor,
            limits,
            path,
            value_source: ValueSource::ConfigLiteral,
            nesting_depth: 0,
            value_nodes: 0,
            diagnostics,
            diagnostic_errors: RefCell::new(ErrSlot::new()),
            tree_diagnostic_errors: ErrSlot::new(),
            value_errors: ErrSlot::new(),
        })
    }

    pub(super) fn take_inherited<'schema, 'source, 'limits, 'slot>(
        context: &mut ParserContext<'schema, 'source, 'limits>,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, ChargedVec<ConstraintList<'schema>>> {
        let empty = Self::budget_call(
            &mut context.session.validation().budget_errors,
            |budget_out| ChargedVec::new(Some(&context.budget), budget_out),
            out.reborrow(),
        )?;
        Ok(replace(&mut context.inherited_constraints, empty))
    }

    pub(super) fn construct_text<'schema, 'source, 'limits, 'slot>(
        context: &mut ParserContext<'schema, 'source, 'limits>,
        bytes: usize,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, ()> {
        Self::budget_call(
            &mut context.session.validation().budget_errors,
            |budget_out| {
                context.budget.construct_text(
                    bytes,
                    context.config_limits.max_constructed_text_bytes(),
                    budget_out,
                )
            },
            out.reborrow(),
        )?;
        Ok(())
    }

    pub(super) fn begin_location<'schema, 'source, 'limits, 'slot>(
        context: &mut ParserContext<'schema, 'source, 'limits>,
        expected: TypeView<'schema>,
        span: LocatedSpan,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, NodeId> {
        Self::diagnostic_call(
            &context.diagnostic_errors,
            |diagnostic_out| {
                context.locations.begin(
                    LocationNode::new(
                        context.current_node,
                        context.next_member,
                        span,
                        expected,
                        context.path.len(),
                    ),
                    context.value_nodes,
                    diagnostic_out,
                )
            },
            out.reborrow(),
        )
    }

    #[cfg(test)]
    pub(super) fn span_location<'schema, 'source, 'limits>(
        context: &ParserContext<'schema, 'source, 'limits>,
        span: LocatedSpan,
    ) -> Option<SpanLocation> {
        if span.source_id() == context.source_id {
            span.location()
        } else {
            context.schema.span_location(span.source_span())
        }
    }

    pub(super) fn count_value_node<'schema, 'source, 'limits, 'slot>(
        context: &mut ParserContext<'schema, 'source, 'limits>,
        span: LocatedSpan,
        value_source: Option<ValueSource>,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, ()> {
        context.budget.consume(1, out.reborrow())?;
        Self::check_depth(
            context,
            context.nesting_depth,
            span,
            value_source,
            out.reborrow(),
        )?;
        let Some(next) = context.value_nodes.checked_add(1) else {
            let diagnostic = Self::diagnostic_call(
                &context.diagnostic_errors,
                |diagnostic_out| {
                    ParserControl::resource_diagnostic(
                        context,
                        "configuration value node count exceeded the limit",
                        span,
                        value_source,
                        diagnostic_out,
                    )
                },
                out.reborrow(),
            )?;
            return Err(out.fail(diagnostic.into()));
        };
        if next > context.limits.max_value_nodes() {
            let diagnostic = Self::diagnostic_call(
                &context.diagnostic_errors,
                |diagnostic_out| {
                    ParserControl::resource_diagnostic(
                        context,
                        "configuration value node count exceeded the limit",
                        span,
                        value_source,
                        diagnostic_out,
                    )
                },
                out.reborrow(),
            )?;
            return Err(out.fail(diagnostic.into()));
        }
        context.value_nodes = next;
        Ok(())
    }

    pub(super) fn check_depth<'schema, 'source, 'limits, 'slot>(
        context: &ParserContext<'schema, 'source, 'limits>,
        depth: usize,
        span: LocatedSpan,
        value_source: Option<ValueSource>,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, ()> {
        if context.limits.values().allows_depth(depth) {
            return Ok(());
        }
        let diagnostic = Self::diagnostic_call(
            &context.diagnostic_errors,
            |diagnostic_out| {
                ParserControl::resource_diagnostic(
                    context,
                    "configuration nesting depth exceeded the limit",
                    span,
                    value_source,
                    diagnostic_out,
                )
            },
            out.reborrow(),
        )?;
        Err(out.fail(diagnostic.into()))
    }

    pub(super) fn enter_nesting<'schema, 'source, 'limits, 'slot>(
        context: &mut ParserContext<'schema, 'source, 'limits>,
        span: LocatedSpan,
        value_source: Option<ValueSource>,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, ()> {
        let Some(next) = context.nesting_depth.checked_add(1) else {
            let diagnostic = Self::diagnostic_call(
                &context.diagnostic_errors,
                |diagnostic_out| {
                    ParserControl::resource_diagnostic(
                        context,
                        "configuration nesting depth exceeded the limit",
                        span,
                        value_source,
                        diagnostic_out,
                    )
                },
                out.reborrow(),
            )?;
            return Err(out.fail(diagnostic.into()));
        };
        context.nesting_depth = next;
        Ok(())
    }

    pub(super) fn leave_nesting<'schema, 'source, 'limits>(
        context: &mut ParserContext<'schema, 'source, 'limits>,
    ) {
        context.nesting_depth -= 1;
    }

    fn resource_diagnostic<'schema, 'source, 'limits, 'slot>(
        context: &ParserContext<'schema, 'source, 'limits>,
        message: &str,
        span: LocatedSpan,
        value_source: Option<ValueSource>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Diagnostic> {
        let path = LiteralParser::copy_path_with_source(
            context,
            Some(span),
            value_source,
            out.reborrow(),
        )?;
        Ok(DiagnosticFactory::make_diagnostic(
            DiagnosticCode::RESOURCE_LIMIT_EXCEEDED,
            message,
            Some(span),
            Some(path),
            value_source,
        ))
    }
}

impl ParserControl {
    pub(super) fn diagnostic_call<'slot, 'borrow, T>(
        errors: &RefCell<ErrSlot<Diagnostic>>,
        operation: impl for<'call> FnOnce(Out<'call, 'call, Diagnostic>) -> OResult<'call, T>,
        out: Out<'slot, 'borrow, ParseFailure>,
    ) -> OResult<'slot, T> {
        match errors.borrow_mut().try_call(operation) {
            Ok(value) => Ok(value),
            Err(caught) => Err(out.fail(caught.take().into())),
        }
    }

    pub(super) fn budget_call<'slot, 'borrow, T>(
        errors: &mut ErrSlot<BudgetError>,
        operation: impl for<'call> FnOnce(Out<'call, 'call, BudgetError>) -> OResult<'call, T>,
        out: Out<'slot, 'borrow, ParseFailure>,
    ) -> OResult<'slot, T> {
        match errors.try_call(operation) {
            Ok(value) => Ok(value),
            Err(caught) => Err(out.fail(ParseFailure::from(caught.take()))),
        }
    }

    pub(super) fn schema_call<'slot, 'borrow, T>(
        errors: &mut ErrSlot<SchemaAccessError>,
        operation: impl for<'call> FnOnce(Out<'call, 'call, SchemaAccessError>) -> OResult<'call, T>,
        out: Out<'slot, 'borrow, ParseFailure>,
    ) -> OResult<'slot, T> {
        match errors.try_call(operation) {
            Ok(value) => Ok(value),
            Err(caught) => Err(out.fail(ParseFailure::from(caught.take()))),
        }
    }
}
