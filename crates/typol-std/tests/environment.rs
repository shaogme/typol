#[macro_use]
#[path = "support/assertions.rs"]
mod assertions;

use opeo::{ErrSlot, OResult, Out};
use std::{
    cell::Cell,
    env::{current_exe, var_os},
    error::Error,
    fmt::{self, Debug},
    io,
    process::Command,
    rc::Rc,
};
use typol_config::{
    ConfigLimits, ConfigParseErrors, ConfigParseOptions, ConfigParseRequest, ConfigParser,
    Environment, EnvironmentError, EnvironmentReadContext,
};
use typol_core::{
    BudgetError, ConfigValue, Diagnostic, DiagnosticCode, ExecutionBudget, ExecutionLimits,
    SliceSource, ValueKind,
};
use typol_schema::{
    ResolverError, SchemaCompileErrors, SchemaCompileOptions, SchemaCompiler, SchemaResolveRequest,
    SchemaResolver, SchemaSource,
};
use typol_std::{ProcessEnvironment, ProcessEnvironmentLimitParams, ProcessEnvironmentLimits};

#[cfg(unix)]
use std::{ffi::OsString, os::unix::ffi::OsStringExt};

#[cfg(windows)]
use std::{ffi::OsString, os::windows::ffi::OsStringExt};

struct NoResolver {
    calls: Rc<Cell<usize>>,
}

#[derive(Debug)]
enum ParseFailure {
    Schema {
        errors: Box<SchemaCompileErrors>,
        resolver_calls: usize,
    },
    Config {
        errors: Box<ConfigParseErrors>,
        resolver_calls: usize,
    },
}

struct RedirectEnvironment {
    process: ProcessEnvironment,
    name: &'static str,
}

struct ParsedConfig {
    value: ConfigValue,
    resolver_calls: usize,
}

struct CountingEnvironment {
    process: ProcessEnvironment,
    calls: usize,
    budget_steps: Vec<usize>,
}

trait IntoTestError<T> {
    fn into_test_error(self) -> Result<T, io::Error>;
}

impl<T, E: Debug> IntoTestError<T> for Result<T, E> {
    fn into_test_error(self) -> Result<T, io::Error> {
        self.map_err(|error| io::Error::other(format!("test operation failed: {error:?}")))
    }
}

impl SchemaResolver for NoResolver {
    fn resolve<'src, 'slot, 'borrow>(
        &'src mut self,
        _request: SchemaResolveRequest<'_>,
        out: Out<'slot, 'borrow, ResolverError>,
    ) -> OResult<'slot, SchemaSource<'src>> {
        self.calls.set(self.calls.get() + 1);
        Err(out.fail(ResolverError::NotFound))
    }
}

impl Environment for CountingEnvironment {
    fn read<'env, 'slot, 'borrow>(
        &'env mut self,
        name: &str,
        context: EnvironmentReadContext<'_>,
        out: Out<'slot, 'borrow, EnvironmentError>,
    ) -> OResult<'slot, Option<&'env [u8]>> {
        self.calls += 1;
        self.budget_steps.push(context.execution().steps());
        self.process.read(name, context, out)
    }
}

const DEFAULT_EXECUTION: ExecutionLimits = ExecutionLimits::new(16_777_216, 16_777_216);

fn limits(value_bytes: usize, names: usize) -> ProcessEnvironmentLimits {
    ProcessEnvironmentLimits::new(ProcessEnvironmentLimitParams {
        max_environment_name_bytes: names,
        max_environment_value_bytes: value_bytes,
    })
}

fn read_process<'env>(
    process: &'env mut ProcessEnvironment,
    name: &str,
) -> Result<Option<&'env [u8]>, EnvironmentError> {
    read_process_with(process, name, DEFAULT_EXECUTION, usize::MAX)
}

