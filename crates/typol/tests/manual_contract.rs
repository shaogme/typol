use std::{any::TypeId, collections::HashMap, error::Error};
use typol::{
    ConfigInput, ConfigParseOptions, EmptyEnvironment, SchemaLoader, Typol, TypolOptions,
    TypolStage, WriteOptions,
    binding::{
        BindOptions, CodecErrors, CodecOptions, CodecStage, ConversionContext, ConversionLimits,
        ConversionValue, DescribeContext, DescriptorNode, DescriptorProvider, TypeDescriptor,
        TypeShape, TypedSchema, Typol as TypolTrait,
    },
    config::ConfigParseRequest,
    core::{DiagnosticCode, SliceSource, ValueId, ValueSource},
    opeo::{OResult, Out},
    schema::{
        ResolverError, SchemaCompileOptions, SchemaCompiler, SchemaResolveRequest, SchemaResolver,
        SchemaSource,
    },
    semantic::CompiledSchema,
};
use typol_test_support::{
    TestExpect, TestExtract, TestOpeo, TestResult, test_verify as assert,
    test_verify_eq as assert_eq,
};

struct Resolver;

impl SchemaResolver for Resolver {
    fn resolve<'slot, 'borrow, 'src>(
        &'src mut self,
        _request: SchemaResolveRequest<'_>,
        out: Out<'slot, 'borrow, ResolverError>,
    ) -> OResult<'slot, SchemaSource<'src>> {
        Err(out.fail(ResolverError::NotFound))
    }
}

fn compile(text: &str) -> TestResult<CompiledSchema> {
    let compiler =
        TestOpeo::value(|out| SchemaCompiler::new(SchemaCompileOptions::default(), out))?;
    TestOpeo::value(|out| {
        compiler.compile(
            SchemaSource::from_str(b"manual", text),
            &mut Resolver,
            None,
            out,
        )
    })
}

struct Manual {
    value: u8,
}

impl TypolTrait for Manual {
    fn describe<'slot>(
        context: &mut DescribeContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, TypeDescriptor> {
        let name = context.name("value", out.reborrow())?;
        let field = context.insert(
            DescriptorNode::Field {
                name,
                value: DescriptorProvider::of::<u8>(),
            },
            context.depth(),
            out.reborrow(),
        )?;
        let members = context.members(&[field], out.reborrow())?;
        let name = context.name("Root", out.reborrow())?;
        context.insert(
            DescriptorNode::Type {
                identity: TypeId::of::<Self>(),
                name: Some(name),
                shape: TypeShape::Structure(members),
            },
            context.depth(),
            out.reborrow(),
        )
    }

    fn to_config_value<'slot>(
        &self,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        context.start::<Self>(out.reborrow())?;
        context.structure(
            |slot, child, child_out| match slot {
                0 => child.unsigned(self.value.into(), child_out),
                _ => child.reject_with_path(child_out),
            },
            out.reborrow(),
        )
    }

    fn from_config_value<'slot, 'borrow>(
        value: ConversionValue<'_>,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, Self> {
        context.start::<Self>(out.reborrow())?;
        let fields = context.read_structure(value, out.reborrow())?;
        let value = context.read_field(fields, 0, out.reborrow())?;
        Ok(Self { value })
    }
}

#[derive(Typol)]
#[typol(rename = "Outer", trusted)]
struct DerivedWithManual {
    child: Manual,
}

struct ManualText {
    value: String,
}

impl TypolTrait for ManualText {
    fn describe<'slot>(
        context: &mut DescribeContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, TypeDescriptor> {
        let name = context.name("value", out.reborrow())?;
        let field = context.insert(
            DescriptorNode::Field {
                name,
                value: DescriptorProvider::of::<String>(),
            },
            context.depth(),
            out.reborrow(),
        )?;
        let members = context.members(&[field], out.reborrow())?;
        let name = context.name("Root", out.reborrow())?;
        context.insert(
            DescriptorNode::Type {
                identity: TypeId::of::<Self>(),
                name: Some(name),
                shape: TypeShape::Structure(members),
            },
            context.depth(),
            out,
        )
    }

