use crate::{
    binding_impl::{
        FieldBinding, FieldBindings,
        errors::{
            BindingErrorCollector, BindingMismatchKind, BindingPath, BindingShape, MismatchDetails,
            PathSegments,
        },
        matcher::Pending,
    },
    descriptor::{DescribeContext, DescriptorNode, TypeDescriptor},
    diagnostics::{CodecErrors, CodecStage, Failure},
};
use core::cmp::Ordering;
use opeo::{OResult, Out, ResultOutExt};
use typol_core::{ChargedVec, DiagnosticContext, diagnostic_context};
use typol_semantic::FieldList;

static FIELD_BINDING_INVARIANT: DiagnosticContext =
    diagnostic_context!("typol", "binding", "field-binding-invariant");

pub(super) struct FieldMatch<'members, 'context, 'session, 'children, 'schema, 'path> {
    pub(super) members: &'members [TypeDescriptor],
    pub(super) fields: FieldList<'schema>,
    pub(super) context: &'context DescribeContext<'session>,
    pub(super) children: &'children mut ChargedVec<Pending<'schema>>,
    pub(super) depth: usize,
    pub(super) path: &'path BindingPath,
    pub(super) errors: &'children mut BindingErrorCollector,
}

pub(super) struct DuplicateCheck<'items, 'session, V> {
    pub(super) sorted: &'items [(&'items str, V)],
    pub(super) path: &'items BindingPath,
    pub(super) variants: bool,
    pub(super) context: &'items DescribeContext<'session>,
    pub(super) errors: &'items mut BindingErrorCollector,
}

pub(super) struct FieldMatcher;

