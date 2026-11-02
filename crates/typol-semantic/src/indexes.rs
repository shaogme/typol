use crate::model::{FieldId, SchemaAccessError, VariantId};
use typol_core::ChargedVec;

#[cfg(feature = "compiler")]
use typol_core::{BudgetError, DiagnosticContext, ExecutionBudget, diagnostic_context};

#[cfg(feature = "compiler")]
use opeo::{OResult, Out, ResultOutExt};

#[cfg(feature = "compiler")]
use crate::{
    builder::BuildError,
    model::{FieldRecord, TypeRecord, VariantRecord},
};

#[cfg(feature = "compiler")]
static INDEX_BUILD_INTERNAL: DiagnosticContext =
    diagnostic_context!("typol-semantic", "schema-index-build", "field-slot-missing",);
#[cfg(feature = "compiler")]
static INDEX_SORT_INVARIANT: DiagnosticContext = diagnostic_context!(
    "typol-semantic",
    "schema-index-build",
    "sorted-field-reference-missing",
);
#[cfg(feature = "compiler")]
static VARIANT_INDEX_INVARIANT: DiagnosticContext = diagnostic_context!(
    "typol-semantic",
    "schema-index-build",
    "sorted-variant-reference-missing",
);

/// The sorted index stores declaration slots only; fields themselves remain in declaration order.
pub(crate) struct SchemaIndexes {
    fields: ChargedVec<ChargedVec<usize>>,
    variants: ChargedVec<ChargedVec<usize>>,
    environments: usize,
}

impl SchemaIndexes {
    #[cfg(feature = "compiler")]
    pub(crate) fn new<'slot, E>(
        execution: &ExecutionBudget,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, Self>
    where
        E: From<BudgetError>,
    {
        Ok(Self {
            fields: ChargedVec::new(Some(execution), out.reborrow())?,
            variants: ChargedVec::new(Some(execution), out.reborrow())?,
            environments: 0,
        })
    }

    pub(crate) fn field_slots(&self, first: FieldId) -> Result<&[usize], SchemaAccessError> {
        self.fields
            .get(first.index())
            .map(ChargedVec::as_slice)
            .ok_or_else(|| SchemaAccessError::new("field index", first.index()))
    }

    pub(crate) fn variant_slots(&self, first: VariantId) -> Result<&[usize], SchemaAccessError> {
        self.variants
            .get(first.index())
            .map(ChargedVec::as_slice)
            .ok_or_else(|| SchemaAccessError::new("variant index", first.index()))
    }

    pub(crate) fn environment_count(&self) -> usize {
        self.environments
    }