    fn to_config_value<'slot>(
        &self,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        context.start::<Self>(out.reborrow())?;
        context.structure(
            |slot, child, child_out| match slot {
                0 => child.encode_here(&self.value, child_out),
                _ => child.reject_with_path(child_out),
            },
            out.reborrow(),
        )
    }

    fn from_config_value<'slot>(
        value: ConversionValue<'_>,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, Self> {
        context.start::<Self>(out.reborrow())?;
        let fields = context.read_structure(value, out.reborrow())?;
        // Reuse the built-in `String` conversion to charge target text through `target_text`.
        // Reuse the built-in String conversion to charge target text through target_text.
        let value = context.read_field::<String>(fields, 0, out.reborrow())?;
        Ok(Self { value })
    }
}

struct NodeForge {
    value: u8,
}

#[derive(Debug, PartialEq, Typol)]
enum BudgetChoice {
    Unit,
    Value(u8),
}

#[derive(Debug, PartialEq, Typol)]
#[typol(rename = "Root")]
struct DerivedBudget {
    values: Vec<Option<u8>>,
    choice: BudgetChoice,
    entries: HashMap<String, u8>,
}

impl TypolTrait for NodeForge {
    fn describe<'slot>(
        context: &mut DescribeContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, TypeDescriptor> {
        let name = context.name("value", out.reborrow())?;
        let field = context.insert(
            DescriptorNode::Field {
                name,
                value: DescriptorProvider::of::<u8>(),
            },
            context.depth(),
            out.reborrow(),
        )?;
        let members = context.members(&[field], out.reborrow())?;
        let name = context.name("Root", out.reborrow())?;
        context.insert(
            DescriptorNode::Type {
                identity: TypeId::of::<Self>(),
                name: Some(name),
                shape: TypeShape::Structure(members),
            },
            context.depth(),
            out,
        )
    }

    fn to_config_value<'slot>(
        &self,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        context.start::<Self>(out.reborrow())?;
        let _discarded = context.unsigned(1, out.reborrow())?;
        let value = context.unsigned(self.value.into(), out.reborrow())?;
        context.structure(
            |slot, _, child_out| match slot {
                0 => Ok(value),
                _ => ConversionContext::reject(child_out),
            },
            out.reborrow(),
        )
    }

    fn from_config_value<'slot>(
        value: ConversionValue<'_>,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, Self> {
        context.start::<Self>(out.reborrow())?;
        let fields = context.read_structure(value, out.reborrow())?;
        let value = context.read_field(fields, 0, out.reborrow())?;
        context.read_field::<u8>(fields, 0, out.reborrow())?;
        Ok(Self { value })
    }
}

#[test]
fn handwritten_conversion_cannot_forge_validation_or_default_provenance() -> TestResult {
    let schema = compile("entry struct Root { value: u8 @range(1, 3) }")?;
    let binding = TestOpeo::value(|out| {
        TypedSchema::<Manual>::bind_trusted(&schema, BindOptions::default(), out)
    })?;
    assert_eq!(
        TestOpeo::value(|out| {
            binding.serialize(&Manual { value: 2 }, CodecOptions::default(), out)
        })?,
        "{value: 2}"
    );
    let errors = TestOpeo::error(|out| {
        binding.serialize(&Manual { value: 9 }, CodecOptions::default(), out)
    })?;
    assert_eq!(errors.stage(), CodecStage::Validation);
    assert_eq!(
        errors
            .diagnostics()
            .first()
            .test_expect("validation diagnostics should be preserved")?
            .code(),
        DiagnosticCode::CONFIG_VALIDATION_ERROR
    );
    assert_eq!(
        errors
            .diagnostics()
            .first()
            .test_expect("validation diagnostics should be preserved")?
            .value_source(),
        Some(ValueSource::RustValue)
    );
    assert_eq!(
        errors
            .diagnostics()
            .first()
            .test_expect("validation diagnostics should be preserved")?
            .primary(),
        None
    );
    Ok(())
}

