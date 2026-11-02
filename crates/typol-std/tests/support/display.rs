mod records;
mod values;

pub use records::{EnumPayload, FieldValue, MapEntry, NoImports};
pub use values::ConfigValue;

use std::{error::Error, io};

use opeo::{OResult, Out};
use typol_config::{
    ConfigParseErrors, ConfigParseOptions, ConfigParseRequest, ConfigParser, Environment,
    EnvironmentError, EnvironmentReadContext,
};
use typol_core::{
    ConfigValue as ArenaValue, ConfigValueBuilder, NamedValue, SliceSource, ValueError, ValueSource,
};
use typol_schema::{SchemaCompileErrors, SchemaCompileOptions, SchemaCompiler, SchemaSource};
use typol_semantic::CompiledSchema;
use typol_std::{DisplayError, DisplayOptions, SafeValueFormatter, opeo::ErrSlot};

pub struct Env;

impl Environment for Env {
    fn read<'env, 'slot, 'borrow>(
        &'env mut self,
        name: &str,
        _: EnvironmentReadContext<'_>,
        _: Out<'slot, 'borrow, EnvironmentError>,
    ) -> OResult<'slot, Option<&'env [u8]>> {
        Ok((name == "X").then_some(b"PRIVATE-ENV".as_slice()))
    }
}

fn build_value<'slot>(
    value: ConfigValue,
    root_field: Option<&str>,
    mut out: Out<'slot, '_, ValueError>,
) -> OResult<'slot, ArenaValue> {
    let mut builder = ConfigValueBuilder::new(None, out.reborrow())?;
    let value = value.append_to(&mut builder, out.reborrow())?;
    let root = match root_field {
        Some(name) => builder.add_structure(
            &[NamedValue::new(name, value)],
            ValueSource::RustValue,
            out.reborrow(),
        )?,
        None => value,
    };
    builder.finish(root, out)
}

pub struct DisplayTest;

impl DisplayTest {
    pub fn compile(text: &str) -> Result<CompiledSchema, Box<dyn Error>> {
        Self::compile_with_options(text, SchemaCompileOptions::default())
    }

    pub fn compile_with_options(
        text: &str,
        options: SchemaCompileOptions,
    ) -> Result<CompiledSchema, Box<dyn Error>> {
        let compiler = ErrSlot::<SchemaCompileErrors>::new()
            .call(|out| SchemaCompiler::new(options, out))
            .map_err(|error| {
                io::Error::other(format!("Schema initialization failed: {error:?}"))
            })?;
        ErrSlot::<SchemaCompileErrors>::new()
            .call(|out| {
                compiler.compile(
                    SchemaSource::from_str(b"root", text),
                    &mut NoImports,
                    None,
                    out,
                )
            })
            .map_err(|error| {
                io::Error::other(format!("Schema compilation failed: {error:?}")).into()
            })
    }

    pub fn parse(schema: &CompiledSchema, text: &str) -> Result<ArenaValue, Box<dyn Error>> {
        Self::parse_with_environment(schema, text, &mut Env)
    }

    pub fn parse_with_environment(
        schema: &CompiledSchema,
        text: &str,
        environment: &mut dyn Environment,
    ) -> Result<ArenaValue, Box<dyn Error>> {
        ErrSlot::<ConfigParseErrors>::new()
            .call(|out| {
                ConfigParser::new(schema).parse(
                    ConfigParseRequest::new(
                        &mut SliceSource::new(text.as_bytes()),
                        "config",
                        ConfigParseOptions::default(),
                    ),
                    environment,
                    out,
                )
            })
            .map(|value| value.into_config_value())
            .map_err(|error| {
                io::Error::other(format!("configuration parsing failed: {error:?}")).into()
            })
    }

    pub fn build(value: ConfigValue) -> Result<ArenaValue, Box<dyn Error>> {
        ErrSlot::<ValueError>::new()
            .call(|out| build_value(value, None, out))
            .map_err(|error| {
                io::Error::other(format!("value construction failed: {error:?}")).into()
            })
    }

    pub fn root(value: ConfigValue) -> Result<ArenaValue, Box<dyn Error>> {
        ErrSlot::<ValueError>::new()
            .call(|out| build_value(value, Some("value"), out))
            .map_err(|error| {
                io::Error::other(format!("root value construction failed: {error:?}")).into()
            })
    }

    pub fn format(schema: &CompiledSchema, value: &ArenaValue) -> Result<String, Box<dyn Error>> {
        let result = Self::format_with_options(schema, value, DisplayOptions::default());
        result
            .map_err(|error| io::Error::other(format!("value formatting failed: {error:?}")).into())
    }

    pub fn format_with_options(
        schema: &CompiledSchema,
        value: &ArenaValue,
        options: DisplayOptions,
    ) -> Result<String, DisplayError> {
        ErrSlot::<DisplayError>::new()
            .call(|out| SafeValueFormatter::format(schema, value, options, out))
    }
}
