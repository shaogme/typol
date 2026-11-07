use typol_test_support::ConfigTestSupport as Support;

use std::{
    env,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use typol_config::{
    ConfigLimits, ConfigParseOptions, ConfigParseRequest, Environment, EnvironmentError,
    EnvironmentReadContext,
    opeo::{OResult, Out},
};
use typol_core::{
    DiagnosticCode, ExecutionLimits, Read, SliceSource, ValueKind, ValuePayloadView, ValueView,
};
use typol_policy::{ExcerptLimits, RegexOptions};
use typol_schema::{SchemaCompileOptions, SchemaLimits, SchemaSource};
use typol_semantic::CompiledSchema;
use typol_test_support::*;

struct NoHost;

impl Environment for NoHost {
    fn read<'env, 'slot, 'borrow>(
        &'env mut self,
        _: &str,
        _: EnvironmentReadContext<'_>,
        out: Out<'slot, 'borrow, EnvironmentError>,
    ) -> OResult<'slot, Option<&'env [u8]>> {
        Err(out.fail(EnvironmentError::InternalError))
    }
}

fn compile(text: &str, limits: SchemaLimits) -> TestResult<CompiledSchema> {
    Support::compile_schema(
        SchemaCompileOptions::new(limits),
        SchemaSource::from_str(b"schema", text),
        None,
        None,
    )
    .test_value()
}

#[derive(Clone, Copy)]
enum Layer {
    Array,
    Map,
    Struct,
    Option,
    Single,
    Object,
}

struct Input {
    schema: String,
    prefix: String,
    suffix: String,
}

impl Input {
    fn new(layers: &[Layer], depth: usize) -> TestResult<Self> {
        let mut schema = String::from("struct Leaf { n: u8 } @assert(self.n > 0, \"leaf\")\n");
        let mut ty = String::from("Leaf");
        let mut opens = Vec::new();
        let mut closes = Vec::new();
        for index in 0..depth {
            let name = format!("N{index}");
            match *layers
                .get(index % layers.len())
                .test_expect("deep input requires at least one container layer")?
            {
                Layer::Array => {
                    ty = format!("[{ty}]");
                    opens.push("[");
                    closes.push("]");
                }
                Layer::Map => {
                    ty = format!("map<string, {ty}>");
                    opens.push("{\"key\":");
                    closes.push("}");
                }
                Layer::Struct => {
                    schema.push_str(&format!(
                        "struct {name} {{ child: {ty} }} @assert(true, \"branch\")\n"
                    ));
                    ty = name;
                    opens.push("{child:");
                    closes.push("}");
                }
                Layer::Option => {
                    ty = format!("option<[{ty}]>");
                    opens.push("[");
                    closes.push("]");
                }
                Layer::Single => {
                    schema.push_str(&format!("enum {name} {{ V({ty}) }}\n"));
                    ty = name;
                    opens.push(".V(");
                    closes.push(")");
                }
                Layer::Object => {
                    schema.push_str(&format!("enum {name} {{ V {{ child: {ty} }} }}\n"));
                    ty = name;
                    opens.push(".V{child:");
                    closes.push("}");
                }
            }
        }
        schema.push_str(&format!("entry struct Root {{ value: {ty}, tail: u8 }}"));
        Ok(Self {
            schema,
            prefix: opens.into_iter().rev().collect(),
            suffix: closes.into_iter().collect(),
        })
    }

    fn source(&self, leaf: &str, tail: &str) -> String {
        format!("{{value:{}{leaf}{},tail:{tail}}}", self.prefix, self.suffix)
    }
}

