mod assertions;
mod collection;
mod scalar;

use self::{
    assertions::AssertionValidator, collection::CollectionValidator, scalar::ScalarValidator,
};
use crate::{
    context::{AssignmentContext, RuleContext, SemanticStatus},
    diagnostics::{BudgetDiagnostic, DiagnosticBuffer, DiagnosticFactory},
    instance::LocationIndex,
    scalar::Scalar,
    session::ValidationResources,
};
use opeo::{ErrSlot, OResult, Out, ResultOutExt};
use typol_core::{
    BudgetError, ChargedVec, ConfigValue, Diagnostic, DiagnosticCode, DiagnosticContext, ValueKind,
    ValuePayloadView, ValueView, diagnostic_context,
};
use typol_model::BuiltinType;
use typol_semantic::{
    CompiledSchema, Constraint, EffectiveRules, SchemaAccessError, TypeKind, TypeView,
    VariantPayloadInfo,
};

static VALIDATION_VALUE_SHAPE: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "value-validation",
    "assignment-value-shape-mismatch",
);
static VALIDATION_RULE_OWNER: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "value-validation",
    "effective-rule-owner-mismatch",
);
static VALIDATION_FALLBACK_TYPE: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "fallback-validation",
    "fallback-is-not-target-scalar",
);
static VALIDATION_SCHEMA_ACCESS: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "value-validation",
    "schema-view-access-failed",
);

pub(crate) struct Validator;

impl Validator {
    pub(crate) fn budget_call<'slot, 'borrow, T>(
        errors: &mut ErrSlot<BudgetError>,
        operation: impl for<'call> FnOnce(Out<'call, 'call, BudgetError>) -> OResult<'call, T>,
        out: Out<'slot, 'borrow, Diagnostic>,
    ) -> OResult<'slot, T> {
        match errors.try_call(operation) {
            Ok(value) => Ok(value),
            Err(caught) => Err(out.fail(BudgetDiagnostic::from_error(caught.take()))),
        }
    }

    pub(crate) fn schema_call<'slot, 'borrow, T>(
        errors: &mut ErrSlot<SchemaAccessError>,
        operation: impl for<'call> FnOnce(Out<'call, 'call, SchemaAccessError>) -> OResult<'call, T>,
        out: Out<'slot, 'borrow, Diagnostic>,
    ) -> OResult<'slot, T> {
        match errors.try_call(operation) {
            Ok(value) => Ok(value),
            Err(caught) => Err(out.fail(DiagnosticFactory::internal(
                VALIDATION_SCHEMA_ACCESS.with_object_id(caught.take().index() as u64),
                None,
                None,
                None,
            ))),
        }
    }
}

pub(crate) struct ValidatedFallback {
    _validated: (),
}

pub(crate) struct AssertionsRequest<'schema, 'value, 'index, 'limits> {
    pub(crate) schema: &'schema CompiledSchema,
    pub(crate) root: &'value ConfigValue,
    pub(crate) locations: &'index LocationIndex<'schema>,
    pub(crate) resources: &'index mut ValidationResources<'limits>,
    pub(crate) diagnostics: &'index mut DiagnosticBuffer<'schema>,
}

