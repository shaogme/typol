use std::{error::Error, io::Error as IoError, thread};
use typol_core::{
    ConfigValue, ConfigValueBuilder, ExecutionBudget, ExecutionLimits, NamedValue, ValueCopyLimits,
    ValueError, ValueErrorKind, ValueKind, ValuePayload, ValuePayloadView, ValueSource, ValueView,
};
use typol_opeo_test_support::TestOpeo;

fn copy_value(value: &ConfigValue, max_depth: usize) -> Result<ConfigValue, ValueError> {
    let mut builder = TestOpeo::call(|out| ConfigValueBuilder::new(None, out))?;
    let root = TestOpeo::call(|out| {
        builder.append_view(value.root(), ValueCopyLimits::new(max_depth), out)
    })?;
    TestOpeo::call(|out| builder.finish(root, out))
}

fn optional_chain(depth: usize) -> Result<ConfigValue, ValueError> {
    let mut builder = TestOpeo::call(|out| ConfigValueBuilder::new(None, out))?;
    let mut root =
        TestOpeo::call(|out| builder.add_string("leaf", ValueSource::ConfigLiteral, out))?;
    for _ in 0..depth {
        root = TestOpeo::call(|out| builder.add_optional(Some(root), ValueSource::RustValue, out))?;
    }
    TestOpeo::call(|out| builder.finish(root, out))
}

fn structure_field<'value>(
    value: ValueView<'value>,
    expected_name: &str,
) -> Option<ValueView<'value>> {
    let ValueKind::Structure(mut entries) = value.as_ref() else {
        return None;
    };
    entries
        .find(|entry| entry.name() == Some(expected_name))
        .map(|entry| entry.value())
}

enum BoundaryValue {
    Scalar,
    EmptyArray,
    EmptyMap,
    OptionalNone,
    EnumSingle,
    EmptyEnumStructure,
    EnumStructureField,
}

fn boundary_value(kind: BoundaryValue) -> Result<ConfigValue, ValueError> {
    let mut builder = TestOpeo::call(|out| ConfigValueBuilder::new(None, out))?;
    let scalar = TestOpeo::call(|out| builder.add_signed(-7, ValueSource::RustValue, out))?;
    let root = match kind {
        BoundaryValue::Scalar => scalar,
        BoundaryValue::EmptyArray => {
            TestOpeo::call(|out| builder.add_array(&[], ValueSource::ConfigLiteral, out))?
        }
        BoundaryValue::EmptyMap => {
            TestOpeo::call(|out| builder.add_map(&[], ValueSource::RustValue, out))?
        }
        BoundaryValue::OptionalNone => {
            TestOpeo::call(|out| builder.add_optional(None, ValueSource::ExplicitFallback, out))?
        }
        BoundaryValue::EnumSingle => TestOpeo::call(|out| {
            builder.add_enumeration(
                "Single",
                ValuePayload::Single(scalar),
                ValueSource::SchemaDefault,
                out,
            )
        })?,
        BoundaryValue::EmptyEnumStructure => TestOpeo::call(|out| {
            builder.add_enumeration(
                "Empty",
                ValuePayload::Structure(&[]),
                ValueSource::ConfigLiteral,
                out,
            )
        })?,
        BoundaryValue::EnumStructureField => TestOpeo::call(|out| {
            builder.add_enumeration(
                "Fields",
                ValuePayload::Structure(&[NamedValue::new("value", scalar)]),
                ValueSource::EnvironmentVariable,
                out,
            )
        })?,
    };
    TestOpeo::call(|out| builder.finish(root, out))
}

fn assert_copy_limit(
    value: &ConfigValue,
    limit: usize,
    should_succeed: bool,
) -> Result<(), Box<dyn Error>> {
    let result = copy_value(value, limit);
    if result.is_ok() != should_succeed {
        return Err(IoError::other("value copy did not follow the requested depth limit").into());
    }
    if let Err(error) = result {
        if error.kind() != ValueErrorKind::ResourceLimitExceeded {
            return Err(IoError::other("depth error used the wrong category").into());
        }
    }
    Ok(())
}

