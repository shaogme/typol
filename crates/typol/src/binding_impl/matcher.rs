mod fields;
mod index;
mod support;

use crate::{
    binding_impl::{
        BindingErrors, FieldBindings, TypeBinding, VariantBinding,
        errors::{
            BindingErrorCollector, BindingMismatchKind, BindingPath, BindingShape, MismatchDetails,
            PathSegments,
        },
    },
    descriptor::{DescribeContext, DescriptorNode, DescriptorProvider, TypeShape, VariantShape},
    diagnostics::{CodecErrors, CodecStage, Failure},
    options::MAX_CONVERSION_DEPTH,
};
use core::any::TypeId;
use fields::{DuplicateCheck, FieldMatch, FieldMatcher};
use index::BindingIndex;
use opeo::{OResult, Out, ResultOutExt};
use support::MatcherSupport;
use typol_core::{BudgetError, ChargedVec, DiagnosticContext, diagnostic_context};
use typol_semantic::{TypeKind, TypeView, VariantPayloadInfo};

static BINDING_INVARIANT: DiagnosticContext =
    diagnostic_context!("typol", "binding", "binding-plan-invariant");

pub(crate) struct Pending<'schema> {
    pub(crate) provider: DescriptorProvider,
    pub(crate) ty: TypeView<'schema>,
    pub(crate) depth: usize,
    pub(crate) path: BindingPath,
    pub(crate) field_context: bool,
}

pub(crate) enum Task<'schema> {
    Enter(Pending<'schema>),
    Exit(TypeBinding<'schema>),
}

struct NameCheck<'name, 'path, 'schema> {
    name: Option<&'name str>,
    ty: TypeView<'schema>,
    path: &'path BindingPath,
    root: bool,
}

#[derive(Clone, Copy)]
pub(crate) struct ActiveBinding<'schema> {
    identity: TypeId,
    ty: TypeView<'schema>,
}

pub(crate) struct Matcher<'schema> {
    stack: ChargedVec<Task<'schema>>,
    active: ChargedVec<ActiveBinding<'schema>>,
    types: ChargedVec<TypeBinding<'schema>>,
    index: BindingIndex<'schema>,
    errors: BindingErrorCollector,
}

pub(crate) struct MatcherBuffers<'schema> {
    pub(crate) stack: ChargedVec<Task<'schema>>,
    pub(crate) active: ChargedVec<ActiveBinding<'schema>>,
    pub(crate) types: ChargedVec<TypeBinding<'schema>>,
}

impl<'schema> Matcher<'schema> {
    pub(crate) fn new<'slot>(
        buffers: MatcherBuffers<'schema>,
        context: &DescribeContext<'_>,
        diagnostic_limit: usize,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, Self> {
        let MatcherBuffers {
            stack,
            active,
            types,
        } = buffers;
        let index = BindingIndex::new(context.session(), out.reborrow())?;
        let errors =
            BindingErrorCollector::new(diagnostic_limit, context.session(), out.reborrow())?;
        Ok(Self {
            stack,
            active,
            types,
            index,
            errors,
        })
    }