fn read_process_with<'env>(
    process: &'env mut ProcessEnvironment,
    name: &str,
    execution: ExecutionLimits,
    max_value_bytes: usize,
) -> Result<Option<&'env [u8]>, EnvironmentError> {
    let budget = ErrSlot::<BudgetError>::new()
        .call(|out| ExecutionBudget::new(execution, out))
        .map_err(|_| EnvironmentError::InternalError)?;
    let context = EnvironmentReadContext::new(&budget, max_value_bytes);
    ErrSlot::<EnvironmentError>::new().call(|out| process.read(name, context, out))
}

fn parse(text: &str, environment: &mut dyn Environment) -> Result<ParsedConfig, ParseFailure> {
    parse_with_options(text, environment, ConfigParseOptions::default())
}

fn parse_with_options(
    text: &str,
    environment: &mut dyn Environment,
    options: ConfigParseOptions,
) -> Result<ParsedConfig, ParseFailure> {
    let compiler = ErrSlot::<SchemaCompileErrors>::new()
        .call(|out| SchemaCompiler::new(SchemaCompileOptions::default(), out))
        .map_err(|errors| ParseFailure::Schema {
            errors: Box::new(errors),
            resolver_calls: 0,
        })?;
    let resolver_calls = Rc::new(Cell::new(0));
    let mut resolver = NoResolver {
        calls: Rc::clone(&resolver_calls),
    };
    let schema = ErrSlot::<SchemaCompileErrors>::new()
        .call(|out| {
            compiler.compile(
                SchemaSource::from_str(
                    b"root",
                    r#"entry struct Root {
            a: string @env("VALUE") = "default",
            b: u8 @env("VALUE") = 3,
            c: string @env("OTHER") = "other",
        }"#,
                ),
                &mut resolver,
                None,
                out,
            )
        })
        .map_err(|errors| ParseFailure::Schema {
            errors: Box::new(errors),
            resolver_calls: resolver_calls.get(),
        })?;
    let resolver_calls = resolver_calls.get();
    let value = ErrSlot::<ConfigParseErrors>::new()
        .call(|out| {
            ConfigParser::new(&schema).parse(
                ConfigParseRequest::new(
                    &mut SliceSource::new((text).as_bytes()),
                    "config",
                    options,
                ),
                environment,
                out,
            )
        })
        .map_err(|errors| ParseFailure::Config {
            errors: Box::new(errors),
            resolver_calls,
        })?;
    let value = value.into_config_value();
    Ok(ParsedConfig {
        value,
        resolver_calls,
    })
}

impl fmt::Display for ParseFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl Error for ParseFailure {}

impl ParseFailure {
    fn first_diagnostic(&self) -> Option<&Diagnostic> {
        match self {
            Self::Schema { errors, .. } => errors.diagnostics().first(),
            Self::Config { errors, .. } => errors.diagnostics().first(),
        }
    }

    fn resolver_calls(&self) -> usize {
        match self {
            Self::Schema { resolver_calls, .. } | Self::Config { resolver_calls, .. } => {
                *resolver_calls
            }
        }
    }
}

fn first(value: &ConfigValue) -> Result<ValueKind<'_>, io::Error> {
    let ValueKind::Structure(mut fields) = value.as_ref() else {
        return Err(io::Error::other("expected a structure"));
    };
    fields
        .next()
        .map(|field| field.value().as_ref())
        .ok_or_else(|| io::Error::other("structure should contain its first field"))
}

