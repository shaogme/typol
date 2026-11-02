use opeo::{OResult, Out};
use typol_core::{
    DiagnosticContext, ValueEntries, ValueKind, ValuePayloadView, ValueView, diagnostic_context,
};
use typol_model::BuiltinType;
use typol_semantic::{FieldList, TypeKind, TypeView, VariantPayloadInfo};

pub(crate) static NAMED_ENTRY_NAME_MISSING: DiagnosticContext =
    diagnostic_context!("typol", "value", "named-entry-name-missing");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ValueDepth(usize);

pub(crate) struct ValueNode;

pub(crate) enum FieldIndexError {
    FieldCountMismatch,
    MissingName,
    UnknownField,
    DuplicateField,
    InvalidSlot,
    MissingField,
}

pub(crate) struct EnumValue<'value> {
    variant: &'value str,
    payload: ValuePayloadView<'value>,
}

pub(crate) enum ValueNodeKind<'schema, 'value> {
    Signed {
        builtin: BuiltinType,
        value: i128,
    },
    Unsigned {
        builtin: BuiltinType,
        value: u128,
    },
    Float32(f32),
    Float64(f64),
    Bool(bool),
    String(&'value str),
    Optional {
        inner: TypeView<'schema>,
        value: Option<ValueView<'value>>,
    },
    Array {
        element: TypeView<'schema>,
        #[cfg(feature = "serde")]
        expected_length: Option<u32>,
        values: ValueEntries<'value>,
    },
    Map {
        value_type: TypeView<'schema>,
        values: ValueEntries<'value>,
    },
    Structure {
        fields: FieldList<'schema>,
        values: ValueEntries<'value>,
    },
    Enumeration(EnumValue<'value>),
}

pub(crate) enum EnumPayloadValue<'schema, 'value> {
    Unit,
    Single {
        ty: TypeView<'schema>,
        value: ValueView<'value>,
    },
    Structure {
        fields: FieldList<'schema>,
        values: ValueEntries<'value>,
    },
}

impl ValueNodeKind<'_, '_> {
    #[cfg(feature = "serde")]
    pub(crate) const fn builtin(&self) -> Option<BuiltinType> {
        match self {
            Self::Signed { builtin, .. } | Self::Unsigned { builtin, .. } => Some(*builtin),
            Self::Float32(_) => Some(BuiltinType::F32),
            Self::Float64(_) => Some(BuiltinType::F64),
            Self::Bool(_) => Some(BuiltinType::Bool),
            Self::String(_) => Some(BuiltinType::String),
            Self::Optional { .. }
            | Self::Array { .. }
            | Self::Map { .. }
            | Self::Structure { .. }
            | Self::Enumeration(_) => None,
        }
    }
}

impl ValueDepth {
    pub(crate) const fn root() -> Self {
        Self(0)
    }

    #[cfg(feature = "config-ir")]
    pub(crate) const fn from_level(level: usize) -> Self {
        Self(level)
    }

    pub(crate) const fn level(self) -> usize {
        self.0
    }

    pub(crate) fn child(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }
}

impl ValueNode {
    pub(crate) fn resolve<'schema, 'value>(
        ty: TypeKind<'schema>,
        value: ValueView<'value>,
    ) -> Option<ValueNodeKind<'schema, 'value>> {
        match (ty, value.as_ref()) {
            (
                TypeKind::Builtin(
                    builtin @ (BuiltinType::I8
                    | BuiltinType::I16
                    | BuiltinType::I32
                    | BuiltinType::I64
                    | BuiltinType::I128),
                ),
                ValueKind::Signed(value),
            ) => Some(ValueNodeKind::Signed { builtin, value }),
            (
                TypeKind::Builtin(
                    builtin @ (BuiltinType::U8
                    | BuiltinType::U16
                    | BuiltinType::U32
                    | BuiltinType::U64
                    | BuiltinType::U128),
                ),
                ValueKind::Unsigned(value),
            ) => Some(ValueNodeKind::Unsigned { builtin, value }),
            (TypeKind::Builtin(BuiltinType::F32), ValueKind::Float32(value)) => {
                Some(ValueNodeKind::Float32(value))
            }
            (TypeKind::Builtin(BuiltinType::F64), ValueKind::Float64(value)) => {
                Some(ValueNodeKind::Float64(value))
            }
            (TypeKind::Builtin(BuiltinType::Bool), ValueKind::Bool(value)) => {
                Some(ValueNodeKind::Bool(value))
            }
            (TypeKind::Builtin(BuiltinType::String), ValueKind::String(value)) => {
                Some(ValueNodeKind::String(value))
            }
            (TypeKind::Option { inner }, ValueKind::Optional(value)) => {
                Some(ValueNodeKind::Optional { inner, value })
            }
            (TypeKind::Array { element, length }, ValueKind::Array(values))
                if length
                    .is_none_or(|length| usize::try_from(length).ok() == Some(values.len())) =>
            {
                Some(ValueNodeKind::Array {
                    element,
                    #[cfg(feature = "serde")]
                    expected_length: length,
                    values,
                })
            }
            (TypeKind::Map { value }, ValueKind::Map(values)) => Some(ValueNodeKind::Map {
                value_type: value,
                values,
            }),
            (TypeKind::Struct { fields }, ValueKind::Structure(values))
                if fields.len() == values.len() =>
            {
                Some(ValueNodeKind::Structure { fields, values })
            }
            (TypeKind::Enum { .. }, ValueKind::Enumeration { variant, payload }) => {
                Some(ValueNodeKind::Enumeration(EnumValue { variant, payload }))
            }
            _ => None,
        }
    }

    pub(crate) fn index_fields<'slot, 'value, E>(
        values: ValueEntries<'value>,
        slots: &mut [usize],
        mut find_field: impl for<'borrow> FnMut(
            &str,
            Out<'slot, 'borrow, E>,
        ) -> OResult<'slot, Option<usize>>,
        mut fail: impl for<'borrow> FnMut(FieldIndexError, Out<'slot, 'borrow, E>) -> OResult<'slot, ()>,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, ()> {
        if slots.len() != values.len() {
            return fail(FieldIndexError::FieldCountMismatch, out.reborrow());
        }
        slots.fill(usize::MAX);
        for index in 0..values.len() {
            let Some(value) = values.get(index) else {
                return fail(FieldIndexError::InvalidSlot, out.reborrow());
            };
            let Some(value) = value.as_named() else {
                return fail(FieldIndexError::MissingName, out.reborrow());
            };
            let Some(slot) = find_field(value.name(), out.reborrow())? else {
                return fail(FieldIndexError::UnknownField, out.reborrow());
            };
            let Some(assigned) = slots.get_mut(slot) else {
                return fail(FieldIndexError::InvalidSlot, out.reborrow());
            };
            if *assigned != usize::MAX {
                return fail(FieldIndexError::DuplicateField, out.reborrow());
            }
            *assigned = index;
        }
        if slots.contains(&usize::MAX) {
            return fail(FieldIndexError::MissingField, out.reborrow());
        }
        Ok(())
    }
}

impl EnumValue<'_> {
    pub(crate) const fn variant(&self) -> &str {
        self.variant
    }
}

impl<'value> EnumValue<'value> {
    pub(crate) const fn payload(&self) -> ValuePayloadView<'value> {
        self.payload
    }

    pub(crate) fn resolve_payload<'schema>(
        &self,
        expected: VariantPayloadInfo<'schema>,
    ) -> Option<EnumPayloadValue<'schema, 'value>> {
        match (expected, self.payload) {
            (VariantPayloadInfo::Unit, ValuePayloadView::Unit) => Some(EnumPayloadValue::Unit),
            (VariantPayloadInfo::Single { ty }, ValuePayloadView::Single(value)) => {
                Some(EnumPayloadValue::Single { ty, value })
            }
            (VariantPayloadInfo::Structure { fields }, ValuePayloadView::Structure(values))
                if fields.len() == values.len() =>
            {
                Some(EnumPayloadValue::Structure { fields, values })
            }
            _ => None,
        }
    }
}
