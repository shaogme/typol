use crate::{
    annotations::{AnnotationCompilation, CompiledConstraint, ConstraintData},
    diagnostics::EmergencyDiagnostics,
    expressions::ExpressionType,
    module_tree::TypeDeclarationKind,
    symbols::{FieldOwner, SymbolTable},
    types::{ResolvedTypeGraph, ResolvedTypeKind, TypeUseId},
};
use opeo::{OResult, Out, ResultOutExt};
use typol_semantic::{
    EvalError, ExecutionField, ExecutionModel, ExecutionType, ExecutionVariant, FieldId,
    StoredPayloadInfo, StoredTypeKind, TypeId, VariantId,
};
use typol_syntax::VariantPayloadKind;

pub(crate) struct CompileModel<'a, 'src> {
    types: &'a ResolvedTypeGraph,
    symbols: &'a SymbolTable<'src>,
    annotations: &'a AnnotationCompilation,
}

impl<'a, 'src> CompileModel<'a, 'src> {
    pub(crate) fn new(
        types: &'a ResolvedTypeGraph,
        symbols: &'a SymbolTable<'src>,
        annotations: &'a AnnotationCompilation,
    ) -> Self {
        Self {
            types,
            symbols,
            annotations,
        }
    }

    fn consume<'slot>(
        &self,
        count: usize,
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, ()> {
        self.symbols
            .context()
            .execution()
            .consume::<EvalError>(count, out.reborrow())?;
        Ok(())
    }

    fn rules_host<'slot>(
        &self,
        rules: &[CompiledConstraint],
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, bool> {
        for rule in rules {
            self.consume(1, out.reborrow())?;
            if matches!(rule.data(), ConstraintData::Host) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(crate) fn use_id<'slot>(
        &self,
        id: TypeUseId,
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, Option<TypeId>> {
        self.consume(1, out.reborrow())?;
        Ok(self.types.semantic_id(ExpressionType::Resolved(id)))
    }

    fn use_kind(&self, id: TypeUseId) -> Option<StoredTypeKind<'_>> {
        Some(match self.types.type_use(id)?.kind() {
            ResolvedTypeKind::Builtin(b) => StoredTypeKind::Builtin(b),
            ResolvedTypeKind::Option(inner) => StoredTypeKind::Option {
                inner: self.types.semantic_id(ExpressionType::Resolved(inner))?,
            },
            ResolvedTypeKind::Array { element, length } => StoredTypeKind::Array {
                element: self.types.semantic_id(ExpressionType::Resolved(element))?,
                length,
            },
            ResolvedTypeKind::Map(value) => StoredTypeKind::Map {
                value: self.types.semantic_id(ExpressionType::Resolved(value))?,
            },
            ResolvedTypeKind::User(id) => {
                let symbol = self.symbols.type_symbol(id)?;
                match symbol.declaration() {
                    TypeDeclarationKind::Struct(_) => StoredTypeKind::Struct {
                        fields: symbol.fields(),
                    },
                    TypeDeclarationKind::Enum(_) => StoredTypeKind::Enum {
                        variants: symbol.variants(),
                    },
                    TypeDeclarationKind::Alias(_) => return None,
                }
            }
        })
    }

    fn nominal(&self, id: TypeId) -> Option<(u8, usize)> {
        match self.types.expression_type(id)? {
            ExpressionType::User(id) => {
                if let Some(target) = self.types.alias_target(id) {
                    match self.types.type_use(target)?.kind() {
                        ResolvedTypeKind::User(id) => Some((0, id.index())),
                        _ => None,
                    }
                } else {
                    Some((0, id.index()))
                }
            }
            ExpressionType::VariantStructure(id) => Some((1, id.index())),
            ExpressionType::Resolved(id) => match self.types.type_use(id)?.kind() {
                ResolvedTypeKind::User(id) => Some((0, id.index())),
                _ => None,
            },
            _ => None,
        }
    }

    fn use_host<'slot>(
        &self,
        id: TypeUseId,
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, Option<bool>> {
        self.consume(1, out.reborrow())?;
        let Some(use_) = self.types.type_use(id) else {
            return Ok(None);
        };
        if self.rules_host(
            self.annotations.type_use_constraints(id).unwrap_or(&[]),
            out.reborrow(),
        )? {
            return Ok(Some(true));
        }
        let mut link = use_.alias_chain();
        while let Some(id) = link {
            self.consume(1, out.reborrow())?;
            let Some(alias) = self.types.alias_link(id) else {
                return Ok(None);
            };
            let is_host = self.rules_host(
                self.annotations
                    .alias_constraints(alias.symbol())
                    .unwrap_or(&[]),
                out.reborrow(),
            )?;
            if is_host {
                return Ok(Some(true));
            }
            link = alias.next();
        }
        Ok(Some(false))
    }
}

impl ExecutionModel for CompileModel<'_, '_> {
    fn type_info<'slot>(
        &self,
        id: TypeId,
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, Option<ExecutionType<'_>>> {
        self.consume(5, out.reborrow())?;
        Ok((|| {
            let kind = match self.types.expression_type(id)? {
                ExpressionType::Resolved(id) => self.use_kind(id)?,
                ExpressionType::Builtin(b) => StoredTypeKind::Builtin(b),
                ExpressionType::VariantStructure(id) => StoredTypeKind::Struct {
                    fields: self.symbols.variant_symbol(id)?.fields(),
                },
                ExpressionType::User(id) => {
                    let symbol = self.symbols.type_symbol(id)?;
                    match symbol.declaration() {
                        TypeDeclarationKind::Struct(_) => StoredTypeKind::Struct {
                            fields: symbol.fields(),
                        },
                        TypeDeclarationKind::Enum(_) => StoredTypeKind::Enum {
                            variants: symbol.variants(),
                        },
                        TypeDeclarationKind::Alias(_) => {
                            self.use_kind(self.types.alias_target(id)?)?
                        }
                    }
                }
            };
            Some(ExecutionType::new(kind))
        })())
    }