#[test]
fn facade_trusted_binding_reads_and_writes_handwritten_types() -> TestResult {
    let loaded = TestOpeo::value(|out| {
        SchemaLoader::new(TypolOptions::default()).compile_text(
            b"manual",
            "entry struct Root { value: u8 }",
            out,
        )
    })?;
    let binding = TestOpeo::value(|out| loaded.bind_trusted::<Manual>(out))?;
    assert_eq!(
        TestOpeo::value(|out| {
            binding.write(&Manual { value: 5 }, WriteOptions::default(), out)
        })?,
        "{value: 5}"
    );
    let mut environment = EmptyEnvironment;
    let value = TestOpeo::value(|out| {
        binding.read(ConfigInput::text("{value: 5}"), &mut environment, out)
    })?;
    assert_eq!(value.value, 5);

    let mismatched = TestOpeo::value(|out| {
        SchemaLoader::new(TypolOptions::default()).compile_text(
            b"mismatched-manual",
            "entry struct Root { other: u8 }",
            out,
        )
    })?;
    let errors = TestOpeo::error(|out| mismatched.bind_trusted::<Manual>(out))?;
    assert_eq!(errors.stage(), TypolStage::Binding);
    assert!(errors.codec_errors().is_some());
    assert!(
        errors
            .source()
            .and_then(|source| source.downcast_ref::<CodecErrors>())
            .is_some()
    );
    Ok(())
}

#[test]
fn derived_parent_with_handwritten_child_keeps_trusted_path() -> TestResult {
    let schema = compile("struct Root { value: u8 } entry struct Outer { child: Root }")?;
    let binding = TestOpeo::value(|out| {
        TypedSchema::<DerivedWithManual>::bind_trusted(&schema, BindOptions::default(), out)
    })?;
    let value = DerivedWithManual {
        child: Manual { value: 17 },
    };
    let encoded = TestOpeo::value(|out| binding.serialize(&value, CodecOptions::default(), out))?;
    assert_eq!(encoded, "{child: {value: 17}}");
    let mut source = SliceSource::new(encoded.as_bytes());
    let request =
        ConfigParseRequest::new(&mut source, "nested-manual", ConfigParseOptions::default());
    let decoded = TestOpeo::value(|out| {
        binding.deserialize(
            request,
            &mut EmptyEnvironment,
            ConversionLimits::default(),
            out,
        )
    })?;
    assert_eq!(decoded.child.value, 17);
    Ok(())
}

#[test]
fn handwritten_structs_can_reuse_budgeted_builtin_text_conversion() -> TestResult {
    let schema = compile("entry struct Root { value: string }")?;
    let binding = TestOpeo::value(|out| {
        TypedSchema::<ManualText>::bind_trusted(&schema, BindOptions::default(), out)
    })?;
    let mut source = SliceSource::new(b"{value: \"budgeted\"}");
    let request = ConfigParseRequest::new(&mut source, "manual", ConfigParseOptions::default());
    let value = TestOpeo::value(|out| {
        binding.deserialize(
            request,
            &mut EmptyEnvironment,
            ConversionLimits::default(),
            out,
        )
    })?;
    assert_eq!(value.value, "budgeted");
    Ok(())
}

