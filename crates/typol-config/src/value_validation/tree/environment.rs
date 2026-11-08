use crate::{
    context::AssignmentContext,
    diagnostics::DiagnosticFactory,
    validation::Validator,
    value::ValueFactory,
    value_validation::{TreeContext, TreeTarget, support::TreeSupport},
};
use core::borrow::BorrowMut;
use opeo::{ErrSlot, OResult, Out};
use typol_core::{
    Diagnostic, DiagnosticCode, EnvironmentReference, ValueError, ValueKind, ValueScalar,
    ValueSource, ValueView,
};
use typol_model::BuiltinType;
use typol_semantic::TypeKind;

pub(super) struct EnvironmentReferenceValidator;

pub(super) enum EnvironmentValidation {
    Valid,
    Reported,
    Stopped,
}

impl EnvironmentReferenceValidator {
    pub(super) fn validate<'schema, 'slot>(
        context: &mut TreeContext<'schema, '_, '_>,
        target: TreeTarget<'schema>,
        value: ValueView<'_>,
        fallback_validated: bool,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, EnvironmentValidation> {
        let prior_errors = context.diagnostics.len();
        if Self::is_valid(context, target, value, fallback_validated, out.reborrow())? {
            return Ok(EnvironmentValidation::Valid);
        }
        if context.diagnostics.len() > prior_errors {
            return Ok(if context.diagnostics.stopped() {
                EnvironmentValidation::Stopped
            } else {
                EnvironmentValidation::Reported
            });
        }
        let diagnostic = DiagnosticFactory::make_diagnostic(
            DiagnosticCode::CONFIG_VALUE_ERROR,
            "environment source reference is invalid for this Schema field",
            target.span,
            None,
            Some(ValueSource::EnvironmentVariable),
        );
        if context.diagnostics.push(diagnostic) {
            Ok(EnvironmentValidation::Reported)
        } else {
            Ok(EnvironmentValidation::Stopped)
        }
    }

    pub(super) fn is_valid<'schema, 'slot>(
        context: &mut TreeContext<'schema, '_, '_>,
        target: TreeTarget<'schema>,
        value: ValueView<'_>,
        fallback_validated: bool,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, bool> {
        let Some(reference) = value.environment_reference() else {
            return Ok(value.source() != ValueSource::EnvironmentVariable);
        };
        if let ValueKind::Optional(Some(child)) = value.as_ref()
            && let Some(child_reference) = child.environment_reference()
            && !Self::references_match(&reference, &child_reference, context, out.reborrow())?
        {
            return Ok(false);
        }
        if matches!(value.as_ref(), ValueKind::Optional(None)) {
            return Ok(false);
        }
        if !Self::name_is_valid(context, reference.name(), out.reborrow())? {
            return Ok(false);
        }
        let kind = TreeSupport::schema_call(
            &mut context.resources.schema_access_errors,
            |schema_out| target.expected.kind(schema_out),
            out.reborrow(),
        )?;
        let (builtin, optional) = match kind {
            TypeKind::Builtin(builtin) => (builtin, false),
            TypeKind::Option { inner } => match TreeSupport::schema_call(
                &mut context.resources.schema_access_errors,
                |schema_out| inner.kind(schema_out),
                out.reborrow(),
            )? {
                TypeKind::Builtin(builtin) => (builtin, true),
                _ => return Ok(false),
            },
            _ => return Ok(false),
        };
        let Some(field) = target.environment_field else {
            return Ok(false);
        };
        let authorization = TreeSupport::schema_call(
            &mut context.resources.schema_access_errors,
            |schema_out| field.environment(schema_out),
            out.reborrow(),
        )?;
        let Some(authorization) = authorization else {
            return Ok(false);
        };
        let authorized_name = authorization.name();
        let equal = TreeSupport::budget_call(
            &mut context.resources.budget_errors,
            |budget_out| {
                context
                    .resources
                    .budget
                    .compare_text(reference.name(), authorized_name, budget_out)
            },
            out.reborrow(),
        )?
        .is_eq();
        if !equal {
            return Ok(false);
        }
        if fallback_validated {
            return Ok(true);
        }
        if !Self::fallback_matches(builtin, &reference, context, out.reborrow())? {
            return Ok(false);
        }
        Self::validate_fallback(target, optional, &reference, context, out.reborrow())
    }

    fn references_match<'schema, 'slot>(
        left: &EnvironmentReference<'_>,
        right: &EnvironmentReference<'_>,
        context: &mut TreeContext<'schema, '_, '_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, bool> {
        let names_match = TreeSupport::budget_call(
            &mut context.resources.budget_errors,
            |budget_out| {
                context
                    .resources
                    .budget
                    .compare_text(left.name(), right.name(), budget_out)
            },
            out.reborrow(),
        )?
        .is_eq();
        if !names_match {
            return Ok(false);
        }
        match (left.fallback().copied(), right.fallback().copied()) {
            (None, None) => Ok(true),
            (Some(ValueScalar::Signed(left)), Some(ValueScalar::Signed(right))) => {
                Ok(left == right)
            }
            (Some(ValueScalar::Unsigned(left)), Some(ValueScalar::Unsigned(right))) => {
                Ok(left == right)
            }
            (Some(ValueScalar::Float32(left)), Some(ValueScalar::Float32(right))) => {
                Ok(left.to_bits() == right.to_bits())
            }
            (Some(ValueScalar::Float64(left)), Some(ValueScalar::Float64(right))) => {
                Ok(left.to_bits() == right.to_bits())
            }
            (Some(ValueScalar::String(left)), Some(ValueScalar::String(right))) => {
                Ok(TreeSupport::budget_call(
                    &mut context.resources.budget_errors,
                    |budget_out| {
                        context
                            .resources
                            .budget
                            .compare_text(left, right, budget_out)
                    },
                    out.reborrow(),
                )?
                .is_eq())
            }
            (Some(ValueScalar::Bool(left)), Some(ValueScalar::Bool(right))) => Ok(left == right),
            _ => Ok(false),
        }
    }

    fn name_is_valid<'schema, 'slot>(
        context: &mut TreeContext<'schema, '_, '_>,
        name: &str,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, bool> {
        if name.is_empty() || name.len() > context.resources.value_limits.max_string_bytes() {
            return Ok(false);
        }
        for (index, byte) in name.bytes().enumerate() {
            TreeSupport::budget_call(
                &mut context.resources.budget_errors,
                |budget_out| context.resources.budget.consume(1, budget_out),
                out.reborrow(),
            )?;
            if !(byte == b'_' || byte.is_ascii_alphabetic() || (index > 0 && byte.is_ascii_digit()))
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn fallback_matches<'schema, 'slot>(
        builtin: BuiltinType,
        reference: &EnvironmentReference<'_>,
        context: &mut TreeContext<'schema, '_, '_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, bool> {
        let Some(fallback) = reference.fallback() else {
            return Ok(true);
        };
        let matches = match (builtin, fallback) {
            (BuiltinType::String, ValueScalar::String(value)) => {
                value.len() <= context.resources.value_limits.max_string_bytes()
                    && value.chars().all(|character| {
                        !character.is_control() || matches!(character, '\n' | '\r' | '\t')
                    })
            }
            (BuiltinType::Bool, ValueScalar::Bool(_)) => true,
            (BuiltinType::I8, ValueScalar::Signed(value)) => i8::try_from(*value).is_ok(),
            (BuiltinType::I16, ValueScalar::Signed(value)) => i16::try_from(*value).is_ok(),
            (BuiltinType::I32, ValueScalar::Signed(value)) => i32::try_from(*value).is_ok(),
            (BuiltinType::I64, ValueScalar::Signed(value)) => i64::try_from(*value).is_ok(),
            (BuiltinType::I128, ValueScalar::Signed(_)) => true,
            (BuiltinType::U8, ValueScalar::Unsigned(value)) => u8::try_from(*value).is_ok(),
            (BuiltinType::U16, ValueScalar::Unsigned(value)) => u16::try_from(*value).is_ok(),
            (BuiltinType::U32, ValueScalar::Unsigned(value)) => u32::try_from(*value).is_ok(),
            (BuiltinType::U64, ValueScalar::Unsigned(value)) => u64::try_from(*value).is_ok(),
            (BuiltinType::U128, ValueScalar::Unsigned(_)) => true,
            (BuiltinType::F32, ValueScalar::Float32(value)) => value.is_finite(),
            (BuiltinType::F64, ValueScalar::Float64(value)) => value.is_finite(),
            _ => false,
        };
        if matches {
            TreeSupport::budget_call(
                &mut context.resources.budget_errors,
                |budget_out| context.resources.budget.consume(1, budget_out),
                out.reborrow(),
            )?;
        }
        Ok(matches)
    }

    fn validate_fallback<'schema, 'slot>(
        target: TreeTarget<'schema>,
        optional: bool,
        reference: &EnvironmentReference<'_>,
        context: &mut TreeContext<'schema, '_, '_>,
        out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, bool> {
        let Some(fallback) = reference.fallback().copied() else {
            return Ok(true);
        };
        let mut value_errors = ErrSlot::<ValueError>::new();
        let scalar = match value_errors.try_call(|value_out| match fallback {
            ValueScalar::Signed(value) => ValueFactory::signed(
                value,
                ValueSource::ExplicitFallback,
                Some(&context.resources.budget),
                value_out,
            ),
            ValueScalar::Unsigned(value) => ValueFactory::unsigned(
                value,
                ValueSource::ExplicitFallback,
                Some(&context.resources.budget),
                value_out,
            ),
            ValueScalar::Float32(value) => ValueFactory::float32(
                value,
                ValueSource::ExplicitFallback,
                Some(&context.resources.budget),
                value_out,
            ),
            ValueScalar::Float64(value) => ValueFactory::float64(
                value,
                ValueSource::ExplicitFallback,
                Some(&context.resources.budget),
                value_out,
            ),
            ValueScalar::String(value) => ValueFactory::string(
                value,
                ValueSource::ExplicitFallback,
                Some(&context.resources.budget),
                value_out,
            ),
            ValueScalar::Bool(value) => ValueFactory::boolean(
                value,
                ValueSource::ExplicitFallback,
                Some(&context.resources.budget),
                value_out,
            ),
        }) {
            Ok(value) => value,
            Err(caught) => {
                return Err(out.fail(ValueFactory::failure(
                    caught.take(),
                    target.span,
                    Some(ValueSource::ExplicitFallback),
                )));
            }
        };
        let fallback = if optional {
            let mut value_errors = ErrSlot::<ValueError>::new();
            match value_errors.try_call(|value_out| {
                ValueFactory::optional_owned(
                    scalar,
                    ValueSource::ExplicitFallback,
                    Some(&context.resources.budget),
                    value_out,
                )
            }) {
                Ok(value) => value,
                Err(caught) => {
                    return Err(out.fail(ValueFactory::failure(
                        caught.take(),
                        target.span,
                        Some(ValueSource::ExplicitFallback),
                    )));
                }
            }
        } else {
            scalar
        };
        let metadata = AssignmentContext {
            schema: context.schema,
            node: None,
            locations: context.locations,
            inherited_constraints: context.inherited_constraints,
            expected: target.expected,
            field: target.field,
            variant: target.variant,
            span: target.span,
            path: context.path,
            source: Some(ValueSource::ExplicitFallback),
            shape_validated: false,
        };
        let diagnostic_errors = context.diagnostic_errors.borrow_mut();
        match diagnostic_errors.try_call(|diagnostic_out| {
            Validator::fallback(
                metadata,
                &fallback,
                context.resources,
                context.diagnostics,
                diagnostic_out,
            )
        }) {
            Ok(proof) => Ok(proof.is_some()),
            Err(caught) => Err(out.fail(caught.take())),
        }
    }
}
