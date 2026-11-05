mod json;
mod resources;

use super::{ConfigIrError, ConfigIrOptions};
use crate::value_walk::{EnumPayloadValue, FieldIndexError, ValueNode, ValueNodeKind};
use alloc::vec::Vec;
use core::mem::size_of;
use opeo::{ErrSlot, OResult, Out};
use serde::{
    Serialize, Serializer,
    ser::{Error as SerError, SerializeSeq, SerializeStruct},
};
use typol_core::{ConfigValue, ValueEntries, ValueEntryView, ValueView, Write};
use typol_model::BuiltinType;
use typol_semantic::{CompiledSchema, FieldList, SchemaAccessError, TypeKind, TypeView};

use self::{
    json::JsonSerializer,
    resources::{BoundedBuffer, BuiltinNames, Decimal, SerializationState, WorkspaceBudget},
};

const IR_FORMAT: &str = "typol-config-ir";
const IR_VERSION: u32 = 1;

pub(super) struct ConfigIrSerializer;

struct TypeDescriptor<'a> {
    ty: TypeView<'a>,
    state: &'a SerializationState,
}

struct NodeValue<'a> {
    ty: TypeView<'a>,
    value: ValueView<'a>,
    secret: bool,
    state: &'a SerializationState,
    workspace: &'a WorkspaceBudget,
}

struct ArrayItems<'a> {
    element: TypeView<'a>,
    items: ValueEntries<'a>,
    state: &'a SerializationState,
    workspace: &'a WorkspaceBudget,
}

struct MapItems<'a> {
    value_type: TypeView<'a>,
    entries: ValueEntries<'a>,
    state: &'a SerializationState,
    workspace: &'a WorkspaceBudget,
}

struct MapEntryNode<'a> {
    key: &'a str,
    value_type: TypeView<'a>,
    value: ValueView<'a>,
    state: &'a SerializationState,
    workspace: &'a WorkspaceBudget,
}

struct StructFields<'a> {
    fields: FieldList<'a>,
    values: ValueEntries<'a>,
    state: &'a SerializationState,
    workspace: &'a WorkspaceBudget,
}

struct FieldNode<'a> {
    name: &'a str,
    ty: TypeView<'a>,
    value: ValueView<'a>,
    secret: bool,
    state: &'a SerializationState,
    workspace: &'a WorkspaceBudget,
}

#[derive(Clone, Copy)]
enum PayloadData<'a> {
    Unit,
    Single {
        ty: TypeView<'a>,
        value: ValueView<'a>,
    },
    Structure {
        fields: FieldList<'a>,
        values: ValueEntries<'a>,
    },
}

struct EnumPayloadNode<'a> {
    data: PayloadData<'a>,
    state: &'a SerializationState,
    workspace: &'a WorkspaceBudget,
}

struct Envelope<'a> {
    root_type: &'a str,
    ty: TypeView<'a>,
    value: ValueView<'a>,
    state: &'a SerializationState,
    workspace: &'a WorkspaceBudget,
}

impl ConfigIrSerializer {
    pub(super) fn serialize_to(
        schema: &CompiledSchema,
        value: &ConfigValue,
        options: ConfigIrOptions,
        output: &mut dyn Write,
    ) -> Result<(), ConfigIrError> {
        let entry = schema.entry();
        let root_type = schema_access(|out| entry.qualified_name(out))
            .map_err(|_| ConfigIrError::SchemaAccess)?
            .ok_or(ConfigIrError::SchemaAccess)?;
        let state = SerializationState::new(options.max_nesting_depth(), options.reject_secrets());
        let workspace = WorkspaceBudget::new(options.max_workspace_bytes());
        let mut output = BoundedBuffer::new(options.max_output_bytes(), output);
        let envelope = Envelope {
            root_type,
            ty: entry,
            value: value.root(),
            state: &state,
            workspace: &workspace,
        };
        let result = {
            let mut serializer = JsonSerializer::new(&mut output, options.pretty());
            envelope.serialize(&mut serializer)
        };
        if result.is_err() {
            return Err(output
                .take_failure()
                .or(state.take_failure())
                .unwrap_or(ConfigIrError::SerializationFailed));
        }
        if output.finish().is_err() {
            return Err(output
                .take_failure()
                .unwrap_or(ConfigIrError::SerializationFailed));
        }
        Ok(())
    }
}