#[test]
fn exact_depth_is_allowed_and_excess_depth_is_a_resource_error() -> Result<(), Box<dyn Error>> {
    let source = optional_chain(256)?;
    let copied = copy_value(&source, 256)?;
    let mut view = copied.root();
    for _ in 0..256 {
        let ValueKind::Optional(Some(child)) = view.as_ref() else {
            return Err(IoError::other("copied optional chain ended early").into());
        };
        view = child;
    }
    if !matches!(view.as_ref(), ValueKind::String("leaf")) {
        return Err(IoError::other("copied optional chain has the wrong leaf").into());
    }

    let mut builder = TestOpeo::call(|out| ConfigValueBuilder::new(None, out))?;
    let error =
        TestOpeo::call(|out| builder.append_view(source.root(), ValueCopyLimits::new(255), out))
            .err()
            .ok_or_else(|| IoError::other("copy above its depth limit succeeded"))?;
    if error.kind() != ValueErrorKind::ResourceLimitExceeded {
        return Err(IoError::other("depth error used the wrong category").into());
    }
    let fallback = TestOpeo::call(|out| builder.add_bool(true, ValueSource::RustValue, out))?;
    let completed = TestOpeo::call(|out| builder.finish(fallback, out))?;
    if !matches!(completed.as_ref(), ValueKind::Bool(true)) {
        return Err(IoError::other("failed copy exposed an incomplete root").into());
    }
    Ok(())
}

#[test]
fn depth_rules_cover_roots_empty_containers_and_enum_payloads() -> Result<(), Box<dyn Error>> {
    assert_copy_limit(&boundary_value(BoundaryValue::Scalar)?, 0, true)?;
    assert_copy_limit(&boundary_value(BoundaryValue::EmptyArray)?, 0, true)?;
    assert_copy_limit(&boundary_value(BoundaryValue::EmptyMap)?, 0, true)?;
    assert_copy_limit(&boundary_value(BoundaryValue::OptionalNone)?, 0, true)?;
    assert_copy_limit(&boundary_value(BoundaryValue::EnumSingle)?, 1, true)?;
    assert_copy_limit(&boundary_value(BoundaryValue::EnumSingle)?, 0, false)?;
    assert_copy_limit(&boundary_value(BoundaryValue::EmptyEnumStructure)?, 1, true)?;
    assert_copy_limit(
        &boundary_value(BoundaryValue::EmptyEnumStructure)?,
        0,
        false,
    )?;
    assert_copy_limit(&boundary_value(BoundaryValue::EnumStructureField)?, 2, true)?;
    assert_copy_limit(
        &boundary_value(BoundaryValue::EnumStructureField)?,
        1,
        false,
    )?;
    Ok(())
}

