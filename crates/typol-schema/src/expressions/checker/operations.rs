use crate::{
    diagnostics::{BuildFailure, DiagnosticCollector},
    expressions::{
        ExpressionBinding, ExpressionNodeId, ExpressionType, TypedExpressionKind,
        TypedExpressionNode, TypedLiteral, TypedMethodArgument,
        checker::type_checks::TypeChecks,
        compile_support::{AllBindings, Context, ExpressionCompileSupport, KeywordInput},
        methods::{LocalBinding, MethodPlan},
        numeric::{CompileLiteralInput, ExpressionNumbers},
    },
    symbols::{SymbolState, SymbolTable},
    types::ResolvedTypeGraph,
};

use opeo::{OResult, Out};
use typol_core::{ChargedVec, DiagnosticMessage, SourceSpan};
use typol_syntax::{
    BinaryOperator, BuiltinType, Expr, NumberKind, NumberLiteral, ScalarLiteral, UnaryOperator,
};

pub(super) struct ExpressionOperationsInput<'symbols, 'source, 'diagnostics, 'nodes, 'locals> {
    pub(super) symbols: &'symbols SymbolTable<'source>,
    pub(super) types: &'symbols ResolvedTypeGraph,
    pub(super) diagnostics: &'diagnostics mut DiagnosticCollector,
    pub(super) context: &'symbols Context,
    pub(super) nodes: &'nodes mut ChargedVec<TypedExpressionNode>,
    pub(super) locals: &'locals [LocalBinding],
    pub(super) fallback_span: SourceSpan,
}

pub(super) struct LiteralCompileInput<'literal> {
    pub(super) literal: ScalarLiteral<'literal>,
    pub(super) expected: Option<ExpressionType>,
    pub(super) allow_negative_minimum: bool,
    pub(super) span: SourceSpan,
}

struct NumberCompileInput<'literal> {
    number: NumberLiteral<'literal>,
    expected: Option<ExpressionType>,
    allow_negative_minimum: bool,
    span: SourceSpan,
}

pub(super) struct BinaryCompileInput<'expression> {
    pub(super) expression: Expr<'expression>,
    pub(super) operator: BinaryOperator,
    pub(super) left_id: ExpressionNodeId,
    pub(super) right_id: ExpressionNodeId,
}

pub(super) struct UnaryCompileInput<'expression> {
    pub(super) operator: UnaryOperator,
    pub(super) operand: Expr<'expression>,
    pub(super) expression_span: SourceSpan,
    pub(super) child: Option<ExpressionNodeId>,
}

pub(super) struct ExpressionOperations<'symbols, 'source, 'diagnostics, 'nodes, 'locals> {
    symbols: &'symbols SymbolTable<'source>,
    types: &'symbols ResolvedTypeGraph,
    diagnostics: &'diagnostics mut DiagnosticCollector,
    context: &'symbols Context,
    nodes: &'nodes mut ChargedVec<TypedExpressionNode>,
    locals: &'locals [LocalBinding],
    fallback_span: SourceSpan,
}