impl FieldMatcher {
    pub(super) fn match_fields<'slot, 'members, 'context, 'session, 'children, 'schema, 'path>(
        request: FieldMatch<'members, 'context, 'session, 'children, 'schema, 'path>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, FieldBindings<'schema>> {
        let FieldMatch {
            members,
            fields,
            context,
            children,
            depth,
            path,
            errors,
        } = request;
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
            let DescriptorNode::Field { name, value } = context.get(*descriptor, out.reborrow())?
            else {
                return Err(out.fail(internal_error()));
            };
            context.session().budget_codec(
                CodecStage::Binding,
                |budget_out| sorted.try_push((name.text(), (slot, *value)), budget_out),
                out.reborrow(),
            )?;
        }
        sorted.sort_by(
            |left, right, mut sort_out| {
                let order = context.session().budget_codec(
                    CodecStage::Binding,
                    |budget_out| context.session().compare_text(left.0, right.0, budget_out),
                    sort_out.reborrow(),
                )?;
                Ok(order)
            },
            out.reborrow(),
        )?;
        Self::reject_duplicates(
            DuplicateCheck {
                sorted: &sorted,
                path,
                variants: false,
                context,
                errors,
            },
            out.reborrow(),
        )?;
        if errors.is_truncated() {
            return Ok(FieldBindings::empty());
        }
        let mut result = context.session().budget_codec(
            CodecStage::Binding,
            |budget_out| context.session().workspace(budget_out),
            out.reborrow(),
        )?;
        context.session().budget_codec(
            CodecStage::Binding,
            |budget_out| result.try_reserve_exact(fields.len(), budget_out),
            out.reborrow(),
        )?;
        context.session().budget_codec(
            CodecStage::Binding,
            |budget_out| children.try_reserve(fields.len(), budget_out),
            out.reborrow(),
        )?;
        let mut matched = context.session().budget_codec(
            CodecStage::Binding,
            |budget_out| context.session().workspace(budget_out),
            out.reborrow(),
        )?;
        context.session().budget_codec(
            CodecStage::Binding,
            |budget_out| matched.try_reserve_exact(members.len(), budget_out),
            out.reborrow(),
        )?;
        context.session().budget_codec(
            CodecStage::Binding,
            |budget_out| matched.resize(members.len(), false, budget_out),
            out.reborrow(),
        )?;
        for field in fields {
            if errors.is_truncated() {
                break;
            }
            let field_name = context.session().schema_codec(
                CodecStage::Binding,
                |schema_out| field.name(schema_out),
                out.reborrow(),
            )?;
            let Some(index) = Self::search(&sorted, field_name, context, out.reborrow())? else {
                let mismatch_path = path.extend(
                    context.session(),
                    PathSegments::different(".", field_name, ""),
                    out.reborrow(),
                )?;
                errors.record(
                    mismatch_path,
                    MismatchDetails {
                        kind: BindingMismatchKind::MissingField,
                        expected: BindingShape::NamedType,
                        actual: BindingShape::Missing,
                    },
                    context.session(),
                    out.reborrow(),
                )?;
                continue;
            };
            let (rust_slot, provider) = sorted
                .get(index)
                .map(|entry| entry.1)
                .ok_or_else(internal_error)
                .store_err_into(out.reborrow())?;
            let matched_slot = matched
                .get_mut(rust_slot)
                .ok_or_else(internal_error)
                .store_err_into(out.reborrow())?;
            *matched_slot = true;
            context.session().budget_codec(
                CodecStage::Binding,
                |budget_out| result.try_push(FieldBinding { rust_slot, field }, budget_out),
                out.reborrow(),
            )?;
            let ty = context.session().schema_codec(
                CodecStage::Binding,
                |schema_out| field.ty(schema_out),
                out.reborrow(),
            )?;
            let rust_name = sorted
                .get(index)
                .map(|entry| entry.0)
                .ok_or_else(internal_error)
                .store_err_into(out.reborrow())?;
            let child_path = path.extend(
                context.session(),
                PathSegments::different(".", field_name, rust_name),
                out.reborrow(),
            )?;
            context.session().budget_codec(
                CodecStage::Binding,
                |budget_out| {
                    children.try_push(
                        Pending {
                            provider,
                            ty,
                            depth,
                            path: child_path,
                            field_context: true,
                        },
                        budget_out,
                    )
                },
                out.reborrow(),
            )?;
        }
        for (rust_name, (rust_slot, _)) in &sorted {
            if errors.is_truncated() {
                break;
            }
            let was_matched = matched
                .get(*rust_slot)
                .copied()
                .ok_or_else(internal_error)
                .store_err_into(out.reborrow())?;
            if !was_matched {
                let mismatch_path = path.extend(
                    context.session(),
                    PathSegments::different(".", "", rust_name),
                    out.reborrow(),
                )?;
                errors.record(
                    mismatch_path,
                    MismatchDetails {
                        kind: BindingMismatchKind::ExtraField,
                        expected: BindingShape::Missing,
                        actual: BindingShape::Extra,
                    },
                    context.session(),
                    out.reborrow(),
                )?;
            }
        }
        FieldBindings::new(result, members.len(), context.session(), out.reborrow())
    }

    pub(super) fn search<'slot, V>(
        sorted: &[(&str, V)],
        name: &str,
        context: &DescribeContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, Option<usize>> {
        let mut low = 0;
        let mut high = sorted.len();
        while low < high {
            let middle = low + (high - low) / 2;
            let entry = sorted
                .get(middle)
                .ok_or_else(internal_error)
                .store_err_into(out.reborrow())?;
            let ordering = context.session().budget_codec(
                CodecStage::Binding,
                |budget_out| context.session().compare_text(entry.0, name, budget_out),
                out.reborrow(),
            )?;
            match ordering {
                Ordering::Less => low = middle + 1,
                Ordering::Greater => high = middle,
                Ordering::Equal => return Ok(Some(middle)),
            }
        }
        Ok(None)
    }

    pub(super) fn reject_duplicates<'slot, V>(
        request: DuplicateCheck<'_, '_, V>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, bool> {
        let DuplicateCheck {
            sorted,
            path,
            variants,
            context,
            errors,
        } = request;
        let mut found_duplicate = false;
        for pair in sorted.windows(2) {
            let left = pair
                .first()
                .ok_or_else(internal_error)
                .store_err_into(out.reborrow())?;
            let right = pair
                .get(1)
                .ok_or_else(internal_error)
                .store_err_into(out.reborrow())?;
            let ordering = context.session().budget_codec(
                CodecStage::Binding,
                |budget_out| context.session().compare_text(left.0, right.0, budget_out),
                out.reborrow(),
            )?;
            if ordering.is_eq() {
                found_duplicate = true;
                let mismatch_path = path.extend(
                    context.session(),
                    PathSegments::different(".", "", left.0),
                    out.reborrow(),
                )?;
                errors.record(
                    mismatch_path,
                    MismatchDetails {
                        kind: if variants {
                            BindingMismatchKind::DuplicateVariant
                        } else {
                            BindingMismatchKind::DuplicateField
                        },
                        expected: BindingShape::Duplicate,
                        actual: BindingShape::Duplicate,
                    },
                    context.session(),
                    out.reborrow(),
                )?;
            }
        }
        Ok(found_duplicate)
    }
}

fn internal_error() -> CodecErrors {
    CodecErrors::emergency(
        CodecStage::Binding,
        Failure::internal(FIELD_BINDING_INVARIANT),
    )
}
