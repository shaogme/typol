mod operations;
mod tasks;
mod type_checks;

use crate::{
    diagnostics::{BuildFailure, DiagnosticCollector},
    expressions::{
        CompiledExpression, ExpressionNodeId, ExpressionType, TypedExpressionKind,
        TypedExpressionNode, TypedLiteral, TypedMethodArgument,
        compile_support::{Context, ExpressionCompileSupport},
        methods::{ExpressionMethods, LocalBinding, MethodContext, MethodPreparationInput},
    },
    symbols::{FieldOwner, LookupResult, SymbolTable},
    types::{ResolvedTypeGraph, ResolvedTypeKind},
};
use opeo::{OResult, Out};
use typol_core::{ChargedVec, SourceSpan};
use typol_i18n::diagnostic_message;
use typol_syntax::{BinaryOperator, BuiltinType, Expr, ExprKind, ScalarLiteral, UnaryOperator};

use operations::{
    BinaryCompileInput, ExpressionOperations, ExpressionOperationsInput, LiteralCompileInput,
    UnaryCompileInput,
};
use tasks::{CheckTask, CheckTaskStack, EnterTask};
use type_checks::TypeChecks;

pub(super) use tasks::ExpressionCompileInput;

struct FinishExpressionInput<'expression> {
    expression: Expr<'expression>,
    expected: Option<ExpressionType>,
    allow_negative_minimum: bool,
    child: Option<ExpressionNodeId>,
}

pub(super) struct ExpressionChecker<'c, 'd> {
    symbols: &'c SymbolTable<'c>,
    types: &'c ResolvedTypeGraph,
    diagnostics: &'d mut DiagnosticCollector,
    context: Context,
    nodes: ChargedVec<TypedExpressionNode>,
    locals: ChargedVec<LocalBinding>,
    next_local: usize,
    fallback_span: SourceSpan,
}

