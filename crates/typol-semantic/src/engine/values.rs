use crate::{EvalError, TypeId};
use opeo::{OResult, Out};
use typol_core::{ConfigValue, DiagnosticContext, ValueKind, ValueView, diagnostic_context};

static EVAL_VALUE_INTERNAL: DiagnosticContext = diagnostic_context!(
    "typol-semantic",
    "evaluation-value",
    "value-kind-access-mismatch",
);

#[derive(Clone, Copy)]
pub enum EvalKind<'a> {
    Boolean(bool),
    String(&'a str),
    Signed(i128),
    Unsigned(u128),
    Magnitude(u128),
    Float(f64),
    None,
    Value(ValueView<'a>),
    Structure(ValueView<'a>),
}

#[derive(Clone, Copy)]
pub struct EvalValue<'a> {
    kind: EvalKind<'a>,
    ty: TypeId,
    host: bool,
}

impl<'a> EvalValue<'a> {
    pub const fn new(kind: EvalKind<'a>, ty: TypeId, host: bool) -> Self {
        Self { kind, ty, host }
    }

    pub const fn kind(self) -> EvalKind<'a> {
        self.kind
    }

    pub const fn ty(self) -> TypeId {
        self.ty
    }

    pub const fn host(self) -> bool {
        self.host
    }

    pub fn config(value: ValueView<'a>, ty: TypeId, host: bool) -> Self {
        let kind = match value.as_ref() {
            ValueKind::Signed(value) => EvalKind::Signed(value),
            ValueKind::Unsigned(value) => EvalKind::Unsigned(value),
            ValueKind::Float32(value) => EvalKind::Float(f64::from(value)),
            ValueKind::Float64(value) => EvalKind::Float(value),
            ValueKind::Bool(value) => EvalKind::Boolean(value),
            ValueKind::String(value) => EvalKind::String(value),
            ValueKind::Structure(_) => EvalKind::Structure(value),
            _ => EvalKind::Value(value),
        };
        Self { kind, ty, host }
    }

    pub fn from_config(value: &'a ConfigValue, ty: TypeId, host: bool) -> Self {
        Self::config(value.root(), ty, host)
    }

    pub fn boolean<'slot, 'borrow>(
        self,
        out: Out<'slot, 'borrow, EvalError>,
    ) -> OResult<'slot, bool> {
        match self.kind {
            EvalKind::Boolean(value) => Ok(value),
            _ => Err(out.fail(EvalError::internal(EVAL_VALUE_INTERNAL))),
        }
    }

    pub fn string<'slot, 'borrow>(
        self,
        out: Out<'slot, 'borrow, EvalError>,
    ) -> OResult<'slot, &'a str> {
        match self.kind {
            EvalKind::String(value) => Ok(value),
            _ => Err(out.fail(EvalError::internal(EVAL_VALUE_INTERNAL))),
        }
    }

    pub fn fields<'slot, 'borrow>(
        self,
        out: Out<'slot, 'borrow, EvalError>,
    ) -> OResult<'slot, ValueView<'a>> {
        match self.kind {
            EvalKind::Structure(value) => Ok(value),
            _ => Err(out.fail(EvalError::internal(EVAL_VALUE_INTERNAL))),
        }
    }

    pub fn value<'slot, 'borrow>(
        self,
        out: Out<'slot, 'borrow, EvalError>,
    ) -> OResult<'slot, ValueView<'a>> {
        match self.kind {
            EvalKind::Value(value) | EvalKind::Structure(value) => Ok(value),
            _ => Err(out.fail(EvalError::internal(EVAL_VALUE_INTERNAL))),
        }
    }
}

#[derive(Clone, Copy, Default)]
pub struct Bindings<'a> {
    pub self_value: Option<EvalValue<'a>>,
    pub root: Option<&'a ConfigValue>,
    pub item: Option<EvalValue<'a>>,
    pub key: Option<&'a str>,
    pub value: Option<EvalValue<'a>>,
}
