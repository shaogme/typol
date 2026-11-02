extern crate alloc;

use crate::{TestFailure, TestOpeo, TestResult};
use alloc::{borrow::ToOwned, string::String, vec, vec::Vec};
use opeo::{OResult, Out};
use typol_core::{
    AllocationError, BudgetError, ConfigValue, ConfigValueBuilder, NamedValue, ValueEntryView,
    ValueError, ValueId, ValueKind, ValuePayload, ValuePayloadView, ValueSource, ValueView,
};

pub struct Named {
    name: String,
    value: ConfigValue,
}

impl Named {
    pub fn try_new_unmetered<'slot>(
        name: &str,
        value: ConfigValue,
        _out: Out<'slot, '_, AllocationError>,
    ) -> OResult<'slot, Self> {
        Ok(Self {
            name: name.to_owned(),
            value,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn key(&self) -> &str {
        &self.name
    }

    pub fn value(&self) -> &ConfigValue {
        &self.value
    }
}

pub type FieldValue = Named;
pub type MapEntry = Named;

pub enum Payload {
    Unit,
    Single(ConfigValue),
    Structure(Vec<Named>),
}

enum CopyShape<'a> {
    Signed(i128),
    Unsigned(u128),
    Float32(f32),
    Float64(f64),
    String(&'a str),
    Bool(bool),
    Optional(bool),
    Array,
    Map(Vec<Option<&'a str>>),
    Structure(Vec<Option<&'a str>>),
    EnumUnit(&'a str),
    EnumSingle(&'a str),
    EnumStructure(&'a str, Vec<Option<&'a str>>),
}

enum CopyTask<'a> {
    Enter(ValueView<'a>, Option<ValueSource>),
    Finish(CopyShape<'a>, usize, ValueSource),
}

fn try_build(
    add: impl for<'slot> FnOnce(
        &mut ConfigValueBuilder,
        Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ValueId>,
) -> Result<ConfigValue, ValueError> {
    TestOpeo::call(|mut out| {
        let mut builder = ConfigValueBuilder::new(None, out.reborrow())?;
        let root = add(&mut builder, out.reborrow())?;
        builder.finish(root, out.reborrow())
    })
}

fn build(
    add: impl for<'slot> FnOnce(
        &mut ConfigValueBuilder,
        Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ValueId>,
) -> TestResult<ConfigValue> {
    try_build(add).map_err(TestFailure::from_debug)
}

fn signed(value: i128) -> TestResult<ConfigValue> {
    build(|builder, out| builder.add_signed(value, ValueSource::RustValue, out))
}

fn unsigned(value: u128) -> TestResult<ConfigValue> {
    build(|builder, out| builder.add_unsigned(value, ValueSource::RustValue, out))
}

fn float32(value: f32) -> TestResult<ConfigValue> {
    build(|builder, out| builder.add_float32(value, ValueSource::RustValue, out))
}

fn float64(value: f64) -> TestResult<ConfigValue> {
    build(|builder, out| builder.add_float64(value, ValueSource::RustValue, out))
}

fn boolean(value: bool) -> TestResult<ConfigValue> {
    build(|builder, out| builder.add_bool(value, ValueSource::RustValue, out))
}

fn try_string(value: &str) -> Result<ConfigValue, ValueError> {
    TestOpeo::call(|mut out| {
        let mut builder = ConfigValueBuilder::new(None, out.reborrow())?;
        let root = builder.add_string(value, ValueSource::RustValue, out.reborrow())?;
        builder.finish(root, out)
    })
}

fn string(value: &str) -> TestResult<ConfigValue> {
    try_string(value).map_err(TestFailure::from_debug)
}

fn none() -> TestResult<ConfigValue> {
    build(|builder, out| builder.add_optional(None, ValueSource::RustValue, out))
}

fn some(value: ConfigValue) -> TestResult<ConfigValue> {
    build(|builder, mut out| {
        let child = copy_into(builder, value.root(), None, &mut out)?;
        builder.add_optional(Some(child), ValueSource::RustValue, out)
    })
}

fn try_some(value: ConfigValue) -> Result<ConfigValue, ValueError> {
    try_build(|builder, mut out| {
        let child = copy_into(builder, value.root(), None, &mut out)?;
        builder.add_optional(Some(child), ValueSource::RustValue, out)
    })
}

fn array(values: Vec<ConfigValue>) -> TestResult<ConfigValue> {
    build(|builder, mut out| {
        let mut children = Vec::with_capacity(values.len());
        for value in values {
            children.push(copy_into(builder, value.root(), None, &mut out)?);
        }
        builder.add_array(&children, ValueSource::RustValue, out)
    })
}

fn try_array(values: Vec<ConfigValue>) -> Result<ConfigValue, ValueError> {
    try_build(|builder, mut out| {
        let mut children = Vec::with_capacity(values.len());
        for value in values {
            children.push(copy_into(builder, value.root(), None, &mut out)?);
        }
        builder.add_array(&children, ValueSource::RustValue, out)
    })
}

fn named(name: &str, value: ConfigValue) -> Named {
    Named {
        name: name.to_owned(),
        value,
    }
}

fn structure(values: Vec<Named>) -> TestResult<ConfigValue> {
    named_value(values, false)
}

fn try_structure(values: Vec<Named>) -> Result<ConfigValue, ValueError> {
    try_named_value(values, false)
}

fn map(values: Vec<Named>) -> TestResult<ConfigValue> {
    named_value(values, true)
}

fn try_map(values: Vec<Named>) -> Result<ConfigValue, ValueError> {
    try_named_value(values, true)
}

fn named_value(values: Vec<Named>, map: bool) -> TestResult<ConfigValue> {
    try_named_value(values, map).map_err(TestFailure::from_debug)
}

fn try_named_value(values: Vec<Named>, map: bool) -> Result<ConfigValue, ValueError> {
    try_build(|builder, mut out| {
        let mut named_values = Vec::with_capacity(values.len());
        for entry in values {
            let child = copy_into(builder, entry.value.root(), None, &mut out)?;
            named_values.push((entry.name, child));
        }
        let entries = named_values
            .iter()
            .map(|(name, value)| NamedValue::new(name, *value))
            .collect::<Vec<_>>();
        if map {
            builder.add_map(&entries, ValueSource::RustValue, out)
        } else {
            builder.add_structure(&entries, ValueSource::RustValue, out)
        }
    })
}

fn enumeration(variant: &str, payload: Payload) -> TestResult<ConfigValue> {
    build(|builder, mut out| match payload {
        Payload::Unit => {
            builder.add_enumeration(variant, ValuePayload::Unit, ValueSource::RustValue, out)
        }
        Payload::Single(value) => {
            let value = copy_into(builder, value.root(), None, &mut out)?;
            builder.add_enumeration(
                variant,
                ValuePayload::Single(value),
                ValueSource::RustValue,
                out,
            )
        }
        Payload::Structure(fields) => {
            let mut named_values = Vec::with_capacity(fields.len());
            for field in fields {
                let value = copy_into(builder, field.value.root(), None, &mut out)?;
                named_values.push((field.name, value));
            }
            let entries = named_values
                .iter()
                .map(|(name, value)| NamedValue::new(name, *value))
                .collect::<Vec<_>>();
            builder.add_enumeration(
                variant,
                ValuePayload::Structure(&entries),
                ValueSource::RustValue,
                out,
            )
        }
    })
}

fn try_enumeration(variant: &str, payload: Payload) -> Result<ConfigValue, ValueError> {
    try_build(|builder, mut out| match payload {
        Payload::Unit => {
            builder.add_enumeration(variant, ValuePayload::Unit, ValueSource::RustValue, out)
        }
        Payload::Single(value) => {
            let value = copy_into(builder, value.root(), None, &mut out)?;
            builder.add_enumeration(
                variant,
                ValuePayload::Single(value),
                ValueSource::RustValue,
                out,
            )
        }
        Payload::Structure(fields) => {
            let mut named_values = Vec::with_capacity(fields.len());
            for field in fields {
                let value = copy_into(builder, field.value.root(), None, &mut out)?;
                named_values.push((field.name, value));
            }
            let entries = named_values
                .iter()
                .map(|(name, value)| NamedValue::new(name, *value))
                .collect::<Vec<_>>();
            builder.add_enumeration(
                variant,
                ValuePayload::Structure(&entries),
                ValueSource::RustValue,
                out,
            )
        }
    })
}

fn with_source(value: ConfigValue, source: ValueSource) -> TestResult<ConfigValue> {
    build(|builder, mut out| copy_into(builder, value.root(), Some(source), &mut out))
}

fn copy_into<'slot, 'a>(
    builder: &mut ConfigValueBuilder,
    root: ValueView<'a>,
    root_source: Option<ValueSource>,
    out: &mut Out<'slot, '_, ValueError>,
) -> OResult<'slot, ValueId> {
    let mut tasks = vec![CopyTask::Enter(root, root_source)];
    let mut values = Vec::new();
    while let Some(task) = tasks.pop() {
        match task {
            CopyTask::Enter(value, override_source) => {
                let source = override_source.unwrap_or_else(|| value.source());
                let mut children = Vec::new();
                let shape = match value.as_ref() {
                    ValueKind::Signed(value) => CopyShape::Signed(value),
                    ValueKind::Unsigned(value) => CopyShape::Unsigned(value),
                    ValueKind::Float32(value) => CopyShape::Float32(value),
                    ValueKind::Float64(value) => CopyShape::Float64(value),
                    ValueKind::String(value) => CopyShape::String(value),
                    ValueKind::Bool(value) => CopyShape::Bool(value),
                    ValueKind::Optional(None) => CopyShape::Optional(false),
                    ValueKind::Optional(Some(child)) => {
                        children.push(child);
                        CopyShape::Optional(true)
                    }
                    ValueKind::Array(entries) => {
                        children.extend(entries.map(|entry| entry.value()));
                        CopyShape::Array
                    }
                    ValueKind::Map(entries) => {
                        let mut names = Vec::with_capacity(entries.len());
                        for entry in entries {
                            names.push(entry.name());
                            children.push(entry.value());
                        }
                        CopyShape::Map(names)
                    }
                    ValueKind::Structure(entries) => {
                        let mut names = Vec::with_capacity(entries.len());
                        for entry in entries {
                            names.push(entry.name());
                            children.push(entry.value());
                        }
                        CopyShape::Structure(names)
                    }
                    ValueKind::Enumeration { variant, payload } => match payload {
                        ValuePayloadView::Unit => CopyShape::EnumUnit(variant),
                        ValuePayloadView::Single(child) => {
                            children.push(child);
                            CopyShape::EnumSingle(variant)
                        }
                        ValuePayloadView::Structure(entries) => {
                            let mut names = Vec::with_capacity(entries.len());
                            for entry in entries {
                                names.push(entry.name());
                                children.push(entry.value());
                            }
                            CopyShape::EnumStructure(variant, names)
                        }
                    },
                };
                let count = children.len();
                tasks.push(CopyTask::Finish(shape, count, source));
                tasks.extend(
                    children
                        .into_iter()
                        .rev()
                        .map(|child| CopyTask::Enter(child, None)),
                );
            }
            CopyTask::Finish(shape, count, source) => {
                let start = values.len().saturating_sub(count);
                let children = values.split_off(start);
                let id = match shape {
                    CopyShape::Signed(value) => {
                        builder.add_signed(value, source, out.reborrow())?
                    }
                    CopyShape::Unsigned(value) => {
                        builder.add_unsigned(value, source, out.reborrow())?
                    }
                    CopyShape::Float32(value) => {
                        builder.add_float32(value, source, out.reborrow())?
                    }
                    CopyShape::Float64(value) => {
                        builder.add_float64(value, source, out.reborrow())?
                    }
                    CopyShape::String(value) => {
                        builder.add_string(value, source, out.reborrow())?
                    }
                    CopyShape::Bool(value) => builder.add_bool(value, source, out.reborrow())?,
                    CopyShape::Optional(has_value) => {
                        let child = if has_value {
                            Some(*children.first().ok_or_else(|| {
                                out.reborrow()
                                    .fail(ValueError::from(BudgetError::CapacityExceeded))
                            })?)
                        } else {
                            None
                        };
                        builder.add_optional(child, source, out.reborrow())?
                    }
                    CopyShape::Array => builder.add_array(&children, source, out.reborrow())?,
                    CopyShape::Map(names) => {
                        let entries = named_ids(names, children)
                            .map_err(ValueError::from)
                            .map_err(|error| out.reborrow().fail(error))?;
                        builder.add_map(&entries, source, out.reborrow())?
                    }
                    CopyShape::Structure(names) => {
                        let entries = named_ids(names, children)
                            .map_err(ValueError::from)
                            .map_err(|error| out.reborrow().fail(error))?;
                        builder.add_structure(&entries, source, out.reborrow())?
                    }
                    CopyShape::EnumUnit(variant) => builder.add_enumeration(
                        variant,
                        ValuePayload::Unit,
                        source,
                        out.reborrow(),
                    )?,
                    CopyShape::EnumSingle(variant) => {
                        let child = *children.first().ok_or_else(|| {
                            out.reborrow()
                                .fail(ValueError::from(BudgetError::CapacityExceeded))
                        })?;
                        builder.add_enumeration(
                            variant,
                            ValuePayload::Single(child),
                            source,
                            out.reborrow(),
                        )?
                    }
                    CopyShape::EnumStructure(variant, names) => {
                        let entries = named_ids(names, children)
                            .map_err(ValueError::from)
                            .map_err(|error| out.reborrow().fail(error))?;
                        builder.add_enumeration(
                            variant,
                            ValuePayload::Structure(&entries),
                            source,
                            out.reborrow(),
                        )?
                    }
                };
                values.push(id);
            }
        }
    }
    values.pop().ok_or_else(|| {
        out.reborrow()
            .fail(ValueError::from(BudgetError::CapacityExceeded))
    })
}

fn named_ids<'a>(
    names: Vec<Option<&'a str>>,
    values: Vec<ValueId>,
) -> Result<Vec<NamedValue<'a>>, BudgetError> {
    names
        .into_iter()
        .zip(values)
        .map(|(name, value)| {
            name.map(|name| NamedValue::new(name, value))
                .ok_or(BudgetError::CapacityExceeded)
        })
        .collect()
}

pub struct ValueFactory;

impl ValueFactory {
    pub fn signed(value: i128) -> TestResult<ConfigValue> {
        signed(value)
    }

    pub fn unsigned(value: u128) -> TestResult<ConfigValue> {
        unsigned(value)
    }

    pub fn float32(value: f32) -> TestResult<ConfigValue> {
        float32(value)
    }

    pub fn float64(value: f64) -> TestResult<ConfigValue> {
        float64(value)
    }

    pub fn boolean(value: bool) -> TestResult<ConfigValue> {
        boolean(value)
    }

    pub fn try_string(value: &str) -> Result<ConfigValue, ValueError> {
        try_string(value)
    }

    pub fn string(value: &str) -> TestResult<ConfigValue> {
        string(value)
    }

    pub fn none() -> TestResult<ConfigValue> {
        none()
    }

    pub fn some(value: ConfigValue) -> TestResult<ConfigValue> {
        some(value)
    }

    pub fn try_some(value: ConfigValue) -> Result<ConfigValue, ValueError> {
        try_some(value)
    }

    pub fn array(values: Vec<ConfigValue>) -> TestResult<ConfigValue> {
        array(values)
    }

    pub fn try_array(values: Vec<ConfigValue>) -> Result<ConfigValue, ValueError> {
        try_array(values)
    }

    pub fn named(name: &str, value: ConfigValue) -> Named {
        named(name, value)
    }

    pub fn structure(values: Vec<Named>) -> TestResult<ConfigValue> {
        structure(values)
    }

    pub fn try_structure(values: Vec<Named>) -> Result<ConfigValue, ValueError> {
        try_structure(values)
    }

    pub fn map(values: Vec<Named>) -> TestResult<ConfigValue> {
        map(values)
    }

    pub fn try_map(values: Vec<Named>) -> Result<ConfigValue, ValueError> {
        try_map(values)
    }

    pub fn enumeration(variant: &str, payload: Payload) -> TestResult<ConfigValue> {
        enumeration(variant, payload)
    }

    pub fn try_enumeration(variant: &str, payload: Payload) -> Result<ConfigValue, ValueError> {
        try_enumeration(variant, payload)
    }

    pub fn with_source(value: ConfigValue, source: ValueSource) -> TestResult<ConfigValue> {
        with_source(value, source)
    }
}

pub struct ValueViewInput<'a>(ValueView<'a>);

impl<'a> ValueViewInput<'a> {
    pub fn value_view(self) -> ValueView<'a> {
        self.0
    }
}

impl<'a> From<&'a ConfigValue> for ValueViewInput<'a> {
    fn from(value: &'a ConfigValue) -> Self {
        Self(value.root())
    }
}

