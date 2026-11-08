mod bounds;
mod summary;
pub(crate) use bounds::AnnotationBounds;

use crate::{
    SchemaLimits,
    annotations::{AnnotationCompilation, AnnotationOwner, AnnotationTask, TypedNumber},
    context::CompileContext,
    diagnostics::{BuildFailure, DiagnosticCollector, EmergencyDiagnostics},
    module_tree::TypeDeclarationKind,
    symbols::{FieldSymbolId, LookupResult, SymbolTable, TypeSymbolId},
    types::{ResolvedTypeGraph, ResolvedTypeKind, TypeUseId},
};
use core::cmp::Ordering;
use opeo::{OResult, Out};
use typol_core::{
    BudgetError, ChargedVec, DiagnosticContext, DiagnosticMessage, NumericLiteral,
    NumericTokenKind, SourceSpan,
};
use typol_i18n::diagnostic_message;
use typol_syntax::{AstAccessError, BuiltinType, NumberKind, NumberLiteral};

use summary::ConstraintSummary;

pub(crate) enum AnnotationError {
    Invalid(DiagnosticMessage<'static>),
    DuplicateBound {
        message: DiagnosticMessage<'static>,
        span: SourceSpan,
        earlier_span: SourceSpan,
    },
    Allocation,
    Resource,
    Capacity,
    Internal(DiagnosticContext),
}

impl AnnotationError {
    pub(crate) const fn internal(context: DiagnosticContext) -> Self {
        Self::Internal(context)
    }
}

impl From<BudgetError> for AnnotationError {
    fn from(error: BudgetError) -> Self {
        match error {
            BudgetError::Resource
            | BudgetError::RegexSteps
            | BudgetError::RegexSearchSteps
            | BudgetError::RegexWorkspace => Self::Resource,
            BudgetError::Allocation => Self::Allocation,
            BudgetError::CapacityExceeded => Self::Capacity,
            BudgetError::InvariantViolation(context) => Self::Internal(*context),
        }
    }
}

impl From<AstAccessError> for AnnotationError {
    fn from(error: AstAccessError) -> Self {
        Self::Internal(EmergencyDiagnostics::ast_access_context(error))
    }
}

pub(crate) struct AnnotationValues;

pub(super) struct ConstraintValidationInput<'task, 'src, 'output, 'types, 'context, 'diagnostics> {
    pub(super) task: &'task AnnotationTask<'src>,
    pub(super) output: &'output AnnotationCompilation,
    pub(super) types: &'types ResolvedTypeGraph,
    pub(super) context: &'context CompileContext,
    pub(super) diagnostics: &'diagnostics mut DiagnosticCollector,
}

pub(super) struct UniquePathInput<'path, 'symbols, 'types, 'limits> {
    pub(super) path: &'path str,
    pub(super) owner: TypeSymbolId,
    pub(super) symbols: &'symbols SymbolTable<'symbols>,
    pub(super) types: &'types ResolvedTypeGraph,
    pub(super) limits: &'limits SchemaLimits,
}

impl AnnotationValues {
    pub(crate) fn validate_combined_constraints<'slot>(
        input: ConstraintValidationInput<'_, '_, '_, '_, '_, '_>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ()> {
        let ConstraintValidationInput {
            task,
            output,
            types,
            context,
            diagnostics,
        } = input;
        let span = task.span;
        let mut summary = ConstraintSummary::default();
        let mut link = types
            .type_use(task.target)
            .and_then(|value| value.alias_chain());
        let mut depth = 0usize;
        while let Some(link_id) = link {
            context.consume(1, span, diagnostics, out.reborrow())?;
            let Some(next_depth) = depth.checked_add(1) else {
                diagnostics.push(EmergencyDiagnostics::resource(span));
                return Err(out.fail(BuildFailure::empty()));
            };
            depth = next_depth;
            if depth > context.limits().max_nesting_depth() {
                diagnostics.push(EmergencyDiagnostics::resource(span));
                return Err(out.fail(BuildFailure::empty()));
            }
            let Some(alias) = types.alias_link(link_id) else {
                diagnostics.push(schema_internal!(span));
                return Err(out.fail(BuildFailure::empty()));
            };
            let constraints = diagnostics.require(
                output.alias_constraints(alias.symbol()),
                span,
                out.reborrow(),
            )?;
            for constraint in constraints {
                context.consume(1, span, diagnostics, out.reborrow())?;
                summary.include(constraint, diagnostics);
            }
            link = alias.next();
        }

        match task.owner {
            AnnotationOwner::Field(id) => {
                let constraints =
                    diagnostics.require(output.field_constraints(id), span, out.reborrow())?;
                for constraint in constraints {
                    context.consume(1, span, diagnostics, out.reborrow())?;
                    summary.include(constraint, diagnostics);
                }
            }
            AnnotationOwner::VariantPayload(id) => {
                let constraints = diagnostics.require(
                    output.variant_payload_constraints(id),
                    span,
                    out.reborrow(),
                )?;
                for constraint in constraints {
                    context.consume(1, span, diagnostics, out.reborrow())?;
                    summary.include(constraint, diagnostics);
                }
            }
            AnnotationOwner::TypeUse(id) => {
                let constraints =
                    diagnostics.require(output.type_use_constraints(id), span, out.reborrow())?;
                for constraint in constraints {
                    context.consume(1, span, diagnostics, out.reborrow())?;
                    summary.include(constraint, diagnostics);
                }
            }
            AnnotationOwner::Alias(_) => {}
        }

        let Some(target) = Self::resolved_kind(types, task.target) else {
            diagnostics.push(schema_internal!(span));
            return Err(out.fail(BuildFailure::empty()));
        };
        if let ResolvedTypeKind::Array {
            length: Some(length),
            ..
        } = target
        {
            let length = u64::from(length);
            let (size_min, size_max) = summary.size_bounds();
            if size_min.is_some_and(|min| length < min) || size_max.is_some_and(|max| length > max)
            {
                diagnostics.push(diagnostics.make_semantic(
                    diagnostic_message!(
                        "diagnostic.schema.annotation_size_fixed_array_conflict",
                        "fixed-size array length is incompatible with the @size bounds"
                    ),
                    summary.size_span().unwrap_or(span),
                ));
            }
        }
        Ok(())
    }

