mod defaults;
mod programs;
mod shapes;

use self::{
    defaults::{DefaultValidation, DefaultValidationInput},
    programs::{ProgramBindings, ProgramValidation},
    shapes::ShapeValidation,
};
use crate::{
    BuildError, CompiledSchema, ConstraintList, RefinementId, ShapeKind, SourceLookup,
    StoredConstraintData, StoredTypeKind, StoredVariantPayload, TypeId, TypedExpression,
};
use core::cmp::Ordering;
use opeo::{ErrSlot, OResult, Out, ResultOutExt};
use typol_core::{
    ChargedVec, DiagnosticContext, ExecutionBudget, RegexExecutionBudget, SourceSpan,
    diagnostic_context,
};
use typol_policy::{RegexLimits, ValueLimits};

static SCHEMA_VALIDATION_INTERNAL: DiagnosticContext = diagnostic_context!(
    "typol-semantic",
    "schema-validation",
    "stored-schema-invariant-failed",
);

pub(crate) struct Validation;

pub(crate) struct SchemaValidationRequest<'a, S: SourceLookup + ?Sized> {
    pub(crate) schema: &'a CompiledSchema,
    pub(crate) sources: &'a S,
    pub(crate) execution: &'a ExecutionBudget,
    pub(crate) regex_execution: &'a RegexExecutionBudget,
    pub(crate) value_limits: &'a ValueLimits,
    pub(crate) regex_limits: &'a RegexLimits,
    pub(crate) validate_defaults: bool,
}

