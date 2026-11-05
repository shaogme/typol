use typol_test_support::ConfigTestSupport as Support;

use core::cmp::Ordering;
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
use typol_core::{ExecutionLimits, Read, SliceSource, ValueKind};
use typol_policy::{RegexLimits, ResourceLimits, ValueLimits};
use typol_schema::{SchemaCompileOptions, SchemaLimits, SchemaSource};
use typol_semantic::{
    ComparisonOptions, CompiledSchema, SchemaComparison, SchemaEvaluator as Evaluator,
    SchemaValue as EvalValue,
};
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

#[test]
fn execution_stack_child() -> TestResult<()> {
    if env::var_os("TYPOL_EXECUTION_STACK_CHILD").is_none() {
        return Ok(());
    }
    thread::Builder::new()
        .stack_size(128 * 1024)
        .spawn(|| -> TestResult<()> {
            let mut schema_errors = TestSlot::default();
            let mut evaluation_errors = TestSlot::default();
            let depth = 4096;
            let mut schema_limits = SchemaLimits::default();
            schema_limits.set_max_nesting_depth(depth * 5 + 16);
            let mut config_limits = ConfigLimits::default();
            config_limits = config_limits.with_max_nesting_depth(depth * 5 + 16);
            let mut value_limits = ValueLimits::default();
            value_limits.set_max_nesting_depth(depth * 5 + 16);
            let regex_limits = RegexLimits::default();
            for expression in [
                format!("{}true", "!".repeat(depth)),
                format!("true{}", " && true".repeat(depth)),
                format!("1{} > 0", " + 1".repeat(depth)),
            ] {
                let text = format!("entry struct Root {{}} @assert({expression}, \"rule\")");
                let schema = compile(&text, schema_limits)?;
                drop(
                    Support::parse(
                        &schema,
                        config_request(
                            (&mut SliceSource::new(("{}").as_bytes()), "config"),
                            &config_limits,
                        ),
                        &mut NoHost,
                    )
                    .test_value()?,
                );
            }
            let ty = format!("{}u8{}", "[".repeat(depth), "]".repeat(depth));
            let value = format!("{}1{}", "[".repeat(depth), "]".repeat(depth));
            let expression = format!(
                "item{} > 0{}",
                ".all(|x| x".repeat(depth),
                ")".repeat(depth)
            );
            let text = format!(
                "entry struct Root {{ values: [{ty}] @all({expression}, \"rule\") = [{value}] }}"
            );
            let schema = compile(&text, schema_limits)?;
            let config = String::from("{}");
            drop(
                Support::parse_validated_config(
                    &schema,
                    config_request(
                        (&mut SliceSource::new(config.as_bytes()), "config"),
                        &config_limits,
                    ),
                    &mut NoHost,
                )
                .test_value()?,
            );
            drop(schema);
            let text = format!(
                "enum E {{ V({ty}) }} entry struct Root {{ values: [E] @unique = [.V({value})] }}"
            );
            let schema = compile(&text, schema_limits)?;
            let root = Support::parse_validated_config(
                &schema,
                config_request(
                    (&mut SliceSource::new(("{}").as_bytes()), "config"),
                    &config_limits,
                ),
                &mut NoHost,
            )
            .test_value()?;
            let ValueKind::Structure(fields) = root.as_config_value().as_ref() else {
                return Err(TestFailure::new("parsed root value should be a struct"));
            };
            let field = schema_errors
                .access(|out| schema.entry().field_named("values", out))
                .map_err(|caught| {
                    let _ = caught;
                    TestFailure::new("failed to read schema field")
                })?
                .test_expect("schema is missing the values field")?;
            let evaluator = Evaluator::new(&schema, &value_limits, &regex_limits, None);
            let field_type = schema_errors
                .access(|out| field.ty(out))
                .map_err(|caught| {
                    let _ = caught;
                    TestFailure::new("failed to read schema field type")
                })?;
            let value = EvalValue::config(
                fields.into_iter().next().test_value()?.value(),
                field_type,
                false,
            );
            let ordering = evaluation_errors
                .access(|out| {
                    SchemaComparison::compare(
                        &evaluator,
                        value,
                        value,
                        ComparisonOptions {
                            equality_only: true,
                            depth: 0,
                        },
                        out,
                    )
                })
                .map_err(|caught| {
                    let _ = caught;
                    TestFailure::new("failed to compare schema values during execution")
                })?;
            test_eq!(ordering, Ordering::Equal)?;
            drop(root);
            drop(schema);
            Ok(())
        })
        .test_value()?
        .join()
        .map_err(|_| TestFailure::new("deep-expression worker thread terminated unexpectedly"))?
}

#[test]
fn deep_config_execution_and_destruction_use_bounded_host_stack() -> TestResult<()> {
    let mut child = Command::new(env::current_exe().test_value()?)
        .args(["--exact", "execution_stack_child"])
        .env("TYPOL_EXECUTION_STACK_CHILD", "1")
        .stdout(Stdio::null())
        .spawn()
        .test_value()?;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(status) = child.try_wait().test_value()? {
            test_assert!(status.success())?;
            return Ok(());
        }
        if Instant::now() >= deadline {
            child.kill().test_value()?;
            child.wait().test_value()?;
            return Err(TestFailure::new(
                "configuration execution did not finish before the deadline",
            ));
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn config_request<'a>(
    source: (&'a mut impl Read, &'a str),
    limits: &ConfigLimits,
) -> ConfigParseRequest<'a> {
    ConfigParseRequest::new(
        source.0,
        source.1,
        ConfigParseOptions::new(
            ResourceLimits::default(),
            *limits,
            ExecutionLimits::new(1_000_000_000, 268_435_456),
        ),
    )
}