    fn ast_access<'slot, 'borrow, T>(
        context: &CompileContext,
        out: Out<'slot, 'borrow, AnnotationError>,
        operation: impl for<'call> FnOnce(Out<'call, 'call, AstAccessError>) -> OResult<'call, T>,
    ) -> OResult<'slot, T> {
        context.ast_access(out, operation, AnnotationError::from)
    }

    pub(crate) fn resolve_unique_path<'slot>(
        input: UniquePathInput<'_, '_, '_, '_>,
        mut out: Out<'slot, '_, AnnotationError>,
    ) -> OResult<'slot, ChargedVec<FieldSymbolId>> {
        let UniquePathInput {
            path,
            owner,
            symbols,
            types,
            limits,
        } = input;
        let context = symbols.context();
        context.execution().consume(path.len(), out.reborrow())?;
        let segment_count = path.split('.').count();
        if segment_count == 0 || segment_count > limits.max_nesting_depth() {
            return Err(out.fail(AnnotationError::Resource));
        }
        let mut fields = ChargedVec::new(Some(context.execution()), out.reborrow())?;
        fields.try_reserve_exact(segment_count, out.reborrow())?;
        let mut current_owner = owner;
        context.execution().consume(path.len(), out.reborrow())?;
        for (index, segment) in path.split('.').enumerate() {
            context
                .execution()
                .consume(segment.len() + 1, out.reborrow())?;
            if !Self::valid_identifier(segment) {
                return Err(out.fail(AnnotationError::Invalid(diagnostic_message!(
                    "diagnostic.schema.annotation_unique_path_invalid",
                    "@unique(by=...) path format is invalid"
                ))));
            }
            let field_id = match symbols.lookup_field::<AnnotationError>(
                current_owner,
                segment,
                out.reborrow(),
            )? {
                LookupResult::Found(id) => id,
                LookupResult::Missing | LookupResult::Ambiguous => {
                    return Err(out.fail(AnnotationError::Invalid(diagnostic_message!(
                        "diagnostic.schema.annotation_unique_path_field_missing",
                        "@unique(by=...) refers to a missing or ambiguous field"
                    ))));
                }
            };
            fields.try_push(field_id, out.reborrow())?;
            let Some(field_type) = types.field_type(field_id) else {
                return Err(out.fail(AnnotationError::internal(schema_context!(
                    "annotation-values",
                    "unique-field-type-missing"
                ))));
            };
            if index + 1 == segment_count {
                let (kind, _) = match Self::unwrap_option(types, field_type) {
                    Ok(value) => value,
                    Err(error) => return Err(out.fail(error)),
                };
                if !Self::is_scalar_or_enum(types, kind, symbols, out.reborrow())? {
                    return Err(out.fail(AnnotationError::Invalid(diagnostic_message!(
                        "diagnostic.schema.annotation_unique_path_final_field",
                        "the final field in @unique(by=...) must be a scalar or enum"
                    ))));
                }
            } else {
                let Some(kind) = Self::resolved_kind(types, field_type) else {
                    return Err(out.fail(AnnotationError::internal(schema_context!(
                        "annotation-values",
                        "unique-field-kind-missing"
                    ))));
                };
                let ResolvedTypeKind::User(next_owner) = kind else {
                    return Err(out.fail(AnnotationError::Invalid(diagnostic_message!(
                        "diagnostic.schema.annotation_unique_path_intermediate_field",
                        "intermediate fields in @unique(by=...) must be non-optional structs"
                    ))));
                };
                let Some(owner_symbol) = symbols.type_symbol(next_owner) else {
                    return Err(out.fail(AnnotationError::internal(schema_context!(
                        "annotation-values",
                        "unique-field-owner-missing"
                    ))));
                };
                if !matches!(owner_symbol.declaration(), TypeDeclarationKind::Struct(_)) {
                    return Err(out.fail(AnnotationError::Invalid(diagnostic_message!(
                        "diagnostic.schema.annotation_unique_path_intermediate_field",
                        "intermediate fields in @unique(by=...) must be non-optional structs"
                    ))));
                }
                current_owner = next_owner;
            }
        }
        Ok(fields)
    }

    pub(crate) fn effective_builtin(
        types: &ResolvedTypeGraph,
        target: TypeUseId,
    ) -> Result<Option<(BuiltinType, bool)>, AnnotationError> {
        let (kind, optional) = match Self::unwrap_option(types, target) {
            Ok(result) => result,
            Err(AnnotationError::Invalid(_)) => return Ok(None),
            Err(error) => return Err(error),
        };
        match kind {
            ResolvedTypeKind::Builtin(builtin) => Ok(Some((builtin, optional))),
            _ => Ok(None),
        }
    }

    pub(crate) fn unwrap_option(
        types: &ResolvedTypeGraph,
        target: TypeUseId,
    ) -> Result<(ResolvedTypeKind, bool), AnnotationError> {
        match Self::resolved_kind(types, target).ok_or_else(|| {
            AnnotationError::internal(schema_context!("annotation-values", "target-kind-missing"))
        })? {
            ResolvedTypeKind::Option(inner) => {
                let kind = Self::resolved_kind(types, inner).ok_or_else(|| {
                    AnnotationError::internal(schema_context!(
                        "annotation-values",
                        "option-inner-kind-missing"
                    ))
                })?;
                if matches!(kind, ResolvedTypeKind::Option(_)) {
                    return Err(AnnotationError::Invalid(diagnostic_message!(
                        "diagnostic.schema.annotation_nested_options_unsupported",
                        "annotation target does not support nested options"
                    )));
                }
                Ok((kind, true))
            }
            kind => Ok((kind, false)),
        }
    }

    pub(crate) fn resolved_kind(
        types: &ResolvedTypeGraph,
        target: TypeUseId,
    ) -> Option<ResolvedTypeKind> {
        types.type_use(target).map(|value| value.kind())
    }

    pub(crate) fn numeric_builtin(kind: ResolvedTypeKind) -> Option<BuiltinType> {
        let ResolvedTypeKind::Builtin(builtin) = kind else {
            return None;
        };
        Self::is_numeric_builtin(builtin).then_some(builtin)
    }

    pub(crate) fn is_numeric_builtin(kind: BuiltinType) -> bool {
        matches!(
            kind,
            BuiltinType::I8
                | BuiltinType::I16
                | BuiltinType::I32
                | BuiltinType::I64
                | BuiltinType::I128
                | BuiltinType::U8
                | BuiltinType::U16
                | BuiltinType::U32
                | BuiltinType::U64
                | BuiltinType::U128
                | BuiltinType::F32
                | BuiltinType::F64
        )
    }

    fn is_scalar_or_enum<'slot>(
        types: &ResolvedTypeGraph,
        kind: ResolvedTypeKind,
        symbols: &SymbolTable<'_>,
        mut out: Out<'slot, '_, AnnotationError>,
    ) -> OResult<'slot, bool> {
        symbols.context().execution().consume(1, out.reborrow())?;
        match kind {
            ResolvedTypeKind::Builtin(_) => Ok(true),
            ResolvedTypeKind::User(id) => {
                let Some(item) = symbols.type_symbol(id) else {
                    return Err(out.fail(AnnotationError::internal(schema_context!(
                        "annotation-values",
                        "scalar-or-enum-type-missing"
                    ))));
                };
                Ok(matches!(item.declaration(), TypeDeclarationKind::Enum(_)))
            }
            ResolvedTypeKind::Option(inner) => {
                let Some(inner) = Self::resolved_kind(types, inner) else {
                    return Err(out.fail(AnnotationError::internal(schema_context!(
                        "annotation-values",
                        "option-inner-type-missing"
                    ))));
                };
                Self::is_scalar_or_enum(types, inner, symbols, out.reborrow())
            }
            ResolvedTypeKind::Array { .. } | ResolvedTypeKind::Map(_) => Ok(false),
        }
    }

    pub(crate) fn parse_typed_number<'slot>(
        number: NumberLiteral<'_>,
        builtin: BuiltinType,
        context: &CompileContext,
        mut out: Out<'slot, '_, AnnotationError>,
    ) -> OResult<'slot, Option<TypedNumber>> {
        let raw = Self::ast_access(context, out.reborrow(), |ast_out| number.raw(ast_out))?;
        context.execution().consume(raw.len(), out.reborrow())?;
        Self::parse_typed_number_value(number, builtin, context, out.reborrow())
    }

    fn parse_typed_number_value<'slot>(
        number: NumberLiteral<'_>,
        builtin: BuiltinType,
        context: &CompileContext,
        mut out: Out<'slot, '_, AnnotationError>,
    ) -> OResult<'slot, Option<TypedNumber>> {
        let raw = Self::ast_access(context, out.reborrow(), |ast_out| number.raw(ast_out))?;
        if Self::is_float_builtin(builtin) {
            let kind = match number.kind() {
                NumberKind::Integer => NumericTokenKind::Integer,
                NumberKind::Float => NumericTokenKind::Float,
            };
            let value = if builtin == BuiltinType::F32 {
                NumericLiteral::contextual_f32(raw, kind, number.is_negative())
                    .ok()
                    .map(f64::from)
            } else {
                NumericLiteral::contextual_f64(raw, kind, number.is_negative()).ok()
            };
            return Ok(value.map(TypedNumber::Float));
        }
        if number.kind() != NumberKind::Integer {
            return Ok(None);
        }
        let Some(magnitude) = Self::parse_integer_magnitude(raw) else {
            return Ok(None);
        };
        if Self::is_signed_builtin(builtin) {
            let Some((min, max)) = Self::signed_bounds(builtin) else {
                return Ok(None);
            };
            let value = if number.is_negative() {
                let limit = min.unsigned_abs();
                if magnitude > limit {
                    return Ok(None);
                }
                if magnitude == limit {
                    min
                } else {
                    let Some(value) = i128::try_from(magnitude).ok() else {
                        return Ok(None);
                    };
                    -value
                }
            } else {
                let Some(value) = i128::try_from(magnitude).ok() else {
                    return Ok(None);
                };
                if value > max {
                    return Ok(None);
                }
                value
            };
            Ok((value >= min && value <= max).then_some(TypedNumber::Signed(value)))
        } else {
            if number.is_negative() {
                return Ok(None);
            }
            let Some(max) = Self::unsigned_max(builtin) else {
                return Ok(None);
            };
            Ok((magnitude <= max).then_some(TypedNumber::Unsigned(magnitude)))
        }
    }

    fn parse_integer_magnitude(raw: &str) -> Option<u128> {
        if let Some(hex) = raw.strip_prefix("0x") {
            u128::from_str_radix(hex, 16).ok()
        } else {
            raw.parse::<u128>().ok()
        }
    }

    fn signed_bounds(builtin: BuiltinType) -> Option<(i128, i128)> {
        Some(match builtin {
            BuiltinType::I8 => (i8::MIN as i128, i8::MAX as i128),
            BuiltinType::I16 => (i16::MIN as i128, i16::MAX as i128),
            BuiltinType::I32 => (i32::MIN as i128, i32::MAX as i128),
            BuiltinType::I64 => (i64::MIN as i128, i64::MAX as i128),
            BuiltinType::I128 => (i128::MIN, i128::MAX),
            _ => return None,
        })
    }

    fn unsigned_max(builtin: BuiltinType) -> Option<u128> {
        Some(match builtin {
            BuiltinType::U8 => u8::MAX as u128,
            BuiltinType::U16 => u16::MAX as u128,
            BuiltinType::U32 => u32::MAX as u128,
            BuiltinType::U64 => u64::MAX as u128,
            BuiltinType::U128 => u128::MAX,
            _ => return None,
        })
    }

    fn is_signed_builtin(builtin: BuiltinType) -> bool {
        matches!(
            builtin,
            BuiltinType::I8
                | BuiltinType::I16
                | BuiltinType::I32
                | BuiltinType::I64
                | BuiltinType::I128
        )
    }

    fn is_float_builtin(builtin: BuiltinType) -> bool {
        matches!(builtin, BuiltinType::F32 | BuiltinType::F64)
    }

    pub(crate) fn parse_nonnegative_u64<'slot>(
        number: NumberLiteral<'_>,
        context: &CompileContext,
        mut out: Out<'slot, '_, AnnotationError>,
    ) -> OResult<'slot, Option<u64>> {
        let raw = Self::ast_access(context, out.reborrow(), |ast_out| number.raw(ast_out))?;
        context.execution().consume(raw.len(), out.reborrow())?;
        Self::parse_nonnegative_value(number, context, out.reborrow())
    }

    fn parse_nonnegative_value<'slot>(
        number: NumberLiteral<'_>,
        context: &CompileContext,
        mut out: Out<'slot, '_, AnnotationError>,
    ) -> OResult<'slot, Option<u64>> {
        let raw = Self::ast_access(context, out.reborrow(), |ast_out| number.raw(ast_out))?;
        if number.kind() != NumberKind::Integer || number.is_negative() || raw.starts_with("0x") {
            return Ok(None);
        }
        Ok(raw.parse::<u64>().ok())
    }

    pub(crate) fn bounds_reversed(min: Option<TypedNumber>, max: Option<TypedNumber>) -> bool {
        matches!((min, max), (Some(min), Some(max)) if Self::compare_numbers(min, max).is_some_and(|order| order.is_gt()))
    }

    pub(super) fn compare_numbers(left: TypedNumber, right: TypedNumber) -> Option<Ordering> {
        match (left, right) {
            (TypedNumber::Signed(left), TypedNumber::Signed(right)) => Some(left.cmp(&right)),
            (TypedNumber::Unsigned(left), TypedNumber::Unsigned(right)) => Some(left.cmp(&right)),
            (TypedNumber::Float(left), TypedNumber::Float(right)) => left.partial_cmp(&right),
            _ => None,
        }
    }

    pub(crate) fn valid_environment_name(name: &str) -> bool {
        let mut bytes = name.bytes();
        let Some(first) = bytes.next() else {
            return false;
        };
        if !(first.is_ascii_alphabetic() || first == b'_') {
            return false;
        }
        bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    }

    fn valid_identifier(name: &str) -> bool {
        let mut bytes = name.bytes();
        let Some(first) = bytes.next() else {
            return false;
        };
        (first.is_ascii_alphabetic() || first == b'_')
            && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    }

    pub(crate) fn span_key(span: SourceSpan) -> (u32, usize) {
        (span.source_id().get(), span.span().start())
    }
}
