use std::thread;
use typol_config::{
    ConfigLimits, ConfigValidationOptions, ConfigValidationRequest, ConfigValueErrors,
    ConfigValueProcessor,
};
use typol_core::{AllocationError, ConfigValue, DiagnosticCode, ValueError};
use typol_policy::ResourceLimits;
use typol_schema::{SchemaCompileOptions, SchemaLimits, SchemaSource};
use typol_test_support::ConfigTestSupport as Support;
use typol_test_support::*;

struct ValueSlots {
    allocations: TestSlot<AllocationError>,
    values: TestSlot<ValueError>,
    validation: TestSlot<ConfigValueErrors>,
}

impl ValueSlots {
    fn new() -> Self {
        Self {
            allocations: TestSlot::default(),
            values: TestSlot::default(),
            validation: TestSlot::default(),
        }
    }
}

fn object(fields: Vec<(&str, ConfigValue)>, slots: &mut ValueSlots) -> TestResult<ConfigValue> {
    let mut values = Vec::new();
    for (name, value) in fields {
        let field = slots
            .allocations
            .access(|out| FieldValue::try_new_unmetered(name, value, out))
            .map_err(|caught| TestFailure::new(format!("failed to create field: {:?}", caught)))?;
        values.push(field);
    }
    slots
        .values
        .access(|out| ValueFactory::try_structure(values).map_err(|error| out.fail(error)))
        .map_err(|caught| {
            TestFailure::new(format!("failed to create structure value: {:?}", caught))
        })
}

fn nested_value(depth: usize, slots: &mut ValueSlots) -> TestResult<ConfigValue> {
    let mut value = object(vec![("n", ValueFactory::unsigned(1)?)], slots)?;
    for _ in 0..depth {
        value = object(vec![("child", value)], slots)?;
    }
    object(vec![("value", value)], slots)
}

#[test]
fn deep_dynamic_trees_use_an_explicit_stack_and_cleanup_on_failure() -> TestResult<()> {
    let _ = Support::parse;
    thread::Builder::new()
        .stack_size(128 * 1024)
        .spawn(|| -> TestResult<()> {
            let depth = 512;
            let mut slots = ValueSlots::new();
            let mut schema_text =
                String::from("struct Leaf { n: u8 } @assert(self.n > 0, \"leaf\")\n");
            let mut ty = String::from("Leaf");
            for index in 0..depth {
                let name = format!("Layer{index}");
                schema_text.push_str(&format!("struct {name} {{ child: {ty} }}\n"));
                ty = name;
            }
            schema_text.push_str(&format!("entry struct Root {{ value: {ty} }}"));

            let mut schema_limits = SchemaLimits::default();
            schema_limits.set_max_nesting_depth(depth + 4);
            let mut config_limits = ConfigLimits::default();
            config_limits = config_limits.with_max_nesting_depth(depth + 4);
            let schema = Support::compile_schema(
                SchemaCompileOptions::new(schema_limits),
                SchemaSource::from_str(b"root", &schema_text),
                None,
                None,
            )
            .test_value()?;
            let defaults = ConfigValidationOptions::default();
            let options = ConfigValidationOptions::new(
                ResourceLimits::default(),
                config_limits,
                *defaults.execution(),
            );
            let value = nested_value(depth, &mut slots)?;
            match slots.validation.access(|out| {
                ConfigValueProcessor::new(&schema).validate(
                    value,
                    ConfigValidationRequest::new(options),
                    out,
                )
            }) {
                Ok(value) => drop(value),
                Err(caught) => {
                    return Err(TestFailure::new(format!(
                        "deep valid-tree validation failed: {:?}",
                        caught
                    )));
                }
            }

            config_limits = config_limits.with_max_nesting_depth(depth);
            let options = ConfigValidationOptions::new(
                ResourceLimits::default(),
                config_limits,
                *defaults.execution(),
            );
            let value = nested_value(depth, &mut slots)?;
            let caught = match slots.validation.access(|out| {
                ConfigValueProcessor::new(&schema).validate(
                    value,
                    ConfigValidationRequest::new(options),
                    out,
                )
            }) {
                Ok(_) => {
                    return Err(TestFailure::new(
                        "depth limit must reject the configuration tree",
                    ));
                }
                Err(caught) => caught,
            };
            test_eq!(
                TestCheck::item(caught.diagnostics(), 0)?.code(),
                DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
            )?;
            Ok(())
        })
        .test_value()?
        .join()
        .map_err(|_| TestFailure::new("deep-validation thread terminated unexpectedly"))??;

    Ok(())
}
