#[path = "support/schema_compile.rs"]
mod schema_support;

use std::{
    env,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use typol_core::{DiagnosticCode, ValueKind};
use typol_schema::{
    ResolverError, SchemaCompileOptions, SchemaLimits, SchemaResolveRequest, SchemaResolver,
    SchemaSource,
    opeo::{OResult, Out},
};
use typol_semantic::{CompiledSchema, SchemaAccessError};
use typol_test_support::*;

fn access<T>(
    operation: impl for<'slot> FnOnce(Out<'slot, 'slot, SchemaAccessError>) -> OResult<'slot, T>,
) -> TestResult<T> {
    schema_support::SchemaCompileTestSupport::schema_access(operation).test_value()
}

fn present<T>(value: Option<T>) -> TestResult<T> {
    value.ok_or_else(|| TestFailure::new("expected member is missing from the schema"))
}

struct EmptyResolver;

impl SchemaResolver for EmptyResolver {
    fn resolve<'src, 'slot>(
        &'src mut self,
        _request: SchemaResolveRequest<'_>,
        out: Out<'slot, '_, ResolverError>,
    ) -> OResult<'slot, SchemaSource<'src>> {
        Err(out.fail(ResolverError::NotFound))
    }
}

fn compile(source: &str, limits: &SchemaLimits) -> TestResult<CompiledSchema> {
    schema_support::SchemaCompileTestSupport::run(
        SchemaCompileOptions::new(*limits),
        SchemaSource::from_str(b"root", source),
        &mut EmptyResolver,
        None,
    )
    .test_value()
}

#[test]
fn schema_stack_child() -> TestResult {
    if env::var_os("TYPOL_SCHEMA_STACK_CHILD").is_none() {
        return Ok(());
    }
    thread::Builder::new().stack_size(128 * 1024).spawn(|| -> TestResult {
        let depth = 4096;
        let mut limits = SchemaLimits::default();
        limits.set_max_nesting_depth(depth * 5 + 16);
        // Full compilation and teardown exercise deep inputs across types, default construction, cache copying, and module exports.
        let ty = format!("{}u8{}", "[".repeat(depth), "]".repeat(depth));
        let value = format!("{}1{}", "[".repeat(depth), "]".repeat(depth));
        let source = format!("struct Holder {{ payload: {ty} = {value} }} entry struct Root {{ holder: Holder = {{}} }}");
        let schema = compile(&source, &limits)?;
        let holder_field = present(access(|out| schema.entry().field_named("holder", out))?)?;
        let holder = present(access(|out| holder_field.default_value(out))?)?;
        let ValueKind::Structure(mut fields) = holder.as_ref() else {
            return Err(String::from("expected a struct").into());
        };
        let mut value_ref = fields.next().test_value()?.value();
        for _ in 0..depth {
            let ValueKind::Array(mut values) = value_ref.as_ref() else {
                return Err(String::from("deep array should be preserved").into());
            };
            value_ref = values.next().test_value()?.value();
        }
        test_verify!(matches!(value_ref.as_ref(), ValueKind::Unsigned(1)));
        drop(schema);
        let source = format!("{}type Text = string @regex(\"x\");{} entry struct Root {{}}", "mod M {".repeat(depth), "}".repeat(depth));
        drop(compile(&source, &limits)?);
        for expression in [format!("{}true", "!".repeat(depth)), format!("true{}", " && true".repeat(depth)), format!("1{} > 0", " + 1".repeat(depth))] {
            drop(compile(&format!("entry struct Root {{}} @assert({expression}, \"rule\")"), &limits)?);
        }
        let expression = format!("item{} > 0{}", ".all(|x| x".repeat(depth), ")".repeat(depth));
        let source = format!("entry struct Root {{ values: [{}] @all({expression}, \"rule\") = [{}] }}", ty, value);
        drop(compile(&source, &limits)?);
        let source = format!("enum E {{ V({ty}) }} entry struct Root {{ values: [E] @unique = [.V({value}), .V({value})] }}");
        let errors = schema_support::SchemaCompileTestSupport::run(
            SchemaCompileOptions::new(limits),
            SchemaSource::from_str(b"root", &source),
            &mut EmptyResolver,
            None,
        )
        .test_error()?;
        test_verify!(errors.diagnostics().iter().all(|error| error.code() == DiagnosticCode::SCHEMA_SEMANTIC_ERROR));
        drop(errors);
        Ok(())
    })
    .test_value()?
    .join()
    .map_err(|_| String::from("schema small-stack child thread exited unexpectedly"))??;

    Ok(())
}

#[test]
fn full_schema_compilation_and_destruction_use_bounded_host_stack() -> TestResult {
    let mut child = Command::new(env::current_exe().test_value()?)
        .args(["--exact", "schema_stack_child"])
        .env("TYPOL_SCHEMA_STACK_CHILD", "1")
        .stdout(Stdio::null())
        .spawn()
        .test_value()?;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(status) = child.try_wait().test_value()? {
            test_verify!(status.success(), "schema small-stack child process failed");
            break;
        }
        if Instant::now() >= deadline {
            child.kill().test_value()?;
            child.wait().test_value()?;
            return Err(
                String::from("schema compilation did not finish before the deadline").into(),
            );
        }
        thread::sleep(Duration::from_millis(10));
    }

    Ok(())
}