fn serialization_failure<E>(state: &SerializationState, error: ConfigIrError) -> E
where
    E: SerError,
{
    state.record(error);
    E::custom("config IR serialization failed")
}

fn schema_access_for<T, E>(
    state: &SerializationState,
    operation: impl for<'slot> FnOnce(Out<'slot, 'slot, SchemaAccessError>) -> OResult<'slot, T>,
) -> Result<T, E>
where
    E: SerError,
{
    schema_access(operation).map_err(|_| serialization_failure(state, ConfigIrError::SchemaAccess))
}

impl Serialize for TypeDescriptor<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let _depth = self
            .state
            .enter_type()
            .map_err(|error| serialization_failure(self.state, error))?;
        let kind = schema_access_for::<_, S::Error>(self.state, |out| self.ty.kind(out))?;
        match kind {
            TypeKind::Builtin(builtin) => {
                let mut state = serializer.serialize_struct("TypeDescriptor", 1)?;
                state.serialize_field("kind", BuiltinNames::name(builtin))?;
                state.end()
            }
            TypeKind::Option { inner } => {
                let mut state = serializer.serialize_struct("TypeDescriptor", 2)?;
                state.serialize_field("kind", "option")?;
                state.serialize_field(
                    "inner",
                    &Self {
                        ty: inner,
                        state: self.state,
                    },
                )?;
                state.end()
            }
            TypeKind::Array { element, length } => {
                let mut state = serializer.serialize_struct("TypeDescriptor", 3)?;
                state.serialize_field("kind", "array")?;
                state.serialize_field(
                    "element",
                    &Self {
                        ty: element,
                        state: self.state,
                    },
                )?;
                state.serialize_field("length", &length)?;
                state.end()
            }
            TypeKind::Map { value } => {
                let mut state = serializer.serialize_struct("TypeDescriptor", 2)?;
                state.serialize_field("kind", "map")?;
                state.serialize_field(
                    "value",
                    &Self {
                        ty: value,
                        state: self.state,
                    },
                )?;
                state.end()
            }
            TypeKind::Struct { .. } => {
                let name = schema_access_for::<_, S::Error>(self.state, |out| {
                    self.ty.qualified_name(out)
                })?
                .ok_or_else(|| serialization_failure(self.state, ConfigIrError::SchemaAccess))?;
                let mut state = serializer.serialize_struct("TypeDescriptor", 2)?;
                state.serialize_field("kind", "struct")?;
                state.serialize_field("name", name)?;
                state.end()
            }
            TypeKind::Enum { .. } => {
                let name = schema_access_for::<_, S::Error>(self.state, |out| {
                    self.ty.qualified_name(out)
                })?
                .ok_or_else(|| serialization_failure(self.state, ConfigIrError::SchemaAccess))?;
                let mut state = serializer.serialize_struct("TypeDescriptor", 2)?;
                state.serialize_field("kind", "enum")?;
                state.serialize_field("name", name)?;
                state.end()
            }
        }
    }
}

