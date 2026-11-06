#![no_std]

extern crate alloc;
extern crate std;

#[cfg(feature = "btree-map")]
use alloc::collections::BTreeMap;

use alloc::{string::String, vec::Vec};
use typol::{
    Typol,
    binding::{BindOptions, BudgetedTypol, CodecErrors, TypedSchema},
    opeo::{OResult, Out},
    semantic::CompiledSchema,
};

#[cfg(any(not(feature = "std"), feature = "btree-map"))]
use typol::{
    EmptyEnvironment, SchemaLoader, TypolError, TypolOptions, WriteOptions, io::ReadRequest,
    opeo::ErrSlot,
};

#[cfg(not(feature = "std"))]
use typol::{
    OrderedMap, SourceExpectation, SourceRevision, SourceWriter, TypolStage, core::SourceId,
};

#[cfg(all(feature = "hashbrown", not(feature = "std")))]
use hashbrown::HashMap;

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

#[cfg(not(feature = "std"))]
#[derive(Typol)]
struct OrderedSettings {
    values: OrderedMap<u8>,
}

#[cfg(not(feature = "std"))]
#[derive(Typol)]
struct RecursiveOrdered {
    children: OrderedMap<RecursiveOrdered>,
}

#[cfg(feature = "btree-map")]
#[derive(Typol)]
struct BTreeSettings {
    values: BTreeMap<String, u8>,
}

#[cfg(feature = "btree-map")]
#[derive(Typol)]
struct RecursiveBTree {
    children: BTreeMap<String, RecursiveBTree>,
}

#[cfg(all(feature = "hashbrown", not(feature = "std")))]
#[derive(Typol)]
struct HashbrownSettings {
    values: HashMap<String, u8>,
}

#[cfg(all(feature = "hashbrown", not(feature = "std")))]
#[derive(Typol)]
struct RecursiveHashbrown {
    children: HashMap<String, RecursiveHashbrown>,
}

fn bind_service<'slot, 'schema, 'borrow>(
    schema: &'schema CompiledSchema,
    out: Out<'slot, 'borrow, CodecErrors>,
) -> OResult<'slot, TypedSchema<'schema, Service>> {
    TypedSchema::bind(schema, BindOptions::default(), out)
}

#[cfg(not(feature = "std"))]
struct NoStdConfigWriter {
    contents: Option<Vec<u8>>,
}

#[cfg(not(feature = "std"))]
impl SourceWriter for NoStdConfigWriter {
    type Target = str;
    type Error = ();

