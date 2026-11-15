use crate::diagnostics::{BudgetDiagnostic, DetailedDiagnosticRequest, DiagnosticFactory};
use opeo::{ErrSlot, OResult, Out};
use typol_core::{
    BudgetError, ChargedVec, ConfigValue, Diagnostic, DiagnosticCode, DiagnosticContext,
    ExecutionBudget, InstancePath, InternalError, LocatedSpan, ValueEntries, ValueError,
    ValueErrorKind, ValueSource, ValueView, diagnostic_context,
};
use typol_i18n::{Translator, t};
use typol_policy::ValueLimits;
use typol_semantic::{FieldList, FieldView, SchemaAccessError, TypeView};

pub(super) static NORMALIZE_MAP_CHILD_ROLE: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "value-normalization",
    "map-child-role-mismatch",
);
pub(super) static NORMALIZE_FIELD_SLOT: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "value-normalization",
    "field-slot-out-of-bounds",
);
pub(super) static NORMALIZE_FINISH_INVARIANT: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "value-normalization",
    "normalized-child-layout-mismatch",
);
pub(super) static NORMALIZE_TAKE_SINGLE: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "value-normalization",
    "single-value-count-mismatch",
);

pub(crate) struct NormalizeRequest<'a> {
    pub(crate) translator: Translator,
    pub(crate) expected: TypeView<'a>,
    pub(crate) value: ValueView<'a>,
    pub(crate) depth: usize,
    pub(crate) primary: Option<LocatedSpan>,
    pub(crate) text_limit: usize,
    pub(crate) schema_default: bool,
    pub(crate) allow_unvalidated_environment_sources: bool,
    pub(crate) existing_nodes_precounted: bool,
    pub(crate) missing_value_source: ValueSource,
}

pub(super) struct NormalizeBudgetErrors {
    pub(super) slot: ErrSlot<BudgetError>,
    pub(super) translator: Translator,
}

pub(super) struct NormalizeSchemaErrors {
    pub(super) slot: ErrSlot<SchemaAccessError>,
    pub(super) translator: Translator,
}

pub(super) struct NormalizeValueErrors {
    pub(super) slot: ErrSlot<ValueError>,
    pub(super) translator: Translator,
}

pub(super) struct NormalizeErrors {
    pub(super) budget: NormalizeBudgetErrors,
    pub(super) schema: NormalizeSchemaErrors,
    pub(super) value: NormalizeValueErrors,
}

pub(super) struct NormalizeState<'a, 'work> {
    pub(super) request: NormalizeRequest<'a>,
    pub(super) nodes: &'work mut usize,
    pub(super) limits: &'work ValueLimits,
    pub(super) budget: &'work ExecutionBudget,
    pub(super) errors: NormalizeErrors,
}

#[derive(Clone, Copy)]
pub(super) struct Input<'a> {
    pub(super) ty: TypeView<'a>,
    pub(super) value: Option<ValueView<'a>>,
    pub(super) field: Option<FieldView<'a>>,
    pub(super) hidden: bool,
    pub(super) schema_default: bool,
    pub(super) environment_root: bool,
}