impl<'a> From<ValueView<'a>> for ValueViewInput<'a> {
    fn from(value: ValueView<'a>) -> Self {
        Self(value)
    }
}

impl<'a> From<ValueEntryView<'a>> for ValueViewInput<'a> {
    fn from(value: ValueEntryView<'a>) -> Self {
        Self(value.value())
    }
}

#[cfg(test)]
mod helper_tests {
    extern crate alloc;

    use super::{FieldValue, MapEntry, Named, Payload, ValueFactory, ValueViewInput};
    use crate::{TestFailure, TestOpeo, TestResult, test_assert, test_eq};
    use alloc::vec;
    use typol_core::{ValueKind, ValueSource, ValueView};

    fn view<'a>(value: impl Into<ValueViewInput<'a>>) -> ValueView<'a> {
        value.into().value_view()
    }

    #[test]
    fn shared_value_builders_cover_arena_shapes() -> TestResult<()> {
        let _ = ValueFactory::signed(-1)?;
        let _ = ValueFactory::unsigned(1)?;
        let _ = ValueFactory::float32(1.0)?;
        let _ = ValueFactory::float64(1.0)?;
        let _ = ValueFactory::boolean(true)?;
        let text = ValueFactory::try_string("text").map_err(TestFailure::from_debug)?;
        let _ = ValueFactory::string("text")?;
        let _ = ValueFactory::none()?;
        let _ = ValueFactory::some(ValueFactory::unsigned(1)?)?;
        let _ =
            ValueFactory::try_some(ValueFactory::unsigned(1)?).map_err(TestFailure::from_debug)?;
        let _ = ValueFactory::array(vec![ValueFactory::unsigned(1)?])?;
        let _ = ValueFactory::try_array(vec![ValueFactory::unsigned(1)?])
            .map_err(TestFailure::from_debug)?;
        let structure = ValueFactory::structure(vec![ValueFactory::named(
            "field",
            ValueFactory::unsigned(1)?,
        )])?;
        let _ = ValueFactory::try_structure(vec![ValueFactory::named(
            "field",
            ValueFactory::unsigned(1)?,
        )])
        .map_err(TestFailure::from_debug)?;
        let _ = ValueFactory::map(vec![ValueFactory::named("key", ValueFactory::unsigned(1)?)])?;
        let _ = ValueFactory::try_map(vec![ValueFactory::named("key", ValueFactory::unsigned(1)?)])
            .map_err(TestFailure::from_debug)?;
        let _ = ValueFactory::enumeration("Unit", Payload::Unit)?;
        let _ = ValueFactory::enumeration("Single", Payload::Single(ValueFactory::unsigned(1)?))?;
        let _ = ValueFactory::enumeration(
            "Structure",
            Payload::Structure(vec![ValueFactory::named(
                "field",
                ValueFactory::unsigned(1)?,
            )]),
        )?;
        let _ = ValueFactory::try_enumeration("Unit", Payload::Unit)
            .map_err(TestFailure::from_debug)?;
        let _ = ValueFactory::with_source(text, ValueSource::SchemaDefault)?;

        let named_value = ValueFactory::unsigned(2)?;
        let named_fixture =
            TestOpeo::call(|out| Named::try_new_unmetered("field", named_value, out))
                .map_err(|_| TestFailure::new("named fixture should build"))?;
        test_eq!(named_fixture.name(), "field")?;
        test_eq!(named_fixture.key(), "field")?;
        test_assert!(matches!(
            named_fixture.value().root().as_ref(),
            ValueKind::Unsigned(2)
        ))?;

        let ValueKind::Structure(entries) = view(&structure).as_ref() else {
            return Err(TestFailure::new(
                "structure fixture should have a structure root",
            ));
        };
        test_eq!(entries.into_iter().count(), 1)?;
        let ValueKind::Structure(entries) = view(&structure).as_ref() else {
            return Err(TestFailure::new(
                "structure fixture should have a structure root",
            ));
        };
        test_eq!(
            entries.into_iter().next().and_then(|entry| entry.name()),
            Some("field")
        )?;
        let ValueKind::Structure(mut entries) = view(&structure).as_ref() else {
            return Err(TestFailure::new(
                "structure fixture should have a structure root",
            ));
        };
        test_assert!(entries.next().is_some())?;

        let map = ValueFactory::map(vec![ValueFactory::named("key", ValueFactory::unsigned(3)?)])?;
        let ValueKind::Map(mut entries) = map.root().as_ref() else {
            return Err(TestFailure::new("map fixture should have a map root"));
        };
        test_eq!(entries.next().and_then(|entry| entry.name()), Some("key"))?;

        let ValueKind::Structure(mut entries) = structure.root().as_ref() else {
            return Err(TestFailure::new(
                "structure fixture should have a structure root",
            ));
        };
        let entry = entries
            .next()
            .ok_or_else(|| TestFailure::new("structure fixture should have an entry"))?;
        test_assert!(matches!(view(entry).as_ref(), ValueKind::Unsigned(1)))?;

        let _alias: FieldValue = ValueFactory::named("alias", ValueFactory::unsigned(4)?);
        let _map_entry: MapEntry = ValueFactory::named("alias", ValueFactory::unsigned(5)?);
        let _ = Payload::Single(ValueFactory::unsigned(6)?);
        Ok(())
    }
}
