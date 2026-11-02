use opeo::{OResult, Out};
use typol_core::{NamedValue, ValueId};
use typol_schema::{ResolverError, SchemaResolveRequest, SchemaResolver, SchemaSource};

use super::values::ConfigValue;

pub struct FieldValue {
    name: String,
    value: ConfigValue,
}

pub struct MapEntry {
    key: String,
    value: ConfigValue,
}

pub enum EnumPayload {
    Unit,
    Single(Box<ConfigValue>),
    Structure(Vec<FieldValue>),
}

pub(super) struct NamedValueRecord {
    name: String,
    value: ValueId,
}

pub struct NoImports;

impl FieldValue {
    pub fn new_unmetered(name: String, value: ConfigValue) -> Self {
        Self { name, value }
    }

    pub(super) fn into_parts(self) -> (String, ConfigValue) {
        (self.name, self.value)
    }
}

impl MapEntry {
    pub fn new_unmetered(key: String, value: ConfigValue) -> Self {
        Self { key, value }
    }

    pub(super) fn into_parts(self) -> (String, ConfigValue) {
        (self.key, self.value)
    }
}

impl EnumPayload {
    pub fn unit() -> Self {
        Self::Unit
    }

    pub fn single_unmetered<'slot, E>(
        value: ConfigValue,
        _: Out<'slot, '_, E>,
    ) -> OResult<'slot, Self> {
        Ok(Self::Single(Box::new(value)))
    }

    pub fn structure_unmetered<'slot, E>(
        fields: Vec<FieldValue>,
        _: Out<'slot, '_, E>,
    ) -> OResult<'slot, Self> {
        Ok(Self::Structure(fields))
    }
}

impl NamedValueRecord {
    pub(super) fn new(name: String, value: ValueId) -> Self {
        Self { name, value }
    }

    pub(super) fn named_values(records: &[Self]) -> Vec<NamedValue<'_>> {
        records
            .iter()
            .map(|record| NamedValue::new(&record.name, record.value))
            .collect()
    }
}

impl SchemaResolver for NoImports {
    fn resolve<'src, 'slot, 'borrow>(
        &'src mut self,
        _: SchemaResolveRequest<'_>,
        out: Out<'slot, 'borrow, ResolverError>,
    ) -> OResult<'slot, SchemaSource<'src>> {
        Err(out.fail(ResolverError::NotFound))
    }
}