impl ExpressionChecker<'_, '_> {
    fn operations(&mut self) -> ExpressionOperations<'_, '_, '_, '_, '_> {
        ExpressionOperations::new(ExpressionOperationsInput {
            symbols: self.symbols,
            types: self.types,
            diagnostics: self.diagnostics,
            context: &self.context,
            nodes: &mut self.nodes,
            locals: &self.locals,
            fallback_span: self.fallback_span,
        })
    }

    pub(super) fn compile_boolean<'slot>(
        input: ExpressionCompileInput<'_, '_, '_>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, CompiledExpression> {
        let ExpressionCompileInput {
            expression,
            context,
            symbols,
            types,
            diagnostics,
        } = input;
        let nodes = diagnostics.new_vec(symbols.context().execution(), out.reborrow())?;
        let locals = diagnostics.new_vec(symbols.context().execution(), out.reborrow())?;
        let mut checker = ExpressionChecker {
            symbols,
            types,
            diagnostics,
            context,
            nodes,
            locals,
            next_local: 0,
            fallback_span: expression.span(),
        };
        let root = checker.compile(expression, None, false, out.reborrow())?;
        let result_type =
            checker
                .operations()
                .node_type(root, expression.span(), out.reborrow())?;
        if !checker
            .operations()
            .is_builtin(result_type, BuiltinType::Bool, out.reborrow())?
        {
            checker.operations().fail(
                "expression result must be bool",
                expression.span(),
                out.reborrow(),
            )?;
        }
        let Some(result_id) = checker.types.semantic_id(result_type) else {
            checker
                .diagnostics
                .push(schema_internal!(expression.span()));
            return Err(out.fail(BuildFailure::empty()));
        };
        Ok(CompiledExpression::new(
            checker.nodes,
            root,
            result_id,
            expression.span(),
        ))
    }

    fn compile<'slot>(
        &mut self,
        expression: Expr<'_>,
        expected: Option<ExpressionType>,
        allow_negative_minimum: bool,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ExpressionNodeId> {
        let mut tasks = CheckTaskStack::allocate_tasks(
            self.symbols.context(),
            self.diagnostics,
            out.reborrow(),
        )?;
        CheckTaskStack::push(
            &mut tasks,
            CheckTask::enter(expression, expected, allow_negative_minimum),
            out.reborrow(),
        )?;
        let mut result = None;
        while let Some(task) = tasks.pop() {
            self.symbols.context().consume(
                1,
                self.fallback_span,
                self.diagnostics,
                out.reborrow(),
            )?;
            match task {
                CheckTask::Enter(EnterTask {
                    expression,
                    expected,
                    allow_minimum,
                }) => match TypeChecks::ast_access(
                    self.symbols.context(),
                    out.reborrow(),
                    |ast_out| expression.kind(ast_out),
                )? {
                    ExprKind::FieldAccess { receiver, .. } => {
                        CheckTaskStack::push(
                            &mut tasks,
                            CheckTask::Finish(expression, expected),
                            out.reborrow(),
                        )?;
                        CheckTaskStack::push(
                            &mut tasks,
                            CheckTask::enter(receiver, None, false),
                            out.reborrow(),
                        )?;
                    }
                    ExprKind::Unary { operator, operand } => {
                        let operand_kind = TypeChecks::ast_access(
                            self.symbols.context(),
                            out.reborrow(),
                            |ast_out| operand.kind(ast_out),
                        )?;
                        let allow_minimum = operator == UnaryOperator::Negate
                            && matches!(operand_kind, ExprKind::Literal(ScalarLiteral::Number(_)));
                        CheckTaskStack::push(
                            &mut tasks,
                            CheckTask::Finish(expression, expected),
                            out.reborrow(),
                        )?;
                        CheckTaskStack::push(
                            &mut tasks,
                            CheckTask::enter(operand, expected, allow_minimum),
                            out.reborrow(),
                        )?;
                    }
                    ExprKind::Binary {
                        operator,
                        left,
                        right,
                    } => {
                        let logical = matches!(operator, BinaryOperator::And | BinaryOperator::Or);
                        let (left_contextual, right_contextual) = if logical {
                            (false, false)
                        } else {
                            (
                                ExpressionCompileSupport::is_contextual_expression(
                                    left,
                                    self.symbols.context(),
                                    self.diagnostics,
                                    out.reborrow(),
                                )?,
                                ExpressionCompileSupport::is_contextual_expression(
                                    right,
                                    self.symbols.context(),
                                    self.diagnostics,
                                    out.reborrow(),
                                )?,
                            )
                        };
                        let reversed = !logical && left_contextual && !right_contextual;
                        let first = if reversed { right } else { left };
                        let first_type = if logical {
                            Some(ExpressionCompileSupport::bool_type())
                        } else if ExpressionCompileSupport::is_arithmetic(operator)
                            && left_contextual
                            && right_contextual
                        {
                            expected
                        } else {
                            None
                        };
                        CheckTaskStack::push(
                            &mut tasks,
                            CheckTask::BinaryFirst {
                                expression,
                                expected,
                                reversed,
                            },
                            out.reborrow(),
                        )?;
                        CheckTaskStack::push(
                            &mut tasks,
                            CheckTask::enter(first, first_type, false),
                            out.reborrow(),
                        )?;
                    }
                    ExprKind::MethodCall { receiver, .. } => {
                        CheckTaskStack::push(
                            &mut tasks,
                            CheckTask::MethodReceiver(expression, expected),
                            out.reborrow(),
                        )?;
                        CheckTaskStack::push(
                            &mut tasks,
                            CheckTask::enter(receiver, None, false),
                            out.reborrow(),
                        )?;
                    }
                    _ => {
                        result = Some(self.finish_expression(
                            FinishExpressionInput {
                                expression,
                                expected,
                                allow_negative_minimum: allow_minimum,
                                child: None,
                            },
                            out.reborrow(),
                        )?);
                    }
                },
                CheckTask::Finish(expression, expected) => {
                    result = Some(self.finish_expression(
                        FinishExpressionInput {
                            expression,
                            expected,
                            allow_negative_minimum: false,
                            child: result.take(),
                        },
                        out.reborrow(),
                    )?);
                }
                CheckTask::BinaryFirst {
                    expression,
                    expected,
                    reversed,
                } => {
                    let first = self.diagnostics.require(
                        result.take(),
                        expression.span(),
                        out.reborrow(),
                    )?;
                    let ExprKind::Binary { left, right, .. } = TypeChecks::ast_access(
                        self.symbols.context(),
                        out.reborrow(),
                        |ast_out| expression.kind(ast_out),
                    )?
                    else {
                        return self
                            .operations()
                            .internal(expression.span(), out.reborrow());
                    };
                    let child_type =
                        self.operations()
                            .node_type(first, expression.span(), out.reborrow())?;
                    CheckTaskStack::push(
                        &mut tasks,
                        CheckTask::BinaryLast {
                            expression,
                            expected,
                            first,
                            reversed,
                        },
                        out.reborrow(),
                    )?;
                    CheckTaskStack::push(
                        &mut tasks,
                        CheckTask::enter(
                            if reversed { left } else { right },
                            Some(child_type),
                            false,
                        ),
                        out.reborrow(),
                    )?;
                }
                CheckTask::BinaryLast {
                    expression,
                    expected,
                    first,
                    reversed,
                } => {
                    let second = self.diagnostics.require(
                        result.take(),
                        expression.span(),
                        out.reborrow(),
                    )?;
                    let ExprKind::Binary { operator, .. } = TypeChecks::ast_access(
                        self.symbols.context(),
                        out.reborrow(),
                        |ast_out| expression.kind(ast_out),
                    )?
                    else {
                        return self
                            .operations()
                            .internal(expression.span(), out.reborrow());
                    };
                    let (left, right) = if reversed {
                        (second, first)
                    } else {
                        (first, second)
                    };
                    let node = self.operations().finish_binary(
                        BinaryCompileInput {
                            expression,
                            operator,
                            left_id: left,
                            right_id: right,
                        },
                        out.reborrow(),
                    )?;
                    self.operations().check_expected(
                        node,
                        expected,
                        expression.span(),
                        out.reborrow(),
                    )?;
                    result = Some(node);
                }
                CheckTask::MethodReceiver(expression, expected) => {
                    let receiver = self.diagnostics.require(
                        result.take(),
                        expression.span(),
                        out.reborrow(),
                    )?;
                    let ExprKind::MethodCall {
                        method, argument, ..
                    } = TypeChecks::ast_access(
                        self.symbols.context(),
                        out.reborrow(),
                        |ast_out| expression.kind(ast_out),
                    )?
                    else {
                        return self
                            .operations()
                            .internal(expression.span(), out.reborrow());
                    };
                    let plan = ExpressionMethods::new(MethodContext {
                        symbols: self.symbols,
                        types: self.types,
                        diagnostics: self.diagnostics,
                        nodes: &self.nodes,
                        locals: &mut self.locals,
                        next_local: &mut self.next_local,
                        fallback_span: self.fallback_span,
                    })
                    .prepare(
                        MethodPreparationInput {
                            expression,
                            receiver_id: receiver,
                            method_name: method,
                            argument,
                        },
                        out.reborrow(),
                    )?;
                    if let Some(child) = plan.child {
                        let child_type = plan.child_type;
                        CheckTaskStack::push(
                            &mut tasks,
                            CheckTask::MethodLast {
                                expression,
                                expected,
                                plan,
                            },
                            out.reborrow(),
                        )?;
                        CheckTaskStack::push(
                            &mut tasks,
                            CheckTask::enter(child, child_type, false),
                            out.reborrow(),
                        )?;
                    } else {
                        let node = self.operations().push_method(
                            &plan,
                            None,
                            expression.span(),
                            out.reborrow(),
                        )?;
                        self.operations().check_expected(
                            node,
                            expected,
                            expression.span(),
                            out.reborrow(),
                        )?;
                        result = Some(node);
                    }
                }
                CheckTask::MethodLast {
                    expression,
                    expected,
                    plan,
                } => {
                    let child = self.diagnostics.require(
                        result.take(),
                        expression.span(),
                        out.reborrow(),
                    )?;
                    let argument = if let Some(parameter) = plan.local {
                        self.locals.pop();
                        TypedMethodArgument::Closure {
                            parameter,
                            body: child,
                        }
                    } else {
                        TypedMethodArgument::Expression(child)
                    };
                    let node = self.operations().push_method(
                        &plan,
                        Some(argument),
                        expression.span(),
                        out.reborrow(),
                    )?;
                    self.operations().check_expected(
                        node,
                        expected,
                        expression.span(),
                        out.reborrow(),
                    )?;
                    result = Some(node);
                }
            }
        }
        self.diagnostics
            .require(result, expression.span(), out.reborrow())
    }

    fn finish_expression<'slot>(
        &mut self,
        input: FinishExpressionInput<'_>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ExpressionNodeId> {
        let FinishExpressionInput {
            expression,
            expected,
            allow_negative_minimum,
            child,
        } = input;
        let (kind, ty) =
            match TypeChecks::ast_access(self.symbols.context(), out.reborrow(), |ast_out| {
                expression.kind(ast_out)
            })? {
                ExprKind::Literal(literal) => self.operations().compile_literal(
                    LiteralCompileInput {
                        literal,
                        expected,
                        allow_negative_minimum,
                        span: expression.span(),
                    },
                    out.reborrow(),
                )?,
                ExprKind::None => {
                    let Some(ty) = expected else {
                        return self.operations().fail(
                            "none must be compared with a typed option value",
                            expression.span(),
                            out.reborrow(),
                        );
                    };
                    if !self.operations().is_option(ty, out.reborrow())? {
                        self.operations().fail(
                            "none can be compared only with an option value",
                            expression.span(),
                            out.reborrow(),
                        )?;
                    }
                    (TypedExpressionKind::Literal(TypedLiteral::None), ty)
                }
                ExprKind::Name(name) => {
                    let name_text = TypeChecks::ast_access(
                        self.symbols.context(),
                        out.reborrow(),
                        |ast_out| name.text(ast_out),
                    )?;
                    let (binding, ty) =
                        self.operations()
                            .resolve_name(name_text, name.span(), out.reborrow())?;
                    (TypedExpressionKind::Binding(binding), ty)
                }
                ExprKind::FieldAccess { receiver, field } => {
                    let receiver_id =
                        self.diagnostics
                            .require(child, receiver.span(), out.reborrow())?;
                    let receiver_type = self.operations().node_type(
                        receiver_id,
                        receiver.span(),
                        out.reborrow(),
                    )?;
                    let (owner, lookup) = match receiver_type {
                        ExpressionType::VariantStructure(variant) => {
                            let field_name = TypeChecks::ast_access(
                                self.symbols.context(),
                                out.reborrow(),
                                |ast_out| field.text(ast_out),
                            )?;
                            let lookup = self.symbols.lookup_variant_field(
                                variant,
                                field_name,
                                out.reborrow(),
                            )?;
                            (FieldOwner::Variant(variant), lookup)
                        }
                        ExpressionType::User(owner) => (
                            FieldOwner::Type(owner),
                            self.symbols.lookup_field(
                                owner,
                                TypeChecks::ast_access(
                                    self.symbols.context(),
                                    out.reborrow(),
                                    |ast_out| field.text(ast_out),
                                )?,
                                out.reborrow(),
                            )?,
                        ),
                        ExpressionType::Resolved(id) => {
                            match self.types.type_use(id).map(|item| item.kind()) {
                                Some(ResolvedTypeKind::User(owner)) => {
                                    let field_name = TypeChecks::ast_access(
                                        self.symbols.context(),
                                        out.reborrow(),
                                        |ast_out| field.text(ast_out),
                                    )?;
                                    let lookup = self.symbols.lookup_field(
                                        owner,
                                        field_name,
                                        out.reborrow(),
                                    )?;
                                    (FieldOwner::Type(owner), lookup)
                                }
                                _ => {
                                    return self.operations().fail(
                                        diagnostic_message!(
                                            "diagnostic.schema.field_access_requires_struct",
                                            "field access requires a struct value"
                                        ),
                                        field.span(),
                                        out.reborrow(),
                                    );
                                }
                            }
                        }
                        ExpressionType::Builtin(_) => {
                            return self.operations().fail(
                                diagnostic_message!(
                                    "diagnostic.schema.field_access_requires_struct",
                                    "field access requires a struct value"
                                ),
                                field.span(),
                                out.reborrow(),
                            );
                        }
                    };
                    let field_id = match lookup {
                        LookupResult::Found(id) => id,
                        LookupResult::Missing => {
                            return self.operations().fail(
                                diagnostic_message!(
                                    "diagnostic.schema.field_missing_from_struct",
                                    "field does not exist in the struct"
                                ),
                                field.span(),
                                out.reborrow(),
                            );
                        }
                        LookupResult::Ambiguous => {
                            return Err(out.fail(BuildFailure::empty()));
                        }
                    };
                    let Some(actual_owner) = self
                        .symbols
                        .field_symbol(field_id)
                        .map(|symbol| symbol.owner())
                    else {
                        return self.operations().internal(field.span(), out.reborrow());
                    };
                    if actual_owner != owner {
                        self.operations().fail(
                            "field does not belong to the current struct",
                            field.span(),
                            out.reborrow(),
                        )?;
                    }
                    let Some(field_type) = self.types.field_type(field_id) else {
                        return Err(out.fail(BuildFailure::empty()));
                    };
                    (
                        TypedExpressionKind::Field {
                            receiver: receiver_id,
                            field: field_id,
                        },
                        ExpressionType::Resolved(field_type),
                    )
                }
                ExprKind::Unary { operator, operand } => self.operations().compile_unary(
                    UnaryCompileInput {
                        operator,
                        operand,
                        expression_span: expression.span(),
                        child,
                    },
                    out.reborrow(),
                )?,
                ExprKind::Binary { .. } | ExprKind::MethodCall { .. } => {
                    return self
                        .operations()
                        .internal(expression.span(), out.reborrow());
                }
            };

        self.operations()
            .check_type_expected(ty, expected, expression.span(), out.reborrow())?;
        self.operations()
            .push_node(kind, ty, expression.span(), out.reborrow())
    }
}
