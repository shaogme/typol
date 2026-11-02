use crate::{
    diagnostics::{BuildFailure, DiagnosticCollector, EmergencyDiagnostics},
    expressions::{
        compile_support::{ExpressionCompileSupport, KeywordInput},
        numeric::ExpressionNumbers,
        type_analysis::{TypeAnalyzer, TypeShape},
        {ClosureLocalId, ExpressionMethod, ExpressionNodeId, ExpressionType, TypedExpressionNode},
    },
    symbols::SymbolTable,
    types::ResolvedTypeGraph,
};
use opeo::{OResult, Out};
use typol_core::{ChargedString, ChargedVec, SourceSpan};
use typol_syntax::{AstAccessError, BuiltinType, Expr, MethodArgument, MethodArgumentKind, Name};

pub(super) struct LocalBinding {
    pub(super) name: ChargedString,
    pub(super) id: ClosureLocalId,
    pub(super) ty: ExpressionType,
}

pub(super) struct MethodPlan<'a> {
    pub(super) receiver: ExpressionNodeId,
    pub(super) method: ExpressionMethod,
    pub(super) ty: ExpressionType,
    pub(super) child: Option<Expr<'a>>,
    pub(super) child_type: Option<ExpressionType>,
    pub(super) local: Option<ClosureLocalId>,
}

pub(super) struct MethodPreparationInput<'a> {
    pub(super) expression: Expr<'a>,
    pub(super) receiver_id: ExpressionNodeId,
    pub(super) method_name: Name<'a>,
    pub(super) argument: Option<MethodArgument<'a>>,
}

pub(super) struct MethodContext<'a, 'c> {
    pub(super) symbols: &'c SymbolTable<'c>,
    pub(super) types: &'c ResolvedTypeGraph,
    pub(super) diagnostics: &'a mut DiagnosticCollector,
    pub(super) nodes: &'a ChargedVec<TypedExpressionNode>,
    pub(super) locals: &'a mut ChargedVec<LocalBinding>,
    pub(super) next_local: &'a mut usize,
    pub(super) fallback_span: SourceSpan,
}

pub(super) struct ExpressionMethods<'a, 'c> {
    context: MethodContext<'a, 'c>,
}

impl<'b, 'c> ExpressionMethods<'b, 'c> {
    pub(super) fn new(context: MethodContext<'b, 'c>) -> Self {
        Self { context }
    }

