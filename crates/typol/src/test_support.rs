use crate::value::{EnumPayload, FieldValue, MapEntry, ValueFactory};
use opeo::{OResult, Out};
use std::process::abort;
use typol_core::{ChargedVec, ConfigValue, ValueError};

pub(crate) use typol_test_support::{TestOpeo, TestOpeoOutcome};

pub(crate) struct TestValues;

impl TestValues {
    pub(crate) fn signed(value: i128) -> ConfigValue {
        build(|out| ValueFactory::signed(value, None, out))
    }

    pub(crate) fn unsigned(value: u128) -> ConfigValue {
        build(|out| ValueFactory::unsigned(value, None, out))
    }

    pub(crate) fn float32(value: f32) -> ConfigValue {
        build(|out| ValueFactory::float32(value, None, out))
    }

    pub(crate) fn float64(value: f64) -> ConfigValue {
        build(|out| ValueFactory::float64(value, None, out))
    }

    pub(crate) fn boolean(value: bool) -> ConfigValue {
        build(|out| ValueFactory::boolean(value, None, out))
    }

    pub(crate) fn string_unmetered(value: String) -> ConfigValue {
        build(|out| ValueFactory::string(&value, None, out))
    }

    pub(crate) fn array_unmetered(values: Vec<ConfigValue>) -> ConfigValue {
        let values = ChargedVec::from_vec(values);
        build(|out| ValueFactory::array(values, None, out))
    }

    pub(crate) fn enumeration_unmetered(name: String, payload: EnumPayload) -> ConfigValue {
        build(|out| ValueFactory::enumeration(&name, payload, None, out))
    }

    #[cfg(feature = "serde")]
    pub(crate) fn some_unmetered(value: ConfigValue) -> ConfigValue {
        build(|out| ValueFactory::optional(Some(value), None, out))
    }

    pub(crate) fn field(name: String, value: ConfigValue) -> FieldValue {
        build(|out| FieldValue::new(&name, value, None, out))
    }

    pub(crate) fn map_entry(name: String, value: ConfigValue) -> MapEntry {
        build(|out| MapEntry::new(&name, value, None, out))
    }

    pub(crate) fn try_structure<'slot>(
        values: Vec<FieldValue>,
        out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ConfigValue> {
        let values = ChargedVec::from_vec(values);
        ValueFactory::structure(values, None, out)
    }

    pub(crate) fn try_map<'slot>(
        values: Vec<MapEntry>,
        out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ConfigValue> {
        let values = ChargedVec::from_vec(values);
        ValueFactory::map(values, None, out)
    }

    pub(crate) fn structure_unmetered(values: Vec<FieldValue>) -> ConfigValue {
        build(|out| ValueFactory::structure(ChargedVec::from_vec(values), None, out))
    }

    pub(crate) fn map_unmetered(values: Vec<MapEntry>) -> ConfigValue {
        build(|out| ValueFactory::map(ChargedVec::from_vec(values), None, out))
    }
}

fn build<T>(
    operation: impl for<'slot> FnOnce(Out<'slot, 'slot, ValueError>) -> OResult<'slot, T>,
) -> T {
    match TestOpeo::call(operation) {
        Ok(value) => value,
        Err(_error) => abort(),
    }
}

#[cfg(test)]
mod tests {
    use super::TestValues;
    use typol_core::ValueKind;

    #[test]
    fn unmetered_container_helpers_preserve_their_shapes() {
        let structure = TestValues::structure_unmetered(Vec::new());
        let map = TestValues::map_unmetered(Vec::new());

        assert!(matches!(structure.as_ref(), ValueKind::Structure(_)));
        assert!(matches!(map.as_ref(), ValueKind::Map(_)));
    }
}