    fn replace_atomically(
        &mut self,
        _: &Self::Target,
        expectation: SourceExpectation<'_>,
        replacement: &[u8],
        max_bytes: usize,
    ) -> Result<SourceRevision, Self::Error> {
        if replacement.len() > max_bytes {
            return Err(());
        }
        match expectation {
            SourceExpectation::ExactSnapshot(expected)
                if self.contents.as_deref() != Some(expected) =>
            {
                return Err(());
            }
            SourceExpectation::MustBeAbsent if self.contents.is_some() => return Err(()),
            SourceExpectation::DocumentSnapshot => return Err(()),
            SourceExpectation::ExactSnapshot(_)
            | SourceExpectation::MustBeAbsent
            | SourceExpectation::ReplaceAny => {}
        }
        self.contents = Some(replacement.to_vec());
        Ok(SourceRevision::new(1))
    }
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

#[cfg(not(feature = "std"))]
#[test]
fn ordered_map_decodes_and_writes_keys_in_order() {
    let mut values = OrderedMap::new();
    assert!(values.try_insert(String::from("z"), 9).is_ok());
    assert!(values.try_insert(String::from("a"), 1).is_ok());
    assert!(matches!(
        values.try_insert(String::from("a"), 2),
        Ok(Some(1))
    ));
    assert_eq!(values.len(), 2);
    assert_eq!(values.get("a"), Some(&2));
    assert_eq!(
        values.iter().map(|(key, _)| key).collect::<Vec<_>>(),
        ["a", "z"]
    );

    let mut errors = ErrSlot::<TypolError>::new();
    let result = errors.try_call(|mut out| {
        let loaded = SchemaLoader::new(TypolOptions::default()).compile_text(
            b"ordered-map-test",
            "entry struct OrderedSettings { values: map<string, u8> }",
            out.reborrow(),
        )?;
        let binding = loaded.bind::<OrderedSettings>(out.reborrow())?;
        let mut environment = EmptyEnvironment;
        let config = binding.read(
            ReadRequest::from_text("{values: {\"z\": 9, \"m\": 5, \"a\": 1}}"),
            &mut environment,
            out.reborrow(),
        )?;
        assert_eq!(
            config.values.iter().map(|(key, _)| key).collect::<Vec<_>>(),
            ["a", "m", "z"]
        );
        let written = binding.write_to_string(&config, WriteOptions::default(), out.reborrow())?;
        assert_eq!(written, "{values: {\"a\": 1, \"m\": 5, \"z\": 9}}");
        let recursive = SchemaLoader::new(TypolOptions::default()).compile_text(
            b"recursive-ordered-map-test",
            "entry struct RecursiveOrdered { children: map<string, RecursiveOrdered> }",
            out.reborrow(),
        )?;
        let _recursive_binding = recursive.bind::<RecursiveOrdered>(out)?;
        Ok(())
    });
    let error = result.err().map(|caught| caught.take());
    let code = error
        .as_ref()
        .and_then(TypolError::codec_errors)
        .and_then(|errors| errors.diagnostics().last())
        .map(|diagnostic| diagnostic.code().get());
    assert!(
        error.is_none(),
        "ordered map failed with {:?} and diagnostic {code:?}",
        error.as_ref().map(TypolError::stage)
    );
}

#[cfg(feature = "btree-map")]
#[test]
fn btree_map_binds_reads_and_writes_in_key_order() {
    assert_budgeted::<BTreeMap<String, u8>>();
    assert_budgeted::<BTreeSettings>();

    let mut errors = ErrSlot::<TypolError>::new();
    let result = errors.try_call(|mut out| {
        let loaded = SchemaLoader::new(TypolOptions::default()).compile_text(
            b"btree-map-test",
            "entry struct BTreeSettings { values: map<string, u8> }",
            out.reborrow(),
        )?;
        let binding = loaded.bind::<BTreeSettings>(out.reborrow())?;
        let mut environment = EmptyEnvironment;
        let config = binding.read(
            ReadRequest::from_text("{values: {\"z\": 9, \"a\": 1}}"),
            &mut environment,
            out.reborrow(),
        )?;
        assert_eq!(config.values.get("a"), Some(&1));
        let written = binding.write_to_string(&config, WriteOptions::default(), out.reborrow())?;
        assert_eq!(written, "{values: {\"a\": 1, \"z\": 9}}");

        let recursive = SchemaLoader::new(TypolOptions::default()).compile_text(
            b"recursive-btree-map-test",
            "entry struct RecursiveBTree { children: map<string, RecursiveBTree> }",
            out.reborrow(),
        )?;
        let _recursive_binding = recursive.bind::<RecursiveBTree>(out)?;
        Ok(())
    });
    assert!(result.is_ok());
}

#[cfg(all(feature = "hashbrown", not(feature = "std")))]
#[test]
fn hashbrown_map_binds_without_enabling_std() {
    let mut errors = ErrSlot::<TypolError>::new();
    let result = errors.try_call(|mut out| {
        let loaded = SchemaLoader::new(TypolOptions::default()).compile_text(
            b"hashbrown-map-test",
            "entry struct HashbrownSettings { values: map<string, u8> }",
            out.reborrow(),
        )?;
        let binding = loaded.bind::<HashbrownSettings>(out.reborrow())?;
        let mut environment = EmptyEnvironment;
        let config = binding.read(
            ReadRequest::from_text("{values: {\"z\": 9, \"a\": 1}}"),
            &mut environment,
            out.reborrow(),
        )?;
        assert_eq!(config.values.get("a"), Some(&1));
        let written = binding.write_to_string(&config, WriteOptions::default(), out.reborrow())?;
        assert_eq!(written, "{values: {\"a\": 1, \"z\": 9}}");
        let mut alternate_values = HashMap::new();
        alternate_values.insert(String::from("a"), 1);
        alternate_values.insert(String::from("z"), 9);
        let alternate = HashbrownSettings {
            values: alternate_values,
        };
        let alternate_written =
            binding.write_to_string(&alternate, WriteOptions::default(), out.reborrow())?;
        assert_eq!(written, alternate_written);
        let recursive = SchemaLoader::new(TypolOptions::default()).compile_text(
            b"recursive-hashbrown-map-test",
            "entry struct RecursiveHashbrown { children: map<string, RecursiveHashbrown> }",
            out.reborrow(),
        )?;
        let _recursive_binding = recursive.bind::<RecursiveHashbrown>(out)?;
        Ok(())
    });
    assert!(result.is_ok());
}

#[cfg(all(feature = "hashbrown", feature = "serde", not(feature = "std")))]
#[test]
fn hashbrown_map_uses_its_serde_implementation_when_enabled() {
    fn assert_serde<T: serde::Serialize + serde::de::DeserializeOwned>() {}

    assert_serde::<HashMap<String, u8>>();
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
            ReadRequest::from_text("{name: \"api\", enabled: true, ports: [8080]}"),
            &mut environment,
            out,
        )?;
        assert_eq!(config.name, "api");
        assert!(config.enabled);
        assert_eq!(config.ports, [8080]);
        let mut parse_errors = ErrSlot::<TypolError>::new();
        let caught = parse_errors
            .try_call(|error_out| {
                loaded.read_value(
                    ReadRequest::from_text("{name: }"),
                    &mut environment,
                    error_out,
                )
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

#[cfg(not(feature = "std"))]
#[test]
fn no_std_consumer_writes_config_documents_through_its_backend() {
    let mut errors = ErrSlot::<TypolError>::new();
    let result = errors.try_call(|mut out| {
        let loaded = SchemaLoader::new(TypolOptions::default()).compile_text(
            b"no-std-write-test",
            "entry struct Settings { enabled: bool }",
            out.reborrow(),
        )?;
        let mut environment = EmptyEnvironment;
        let source = b"{enabled: true}";
        let mut document = loaded.read_document(
            ReadRequest::from_text("{enabled: true}"),
            &mut environment,
            out.reborrow(),
        )?;
        let mut writer = NoStdConfigWriter {
            contents: Some(source.to_vec()),
        };
        let expectation = document.source_expectation();
        let revision = document.write_source("memory", &mut writer, expectation);
        assert!(revision.is_ok());
        assert_eq!(writer.contents.as_deref(), Some(source.as_slice()));
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
                ReadRequest::from_text("{name: \"api\", enabled: true, ports: [8080]}"),
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
