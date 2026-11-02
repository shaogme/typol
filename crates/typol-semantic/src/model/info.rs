use crate::{
    constraints::ConstraintList,
    expressions::AssertionList,
    model::{EnvironmentAuthorization, FieldId, TypeId, VariantId},
};
use typol_core::{ConfigValue, SourceSpan};
use typol_model::BuiltinType;

#[derive(Clone, Copy)]
pub enum StoredPayloadInfo<'a> {
    Unit,
    Single { ty: TypeId },
    Structure { fields: &'a [FieldId] },
}

#[derive(Clone, Copy)]
pub enum StoredTypeKind<'a> {
    Builtin(BuiltinType),
    Option {
        inner: TypeId,
    },
    Array {
        element: TypeId,
        length: Option<u32>,
    },
    Map {
        value: TypeId,
    },
    Struct {
        fields: &'a [FieldId],
    },
    Enum {
        variants: &'a [VariantId],
    },
}

pub(crate) struct FieldInfoParts<'a> {
    pub(crate) slot: usize,
    pub(crate) name: &'a str,
    pub(crate) ty: TypeId,
    pub(crate) span: SourceSpan,
    pub(crate) default: Option<&'a ConfigValue>,
    pub(crate) constraints: ConstraintList<'a>,
    pub(crate) secret: bool,
    pub(crate) environment: Option<EnvironmentAuthorization<'a>>,
}

pub(crate) struct VariantInfoParts<'a> {
    pub(crate) structure_type: Option<TypeId>,
    pub(crate) name: &'a str,
    pub(crate) payload: StoredPayloadInfo<'a>,
    pub(crate) span: SourceSpan,
    pub(crate) constraints: ConstraintList<'a>,
    pub(crate) secret: bool,
    pub(crate) assertions: AssertionList<'a>,
}

pub(crate) struct FieldInfo<'a> {
    slot: usize,
    name: &'a str,
    ty: TypeId,
    span: SourceSpan,
    default: Option<&'a ConfigValue>,
    constraints: ConstraintList<'a>,
    secret: bool,
    environment: Option<EnvironmentAuthorization<'a>>,
}

pub(crate) struct VariantInfo<'a> {
    structure_type: Option<TypeId>,
    name: &'a str,
    payload: StoredPayloadInfo<'a>,
    span: SourceSpan,
    constraints: ConstraintList<'a>,
    secret: bool,
    assertions: AssertionList<'a>,
}

impl<'a> FieldInfo<'a> {
    pub(crate) fn new(parts: FieldInfoParts<'a>) -> Self {
        Self {
            slot: parts.slot,
            name: parts.name,
            ty: parts.ty,
            span: parts.span,
            default: parts.default,
            constraints: parts.constraints,
            secret: parts.secret,
            environment: parts.environment,
        }
    }

    pub(crate) fn slot(&self) -> usize {
        self.slot
    }

    pub fn name(&self) -> &'a str {
        self.name
    }

    pub fn ty(&self) -> TypeId {
        self.ty
    }

    pub fn span(&self) -> SourceSpan {
        self.span
    }

    pub fn default_value(&self) -> Option<&'a ConfigValue> {
        self.default
    }

    pub fn constraints(&self) -> ConstraintList<'a> {
        self.constraints
    }

    pub fn is_secret(&self) -> bool {
        self.secret
    }

    pub fn environment(&self) -> Option<EnvironmentAuthorization<'a>> {
        self.environment
    }
}

impl<'a> VariantInfo<'a> {
    pub(crate) fn new(parts: VariantInfoParts<'a>) -> Self {
        Self {
            structure_type: parts.structure_type,
            name: parts.name,
            payload: parts.payload,
            span: parts.span,
            constraints: parts.constraints,
            secret: parts.secret,
            assertions: parts.assertions,
        }
    }

    pub fn structure_type(&self) -> Option<TypeId> {
        self.structure_type
    }

    pub fn name(&self) -> &'a str {
        self.name
    }

    pub fn payload(&self) -> StoredPayloadInfo<'a> {
        self.payload
    }

    pub fn span(&self) -> SourceSpan {
        self.span
    }

    pub fn constraints(&self) -> ConstraintList<'a> {
        self.constraints
    }

    pub fn is_secret(&self) -> bool {
        self.secret
    }

    pub fn assertions(&self) -> AssertionList<'a> {
        self.assertions
    }
}