#[test]
fn process_environment_child() -> Result<(), Box<dyn Error>> {
    if var_os("TYPOL_ENV_CHILD").is_none() {
        return Ok(());
    }
    let mut provider = ProcessEnvironment::new(ProcessEnvironmentLimits::default());
    ensure_eq!(
        read_process(&mut provider, "MISSING").into_test_error()?,
        None
    );
    ensure_eq!(
        read_process(&mut provider, "EMPTY").into_test_error()?,
        Some(&b""[..])
    );
    ensure_eq!(
        read_process(&mut provider, "VALUE").into_test_error()?,
        Some(&b"7"[..])
    );
    ensure_eq!(
        read_process(&mut provider, "SPACES").into_test_error()?,
        Some(&b" 7 "[..])
    );
    ensure_eq!(
        read_process(&mut provider, "UNICODE").into_test_error()?,
        Some("Ḁ😀".as_bytes())
    );
    for name in ["", "A=B", "A\0B"] {
        ensure_eq!(
            read_process(&mut provider, name),
            Err(EnvironmentError::HostFailure)
        );
    }
    ensure!(!format!("{provider:?}").contains("Ḁ"));
    #[cfg(unix)]
    ensure_eq!(
        read_process(&mut provider, "INVALID").into_test_error()?,
        Some(&b"\xff"[..])
    );
    #[cfg(windows)]
    ensure_eq!(
        read_process(&mut provider, "INVALID"),
        Err(EnvironmentError::HostFailure)
    );
    let mut exact = ProcessEnvironment::new(limits(7, 7));
    ensure_eq!(
        read_process(&mut exact, "UNICODE").into_test_error()?,
        Some("Ḁ😀".as_bytes())
    );
    let mut short = ProcessEnvironment::new(limits(6, 7));
    ensure_eq!(
        read_process(&mut short, "UNICODE"),
        Err(EnvironmentError::ResourceLimitExceeded)
    );
    let mut zero = ProcessEnvironment::new(limits(0, 5));
    ensure_eq!(
        read_process(&mut zero, "EMPTY").into_test_error()?,
        Some(&b""[..])
    );
    ensure_eq!(
        read_process(&mut zero, "VALUE"),
        Err(EnvironmentError::ResourceLimitExceeded)
    );
    ensure_eq!(
        read_process(&mut zero, "EMPTY").into_test_error()?,
        Some(&b""[..])
    );
    let execution = ExecutionLimits::new(0, 4096);
    let mut limited = ProcessEnvironment::new(limits(10, 10));
    ensure_eq!(
        read_process_with(&mut limited, "VALUE", execution, usize::MAX),
        Err(EnvironmentError::ResourceLimitExceeded)
    );
    let mut exhausted = ProcessEnvironment::new(ProcessEnvironmentLimits::default());
    ensure_eq!(
        read_process_with(
            &mut exhausted,
            "VALUE",
            ExecutionLimits::new(0, 4096),
            usize::MAX,
        ),
        Err(EnvironmentError::ResourceLimitExceeded)
    );
    let mut named = ProcessEnvironment::new(limits(10, 4));
    ensure_eq!(
        read_process(&mut named, "VALUE"),
        Err(EnvironmentError::ResourceLimitExceeded)
    );
    let mut counted = CountingEnvironment {
        process: provider,
        calls: 0,
        budget_steps: Vec::new(),
    };
    for text in [r#"{b: env("VALUE", 256)}"#, r#"{b: env("OTHER")}"#] {
        let error = parse(text, &mut counted)
            .err()
            .ok_or_else(|| io::Error::other("expected configuration operation to fail"))?;
        ensure_eq!(error.resolver_calls(), 0);
        ensure_eq!(counted.calls, 0);
    }
    let parsed = parse("{}", &mut counted)?;
    ensure_eq!(parsed.resolver_calls, 0);
    ensure_eq!(counted.calls, 0);
    for _ in 0..2 {
        let parsed = parse(
            r#"{a: env("VALUE"), b: env("VALUE"), c: env("OTHER")}"#,
            &mut counted,
        )?;
        ensure_eq!(parsed.resolver_calls, 0);
        ensure!(matches!(first(&parsed.value)?, ValueKind::String("7")));
    }
    ensure_eq!(counted.calls, 4);
    ensure!(
        counted
            .budget_steps
            .first()
            .zip(counted.budget_steps.get(1))
            .is_some_and(|(first, second)| second > first)
    );
    ensure!(
        counted
            .budget_steps
            .get(2)
            .zip(counted.budget_steps.get(3))
            .is_some_and(|(first, second)| second > first)
    );

    let config_limits = ConfigLimits::default().with_max_environment_bytes(1);
    let options = ConfigParseOptions::default().with_config(config_limits);
    let mut cumulative_limited = ProcessEnvironment::new(ProcessEnvironmentLimits::default());
    let error = parse_with_options(
        r#"{a: env("VALUE"), c: env("OTHER")}"#,
        &mut cumulative_limited,
        options,
    )
    .err()
    .ok_or_else(|| {
        io::Error::other("cumulative environment budget should reject the second value")
    })?;
    ensure_eq!(
        error
            .first_diagnostic()
            .ok_or_else(|| io::Error::other("diagnostic is missing"))?
            .code(),
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
    );
    let mut missing = RedirectEnvironment {
        process: counted.process,
        name: "MISSING",
    };
    let parsed = parse(r#"{a: env("VALUE", "fallback")}"#, &mut missing)?;
    ensure_eq!(parsed.resolver_calls, 0);
    ensure!(matches!(
        first(&parsed.value)?,
        ValueKind::String("fallback")
    ));
    for name in ["EMPTY", "SPACES", "INVALID"] {
        let mut redirected = RedirectEnvironment {
            process: ProcessEnvironment::new(ProcessEnvironmentLimits::default()),
            name,
        };
        let errors = parse(r#"{b: env("VALUE", 9)}"#, &mut redirected)
            .err()
            .ok_or_else(|| io::Error::other("expected operation to fail"))?;
        ensure_eq!(errors.resolver_calls(), 0);
        ensure_eq!(
            errors
                .first_diagnostic()
                .ok_or_else(|| io::Error::other("diagnostic is missing"))?
                .code(),
            if name == "INVALID" {
                DiagnosticCode::CONFIG_ENVIRONMENT_ERROR
            } else {
                DiagnosticCode::CONFIG_VALUE_ERROR
            }
        );
        let value = if name != "INVALID" {
            Some(parse(r#"{a: env("VALUE", "fallback")}"#, &mut redirected)?)
        } else {
            None
        };
        if let Some(value) = value {
            ensure_eq!(value.resolver_calls, 0);
            ensure!(
                matches!(first(&value.value)?, ValueKind::String(text) if text == if name == "EMPTY" { "" } else { " 7 " })
            );
        }
    }
    let mut limited = ProcessEnvironment::new(limits(0, 4096));
    let error = parse(r#"{b: env("VALUE", 9)}"#, &mut limited)
        .err()
        .ok_or_else(|| io::Error::other("expected operation to fail"))?;
    ensure_eq!(error.resolver_calls(), 0);
    ensure_eq!(
        error
            .first_diagnostic()
            .ok_or_else(|| io::Error::other("diagnostic is missing"))?
            .code(),
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
    );

    Ok(())
}

impl Environment for RedirectEnvironment {
    fn read<'env, 'slot, 'borrow>(
        &'env mut self,
        _: &str,
        context: EnvironmentReadContext<'_>,
        out: Out<'slot, 'borrow, EnvironmentError>,
    ) -> OResult<'slot, Option<&'env [u8]>> {
        self.process.read(self.name, context, out)
    }
}

#[test]
fn process_environment_runs_in_isolated_child() -> Result<(), Box<dyn Error>> {
    let mut command = Command::new(current_exe()?);
    command
        .args(["--exact", "process_environment_child", "--nocapture"])
        .env_clear()
        .env("TYPOL_ENV_CHILD", "1")
        .env("VALUE", "7")
        .env("OTHER", "8")
        .env("EMPTY", "")
        .env("SPACES", " 7 ")
        .env("UNICODE", "Ḁ😀");
    #[cfg(unix)]
    command.env("INVALID", OsString::from_vec(vec![255]));
    #[cfg(windows)]
    command.env("INVALID", OsString::from_wide(&[0xd800]));
    let output = command.output()?;
    ensure!(
        output.status.success(),
        "child process failed: {}",
        String::from_utf8_lossy(&output.stdout)
    );

    Ok(())
}
