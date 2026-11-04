use crate::{
    binding_impl::{errors::BindingShape, matcher::Pending},
    descriptor::{DescribeContext, TypeShape, VariantShape},
    diagnostics::{CodecErrors, CodecStage},
};
use opeo::{OResult, Out};
use typol_core::{BudgetError, ChargedVec};
use typol_semantic::{TypeKind, VariantPayloadInfo};

pub(super) struct MatcherSupport;

impl MatcherSupport {
    pub(super) fn binding_budget_error(error: BudgetError) -> CodecErrors {
        CodecErrors::emergency(CodecStage::Binding, error.into())
    }

    pub(super) fn enqueue_child<'slot, 'schema>(
        children: &mut ChargedVec<Pending<'schema>>,
        pending: Pending<'schema>,
        context: &DescribeContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ()> {
        context.session().budget_codec(
            CodecStage::Binding,
            |budget_out| children.try_reserve(1, budget_out),
            out.reborrow(),
        )?;
        context.session().budget_codec(
            CodecStage::Binding,
            |budget_out| children.try_push(pending, budget_out),
            out.reborrow(),
        )
    }

    pub(super) fn schema_shape(kind: TypeKind<'_>) -> BindingShape {
        match kind {
            TypeKind::Builtin(kind) => BindingShape::Builtin(kind),
            TypeKind::Option { .. } => BindingShape::Option,
            TypeKind::Array { length: None, .. } => BindingShape::Sequence,
            TypeKind::Array { length, .. } => BindingShape::Array { length },
            TypeKind::Map { .. } => BindingShape::Map { container: None },
            TypeKind::Struct { .. } => BindingShape::Structure,
            TypeKind::Enum { .. } => BindingShape::Enumeration,
        }
    }

    pub(super) fn rust_shape_shape(shape: &TypeShape) -> BindingShape {
        match shape {
            TypeShape::Transparent(_) => BindingShape::Transparent,
            TypeShape::Scalar(kind) => BindingShape::Builtin(*kind),
            TypeShape::Option(_) => BindingShape::Option,
            TypeShape::Sequence(_) => BindingShape::Sequence,
            TypeShape::Array { length, .. } => BindingShape::Array {
                length: Some(*length),
            },
            TypeShape::Map { container, .. } => BindingShape::Map {
                container: Some(*container),
            },
            TypeShape::Structure(_) => BindingShape::Structure,
            TypeShape::Enumeration(_) => BindingShape::Enumeration,
        }
    }

    pub(super) fn schema_payload_shape(payload: VariantPayloadInfo<'_>) -> BindingShape {
        match payload {
            VariantPayloadInfo::Unit => BindingShape::UnitPayload,
            VariantPayloadInfo::Single { .. } => BindingShape::SinglePayload,
            VariantPayloadInfo::Structure { .. } => BindingShape::StructurePayload,
        }
    }

    pub(super) fn rust_payload_shape(shape: &VariantShape) -> BindingShape {
        match shape {
            VariantShape::Unit => BindingShape::UnitPayload,
            VariantShape::Single(_) => BindingShape::SinglePayload,
            VariantShape::Structure(_) => BindingShape::StructurePayload,
        }
    }
}