    pub(crate) fn pop_task(&mut self) -> Option<Task<'schema>> {
        self.stack.pop()
    }

    pub(crate) fn finish_binding<'slot>(
        &mut self,
        binding: TypeBinding<'schema>,
        context: &DescribeContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ()> {
        self.active
            .pop()
            .ok_or_else(internal_error)
            .store_err_into(out.reborrow())?;
        let identity = binding.identity;
        let ty = binding.ty;
        context.session().budget_codec(
            CodecStage::Binding,
            |budget_out| self.types.try_push(binding, budget_out),
            out.reborrow(),
        )?;
        let inserted = self
            .index
            .insert(identity, ty, context.session(), out.reborrow())?;
        if !inserted {
            return Err(out.fail(internal_error()));
        }
        Ok(())
    }

    pub(crate) fn sort_types<'slot>(
        &mut self,
        context: &DescribeContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ()> {
        self.types.sort_by(
            |left, right, mut sort_out| {
                context.session().budget_codec(
                    CodecStage::Binding,
                    |budget_out| context.session().visit(budget_out),
                    sort_out.reborrow(),
                )?;
                Ok(left
                    .identity
                    .cmp(&right.identity)
                    .then_with(|| left.ty.cmp(&right.ty)))
            },
            out.reborrow(),
        )?;
        Ok(())
    }

    pub(crate) fn into_types(self) -> ChargedVec<TypeBinding<'schema>> {
        self.types
    }

    pub(crate) fn has_mismatches(&self) -> bool {
        !self.errors.is_empty()
    }

    pub(crate) fn is_truncated(&self) -> bool {
        self.errors.is_truncated()
    }

    pub(crate) fn into_errors(self) -> BindingErrors {
        self.errors.into_errors()
    }

    pub(crate) fn push<'slot>(
        &mut self,
        task: Task<'schema>,
        context: &DescribeContext<'_>,
        out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ()> {
        context.session().budget_codec(
            CodecStage::Binding,
            |budget_out| self.stack.try_push(task, budget_out),
            out,
        )
    }

    pub(crate) fn enter<'slot>(
        &mut self,
        pending: Pending<'schema>,
        context: &mut DescribeContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ()> {
        if pending.depth > MAX_CONVERSION_DEPTH {
            return Err(out.fail(CodecErrors::emergency(
                CodecStage::Binding,
                Failure::ConversionDepth {
                    requested_depth: pending.depth,
                    maximum_depth: MAX_CONVERSION_DEPTH,
                },
            )));
        }
        context.check_depth(pending.depth, out.reborrow())?;
        let identity = pending.provider.identity();
        for active in &self.active {
            context.session().budget_codec(
                CodecStage::Binding,
                |budget_out| context.session().visit(budget_out),
                out.reborrow(),
            )?;
            if active.identity == identity && active.ty == pending.ty {
                return Ok(());
            }
        }
        if self
            .index
            .contains(identity, pending.ty, context.session(), out.reborrow())?
        {
            return Ok(());
        }
        let descriptor = pending
            .provider
            .resolve(context, pending.depth, out.reborrow())?;
        let DescriptorNode::Type { name, shape, .. } = context.get(descriptor, out.reborrow())?
        else {
            return Err(out.fail(internal_error()));
        };
        let mut binding = TypeBinding {
            identity,
            ty: pending.ty,
            fields: FieldBindings::empty(),
            variants: context.session().budget_codec(
                CodecStage::Binding,
                |budget_out| context.session().workspace(budget_out),
                out.reborrow(),
            )?,
        };
        let depth = pending
            .depth
            .checked_add(1)
            .ok_or_else(|| MatcherSupport::binding_budget_error(BudgetError::Resource))
            .store_err_into(out.reborrow())?;
        let mut children = context.session().budget_codec(
            CodecStage::Binding,
            |budget_out| context.session().workspace(budget_out),
            out.reborrow(),
        )?;
        let kind = context.session().schema_codec(
            CodecStage::Binding,
            |schema_out| pending.ty.kind(schema_out),
            out.reborrow(),
        )?;
        match (shape, kind) {
            (TypeShape::Scalar(rust), TypeKind::Builtin(schema)) => {
                if *rust != schema {
                    self.errors.record(
                        pending.path,
                        MismatchDetails {
                            kind: if pending.field_context {
                                BindingMismatchKind::FieldShape
                            } else {
                                BindingMismatchKind::TypeShape
                            },
                            expected: BindingShape::Builtin(schema),
                            actual: BindingShape::Builtin(*rust),
                        },
                        context.session(),
                        out.reborrow(),
                    )?;
                    return Ok(());
                }
            }
            (TypeShape::Option(provider), TypeKind::Option { inner }) => {
                if matches!(
                    context.session().schema_codec(
                        CodecStage::Binding,
                        |schema_out| inner.kind(schema_out),
                        out.reborrow(),
                    )?,
                    TypeKind::Option { .. }
                ) {
                    let path = pending.path.extend(
                        context.session(),
                        PathSegments::same("", "?"),
                        out.reborrow(),
                    )?;
                    self.errors.record(
                        path,
                        MismatchDetails {
                            kind: BindingMismatchKind::OptionNesting,
                            expected: BindingShape::Option,
                            actual: BindingShape::Option,
                        },
                        context.session(),
                        out.reborrow(),
                    )?;
                    return Ok(());
                }
                let path = pending.path.extend(
                    context.session(),
                    PathSegments::same("", "?"),
                    out.reborrow(),
                )?;
                MatcherSupport::enqueue_child(
                    &mut children,
                    Pending {
                        provider: *provider,
                        ty: inner,
                        depth,
                        path,
                        field_context: pending.field_context,
                    },
                    context,
                    out.reborrow(),
                )?;
            }
            (
                TypeShape::Sequence(provider),
                TypeKind::Array {
                    element,
                    length: None,
                },
            ) => {
                let path = pending.path.extend(
                    context.session(),
                    PathSegments::same("", "[]"),
                    out.reborrow(),
                )?;
                MatcherSupport::enqueue_child(
                    &mut children,
                    Pending {
                        provider: *provider,
                        ty: element,
                        depth,
                        path,
                        field_context: pending.field_context,
                    },
                    context,
                    out.reborrow(),
                )?;
            }
            (
                TypeShape::Array {
                    element: provider,
                    length: rust,
                },
                TypeKind::Array {
                    element,
                    length: Some(schema),
                },
            ) => {
                if *rust != schema {
                    self.errors.record(
                        pending.path,
                        MismatchDetails {
                            kind: BindingMismatchKind::ArrayLength,
                            expected: BindingShape::Array {
                                length: Some(schema),
                            },
                            actual: BindingShape::Array {
                                length: Some(*rust),
                            },
                        },
                        context.session(),
                        out.reborrow(),
                    )?;
                    return Ok(());
                }
                let path = pending.path.extend(
                    context.session(),
                    PathSegments::same("", "[]"),
                    out.reborrow(),
                )?;
                MatcherSupport::enqueue_child(
                    &mut children,
                    Pending {
                        provider: *provider,
                        ty: element,
                        depth,
                        path,
                        field_context: pending.field_context,
                    },
                    context,
                    out.reborrow(),
                )?;
            }
            (TypeShape::Map { value, .. }, TypeKind::Map { value: ty }) => {
                let path = pending.path.extend(
                    context.session(),
                    PathSegments::same(".", "value"),
                    out.reborrow(),
                )?;
                MatcherSupport::enqueue_child(
                    &mut children,
                    Pending {
                        provider: *value,
                        ty,
                        depth,
                        path,
                        field_context: pending.field_context,
                    },
                    context,
                    out.reborrow(),
                )?;
            }
            (TypeShape::Structure(members), TypeKind::Struct { fields }) => {
                let type_path = check_name(
                    NameCheck {
                        name: name.as_ref().map(|name| name.text()),
                        ty: pending.ty,
                        path: &pending.path,
                        root: pending.depth == 0,
                    },
                    context,
                    &mut self.errors,
                    out.reborrow(),
                )?;
                if self.errors.is_truncated() {
                    return Ok(());
                }
                binding.fields = FieldMatcher::match_fields(
                    FieldMatch {
                        members,
                        fields,
                        context,
                        children: &mut children,
                        depth,
                        path: &type_path,
                        errors: &mut self.errors,
                    },
                    out.reborrow(),
                )?;
            }
            (TypeShape::Enumeration(members), TypeKind::Enum { variants }) => {
                let type_path = check_name(
                    NameCheck {
                        name: name.as_ref().map(|name| name.text()),
                        ty: pending.ty,
                        path: &pending.path,
                        root: pending.depth == 0,
                    },
                    context,
                    &mut self.errors,
                    out.reborrow(),
                )?;
                if self.errors.is_truncated() {
                    return Ok(());
                }
                if members.is_empty() {
                    self.errors.record(
                        type_path,
                        MismatchDetails {
                            kind: BindingMismatchKind::TypeShape,
                            expected: BindingShape::Enumeration,
                            actual: BindingShape::Enumeration,
                        },
                        context.session(),
                        out.reborrow(),
                    )?;
                    return Ok(());
                }
                context.session().budget_codec(
                    CodecStage::Binding,
                    |budget_out| {
                        binding
                            .variants
                            .try_reserve_exact(variants.len(), budget_out)
                    },
                    out.reborrow(),
                )?;
                let mut sorted = context.session().budget_codec(
                    CodecStage::Binding,
                    |budget_out| context.session().workspace(budget_out),
                    out.reborrow(),
                )?;
                context.session().budget_codec(
                    CodecStage::Binding,
                    |budget_out| sorted.try_reserve_exact(members.len(), budget_out),
                    out.reborrow(),
                )?;
                for (slot, descriptor) in members.iter().enumerate() {
                    let DescriptorNode::Variant { name, shape } =
                        context.get(*descriptor, out.reborrow())?
                    else {
                        return Err(out.fail(internal_error()));
                    };
                    context.session().budget_codec(
                        CodecStage::Binding,
                        |budget_out| sorted.try_push((name.text(), (slot, shape)), budget_out),
                        out.reborrow(),
                    )?;
                }
                sorted.sort_by(
                    |left, right, mut sort_out| {
                        let order = context.session().budget_codec(
                            CodecStage::Binding,
                            |budget_out| {
                                context.session().compare_text(left.0, right.0, budget_out)
                            },
                            sort_out.reborrow(),
                        )?;
                        Ok(order)
                    },
                    out.reborrow(),
                )?;
                FieldMatcher::reject_duplicates(
                    DuplicateCheck {
                        sorted: &sorted,
                        path: &type_path,
                        variants: true,
                        context,
                        errors: &mut self.errors,
                    },
                    out.reborrow(),
                )?;
                if self.errors.is_truncated() {
                    return Ok(());
                }
                let mut seen = context.session().budget_codec(
                    CodecStage::Binding,
                    |budget_out| context.session().workspace(budget_out),
                    out.reborrow(),
                )?;
                context.session().budget_codec(
                    CodecStage::Binding,
                    |budget_out| seen.try_reserve_exact(members.len(), budget_out),
                    out.reborrow(),
                )?;
                context.session().budget_codec(
                    CodecStage::Binding,
                    |budget_out| seen.resize(members.len(), false, budget_out),
                    out.reborrow(),
                )?;
                for variant in variants {
                    if self.errors.is_truncated() {
                        break;
                    }
                    let variant_name = context.session().schema_codec(
                        CodecStage::Binding,
                        |schema_out| variant.name(schema_out),
                        out.reborrow(),
                    )?;
                    let found =
                        FieldMatcher::search(&sorted, variant_name, context, out.reborrow())?;
                    let Some(found) = found else {
                        let path = type_path.extend(
                            context.session(),
                            PathSegments::different(".", variant_name, ""),
                            out.reborrow(),
                        )?;
                        self.errors.record(
                            path,
                            MismatchDetails {
                                kind: BindingMismatchKind::MissingVariant,
                                expected: BindingShape::NamedType,
                                actual: BindingShape::Missing,
                            },
                            context.session(),
                            out.reborrow(),
                        )?;
                        continue;
                    };
                    let (rust_slot, shape) = sorted
                        .get(found)
                        .map(|entry| entry.1)
                        .ok_or_else(internal_error)
                        .store_err_into(out.reborrow())?;
                    let seen_slot = seen
                        .get_mut(rust_slot)
                        .ok_or_else(internal_error)
                        .store_err_into(out.reborrow())?;
                    *seen_slot = true;
                    let variant_path = type_path.extend(
                        context.session(),
                        PathSegments::same(".", variant_name),
                        out.reborrow(),
                    )?;
                    let payload = context.session().schema_codec(
                        CodecStage::Binding,
                        |schema_out| variant.payload(schema_out),
                        out.reborrow(),
                    )?;
                    let fields = match (shape, payload) {
                        (VariantShape::Unit, VariantPayloadInfo::Unit) => FieldBindings::empty(),
                        (VariantShape::Single(provider), VariantPayloadInfo::Single { ty }) => {
                            let path = variant_path.extend(
                                context.session(),
                                PathSegments::same("", "()"),
                                out.reborrow(),
                            )?;
                            MatcherSupport::enqueue_child(
                                &mut children,
                                Pending {
                                    provider: *provider,
                                    ty,
                                    depth,
                                    path,
                                    field_context: pending.field_context,
                                },
                                context,
                                out.reborrow(),
                            )?;
                            FieldBindings::empty()
                        }
                        (
                            VariantShape::Structure(members),
                            VariantPayloadInfo::Structure { fields },
                        ) => FieldMatcher::match_fields(
                            FieldMatch {
                                members,
                                fields,
                                context,
                                children: &mut children,
                                depth,
                                path: &variant_path,
                                errors: &mut self.errors,
                            },
                            out.reborrow(),
                        )?,
                        (rust_shape, schema_payload) => {
                            self.errors.record(
                                variant_path,
                                MismatchDetails {
                                    kind: BindingMismatchKind::VariantPayload,
                                    expected: MatcherSupport::schema_payload_shape(schema_payload),
                                    actual: MatcherSupport::rust_payload_shape(rust_shape),
                                },
                                context.session(),
                                out.reborrow(),
                            )?;
                            continue;
                        }
                    };
                    context.session().budget_codec(
                        CodecStage::Binding,
                        |budget_out| {
                            binding.variants.try_push(
                                VariantBinding {
                                    rust_slot,
                                    variant,
                                    fields,
                                },
                                budget_out,
                            )
                        },
                        out.reborrow(),
                    )?;
                }
                for (rust_name, (rust_slot, _)) in &sorted {
                    if self.errors.is_truncated() {
                        break;
                    }
                    let matched = seen
                        .get(*rust_slot)
                        .copied()
                        .ok_or_else(internal_error)
                        .store_err_into(out.reborrow())?;
                    if !matched {
                        let path = type_path.extend(
                            context.session(),
                            PathSegments::different(".", "", rust_name),
                            out.reborrow(),
                        )?;
                        self.errors.record(
                            path,
                            MismatchDetails {
                                kind: BindingMismatchKind::ExtraVariant,
                                expected: BindingShape::Missing,
                                actual: BindingShape::Extra,
                            },
                            context.session(),
                            out.reborrow(),
                        )?;
                    }
                }
            }
            (rust_shape, schema_kind) => {
                let expected = MatcherSupport::schema_shape(schema_kind);
                let actual = MatcherSupport::rust_shape_shape(rust_shape);
                let mismatch_kind = if matches!(expected, BindingShape::Map { .. })
                    || matches!(actual, BindingShape::Map { .. })
                {
                    BindingMismatchKind::MapContainer
                } else if pending.field_context {
                    BindingMismatchKind::FieldShape
                } else {
                    BindingMismatchKind::TypeShape
                };
                self.errors.record(
                    pending.path,
                    MismatchDetails {
                        kind: mismatch_kind,
                        expected,
                        actual,
                    },
                    context.session(),
                    out.reborrow(),
                )?;
                return Ok(());
            }
        }
        context.session().budget_codec(
            CodecStage::Binding,
            |budget_out| {
                self.active.try_push(
                    ActiveBinding {
                        identity,
                        ty: pending.ty,
                    },
                    budget_out,
                )
            },
            out.reborrow(),
        )?;
        self.push(Task::Exit(binding), context, out.reborrow())?;
        for child in children.into_iter().rev() {
            self.push(Task::Enter(child), context, out.reborrow())?;
        }
        Ok(())
    }
}