impl Validation {
    fn validate_default<'slot>(
        input: DefaultValidationInput<'_>,
        source_span: SourceSpan,
        out: Out<'slot, '_, BuildError>,
    ) -> OResult<'slot, ()> {
        let mut validation_errors = ErrSlot::<BuildError>::new();
        if let Err(mut caught) = validation_errors
            .try_call(|validation_out| DefaultValidation::validate(input, validation_out))
        {
            let error = *caught.get();
            *caught.get_mut() = error.with_source_span(source_span);
            return Err(out.fail(caught.take()));
        }
        Ok(())
    }

    pub(crate) fn program_topology<'slot>(
        expression: &TypedExpression,
        execution: &ExecutionBudget,
        mut out: Out<'slot, '_, BuildError>,
    ) -> OResult<'slot, ()> {
        ProgramValidation::topology(expression, execution, out.reborrow())
    }

    pub(crate) fn validate<'slot, S: SourceLookup + ?Sized>(
        request: SchemaValidationRequest<'_, S>,
        mut out: Out<'slot, '_, BuildError>,
    ) -> OResult<'slot, ()> {
        let SchemaValidationRequest {
            schema,
            sources,
            execution,
            regex_execution,
            value_limits,
            regex_limits,
            validate_defaults,
        } = request;
        let mut validation_errors = ErrSlot::<BuildError>::new();
        let entry = schema
            .records()
            .types
            .get(schema.entry_type().index())
            .ok_or(BuildError::internal(SCHEMA_VALIDATION_INTERNAL))
            .store_err_into(out.reborrow())?;
        ensure(
                matches!(schema.records().shapes.get(entry.record().shape.0), Some(ShapeKind::Struct(owner)) if *owner == schema.entry_type()),
            ).store_err_into(out.reborrow())?;
        for (index, shape) in schema.records().shapes.iter().enumerate() {
            execution.consume(1, out.reborrow())?;
            match *shape {
                ShapeKind::Option(child)
                | ShapeKind::Map(child)
                | ShapeKind::Array { element: child, .. } => {
                    ensure(child.0 < index).store_err_into(out.reborrow())?;
                    let child_shape = schema
                        .records()
                        .shapes
                        .get(child.0)
                        .ok_or(BuildError::internal(SCHEMA_VALIDATION_INTERNAL))
                        .store_err_into(out.reborrow())?;
                    ensure(!matches!(
                        (shape, child_shape),
                        (ShapeKind::Option(_), ShapeKind::Option(_))
                    ))
                    .store_err_into(out.reborrow())?;
                }
                ShapeKind::Struct(owner) | ShapeKind::Enum(owner) => {
                    ensure(owner.index() < schema.records().types.len())
                        .store_err_into(out.reborrow())?
                }
                ShapeKind::Builtin(_) => {}
            }
        }
        for record in &schema.records().types {
            execution.consume(1, out.reborrow())?;
            let shape = schema
                .records()
                .shapes
                .get(record.record().shape.0)
                .ok_or(BuildError::internal(SCHEMA_VALIDATION_INTERNAL))
                .store_err_into(out.reborrow())?;
            let child_shape = match shape {
                ShapeKind::Option(child)
                | ShapeKind::Map(child)
                | ShapeKind::Array { element: child, .. } => Some(*child),
                _ => None,
            };
            ensure(child_shape.is_none() == record.record().child.is_none())
                .store_err_into(out.reborrow())?;
            ensure(
                child_shape
                    == record.record().child.and_then(|id| {
                        schema
                            .records()
                            .types
                            .get(id.index())
                            .map(|r| r.record().shape)
                    }),
            )
            .store_err_into(out.reborrow())?;
            if let ShapeKind::Struct(owner) | ShapeKind::Enum(owner) = shape {
                let owner = schema
                    .records()
                    .types
                    .get(owner.index())
                    .ok_or(BuildError::internal(SCHEMA_VALIDATION_INTERNAL))
                    .store_err_into(out.reborrow())?;
                ensure(owner.record().shape == record.record().shape)
                    .store_err_into(out.reborrow())?;
            }
        }
        for record in &schema.records().constraints {
            execution.consume(1, out.reborrow())?;
            match record.data() {
                StoredConstraintData::All { program } => {
                    ensure(program.index() < schema.records().programs.len())
                        .store_err_into(out.reborrow())?
                }
                StoredConstraintData::Regex { regex } => {
                    ensure(regex.index() < schema.records().regexes.constraints().len())
                        .store_err_into(out.reborrow())?
                }
                _ => {}
            }
        }
        for (index, segment) in schema.records().refinements.iter().enumerate() {
            execution.consume(1, out.reborrow())?;
            ensure(segment.next.is_none_or(|id| id.0 < index)).store_err_into(out.reborrow())?;
            let rule_count = segment
                .rules
                .len()
                .checked_add(segment.tail.len())
                .ok_or(BuildError::Resource)
                .store_err_into(out.reborrow())?;
            execution.consume(rule_count, out.reborrow())?;
            ensure(
                segment
                    .rules
                    .iter()
                    .chain(&segment.tail)
                    .all(|id| id.0 < schema.records().constraints.len()),
            )
            .store_err_into(out.reborrow())?;
            let target_info = schema
                .type_info(segment.target)
                .store_err_into(out.reborrow())?;
            let target = target_info
                .ok_or(BuildError::internal(SCHEMA_VALIDATION_INTERNAL))
                .store_err_into(out.reborrow())?;
            if let Some(next) = segment.next {
                let next = schema
                    .records()
                    .refinements
                    .get(next.0)
                    .ok_or(BuildError::internal(SCHEMA_VALIDATION_INTERNAL))
                    .store_err_into(out.reborrow())?;
                let same_type = schema
                    .same_type(segment.target, next.target)
                    .store_err_into(out.reborrow())?;
                ensure(next.tail.is_empty() && same_type).store_err_into(out.reborrow())?;
            }
            let mut ty = segment.target;
            if let StoredTypeKind::Option { inner } = target.kind() {
                ty = inner;
            }
            for id in segment.rules.iter().chain(&segment.tail) {
                execution.consume(1, out.reborrow())?;
                let rule = schema
                    .records()
                    .constraints
                    .get(id.0)
                    .ok_or(BuildError::internal(SCHEMA_VALIDATION_INTERNAL))
                    .store_err_into(out.reborrow())?;
                ProgramValidation::validate_rule(schema, rule, ty, execution, out.reborrow())?;
            }
        }
        let mut owners = Self::flags(schema.records().fields.len(), execution, out.reborrow())?;
        let mut variant_owners =
            Self::flags(schema.records().variants.len(), execution, out.reborrow())?;
        for (index, record) in schema.records().types.iter().enumerate() {
            execution.consume(1, out.reborrow())?;
            let shape = schema
                .records()
                .shapes
                .get(record.record().shape.0)
                .ok_or(BuildError::internal(SCHEMA_VALIDATION_INTERNAL))
                .store_err_into(out.reborrow())?;
            ensure(
                record.record().fields.is_empty()
                    || matches!(shape,ShapeKind::Struct(owner) if owner.index()==index),
            )
            .store_err_into(out.reborrow())?;
            ensure(
                record.record().variants.is_empty()
                    || matches!(shape,ShapeKind::Enum(owner) if owner.index()==index),
            )
            .store_err_into(out.reborrow())?;
            for id in &record.record().fields {
                execution.consume(1, out.reborrow())?;
                claim(&mut owners, id.index()).store_err_into(out.reborrow())?;
            }
            for id in &record.record().variants {
                execution.consume(1, out.reborrow())?;
                claim(&mut variant_owners, id.index()).store_err_into(out.reborrow())?;
            }
            validate_refinement(
                schema,
                record.record().refinement,
                TypeId::new(index),
                out.reborrow(),
            )?;
            if !record.record().assertions.is_empty() {
                ensure(matches!(shape,ShapeKind::Struct(owner) if owner.index()==index))
                    .store_err_into(out.reborrow())?;
            }
            for id in &record.record().assertions {
                execution.consume(1, out.reborrow())?;
                let program = schema
                    .records()
                    .programs
                    .get(id.index())
                    .ok_or(BuildError::internal(SCHEMA_VALIDATION_INTERNAL))
                    .store_err_into(out.reborrow())?;
                let expression = program.expression();
                if let Err(mut caught) = validation_errors.try_call(|validation_out| {
                    ProgramValidation::validate(
                        schema,
                        expression,
                        ProgramBindings::assertion(Some(TypeId::new(index))),
                        execution,
                        validation_out,
                    )
                }) {
                    let error = *caught.get();
                    *caught.get_mut() = error.with_source_span(expression.span());
                    return Err(out.fail(caught.take()));
                }
            }
        }
        let mut names = ChargedVec::new(Some(execution), out.reborrow())?;
        for record in &schema.records().types {
            execution.consume(1, out.reborrow())?;
            names.clear();
            names.try_reserve(
                record
                    .record()
                    .fields
                    .len()
                    .max(record.record().variants.len()),
                out.reborrow(),
            )?;
            for id in &record.record().fields {
                execution.consume(1, out.reborrow())?;
                let field = schema
                    .records()
                    .fields
                    .get(id.index())
                    .ok_or(BuildError::internal(SCHEMA_VALIDATION_INTERNAL))
                    .store_err_into(out.reborrow())?;
                names.try_push(field.record().name.as_str(), out.reborrow())?;
            }
            names.sort_by(
                |left, right, sort_out| execution.compare_text(left, right, sort_out),
                out.reborrow(),
            )?;
            for pair in names.windows(2) {
                execution.consume(1, out.reborrow())?;
                let [left, right] = pair else {
                    return Err(out.fail(BuildError::internal(SCHEMA_VALIDATION_INTERNAL)));
                };
                let ordering = execution.compare_text(left, right, out.reborrow())?;
                ensure(!ordering.is_eq()).store_err_into(out.reborrow())?;
            }
            names.clear();
            for id in &record.record().variants {
                execution.consume(1, out.reborrow())?;
                let variant = schema
                    .records()
                    .variants
                    .get(id.index())
                    .ok_or(BuildError::internal(SCHEMA_VALIDATION_INTERNAL))
                    .store_err_into(out.reborrow())?;
                names.try_push(variant.record().name.as_str(), out.reborrow())?;
            }
            names.sort_by(
                |left, right, sort_out| execution.compare_text(left, right, sort_out),
                out.reborrow(),
            )?;
            for pair in names.windows(2) {
                execution.consume(1, out.reborrow())?;
                let [left, right] = pair else {
                    return Err(out.fail(BuildError::internal(SCHEMA_VALIDATION_INTERNAL)));
                };
                let ordering = execution.compare_text(left, right, out.reborrow())?;
                ensure(!ordering.is_eq()).store_err_into(out.reborrow())?;
            }
        }
        for field in &schema.records().fields {
            execution.consume(1, out.reborrow())?;
            let type_info = schema
                .type_info(field.record().ty)
                .store_err_into(out.reborrow())?;
            type_info
                .ok_or_else(|| {
                    BuildError::internal(SCHEMA_VALIDATION_INTERNAL)
                        .with_source_span(field.record().span)
                })
                .store_err_into(out.reborrow())?;
            validate_refinement(
                schema,
                field.record().refinement,
                field.record().ty,
                out.reborrow(),
            )?;
            if validate_defaults {
                if let Some(value) = &field.record().default {
                    Self::validate_default(
                        DefaultValidationInput {
                            schema,
                            value,
                            ty: field.record().ty,
                            constraints: ConstraintList::new(schema, field.record().refinement),
                            execution,
                            regex_execution,
                            value_limits,
                            regex_limits,
                        },
                        field.record().span,
                        out.reborrow(),
                    )?;
                }
            }
            if field.record().environment.is_some() {
                let info = schema
                    .type_info(field.record().ty)
                    .store_err_into(out.reborrow())?;
                let info = info
                    .ok_or(BuildError::internal(SCHEMA_VALIDATION_INTERNAL))
                    .store_err_into(out.reborrow())?;
                let kind = match info.kind() {
                    StoredTypeKind::Option { inner } => {
                        let info = schema.type_info(inner).store_err_into(out.reborrow())?;
                        let info = info
                            .ok_or(BuildError::internal(SCHEMA_VALIDATION_INTERNAL))
                            .store_err_into(out.reborrow())?;
                        info.kind()
                    }
                    kind => kind,
                };
                ensure(matches!(kind, StoredTypeKind::Builtin(_)))
                    .store_err_into(out.reborrow())?;
            }
        }
        for (index, variant) in schema.records().variants.iter().enumerate() {
            execution.consume(1, out.reborrow())?;
            let owned = variant_owners
                .get(index)
                .ok_or(BuildError::internal(SCHEMA_VALIDATION_INTERNAL))
                .store_err_into(out.reborrow())?;
            ensure(*owned).store_err_into(out.reborrow())?;
            let self_type = match &variant.record().payload {
                StoredVariantPayload::Single(id) => {
                    let type_info = schema.type_info(*id).store_err_into(out.reborrow())?;
                    type_info
                        .ok_or(BuildError::internal(SCHEMA_VALIDATION_INTERNAL))
                        .store_err_into(out.reborrow())?;
                    validate_refinement(schema, variant.record().refinement, *id, out.reborrow())?;
                    ensure(variant.record().structure_type.is_none())
                        .store_err_into(out.reborrow())?;
                    Some(*id)
                }
                StoredVariantPayload::Structure(ids) => {
                    execution.consume(ids.len(), out.reborrow())?;
                    let ty = variant
                        .record()
                        .structure_type
                        .ok_or(BuildError::internal(SCHEMA_VALIDATION_INTERNAL))
                        .store_err_into(out.reborrow())?;
                    let type_info = schema.type_info(ty).store_err_into(out.reborrow())?;
                    let info = type_info
                        .ok_or(BuildError::internal(SCHEMA_VALIDATION_INTERNAL))
                        .store_err_into(out.reborrow())?;
                    ensure(matches!(
                        info.kind(),
                        StoredTypeKind::Struct { fields } if fields == ids.as_slice()
                    ))
                    .store_err_into(out.reborrow())?;
                    ensure(variant.record().refinement.is_none()).store_err_into(out.reborrow())?;
                    Some(ty)
                }
                StoredVariantPayload::Unit => {
                    ensure(
                        variant.record().refinement.is_none()
                            && variant.record().structure_type.is_none(),
                    )
                    .store_err_into(out.reborrow())?;
                    None
                }
            };
            for id in &variant.record().assertions {
                execution.consume(1, out.reborrow())?;
                let program = schema
                    .records()
                    .programs
                    .get(id.index())
                    .ok_or(BuildError::internal(SCHEMA_VALIDATION_INTERNAL))
                    .store_err_into(out.reborrow())?;
                let expression = program.expression();
                if let Err(mut caught) = validation_errors.try_call(|validation_out| {
                    ProgramValidation::validate(
                        schema,
                        expression,
                        ProgramBindings::assertion(self_type),
                        execution,
                        validation_out,
                    )
                }) {
                    let error = *caught.get();
                    *caught.get_mut() = error.with_source_span(expression.span());
                    return Err(out.fail(caught.take()));
                }
            }
        }
        execution.consume(owners.len(), out.reborrow())?;
        ensure(owners.iter().all(|owner| *owner)).store_err_into(out.reborrow())?;
        ShapeValidation::validate(schema, execution, out.reborrow())?;
        for pair in schema.records().locations.windows(2) {
            execution.consume(1, out.reborrow())?;
            execution.consume(2, out.reborrow())?;
            let left = pair
                .first()
                .ok_or(BuildError::internal(SCHEMA_VALIDATION_INTERNAL))
                .store_err_into(out.reborrow())?;
            let right = pair
                .get(1)
                .ok_or(BuildError::internal(SCHEMA_VALIDATION_INTERNAL))
                .store_err_into(out.reborrow())?;
            ensure(compare_spans(left.0, right.0).is_lt()).store_err_into(out.reborrow())?;
        }
        for span in Self::collect_spans(schema, execution, out.reborrow())? {
            execution.consume(1, out.reborrow())?;
            ensure(sources.is_valid_span(span)).store_err_into(out.reborrow())?;

            let location = schema.span_location_budgeted(span, execution, out.reborrow())?;
            ensure(
                (span.source_id().get() as usize) < sources.source_count() && location.is_some(),
            )
            .store_err_into(out.reborrow())?;
        }
        Ok(())
    }

    pub(crate) fn collect_spans<'slot>(
        schema: &CompiledSchema,
        execution: &ExecutionBudget,
        mut out: Out<'slot, '_, BuildError>,
    ) -> OResult<'slot, ChargedVec<SourceSpan>> {
        let records = schema.records();
        let mut spans = ChargedVec::new(Some(execution), out.reborrow())?;
        for record in &records.types {
            spans.try_push(record.record().span, out.reborrow())?;
        }
        for record in &records.fields {
            spans.try_push(record.record().span, out.reborrow())?;
            if let Some(environment) = &record.record().environment {
                spans.try_push(environment.span(), out.reborrow())?;
            }
        }
        for record in &records.variants {
            spans.try_push(record.record().span, out.reborrow())?;
        }
        for record in &records.constraints {
            spans.try_push(record.span(), out.reborrow())?;
        }
        for program in &records.programs {
            spans.try_push(program.span(), out.reborrow())?;
            spans.try_push(program.expression().span(), out.reborrow())?;
            for node in program.expression().nodes() {
                spans.try_push(node.span(), out.reborrow())?;
            }
        }
        for regex in records.regexes.constraints() {
            spans.try_push(regex.span(), out.reborrow())?;
        }
        Ok(spans)
    }

    fn flags<'slot>(
        count: usize,
        execution: &ExecutionBudget,
        mut out: Out<'slot, '_, BuildError>,
    ) -> OResult<'slot, ChargedVec<bool>> {
        let mut flags = ChargedVec::new(Some(execution), out.reborrow())?;
        flags.try_reserve_exact(count, out.reborrow())?;
        for _ in 0..count {
            execution.consume(1, out.reborrow())?;
            flags.try_push(false, out.reborrow())?;
        }
        Ok(flags)
    }
}

