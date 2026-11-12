use crate::{
    annotations::values::{AnnotationBounds, AnnotationError, AnnotationValues, UniquePathInput},
    annotations::{
        AnnotationContext, ConstraintData, ConstraintKind, ConstraintLayer, EnvironmentRecord,
        TypedNumber,
    },
    context::CompileContext,
    module_tree::TypeDeclarationKind,
    symbols::LookupResult,
    types::{ResolvedTypeKind, TypeUseId},
};
use opeo::{OResult, Out};
use typol_core::{ChargedString, ChargedVec};
use typol_i18n::diagnostic_message;
use typol_semantic::{ProgramId, VariantRestrictionMode as CompiledVariantRestrictionMode};
use typol_syntax::{
    Annotation, AnnotationData, AnnotationKind, BuiltinType, NumberLiteral, VariantRestrictionMode,
};

pub(super) enum CompiledAnnotation {
    Constraint(ConstraintData),
    Environment(EnvironmentRecord),
    Secret,
}

pub(super) struct AnnotationRuleCompiler;

impl AnnotationRuleCompiler {
    pub(super) fn compile_one_annotation<'slot>(
        annotation: Annotation<'_>,
        target: TypeUseId,
        layer: ConstraintLayer,
        context: &AnnotationContext<'_, '_>,
        mut out: Out<'slot, '_, AnnotationError>,
    ) -> OResult<'slot, CompiledAnnotation> {
        let symbols = context.symbols;
        let types = context.types;
        let limits = context.limits;
        context.budget.execution().consume(3, out.reborrow())?;
        let Some(target_kind) = AnnotationValues::resolved_kind(types, target) else {
            return Err(out.fail(AnnotationError::internal(schema_context!(
                "annotation-compile",
                "target-type-missing"
            ))));
        };
        let (value_kind, _) = match AnnotationValues::unwrap_option(types, target) {
            Ok(value) => value,
            Err(error) => return Err(out.fail(error)),
        };
        let kind = context.budget.ast_access(
            out.reborrow(),
            |ast_out| annotation.kind(ast_out),
            AnnotationError::from,
        )?;
        let data = context.budget.ast_access(
            out.reborrow(),
            |ast_out| annotation.data(ast_out),
            AnnotationError::from,
        )?;

        match data {
            AnnotationData::Min(_) | AnnotationData::Max(_) | AnnotationData::Range { .. } => {
                let Some(builtin) = AnnotationValues::numeric_builtin(value_kind) else {
                    return Err(out.fail(AnnotationError::Invalid(diagnostic_message!(
                        "diagnostic.schema.annotation_numeric_only",
                        "numeric bound annotations apply only to numeric types"
                    ))));
                };
                let (min, max) = match data {
                    AnnotationData::Min(number) => (
                        Some(required_typed_number(
                            number,
                            builtin,
                            context.budget,
                            out.reborrow(),
                        )?),
                        None,
                    ),
                    AnnotationData::Max(number) => (
                        None,
                        Some(required_typed_number(
                            number,
                            builtin,
                            context.budget,
                            out.reborrow(),
                        )?),
                    ),
                    AnnotationData::Range { min, max } => (
                        Some(required_typed_number(
                            min,
                            builtin,
                            context.budget,
                            out.reborrow(),
                        )?),
                        Some(required_typed_number(
                            max,
                            builtin,
                            context.budget,
                            out.reborrow(),
                        )?),
                    ),
                    _ => {
                        return Err(out.fail(AnnotationError::internal(schema_context!(
                            "annotation-compile",
                            "numeric-annotation-shape-mismatch"
                        ))));
                    }
                };
                if AnnotationValues::bounds_reversed(min, max) {
                    return Err(out.fail(AnnotationError::Invalid(diagnostic_message!(
                        "diagnostic.schema.annotation_numeric_bounds_reversed",
                        "the minimum numeric constraint cannot exceed the maximum"
                    ))));
                }
                Ok(CompiledAnnotation::Constraint(
                    ConstraintData::NumberBounds { min, max },
                ))
            }
            AnnotationData::Length { min, max } => {
                if value_kind != ResolvedTypeKind::Builtin(BuiltinType::String) {
                    return Err(out.fail(AnnotationError::Invalid(diagnostic_message!(
                        "diagnostic.schema.annotation_length_requires_string",
                        "@length applies only to string or option<string>"
                    ))));
                }
                let min =
                    AnnotationValues::parse_nonnegative_u64(min, context.budget, out.reborrow())?;
                let Some(min) = min else {
                    return Err(out.fail(AnnotationError::Invalid(diagnostic_message!(
                        "diagnostic.schema.annotation_length_bounds_invalid",
                        "@length bounds must be non-negative decimal integers within the u64 range"
                    ))));
                };
                let max =
                    AnnotationValues::parse_nonnegative_u64(max, context.budget, out.reborrow())?;
                let Some(max) = max else {
                    return Err(out.fail(AnnotationError::Invalid(diagnostic_message!(
                        "diagnostic.schema.annotation_length_bounds_invalid",
                        "@length bounds must be non-negative decimal integers within the u64 range"
                    ))));
                };
                if min > max {
                    return Err(out.fail(AnnotationError::Invalid(diagnostic_message!(
                        "diagnostic.schema.annotation_length_bounds_reversed",
                        "the @length minimum cannot exceed the maximum"
                    ))));
                }
                Ok(CompiledAnnotation::Constraint(
                    ConstraintData::LengthBounds { min, max },
                ))
            }
            AnnotationData::Host | AnnotationData::Email => {
                if value_kind != ResolvedTypeKind::Builtin(BuiltinType::String) {
                    return Err(out.fail(AnnotationError::Invalid(
                    diagnostic_message!("diagnostic.schema.annotation_host_email_requires_string", "@host and @email apply only to string or option<string> and take no arguments"),
                )));
                }
                if kind == AnnotationKind::Host {
                    Ok(CompiledAnnotation::Constraint(ConstraintData::Host))
                } else {
                    Ok(CompiledAnnotation::Constraint(ConstraintData::Email))
                }
            }
            AnnotationData::Cidr(argument) => {
                if value_kind != ResolvedTypeKind::Builtin(BuiltinType::String) {
                    return Err(out.fail(AnnotationError::Invalid(diagnostic_message!(
                        "diagnostic.schema.annotation_cidr_requires_string",
                        "@cidr applies only to string or option<string>"
                    ))));
                }
                Ok(CompiledAnnotation::Constraint(ConstraintData::Cidr {
                    v4_only: argument.value().value(),
                }))
            }
            AnnotationData::Size(bounds) => {
                if !matches!(
                    target_kind,
                    ResolvedTypeKind::Array { .. } | ResolvedTypeKind::Map(_)
                ) {
                    return Err(out.fail(AnnotationError::Invalid(diagnostic_message!(
                        "diagnostic.schema.annotation_size_requires_collection",
                        "@size applies only to arrays or maps"
                    ))));
                }
                let (min, max) = AnnotationBounds::parse_collection_bounds(
                    bounds,
                    context.budget,
                    out.reborrow(),
                )?;
                Ok(CompiledAnnotation::Constraint(ConstraintData::SizeBounds {
                    min,
                    max,
                }))
            }
            AnnotationData::Unique(argument) => {
                let ResolvedTypeKind::Array { element, .. } = target_kind else {
                    return Err(out.fail(AnnotationError::Invalid(diagnostic_message!(
                        "diagnostic.schema.annotation_unique_requires_array",
                        "@unique applies only to arrays"
                    ))));
                };
                let by_name = match argument {
                    Some(argument) => Some(context.budget.ast_access(
                        out.reborrow(),
                        |ast_out| argument.value().value(ast_out),
                        AnnotationError::from,
                    )?),
                    None => None,
                };
                let Some(element_kind) = AnnotationValues::resolved_kind(types, element) else {
                    return Err(out.fail(AnnotationError::internal(schema_context!(
                        "annotation-compile",
                        "unique-element-type-missing"
                    ))));
                };
                let by = if let Some(path) = by_name {
                    let ResolvedTypeKind::User(owner) = element_kind else {
                        return Err(out.fail(AnnotationError::Invalid(diagnostic_message!(
                            "diagnostic.schema.annotation_unique_by_requires_struct_array",
                            "@unique(by=...) applies only to arrays of structs"
                        ))));
                    };
                    Some(AnnotationValues::resolve_unique_path(
                        UniquePathInput {
                            path,
                            owner,
                            symbols,
                            types,
                            limits,
                        },
                        out.reborrow(),
                    )?)
                } else {
                    if matches!(element_kind, ResolvedTypeKind::User(owner) if symbols.type_symbol(owner).is_some_and(|item| matches!(item.declaration(), TypeDeclarationKind::Struct(_))))
                    {
                        return Err(out.fail(AnnotationError::Invalid(diagnostic_message!(
                            "diagnostic.schema.annotation_unique_by_required",
                            "@unique on an array of structs must specify a by field path"
                        ))));
                    }
                    None
                };
                Ok(CompiledAnnotation::Constraint(ConstraintData::Unique {
                    by,
                }))
            }
            AnnotationData::All(_) => {
                if !matches!(
                    target_kind,
                    ResolvedTypeKind::Array { .. } | ResolvedTypeKind::Map(_)
                ) {
                    return Err(out.fail(AnnotationError::Invalid(diagnostic_message!(
                        "diagnostic.schema.annotation_all_requires_collection",
                        "@all applies only to arrays or maps"
                    ))));
                }
                Ok(CompiledAnnotation::Constraint(ConstraintData::All {
                    program: ProgramId::new(usize::MAX),
                }))
            }
            AnnotationData::Sum(bounds) => {
                let ResolvedTypeKind::Array { element, .. } = target_kind else {
                    return Err(out.fail(AnnotationError::Invalid(diagnostic_message!(
                        "diagnostic.schema.annotation_sum_requires_numeric_array",
                        "@sum applies only to numeric arrays"
                    ))));
                };
                let Some(element_kind) = AnnotationValues::resolved_kind(types, element) else {
                    return Err(out.fail(AnnotationError::internal(schema_context!(
                        "annotation-compile",
                        "sum-element-type-missing"
                    ))));
                };
                let Some(builtin) = AnnotationValues::numeric_builtin(element_kind) else {
                    return Err(out.fail(AnnotationError::Invalid(diagnostic_message!(
                        "diagnostic.schema.annotation_sum_requires_numeric_array",
                        "@sum applies only to numeric arrays"
                    ))));
                };
                let (min_value, max_value) = AnnotationBounds::parse_sum_bounds(
                    bounds,
                    builtin,
                    context.budget,
                    out.reborrow(),
                )?;
                if AnnotationValues::bounds_reversed(min_value, max_value) {
                    return Err(out.fail(AnnotationError::Invalid(diagnostic_message!(
                        "diagnostic.schema.annotation_sum_bounds_reversed",
                        "the @sum minimum cannot exceed the maximum"
                    ))));
                }
                Ok(CompiledAnnotation::Constraint(ConstraintData::SumBounds {
                    min: min_value,
                    max: max_value,
                }))
            }
            AnnotationData::Variants { mode, names } => {
                let ResolvedTypeKind::User(enum_symbol) = value_kind else {
                    return Err(out.fail(AnnotationError::Invalid(diagnostic_message!(
                        "diagnostic.schema.annotation_variants_target_invalid",
                        "@variants applies only to enum or option<enum> types"
                    ))));
                };
                let Some(symbol) = symbols.type_symbol(enum_symbol) else {
                    return Err(out.fail(AnnotationError::internal(schema_context!(
                        "annotation-compile",
                        "variant-restriction-enum-missing"
                    ))));
                };
                if !matches!(symbol.declaration(), TypeDeclarationKind::Enum(_)) {
                    return Err(out.fail(AnnotationError::Invalid(diagnostic_message!(
                        "diagnostic.schema.annotation_variants_target_invalid",
                        "@variants applies only to enum or option<enum> types"
                    ))));
                }
                let mut variants =
                    ChargedVec::new(Some(context.budget.execution()), out.reborrow())?;
                for index in 0..names.len() {
                    let item = context.budget.ast_access(
                        out.reborrow(),
                        |ast_out| names.get(index, ast_out),
                        AnnotationError::from,
                    )?;
                    let Some(item) = item else {
                        return Err(out.fail(AnnotationError::internal(schema_context!(
                            "annotation-compile",
                            "variant-restriction-name-missing"
                        ))));
                    };
                    let name = context.budget.ast_access(
                        out.reborrow(),
                        |ast_out| item.value(ast_out),
                        AnnotationError::from,
                    )?;
                    context
                        .budget
                        .execution()
                        .consume(name.len(), out.reborrow())?;
                    context
                        .budget
                        .execution()
                        .consume(variants.len(), out.reborrow())?;
                    let variant = match symbols.lookup_variant::<AnnotationError>(
                        enum_symbol,
                        name,
                        out.reborrow(),
                    )? {
                        LookupResult::Found(variant) => variant,
                        LookupResult::Missing | LookupResult::Ambiguous => {
                            return Err(out.fail(AnnotationError::InvalidAt {
                                message: diagnostic_message!(
                                    "diagnostic.schema.annotation_variants_unknown_name",
                                    "@variants refers to an unknown enum variant"
                                ),
                                span: item.span(),
                            }));
                        }
                    };
                    if variants.contains(&variant) {
                        return Err(out.fail(AnnotationError::InvalidAt {
                            message: diagnostic_message!(
                                "diagnostic.schema.annotation_variants_name_repeated",
                                "@variants lists the same enum variant more than once"
                            ),
                            span: item.span(),
                        }));
                    }
                    variants.try_push(variant, out.reborrow())?;
                }
                if variants.is_empty() {
                    return Err(out.fail(AnnotationError::Invalid(diagnostic_message!(
                        "diagnostic.schema.annotation_variants_target_invalid",
                        "@variants requires at least one enum variant"
                    ))));
                }
                variants.sort_unstable_by_key(|variant| variant.index());
                let mode = match mode {
                    VariantRestrictionMode::Allow => CompiledVariantRestrictionMode::Allow,
                    VariantRestrictionMode::Deny => CompiledVariantRestrictionMode::Deny,
                };
                Ok(CompiledAnnotation::Constraint(
                    ConstraintData::VariantRestriction { mode, variants },
                ))
            }
            AnnotationData::Env(value) => {
                if !matches!(layer, ConstraintLayer::Field) {
                    return Err(out.fail(AnnotationError::Invalid(diagnostic_message!(
                        "diagnostic.schema.annotation_env_requires_field",
                        "@env can be used only on struct fields or anonymous struct variant fields"
                    ))));
                }
                let effective_builtin = match AnnotationValues::effective_builtin(types, target) {
                    Ok(result) => result,
                    Err(error) => return Err(out.fail(error)),
                };
                let Some((builtin, _)) = effective_builtin else {
                    return Err(out.fail(AnnotationError::Invalid(
                    diagnostic_message!("diagnostic.schema.annotation_env_target_invalid", "@env targets must be string, bool, a numeric type, or a single-layer option of one of these types"),
                )));
                };
                if !matches!(builtin, BuiltinType::String | BuiltinType::Bool)
                    && !AnnotationValues::is_numeric_builtin(builtin)
                {
                    return Err(out.fail(AnnotationError::Invalid(
                    diagnostic_message!("diagnostic.schema.annotation_env_target_invalid", "@env targets must be string, bool, a numeric type, or a single-layer option of one of these types"),
                )));
                }
                let name = context.budget.ast_access(
                    out.reborrow(),
                    |ast_out| value.value(ast_out),
                    AnnotationError::from,
                )?;
                context
                    .budget
                    .execution()
                    .consume(name.len(), out.reborrow())?;
                if !AnnotationValues::valid_environment_name(name) {
                    return Err(out.fail(AnnotationError::Invalid(diagnostic_message!(
                        "diagnostic.schema.annotation_env_name_invalid",
                        "@env names must follow the environment variable identifier format"
                    ))));
                }
                let owned =
                    ChargedString::copy(name, Some(context.budget.execution()), out.reborrow())?;
                Ok(CompiledAnnotation::Environment(EnvironmentRecord::new(
                    owned,
                    annotation.span(),
                )))
            }
            AnnotationData::Secret => Ok(CompiledAnnotation::Secret),
            AnnotationData::Regex(_) => {
                Err(out.fail(AnnotationError::Invalid(diagnostic_message!(
                    "diagnostic.schema.annotation_metadata_not_value_rule",
                    "metadata cannot be compiled as a value rule"
                ))))
            }
        }
    }

    pub(super) fn constraint_kind(kind: AnnotationKind) -> Option<ConstraintKind> {
        Some(match kind {
            AnnotationKind::Min => ConstraintKind::Min,
            AnnotationKind::Max => ConstraintKind::Max,
            AnnotationKind::Range => ConstraintKind::Range,
            AnnotationKind::Length => ConstraintKind::Length,
            AnnotationKind::Regex => ConstraintKind::Regex,
            AnnotationKind::Host => ConstraintKind::Host,
            AnnotationKind::Email => ConstraintKind::Email,
            AnnotationKind::Cidr => ConstraintKind::Cidr,
            AnnotationKind::Size => ConstraintKind::Size,
            AnnotationKind::Unique => ConstraintKind::Unique,
            AnnotationKind::All => ConstraintKind::All,
            AnnotationKind::Sum => ConstraintKind::Sum,
            AnnotationKind::Variants => ConstraintKind::Variants,
            AnnotationKind::Env | AnnotationKind::Secret => return None,
        })
    }
}

fn required_typed_number<'slot>(
    number: NumberLiteral<'_>,
    builtin: BuiltinType,
    context: &CompileContext,
    mut out: Out<'slot, '_, AnnotationError>,
) -> OResult<'slot, TypedNumber> {
    let value = AnnotationValues::parse_typed_number(number, builtin, context, out.reborrow())?;
    match value {
        Some(value) => Ok(value),
        None => Err(out.fail(AnnotationError::Invalid(diagnostic_message!(
            "diagnostic.schema.annotation_numeric_bound_incompatible",
            "numeric bound is incompatible with the target type"
        )))),
    }
}