impl Serialize for NodeValue<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let _depth = self
            .state
            .enter_value()
            .map_err(|error| serialization_failure(self.state, error))?;
        let kind = schema_access_for::<_, S::Error>(self.state, |out| self.ty.kind(out))?;
        let secret = self.secret
            || schema_access_for::<_, S::Error>(self.state, |out| self.ty.is_secret(out))?;
        if secret && self.state.rejects_secrets() {
            return Err(serialization_failure(
                self.state,
                ConfigIrError::SecretValueRejected,
            ));
        }
        let node = ValueNode::resolve(kind, self.value)
            .ok_or_else(|| serialization_failure(self.state, ConfigIrError::ValueShapeMismatch))?;
        match node {
            ValueNodeKind::Signed {
                builtin,
                value: number,
            } => {
                if !signed_value_fits(builtin, number) {
                    return Err(serialization_failure(
                        self.state,
                        ConfigIrError::ValueShapeMismatch,
                    ));
                }
                let mut state =
                    serializer.serialize_struct("ValueNode", 2 + usize::from(secret))?;
                state.serialize_field(
                    "type",
                    &TypeDescriptor {
                        ty: self.ty,
                        state: self.state,
                    },
                )?;
                state.serialize_field("value", &Decimal::new(number, self.state))?;
                serialize_secret(&mut state, secret)?;
                state.end()
            }
            ValueNodeKind::Unsigned {
                builtin,
                value: number,
            } => {
                if !unsigned_value_fits(builtin, number) {
                    return Err(serialization_failure(
                        self.state,
                        ConfigIrError::ValueShapeMismatch,
                    ));
                }
                let mut state =
                    serializer.serialize_struct("ValueNode", 2 + usize::from(secret))?;
                state.serialize_field(
                    "type",
                    &TypeDescriptor {
                        ty: self.ty,
                        state: self.state,
                    },
                )?;
                state.serialize_field("value", &Decimal::new(number, self.state))?;
                serialize_secret(&mut state, secret)?;
                state.end()
            }
            ValueNodeKind::Float32(number) => {
                if !number.is_finite() {
                    return Err(serialization_failure(
                        self.state,
                        ConfigIrError::UnsupportedFloat,
                    ));
                }
                let mut state =
                    serializer.serialize_struct("ValueNode", 2 + usize::from(secret))?;
                state.serialize_field(
                    "type",
                    &TypeDescriptor {
                        ty: self.ty,
                        state: self.state,
                    },
                )?;
                if number == 0.0 && number.is_sign_negative() {
                    state.serialize_field("value", "-0")?;
                } else {
                    state.serialize_field("value", &Decimal::new(number, self.state))?;
                }
                serialize_secret(&mut state, secret)?;
                state.end()
            }
            ValueNodeKind::Float64(number) => {
                if !number.is_finite() {
                    return Err(serialization_failure(
                        self.state,
                        ConfigIrError::UnsupportedFloat,
                    ));
                }
                let mut state =
                    serializer.serialize_struct("ValueNode", 2 + usize::from(secret))?;
                state.serialize_field(
                    "type",
                    &TypeDescriptor {
                        ty: self.ty,
                        state: self.state,
                    },
                )?;
                if number == 0.0 && number.is_sign_negative() {
                    state.serialize_field("value", "-0")?;
                } else {
                    state.serialize_field("value", &Decimal::new(number, self.state))?;
                }
                serialize_secret(&mut state, secret)?;
                state.end()
            }
            ValueNodeKind::String(text) => {
                let mut state =
                    serializer.serialize_struct("ValueNode", 2 + usize::from(secret))?;
                state.serialize_field(
                    "type",
                    &TypeDescriptor {
                        ty: self.ty,
                        state: self.state,
                    },
                )?;
                state.serialize_field("value", text)?;
                serialize_secret(&mut state, secret)?;
                state.end()
            }
            ValueNodeKind::Bool(boolean) => {
                let mut state =
                    serializer.serialize_struct("ValueNode", 2 + usize::from(secret))?;
                state.serialize_field(
                    "type",
                    &TypeDescriptor {
                        ty: self.ty,
                        state: self.state,
                    },
                )?;
                state.serialize_field("value", &boolean)?;
                serialize_secret(&mut state, secret)?;
                state.end()
            }
            ValueNodeKind::Optional { inner, value } => {
                let mut state =
                    serializer.serialize_struct("ValueNode", 2 + usize::from(secret))?;
                state.serialize_field(
                    "type",
                    &TypeDescriptor {
                        ty: self.ty,
                        state: self.state,
                    },
                )?;
                match value {
                    Some(value) => state.serialize_field(
                        "value",
                        &NodeValue {
                            ty: inner,
                            value,
                            secret: false,
                            state: self.state,
                            workspace: self.workspace,
                        },
                    )?,
                    None => state.serialize_field("value", &Option::<()>::None)?,
                }
                serialize_secret(&mut state, secret)?;
                state.end()
            }
            ValueNodeKind::Array {
                element,
                values: items,
                ..
            } => {
                let mut state =
                    serializer.serialize_struct("ValueNode", 2 + usize::from(secret))?;
                state.serialize_field(
                    "type",
                    &TypeDescriptor {
                        ty: self.ty,
                        state: self.state,
                    },
                )?;
                state.serialize_field(
                    "items",
                    &ArrayItems {
                        element,
                        items,
                        state: self.state,
                        workspace: self.workspace,
                    },
                )?;
                serialize_secret(&mut state, secret)?;
                state.end()
            }
            ValueNodeKind::Map {
                value_type: value,
                values: entries,
            } => {
                let mut state =
                    serializer.serialize_struct("ValueNode", 2 + usize::from(secret))?;
                state.serialize_field(
                    "type",
                    &TypeDescriptor {
                        ty: self.ty,
                        state: self.state,
                    },
                )?;
                state.serialize_field(
                    "entries",
                    &MapItems {
                        value_type: value,
                        entries,
                        state: self.state,
                        workspace: self.workspace,
                    },
                )?;
                serialize_secret(&mut state, secret)?;
                state.end()
            }
            ValueNodeKind::Structure { fields, values } => {
                let mut state =
                    serializer.serialize_struct("ValueNode", 2 + usize::from(secret))?;
                state.serialize_field(
                    "type",
                    &TypeDescriptor {
                        ty: self.ty,
                        state: self.state,
                    },
                )?;
                state.serialize_field(
                    "fields",
                    &StructFields {
                        fields,
                        values,
                        state: self.state,
                        workspace: self.workspace,
                    },
                )?;
                serialize_secret(&mut state, secret)?;
                state.end()
            }
            ValueNodeKind::Enumeration(value) => {
                let variant_view = schema_access_for::<_, S::Error>(self.state, |out| {
                    self.ty.variant_named(value.variant(), out)
                })?
                .ok_or_else(|| {
                    serialization_failure(self.state, ConfigIrError::ValueShapeMismatch)
                })?;
                let variant_secret = schema_access_for::<_, S::Error>(self.state, |out| {
                    variant_view.is_secret(out)
                })?;
                let secret = secret || variant_secret;
                if secret && self.state.rejects_secrets() {
                    return Err(serialization_failure(
                        self.state,
                        ConfigIrError::SecretValueRejected,
                    ));
                }
                let payload_info =
                    schema_access_for::<_, S::Error>(self.state, |out| variant_view.payload(out))?;
                let payload = value.resolve_payload(payload_info).ok_or_else(|| {
                    serialization_failure(self.state, ConfigIrError::ValueShapeMismatch)
                })?;
                let payload_node = match payload {
                    EnumPayloadValue::Unit => EnumPayloadNode {
                        data: PayloadData::Unit,
                        state: self.state,
                        workspace: self.workspace,
                    },
                    EnumPayloadValue::Single { ty, value } => EnumPayloadNode {
                        data: PayloadData::Single { ty, value },
                        state: self.state,
                        workspace: self.workspace,
                    },
                    EnumPayloadValue::Structure { fields, values } => EnumPayloadNode {
                        data: PayloadData::Structure { fields, values },
                        state: self.state,
                        workspace: self.workspace,
                    },
                };
                let variant_name =
                    schema_access_for::<_, S::Error>(self.state, |out| variant_view.name(out))?;
                let mut state =
                    serializer.serialize_struct("ValueNode", 3 + usize::from(secret))?;
                state.serialize_field(
                    "type",
                    &TypeDescriptor {
                        ty: self.ty,
                        state: self.state,
                    },
                )?;
                state.serialize_field("variant", variant_name)?;
                state.serialize_field("payload", &payload_node)?;
                serialize_secret(&mut state, secret)?;
                state.end()
            }
        }
    }
}