fn validate_refinement<'slot>(
    schema: &CompiledSchema,
    head: Option<RefinementId>,
    ty: TypeId,
    mut out: Out<'slot, '_, BuildError>,
) -> OResult<'slot, ()> {
    if let Some(id) = head {
        let segment = schema
            .records()
            .refinements
            .get(id.0)
            .ok_or(BuildError::internal(SCHEMA_VALIDATION_INTERNAL))
            .store_err_into(out.reborrow())?;
        let same_type = schema
            .same_type(segment.target, ty)
            .store_err_into(out.reborrow())?;
        ensure(same_type).store_err_into(out.reborrow())?;
    }
    Ok(())
}

fn claim(owners: &mut [bool], index: usize) -> Result<(), BuildError> {
    let owner = owners
        .get_mut(index)
        .ok_or(BuildError::internal(SCHEMA_VALIDATION_INTERNAL))?;
    ensure(!*owner)?;
    *owner = true;
    Ok(())
}

fn ensure(valid: bool) -> Result<(), BuildError> {
    if valid {
        Ok(())
    } else {
        Err(BuildError::internal(SCHEMA_VALIDATION_INTERNAL))
    }
}

fn compare_spans(left: SourceSpan, right: SourceSpan) -> Ordering {
    left.source_id()
        .get()
        .cmp(&right.source_id().get())
        .then_with(|| left.span().start().cmp(&right.span().start()))
        .then_with(|| left.span().end().cmp(&right.span().end()))
}
