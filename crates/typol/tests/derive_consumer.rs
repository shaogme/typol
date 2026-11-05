use std::{
    env::{temp_dir, var_os},
    fs::{copy, create_dir_all, remove_dir_all, write},
    path::PathBuf,
    process::{Command, Output, id},
};
use typol_test_support::{TestExtract, TestResult, test_verify as assert};

struct Consumer {
    root: PathBuf,
}

impl Consumer {
    fn new() -> TestResult<Self> {
        let root = temp_dir().join(format!("typol-derive-consumer-{}", id()));
        create_dir_all(root.join("src")).test_value()?;
        Ok(Self { root })
    }

    fn compile(&self, source: &str, renamed: bool, run: bool) -> TestResult<Output> {
        let facade = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let dependency = if renamed { "renamed_typol" } else { "typol" };
        write(self.root.join("Cargo.toml"), format!(
            "[package]\nname = \"derive-consumer\"\nversion = \"0.0.0\"\nedition = \"2024\"\n[workspace]\n[dependencies]\n{dependency} = {{ package = \"typol\", path = {:?}, features = [\"std\"] }}\n", facade,
        )).test_value()?;
        copy(
            facade.join("../../Cargo.lock"),
            self.root.join("Cargo.lock"),
        )
        .test_value()?;
        write(self.root.join("src/main.rs"), source).test_value()?;
        Command::new(var_os("CARGO").unwrap_or_else(|| "cargo".into()))
            .arg(if run { "run" } else { "check" })
            .args(["--offline", "--message-format=json"])
            .current_dir(&self.root)
            .env("CARGO_TARGET_DIR", self.root.join("target"))
            .env("RUSTFLAGS", "-Dwarnings")
            .output()
            .test_value()
    }

    fn cleanup(self) -> TestResult {
        remove_dir_all(&self.root).test_value()
    }
}