impl Serialize for ArrayItems<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut sequence = serializer.serialize_seq(Some(self.items.len()))?;
        for value in self.items {
            sequence.serialize_element(&NodeValue {
                ty: self.element,
                value: value.value(),
                secret: false,
                state: self.state,
                workspace: self.workspace,
            })?;
        }
        sequence.end()
    }
}

impl<'a> Serialize for MapItems<'a> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let workspace_bytes = self
            .entries
            .len()
            .checked_mul(size_of::<(&str, ValueEntryView<'_>)>())
            .ok_or_else(|| {
                serialization_failure(self.state, ConfigIrError::WorkspaceLimitExceeded)
            })?;
        let _reservation = self
            .workspace
            .reserve(workspace_bytes)
            .map_err(|error| serialization_failure(self.state, error))?;
        let mut entries: Vec<(&'a str, ValueEntryView<'a>)> = Vec::new();
        if entries.try_reserve_exact(self.entries.len()).is_err() {
            return Err(serialization_failure(
                self.state,
                ConfigIrError::AllocationFailed,
            ));
        }
        for entry in self.entries {
            let Some(named) = entry.as_named() else {
                return Err(serialization_failure(
                    self.state,
                    ConfigIrError::ValueShapeMismatch,
                ));
            };
            entries.push((named.name(), entry));
        }
        entries.sort_unstable_by(|left, right| left.0.cmp(right.0));
        let mut sequence = serializer.serialize_seq(Some(entries.len()))?;
        for (key, entry) in entries {
            sequence.serialize_element(&MapEntryNode {
                key,
                value_type: self.value_type,
                value: entry.value(),
                state: self.state,
                workspace: self.workspace,
            })?;
        }
        sequence.end()
    }
}