impl<'symbols, 'source, 'diagnostics, 'nodes, 'locals>
    ExpressionOperations<'symbols, 'source, 'diagnostics, 'nodes, 'locals>
{
    pub(super) fn new(
        input: ExpressionOperationsInput<'symbols, 'source, 'diagnostics, 'nodes, 'locals>,
    ) -> Self {
        let ExpressionOperationsInput {
            symbols,
            types,
            diagnostics,
            context,
            nodes,
            locals,
            fallback_span,
        } = input;
        Self {
            symbols,
            types,
            diagnostics,
            context,
            nodes,
            locals,
            fallback_span,
        }
    }

    pub(super) fn compile_literal<'slot>(
        &mut self,
        input: LiteralCompileInput<'_>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, (TypedExpressionKind, ExpressionType)> {
        let LiteralCompileInput {
            literal,
            expected,
            allow_negative_minimum,
            span,
        } = input;
        match literal {
            ScalarLiteral::String(value) => {
                if let Some(expected) = expected {
                    if !self.is_builtin(expected, BuiltinType::String, out.reborrow())? {
                        self.fail(
                            "string literal does not match the target type",
                            span,
                            out.reborrow(),
                        )?;
                    }
                }
                let value =
                    TypeChecks::ast_access(self.symbols.context(), out.reborrow(), |ast_out| {
                        value.value(ast_out)
                    })?;
                let value = ExpressionCompileSupport::copy_text(
                    value,
                    self.symbols.context(),
                    out.reborrow(),
                )?;
                Ok((
                    TypedExpressionKind::Literal(TypedLiteral::String(value)),
                    ExpressionType::Builtin(BuiltinType::String),
                ))
            }
            ScalarLiteral::Boolean(value) => {
                if let Some(expected) = expected {
                    if !self.is_builtin(expected, BuiltinType::Bool, out.reborrow())? {
                        self.fail(
                            "boolean literal does not match the target type",
                            span,
                            out.reborrow(),
                        )?;
                    }
                }
                Ok((
                    TypedExpressionKind::Literal(TypedLiteral::Boolean(value.value())),
                    ExpressionType::Builtin(BuiltinType::Bool),
                ))
            }
            ScalarLiteral::Number(number) => self.compile_number(
                NumberCompileInput {
                    number,
                    expected,
                    allow_negative_minimum,
                    span,
                },
                out.reborrow(),
            ),
        }
    }

    fn compile_number<'slot>(
        &mut self,
        input: NumberCompileInput<'_>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, (TypedExpressionKind, ExpressionType)> {
        let NumberCompileInput {
            number,
            expected,
            allow_negative_minimum,
            span,
        } = input;
        let raw = TypeChecks::ast_access(self.symbols.context(), out.reborrow(), |ast_out| {
            number.raw(ast_out)
        })?;
        self.symbols
            .context()
            .consume(raw.len(), span, self.diagnostics, out.reborrow())?;
        let raw_builtin = match number.kind() {
            NumberKind::Integer => BuiltinType::I128,
            NumberKind::Float => BuiltinType::F64,
        };
        let target = if let Some(expected) = expected {
            let Some(builtin) = self.builtin_type(expected, out.reborrow())? else {
                return self.fail(
                    "numeric literal requires a numeric context",
                    span,
                    out.reborrow(),
                );
            };
            if !ExpressionNumbers::is_numeric(builtin) {
                self.fail(
                    "numeric literal does not match the target type",
                    span,
                    out.reborrow(),
                )?;
            }
            if number.kind() == NumberKind::Float && !ExpressionNumbers::is_float_type(builtin) {
                self.fail(
                    "floating-point literal cannot be used with an integer type",
                    span,
                    out.reborrow(),
                )?;
            }
            builtin
        } else {
            raw_builtin
        };
        let literal = ExpressionNumbers::compile_literal(
            CompileLiteralInput {
                number,
                target,
                allow_negative_minimum,
                span,
                context: self.symbols.context(),
                diagnostics: self.diagnostics,
            },
            out.reborrow(),
        )?;
        Ok((
            TypedExpressionKind::Literal(literal),
            ExpressionType::Builtin(target),
        ))
    }

    pub(super) fn compile_unary<'slot>(
        &mut self,
        input: UnaryCompileInput<'_>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, (TypedExpressionKind, ExpressionType)> {
        let UnaryCompileInput {
            operator,
            operand,
            expression_span,
            child,
        } = input;
        let operand_id = self
            .diagnostics
            .require(child, operand.span(), out.reborrow())?;
        let operand_type = self.node_type(operand_id, operand.span(), out.reborrow())?;
        let result_type = match operator {
            UnaryOperator::Not => {
                if !self.is_builtin(operand_type, BuiltinType::Bool, out.reborrow())? {
                    self.fail("! applies only to bool", expression_span, out.reborrow())?;
                }
                ExpressionType::Builtin(BuiltinType::Bool)
            }
            UnaryOperator::Negate => {
                let Some(builtin) = self.builtin_type(operand_type, out.reborrow())? else {
                    return self.fail(
                        "unary minus applies only to numbers",
                        expression_span,
                        out.reborrow(),
                    );
                };
                if !ExpressionNumbers::is_signed(builtin) {
                    self.fail(
                        "unary minus applies only to signed integers and floating-point numbers",
                        expression_span,
                        out.reborrow(),
                    )?;
                }
                operand_type
            }
        };
        Ok((
            TypedExpressionKind::Unary {
                operator,
                operand: operand_id,
            },
            result_type,
        ))
    }

    pub(super) fn finish_binary<'slot>(
        &mut self,
        input: BinaryCompileInput<'_>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ExpressionNodeId> {
        let BinaryCompileInput {
            expression,
            operator,
            left_id,
            right_id,
        } = input;
        let left_type = self.node_type(left_id, expression.span(), out.reborrow())?;
        let right_type = self.node_type(right_id, expression.span(), out.reborrow())?;
        let result_type = match operator {
            BinaryOperator::And | BinaryOperator::Or => {
                if !self.is_builtin(left_type, BuiltinType::Bool, out.reborrow())?
                    || !self.is_builtin(right_type, BuiltinType::Bool, out.reborrow())?
                {
                    self.fail(
                        "both operands of && and || must be bool",
                        expression.span(),
                        out.reborrow(),
                    )?;
                }
                ExpressionCompileSupport::bool_type()
            }
            BinaryOperator::Equal | BinaryOperator::NotEqual => {
                if !self.same_type(left_type, right_type, out.reborrow())? {
                    self.fail(
                        "both sides of a comparison must have the same type",
                        expression.span(),
                        out.reborrow(),
                    )?;
                }
                if !self.is_equality_comparable(left_type, false, out.reborrow())? {
                    self.fail(
                        "this type does not support equality comparisons",
                        expression.span(),
                        out.reborrow(),
                    )?;
                }
                ExpressionCompileSupport::bool_type()
            }
            BinaryOperator::Less
            | BinaryOperator::LessEqual
            | BinaryOperator::Greater
            | BinaryOperator::GreaterEqual => {
                if !self.same_type(left_type, right_type, out.reborrow())? {
                    self.fail(
                        "both sides of a comparison must have the same type",
                        expression.span(),
                        out.reborrow(),
                    )?;
                }
                let valid = match self.builtin_type(left_type, out.reborrow())? {
                    Some(ty) => ExpressionNumbers::is_numeric(ty) || ty == BuiltinType::String,
                    None => false,
                };
                if !valid {
                    self.fail(
                        "ordered comparisons apply only to numbers or strings of the same type",
                        expression.span(),
                        out.reborrow(),
                    )?;
                }
                ExpressionCompileSupport::bool_type()
            }
            BinaryOperator::Add
            | BinaryOperator::Subtract
            | BinaryOperator::Multiply
            | BinaryOperator::Divide => {
                if !self.same_type(left_type, right_type, out.reborrow())? {
                    self.fail(
                        "both sides of an arithmetic operation must have the same type",
                        expression.span(),
                        out.reborrow(),
                    )?;
                }
                let Some(ty) = self.builtin_type(left_type, out.reborrow())? else {
                    return self.fail(
                        "arithmetic operations apply only to numbers",
                        expression.span(),
                        out.reborrow(),
                    );
                };
                if !ExpressionNumbers::is_numeric(ty) {
                    self.fail(
                        "arithmetic operations apply only to numbers",
                        expression.span(),
                        out.reborrow(),
                    )?;
                }
                left_type
            }
        };
        self.push_node(
            TypedExpressionKind::Binary {
                operator,
                left: left_id,
                right: right_id,
            },
            result_type,
            expression.span(),
            out.reborrow(),
        )
    }

    pub(super) fn push_method<'slot>(
        &mut self,
        plan: &MethodPlan<'_>,
        argument: Option<TypedMethodArgument>,
        span: SourceSpan,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ExpressionNodeId> {
        self.push_node(
            TypedExpressionKind::Method {
                receiver: plan.receiver,
                method: plan.method,
                argument,
            },
            plan.ty,
            span,
            out.reborrow(),
        )
    }

    pub(super) fn resolve_name<'slot>(
        &mut self,
        name: &str,
        span: SourceSpan,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, (ExpressionBinding, ExpressionType)> {
        let keyword = ExpressionCompileSupport::keyword(
            KeywordInput {
                name,
                keywords: &["self", "root", "item", "key", "value"],
                context: self.symbols.context(),
                span,
                diagnostics: self.diagnostics,
            },
            out.reborrow(),
        )?;
        match keyword.unwrap_or("") {
            "self" => match self.context.self_type() {
                Some(ty) => Ok((ExpressionBinding::SelfValue, ty)),
                None => self.fail(
                    "unit-variant assertion cannot reference self",
                    span,
                    out.reborrow(),
                ),
            },
            "root" => match self.context.root_type() {
                Some(ExpressionType::User(id)) => {
                    let state = self.diagnostics.require(
                        self.types.declaration_state(id),
                        span,
                        out.reborrow(),
                    )?;
                    if state != SymbolState::Valid {
                        Err(out.fail(BuildFailure::empty()))
                    } else {
                        Ok((ExpressionBinding::Root, ExpressionType::User(id)))
                    }
                }
                Some(ty) => Ok((ExpressionBinding::Root, ty)),
                None => self.fail("root type is unavailable", span, out.reborrow()),
            },
            "item" => match self.context.all_bindings() {
                Some(AllBindings::Array(ty)) => Ok((ExpressionBinding::Item, ty)),
                _ if self.context.all_bindings().is_some() => {
                    self.fail("the current @all has no item binding", span, out.reborrow())
                }
                _ => self.fail(
                    "item is available only in @all expressions",
                    span,
                    out.reborrow(),
                ),
            },
            "key" => match self.context.all_bindings() {
                Some(AllBindings::Map(_)) => Ok((
                    ExpressionBinding::Key,
                    ExpressionCompileSupport::builtin_string(),
                )),
                _ if self.context.all_bindings().is_some() => {
                    self.fail("the current @all has no key binding", span, out.reborrow())
                }
                _ => self.fail(
                    "key is available only in map @all expressions",
                    span,
                    out.reborrow(),
                ),
            },
            "value" => match self.context.all_bindings() {
                Some(AllBindings::Map(ty)) => Ok((ExpressionBinding::Value, ty)),
                _ if self.context.all_bindings().is_some() => self.fail(
                    "the current @all has no value binding",
                    span,
                    out.reborrow(),
                ),
                _ => self.fail(
                    "value is available only in map @all expressions",
                    span,
                    out.reborrow(),
                ),
            },
            _ => {
                for local in self.locals.iter().rev() {
                    let order =
                        self.symbols
                            .context()
                            .compare_text(&local.name, name, out.reborrow())?;
                    if order.is_eq() {
                        return Ok((ExpressionBinding::Closure(local.id), local.ty));
                    }
                }
                self.fail(
                    "unknown expression name; field paths must start with self or root",
                    span,
                    out.reborrow(),
                )
            }
        }
    }

    pub(super) fn node_type<'slot>(
        &mut self,
        id: ExpressionNodeId,
        span: SourceSpan,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ExpressionType> {
        self.symbols
            .context()
            .consume(2, span, self.diagnostics, out.reborrow())?;
        let Some(ty) = self
            .nodes
            .get(id.index())
            .and_then(|node| self.types.expression_type(node.ty()))
        else {
            self.diagnostics.push(schema_internal!(span));
            return Err(out.fail(BuildFailure::empty()));
        };
        Ok(ty)
    }

    pub(super) fn check_expected<'slot>(
        &mut self,
        node: ExpressionNodeId,
        expected: Option<ExpressionType>,
        span: SourceSpan,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ()> {
        let Some(expected) = expected else {
            return Ok(());
        };
        let actual = self.node_type(node, span, out.reborrow())?;
        self.check_type_expected(actual, Some(expected), span, out.reborrow())
    }

    pub(super) fn check_type_expected<'slot>(
        &mut self,
        actual: ExpressionType,
        expected: Option<ExpressionType>,
        span: SourceSpan,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ()> {
        if let Some(expected) = expected {
            if !self.same_type(actual, expected, out.reborrow())? {
                self.fail(
                    "expression type does not match the context type",
                    span,
                    out.reborrow(),
                )?;
            }
        }
        Ok(())
    }

    pub(super) fn push_node<'slot>(
        &mut self,
        kind: TypedExpressionKind,
        ty: ExpressionType,
        span: SourceSpan,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ExpressionNodeId> {
        self.nodes.try_reserve(1, out.reborrow())?;
        let id = ExpressionNodeId::new(self.nodes.len());
        let Some(ty) = self.types.semantic_id(ty) else {
            self.diagnostics.push(schema_internal!(span));
            return Err(out.fail(BuildFailure::empty()));
        };
        self.nodes
            .try_push(TypedExpressionNode::new(kind, ty, span), out.reborrow())?;
        Ok(id)
    }

    pub(super) fn builtin_type<'slot>(
        &mut self,
        ty: ExpressionType,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, Option<BuiltinType>> {
        TypeChecks::new(
            self.types,
            self.symbols,
            self.diagnostics,
            self.fallback_span,
        )
        .builtin_type(ty, out.reborrow())
    }

    pub(super) fn is_builtin<'slot>(
        &mut self,
        ty: ExpressionType,
        expected: BuiltinType,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, bool> {
        TypeChecks::new(
            self.types,
            self.symbols,
            self.diagnostics,
            self.fallback_span,
        )
        .is_builtin(ty, expected, out.reborrow())
    }

    pub(super) fn is_option<'slot>(
        &mut self,
        ty: ExpressionType,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, bool> {
        TypeChecks::new(
            self.types,
            self.symbols,
            self.diagnostics,
            self.fallback_span,
        )
        .is_option(ty, out.reborrow())
    }

    pub(super) fn same_type<'slot>(
        &mut self,
        left: ExpressionType,
        right: ExpressionType,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, bool> {
        TypeChecks::new(
            self.types,
            self.symbols,
            self.diagnostics,
            self.fallback_span,
        )
        .same_type(left, right, out.reborrow())
    }

    pub(super) fn is_equality_comparable<'slot>(
        &mut self,
        ty: ExpressionType,
        inside_enum_payload: bool,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, bool> {
        TypeChecks::new(
            self.types,
            self.symbols,
            self.diagnostics,
            self.fallback_span,
        )
        .is_equality_comparable(ty, inside_enum_payload, out.reborrow())
    }

    pub(super) fn fail<'slot, 'message, T>(
        &mut self,
        message: impl Into<DiagnosticMessage<'message>>,
        span: SourceSpan,
        out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, T> {
        self.diagnostics
            .push(self.diagnostics.make_semantic(message, span));
        Err(out.fail(BuildFailure::empty()))
    }

    pub(super) fn internal<'slot, T>(
        &mut self,
        span: SourceSpan,
        out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, T> {
        self.diagnostics.abort(schema_internal!(span));
        Err(out.fail(BuildFailure::empty()))
    }
}