#[test]
fn real_consumers_cover_renaming_generics_shapes_and_compile_diagnostics() -> TestResult {
    let consumer = Consumer::new()?;
    let source = r#"
use std::collections::HashMap;
use renamed_typol::{
    ConfigParseOptions, Environment, EnvironmentError, EnvironmentReadContext, Typol,
    binding::{BindOptions, WriteOptions, ConversionLimits, TypedSchema},
    config::ConfigParseRequest,
    core::SliceSource,
    opeo::{ErrSlot, OResult, Out},
    schema::{ResolverError, SchemaCompileOptions, SchemaCompiler, SchemaResolveRequest,
        SchemaResolver, SchemaSource},
};

fn invoke<E: std::fmt::Debug, T>(operation: impl for<'slot> FnOnce(Out<'slot, 'slot, E>) -> OResult<'slot, T>) -> T {
    let mut slot = ErrSlot::new();
    match slot.try_call(operation) {
        Ok(value) => value,
        Err(error) => panic!("OPEO call failed: {:?}", error.take()),
    }
}

fn invoke_error<E, T>(operation: impl for<'slot> FnOnce(Out<'slot, 'slot, E>) -> OResult<'slot, T>) -> E {
    let mut slot = ErrSlot::new();
    match slot.try_call(operation) {
        Ok(_) => panic!("expected the OPEO call to fail"),
        Err(error) => error.take(),
    }
}

mod facade { pub use renamed_typol::*; }

#[derive(Debug, PartialEq, Typol)]
#[typol(crate = "facade")]
struct Root<T, const N: usize> where T: PartialEq { value: [T; N] }

#[derive(Typol)]
#[typol(crate = "renamed_typol")]
struct Empty {}

#[derive(Debug, PartialEq, Typol)]
#[typol(crate = "renamed_typol", rename = "RenamedEmpty")]
struct UnitEmpty;

#[derive(Typol)]
#[typol(crate = "::renamed_typol")]
enum Choice { Unit, One(u8), Object { value: u8 }, Empty {} }

#[derive(Debug, PartialEq, Typol)]
#[typol(crate = "renamed_typol", rename = "Recursive")]
struct Recursive { children: Vec<Self> }

#[derive(Debug, PartialEq, Typol)]
#[typol(crate = "renamed_typol", rename = "Expr")]
enum RecursiveEnum { Number(i64), Add(Vec<Self>) }

#[derive(Debug, PartialEq, Typol)]
#[typol(crate = "renamed_typol")]
struct RecursiveRoot { node: Recursive, expression: RecursiveEnum }

#[derive(Debug, PartialEq, Typol)]
#[typol(crate = "renamed_typol")]
struct Left { right: Vec<Right> }

#[derive(Debug, PartialEq, Typol)]
#[typol(crate = "renamed_typol")]
struct Right { left: Vec<Left> }

#[derive(Debug, PartialEq, Typol)]
#[typol(crate = "renamed_typol")]
struct MutualRoot { value: Left }

#[derive(Debug, PartialEq, Typol)]
#[typol(crate = "renamed_typol")]
struct MapNode { children: HashMap<String, MapNode> }

#[derive(Debug, PartialEq, Typol)]
#[typol(crate = "renamed_typol")]
struct MapRoot { root: MapNode }

#[derive(Debug, PartialEq, Typol)]
#[typol(crate = "renamed_typol")]
struct Shared { children: Vec<Shared> }

#[derive(Debug, PartialEq, Typol)]
#[typol(crate = "renamed_typol")]
struct SharedRoot { value: Shared }

struct Resolver;
impl SchemaResolver for Resolver {
    fn resolve<'slot, 'borrow, 'src>(&'src mut self, _request: SchemaResolveRequest<'_>, out: Out<'slot, 'borrow, ResolverError>) -> OResult<'slot, SchemaSource<'src>> {
        Err(out.fail(ResolverError::NotFound))
    }
}
struct Provider;
impl Environment for Provider {
    fn read<'slot, 'borrow, 'env>(&'env mut self, _: &str,
        _: EnvironmentReadContext<'_>, _: Out<'slot, 'borrow, EnvironmentError>) -> OResult<'slot, Option<&'env [u8]>> { Ok(None) }
}
fn main() {
    let compiler = invoke(|out| SchemaCompiler::new(SchemaCompileOptions::default(), out));
    let schema = invoke(|out| compiler.compile(SchemaSource::from_str(b"root", "entry struct Root { value: [u8; 2] }"), &mut Resolver, None, out));
    let binding = invoke(|out| TypedSchema::<Root<u8, 2>>::bind(&schema, BindOptions::default(), out));
    let value = Root { value: [1, 2] };
    let text = invoke(|out| binding.serialize_to_string(&value, WriteOptions::default(), out));
    let mut input = SliceSource::new(text.as_bytes());
    let request = ConfigParseRequest::new(&mut input, "consumer", ConfigParseOptions::default());
    assert_eq!(invoke(|out| binding.deserialize(request, &mut Provider, ConversionLimits::default(), out)), value);
    let compiler = invoke(|out| SchemaCompiler::new(SchemaCompileOptions::default(), out));
    let mutual = invoke(|out| compiler.compile(SchemaSource::from_str(b"mutual", "struct Left { right: [Right] } struct Right { left: [Left] } entry struct MutualRoot { value: Left }"), &mut Resolver, None, out));
    let binding = invoke(|out| TypedSchema::<MutualRoot>::bind(&mutual, BindOptions::default(), out));
    let value = MutualRoot { value: Left { right: vec![Right { left: vec![Left { right: vec![] }] }] } };
    let text = invoke(|out| binding.serialize_to_string(&value, WriteOptions::default(), out));
    let mut input = SliceSource::new(text.as_bytes());
    let request = ConfigParseRequest::new(&mut input, "consumer", ConfigParseOptions::default());
    assert_eq!(invoke(|out| binding.deserialize(request, &mut Provider, ConversionLimits::default(), out)), value);
    let depth_limits = ConversionLimits::new(100, 0).expect("zero conversion depth is valid");
    let _depth_error = invoke_error(|out| binding.serialize_to_string(&value, WriteOptions::default().with_conversion(depth_limits), out));
    let node_limits = ConversionLimits::new(0, 128).expect("zero conversion nodes are valid");
    let _node_error = invoke_error(|out| binding.serialize_to_string(&value, WriteOptions::default().with_conversion(node_limits), out));
    let compiler = invoke(|out| SchemaCompiler::new(SchemaCompileOptions::default(), out));
    let map_schema = invoke(|out| compiler.compile(SchemaSource::from_str(b"recursive-map", "struct MapNode { children: map<string, MapNode> } entry struct MapRoot { root: MapNode }"), &mut Resolver, None, out));
    let binding = invoke(|out| TypedSchema::<MapRoot>::bind(&map_schema, BindOptions::default(), out));
    let value = MapRoot { root: MapNode { children: HashMap::from([("child".to_owned(), MapNode { children: HashMap::new() })]) } };
    let text = invoke(|out| binding.serialize_to_string(&value, WriteOptions::default(), out));
    let mut input = SliceSource::new(text.as_bytes());
    let request = ConfigParseRequest::new(&mut input, "consumer", ConfigParseOptions::default());
    assert_eq!(invoke(|out| binding.deserialize(request, &mut Provider, ConversionLimits::default(), out)), value);
    let compiler = invoke(|out| SchemaCompiler::new(SchemaCompileOptions::default(), out));
    let use_sites = invoke(|out| compiler.compile(SchemaSource::from_str(b"recursive-use-sites", "mod first { struct Shared { children: [second::Shared] } } mod second { struct Shared { children: [first::Shared] } } entry struct SharedRoot { value: first::Shared }"), &mut Resolver, None, out));
    let binding = invoke(|out| TypedSchema::<SharedRoot>::bind(&use_sites, BindOptions::default(), out));
    let value = SharedRoot { value: Shared { children: vec![Shared { children: vec![] }] } };
    let text = invoke(|out| binding.serialize_to_string(&value, WriteOptions::default(), out));
    let mut input = SliceSource::new(text.as_bytes());
    let request = ConfigParseRequest::new(&mut input, "consumer", ConfigParseOptions::default());
    assert_eq!(invoke(|out| binding.deserialize(request, &mut Provider, ConversionLimits::default(), out)), value);
    let errors = invoke_error(|out| TypedSchema::<Root<u16, 2>>::bind(&schema, BindOptions::default(), out));
    assert_eq!(
        errors.diagnostics().first().map(|diagnostic| diagnostic.code().get()),
        Some(14)
    );
    let compiler = invoke(|out| SchemaCompiler::new(SchemaCompileOptions::default(), out));
    let empty = invoke(|out| compiler.compile(SchemaSource::from_str(b"empty", "entry struct Empty {}"), &mut Resolver, None, out));
    let binding = invoke(|out| TypedSchema::<Empty>::bind(&empty, BindOptions::default(), out));
    assert_eq!(invoke(|out| binding.serialize_to_string(&Empty {}, WriteOptions::default(), out)), "{}");
    let compiler = invoke(|out| SchemaCompiler::new(SchemaCompileOptions::default(), out));
    let renamed_empty = invoke(|out| compiler.compile(SchemaSource::from_str(b"renamed-empty", "entry struct RenamedEmpty {}"), &mut Resolver, None, out));
    let binding = invoke(|out| TypedSchema::<UnitEmpty>::bind(&renamed_empty, BindOptions::default(), out));
    let unit = UnitEmpty;
    let text = invoke(|out| binding.serialize_to_string(&unit, WriteOptions::default(), out));
    assert_eq!(text, "{}");
    let mut input = SliceSource::new(text.as_bytes());
    let request = ConfigParseRequest::new(&mut input, "consumer", ConfigParseOptions::default());
    assert_eq!(invoke(|out| binding.deserialize(request, &mut Provider, ConversionLimits::default(), out)), unit);
    let compiler = invoke(|out| SchemaCompiler::new(SchemaCompileOptions::default(), out));
    let mismatched_empty = invoke(|out| compiler.compile(SchemaSource::from_str(b"mismatched-empty", "entry struct RenamedEmpty { value: u8 }"), &mut Resolver, None, out));
    let errors = invoke_error(|out| TypedSchema::<UnitEmpty>::bind(&mismatched_empty, BindOptions::default(), out));
    assert_eq!(errors.diagnostics().first().map(|diagnostic| diagnostic.code().get()), Some(14));
    let compiler = invoke(|out| SchemaCompiler::new(SchemaCompileOptions::default(), out));
    let recursive = invoke(|out| compiler.compile(SchemaSource::from_str(b"recursive", "enum Expr { Number(i64), Add([Expr]) } struct Recursive { children: [Recursive] } entry struct RecursiveRoot { node: Recursive, expression: Expr }"), &mut Resolver, None, out));
    let binding = invoke(|out| TypedSchema::<RecursiveRoot>::bind(&recursive, BindOptions::default(), out));
    let value = RecursiveRoot {
        node: Recursive { children: vec![Recursive { children: vec![] }] },
        expression: RecursiveEnum::Add(vec![RecursiveEnum::Number(42)]),
    };
    let text = invoke(|out| binding.serialize_to_string(&value, WriteOptions::default(), out));
    let mut input = SliceSource::new(text.as_bytes());
    let request = ConfigParseRequest::new(&mut input, "consumer", ConfigParseOptions::default());
    assert_eq!(invoke(|out| binding.deserialize(request, &mut Provider, ConversionLimits::default(), out)), value);
    let _ = Choice::Unit;
    let _ = Choice::One(1);
    let _ = Choice::Object { value: 1 };
    let _ = Choice::Empty {};
}
"#;
    let output = consumer.compile(source, true, true)?;
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let manual_template = r#"
use typol::{
    Typol,
            binding::{BindOptions, CodecErrors, ConversionContext, ConversionValue, DescribeContext,
            TypeDescriptor, Typol as TypolTrait, TypedSchema},
        core::ValueId,
        model::BuiltinType,
        semantic::CompiledSchema,
    opeo::{OResult, Out},
};