#[test]
fn handwritten_conversion_nodes_are_counted_without_manual_accounting() -> TestResult {
    let schema = compile("entry struct Root { value: u8 }")?;
    let binding = TestOpeo::value(|out| {
        TypedSchema::<Manual>::bind_trusted(&schema, BindOptions::default(), out)
    })?;
    let limits = ConversionLimits::new(2, 128).test_value()?;
    let options = CodecOptions::default().with_conversion(limits);
    assert_eq!(
        TestOpeo::value(|out| { binding.serialize(&Manual { value: 7 }, options, out) })?,
        "{value: 7}"
    );
    let mut source = SliceSource::new(b"{value: 7}");
    let request = ConfigParseRequest::new(&mut source, "manual", ConfigParseOptions::default());
    assert_eq!(
        TestOpeo::value(|out| {
            binding.deserialize(request, &mut EmptyEnvironment, limits, out)
        })?
        .value,
        7
    );

    let zero_limits = ConversionLimits::new(0, 128).test_value()?;
    let zero_options = CodecOptions::default().with_conversion(zero_limits);
    let errors = TestOpeo::error(|out| binding.serialize(&Manual { value: 7 }, zero_options, out))?;
    assert_eq!(errors.stage(), CodecStage::Conversion);
    let mut source = SliceSource::new(b"{value: 7}");
    let request = ConfigParseRequest::new(&mut source, "manual", ConfigParseOptions::default());
    let errors = TestOpeo::error(|out| {
        binding.deserialize(request, &mut EmptyEnvironment, zero_limits, out)
    })?;
    assert_eq!(errors.stage(), CodecStage::Conversion);

    let schema = compile("entry struct Root { value: u8 }")?;
    let binding = TestOpeo::value(|out| {
        TypedSchema::<NodeForge>::bind_trusted(&schema, BindOptions::default(), out)
    })?;
    let options = CodecOptions::default().with_conversion(limits);
    let errors = TestOpeo::error(|out| binding.serialize(&NodeForge { value: 7 }, options, out))?;
    assert_eq!(errors.stage(), CodecStage::Conversion);

    let mut source = SliceSource::new(b"{value: 7}");
    let request = ConfigParseRequest::new(&mut source, "manual", ConfigParseOptions::default());
    let errors =
        TestOpeo::error(|out| binding.deserialize(request, &mut EmptyEnvironment, limits, out))?;
    assert_eq!(errors.stage(), CodecStage::Conversion);
    Ok(())
}

#[test]
fn derive_collections_enum_payloads_and_map_keys_share_exact_node_limits() -> TestResult {
    let schema = compile(
        "enum BudgetChoice { Unit, Value(u8) } entry struct Root { values: [option<u8>], choice: BudgetChoice, entries: map<string, u8> }",
    )?;
    let binding = TestOpeo::value(|out| {
        TypedSchema::<DerivedBudget>::bind(&schema, BindOptions::default(), out)
    })?;
    let value = DerivedBudget {
        values: vec![Some(4), None],
        choice: BudgetChoice::Value(5),
        entries: HashMap::from([("key".to_owned(), 6)]),
    };
    let exact_limits = ConversionLimits::new(10, 128).test_value()?;
    let exact_options = CodecOptions::default().with_conversion(exact_limits);
    let encoded = TestOpeo::value(|out| binding.serialize(&value, exact_options, out))?;
    let mut source = SliceSource::new(encoded.as_bytes());
    let request = ConfigParseRequest::new(&mut source, "derived", ConfigParseOptions::default());
    assert_eq!(
        TestOpeo::value(|out| {
            binding.deserialize(request, &mut EmptyEnvironment, exact_limits, out)
        })?,
        value
    );

    let below_limits = ConversionLimits::new(9, 128).test_value()?;
    let below_options = CodecOptions::default().with_conversion(below_limits);
    let errors = TestOpeo::error(|out| binding.serialize(&value, below_options, out))?;
    assert_eq!(errors.stage(), CodecStage::Conversion);
    let mut source = SliceSource::new(encoded.as_bytes());
    let request = ConfigParseRequest::new(&mut source, "derived", ConfigParseOptions::default());
    let errors = TestOpeo::error(|out| {
        binding.deserialize(request, &mut EmptyEnvironment, below_limits, out)
    })?;
    assert_eq!(errors.stage(), CodecStage::Conversion);
    Ok(())
}