    fn field_info<'slot>(
        &self,
        id: FieldId,
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, Option<ExecutionField<'_>>> {
        self.consume(4, out.reborrow())?;
        let host = self.rules_host(
            self.annotations.field_constraints(id).unwrap_or(&[]),
            out.reborrow(),
        )?;
        let Some(field) = self.symbols.field_symbol(id) else {
            return Ok(None);
        };
        let Some(type_use) = self.types.field_type(id) else {
            return Ok(None);
        };
        let Some(type_id) = self.types.semantic_id(ExpressionType::Resolved(type_use)) else {
            return Ok(None);
        };
        let Some(owner_fields) = (match field.owner() {
            FieldOwner::Type(owner) => self.symbols.type_symbol(owner).map(|owner| owner.fields()),
            FieldOwner::Variant(owner) => self
                .symbols
                .variant_symbol(owner)
                .map(|owner| owner.fields()),
        }) else {
            return Ok(None);
        };
        let slot = field.slot();
        self.consume(1, out.reborrow())?;
        if owner_fields.get(slot) != Some(&id) {
            return Ok(None);
        }
        let name = self.symbols.context().ast_access(
            out.reborrow(),
            |ast_out| field.declaration().name(ast_out),
            |error| {
                EvalError::internal(EmergencyDiagnostics::ast_access_context(error))
                    .with_source_span(field.declaration().span())
            },
        )?;
        Ok(Some(ExecutionField::new(name, type_id, host, slot)))
    }

    fn variant_info<'slot>(
        &self,
        id: VariantId,
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, Option<ExecutionVariant<'_>>> {
        self.consume(4, out.reborrow())?;
        let host = self.rules_host(
            self.annotations
                .variant_payload_constraints(id)
                .unwrap_or(&[]),
            out.reborrow(),
        )?;
        let Some(variant) = self.symbols.variant_symbol(id) else {
            return Ok(None);
        };
        let payload_view = self.symbols.context().ast_access(
            out.reborrow(),
            |ast_out| variant.declaration().payload(ast_out),
            |error| {
                EvalError::internal(EmergencyDiagnostics::ast_access_context(error))
                    .with_source_span(variant.declaration().span())
            },
        )?;
        let payload_kind = self.symbols.context().ast_access(
            out.reborrow(),
            |ast_out| payload_view.kind(ast_out),
            |error| {
                EvalError::internal(EmergencyDiagnostics::ast_access_context(error))
                    .with_source_span(variant.declaration().span())
            },
        )?;
        let payload = match payload_kind {
            VariantPayloadKind::Unit => StoredPayloadInfo::Unit,
            VariantPayloadKind::Single(_) => StoredPayloadInfo::Single {
                ty: {
                    let Some(type_use) = self.types.variant_payload_type(id) else {
                        return Ok(None);
                    };
                    let Some(type_id) = self.types.semantic_id(ExpressionType::Resolved(type_use))
                    else {
                        return Ok(None);
                    };
                    type_id
                },
            },
            VariantPayloadKind::Structure(_) => StoredPayloadInfo::Structure {
                fields: variant.fields(),
            },
        };
        let name = self.symbols.context().ast_access(
            out.reborrow(),
            |ast_out| variant.declaration().name(ast_out),
            |error| {
                EvalError::internal(EmergencyDiagnostics::ast_access_context(error))
                    .with_source_span(variant.declaration().span())
            },
        )?;
        Ok(Some(ExecutionVariant::new(name, payload, host)))
    }

    fn same_type<'slot>(
        &self,
        mut left: TypeId,
        mut right: TypeId,
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, bool> {
        loop {
            self.consume(1, out.reborrow())?;
            if left == right {
                return Ok(true);
            }
            let left_info = self.type_info(left, out.reborrow())?;
            let right_info = self.type_info(right, out.reborrow())?;
            let (Some(left_info), Some(right_info)) = (left_info, right_info) else {
                return Ok(false);
            };
            match (left_info.kind(), right_info.kind()) {
                (StoredTypeKind::Builtin(a), StoredTypeKind::Builtin(b)) => {
                    return Ok(a == b);
                }
                (StoredTypeKind::Struct { fields: a }, StoredTypeKind::Struct { fields: b }) => {
                    let steps = a
                        .len()
                        .min(b.len())
                        .checked_add(6)
                        .ok_or(EvalError::Resource)
                        .store_err_into(out.reborrow())?;
                    self.consume(steps, out.reborrow())?;
                    return Ok(a == b && self.nominal(left) == self.nominal(right));
                }
                (StoredTypeKind::Enum { variants: a }, StoredTypeKind::Enum { variants: b }) => {
                    let steps = a
                        .len()
                        .min(b.len())
                        .checked_add(6)
                        .ok_or(EvalError::Resource)
                        .store_err_into(out.reborrow())?;
                    self.consume(steps, out.reborrow())?;
                    return Ok(a == b && self.nominal(left) == self.nominal(right));
                }
                (StoredTypeKind::Option { inner: a }, StoredTypeKind::Option { inner: b })
                | (StoredTypeKind::Map { value: a }, StoredTypeKind::Map { value: b }) => {
                    left = a;
                    right = b;
                }
                (
                    StoredTypeKind::Array {
                        element: a,
                        length: al,
                    },
                    StoredTypeKind::Array {
                        element: b,
                        length: bl,
                    },
                ) if al == bl => {
                    left = a;
                    right = b;
                }
                _ => return Ok(false),
            }
        }
    }

    fn host<'slot>(
        &self,
        id: TypeId,
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, Option<bool>> {
        self.consume(2, out.reborrow())?;
        match self.types.expression_type(id) {
            Some(ExpressionType::Resolved(id)) => self.use_host(id, out.reborrow()),
            Some(ExpressionType::User(id)) => match self.types.alias_target(id) {
                Some(id) => self.use_host(id, out.reborrow()),
                None => Ok(Some(false)),
            },
            Some(_) => Ok(Some(false)),
            None => Ok(None),
        }
    }

    fn entry_type<'slot>(
        &self,
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, Option<TypeId>> {
        self.consume(2, out.reborrow())?;
        Ok(self
            .symbols
            .entry_type()
            .and_then(|id| self.types.semantic_id(ExpressionType::User(id))))
    }
}
