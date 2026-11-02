use crate::{
    BuildError, ClosureLocalId, CompiledSchema, ExpressionBinding, ExpressionMethod,
    ExpressionNodeId, FieldId, NumberValue, StoredTypeKind, TypeId, TypedExpression,
    TypedExpressionKind, TypedLiteral, TypedMethodArgument,
    validation::defaults::DefaultValidation,
};
use opeo::{OResult, Out, ResultOutExt};
use typol_core::{ChargedVec, DiagnosticContext, ExecutionBudget, diagnostic_context};
use typol_model::{BinaryOperator, BuiltinType, UnaryOperator};

pub(in crate::validation) static PROGRAM_VALIDATION_INTERNAL: DiagnosticContext = diagnostic_context!(
    "typol-semantic",
    "expression-validation",
    "typed-program-invariant-failed",
);

#[derive(Clone, Copy)]
pub(in crate::validation) struct ProgramBindings {
    self_type: Option<TypeId>,
    root: bool,
    item: Option<TypeId>,
    value: Option<TypeId>,
    key: bool,
}

impl ProgramBindings {
    pub(in crate::validation) fn assertion(self_type: Option<TypeId>) -> Self {
        Self {
            self_type,
            root: true,
            item: None,
            value: None,
            key: false,
        }
    }

    pub(super) fn all(schema: &CompiledSchema, target: TypeId) -> Result<Self, BuildError> {
        let mut result = Self {
            self_type: None,
            root: false,
            item: None,
            value: None,
            key: false,
        };
        match schema
            .type_info(target)?
            .ok_or(BuildError::internal(PROGRAM_VALIDATION_INTERNAL))?
            .kind()
        {
            StoredTypeKind::Array { element, .. } => result.item = Some(element),
            StoredTypeKind::Map { value } => {
                result.value = Some(value);
                result.key = true;
            }
            _ => return Err(BuildError::internal(PROGRAM_VALIDATION_INTERNAL)),
        };
        Ok(result)
    }
}

struct Scope {
    parameter: ClosureLocalId,
    ty: TypeId,
    parent: Option<usize>,
}

struct Pending {
    index: usize,
    scope: Option<usize>,
    allow_magnitude: bool,
}

#[derive(Clone, Copy)]
struct NodeValidation<'schema> {
    ty: TypeId,
    type_kind: StoredTypeKind<'schema>,
    scope: Option<usize>,
    allow_magnitude: bool,
}

#[derive(Clone, Copy)]
struct BinaryOperation {
    operator: BinaryOperator,
    left: ExpressionNodeId,
    right: ExpressionNodeId,
}

#[derive(Clone, Copy)]
struct MethodOperation<'a> {
    receiver: ExpressionNodeId,
    method: ExpressionMethod,
    argument: &'a Option<TypedMethodArgument>,
}

pub(in crate::validation) struct ProgramValidationState<'schema, 'program, 'execution> {
    schema: &'schema CompiledSchema,
    program: &'program TypedExpression,
    bindings: ProgramBindings,
    execution: &'execution ExecutionBudget,
    visited: ChargedVec<bool>,
    scopes: ChargedVec<Scope>,
    pending: ChargedVec<Pending>,
}

