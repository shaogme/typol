use crate::{
    diagnostics::{BuildFailure, DiagnosticCollector},
    expressions::ExpressionType,
    module_tree::TypeDeclarationKind,
    symbols::{FieldSymbolId, SymbolTable, TypeSymbolId, VariantSymbolId},
    types::{ResolvedTypeGraph, ResolvedTypeKind, TypeShapeId, TypeUseId},
};
use typol_i18n::diagnostic_message;

use opeo::{OResult, Out};
use typol_core::{ChargedVec, DiagnosticMessage, SourceSpan};
use typol_syntax::{AstAccessError, BuiltinType, VariantPayloadKind};

#[derive(Clone, Copy)]
pub(super) enum TypeShape {
    Builtin(BuiltinType),
    Option(TypeUseId),
    Array(TypeUseId, Option<u32>),
    Map(TypeUseId),
    User(TypeSymbolId),
    VariantStructure(VariantSymbolId),
}

pub(super) struct TypeAnalyzer<'t, 'src, 'd> {
    types: &'t ResolvedTypeGraph,
    symbols: &'t SymbolTable<'src>,
    diagnostics: &'d mut DiagnosticCollector,
    fallback_span: SourceSpan,
}

impl<'t, 'src, 'd> TypeAnalyzer<'t, 'src, 'd> {
    fn ast_access<'slot, 'borrow, T>(
        &self,
        out: Out<'slot, 'borrow, BuildFailure>,
        operation: impl for<'call> FnOnce(Out<'call, 'call, AstAccessError>) -> OResult<'call, T>,
    ) -> OResult<'slot, T> {
        self.symbols
            .context()
            .ast_access(out, operation, BuildFailure::from)
    }

    pub(super) fn new(
        types: &'t ResolvedTypeGraph,
        symbols: &'t SymbolTable<'src>,
        diagnostics: &'d mut DiagnosticCollector,
        fallback_span: SourceSpan,
    ) -> Self {
        Self {
            types,
            symbols,
            diagnostics,
            fallback_span,
        }
    }

    pub(super) fn shape<'slot>(
        &mut self,
        ty: ExpressionType,
        span: SourceSpan,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, TypeShape> {
        self.symbols
            .context()
            .consume(1, span, self.diagnostics, out.reborrow())?;
        match ty {
            ExpressionType::Builtin(builtin) => Ok(TypeShape::Builtin(builtin)),
            ExpressionType::User(symbol) => Ok(TypeShape::User(symbol)),
            ExpressionType::VariantStructure(variant) => Ok(TypeShape::VariantStructure(variant)),
            ExpressionType::Resolved(id) => {
                let Some(use_) = self.types.type_use(id) else {
                    return self.internal(span, out.reborrow());
                };
                Ok(match use_.kind() {
                    ResolvedTypeKind::Builtin(builtin) => TypeShape::Builtin(builtin),
                    ResolvedTypeKind::Option(inner) => TypeShape::Option(inner),
                    ResolvedTypeKind::Array { element, length } => {
                        TypeShape::Array(element, length)
                    }
                    ResolvedTypeKind::Map(value) => TypeShape::Map(value),
                    ResolvedTypeKind::User(symbol) => TypeShape::User(symbol),
                })
            }
        }
    }

    pub(super) fn builtin_type<'slot>(
        &mut self,
        ty: ExpressionType,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, Option<BuiltinType>> {
        match self.shape(ty, self.fallback_span, out.reborrow())? {
            TypeShape::Builtin(builtin) => Ok(Some(builtin)),
            _ => Ok(None),
        }
    }

    pub(super) fn is_builtin<'slot>(
        &mut self,
        ty: ExpressionType,
        expected: BuiltinType,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, bool> {
        Ok(self.builtin_type(ty, out.reborrow())? == Some(expected))
    }

    pub(super) fn is_option<'slot>(
        &mut self,
        ty: ExpressionType,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, bool> {
        Ok(matches!(
            self.shape(ty, self.fallback_span, out.reborrow())?,
            TypeShape::Option(_)
        ))
    }

    pub(super) fn is_enum_type<'slot>(
        &mut self,
        ty: ExpressionType,
        span: SourceSpan,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, bool> {
        let TypeShape::User(symbol) = self.shape(ty, span, out.reborrow())? else {
            return Ok(false);
        };
        let type_symbol =
            self.diagnostics
                .require(self.symbols.type_symbol(symbol), span, out.reborrow())?;
        Ok(matches!(
            type_symbol.declaration(),
            TypeDeclarationKind::Enum(_)
        ))
    }

    pub(super) fn same_type<'slot>(
        &mut self,
        left: ExpressionType,
        right: ExpressionType,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, bool> {
        self.symbols
            .context()
            .consume(4, self.fallback_span, self.diagnostics, out.reborrow())?;
        let shape_ids: Option<(TypeShapeId, TypeShapeId)> =
            self.types.shape_id(left).zip(self.types.shape_id(right));
        if let Some((left, right)) = shape_ids {
            return Ok(left == right);
        }
        let (mut left, mut right) = (left, right);
        loop {
            self.symbols.context().consume(
                1,
                self.fallback_span,
                self.diagnostics,
                out.reborrow(),
            )?;
            match (
                self.shape(left, self.fallback_span, out.reborrow())?,
                self.shape(right, self.fallback_span, out.reborrow())?,
            ) {
                (TypeShape::Builtin(a), TypeShape::Builtin(b)) => {
                    break Ok(a == b);
                }
                (TypeShape::User(a), TypeShape::User(b)) => {
                    break Ok(a == b);
                }
                (TypeShape::VariantStructure(a), TypeShape::VariantStructure(b)) => {
                    break Ok(a == b);
                }
                (TypeShape::Option(a), TypeShape::Option(b))
                | (TypeShape::Map(a), TypeShape::Map(b)) => {
                    left = ExpressionType::Resolved(a);
                    right = ExpressionType::Resolved(b);
                }
                (TypeShape::Array(a, a_len), TypeShape::Array(b, b_len)) if a_len == b_len => {
                    left = ExpressionType::Resolved(a);
                    right = ExpressionType::Resolved(b);
                }
                _ => break Ok(false),
            }
        }
    }

    pub(super) fn is_equality_comparable<'slot>(
        &mut self,
        ty: ExpressionType,
        inside_enum_payload: bool,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, bool> {
        let mut pending = self
            .diagnostics
            .new_vec(self.symbols.context().execution(), out.reborrow())?;
        pending.try_reserve(1, out.reborrow())?;
        pending.try_push((ty, inside_enum_payload), out.reborrow())?;
        let mut visited_types = self
            .diagnostics
            .new_vec(self.symbols.context().execution(), out.reborrow())?;
        visited_types.try_reserve_exact(self.symbols.types().len(), out.reborrow())?;
        for _ in 0..self.symbols.types().len() {
            visited_types.try_push(false, out.reborrow())?;
        }

        while let Some((current, nested)) = pending.pop() {
            self.symbols.context().consume(
                1,
                self.fallback_span,
                self.diagnostics,
                out.reborrow(),
            )?;
            match self.shape(current, self.fallback_span, out.reborrow())? {
                TypeShape::Builtin(_) => {}
                TypeShape::Option(inner) => {
                    pending.try_reserve(1, out.reborrow())?;
                    pending.try_push((ExpressionType::Resolved(inner), nested), out.reborrow())?;
                }
                TypeShape::Array(..) | TypeShape::Map(_) => {
                    return Ok(false);
                }
                TypeShape::VariantStructure(variant) => {
                    if !nested {
                        return Ok(false);
                    }
                    let Some(variant) = self.symbols.variant_symbol(variant) else {
                        return self.fail(
                            "variant handle is invalid",
                            self.fallback_span,
                            out.reborrow(),
                        );
                    };
                    for field_id in variant.fields() {
                        self.push_comparable_field(*field_id, &mut pending, out.reborrow())?;
                    }
                }
                TypeShape::User(symbol) => {
                    let Some(visited) = visited_types.get_mut(symbol.index()) else {
                        return self.fail(
                            "expression type handle is invalid",
                            self.fallback_span,
                            out.reborrow(),
                        );
                    };
                    if *visited {
                        continue;
                    }
                    *visited = true;
                    let Some(type_symbol) = self.symbols.type_symbol(symbol) else {
                        return self.fail(
                            "expression type handle is invalid",
                            self.fallback_span,
                            out.reborrow(),
                        );
                    };
                    match type_symbol.declaration() {
                        TypeDeclarationKind::Struct(_) => {
                            if !nested {
                                return Ok(false);
                            }
                            for field_id in type_symbol.fields() {
                                self.push_comparable_field(
                                    *field_id,
                                    &mut pending,
                                    out.reborrow(),
                                )?;
                            }
                        }
                        TypeDeclarationKind::Enum(declaration) => {
                            let variants = self.ast_access(out.reborrow(), |ast_out| {
                                declaration.variants(ast_out)
                            })?;
                            for index in 0..variants.len() {
                                let Some(variant) = self.ast_access(out.reborrow(), |ast_out| {
                                    variants.get(index, ast_out)
                                })?
                                else {
                                    return self.fail(
                                        "enum variant list index is invalid",
                                        self.fallback_span,
                                        out.reborrow(),
                                    );
                                };
                                let Some(variant_id) = type_symbol.variants().get(index) else {
                                    return self.fail(
                                        "variant handle is invalid",
                                        variant.span(),
                                        out.reborrow(),
                                    );
                                };
                                self.symbols.context().consume(
                                    1,
                                    variant.span(),
                                    self.diagnostics,
                                    out.reborrow(),
                                )?;
                                let payload = self.ast_access(out.reborrow(), |ast_out| {
                                    variant.payload(ast_out)
                                })?;
                                let payload_kind = self
                                    .ast_access(out.reborrow(), |ast_out| payload.kind(ast_out))?;
                                match payload_kind {
                                    VariantPayloadKind::Unit => {}
                                    VariantPayloadKind::Single(_) => {
                                        let Some(payload) =
                                            self.types.variant_payload_type(*variant_id)
                                        else {
                                            return self.fail(
                                                "variant payload type is unavailable",
                                                variant.span(),
                                                out.reborrow(),
                                            );
                                        };
                                        pending.try_reserve(1, out.reborrow())?;
                                        pending.try_push(
                                            (ExpressionType::Resolved(payload), true),
                                            out.reborrow(),
                                        )?;
                                    }
                                    VariantPayloadKind::Structure(_) => {
                                        let Some(variant_symbol) =
                                            self.symbols.variant_symbol(*variant_id)
                                        else {
                                            return self.fail(
                                                "variant handle is invalid",
                                                variant.span(),
                                                out.reborrow(),
                                            );
                                        };
                                        for field_id in variant_symbol.fields() {
                                            self.push_comparable_field(
                                                *field_id,
                                                &mut pending,
                                                out.reborrow(),
                                            )?;
                                        }
                                    }
                                }
                            }
                        }
                        TypeDeclarationKind::Alias(_) => {
                            return Ok(false);
                        }
                    }
                }
            }
        }
        Ok(true)
    }

    fn push_comparable_field<'slot>(
        &mut self,
        field: FieldSymbolId,
        pending: &mut ChargedVec<(ExpressionType, bool)>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ()> {
        self.symbols
            .context()
            .consume(1, self.fallback_span, self.diagnostics, out.reborrow())?;
        let Some(field_type) = self.types.field_type(field) else {
            return self.fail(
                diagnostic_message!(
                    "diagnostic.schema.field_type_unavailable",
                    "field type is unavailable"
                ),
                self.fallback_span,
                out.reborrow(),
            );
        };
        pending.try_reserve(1, out.reborrow())?;
        pending.try_push((ExpressionType::Resolved(field_type), true), out.reborrow())?;
        Ok(())
    }

    fn fail<'slot, 'message, T>(
        &mut self,
        message: impl Into<DiagnosticMessage<'message>>,
        span: SourceSpan,
        out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, T> {
        self.diagnostics
            .push(self.diagnostics.make_semantic(message, span));
        Err(out.fail(BuildFailure::empty()))
    }

    fn internal<'slot, T>(
        &mut self,
        span: SourceSpan,
        out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, T> {
        self.diagnostics.abort(schema_internal!(span));
        Err(out.fail(BuildFailure::empty()))
    }
}