struct Manual;
impl TypolTrait for Manual {
    fn describe<'slot, 'borrow>(
        context: &mut DescribeContext<'_>,
        out: Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, TypeDescriptor> {
        context.scalar::<Self>(BuiltinType::Bool, context.depth(), out)
    }

    fn to_config_value<'slot, 'borrow>(
        &self,
        context: &mut ConversionContext<'_>,
        out: Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        context.reject_with_path(out)
    }

    fn from_config_value<'slot, 'borrow>(
        _value: ConversionValue<'_>,
        context: &mut ConversionContext<'_>,
        out: Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, Self> {
        context.reject_with_path(out)
    }
}

#[derive(Typol)]
#[typol(trusted)]
struct Parent { child: Manual }

fn check(schema: &CompiledSchema, out: Out<'_, '_, CodecErrors>) {
    let _binding = TypedSchema::<Parent>::BINDING_METHOD(
        schema,
        BindOptions::default(),
        out,
    );
}

fn main() { let _check = check; }
"#;
    let trusted_source = manual_template.replace("BINDING_METHOD", "bind_trusted");
    let trusted_output = consumer.compile(&trusted_source, false, false)?;
    assert!(
        trusted_output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&trusted_output.stdout),
        String::from_utf8_lossy(&trusted_output.stderr)
    );
    let regular_source = manual_template.replace("BINDING_METHOD", "bind");
    let regular_output = consumer.compile(&regular_source, false, false)?;
    assert!(!regular_output.status.success());
    let regular_diagnostics = format!(
        "{}\n{}",
        String::from_utf8_lossy(&regular_output.stdout),
        String::from_utf8_lossy(&regular_output.stderr)
    );
    assert!(
        regular_diagnostics.contains("BudgetedTypol"),
        "{regular_diagnostics}"
    );
    for declaration in [
        "union Root { value: u8 }",
        "struct Root(u8);",
        "enum Root { Value(u8, u8) }",
        "struct Root { value: &'static str }",
        "#[typol(skip)] struct Root {}",
        "struct Root { value: u8, #[typol(rename = \"value\")] other: u8 }",
        "struct Root { value: usize }",
    ] {
        let facade_crate = "typol";
        let source = format!(
            "use {facade_crate}::Typol;\n#[derive(Typol)]\n{declaration}\nfn main() {{}}\n"
        );
        let output = consumer.compile(&source, false, false)?;
        assert!(!output.status.success(), "should reject {declaration}");
        let diagnostics = String::from_utf8_lossy(&output.stdout);
        assert!(diagnostics.contains("\"level\":\"error\""), "{diagnostics}");
        assert!(diagnostics.contains("src/main.rs"), "{diagnostics}");
        assert!(diagnostics.contains("\"message\":"), "{diagnostics}");
    }
    consumer.cleanup()?;
    Ok(())
}