impl<'schema, 'program, 'execution> ProgramValidationState<'schema, 'program, 'execution> {
    pub(super) fn validate<'slot>(
        schema: &'schema CompiledSchema,
        program: &'program TypedExpression,
        bindings: ProgramBindings,
        execution: &'execution ExecutionBudget,
        mut out: Out<'slot, '_, BuildError>,
    ) -> OResult<'slot, ()> {
        let root = program
            .nodes()
            .get(program.root())
            .ok_or(BuildError::internal(PROGRAM_VALIDATION_INTERNAL))
            .store_err_into(out.reborrow())?;
        let same_type = schema
            .same_type(program.ty(), root.ty())
            .store_err_into(out.reborrow())?;
        Self::ensure(Self::is_builtin(schema, program.ty(), BuiltinType::Bool) && same_type)
            .store_err_into(out.reborrow())?;
        let mut state = Self {
            schema,
            program,
            bindings,
            execution,
            visited: Self::flags(program.node_count(), execution, out.reborrow())?,
            scopes: ChargedVec::new(Some(execution), out.reborrow())?,
            pending: ChargedVec::new(Some(execution), out.reborrow())?,
        };
        state.schedule(
            Pending {
                index: program.root(),
                scope: None,
                allow_magnitude: false,
            },
            out.reborrow(),
        )?;
        state.run(out.reborrow())
    }

    fn run<'slot>(&mut self, mut out: Out<'slot, '_, BuildError>) -> OResult<'slot, ()> {
        while let Some(task) = self.pending.pop() {
            self.execution.consume(1, out.reborrow())?;
            self.validate_node(task, out.reborrow())?;
        }
        self.execution.consume(self.visited.len(), out.reborrow())?;
        Self::ensure(self.visited.iter().all(|visited| *visited)).store_err_into(out.reborrow())?;
        Ok(())
    }

    fn validate_node<'slot>(
        &mut self,
        task: Pending,
        mut out: Out<'slot, '_, BuildError>,
    ) -> OResult<'slot, ()> {
        let program = self.program;
        let schema = self.schema;
        let node = program
            .nodes()
            .get(task.index)
            .ok_or(BuildError::internal(PROGRAM_VALIDATION_INTERNAL))
            .store_err_into(out.reborrow())?;
        *self
            .visited
            .get_mut(task.index)
            .ok_or(BuildError::internal(PROGRAM_VALIDATION_INTERNAL))
            .store_err_into(out.reborrow())? = true;
        let node_info = schema.type_info(node.ty()).store_err_into(out.reborrow())?;
        let type_kind = node_info
            .ok_or(BuildError::internal(PROGRAM_VALIDATION_INTERNAL))
            .store_err_into(out.reborrow())?
            .kind();
        let validation = NodeValidation {
            ty: node.ty(),
            type_kind,
            scope: task.scope,
            allow_magnitude: task.allow_magnitude,
        };
        match node.kind() {
            TypedExpressionKind::Literal(literal) => {
                self.validate_literal(validation, literal, out.reborrow())
            }
            TypedExpressionKind::Binding(binding) => {
                self.validate_binding(validation, *binding, out.reborrow())
            }
            TypedExpressionKind::Field { receiver, field } => {
                self.validate_field(validation, *receiver, *field, out.reborrow())
            }
            TypedExpressionKind::Unary { operator, operand } => {
                self.validate_unary(validation, *operator, *operand, out.reborrow())
            }
            TypedExpressionKind::Binary {
                operator,
                left,
                right,
            } => self.validate_binary(
                validation,
                BinaryOperation {
                    operator: *operator,
                    left: *left,
                    right: *right,
                },
                out.reborrow(),
            ),
            TypedExpressionKind::Method {
                receiver,
                method,
                argument,
            } => self.validate_method(
                validation,
                MethodOperation {
                    receiver: *receiver,
                    method: *method,
                    argument,
                },
                out.reborrow(),
            ),
        }
    }

    #[inline(never)]
    fn validate_literal<'slot>(
        &mut self,
        node: NodeValidation<'_>,
        literal: &TypedLiteral,
        mut out: Out<'slot, '_, BuildError>,
    ) -> OResult<'slot, ()> {
        let valid = match literal {
            TypedLiteral::String(_) => Self::is_builtin(self.schema, node.ty, BuiltinType::String),
            TypedLiteral::Boolean(_) => Self::is_builtin(self.schema, node.ty, BuiltinType::Bool),
            TypedLiteral::None => matches!(node.type_kind, StoredTypeKind::Option { .. }),
            TypedLiteral::Float(value) => {
                value.is_finite()
                    && (Self::is_builtin(self.schema, node.ty, BuiltinType::F64)
                        || (Self::is_builtin(self.schema, node.ty, BuiltinType::F32)
                            && f64::from(*value as f32) == *value))
            }
            TypedLiteral::Integer(value) => match Self::builtin(self.schema, node.ty) {
                Some(builtin) => {
                    Self::integer_literal(builtin, *value)
                        && (node.allow_magnitude
                            || i128::try_from(*value)
                                .is_ok_and(|value| DefaultValidation::signed_fits(builtin, value))
                            || DefaultValidation::unsigned_fits(builtin, *value))
                }
                None => false,
            },
        };
        Self::ensure(valid).store_err_into(out.reborrow())
    }

    #[inline(never)]
    fn validate_binding<'slot>(
        &mut self,
        node: NodeValidation<'_>,
        binding: ExpressionBinding,
        mut out: Out<'slot, '_, BuildError>,
    ) -> OResult<'slot, ()> {
        let ty = match binding {
            ExpressionBinding::SelfValue => self.bindings.self_type,
            ExpressionBinding::Root if self.bindings.root => Some(self.schema.entry_type()),
            ExpressionBinding::Item => self.bindings.item,
            ExpressionBinding::Value => self.bindings.value,
            ExpressionBinding::Key if self.bindings.key => {
                Self::ensure(Self::is_builtin(self.schema, node.ty, BuiltinType::String))
                    .store_err_into(out.reborrow())?;
                Some(node.ty)
            }
            ExpressionBinding::Closure(parameter) => {
                self.scope_type(node.scope, parameter, out.reborrow())?
            }
            _ => None,
        }
        .ok_or(BuildError::internal(PROGRAM_VALIDATION_INTERNAL))
        .store_err_into(out.reborrow())?;
        let same_type = self
            .schema
            .same_type(ty, node.ty)
            .store_err_into(out.reborrow())?;
        Self::ensure(same_type).store_err_into(out.reborrow())
    }

    #[inline(never)]
    fn validate_field<'slot>(
        &mut self,
        node: NodeValidation<'_>,
        receiver: ExpressionNodeId,
        field_id: FieldId,
        mut out: Out<'slot, '_, BuildError>,
    ) -> OResult<'slot, ()> {
        let receiver_ty = self.child_type(receiver).store_err_into(out.reborrow())?;
        let receiver_type_info = self
            .schema
            .type_info(receiver_ty)
            .store_err_into(out.reborrow())?;
        let receiver_info = receiver_type_info
            .ok_or(BuildError::internal(PROGRAM_VALIDATION_INTERNAL))
            .store_err_into(out.reborrow())?;
        self.execution
            .consume(receiver_info.fields().len(), out.reborrow())?;
        Self::ensure(receiver_info.fields().contains(&field_id)).store_err_into(out.reborrow())?;
        let field_info = self
            .schema
            .field_info(field_id)
            .store_err_into(out.reborrow())?;
        let field = field_info
            .ok_or(BuildError::internal(PROGRAM_VALIDATION_INTERNAL))
            .store_err_into(out.reborrow())?;
        let same_type = self
            .schema
            .same_type(field.ty(), node.ty)
            .store_err_into(out.reborrow())?;
        Self::ensure(same_type).store_err_into(out.reborrow())?;
        self.schedule(
            Pending {
                index: receiver.index(),
                scope: node.scope,
                allow_magnitude: false,
            },
            out.reborrow(),
        )
    }

    #[inline(never)]
    fn validate_unary<'slot>(
        &mut self,
        node: NodeValidation<'_>,
        operator: UnaryOperator,
        operand: ExpressionNodeId,
        mut out: Out<'slot, '_, BuildError>,
    ) -> OResult<'slot, ()> {
        let operand_ty = self.child_type(operand).store_err_into(out.reborrow())?;
        let same_type = self
            .schema
            .same_type(operand_ty, node.ty)
            .store_err_into(out.reborrow())?;
        Self::ensure(same_type).store_err_into(out.reborrow())?;
        Self::ensure(match operator {
            UnaryOperator::Not => Self::is_builtin(self.schema, node.ty, BuiltinType::Bool),
            UnaryOperator::Negate => {
                Self::builtin(self.schema, node.ty).is_some_and(Self::signed_or_float)
            }
        })
        .store_err_into(out.reborrow())?;
        self.schedule(
            Pending {
                index: operand.index(),
                scope: node.scope,
                allow_magnitude: operator == UnaryOperator::Negate,
            },
            out.reborrow(),
        )
    }

    #[inline(never)]
    fn validate_binary<'slot>(
        &mut self,
        node: NodeValidation<'_>,
        operation: BinaryOperation,
        mut out: Out<'slot, '_, BuildError>,
    ) -> OResult<'slot, ()> {
        let left_ty = self
            .child_type(operation.left)
            .store_err_into(out.reborrow())?;
        let right_ty = self
            .child_type(operation.right)
            .store_err_into(out.reborrow())?;
        let same_type = self
            .schema
            .same_type(left_ty, right_ty)
            .store_err_into(out.reborrow())?;
        Self::ensure(same_type).store_err_into(out.reborrow())?;
        let valid = match operation.operator {
            BinaryOperator::And | BinaryOperator::Or => {
                Self::is_builtin(self.schema, left_ty, BuiltinType::Bool)
                    && Self::is_builtin(self.schema, node.ty, BuiltinType::Bool)
            }
            BinaryOperator::Add
            | BinaryOperator::Subtract
            | BinaryOperator::Multiply
            | BinaryOperator::Divide => {
                Self::builtin(self.schema, left_ty).is_some_and(Self::numeric)
                    && self
                        .schema
                        .same_type(left_ty, node.ty)
                        .store_err_into(out.reborrow())?
            }
            BinaryOperator::Less
            | BinaryOperator::LessEqual
            | BinaryOperator::Greater
            | BinaryOperator::GreaterEqual => {
                Self::builtin(self.schema, left_ty)
                    .is_some_and(|builtin| Self::numeric(builtin) || builtin == BuiltinType::String)
                    && Self::is_builtin(self.schema, node.ty, BuiltinType::Bool)
            }
            BinaryOperator::Equal | BinaryOperator::NotEqual => {
                Self::comparable(self.schema, left_ty, self.execution, out.reborrow())?
                    && Self::is_builtin(self.schema, node.ty, BuiltinType::Bool)
            }
        };
        Self::ensure(valid).store_err_into(out.reborrow())?;
        self.schedule(
            Pending {
                index: operation.right.index(),
                scope: node.scope,
                allow_magnitude: false,
            },
            out.reborrow(),
        )?;
        self.schedule(
            Pending {
                index: operation.left.index(),
                scope: node.scope,
                allow_magnitude: false,
            },
            out.reborrow(),
        )
    }

    #[inline(never)]
    fn validate_method<'slot>(
        &mut self,
        node: NodeValidation<'_>,
        operation: MethodOperation<'_>,
        mut out: Out<'slot, '_, BuildError>,
    ) -> OResult<'slot, ()> {
        let receiver_ty = self
            .child_type(operation.receiver)
            .store_err_into(out.reborrow())?;
        let receiver_type_info = self
            .schema
            .type_info(receiver_ty)
            .store_err_into(out.reborrow())?;
        let receiver_info = receiver_type_info
            .ok_or(BuildError::internal(PROGRAM_VALIDATION_INTERNAL))
            .store_err_into(out.reborrow())?;
        let kind = receiver_info.kind();
        let no_argument = operation.argument.is_none();
        let expression_argument_type = match operation.argument {
            Some(TypedMethodArgument::Expression(id)) => {
                Some(self.child_type(*id).store_err_into(out.reborrow())?)
            }
            _ => None,
        };
        let closure_body_type = match operation.argument {
            Some(TypedMethodArgument::Closure { body, .. }) => {
                Some(self.child_type(*body).store_err_into(out.reborrow())?)
            }
            _ => None,
        };
        let valid = match operation.method {
            ExpressionMethod::Length => {
                no_argument
                    && matches!(
                        kind,
                        StoredTypeKind::Array { .. } | StoredTypeKind::Map { .. }
                    )
                    && Self::is_builtin(self.schema, node.ty, BuiltinType::U64)
            }
            ExpressionMethod::IsEmpty => {
                no_argument
                    && matches!(
                        kind,
                        StoredTypeKind::Array { .. } | StoredTypeKind::Map { .. }
                    )
                    && Self::is_builtin(self.schema, node.ty, BuiltinType::Bool)
            }
            ExpressionMethod::IsSome | ExpressionMethod::IsNone => {
                no_argument
                    && matches!(kind, StoredTypeKind::Option { .. })
                    && Self::is_builtin(self.schema, node.ty, BuiltinType::Bool)
            }
            ExpressionMethod::StartsWith | ExpressionMethod::IsVariant => {
                let target = match operation.method {
                    ExpressionMethod::StartsWith => {
                        Self::is_builtin(self.schema, receiver_ty, BuiltinType::String)
                    }
                    _ => matches!(kind, StoredTypeKind::Enum { .. }),
                };
                target
                    && Self::is_builtin(self.schema, node.ty, BuiltinType::Bool)
                    && expression_argument_type
                        .is_some_and(|ty| Self::is_builtin(self.schema, ty, BuiltinType::String))
            }
            ExpressionMethod::Sum => match kind {
                StoredTypeKind::Array { element, .. } => {
                    no_argument
                        && self
                            .schema
                            .same_type(element, node.ty)
                            .store_err_into(out.reborrow())?
                        && Self::builtin(self.schema, element).is_some_and(Self::numeric)
                }
                _ => false,
            },
            ExpressionMethod::Any | ExpressionMethod::All => {
                matches!(kind, StoredTypeKind::Array { .. })
                    && closure_body_type
                        .is_some_and(|ty| Self::is_builtin(self.schema, ty, BuiltinType::Bool))
                    && Self::is_builtin(self.schema, node.ty, BuiltinType::Bool)
            }
        };
        Self::ensure(valid).store_err_into(out.reborrow())?;
        self.schedule(
            Pending {
                index: operation.receiver.index(),
                scope: node.scope,
                allow_magnitude: false,
            },
            out.reborrow(),
        )?;
        match operation.argument {
            Some(TypedMethodArgument::Expression(id)) => self.schedule(
                Pending {
                    index: id.index(),
                    scope: node.scope,
                    allow_magnitude: false,
                },
                out.reborrow(),
            ),
            Some(TypedMethodArgument::Closure { parameter, body }) => {
                let StoredTypeKind::Array { element, .. } = kind else {
                    return Err(out.fail(BuildError::internal(PROGRAM_VALIDATION_INTERNAL)));
                };
                self.execution.consume(self.scopes.len(), out.reborrow())?;
                Self::ensure(
                    !self
                        .scopes
                        .iter()
                        .any(|local| local.parameter == *parameter),
                )
                .store_err_into(out.reborrow())?;
                self.scopes.try_reserve(1, out.reborrow())?;
                let local = self.scopes.len();
                self.scopes.try_push(
                    Scope {
                        parameter: *parameter,
                        ty: element,
                        parent: node.scope,
                    },
                    out.reborrow(),
                )?;
                self.schedule(
                    Pending {
                        index: body.index(),
                        scope: Some(local),
                        allow_magnitude: false,
                    },
                    out.reborrow(),
                )
            }
            None => Ok(()),
        }
    }

    fn child_type(&self, id: ExpressionNodeId) -> Result<TypeId, BuildError> {
        self.program
            .nodes()
            .get(id.index())
            .map(|node| node.ty())
            .ok_or(BuildError::internal(PROGRAM_VALIDATION_INTERNAL))
    }

    fn scope_type<'slot>(
        &self,
        mut current: Option<usize>,
        parameter: ClosureLocalId,
        mut out: Out<'slot, '_, BuildError>,
    ) -> OResult<'slot, Option<TypeId>> {
        while let Some(id) = current {
            self.execution.consume(1, out.reborrow())?;
            let local = self
                .scopes
                .get(id)
                .ok_or(BuildError::internal(PROGRAM_VALIDATION_INTERNAL))
                .store_err_into(out.reborrow())?;
            if local.parameter == parameter {
                return Ok(Some(local.ty));
            }
            current = local.parent;
        }
        Ok(None)
    }

    fn schedule<'slot>(
        &mut self,
        task: Pending,
        mut out: Out<'slot, '_, BuildError>,
    ) -> OResult<'slot, ()> {
        self.pending.try_reserve(1, out.reborrow())?;
        self.pending.try_push(task, out.reborrow())
    }

    pub(super) fn valid_bounds(
        builtin: BuiltinType,
        min: Option<NumberValue>,
        max: Option<NumberValue>,
    ) -> bool {
        let fits = |number| match number {
            NumberValue::Signed(number) => DefaultValidation::signed_fits(builtin, number),
            NumberValue::Unsigned(number) => DefaultValidation::unsigned_fits(builtin, number),
            NumberValue::Float(number) => {
                number.is_finite()
                    && (builtin == BuiltinType::F64
                        || (builtin == BuiltinType::F32 && f64::from(number as f32) == number))
            }
        };
        min.is_none_or(fits)
            && max.is_none_or(fits)
            && match (min, max) {
                (Some(NumberValue::Signed(left)), Some(NumberValue::Signed(right))) => {
                    left <= right
                }
                (Some(NumberValue::Unsigned(left)), Some(NumberValue::Unsigned(right))) => {
                    left <= right
                }
                (Some(NumberValue::Float(left)), Some(NumberValue::Float(right))) => left <= right,
                (Some(_), Some(_)) => false,
                _ => true,
            }
    }

    pub(super) fn builtin(schema: &CompiledSchema, ty: TypeId) -> Option<BuiltinType> {
        match schema.type_info(ty).ok().flatten()?.kind() {
            StoredTypeKind::Builtin(builtin) => Some(builtin),
            _ => None,
        }
    }

    pub(super) fn is_builtin(schema: &CompiledSchema, ty: TypeId, builtin: BuiltinType) -> bool {
        Self::builtin(schema, ty) == Some(builtin)
    }

    pub(super) fn numeric(builtin: BuiltinType) -> bool {
        !matches!(builtin, BuiltinType::String | BuiltinType::Bool)
    }

    pub(super) fn comparable<'slot>(
        schema: &CompiledSchema,
        mut ty: TypeId,
        execution: &ExecutionBudget,
        mut out: Out<'slot, '_, BuildError>,
    ) -> OResult<'slot, bool> {
        for _ in 0..schema.records().types.len() {
            execution.consume(1, out.reborrow())?;
            let type_info = schema.type_info(ty).store_err_into(out.reborrow())?;
            let type_info = type_info
                .ok_or(BuildError::internal(PROGRAM_VALIDATION_INTERNAL))
                .store_err_into(out.reborrow())?;
            match type_info.kind() {
                StoredTypeKind::Builtin(_) | StoredTypeKind::Enum { .. } => {
                    return Ok(true);
                }
                StoredTypeKind::Option { inner } => ty = inner,
                _ => return Ok(false),
            }
        }
        Ok(false)
    }

    fn signed_or_float(builtin: BuiltinType) -> bool {
        matches!(
            builtin,
            BuiltinType::I8
                | BuiltinType::I16
                | BuiltinType::I32
                | BuiltinType::I64
                | BuiltinType::I128
                | BuiltinType::F32
                | BuiltinType::F64
        )
    }

    fn integer_literal(builtin: BuiltinType, value: u128) -> bool {
        match builtin {
            BuiltinType::I8 => value <= 128,
            BuiltinType::I16 => value <= 32768,
            BuiltinType::I32 => value <= 2_147_483_648,
            BuiltinType::I64 => value <= 9_223_372_036_854_775_808,
            BuiltinType::I128 => value <= 1_u128 << 127,
            _ => DefaultValidation::unsigned_fits(builtin, value),
        }
    }

    pub(super) fn ensure(valid: bool) -> Result<(), BuildError> {
        if valid {
            Ok(())
        } else {
            Err(BuildError::internal(PROGRAM_VALIDATION_INTERNAL))
        }
    }

    fn flags<'slot>(
        count: usize,
        execution: &ExecutionBudget,
        mut out: Out<'slot, '_, BuildError>,
    ) -> OResult<'slot, ChargedVec<bool>> {
        let mut result = ChargedVec::new(Some(execution), out.reborrow())?;
        result.try_reserve_exact(count, out.reborrow())?;
        for _ in 0..count {
            result.try_push(false, out.reborrow())?;
        }
        Ok(result)
    }
}