impl Serialize for MapEntryNode<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("MapEntry", 2)?;
        state.serialize_field("key", self.key)?;
        state.serialize_field(
            "value",
            &NodeValue {
                ty: self.value_type,
                value: self.value,
                secret: false,
                state: self.state,
                workspace: self.workspace,
            },
        )?;
        state.end()
    }
}

impl Serialize for StructFields<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let workspace_bytes = self
            .fields
            .len()
            .checked_mul(size_of::<usize>())
            .ok_or_else(|| {
                serialization_failure(self.state, ConfigIrError::WorkspaceLimitExceeded)
            })?;
        let _reservation = self
            .workspace
            .reserve(workspace_bytes)
            .map_err(|error| serialization_failure(self.state, error))?;
        let mut slots = Vec::new();
        if slots.try_reserve_exact(self.fields.len()).is_err() {
            return Err(serialization_failure(
                self.state,
                ConfigIrError::AllocationFailed,
            ));
        }
        slots.resize(self.fields.len(), usize::MAX);
        let mut errors = ErrSlot::<S::Error>::new();
        match errors.try_call(|out| {
            ValueNode::index_fields(
                self.values,
                &mut slots,
                |name, find_out| {
                    let found = schema_access_for::<_, S::Error>(self.state, |schema_out| {
                        self.fields.named(name, schema_out)
                    })
                    .map_err(|error| find_out.fail(error))?;
                    Ok(found.map(|(slot, _)| slot))
                },
                |_: FieldIndexError, out| {
                    Err(out.fail(serialization_failure(
                        self.state,
                        ConfigIrError::ValueShapeMismatch,
                    )))
                },
                out,
            )
        }) {
            Ok(()) => {}
            Err(caught) => return Err(caught.take()),
        }
        let mut sequence = serializer.serialize_seq(Some(self.fields.len()))?;
        for (field_slot, field) in self.fields.iter().enumerate() {
            let name = schema_access_for::<_, S::Error>(self.state, |out| field.name(out))?;
            let entry_slot = slots.get(field_slot).copied().ok_or_else(|| {
                serialization_failure(self.state, ConfigIrError::ValueShapeMismatch)
            })?;
            let value = self.values.get(entry_slot).ok_or_else(|| {
                serialization_failure(self.state, ConfigIrError::ValueShapeMismatch)
            })?;
            let ty = schema_access_for::<_, S::Error>(self.state, |out| field.ty(out))?;
            let secret = schema_access_for::<_, S::Error>(self.state, |out| field.is_secret(out))?;
            sequence.serialize_element(&FieldNode {
                name,
                ty,
                value: value.value(),
                secret,
                state: self.state,
                workspace: self.workspace,
            })?;
        }
        sequence.end()
    }
}

