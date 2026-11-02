use typol_core::{ChargedVec, ExecutionBudget, ValueEntries};

use crate::{
    ExpressionMethodInfo, FieldId, StoredExpressionArgument, StoredPayloadInfo, StoredTypeKind,
    TypeId, VariantId,
    engine::{Bindings, EvalError, EvalValue},
};
use opeo::{OResult, Out};

pub struct ExecutionType<'a>(StoredTypeKind<'a>);

impl<'a> ExecutionType<'a> {
    pub fn new(kind: StoredTypeKind<'a>) -> Self {
        Self(kind)
    }

    pub fn kind(&self) -> StoredTypeKind<'a> {
        self.0
    }

    pub fn fields(&self) -> &'a [FieldId] {
        match self.0 {
            StoredTypeKind::Struct { fields } => fields,
            _ => &[],
        }
    }
}

pub struct ExecutionField<'a> {
    name: &'a str,
    ty: TypeId,
    host: bool,
    slot: usize,
}

impl<'a> ExecutionField<'a> {
    pub fn new(name: &'a str, ty: TypeId, host: bool, slot: usize) -> Self {
        Self {
            name,
            ty,
            host,
            slot,
        }
    }

    pub fn name(&self) -> &str {
        self.name
    }

    pub fn ty(&self) -> TypeId {
        self.ty
    }

    pub fn host(&self) -> bool {
        self.host
    }

    pub fn slot(&self) -> usize {
        self.slot
    }
}

pub struct ExecutionVariant<'a> {
    name: &'a str,
    payload: StoredPayloadInfo<'a>,
    host: bool,
}

impl<'a> ExecutionVariant<'a> {
    pub fn new(name: &'a str, payload: StoredPayloadInfo<'a>, host: bool) -> Self {
        Self {
            name,
            payload,
            host,
        }
    }

    pub fn name(&self) -> &str {
        self.name
    }

    pub fn payload(&self) -> StoredPayloadInfo<'a> {
        self.payload
    }

    pub fn host(&self) -> bool {
        self.host
    }
}

/// Execution borrows only type and member information; valid objects under compilation can provide the same view.
pub trait ExecutionModel {
    fn type_info<'slot, 'borrow>(
        &self,
        id: TypeId,
        out: Out<'slot, 'borrow, EvalError>,
    ) -> OResult<'slot, Option<ExecutionType<'_>>>;

    fn field_info<'slot, 'borrow>(
        &self,
        id: FieldId,
        out: Out<'slot, 'borrow, EvalError>,
    ) -> OResult<'slot, Option<ExecutionField<'_>>>;

    fn variant_info<'slot, 'borrow>(
        &self,
        id: VariantId,
        out: Out<'slot, 'borrow, EvalError>,
    ) -> OResult<'slot, Option<ExecutionVariant<'_>>>;

    fn same_type<'slot, 'borrow>(
        &self,
        left: TypeId,
        right: TypeId,
        out: Out<'slot, 'borrow, EvalError>,
    ) -> OResult<'slot, bool>;

    fn host<'slot, 'borrow>(
        &self,
        id: TypeId,
        out: Out<'slot, 'borrow, EvalError>,
    ) -> OResult<'slot, Option<bool>>;

    fn entry_type<'slot, 'borrow>(
        &self,
        out: Out<'slot, 'borrow, EvalError>,
    ) -> OResult<'slot, Option<TypeId>>;

    fn variant_named<'slot>(
        &self,
        owner: TypeId,
        name: &str,
        budget: &ExecutionBudget,
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, Option<VariantId>> {
        let Some(info) = self.type_info(owner, out.reborrow())? else {
            return Ok(None);
        };
        let StoredTypeKind::Enum { variants } = info.kind() else {
            return Ok(None);
        };
        for id in variants {
            budget.consume(1, out.reborrow())?;
            if let Some(variant) = self.variant_info(*id, out.reborrow())? {
                if budget
                    .compare_text(variant.name(), name, out.reborrow())?
                    .is_eq()
                {
                    return Ok(Some(*id));
                }
            }
        }
        Ok(None)
    }
}

pub(crate) struct EvaluationStack;

impl EvaluationStack {
    pub(crate) fn push<'slot, T>(
        items: &mut ChargedVec<T>,
        value: T,
        out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, ()> {
        items.try_push(value, out)
    }
}

pub(crate) struct Local<'a> {
    pub(crate) parameter: u32,
    pub(crate) value: EvalValue<'a>,
}

pub(crate) struct FinishInputs<'a, 'locals> {
    pub(crate) index: usize,
    pub(crate) bindings: Bindings<'a>,
    pub(crate) locals: &'locals [Local<'a>],
    pub(crate) first: Option<EvalValue<'a>>,
    pub(crate) second: Option<EvalValue<'a>>,
}

pub(crate) struct MethodCall {
    pub(crate) method: ExpressionMethodInfo,
    pub(crate) argument: Option<StoredExpressionArgument>,
    pub(crate) ty: TypeId,
}

pub(crate) enum Task<'a> {
    Enter(usize, usize),
    Child(usize),
    BinaryLeft(usize, usize),
    BinaryRight(usize, EvalValue<'a>),
    MethodReceiver(usize, usize),
    MethodArgument(usize, EvalValue<'a>),
    Quantifier {
        ty: TypeId,
        values: ValueEntries<'a>,
        completed: bool,
        depth: usize,
        parameter: u32,
        body: usize,
        element: TypeId,
        any: bool,
    },
}
