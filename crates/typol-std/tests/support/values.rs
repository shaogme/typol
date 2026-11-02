use opeo::{OResult, Out};
use typol_core::{ConfigValueBuilder, ValueError, ValueId, ValuePayload, ValueSource};

use super::records::{EnumPayload, FieldValue, MapEntry, NamedValueRecord};

pub enum ConfigValue {
    Signed {
        value: i128,
        source: ValueSource,
    },
    Unsigned {
        value: u128,
        source: ValueSource,
    },
    Float32 {
        value: f32,
        source: ValueSource,
    },
    Float64 {
        value: f64,
        source: ValueSource,
    },
    String {
        value: String,
        source: ValueSource,
    },
    Bool {
        value: bool,
        source: ValueSource,
    },
    Optional {
        value: Option<Box<ConfigValue>>,
        source: ValueSource,
    },
    Array {
        values: Vec<ConfigValue>,
        source: ValueSource,
    },
    Map {
        entries: Vec<MapEntry>,
        source: ValueSource,
    },
    Structure {
        fields: Vec<FieldValue>,
        source: ValueSource,
    },
    Enumeration {
        variant: String,
        payload: EnumPayload,
        source: ValueSource,
    },
}

impl ConfigValue {
    pub fn signed(value: i128) -> Self {
        Self::Signed {
            value,
            source: ValueSource::RustValue,
        }
    }

    pub fn unsigned(value: u128) -> Self {
        Self::Unsigned {
            value,
            source: ValueSource::RustValue,
        }
    }

    pub fn float32(value: f32) -> Self {
        Self::Float32 {
            value,
            source: ValueSource::RustValue,
        }
    }

    pub fn float64(value: f64) -> Self {
        Self::Float64 {
            value,
            source: ValueSource::RustValue,
        }
    }

    pub fn string_unmetered(value: String) -> Self {
        Self::String {
            value,
            source: ValueSource::RustValue,
        }
    }

    pub fn boolean(value: bool) -> Self {
        Self::Bool {
            value,
            source: ValueSource::RustValue,
        }
    }

    pub fn none() -> Self {
        Self::Optional {
            value: None,
            source: ValueSource::RustValue,
        }
    }

    pub fn some_unmetered<'slot, E>(value: Self, _: Out<'slot, '_, E>) -> OResult<'slot, Self> {
        Ok(Self::Optional {
            value: Some(Box::new(value)),
            source: ValueSource::RustValue,
        })
    }

    pub fn array_unmetered(values: Vec<Self>) -> Result<Self, ValueError> {
        Ok(Self::Array {
            values,
            source: ValueSource::RustValue,
        })
    }

    pub fn map_unmetered<'slot, E>(
        entries: Vec<MapEntry>,
        _: Out<'slot, '_, E>,
    ) -> OResult<'slot, Self> {
        Ok(Self::Map {
            entries,
            source: ValueSource::RustValue,
        })
    }

    pub fn structure_unmetered<'slot, E>(
        fields: Vec<FieldValue>,
        _: Out<'slot, '_, E>,
    ) -> OResult<'slot, Self> {
        Ok(Self::Structure {
            fields,
            source: ValueSource::RustValue,
        })
    }

    pub fn enumeration_unmetered(
        variant: String,
        payload: EnumPayload,
    ) -> Result<Self, ValueError> {
        Ok(Self::Enumeration {
            variant,
            payload,
            source: ValueSource::RustValue,
        })
    }

    pub fn with_source(mut self, source: ValueSource) -> Self {
        match &mut self {
            Self::Signed { source: value, .. }
            | Self::Unsigned { source: value, .. }
            | Self::Float32 { source: value, .. }
            | Self::Float64 { source: value, .. }
            | Self::String { source: value, .. }
            | Self::Bool { source: value, .. }
            | Self::Optional { source: value, .. }
            | Self::Array { source: value, .. }
            | Self::Map { source: value, .. }
            | Self::Structure { source: value, .. }
            | Self::Enumeration { source: value, .. } => *value = source,
        }
        self
    }

    pub(super) fn append_to<'slot>(
        self,
        builder: &mut ConfigValueBuilder,
        mut out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ValueId> {
        match self {
            Self::Signed { value, source } => builder.add_signed(value, source, out),
            Self::Unsigned { value, source } => builder.add_unsigned(value, source, out),
            Self::Float32 { value, source } => builder.add_float32(value, source, out),
            Self::Float64 { value, source } => builder.add_float64(value, source, out),
            Self::String { value, source } => builder.add_string(&value, source, out),
            Self::Bool { value, source } => builder.add_bool(value, source, out),
            Self::Optional { value, source } => {
                let value = match value {
                    Some(value) => Some(value.append_to(builder, out.reborrow())?),
                    None => None,
                };
                builder.add_optional(value, source, out)
            }
            Self::Array { values, source } => {
                let mut ids = Vec::with_capacity(values.len());
                for value in values {
                    ids.push(value.append_to(builder, out.reborrow())?);
                }
                builder.add_array(&ids, source, out)
            }
            Self::Map { entries, source } => {
                let fields = entries
                    .into_iter()
                    .map(|entry| {
                        let (name, value) = entry.into_parts();
                        FieldValue::new_unmetered(name, value)
                    })
                    .collect();
                let records = append_named(fields, builder, out.reborrow())?;
                let values = NamedValueRecord::named_values(&records);
                builder.add_map(&values, source, out)
            }
            Self::Structure { fields, source } => {
                let records = append_named(fields, builder, out.reborrow())?;
                let values = NamedValueRecord::named_values(&records);
                builder.add_structure(&values, source, out)
            }
            Self::Enumeration {
                variant,
                payload,
                source,
            } => match payload {
                EnumPayload::Unit => {
                    builder.add_enumeration(&variant, ValuePayload::Unit, source, out)
                }
                EnumPayload::Single(value) => {
                    let value = value.append_to(builder, out.reborrow())?;
                    builder.add_enumeration(&variant, ValuePayload::Single(value), source, out)
                }
                EnumPayload::Structure(fields) => {
                    let records = append_named(fields, builder, out.reborrow())?;
                    let values = NamedValueRecord::named_values(&records);
                    builder.add_enumeration(&variant, ValuePayload::Structure(&values), source, out)
                }
            },
        }
    }
}

fn append_named<'slot>(
    entries: Vec<FieldValue>,
    builder: &mut ConfigValueBuilder,
    mut out: Out<'slot, '_, ValueError>,
) -> OResult<'slot, Vec<NamedValueRecord>> {
    let mut records = Vec::with_capacity(entries.len());
    for entry in entries {
        let (name, value) = entry.into_parts();
        records.push(NamedValueRecord::new(
            name,
            value.append_to(builder, out.reborrow())?,
        ));
    }
    Ok(records)
}
