mod support;

pub(crate) use support::PreparedRecordCompilation;
pub(super) use support::RecordCompilation;
use support::{QualifiedNameInput, RecordSupport, TypeIdRequest};

use crate::{
    SchemaCompileStage,
    context::CompileContext,
    diagnostics::{BuildFailure, DiagnosticCollector},
    expressions::ExpressionType,
    module_tree::{ModuleTree, TypeDeclarationKind},
    symbols::SymbolTable,
    types::{ResolvedTypeGraph, ResolvedTypeKind},
};
use opeo::{OResult, Out};
use typol_semantic::{Builder, ShapeKind, TypeId, TypeRecord, TypeRecordOptions};

struct RecordPrepareInput<'a, 'src> {
    pub(crate) tree: &'a ModuleTree<'src>,
    pub(crate) symbols: &'a SymbolTable<'src>,
    pub(crate) types: &'a ResolvedTypeGraph,
    pub(crate) diagnostics: &'a mut DiagnosticCollector,
    pub(crate) context: &'a CompileContext,
}

pub(crate) struct RecordBuilder;

impl RecordBuilder {
    fn prepare<'slot, 'a, 'src>(
        input: RecordPrepareInput<'a, 'src>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, Builder> {
        let RecordPrepareInput {
            tree,
            symbols,
            types,
            diagnostics,
            context,
        } = input;
        let fallback = tree.root_span();
        context
            .execution()
            .set_stage(SchemaCompileStage::Records as usize);
        let execution = context.execution();
        let value_limits = *context.limits().values();
        let regex_limits = *context.limits().regex();
        let mut model =
            context.builder_access(out.reborrow(), fallback, diagnostics, |builder_out| {
                Builder::new(execution, value_limits, regex_limits, builder_out)
            })?;
        for _ in types.semantic_types() {
            execution.consume(1, out.reborrow())?;
            context.builder_access(out.reborrow(), fallback, diagnostics, |builder_out| {
                model.reserve_type(fallback, builder_out)
            })?;
        }
        for (index, ty) in types.semantic_types().iter().copied().enumerate() {
            execution.consume(1, out.reborrow())?;
            let id = TypeId::new(index);
            let record = match ty {
                ExpressionType::User(symbol) => {
                    let Some(symbol) = symbols.type_symbol(symbol) else {
                        diagnostics.push(schema_internal!(fallback));
                        return Err(out.fail(BuildFailure::empty()));
                    };
                    let (kind, type_span) = match symbol.declaration() {
                        TypeDeclarationKind::Struct(declaration) => {
                            (ShapeKind::Struct(id), declaration.span())
                        }
                        TypeDeclarationKind::Enum(declaration) => {
                            (ShapeKind::Enum(id), declaration.span())
                        }
                        TypeDeclarationKind::Alias(_) => continue,
                    };
                    let shape = context.builder_access(
                        out.reborrow(),
                        symbol.name_span(),
                        diagnostics,
                        |builder_out| model.intern_shape(kind, builder_out),
                    )?;
                    let name = RecordSupport::copy_text(symbol.name(), execution, out.reborrow())?;
                    let qualified_name = RecordSupport::qualified_name(
                        QualifiedNameInput {
                            module_id: symbol.module(),
                            name: symbol.name(),
                            tree,
                            execution,
                            diagnostics,
                            fallback: type_span,
                        },
                        out.reborrow(),
                    )?;
                    let mut record = context.builder_access(
                        out.reborrow(),
                        type_span,
                        diagnostics,
                        |builder_out| {
                            TypeRecord::new(
                                TypeRecordOptions {
                                    name: Some(name),
                                    qualified_name: Some(qualified_name),
                                    shape,
                                    child: None,
                                    span: type_span,
                                },
                                execution,
                                builder_out,
                            )
                        },
                    )?;
                    record.set_fields(RecordSupport::copy_slice(
                        symbol.fields(),
                        diagnostics,
                        execution,
                        out.reborrow(),
                    )?);
                    record.set_variants(RecordSupport::copy_slice(
                        symbol.variants(),
                        diagnostics,
                        execution,
                        out.reborrow(),
                    )?);
                    record
                }
                ExpressionType::Builtin(builtin) => {
                    let shape = context.builder_access(
                        out.reborrow(),
                        fallback,
                        diagnostics,
                        |builder_out| model.intern_shape(ShapeKind::Builtin(builtin), builder_out),
                    )?;
                    context.builder_access(
                        out.reborrow(),
                        fallback,
                        diagnostics,
                        |builder_out| {
                            TypeRecord::new(
                                TypeRecordOptions {
                                    name: None,
                                    qualified_name: None,
                                    shape,
                                    child: None,
                                    span: fallback,
                                },
                                execution,
                                builder_out,
                            )
                        },
                    )?
                }
                ExpressionType::VariantStructure(variant) => {
                    let Some(variant) = symbols.variant_symbol(variant) else {
                        diagnostics.push(schema_internal!(fallback));
                        return Err(out.fail(BuildFailure::empty()));
                    };
                    let span = variant.declaration().span();
                    let shape = context.builder_access(
                        out.reborrow(),
                        span,
                        diagnostics,
                        |builder_out| model.intern_shape(ShapeKind::Struct(id), builder_out),
                    )?;
                    let mut record = context.builder_access(
                        out.reborrow(),
                        span,
                        diagnostics,
                        |builder_out| {
                            TypeRecord::new(
                                TypeRecordOptions {
                                    name: None,
                                    qualified_name: None,
                                    shape,
                                    child: None,
                                    span,
                                },
                                execution,
                                builder_out,
                            )
                        },
                    )?;
                    record.set_fields(RecordSupport::copy_slice(
                        variant.fields(),
                        diagnostics,
                        execution,
                        out.reborrow(),
                    )?);
                    record
                }
                ExpressionType::Resolved(use_id) => {
                    let Some(use_) = types.type_use(use_id) else {
                        diagnostics.push(schema_internal!(fallback));
                        return Err(out.fail(BuildFailure::empty()));
                    };
                    let span = use_.span();
                    let (shape, child) = match use_.kind() {
                        ResolvedTypeKind::Builtin(builtin) => (
                            context.builder_access(
                                out.reborrow(),
                                span,
                                diagnostics,
                                |builder_out| {
                                    model.intern_shape(ShapeKind::Builtin(builtin), builder_out)
                                },
                            )?,
                            None,
                        ),
                        ResolvedTypeKind::User(symbol) => {
                            let user_id = RecordSupport::type_id(
                                TypeIdRequest {
                                    types,
                                    ty: ExpressionType::User(symbol),
                                    span,
                                    diagnostics,
                                    execution,
                                },
                                out.reborrow(),
                            )?;
                            (
                                context.builder_access(
                                    out.reborrow(),
                                    span,
                                    diagnostics,
                                    |builder_out| model.type_shape(user_id, builder_out),
                                )?,
                                None,
                            )
                        }
                        ResolvedTypeKind::Option(child)
                        | ResolvedTypeKind::Map(child)
                        | ResolvedTypeKind::Array { element: child, .. } => {
                            let child = RecordSupport::type_id(
                                TypeIdRequest {
                                    types,
                                    ty: ExpressionType::Resolved(child),
                                    span,
                                    diagnostics,
                                    execution,
                                },
                                out.reborrow(),
                            )?;
                            let shape = context.builder_access(
                                out.reborrow(),
                                span,
                                diagnostics,
                                |builder_out| model.type_shape(child, builder_out),
                            )?;
                            let kind = match use_.kind() {
                                ResolvedTypeKind::Option(_) => ShapeKind::Option(shape),
                                ResolvedTypeKind::Map(_) => ShapeKind::Map(shape),
                                ResolvedTypeKind::Array { length, .. } => ShapeKind::Array {
                                    element: shape,
                                    length,
                                },
                                ResolvedTypeKind::Builtin(_) | ResolvedTypeKind::User(_) => {
                                    diagnostics.abort(schema_internal!(span));
                                    return Err(out.fail(BuildFailure::empty()));
                                }
                            };
                            (
                                context.builder_access(
                                    out.reborrow(),
                                    span,
                                    diagnostics,
                                    |builder_out| model.intern_shape(kind, builder_out),
                                )?,
                                Some(child),
                            )
                        }
                    };
                    context.builder_access(out.reborrow(), span, diagnostics, |builder_out| {
                        TypeRecord::new(
                            TypeRecordOptions {
                                name: None,
                                qualified_name: None,
                                shape,
                                child,
                                span,
                            },
                            execution,
                            builder_out,
                        )
                    })?
                }
            };
            context.builder_access(out.reborrow(), fallback, diagnostics, |builder_out| {
                model.set_type(id, record, builder_out)
            })?;
        }
        for symbol in symbols.types() {
            execution.consume(1, out.reborrow())?;
            if let TypeDeclarationKind::Alias(alias) = symbol.declaration() {
                let Some(alias_target) = types.alias_target(symbol.id()) else {
                    diagnostics.push(schema_internal!(alias.span()));
                    return Err(out.fail(BuildFailure::empty()));
                };
                let target = RecordSupport::type_id(
                    TypeIdRequest {
                        types,
                        ty: ExpressionType::Resolved(alias_target),
                        span: alias.span(),
                        diagnostics,
                        execution,
                    },
                    out.reborrow(),
                )?;
                let shape = context.builder_access(
                    out.reborrow(),
                    alias.span(),
                    diagnostics,
                    |builder_out| model.type_shape(target, builder_out),
                )?;
                let child = types
                    .type_use(alias_target)
                    .and_then(|use_| match use_.kind() {
                        ResolvedTypeKind::Option(child)
                        | ResolvedTypeKind::Map(child)
                        | ResolvedTypeKind::Array { element: child, .. } => {
                            types.semantic_id(ExpressionType::Resolved(child))
                        }
                        _ => None,
                    });
                let name = RecordSupport::copy_text(symbol.name(), execution, out.reborrow())?;
                let record = context.builder_access(
                    out.reborrow(),
                    alias.span(),
                    diagnostics,
                    |builder_out| {
                        TypeRecord::new(
                            TypeRecordOptions {
                                name: Some(name),
                                qualified_name: None,
                                shape,
                                child,
                                span: alias.span(),
                            },
                            execution,
                            builder_out,
                        )
                    },
                )?;
                let id = RecordSupport::type_id(
                    TypeIdRequest {
                        types,
                        ty: ExpressionType::User(symbol.id()),
                        span: alias.span(),
                        diagnostics,
                        execution,
                    },
                    out.reborrow(),
                )?;
                context.builder_access(
                    out.reborrow(),
                    alias.span(),
                    diagnostics,
                    |builder_out| model.set_type(id, record, builder_out),
                )?;
            }
        }
        Ok(model)
    }

    pub(super) fn compile<'slot>(
        compilation: RecordCompilation<'_, '_>,
        diagnostics: &mut DiagnosticCollector,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, PreparedRecordCompilation> {
        let model = Self::prepare(
            RecordPrepareInput {
                tree: compilation.tree,
                symbols: compilation.symbols,
                types: compilation.types,
                diagnostics,
                context: compilation.context,
            },
            out.reborrow(),
        )?;
        RecordSupport::compile(model, compilation, diagnostics, out)
    }
}
