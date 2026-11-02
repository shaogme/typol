use crate::{BuildError, CompiledSchema, ShapeKind, StoredVariantPayload};
use opeo::{OResult, Out, ResultOutExt};
use typol_core::{ChargedVec, DiagnosticContext, ExecutionBudget, diagnostic_context};

static SHAPE_VALIDATION_INTERNAL: DiagnosticContext = diagnostic_context!(
    "typol-semantic",
    "shape-validation",
    "shape-graph-reference-invalid",
);

pub(crate) struct ShapeValidation;

impl ShapeValidation {
    pub(crate) fn validate<'slot>(
        schema: &CompiledSchema,
        execution: &ExecutionBudget,
        mut out: Out<'slot, '_, BuildError>,
    ) -> OResult<'slot, ()> {
        let mut edges = ChargedVec::new(Some(execution), out.reborrow())?;
        edges.try_reserve_exact(schema.records().shapes.len(), out.reborrow())?;
        for shape in &schema.records().shapes {
            execution.consume(1, out.reborrow())?;
            let mut children = ChargedVec::new(Some(execution), out.reborrow())?;
            match shape {
                ShapeKind::Builtin(_) => {}
                ShapeKind::Option(id)
                | ShapeKind::Map(id)
                | ShapeKind::Array { element: id, .. } => add(&mut children, id.0, out.reborrow())?,
                ShapeKind::Struct(owner) => {
                    let owner = schema
                        .records()
                        .types
                        .get(owner.index())
                        .ok_or(BuildError::internal(SHAPE_VALIDATION_INTERNAL))
                        .store_err_into(out.reborrow())?;
                    for field in &owner.record().fields {
                        execution.consume(1, out.reborrow())?;
                        let field = schema
                            .records()
                            .fields
                            .get(field.index())
                            .ok_or(BuildError::internal(SHAPE_VALIDATION_INTERNAL))
                            .store_err_into(out.reborrow())?;
                        let field_type = schema
                            .records()
                            .types
                            .get(field.record().ty.index())
                            .ok_or(BuildError::internal(SHAPE_VALIDATION_INTERNAL))
                            .store_err_into(out.reborrow())?;
                        add(&mut children, field_type.record().shape.0, out.reborrow())?;
                    }
                }
                ShapeKind::Enum(owner) => {
                    let owner = schema
                        .records()
                        .types
                        .get(owner.index())
                        .ok_or(BuildError::internal(SHAPE_VALIDATION_INTERNAL))
                        .store_err_into(out.reborrow())?;
                    for variant_id in &owner.record().variants {
                        execution.consume(1, out.reborrow())?;
                        let variant = schema
                            .records()
                            .variants
                            .get(variant_id.index())
                            .ok_or(BuildError::internal(SHAPE_VALIDATION_INTERNAL))
                            .store_err_into(out.reborrow())?;
                        let target = match variant.record().payload {
                            StoredVariantPayload::Unit => None,
                            StoredVariantPayload::Single(id) => Some(id),
                            StoredVariantPayload::Structure(_) => variant.record().structure_type,
                        };
                        if let Some(target) = target {
                            let target = schema
                                .records()
                                .types
                                .get(target.index())
                                .ok_or(BuildError::internal(SHAPE_VALIDATION_INTERNAL))
                                .store_err_into(out.reborrow())?;
                            add(&mut children, target.record().shape.0, out.reborrow())?;
                        }
                    }
                }
            }
            edges.try_push(children, out.reborrow())?;
        }
        let mut states = ChargedVec::new(Some(execution), out.reborrow())?;
        states.try_reserve_exact(edges.len(), out.reborrow())?;
        for _ in 0..edges.len() {
            execution.consume(1, out.reborrow())?;
            states.try_push(0_u8, out.reborrow())?;
        }
        let mut stack = ChargedVec::new(Some(execution), out.reborrow())?;
        stack.try_reserve_exact(edges.len(), out.reborrow())?;
        for root in 0..edges.len() {
            execution.consume(1, out.reborrow())?;
            let root_state = *states
                .get(root)
                .ok_or(BuildError::internal(SHAPE_VALIDATION_INTERNAL))
                .store_err_into(out.reborrow())?;
            if root_state != 0 {
                continue;
            }
            *states
                .get_mut(root)
                .ok_or(BuildError::internal(SHAPE_VALIDATION_INTERNAL))
                .store_err_into(out.reborrow())? = 1;
            stack.try_push((root, 0), out.reborrow())?;
            while let Some((node, next)) = stack.last_mut() {
                execution.consume(1, out.reborrow())?;
                let node_edges = edges
                    .get(*node)
                    .ok_or(BuildError::internal(SHAPE_VALIDATION_INTERNAL))
                    .store_err_into(out.reborrow())?;
                if let Some(child) = node_edges.get(*next) {
                    *next += 1;
                    let child_state = *states
                        .get(*child)
                        .ok_or(BuildError::internal(SHAPE_VALIDATION_INTERNAL))
                        .store_err_into(out.reborrow())?;
                    ensure(child_state != 1).store_err_into(out.reborrow())?;
                    if child_state == 0 {
                        *states
                            .get_mut(*child)
                            .ok_or(BuildError::internal(SHAPE_VALIDATION_INTERNAL))
                            .store_err_into(out.reborrow())? = 1;
                        stack.try_push((*child, 0), out.reborrow())?;
                    }
                } else {
                    *states
                        .get_mut(*node)
                        .ok_or(BuildError::internal(SHAPE_VALIDATION_INTERNAL))
                        .store_err_into(out.reborrow())? = 2;
                    stack.pop();
                }
            }
        }
        Ok(())
    }
}

fn add<'slot>(
    children: &mut ChargedVec<usize>,
    id: usize,
    mut out: Out<'slot, '_, BuildError>,
) -> OResult<'slot, ()> {
    children.try_reserve(1, out.reborrow())?;
    children.try_push(id, out.reborrow())
}

fn ensure(valid: bool) -> Result<(), BuildError> {
    if valid {
        Ok(())
    } else {
        Err(BuildError::internal(SHAPE_VALIDATION_INTERNAL))
    }
}