#[test]
fn copies_every_kind_name_payload_order_and_source() -> Result<(), Box<dyn Error>> {
    let mut builder = TestOpeo::call(|out| ConfigValueBuilder::new(None, out))?;
    let signed = TestOpeo::call(|out| builder.add_signed(-17, ValueSource::ConfigLiteral, out))?;
    let unsigned = TestOpeo::call(|out| builder.add_unsigned(23, ValueSource::SchemaDefault, out))?;
    let float32 =
        TestOpeo::call(|out| builder.add_float32(1.25, ValueSource::EnvironmentVariable, out))?;
    let float64 =
        TestOpeo::call(|out| builder.add_float64(2.5, ValueSource::ExplicitFallback, out))?;
    let text = TestOpeo::call(|out| builder.add_string("text", ValueSource::RustValue, out))?;
    let boolean = TestOpeo::call(|out| builder.add_bool(true, ValueSource::ConfigLiteral, out))?;
    let optional_none =
        TestOpeo::call(|out| builder.add_optional(None, ValueSource::EnvironmentVariable, out))?;
    let optional_some =
        TestOpeo::call(|out| builder.add_optional(Some(boolean), ValueSource::SchemaDefault, out))?;
    let array = TestOpeo::call(|out| {
        builder.add_array(
            &[signed, unsigned, float32, float64, text, boolean],
            ValueSource::RustValue,
            out,
        )
    })?;
    let map = TestOpeo::call(|out| {
        builder.add_map(
            &[
                NamedValue::new("first", text),
                NamedValue::new("second", unsigned),
            ],
            ValueSource::ExplicitFallback,
            out,
        )
    })?;
    let structure = TestOpeo::call(|out| {
        builder.add_structure(
            &[NamedValue::new("inside", boolean)],
            ValueSource::EnvironmentVariable,
            out,
        )
    })?;
    let enum_unit = TestOpeo::call(|out| {
        builder.add_enumeration("Unit", ValuePayload::Unit, ValueSource::RustValue, out)
    })?;
    let enum_single = TestOpeo::call(|out| {
        builder.add_enumeration(
            "One",
            ValuePayload::Single(text),
            ValueSource::SchemaDefault,
            out,
        )
    })?;
    let enum_structure = TestOpeo::call(|out| {
        builder.add_enumeration(
            "Many",
            ValuePayload::Structure(&[
                NamedValue::new("left", signed),
                NamedValue::new("right", boolean),
            ]),
            ValueSource::ConfigLiteral,
            out,
        )
    })?;
    let root = TestOpeo::call(|out| {
        builder.add_structure(
            &[
                NamedValue::new("array", array),
                NamedValue::new("map", map),
                NamedValue::new("structure", structure),
                NamedValue::new("unit", enum_unit),
                NamedValue::new("single", enum_single),
                NamedValue::new("enum_fields", enum_structure),
                NamedValue::new("none", optional_none),
                NamedValue::new("some", optional_some),
            ],
            ValueSource::RustValue,
            out,
        )
    })?;
    let source = TestOpeo::call(|out| builder.finish(root, out))?;
    let copied = copy_value(&source, 3)?;
    if copied.root().source() != ValueSource::RustValue {
        return Err(IoError::other("root source tag changed").into());
    }

    let root = copied.root();
    let array =
        structure_field(root, "array").ok_or_else(|| IoError::other("array field is missing"))?;
    let ValueKind::Array(entries) = array.as_ref() else {
        return Err(IoError::other("array field changed kind").into());
    };
    let kinds = [
        ValueKind::Signed(-17),
        ValueKind::Unsigned(23),
        ValueKind::Float32(1.25),
        ValueKind::Float64(2.5),
        ValueKind::String("text"),
        ValueKind::Bool(true),
    ];
    for (index, expected) in kinds.into_iter().enumerate() {
        let actual = entries
            .get(index)
            .ok_or_else(|| IoError::other("array item order changed"))?
            .value()
            .as_ref();
        match (actual, expected) {
            (ValueKind::Signed(left), ValueKind::Signed(right)) if left == right => {}
            (ValueKind::Unsigned(left), ValueKind::Unsigned(right)) if left == right => {}
            (ValueKind::Float32(left), ValueKind::Float32(right)) if left == right => {}
            (ValueKind::Float64(left), ValueKind::Float64(right)) if left == right => {}
            (ValueKind::String(left), ValueKind::String(right)) if left == right => {}
            (ValueKind::Bool(left), ValueKind::Bool(right)) if left == right => {}
            _ => return Err(IoError::other("array item value or order changed").into()),
        }
    }
    if entries.len() != 6 {
        return Err(IoError::other("array item count changed").into());
    }

    let map = structure_field(root, "map").ok_or_else(|| IoError::other("map field is missing"))?;
    let ValueKind::Map(entries) = map.as_ref() else {
        return Err(IoError::other("map field changed kind").into());
    };
    if entries.get(0).and_then(|entry| entry.name()) != Some("first")
        || entries.get(1).and_then(|entry| entry.name()) != Some("second")
        || map.source() != ValueSource::ExplicitFallback
    {
        return Err(IoError::other("map names, order, or source changed").into());
    }

    let nested = structure_field(root, "structure")
        .ok_or_else(|| IoError::other("structure field is missing"))?;
    let nested_field = structure_field(nested, "inside")
        .ok_or_else(|| IoError::other("nested structure field is missing"))?;
    if nested.source() != ValueSource::EnvironmentVariable
        || nested_field.source() != ValueSource::ConfigLiteral
    {
        return Err(IoError::other("structure source tags changed").into());
    }

    let unit = structure_field(root, "unit")
        .ok_or_else(|| IoError::other("unit enum field is missing"))?;
    if !matches!(
        unit.as_ref(),
        ValueKind::Enumeration {
            variant: "Unit",
            payload: ValuePayloadView::Unit
        }
    ) || unit.source() != ValueSource::RustValue
    {
        return Err(IoError::other("unit enum changed during copy").into());
    }
    let single = structure_field(root, "single")
        .ok_or_else(|| IoError::other("single enum field is missing"))?;
    let ValueKind::Enumeration {
        variant,
        payload: ValuePayloadView::Single(child),
    } = single.as_ref()
    else {
        return Err(IoError::other("single enum payload changed kind").into());
    };
    if variant != "One" || !matches!(child.as_ref(), ValueKind::String("text")) {
        return Err(IoError::other("single enum payload changed value").into());
    }

    let enum_fields = structure_field(root, "enum_fields")
        .ok_or_else(|| IoError::other("enum structure field is missing"))?;
    let ValueKind::Enumeration {
        variant,
        payload: ValuePayloadView::Structure(fields),
    } = enum_fields.as_ref()
    else {
        return Err(IoError::other("enum structure payload changed kind").into());
    };
    if variant != "Many"
        || fields.get(0).and_then(|entry| entry.name()) != Some("left")
        || fields.get(1).and_then(|entry| entry.name()) != Some("right")
        || enum_fields.source() != ValueSource::ConfigLiteral
    {
        return Err(IoError::other("enum structure names, order, or source changed").into());
    }

    let none = structure_field(root, "none")
        .ok_or_else(|| IoError::other("optional none field is missing"))?;
    let some = structure_field(root, "some")
        .ok_or_else(|| IoError::other("optional some field is missing"))?;
    if !matches!(none.as_ref(), ValueKind::Optional(None))
        || none.source() != ValueSource::EnvironmentVariable
        || !matches!(some.as_ref(), ValueKind::Optional(Some(_)))
        || some.source() != ValueSource::SchemaDefault
    {
        return Err(IoError::other("optional values or source tags changed").into());
    }
    Ok(())
}

