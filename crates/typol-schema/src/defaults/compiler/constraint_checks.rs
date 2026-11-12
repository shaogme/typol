use crate::{
    annotations::{CompiledConstraint, ConstraintData},
    context::CompileContext,
    defaults::{ConstraintLabelInput, DefaultDiagnostic},
    diagnostics::{BuildFailure, DiagnosticCollector},
    symbols::{LookupResult, SymbolTable},
    types::{ResolvedTypeGraph, ResolvedTypeKind, TypeUseId},
};
use core::cell::RefCell;
use opeo::{ErrSlot, OResult, Out};
use typol_core::{ConfigValue, SourceSpan, ValueKind};
use typol_i18n::diagnostic_message;
use typol_semantic::{EvalError, StoredRuleView, ValueRules, VariantId, VariantRestrictionMode};

pub(super) struct VariantRestrictionCheck<'a, 'source> {
    pub(super) context: &'a CompileContext,
    pub(super) diagnostics: &'a mut DiagnosticCollector,
    pub(super) symbols: &'a SymbolTable<'source>,
    pub(super) types: &'a ResolvedTypeGraph,
    pub(super) target: TypeUseId,
    pub(super) value: &'a ConfigValue,
    pub(super) span: SourceSpan,
    pub(super) mode: VariantRestrictionMode,
    pub(super) variants: &'a [VariantId],
}

pub(super) struct VariantRestrictionDiagnostic<'a> {
    pub(super) context: &'a CompileContext,
    pub(super) diagnostics: &'a mut DiagnosticCollector,
    pub(super) value_span: SourceSpan,
    pub(super) constraint_span: SourceSpan,
}

pub(super) struct ScalarConstraintCheck<'a> {
    pub(super) context: &'a CompileContext,
    pub(super) diagnostics: &'a mut DiagnosticCollector,
    pub(super) evaluation_errors: &'a RefCell<ErrSlot<EvalError>>,
    pub(super) value: &'a ConfigValue,
    pub(super) value_span: SourceSpan,
    pub(super) constraint: &'a CompiledConstraint,
}

pub(super) struct DefaultVariantRestriction;

pub(super) struct DefaultScalarConstraint;

impl DefaultScalarConstraint {
    pub(super) fn matches<'slot>(
        input: ScalarConstraintCheck<'_>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, bool> {
        let mut leaf = input.value.root();
        while let ValueKind::Optional(inner) = leaf.as_ref() {
            input
                .context
                .consume(1, input.value_span, input.diagnostics, out.reborrow())?;
            let Some(inner) = inner else {
                return Ok(true);
            };
            leaf = inner;
        }
        let data = match *input.constraint.data() {
            ConstraintData::NumberBounds { min, max } => StoredRuleView::NumberBounds { min, max },
            ConstraintData::LengthBounds { min, max } => StoredRuleView::LengthBounds { min, max },
            ConstraintData::SizeBounds { min, max } => StoredRuleView::SizeBounds { min, max },
            ConstraintData::Host => StoredRuleView::Host,
            ConstraintData::Email => StoredRuleView::Email,
            ConstraintData::Cidr { v4_only } => StoredRuleView::Cidr { v4_only },
            _ => {
                input.diagnostics.push(schema_internal!(input.value_span));
                return Err(out.fail(BuildFailure::empty()));
            }
        };
        let result = {
            let execution = input.context.execution();
            let mut errors = input.evaluation_errors.borrow_mut();
            match errors
                .try_call(|eval_out| ValueRules::scalar(leaf, data, Some(execution), eval_out))
            {
                Ok(value) => Ok(value),
                Err(caught) => Err(caught.take()),
            }
        };
        let Some(matched) = (match result {
            Ok(value) => value,
            Err(error) => {
                let diagnostic =
                    DefaultDiagnostic::execution(error, input.value_span, input.diagnostics);
                input.diagnostics.push(diagnostic);
                return Err(out.fail(BuildFailure::empty()));
            }
        }) else {
            input.diagnostics.push(schema_internal!(input.value_span));
            return Err(out.fail(BuildFailure::empty()));
        };
        Ok(matched)
    }
}

impl DefaultVariantRestriction {
    pub(super) fn matches<'slot>(
        input: VariantRestrictionCheck<'_, '_>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, bool> {
        let mut leaf = input.value.root();
        loop {
            match leaf.as_ref() {
                ValueKind::Optional(Some(inner)) => {
                    input
                        .context
                        .consume(1, input.span, input.diagnostics, out.reborrow())?;
                    leaf = inner;
                }
                ValueKind::Optional(None) => return Ok(true),
                ValueKind::Enumeration { variant, .. } => {
                    let resolved = input
                        .types
                        .type_use(input.target)
                        .map(|type_use| type_use.kind());
                    let resolved = match resolved {
                        Some(ResolvedTypeKind::Option(inner)) => {
                            input.types.type_use(inner).map(|type_use| type_use.kind())
                        }
                        kind => kind,
                    };
                    let Some(ResolvedTypeKind::User(enum_symbol)) = resolved else {
                        input.diagnostics.push(schema_internal!(input.span));
                        return Err(out.fail(BuildFailure::empty()));
                    };
                    let selected = match input.symbols.lookup_variant::<BuildFailure>(
                        enum_symbol,
                        variant,
                        out.reborrow(),
                    )? {
                        LookupResult::Found(selected) => selected,
                        LookupResult::Missing | LookupResult::Ambiguous => {
                            input.diagnostics.push(schema_internal!(input.span));
                            return Err(out.fail(BuildFailure::empty()));
                        }
                    };
                    input.context.consume(
                        input.variants.len(),
                        input.span,
                        input.diagnostics,
                        out.reborrow(),
                    )?;
                    let selected_allowed = input
                        .variants
                        .binary_search_by_key(&selected.index(), |id| id.index())
                        .is_ok();
                    return Ok(match input.mode {
                        VariantRestrictionMode::Allow => selected_allowed,
                        VariantRestrictionMode::Deny => !selected_allowed,
                    });
                }
                _ => {
                    input.diagnostics.push(schema_internal!(input.span));
                    return Err(out.fail(BuildFailure::empty()));
                }
            }
        }
    }

    pub(super) fn report_rejection<'slot>(
        input: VariantRestrictionDiagnostic<'_>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ()> {
        let mut diagnostic = DefaultDiagnostic::semantic(
            diagnostic_message!(
                "diagnostic.schema.annotation_variants_value_rejected",
                "schema default selects an enum variant rejected by @variants"
            ),
            input.value_span,
            input.context,
            input.diagnostics,
            out.reborrow(),
        )?;
        DefaultDiagnostic::constraint_label(
            ConstraintLabelInput {
                diagnostic: &mut diagnostic,
                constraint: input.constraint_span,
                fallback: input.value_span,
                context: input.context,
                sink: input.diagnostics,
            },
            out.reborrow(),
        )?;
        input.diagnostics.push(diagnostic);
        Ok(())
    }
}