fn leaf_number(mut value: ValueView<'_>) -> TestResult<u128> {
    loop {
        match value.as_ref() {
            ValueKind::Structure(fields) => {
                let mut child = None;
                for field in fields {
                    match field.name().unwrap_or_default() {
                        "n" => {
                            let ValueKind::Unsigned(number) = field.value().as_ref() else {
                                return Err(TestFailure::new(
                                    "deep leaf value should be an unsigned integer",
                                ));
                            };
                            return Ok(number);
                        }
                        "tail" => {}
                        _ => child = Some(field.value()),
                    }
                }
                value = child.test_value()?;
            }
            ValueKind::Array(mut values) => value = values.next().test_value()?.value(),
            ValueKind::Map(mut entries) => value = entries.next().test_value()?.value(),
            ValueKind::Optional(Some(inner)) => value = inner,
            ValueKind::Enumeration {
                payload: ValuePayloadView::Single(inner),
                ..
            } => value = inner,
            ValueKind::Enumeration {
                payload: ValuePayloadView::Structure(mut fields),
                ..
            } => value = fields.next().test_value()?.value(),
            _ => {
                return Err(TestFailure::new(
                    "deep container shape does not match the test input",
                ));
            }
        }
    }
}

#[test]
fn parser_stack_child() -> TestResult<()> {
    if env::var_os("TYPOL_PARSER_STACK_CHILD").is_none() {
        return Ok(());
    }
    thread::Builder::new()
        .stack_size(128 * 1024)
        .spawn(|| -> TestResult<()> {
            let depth = 4096;
            let mut schema_limits = SchemaLimits::default();
            schema_limits.set_max_nesting_depth(depth * 3 + 16);
            let mut config_limits = ConfigLimits::default();
            config_limits = config_limits.with_max_nesting_depth(depth * 3 + 16);
            for layers in [
                &[Layer::Array][..],
                &[Layer::Map],
                &[Layer::Struct],
                &[Layer::Option],
                &[Layer::Single],
                &[Layer::Object],
                &[
                    Layer::Array,
                    Layer::Map,
                    Layer::Struct,
                    Layer::Option,
                    Layer::Single,
                    Layer::Object,
                ],
            ] {
                let input = Input::new(layers, depth)?;
                let schema = compile(&input.schema, schema_limits)?;
                let source = input.source("{n:1}", "2");
                let value = Support::parse(
                    &schema,
                    config_request(
                        (&mut SliceSource::new(source.as_bytes()), "config"),
                        &config_limits,
                    ),
                    &mut NoHost,
                )
                .test_value()?;
                test_eq!(leaf_number(value.root())?, 1)?;
                drop(value);
                for (leaf, code) in [
                    ("{n:999}", DiagnosticCode::CONFIG_VALUE_ERROR),
                    ("{n:0}", DiagnosticCode::CONFIG_VALIDATION_ERROR),
                ] {
                    let source = input.source(leaf, if leaf == "{n:999}" { "999" } else { "2" });
                    let errors = Support::parse(
                        &schema,
                        config_request(
                            (&mut SliceSource::new(source.as_bytes()), "config"),
                            &config_limits,
                        ),
                        &mut NoHost,
                    )
                    .test_error()?;
                    let first_error = TestCheck::item(errors.diagnostics(), 0)?;
                    test_eq!(first_error.code(), code)?;
                    if code == DiagnosticCode::CONFIG_VALUE_ERROR {
                        test_eq!(errors.diagnostics().len(), 2)?;
                        let second_error = TestCheck::item(errors.diagnostics(), 1)?;
                        test_eq!(second_error.code(), code)?;
                        test_eq!(
                            second_error
                                .path()
                                .test_value()?
                                .segments()
                                .first()
                                .test_value()?
                                .as_field(),
                            Some("tail")
                        )?;
                    } else {
                        test_eq!(errors.diagnostics().len(), 1)?;
                        let span = first_error.primary().test_value()?.span();
                        test_eq!(source.get(span.start()..span.end()).test_value()?, "{n:0}")?;
                        test_assert!(first_error.path().test_value()?.len() >= depth)?;
                    }
                }
                let mut limited = config_limits;
                limited = limited.with_max_nesting_depth(depth);
                let errors = Support::parse(
                    &schema,
                    config_request(
                        (&mut SliceSource::new(source.as_bytes()), "config"),
                        &limited,
                    ),
                    &mut NoHost,
                )
                .test_error()?;
                test_eq!(
                    errors.diagnostics().last().test_value()?.code(),
                    DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
                )?;
                let truncated = format!("{{value:{}{{n:1", input.prefix);
                let errors = Support::parse(
                    &schema,
                    config_request(
                        (&mut SliceSource::new(truncated.as_bytes()), "config"),
                        &config_limits,
                    ),
                    &mut NoHost,
                )
                .test_error()?;
                test_eq!(
                    errors.diagnostics().last().test_value()?.code(),
                    DiagnosticCode::CONFIG_SYNTAX_ERROR
                )?;
                test_assert!(
                    errors
                        .diagnostics()
                        .iter()
                        .all(|error| error.message() != "required field is missing")
                )?;
                let mismatched = format!("{{value:{}{{n:1]", input.prefix);
                let errors = Support::parse(
                    &schema,
                    config_request(
                        (&mut SliceSource::new(mismatched.as_bytes()), "config"),
                        &config_limits,
                    ),
                    &mut NoHost,
                )
                .test_error()?;
                test_eq!(
                    errors.diagnostics().last().test_value()?.code(),
                    DiagnosticCode::CONFIG_SYNTAX_ERROR
                )?;
                drop(schema);
            }
            Ok(())
        })
        .test_value()?
        .join()
        .map_err(|_| TestFailure::new("deep-parser thread terminated unexpectedly"))?
}

