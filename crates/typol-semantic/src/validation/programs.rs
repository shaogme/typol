mod nodes;

pub(super) use nodes::{PROGRAM_VALIDATION_INTERNAL, ProgramBindings, ProgramValidationState};

use crate::{
    BuildError, CompiledSchema, ConstraintKind, ConstraintRecord, ExpressionBinding,
    ExpressionNodeId, StoredConstraintData, StoredTypeKind, TypeId, TypedExpression,
    TypedExpressionKind, TypedMethodArgument,
};
use opeo::{OResult, Out, ResultOutExt};
use typol_core::ExecutionBudget;
use typol_model::BuiltinType;

pub(crate) struct ProgramValidation;

impl ProgramValidation {
    pub(crate) fn topology<'slot>(
        expression: &TypedExpression,
        execution: &ExecutionBudget,
        mut out: Out<'slot, '_, BuildError>,
    ) -> OResult<'slot, ()> {
        Self::topology_inner(expression, execution, out.reborrow())
    }

    fn topology_inner<'slot>(
        expression: &TypedExpression,
        execution: &ExecutionBudget,
        mut out: Out<'slot, '_, BuildError>,
    ) -> OResult<'slot, ()> {
        ProgramValidationState::ensure(expression.root() < expression.node_count())
            .store_err_into(out.reborrow())?;
        for (index, node) in expression.nodes().iter().enumerate() {
            execution.consume(1, out.reborrow())?;
            let child = |id: ExpressionNodeId| id.index() < index;
            let valid = match node.kind() {
                TypedExpressionKind::Literal(_) => true,
                TypedExpressionKind::Binding(ExpressionBinding::Closure(id)) => {
                    u32::try_from(id.index()).is_ok()
                }
                TypedExpressionKind::Binding(_) => true,
                TypedExpressionKind::Field { receiver, .. }
                | TypedExpressionKind::Unary {
                    operand: receiver, ..
                } => child(*receiver),
                TypedExpressionKind::Binary { left, right, .. } => child(*left) && child(*right),
                TypedExpressionKind::Method {
                    receiver, argument, ..
                } => {
                    child(*receiver)
                        && argument.as_ref().is_none_or(|a| match a {
                            TypedMethodArgument::Expression(id) => child(*id),
                            TypedMethodArgument::Closure { parameter, body } => {
                                child(*body) && u32::try_from(parameter.index()).is_ok()
                            }
                        })
                }
            };
            ProgramValidationState::ensure(valid).store_err_into(out.reborrow())?;
        }
        Ok(())
    }

    pub(crate) fn validate<'slot>(
        schema: &CompiledSchema,
        program: &TypedExpression,
        bindings: ProgramBindings,
        execution: &ExecutionBudget,
        mut out: Out<'slot, '_, BuildError>,
    ) -> OResult<'slot, ()> {
        Self::validate_inner(schema, program, bindings, execution, out.reborrow())
    }

    fn validate_inner<'slot>(
        schema: &CompiledSchema,
        program: &TypedExpression,
        bindings: ProgramBindings,
        execution: &ExecutionBudget,
        mut out: Out<'slot, '_, BuildError>,
    ) -> OResult<'slot, ()> {
        Self::topology_inner(program, execution, out.reborrow())?;
        ProgramValidationState::validate(schema, program, bindings, execution, out.reborrow())
    }

    pub(crate) fn validate_rule<'slot>(
        schema: &CompiledSchema,
        rule: &ConstraintRecord,
        target: TypeId,
        execution: &ExecutionBudget,
        mut out: Out<'slot, '_, BuildError>,
    ) -> OResult<'slot, ()> {
        Self::validate_rule_inner(schema, rule, target, execution, out.reborrow())
    }

    fn validate_rule_inner<'slot>(
        schema: &CompiledSchema,
        rule: &ConstraintRecord,
        target: TypeId,
        execution: &ExecutionBudget,
        mut out: Out<'slot, '_, BuildError>,
    ) -> OResult<'slot, ()> {
        execution.consume(1, out.reborrow())?;
        let type_info = schema.type_info(target).store_err_into(out.reborrow())?;
        let type_info = type_info
            .ok_or(BuildError::internal(PROGRAM_VALIDATION_INTERNAL))
            .store_err_into(out.reborrow())?;
        let kind = type_info.kind();
        let valid = match (rule.kind(), rule.data()) {
            (
                ConstraintKind::Min | ConstraintKind::Max | ConstraintKind::Range,
                StoredConstraintData::NumberBounds { min, max },
            ) => {
                if let Some(builtin) = ProgramValidationState::builtin(schema, target) {
                    ProgramValidationState::numeric(builtin)
                        && ProgramValidationState::valid_bounds(builtin, *min, *max)
                } else {
                    false
                }
            }
            (ConstraintKind::Length, StoredConstraintData::LengthBounds { min, max }) => {
                ProgramValidationState::is_builtin(schema, target, BuiltinType::String)
                    && min <= max
            }
            (ConstraintKind::Regex, StoredConstraintData::Regex { regex }) => {
                ProgramValidationState::is_builtin(schema, target, BuiltinType::String)
                    && regex.index() < schema.records().regexes.constraints().len()
            }
            (ConstraintKind::Host, StoredConstraintData::Host)
            | (ConstraintKind::Email, StoredConstraintData::Email)
            | (ConstraintKind::Cidr, StoredConstraintData::Cidr { .. }) => {
                ProgramValidationState::is_builtin(schema, target, BuiltinType::String)
            }
            (ConstraintKind::Size, StoredConstraintData::SizeBounds { min, max }) => {
                matches!(
                    kind,
                    StoredTypeKind::Array { .. } | StoredTypeKind::Map { .. }
                ) && !matches!((min,max),(Some(min),Some(max)) if min>max)
            }
            (ConstraintKind::Unique, StoredConstraintData::Unique { by }) => {
                let StoredTypeKind::Array { element, .. } = kind else {
                    return Err(out.fail(BuildError::internal(PROGRAM_VALIDATION_INTERNAL)));
                };
                let mut ty = element;
                if let Some(path) = by {
                    ProgramValidationState::ensure(!path.is_empty())
                        .store_err_into(out.reborrow())?;
                    for field in path {
                        execution.consume(1, out.reborrow())?;
                        let type_info = schema.type_info(ty).store_err_into(out.reborrow())?;
                        let info = type_info
                            .ok_or(BuildError::internal(PROGRAM_VALIDATION_INTERNAL))
                            .store_err_into(out.reborrow())?;
                        execution.consume(info.fields().len(), out.reborrow())?;
                        ProgramValidationState::ensure(info.fields().contains(field))
                            .store_err_into(out.reborrow())?;
                        let field_info =
                            schema.field_info(*field).store_err_into(out.reborrow())?;
                        ty = field_info
                            .ok_or(BuildError::internal(PROGRAM_VALIDATION_INTERNAL))
                            .store_err_into(out.reborrow())?
                            .ty();
                    }
                }
                if by.is_some() {
                    ProgramValidationState::comparable(schema, ty, execution, out.reborrow())?
                } else {
                    let type_info = schema.type_info(ty).store_err_into(out.reborrow())?;
                    let info = type_info
                        .ok_or(BuildError::internal(PROGRAM_VALIDATION_INTERNAL))
                        .store_err_into(out.reborrow())?;
                    !matches!(info.kind(), StoredTypeKind::Struct { .. })
                }
            }
            (ConstraintKind::All, StoredConstraintData::All { program }) => {
                let program = schema
                    .records()
                    .programs
                    .get(program.index())
                    .ok_or(BuildError::internal(PROGRAM_VALIDATION_INTERNAL))
                    .store_err_into(out.reborrow())?;
                let bindings =
                    ProgramBindings::all(schema, target).store_err_into(out.reborrow())?;
                Self::validate(
                    schema,
                    program.expression(),
                    bindings,
                    execution,
                    out.reborrow(),
                )?;
                true
            }
            (ConstraintKind::Sum, StoredConstraintData::SumBounds { min, max }) => match kind {
                StoredTypeKind::Array { element, .. } => {
                    ProgramValidationState::builtin(schema, element).is_some_and(|builtin| {
                        ProgramValidationState::numeric(builtin)
                            && ProgramValidationState::valid_bounds(builtin, *min, *max)
                    })
                }
                _ => false,
            },
            _ => false,
        };
        ProgramValidationState::ensure(valid).store_err_into(out.reborrow())?;
        Ok(())
    }
}
