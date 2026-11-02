#![no_std]

extern crate alloc;
extern crate std;

use alloc::{string::String, vec::Vec};
use typol::{
    Typol,
    binding::{BindOptions, BudgetedTypol, CodecErrors, TypedSchema},
    opeo::{OResult, Out},
    semantic::CompiledSchema,
};

#[cfg(not(feature = "std"))]
use typol::{
    ConfigInput, EmptyEnvironment, SchemaLoader, TypolError, TypolOptions, TypolStage,
    core::SourceId, opeo::ErrSlot,
};

#[cfg(all(feature = "config-ir", not(feature = "std")))]
use typol::{ConfigIr, ConfigIrOptions};

#[derive(Typol)]
struct Service {
    name: String,
    enabled: bool,
    ports: Vec<u16>,
}

#[derive(Typol)]
enum Mode {
    Local,
    Remote { endpoint: String },
}

fn bind_service<'slot, 'schema, 'borrow>(
    schema: &'schema CompiledSchema,
    out: Out<'slot, 'borrow, CodecErrors>,
) -> OResult<'slot, TypedSchema<'schema, Service>> {
    TypedSchema::bind(schema, BindOptions::default(), out)
}

fn assert_budgeted<T: BudgetedTypol>() {}

#[test]
fn derived_types_compile_without_the_standard_library() {
    let _: for<'schema, 'slot, 'borrow> fn(
        &'schema CompiledSchema,
        Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, TypedSchema<'schema, Service>> = bind_service;
    assert_budgeted::<Service>();
    assert_budgeted::<Mode>();

    let mode = Mode::Remote {
        endpoint: String::from("127.0.0.1"),
    };
    assert!(matches!(mode, Mode::Remote { endpoint } if endpoint == "127.0.0.1"));
    assert!(matches!(Mode::Local, Mode::Local));
}

#[test]
#[cfg(not(feature = "std"))]
fn in_memory_loading_and_binding_work_in_a_no_std_consumer() {
    let mut errors = ErrSlot::<TypolError>::new();
    let result = errors.try_call(|mut out| {
        let loaded = SchemaLoader::new(TypolOptions::default()).compile_text(
            b"no-std-test",
            "entry struct Service { name: string, enabled: bool, ports: [u16] }",
            out.reborrow(),
        )?;
        assert_eq!(
            loaded.source_names().name(SourceId::new(0)),
            Some("no-std-test")
        );
        assert_eq!(
            loaded.source_names().identity(SourceId::new(0)),
            Some(b"no-std-test".as_slice())
        );
        let binding = loaded.bind::<Service>(out.reborrow())?;
        let mut environment = EmptyEnvironment;
        let config = binding.read(
            ConfigInput::text("{name: \"api\", enabled: true, ports: [8080]}"),
            &mut environment,
            out,
        )?;
        assert_eq!(config.name, "api");
        assert!(config.enabled);
        assert_eq!(config.ports, [8080]);
        let mut parse_errors = ErrSlot::<TypolError>::new();
        let caught = parse_errors
            .try_call(|error_out| {
                loaded.read_value(ConfigInput::text("{name: }"), &mut environment, error_out)
            })
            .err()
            .expect("invalid input should fail");
        let parse_error = caught.take();
        assert_eq!(parse_error.stage(), TypolStage::Decoding);
        assert!(parse_error.config_errors().is_some());
        assert!(parse_error.schema_errors().is_none());
        Ok(())
    });
    assert!(result.is_ok());
}

#[test]
#[cfg(all(feature = "config-ir", not(feature = "std")))]
fn config_ir_serializes_in_memory_values_without_std() {
    let mut errors = ErrSlot::<TypolError>::new();
    let parsed = errors.try_call(|mut out| {
        let loaded = SchemaLoader::new(TypolOptions::default()).compile_text(
            b"no-std-config-ir",
            "entry struct Service { name: string, enabled: bool, ports: [u16] }",
            out.reborrow(),
        )?;
        let mut environment = EmptyEnvironment;
        let value = loaded
            .read_value(
                ConfigInput::text("{name: \"api\", enabled: true, ports: [8080]}"),
                &mut environment,
                out.reborrow(),
            )?
            .into_config_value();
        Ok((loaded, value))
    });
    let (loaded, value) = parsed.expect("in-memory schema and value should load");
    let bytes = ConfigIr::serialize(loaded.schema(), value, ConfigIrOptions::default())
        .expect("Config IR should serialize without std");
    assert!(bytes.ends_with(b"\n"));
    let json = core::str::from_utf8(&bytes).expect("Config IR should be UTF-8 JSON");
    assert!(json.contains("\"format\":\"typol-config-ir\""));
}