#[derive(Clone, Copy)]
pub(super) enum ChildRole<'a> {
    Field(FieldView<'a>),
    VariantField {
        variant: &'a str,
        field: FieldView<'a>,
    },
    ArrayItem(usize),
    MapItem(&'a str),
    OptionalContent,
    EnumPayload(&'a str),
}

#[derive(Clone, Copy)]
pub(super) struct PlanChild<'a> {
    pub(super) input: Input<'a>,
    pub(super) role: ChildRole<'a>,
}

#[derive(Clone, Copy)]
pub(super) enum PlanKind<'a> {
    MissingOptional,
    Scalar,
    OptionalNone,
    OptionalSome,
    Array,
    Map,
    Structure,
    EnumUnit(&'a str),
    EnumSingle(&'a str),
    EnumStructure(&'a str),
}

pub(super) struct Plan<'a> {
    pub(super) input: Input<'a>,
    pub(super) kind: PlanKind<'a>,
    pub(super) children: ChargedVec<PlanChild<'a>>,
}

pub(super) struct FieldsRequest<'a, 'work> {
    pub(super) input: Input<'a>,
    pub(super) fields: FieldList<'a>,
    pub(super) values: ValueEntries<'a>,
    pub(super) output: &'work mut ChargedVec<PlanChild<'a>>,
    pub(super) path: &'work InstancePath,
}

pub(super) enum Task<'a> {
    Enter(Input<'a>, usize),
    Child(PlanChild<'a>, usize),
    Finish(Plan<'a>, usize),
    Restore(usize),
}

pub(super) struct NormalizeOps;

impl NormalizeOps {
    pub(super) fn push<'slot, 'borrow, T>(
        values: &mut ChargedVec<T>,
        errors: &mut NormalizeBudgetErrors,
        value: T,
        out: Out<'slot, 'borrow, Diagnostic>,
    ) -> OResult<'slot, ()> {
        Self::budget_call(errors, |budget_out| values.try_push(value, budget_out), out)
    }

    pub(super) fn budget_call<'slot, 'borrow, T>(
        errors: &mut NormalizeBudgetErrors,
        operation: impl for<'call> FnOnce(Out<'call, 'call, BudgetError>) -> OResult<'call, T>,
        out: Out<'slot, 'borrow, Diagnostic>,
    ) -> OResult<'slot, T> {
        match errors.slot.try_call(operation) {
            Ok(value) => Ok(value),
            Err(caught) => Err(out.fail(BudgetDiagnostic::from_error(
                caught.take(),
                errors.translator,
            ))),
        }
    }

    pub(super) fn schema_call<'slot, 'borrow, T>(
        errors: &mut NormalizeSchemaErrors,
        operation: impl for<'call> FnOnce(Out<'call, 'call, SchemaAccessError>) -> OResult<'call, T>,
        out: Out<'slot, 'borrow, Diagnostic>,
    ) -> OResult<'slot, T> {
        match errors.slot.try_call(operation) {
            Ok(value) => Ok(value),
            Err(caught) => Err(out.fail(DiagnosticFactory::schema_access(
                caught.take(),
                errors.translator,
            ))),
        }
    }

    pub(super) fn value_call<'slot, 'borrow, T>(
        errors: &mut NormalizeValueErrors,
        operation: impl for<'call> FnOnce(Out<'call, 'call, ValueError>) -> OResult<'call, T>,
        out: Out<'slot, 'borrow, Diagnostic>,
    ) -> OResult<'slot, T> {
        match errors.slot.try_call(operation) {
            Ok(value) => Ok(value),
            Err(caught) => Err(out.fail(Self::container_error(caught.take(), errors.translator))),
        }
    }

    pub(super) fn reserve<'slot, T>(
        values: &mut ChargedVec<T>,
        errors: &mut NormalizeErrors,
        count: usize,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ()> {
        Self::budget_call(
            &mut errors.budget,
            |budget_out| values.try_reserve(count, budget_out),
            out.reborrow(),
        )?;
        Ok(())
    }

    pub(super) fn take_single<'slot, 'borrow>(
        values: &mut impl Iterator<Item = ConfigValue>,
        translator: Translator,
        out: Out<'slot, 'borrow, Diagnostic>,
    ) -> OResult<'slot, ConfigValue> {
        let Some(value) = values.next() else {
            return Err(out.fail(BudgetDiagnostic::from_error(
                BudgetError::Internal(InternalError::ConfigValueInvariantFailure(
                    NORMALIZE_TAKE_SINGLE,
                )),
                translator,
            )));
        };
        if values.next().is_some() {
            return Err(out.fail(BudgetDiagnostic::from_error(
                BudgetError::Internal(InternalError::ConfigValueInvariantFailure(
                    NORMALIZE_TAKE_SINGLE,
                )),
                translator,
            )));
        }
        Ok(value)
    }

    pub(super) fn invariant_error(translator: Translator) -> Diagnostic {
        BudgetDiagnostic::from_error(
            BudgetError::Internal(InternalError::ConfigValueInvariantFailure(
                NORMALIZE_FINISH_INVARIANT,
            )),
            translator,
        )
    }

    fn container_error(error: ValueError, translator: Translator) -> Diagnostic {
        match error.kind() {
            ValueErrorKind::AllocationFailed => {
                BudgetDiagnostic::from_error(BudgetError::Allocation, translator)
            }
            ValueErrorKind::ResourceLimit(limit) => {
                BudgetDiagnostic::from_error(BudgetError::ResourceLimit(limit), translator)
            }
            ValueErrorKind::CapacityExceeded(capacity) => {
                BudgetDiagnostic::from_error(BudgetError::CapacityExceeded(capacity), translator)
            }
            ValueErrorKind::Internal(internal) => DiagnosticFactory::internal_error(
                internal,
                None,
                None,
                None,
                translator,
            ),
            ValueErrorKind::EnvironmentReferenceRequired => {
                DiagnosticFactory::make_diagnostic(DetailedDiagnosticRequest {
                    code: DiagnosticCode::CONFIG_VALUE_ERROR,
                    message: t! { name: "diagnostic.config.environment_reference_metadata_required", locale: translator }.into(),
                    primary: None,
                    path: None,
                    value_source: None,
                    context: None,
                    translator,
                })
            }
            ValueErrorKind::DuplicateFieldName => {
                DiagnosticFactory::make_diagnostic(DetailedDiagnosticRequest {
                    code: DiagnosticCode::CONFIG_VALUE_ERROR,
                    message: t! { name: "diagnostic.config.duplicate_object_field", locale: translator }.into(),
                    primary: None,
                    path: None,
                    value_source: None,
                    context: None,
                    translator,
                })
            }
            ValueErrorKind::DuplicateMapKey => {
                DiagnosticFactory::make_diagnostic(DetailedDiagnosticRequest {
                    code: DiagnosticCode::CONFIG_VALUE_ERROR,
                    message: t! { name: "diagnostic.config.duplicate_map_key", locale: translator }.into(),
                    primary: None,
                    path: None,
                    value_source: None,
                    context: None,
                    translator,
                })
            }
        }
    }
}