#[test]
fn deep_view_copy_succeeds_on_a_small_thread_stack() -> Result<(), Box<dyn Error>> {
    let depth = 4096;
    let source = optional_chain(depth)?;
    let copied = thread::Builder::new()
        .stack_size(32 * 1024)
        .spawn(move || -> Result<ConfigValue, ValueError> {
            let mut builder = TestOpeo::call(|out| ConfigValueBuilder::new(None, out))?;
            let root = TestOpeo::call(|out| {
                builder.append_view(source.root(), ValueCopyLimits::new(depth), out)
            })?;
            TestOpeo::call(|out| builder.finish(root, out))
        })?
        .join()
        .map_err(|_| IoError::other("small-stack value copy panicked"))??;
    if copied.node_count() != depth + 1 {
        return Err(IoError::other("deep copy returned the wrong node count").into());
    }
    Ok(())
}

#[test]
fn workspace_and_step_budgets_fail_as_values_errors() -> Result<(), Box<dyn Error>> {
    let source = optional_chain(128)?;
    let workspace_budget =
        TestOpeo::call(|out| ExecutionBudget::new(ExecutionLimits::new(1_000_000, 512), out))?;
    let mut builder = TestOpeo::call(|out| ConfigValueBuilder::new(Some(&workspace_budget), out))?;
    let error =
        TestOpeo::call(|out| builder.append_view(source.root(), ValueCopyLimits::new(128), out))
            .err()
            .ok_or_else(|| IoError::other("copy exceeded no workspace limit"))?;
    if error.kind() != ValueErrorKind::ResourceLimitExceeded {
        return Err(IoError::other("workspace error used the wrong category").into());
    }

    let steps_budget =
        TestOpeo::call(|out| ExecutionBudget::new(ExecutionLimits::new(8, 1_048_576), out))?;
    let mut builder = TestOpeo::call(|out| ConfigValueBuilder::new(Some(&steps_budget), out))?;
    let error =
        TestOpeo::call(|out| builder.append_view(source.root(), ValueCopyLimits::new(128), out))
            .err()
            .ok_or_else(|| IoError::other("copy exceeded no step limit"))?;
    if error.kind() != ValueErrorKind::ResourceLimitExceeded {
        return Err(IoError::other("step error used the wrong category").into());
    }
    Ok(())
}