    fn ast_access<'slot, T>(
        &self,
        mut out: Out<'slot, '_, BuildFailure>,
        operation: impl for<'call> FnOnce(Out<'call, 'call, AstAccessError>) -> OResult<'call, T>,
    ) -> OResult<'slot, T> {
        self.context
            .symbols
            .context()
            .ast_access(out.reborrow(), operation, BuildFailure::from)
    }

    pub(super) fn prepare<'slot, 'a>(
        &mut self,
        input: MethodPreparationInput<'a>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, MethodPlan<'a>> {
        let MethodPreparationInput {
            expression,
            receiver_id,
            method_name,
            argument,
        } = input;
        self.context.symbols.context().consume(
            1,
            expression.span(),
            self.context.diagnostics,
            out.reborrow(),
        )?;
        let Some(receiver_type) = self
            .context
            .nodes
            .get(receiver_id.index())
            .and_then(|node| self.context.types.expression_type(node.ty()))
        else {
            return Err(out.fail(BuildFailure::with_context(schema_context!(
                "expression-methods",
                "receiver-type-missing"
            ))));
        };
        let shape = self.shape(receiver_type, method_name.span(), out.reborrow())?;
        let name = self.ast_access(out.reborrow(), |ast_out| method_name.text(ast_out))?;

        let name = ExpressionCompileSupport::keyword(
            KeywordInput {
                name,
                keywords: &[
                    "len",
                    "is_empty",
                    "is_some",
                    "is_none",
                    "starts_with",
                    "is_variant",
                    "any",
                    "all",
                    "sum",
                ],
                context: self.context.symbols.context(),
                span: method_name.span(),
                diagnostics: self.context.diagnostics,
            },
            out.reborrow(),
        )?
        .unwrap_or("");
        match name {
            "len" if matches!(shape, TypeShape::Array(..) | TypeShape::Map(..)) => {
                self.require_no_argument(argument, method_name.span(), out.reborrow())?;
                Ok(MethodPlan {
                    receiver: receiver_id,
                    method: ExpressionMethod::Length,
                    ty: ExpressionCompileSupport::builtin_u64(),
                    child: None,
                    child_type: None,
                    local: None,
                })
            }
            "is_empty" if matches!(shape, TypeShape::Array(..) | TypeShape::Map(..)) => {
                self.require_no_argument(argument, method_name.span(), out.reborrow())?;
                Ok(MethodPlan {
                    receiver: receiver_id,
                    method: ExpressionMethod::IsEmpty,
                    ty: ExpressionCompileSupport::bool_type(),
                    child: None,
                    child_type: None,
                    local: None,
                })
            }
            "is_some" | "is_none" if matches!(shape, TypeShape::Option(_)) => {
                self.require_no_argument(argument, method_name.span(), out.reborrow())?;
                let method = if name == "is_some" {
                    ExpressionMethod::IsSome
                } else {
                    ExpressionMethod::IsNone
                };
                Ok(MethodPlan {
                    receiver: receiver_id,
                    method,
                    ty: ExpressionCompileSupport::bool_type(),
                    child: None,
                    child_type: None,
                    local: None,
                })
            }
            "starts_with" if matches!(shape, TypeShape::Builtin(BuiltinType::String)) => {
                let argument_value =
                    self.require_expression_argument(argument, method_name.span(), out.reborrow())?;
                Ok(MethodPlan {
                    receiver: receiver_id,
                    method: ExpressionMethod::StartsWith,
                    ty: ExpressionCompileSupport::bool_type(),
                    child: Some(argument_value),
                    child_type: Some(ExpressionCompileSupport::builtin_string()),
                    local: None,
                })
            }
            "is_variant"
                if self.is_enum_type(receiver_type, method_name.span(), out.reborrow())? =>
            {
                let argument_value =
                    self.require_expression_argument(argument, method_name.span(), out.reborrow())?;
                Ok(MethodPlan {
                    receiver: receiver_id,
                    method: ExpressionMethod::IsVariant,
                    ty: ExpressionCompileSupport::bool_type(),
                    child: Some(argument_value),
                    child_type: Some(ExpressionCompileSupport::builtin_string()),
                    local: None,
                })
            }
            "any" | "all" if matches!(shape, TypeShape::Array(..)) => {
                let element = match shape {
                    TypeShape::Array(element, _) => element,
                    _ => {
                        return self.fail(
                            "any/all applies only to arrays",
                            method_name.span(),
                            out.reborrow(),
                        );
                    }
                };
                let Some(argument) = argument else {
                    return self.fail(
                        "any/all requires one closure argument",
                        method_name.span(),
                        out.reborrow(),
                    );
                };
                let MethodArgumentKind::Closure { parameter, body } =
                    self.ast_access(out.reborrow(), |ast_out| argument.kind(ast_out))?
                else {
                    return self.fail(
                        "any/all requires one closure argument",
                        argument.span(),
                        out.reborrow(),
                    );
                };
                let parameter_name =
                    self.ast_access(out.reborrow(), |ast_out| parameter.text(ast_out))?;
                self.check_closure_name(parameter_name, parameter.span(), out.reborrow())?;
                let id = *self.context.next_local;
                if u32::try_from(id).is_err() {
                    self.context
                        .diagnostics
                        .push(EmergencyDiagnostics::resource(argument.span()));
                    return Err(out.fail(BuildFailure::empty()));
                }
                let Some(next_local) = self.context.next_local.checked_add(1) else {
                    self.context
                        .diagnostics
                        .push(EmergencyDiagnostics::resource(argument.span()));
                    return Err(out.fail(BuildFailure::empty()));
                };
                *self.context.next_local = next_local;
                let local = ClosureLocalId::new(id);
                self.context.locals.try_reserve(1, out.reborrow())?;
                let local_name = ExpressionCompileSupport::copy_text(
                    parameter_name,
                    self.context.symbols.context(),
                    out.reborrow(),
                );
                let local_name = local_name?;
                self.context.locals.try_push(
                    LocalBinding {
                        name: local_name,
                        id: local,
                        ty: ExpressionType::Resolved(element),
                    },
                    out.reborrow(),
                )?;
                let method = if name == "any" {
                    ExpressionMethod::Any
                } else {
                    ExpressionMethod::All
                };
                Ok(MethodPlan {
                    receiver: receiver_id,
                    method,
                    ty: ExpressionCompileSupport::bool_type(),
                    child: Some(body),
                    child_type: Some(ExpressionCompileSupport::bool_type()),
                    local: Some(local),
                })
            }
            "sum" if matches!(shape, TypeShape::Array(..)) => {
                self.require_no_argument(argument, method_name.span(), out.reborrow())?;
                let element = match shape {
                    TypeShape::Array(element, _) => element,
                    _ => {
                        return self.fail(
                            "sum applies only to arrays",
                            method_name.span(),
                            out.reborrow(),
                        );
                    }
                };
                let ty = ExpressionType::Resolved(element);
                let Some(element_type) = self.builtin_type(ty, out.reborrow())? else {
                    return self.fail(
                        "sum applies only to numeric arrays",
                        method_name.span(),
                        out.reborrow(),
                    );
                };
                if !ExpressionNumbers::is_numeric(element_type) {
                    self.fail(
                        "sum applies only to numeric arrays",
                        method_name.span(),
                        out.reborrow(),
                    )?;
                }
                Ok(MethodPlan {
                    receiver: receiver_id,
                    method: ExpressionMethod::Sum,
                    ty,
                    child: None,
                    child_type: None,
                    local: None,
                })
            }
            "len" | "is_empty" | "is_some" | "is_none" | "starts_with" | "is_variant" | "any"
            | "all" | "sum" => self.fail(
                "this expression method is not supported for this type",
                method_name.span(),
                out.reborrow(),
            ),
            _ => self.fail(
                "unknown expression method",
                method_name.span(),
                out.reborrow(),
            ),
        }
    }

    fn require_no_argument<'slot>(
        &mut self,
        argument: Option<MethodArgument<'_>>,
        span: SourceSpan,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ()> {
        if argument.is_some() {
            self.fail(
                "this method takes no arguments",
                argument.map_or(span, MethodArgument::span),
                out.reborrow(),
            )?;
        }
        Ok(())
    }

    fn require_expression_argument<'slot, 'a>(
        &mut self,
        argument: Option<MethodArgument<'a>>,
        span: SourceSpan,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, Expr<'a>> {
        let Some(argument) = argument else {
            return self.fail("this method requires one argument", span, out.reborrow());
        };
        let MethodArgumentKind::Expression(value) =
            self.ast_access(out.reborrow(), |ast_out| argument.kind(ast_out))?
        else {
            return self.fail(
                "this method requires a regular expression argument",
                argument.span(),
                out.reborrow(),
            );
        };
        Ok(value)
    }

    fn check_closure_name<'slot>(
        &mut self,
        name: &str,
        span: SourceSpan,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ()> {
        if ExpressionCompileSupport::keyword(
            KeywordInput {
                name,
                keywords: &["self", "root", "item", "key", "value"],
                context: self.context.symbols.context(),
                span,
                diagnostics: self.context.diagnostics,
            },
            out.reborrow(),
        )?
        .is_some()
        {
            self.fail(
                "closure parameter cannot use a reserved expression name",
                span,
                out.reborrow(),
            )?;
        }
        Ok(())
    }

    fn shape<'slot>(
        &mut self,
        ty: ExpressionType,
        span: SourceSpan,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, TypeShape> {
        TypeAnalyzer::new(
            self.context.types,
            self.context.symbols,
            self.context.diagnostics,
            self.context.fallback_span,
        )
        .shape(ty, span, out.reborrow())
    }

    fn builtin_type<'slot>(
        &mut self,
        ty: ExpressionType,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, Option<BuiltinType>> {
        TypeAnalyzer::new(
            self.context.types,
            self.context.symbols,
            self.context.diagnostics,
            self.context.fallback_span,
        )
        .builtin_type(ty, out.reborrow())
    }

    fn is_enum_type<'slot>(
        &mut self,
        ty: ExpressionType,
        span: SourceSpan,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, bool> {
        TypeAnalyzer::new(
            self.context.types,
            self.context.symbols,
            self.context.diagnostics,
            self.context.fallback_span,
        )
        .is_enum_type(ty, span, out.reborrow())
    }

    fn fail<'slot, T>(
        &mut self,
        message: &'static str,
        span: SourceSpan,
        out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, T> {
        self.context
            .diagnostics
            .push(self.context.diagnostics.make_semantic(message, span));
        Err(out.fail(BuildFailure::empty()))
    }
}
