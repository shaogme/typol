use crate::diagnostics::{BudgetDiagnostic, DiagnosticFactory};
use opeo::{ErrSlot, OResult, Out};
use typol_core::{
    BudgetError, ChargedVec, ConfigValue, Diagnostic, DiagnosticCode, DiagnosticContext,
    ExecutionBudget, InstancePath, LocatedSpan, ValueEntries, ValueError, ValueErrorKind,
    ValueView, diagnostic_context,
};
use typol_i18n::diagnostic_message;
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
pub(super) static NORMALIZE_VALUE_CONSTRUCTION: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "value-normalization",
    "value-container-construction-failed",
);

pub(crate) struct NormalizeRequest<'a> {
    pub(crate) expected: TypeView<'a>,
    pub(crate) value: ValueView<'a>,
    pub(crate) depth: usize,
    pub(crate) primary: Option<LocatedSpan>,
    pub(crate) text_limit: usize,
    pub(crate) schema_default: bool,
}

pub(super) struct NormalizeErrors {
    pub(super) budget: ErrSlot<BudgetError>,
    pub(super) schema: ErrSlot<SchemaAccessError>,
    pub(super) value: ErrSlot<ValueError>,
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
        errors: &mut ErrSlot<BudgetError>,
        value: T,
        out: Out<'slot, 'borrow, Diagnostic>,
    ) -> OResult<'slot, ()> {
        Self::budget_call(errors, |budget_out| values.try_push(value, budget_out), out)
    }

    pub(super) fn budget_call<'slot, 'borrow, T>(
        errors: &mut ErrSlot<BudgetError>,
        operation: impl for<'call> FnOnce(Out<'call, 'call, BudgetError>) -> OResult<'call, T>,
        out: Out<'slot, 'borrow, Diagnostic>,
    ) -> OResult<'slot, T> {
        match errors.try_call(operation) {
            Ok(value) => Ok(value),
            Err(caught) => Err(out.fail(BudgetDiagnostic::from_error(caught.take()))),
        }
    }

    pub(super) fn schema_call<'slot, 'borrow, T>(
        errors: &mut ErrSlot<SchemaAccessError>,
        operation: impl for<'call> FnOnce(Out<'call, 'call, SchemaAccessError>) -> OResult<'call, T>,
        out: Out<'slot, 'borrow, Diagnostic>,
    ) -> OResult<'slot, T> {
        match errors.try_call(operation) {
            Ok(value) => Ok(value),
            Err(caught) => Err(out.fail(DiagnosticFactory::schema_access(caught.take()))),
        }
    }

    pub(super) fn value_call<'slot, 'borrow, T>(
        errors: &mut ErrSlot<ValueError>,
        operation: impl for<'call> FnOnce(Out<'call, 'call, ValueError>) -> OResult<'call, T>,
        out: Out<'slot, 'borrow, Diagnostic>,
    ) -> OResult<'slot, T> {
        match errors.try_call(operation) {
            Ok(value) => Ok(value),
            Err(caught) => Err(out.fail(Self::container_error(caught.take()))),
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
        out: Out<'slot, 'borrow, Diagnostic>,
    ) -> OResult<'slot, ConfigValue> {
        let Some(value) = values.next() else {
            return Err(
                out.fail(BudgetDiagnostic::from_error(BudgetError::invariant(
                    &NORMALIZE_TAKE_SINGLE,
                ))),
            );
        };
        if values.next().is_some() {
            return Err(
                out.fail(BudgetDiagnostic::from_error(BudgetError::invariant(
                    &NORMALIZE_TAKE_SINGLE,
                ))),
            );
        }
        Ok(value)
    }

    pub(super) fn invariant_error() -> Diagnostic {
        BudgetDiagnostic::from_error(BudgetError::invariant(&NORMALIZE_FINISH_INVARIANT))
    }

    fn container_error(error: ValueError) -> Diagnostic {
        match error.kind() {
            ValueErrorKind::AllocationFailed => {
                BudgetDiagnostic::from_error(BudgetError::Allocation)
            }
            ValueErrorKind::ResourceLimitExceeded => {
                BudgetDiagnostic::from_error(BudgetError::Resource)
            }
            ValueErrorKind::CapacityExceeded => {
                BudgetDiagnostic::from_error(BudgetError::CapacityExceeded)
            }
            ValueErrorKind::InternalInvariantFailure => DiagnosticFactory::internal(
                error.context().unwrap_or(NORMALIZE_VALUE_CONSTRUCTION),
                None,
                None,
                None,
            ),
            ValueErrorKind::DuplicateFieldName | ValueErrorKind::DuplicateMapKey => {
                DiagnosticFactory::make_diagnostic(
                    DiagnosticCode::CONFIG_VALUE_ERROR,
                    diagnostic_message!(
                        "diagnostic.config.duplicate_object_field_or_map_key",
                        "duplicate object field or map key"
                    ),
                    None,
                    None,
                    None,
                )
            }
        }
    }
}