fn check_name<'slot>(
    request: NameCheck<'_, '_, '_>,
    context: &DescribeContext<'_>,
    errors: &mut BindingErrorCollector,
    mut out: Out<'slot, '_, CodecErrors>,
) -> OResult<'slot, BindingPath> {
    let NameCheck {
        name,
        ty,
        path,
        root,
    } = request;
    let nominal_name = context.session().schema_codec(
        CodecStage::Binding,
        |schema_out| ty.nominal_name(schema_out),
        out.reborrow(),
    )?;
    if let (Some(rust), Some(schema)) = (name, nominal_name) {
        let is_equal = context
            .session()
            .budget_codec(
                CodecStage::Binding,
                |budget_out| context.session().compare_text(rust, schema, budget_out),
                out.reborrow(),
            )?
            .is_eq();
        let type_path = path.extend(
            context.session(),
            PathSegments::different(".", schema, rust),
            out.reborrow(),
        )?;
        if is_equal {
            return Ok(type_path);
        }
        let mismatch_path = type_path.duplicate(context.session(), out.reborrow())?;
        errors.record(
            mismatch_path,
            MismatchDetails {
                kind: if root {
                    BindingMismatchKind::RootTypeName
                } else {
                    BindingMismatchKind::TypeName
                },
                expected: BindingShape::NamedType,
                actual: BindingShape::NamedType,
            },
            context.session(),
            out.reborrow(),
        )?;
        return Ok(type_path);
    }
    let schema = nominal_name.unwrap_or("<anonymous>");
    let rust = name.unwrap_or("<anonymous>");
    let type_path = path.extend(
        context.session(),
        PathSegments::different(".", schema, rust),
        out.reborrow(),
    )?;
    let mismatch_path = type_path.duplicate(context.session(), out.reborrow())?;
    errors.record(
        mismatch_path,
        MismatchDetails {
            kind: if root {
                BindingMismatchKind::RootTypeName
            } else {
                BindingMismatchKind::TypeName
            },
            expected: BindingShape::NamedType,
            actual: BindingShape::NamedType,
        },
        context.session(),
        out.reborrow(),
    )?;
    Ok(type_path)
}

fn internal_error() -> CodecErrors {
    CodecErrors::emergency(CodecStage::Binding, Failure::internal(BINDING_INVARIANT))
}