impl Validator {
    pub(crate) fn completed<'context, 'diagnostics, 'slot>(
        metadata: AssignmentContext<'context>,
        value: ValueView<'_>,
        resources: &mut ValidationResources<'_>,
        diagnostics: &mut DiagnosticBuffer<'diagnostics>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, SemanticStatus> {
        let constraints = Self::effective_constraints(&metadata, value, resources, out.reborrow())?;
        let status = ScalarValidator::validate(
            scalar::ScalarValidationRequest {
                metadata: &metadata,
                value,
                constraints: &constraints,
                resources,
                diagnostics,
            },
            out.reborrow(),
        )?;
        if !matches!(status, SemanticStatus::Success) {
            return Ok(status);
        }
        CollectionValidator::validate(
            collection::CollectionValidationRequest {
                metadata: &metadata,
                value,
                constraints: &constraints,
                resources,
                diagnostics,
            },
            out.reborrow(),
        )
    }

    fn effective_constraints<'context, 'slot>(
        metadata: &AssignmentContext<'context>,
        value: ValueView<'_>,
        resources: &mut ValidationResources<'_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ChargedVec<Constraint<'context>>> {
        let expected_constraints = Self::schema_call(
            &mut resources.schema_access_errors,
            |access_out| metadata.expected.constraints(access_out),
            out.reborrow(),
        )?;
        let shape_valid = metadata.shape_validated
            || Self::shape_budgeted(metadata.expected, value, resources, out.reborrow())?;
        if !expected_constraints.belongs_to(metadata.schema) || !shape_valid {
            return Err(out.fail(DiagnosticFactory::internal(
                VALIDATION_VALUE_SHAPE,
                metadata.span,
                None,
                metadata.source,
            )));
        }
        let local = Self::schema_call(
            &mut resources.schema_access_errors,
            |access_out| {
                RuleContext::constraints(
                    metadata.expected,
                    metadata.field,
                    metadata.variant,
                    access_out,
                )
            },
            out.reborrow(),
        )?;
        Self::budget_call(
            &mut resources.budget_errors,
            |budget_out| {
                resources
                    .budget
                    .consume(metadata.inherited_constraints.len(), budget_out)
            },
            out.reborrow(),
        )?;
        let constraints = match resources.rule_owner_errors.try_call(|rule_out| {
            EffectiveRules::new(
                metadata.schema,
                metadata.inherited_constraints,
                local,
                rule_out,
            )
        }) {
            Ok(constraints) => constraints,
            Err(caught) => {
                let error = caught.take();
                let context = error.index().map_or(VALIDATION_RULE_OWNER, |index| {
                    VALIDATION_RULE_OWNER.with_object_id(index as u64)
                });
                return Err(out.fail(DiagnosticFactory::internal(
                    context,
                    metadata.span,
                    None,
                    metadata.source,
                )));
            }
        };
        let constraints = Self::budget_call(
            &mut resources.budget_errors,
            |budget_out| constraints.collect_budgeted(&resources.budget, budget_out),
            out.reborrow(),
        )?;
        Ok(constraints)
    }

    fn shape_budgeted<'slot>(
        expected: TypeView<'_>,
        value: ValueView<'_>,
        resources: &mut ValidationResources<'_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, bool> {
        let kind = Self::schema_call(
            &mut resources.schema_access_errors,
            |access_out| expected.kind(access_out),
            out.reborrow(),
        )?;
        let fields = match (kind, value.as_ref()) {
            (TypeKind::Struct { fields }, ValueKind::Structure(values)) => Some((fields, values)),
            (TypeKind::Enum { .. }, ValueKind::Enumeration { variant, payload }) => {
                let Some(variant) = Self::budget_call(
                    &mut resources.budget_errors,
                    |budget_out| {
                        expected.variant_named_budgeted(variant, &resources.budget, budget_out)
                    },
                    out.reborrow(),
                )?
                else {
                    return Ok(false);
                };
                match (
                    Self::schema_call(
                        &mut resources.schema_access_errors,
                        |access_out| variant.payload(access_out),
                        out.reborrow(),
                    )?,
                    payload,
                ) {
                    (VariantPayloadInfo::Unit, ValuePayloadView::Unit)
                    | (VariantPayloadInfo::Single { .. }, ValuePayloadView::Single(_)) => {
                        return Ok(true);
                    }
                    (
                        VariantPayloadInfo::Structure { fields },
                        ValuePayloadView::Structure(values),
                    ) => Some((fields, values)),
                    _ => return Ok(false),
                }
            }
            (TypeKind::Map { .. }, ValueKind::Map(entries)) => {
                for entry in entries {
                    Self::budget_call(
                        &mut resources.budget_errors,
                        |budget_out| resources.budget.consume(1, budget_out),
                        out.reborrow(),
                    )?;
                    if entry.as_named().is_none() {
                        return Ok(false);
                    }
                }
                return Ok(true);
            }
            _ => {
                return Self::schema_call(
                    &mut resources.schema_access_errors,
                    |access_out| Self::shape(expected, value, access_out),
                    out.reborrow(),
                );
            }
        };
        if let Some((fields, values)) = fields {
            if fields.len() != values.len() {
                return Ok(false);
            }
            for (field, value) in fields.iter().zip(values) {
                Self::budget_call(
                    &mut resources.budget_errors,
                    |budget_out| resources.budget.consume(1, budget_out),
                    out.reborrow(),
                )?;
                let field_name = Self::schema_call(
                    &mut resources.schema_access_errors,
                    |access_out| field.name(access_out),
                    out.reborrow(),
                )?;
                let Some(value) = value.as_named() else {
                    return Ok(false);
                };
                if !Self::budget_call(
                    &mut resources.budget_errors,
                    |budget_out| {
                        resources
                            .budget
                            .compare_text(field_name, value.name(), budget_out)
                    },
                    out.reborrow(),
                )?
                .is_eq()
                {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    /// Checks only the current node; a completed traversal checks each child, so an arbitrary tree is never treated as validated.
    pub(crate) fn shape<'slot>(
        expected: TypeView<'_>,
        value: ValueView<'_>,
        mut out: Out<'slot, '_, SchemaAccessError>,
    ) -> OResult<'slot, bool> {
        match (expected.kind(out.reborrow())?, value.as_ref()) {
            (TypeKind::Builtin(builtin), value) => match value {
                ValueKind::Signed(value) => Ok(Scalar::signed_width(builtin).is_some_and(|bits| {
                    let shift = 128 - bits;
                    (i128::MIN >> shift) <= value && value <= (i128::MAX >> shift)
                })),
                ValueKind::Unsigned(value) => Ok(Scalar::unsigned_width(builtin)
                    .is_some_and(|bits| value <= (u128::MAX >> (128 - bits)))),
                ValueKind::Float32(value) => Ok(builtin == BuiltinType::F32 && value.is_finite()),
                ValueKind::Float64(value) => Ok(builtin == BuiltinType::F64 && value.is_finite()),
                ValueKind::String(_) => Ok(builtin == BuiltinType::String),
                ValueKind::Bool(_) => Ok(builtin == BuiltinType::Bool),
                _ => Ok(false),
            },
            (TypeKind::Option { .. }, ValueKind::Optional(_)) => Ok(true),
            (TypeKind::Array { length, .. }, ValueKind::Array(values)) => {
                Ok(length.is_none_or(|length| values.len() as u128 == u128::from(length)))
            }
            (TypeKind::Map { .. }, ValueKind::Map(_)) => Ok(true),
            (TypeKind::Struct { fields }, ValueKind::Structure(values)) => {
                if fields.len() != values.len() {
                    return Ok(false);
                }
                for (field, value) in fields.iter().zip(values) {
                    let Some(value) = value.as_named() else {
                        return Ok(false);
                    };
                    if (field.name(out.reborrow()))? != value.name() {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            (TypeKind::Enum { .. }, ValueKind::Enumeration { variant, payload }) => {
                let Some(variant) = expected.variant_named(variant, out.reborrow())? else {
                    return Ok(false);
                };
                match (variant.payload(out.reborrow())?, payload) {
                    (VariantPayloadInfo::Unit, ValuePayloadView::Unit)
                    | (VariantPayloadInfo::Single { .. }, ValuePayloadView::Single(_)) => Ok(true),
                    (
                        VariantPayloadInfo::Structure { fields },
                        ValuePayloadView::Structure(values),
                    ) => {
                        if fields.len() != values.len() {
                            return Ok(false);
                        }
                        for (field, value) in fields.iter().zip(values) {
                            let Some(value) = value.as_named() else {
                                return Ok(false);
                            };
                            if (field.name(out.reborrow()))? != value.name() {
                                return Ok(false);
                            }
                        }
                        Ok(true)
                    }
                    _ => Ok(false),
                }
            }
            _ => Ok(false),
        }
    }

    pub(crate) fn fallback<'slot>(
        metadata: AssignmentContext<'_>,
        value: &ConfigValue,
        resources: &mut ValidationResources<'_>,
        diagnostics: &mut DiagnosticBuffer,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Option<ValidatedFallback>> {
        let expected_kind = Self::schema_call(
            &mut resources.schema_access_errors,
            |access_out| metadata.expected.kind(access_out),
            out.reborrow(),
        )?;
        let status = match (expected_kind, value.as_ref()) {
            (TypeKind::Option { inner }, ValueKind::Optional(Some(leaf))) => {
                let mut inherited = Self::budget_call(
                    &mut resources.budget_errors,
                    |budget_out| ChargedVec::new(Some(&resources.budget), budget_out),
                    out.reborrow(),
                )?;
                let capacity = metadata
                    .inherited_constraints
                    .len()
                    .checked_add(1)
                    .ok_or(DiagnosticFactory::make_diagnostic(
                        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED,
                        "fallback rule count exceeded the limit",
                        metadata.span,
                        None,
                        metadata.source,
                    ))
                    .store_err(out.reborrow())?;
                Self::budget_call(
                    &mut resources.budget_errors,
                    |budget_out| inherited.try_reserve_exact(capacity, budget_out),
                    out.reborrow(),
                )?;
                Self::budget_call(
                    &mut resources.budget_errors,
                    |budget_out| {
                        inherited.extend_from_slice(metadata.inherited_constraints, budget_out)
                    },
                    out.reborrow(),
                )?;
                let local = Self::schema_call(
                    &mut resources.schema_access_errors,
                    |access_out| {
                        RuleContext::constraints(
                            metadata.expected,
                            metadata.field,
                            metadata.variant,
                            access_out,
                        )
                    },
                    out.reborrow(),
                )?;
                inherited.try_push(local, out.reborrow())?;
                Self::completed(
                    AssignmentContext {
                        expected: inner,
                        field: None,
                        variant: None,
                        inherited_constraints: &inherited,
                        shape_validated: false,
                        ..metadata
                    },
                    leaf,
                    resources,
                    diagnostics,
                    out.reborrow(),
                )?
            }
            (TypeKind::Builtin(_), _) => Self::completed(
                AssignmentContext {
                    shape_validated: false,
                    ..metadata
                },
                value.root(),
                resources,
                diagnostics,
                out.reborrow(),
            )?,
            _ => {
                return Err(out.fail(DiagnosticFactory::internal(
                    VALIDATION_FALLBACK_TYPE,
                    metadata.span,
                    None,
                    metadata.source,
                )));
            }
        };
        Ok(matches!(status, SemanticStatus::Success)
            .then_some(ValidatedFallback { _validated: () }))
    }

    pub(crate) fn assertions<'slot, 'borrow>(
        request: AssertionsRequest<'_, '_, '_, '_>,
        out: Out<'slot, 'borrow, Diagnostic>,
    ) -> OResult<'slot, SemanticStatus> {
        AssertionValidator::validate(request, out)
    }
}