    #[cfg(feature = "compiler")]
    pub(crate) fn build<'slot>(
        fields: &mut [FieldRecord],
        variant_records: &[VariantRecord],
        types: &[TypeRecord],
        execution: &ExecutionBudget,
        mut out: Out<'slot, '_, BuildError>,
    ) -> OResult<'slot, Self> {
        let mut index = ChargedVec::new(Some(execution), out.reborrow())?;
        index.try_reserve_exact(fields.len(), out.reborrow())?;
        for _ in 0..fields.len() {
            let slots = ChargedVec::new(Some(execution), out.reborrow())?;
            index.try_push(slots, out.reborrow())?;
        }
        let mut variant_index = ChargedVec::new(Some(execution), out.reborrow())?;
        variant_index.try_reserve_exact(variant_records.len(), out.reborrow())?;
        for _ in 0..variant_records.len() {
            let slots = ChargedVec::new(Some(execution), out.reborrow())?;
            variant_index.try_push(slots, out.reborrow())?;
        }
        for ty in types {
            execution.consume(1, out.reborrow())?;
            if let Some(first) = ty.record().fields.first() {
                let slots = index
                    .get_mut(first.index())
                    .ok_or(BuildError::internal(INDEX_BUILD_INTERNAL));
                let slots = slots.store_err_into(out.reborrow())?;
                slots.try_reserve_exact(ty.record().fields.len(), out.reborrow())?;
                for (slot, id) in ty.record().fields.iter().enumerate() {
                    execution.consume(1, out.reborrow())?;
                    fields
                        .get_mut(id.index())
                        .ok_or(BuildError::internal(INDEX_BUILD_INTERNAL))
                        .map(|field| field.set_slot(slot))
                        .store_err_into(out.reborrow())?;
                    slots.try_push(slot, out.reborrow())?;
                }
                slots.sort_by(
                    |left, right, mut sort_out: Out<'_, '_, BuildError>| {
                        let left_id = ty
                            .record()
                            .fields
                            .get(*left)
                            .ok_or(BudgetError::invariant(&INDEX_SORT_INVARIANT))
                            .store_err_into(sort_out.reborrow())?;
                        let right_id = ty
                            .record()
                            .fields
                            .get(*right)
                            .ok_or(BudgetError::invariant(&INDEX_SORT_INVARIANT))
                            .store_err_into(sort_out.reborrow())?;
                        let left = fields
                            .get(left_id.index())
                            .ok_or(BudgetError::invariant(&INDEX_SORT_INVARIANT))
                            .store_err_into(sort_out.reborrow())?;
                        let right = fields
                            .get(right_id.index())
                            .ok_or(BudgetError::invariant(&INDEX_SORT_INVARIANT))
                            .store_err_into(sort_out.reborrow())?;
                        let order = execution.compare_text(
                            &left.record().name,
                            &right.record().name,
                            sort_out.reborrow(),
                        )?;
                        Ok(order)
                    },
                    out.reborrow(),
                )?;
            }

            let Some(first_variant) = ty.record().variants.first() else {
                continue;
            };
            let ordered = variant_index
                .get_mut(first_variant.index())
                .ok_or(BuildError::internal(INDEX_BUILD_INTERNAL))
                .store_err_into(out.reborrow())?;
            ordered.try_reserve_exact(ty.record().variants.len(), out.reborrow())?;
            for slot in 0..ty.record().variants.len() {
                execution.consume(1, out.reborrow())?;
                ordered.try_push(slot, out.reborrow())?;
            }
            ordered.sort_by(
                |left, right, mut sort_out: Out<'_, '_, BuildError>| {
                    let left_id = ty
                        .record()
                        .variants
                        .get(*left)
                        .ok_or(BudgetError::invariant(&VARIANT_INDEX_INVARIANT))
                        .store_err_into(sort_out.reborrow())?;
                    let right_id = ty
                        .record()
                        .variants
                        .get(*right)
                        .ok_or(BudgetError::invariant(&VARIANT_INDEX_INVARIANT))
                        .store_err_into(sort_out.reborrow())?;
                    let left = variant_records
                        .get(left_id.index())
                        .ok_or(BudgetError::invariant(&VARIANT_INDEX_INVARIANT))
                        .store_err_into(sort_out.reborrow())?;
                    let right = variant_records
                        .get(right_id.index())
                        .ok_or(BudgetError::invariant(&VARIANT_INDEX_INVARIANT))
                        .store_err_into(sort_out.reborrow())?;
                    execution.compare_text(
                        &left.record().name,
                        &right.record().name,
                        sort_out.reborrow(),
                    )
                },
                out.reborrow(),
            )?;
        }
        let mut environments = ChargedVec::new(Some(execution), out.reborrow())?;
        for (id, field) in fields.iter().enumerate() {
            execution.consume(1, out.reborrow())?;
            if let Some(env) = &field.record().environment {
                environments.try_push((id, env.name()), out.reborrow())?;
            }
        }
        environments.sort_by(
            |left, right, sort_out| execution.compare_text(left.1, right.1, sort_out),
            out.reborrow(),
        )?;
        let mut count = 0;
        let mut previous = None;
        // Writes to the fields only after name borrows end, keeping name storage stable during sorting.
        let mut assignments = ChargedVec::new(Some(execution), out.reborrow())?;
        for (id, name) in environments {
            execution.consume(1, out.reborrow())?;
            let same = match previous {
                Some(prior) => execution.compare_text(prior, name, out.reborrow())?.is_eq(),
                None => false,
            };
            if !same {
                count += 1;
                previous = Some(name);
            }
            assignments.try_push((id, count - 1), out.reborrow())?;
        }
        for (id, slot) in assignments {
            execution.consume(1, out.reborrow())?;
            fields
                .get_mut(id)
                .ok_or(BuildError::internal(INDEX_BUILD_INTERNAL))
                .map(|field| field.set_environment_slot(slot))
                .store_err_into(out.reborrow())?;
        }
        Ok(Self {
            fields: index,
            variants: variant_index,
            environments: count,
        })
    }
}