impl Serialize for FieldNode<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("Field", 2)?;
        state.serialize_field("name", self.name)?;
        state.serialize_field(
            "value",
            &NodeValue {
                ty: self.ty,
                value: self.value,
                secret: self.secret,
                state: self.state,
                workspace: self.workspace,
            },
        )?;
        state.end()
    }
}

impl Serialize for EnumPayloadNode<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self.data {
            PayloadData::Unit => {
                let mut state = serializer.serialize_struct("EnumPayload", 1)?;
                state.serialize_field("kind", "unit")?;
                state.end()
            }
            PayloadData::Single { ty, value } => {
                let mut state = serializer.serialize_struct("EnumPayload", 2)?;
                state.serialize_field("kind", "single")?;
                state.serialize_field(
                    "value",
                    &NodeValue {
                        ty,
                        value,
                        secret: false,
                        state: self.state,
                        workspace: self.workspace,
                    },
                )?;
                state.end()
            }
            PayloadData::Structure { fields, values } => {
                let mut state = serializer.serialize_struct("EnumPayload", 2)?;
                state.serialize_field("kind", "struct")?;
                state.serialize_field(
                    "fields",
                    &StructFields {
                        fields,
                        values,
                        state: self.state,
                        workspace: self.workspace,
                    },
                )?;
                state.end()
            }
        }
    }
}

impl Serialize for Envelope<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("ConfigIr", 4)?;
        state.serialize_field("format", IR_FORMAT)?;
        state.serialize_field("version", &IR_VERSION)?;
        state.serialize_field("root_type", self.root_type)?;
        state.serialize_field(
            "value",
            &NodeValue {
                ty: self.ty,
                value: self.value,
                secret: false,
                state: self.state,
                workspace: self.workspace,
            },
        )?;
        state.end()
    }
}

fn serialize_secret<S>(state: &mut S, secret: bool) -> Result<(), S::Error>
where
    S: SerializeStruct,
{
    if secret {
        state.serialize_field("secret", &true)?;
    }
    Ok(())
}

fn signed_value_fits(builtin: BuiltinType, value: i128) -> bool {
    match builtin {
        BuiltinType::I8 => i8::try_from(value).is_ok(),
        BuiltinType::I16 => i16::try_from(value).is_ok(),
        BuiltinType::I32 => i32::try_from(value).is_ok(),
        BuiltinType::I64 => i64::try_from(value).is_ok(),
        BuiltinType::I128 => true,
        _ => false,
    }
}

fn unsigned_value_fits(builtin: BuiltinType, value: u128) -> bool {
    match builtin {
        BuiltinType::U8 => u8::try_from(value).is_ok(),
        BuiltinType::U16 => u16::try_from(value).is_ok(),
        BuiltinType::U32 => u32::try_from(value).is_ok(),
        BuiltinType::U64 => u64::try_from(value).is_ok(),
        BuiltinType::U128 => true,
        _ => false,
    }
}

fn schema_access<T>(
    operation: impl for<'slot> FnOnce(Out<'slot, 'slot, SchemaAccessError>) -> OResult<'slot, T>,
) -> Result<T, SchemaAccessError> {
    let mut errors = ErrSlot::<SchemaAccessError>::new();
    match errors.try_call(operation) {
        Ok(value) => Ok(value),
        Err(caught) => Err(caught.take()),
    }
}