#[test]
fn explicit_containers_and_deep_assertions_use_bounded_host_stack() -> TestResult<()> {
    let mut child = Command::new(env::current_exe().test_value()?)
        .args(["--exact", "parser_stack_child"])
        .env("TYPOL_PARSER_STACK_CHILD", "1")
        .stdout(Stdio::null())
        .spawn()
        .test_value()?;
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Some(status) = child.try_wait().test_value()? {
            test_assert!(status.success())?;
            return Ok(());
        }
        if Instant::now() >= deadline {
            let terminated = child.kill();
            let reaped = child.wait();
            terminated.test_value()?;
            reaped.test_value()?;
            return Err(TestFailure::new(
                "explicit-container parsing did not finish before the deadline",
            ));
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn explicit_container_depth_and_node_limits_accept_exact_boundaries() -> TestResult<()> {
    let depth = 12;
    for layer in [
        Layer::Array,
        Layer::Map,
        Layer::Struct,
        Layer::Option,
        Layer::Single,
        Layer::Object,
    ] {
        let input = Input::new(&[layer], depth)?;
        let source = input.source("{n:1}", "2");
        let schema = compile(&input.schema, SchemaLimits::default())?;
        let mut limits = ConfigLimits::default();
        let nesting = match layer {
            Layer::Option | Layer::Object => depth * 2 + 2,
            _ => depth + 2,
        };
        let nodes = match layer {
            Layer::Option | Layer::Map => depth * 2 + 4,
            _ => depth + 4,
        };
        limits = limits.with_max_nesting_depth(nesting);
        limits = limits.with_max_value_nodes(nodes);
        drop(
            Support::parse(
                &schema,
                config_request(
                    (&mut SliceSource::new(source.as_bytes()), "config"),
                    &limits,
                ),
                &mut NoHost,
            )
            .test_value()?,
        );
        for reduced in ["depth", "nodes"] {
            let mut limited = limits;
            if reduced == "depth" {
                limited = limited.with_max_nesting_depth(nesting - 1);
            } else {
                limited = limited.with_max_value_nodes(nodes - 1);
            }
            let errors = Support::parse(
                &schema,
                config_request(
                    (&mut SliceSource::new(source.as_bytes()), "config"),
                    &limited,
                ),
                &mut NoHost,
            )
            .test_error()?;
            test_eq!(
                errors.diagnostics().last().test_value()?.code(),
                DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
            )?;
        }
    }

    Ok(())
}

fn config_request<'a>(
    source: (&'a mut impl Read, &'a str),
    limits: &ConfigLimits,
) -> ConfigParseRequest<'a> {
    ConfigParseRequest::new(
        source.0,
        source.1,
        ConfigParseOptions::new(
            ExcerptLimits::default(),
            RegexOptions::default(),
            *limits,
            ExecutionLimits::new(1_000_000_000, 268_435_456),
        ),
    )
}
