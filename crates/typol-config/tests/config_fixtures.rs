use typol_test_support::{ConfigTestSupport as Support, TestOpeoOutcome};

use std::collections::BTreeMap;
use typol_config::{
    ConfigLimits, ConfigParseErrors, ConfigParseOptions, ConfigParseRequest, Environment,
    EnvironmentError, EnvironmentReadContext, StreamLimits,
    opeo::{OResult, Out},
};
use typol_core::{
    ConfigValue, DiagnosticCode, ExecutionLimits, Read, SliceSource, SourceId, SourceReadError,
    SourceReadErrorKind, ValueKind, ValuePayloadView, ValueSource, ValueView,
};
use typol_policy::ResourceLimits;
use typol_schema::{
    ResolverError, SchemaCompileOptions, SchemaResolveRequest, SchemaResolver, SchemaSource,
};
use typol_semantic::CompiledSchema;
use typol_test_support::*;

type ConfigLimitBuilder = (fn(ConfigLimits, usize) -> ConfigLimits, usize);

#[derive(Clone, Copy)]
struct TestLimits {
    resources: ResourceLimits,
    config: ConfigLimits,
}

impl From<&ResourceLimits> for TestLimits {
    fn from(resources: &ResourceLimits) -> Self {
        Self {
            resources: *resources,
            config: ConfigLimits::default(),
        }
    }
}

impl From<&ConfigLimits> for TestLimits {
    fn from(config: &ConfigLimits) -> Self {
        Self {
            resources: ResourceLimits::default(),
            config: *config,
        }
    }
}

impl From<&(&ResourceLimits, &ConfigLimits)> for TestLimits {
    fn from(limits: &(&ResourceLimits, &ConfigLimits)) -> Self {
        Self {
            resources: *limits.0,
            config: *limits.1,
        }
    }
}

impl TestLimits {
    fn resources(self) -> ResourceLimits {
        self.resources
    }

    fn config(self) -> ConfigLimits {
        self.config
    }
}

struct Fixture {
    name: &'static str,
    schema: &'static str,
    config: &'static str,
    required: &'static str,
    calls: usize,
}

struct EnvironmentFailureCase {
    fixture: &'static Fixture,
    name: &'static str,
    bytes: &'static [u8],
    code: DiagnosticCode,
}

struct FallbackCase {
    old: &'static str,
    new: &'static str,
    code: DiagnosticCode,
}

struct Counterexample {
    fixture: &'static Fixture,
    old: &'static str,
    new: &'static str,
    assertion: bool,
}

struct ResourceLimitSetter {
    set: fn(&mut ResourceLimits, usize),
    upper: usize,
}

const SERVER: Fixture = Fixture {
    name: "server",
    schema: include_str!("../../../examples/config/server.tschem"),
    config: include_str!("../../../examples/config/server.tconf"),
    required: "TYPOL_CLIENT_SECRET",
    calls: 10,
};
const DATABASE: Fixture = Fixture {
    name: "database",
    schema: include_str!("../../../examples/config/database.tschem"),
    config: include_str!("../../../examples/config/database.tconf"),
    required: "TYPOL_DB_PASSWORD",
    calls: 5,
};
const ALL_TYPES: Fixture = Fixture {
    name: "all_types",
    schema: include_str!("../../../examples/config/all_types.tschem"),
    config: include_str!("../../../examples/config/all_types.tconf"),
    required: "TYPOL_NOTIFY_TOKEN",
    calls: 7,
};
const SECRET: &[u8] = b"fixture-secret-value";

#[derive(Default)]
struct MemoryResolver {
    calls: usize,
    invalid_origin: bool,
    source: OwnedReadSource,
}

impl SchemaResolver for MemoryResolver {
    fn resolve<'a, 'slot, 'borrow>(
        &'a mut self,
        request: SchemaResolveRequest<'_>,
        out: Out<'slot, 'borrow, ResolverError>,
    ) -> OResult<'slot, SchemaSource<'a>> {
        let from = request.from_identity;
        let name = request.module_name;
        let max_bytes = request.source_byte_limit;
        self.calls += 1;
        if from != b"server" && from != b"database" {
            self.invalid_origin = true;
            return Err(out.fail(ResolverError::NotFound));
        }
        if name != "common" {
            return Err(out.fail(ResolverError::NotFound));
        }
        let text = include_str!("../../../examples/config/common.tschem");
        if text.len() > max_bytes {
            return Err(out.fail(ResolverError::ResourceLimitExceeded));
        }
        self.source.replace(text);
        Ok(SchemaSource::from_source(b"common", &mut self.source))
    }
}

#[derive(Default)]
struct MemoryEnvironment {
    values: BTreeMap<String, Vec<u8>>,
    calls: BTreeMap<String, usize>,
    error: Option<EnvironmentError>,
}

impl MemoryEnvironment {
    fn required(fixture: &Fixture) -> Self {
        let mut provider = Self::default();
        provider.set(fixture.required, SECRET);
        provider
    }

    fn set(&mut self, name: &str, bytes: &[u8]) {
        self.values.insert(name.into(), bytes.to_vec());
    }

    fn assert_cached(&self, fixture: &Fixture) -> TestResult<()> {
        test_eq!(self.calls.len(), fixture.calls)?;
        test_assert!(self.calls.values().all(|count| *count == 1))?;
        Ok(())
    }
}

impl Environment for MemoryEnvironment {
    fn read<'env, 'slot, 'borrow>(
        &'env mut self,
        name: &str,
        _: EnvironmentReadContext<'_>,
        out: Out<'slot, 'borrow, EnvironmentError>,
    ) -> OResult<'slot, Option<&'env [u8]>> {
        *self.calls.entry(name.into()).or_default() += 1;
        match self.error {
            Some(error) => Err(out.fail(error)),
            None => Ok(self.values.get(name).map(Vec::as_slice)),
        }
    }
}

fn compile(fixture: &Fixture) -> TestResult<CompiledSchema> {
    let mut resolver = MemoryResolver::default();
    let schema = Support::compile_schema(
        SchemaCompileOptions::default(),
        SchemaSource::from_str(fixture.name.as_bytes(), fixture.schema),
        Some(&mut resolver),
        None,
    )
    .test_value()?;
    test_assert!(!resolver.invalid_origin)?;
    test_eq!(resolver.calls, usize::from(fixture.name != "all_types"))?;
    Ok(schema)
}

fn parse(
    fixture: &Fixture,
    schema: &CompiledSchema,
    text: &str,
    provider: &mut MemoryEnvironment,
    limits: impl Into<TestLimits>,
) -> TestOpeoOutcome<ConfigValue, ConfigParseErrors> {
    Support::parse(
        schema,
        config_request(
            (&mut SliceSource::new((text).as_bytes()), fixture.name),
            limits,
        ),
        provider,
    )
}

fn field<'a>(value: impl Into<ValueViewInput<'a>>, name: &str) -> TestResult<ValueView<'a>> {
    let mut fields = match value.into().value_view().as_ref() {
        ValueKind::Structure(fields)
        | ValueKind::Enumeration {
            payload: ValuePayloadView::Structure(fields),
            ..
        } => fields,
        ValueKind::Optional(Some(inner)) => return field(inner, name),
        _ => {
            return Err(TestFailure::new(
                "expected a struct or anonymous-struct payload",
            ));
        }
    };
    fields
        .find(|field| field.name().unwrap_or_default() == name)
        .test_value()
        .map(|field| field.value())
}

fn at<'a>(value: impl Into<ValueViewInput<'a>>, path: &[&str]) -> TestResult<ValueView<'a>> {
    let mut value = value.into().value_view();
    for name in path {
        value = field(value, name)?;
    }
    Ok(value)
}

fn assert_code(errors: &ConfigParseErrors, code: DiagnosticCode) -> TestResult<()> {
    test_assert!(!errors.diagnostics().is_empty())?;
    test_assert!(errors.diagnostics().iter().all(|d| d.code() == code))?;
    Ok(())
}

fn replace(fixture: &Fixture, old: &str, new: &str) -> TestResult<String> {
    test_eq!(fixture.config.matches(old).count(), 1)?;
    Ok(fixture.config.replacen(old, new, 1))
}

#[test]
fn original_fixtures_return_owned_values_with_fallback_default_and_none() -> TestResult<()> {
    let values: Vec<_> = [&SERVER, &DATABASE, &ALL_TYPES]
        .into_iter()
        .map(|fixture| -> TestResult<_> {
            let schema = compile(fixture)?;
            let text = fixture.config.to_owned();
            let mut provider = MemoryEnvironment::required(fixture);
            let value = parse(
                fixture,
                &schema,
                &text,
                &mut provider,
                &ResourceLimits::default(),
            )
            .test_value()?;
            provider.assert_cached(fixture)?;
            Ok(value)
        })
        .collect::<TestResult<Vec<_>>>()?;
    let server = TestCheck::item(&values, 0)?;
    test_assert!(matches!(
        field(server, "app_name")?.as_ref(),
        ValueKind::String("gateway-core")
    ))?;
    test_eq!(
        field(server, "app_name")?.source(),
        ValueSource::ExplicitFallback
    )?;
    test_eq!(
        at(server, &["worker_pool", "priority"])?.source(),
        ValueSource::SchemaDefault
    )?;
    test_assert!(matches!(
        at(server, &["security", "backup_phone"])?.as_ref(),
        ValueKind::Optional(None)
    ))?;
    test_eq!(
        at(server, &["security", "credential", "client_secret"])?.source(),
        ValueSource::EnvironmentVariable
    )?;
    let database = TestCheck::item(&values, 1)?;
    test_eq!(
        field(database, "default_isolation")?.source(),
        ValueSource::SchemaDefault
    )?;
    test_assert!(matches!(
        at(database, &["auth", "password"])?.as_ref(),
        ValueKind::String("fixture-secret-value")
    ))?;
    let all = TestCheck::item(&values, 2)?;
    test_assert!(matches!(
        at(all, &["collections", "optional_token"])?.as_ref(),
        ValueKind::Optional(None)
    ))?;
    test_assert!(matches!(
        at(all, &["texts", "plain_text"])?.as_ref(),
        ValueKind::String("default-text")
    ))?;
    test_eq!(
        at(all, &["texts", "plain_text"])?.source(),
        ValueSource::SchemaDefault
    )?;

    Ok(())
}

#[test]
fn missing_required_environment_fails_all_original_fixtures_safely() -> TestResult<()> {
    for fixture in [&SERVER, &DATABASE, &ALL_TYPES] {
        let schema = compile(fixture)?;
        let errors = {
            let text = fixture.config.to_owned();
            let mut provider = MemoryEnvironment::default();
            let errors = parse(
                fixture,
                &schema,
                &text,
                &mut provider,
                &ResourceLimits::default(),
            )
            .test_error()?;
            provider.assert_cached(fixture)?;
            errors
        };
        assert_code(&errors, DiagnosticCode::CONFIG_VALUE_ERROR)?;
        test_eq!(errors.config_source_name(), Some(fixture.name))?;
        for diagnostic in errors.diagnostics() {
            test_eq!(diagnostic.message(), "value is hidden")?;
            test_assert!(diagnostic.value_source().is_none())?;
            test_assert!(
                errors
                    .span_location(diagnostic.primary().test_value()?)
                    .is_some()
            )?;
        }
    }

    Ok(())
}

#[test]
fn legal_environment_overrides_preserve_types_and_validate_shared_references() -> TestResult<()> {
    for (fixture, settings) in [
        (
            &SERVER,
            vec![
                ("TYPOL_APP_NAME", "edge-gateway"),
                ("TYPOL_NET_HOST", "edge.example.org"),
                ("TYPOL_NET_PORT", "9443"),
                ("TYPOL_WORKER_THREADS", "128"),
                ("TYPOL_MAX_MEMORY_BYTES", "16777216"),
                ("TYPOL_NODE_ID", "42"),
                ("TYPOL_S3_BUCKET", "acceptance-bucket"),
            ],
        ),
        (
            &DATABASE,
            vec![
                ("TYPOL_DB_HOST", "db.example.org"),
                ("TYPOL_DB_PORT", "6432"),
                ("TYPOL_DB_USER", "acceptance_user"),
                ("TYPOL_DB_NAME", "acceptance_db"),
            ],
        ),
        (
            &ALL_TYPES,
            vec![
                ("TYPOL_SAMPLE_I32", "-42"),
                ("TYPOL_SAMPLE_U16", "42"),
                ("TYPOL_SAMPLE_F64", "12.5"),
                ("TYPOL_SAMPLE_SLUG", "acceptance-slug"),
                ("TYPOL_SAMPLE_ACTIVE", "false"),
                ("TYPOL_SECRET_TOKEN", "optional-secret-token"),
            ],
        ),
    ] {
        let schema = compile(fixture)?;
        let mut provider = MemoryEnvironment::required(fixture);
        for (name, text) in settings {
            provider.set(name, text.as_bytes());
        }
        let value = parse(
            fixture,
            &schema,
            fixture.config,
            &mut provider,
            &ResourceLimits::default(),
        )
        .test_value()?;
        provider.assert_cached(fixture)?;
        match fixture.name {
            "server" => {
                for path in [
                    vec!["app_name"],
                    vec!["security", "credential", "client_id"],
                ] {
                    test_assert!(matches!(
                        at(&value, &path)?.as_ref(),
                        ValueKind::String("edge-gateway")
                    ))?;
                    test_eq!(
                        at(&value, &path)?.source(),
                        ValueSource::EnvironmentVariable
                    )?;
                }
                test_assert!(matches!(
                    at(&value, &["listen_endpoint", "port"])?.as_ref(),
                    ValueKind::Unsigned(9443)
                ))?;
                test_eq!(
                    provider.calls.get("TYPOL_APP_NAME").copied().test_value()?,
                    1
                )?;
                parse(
                    fixture,
                    &schema,
                    fixture.config,
                    &mut provider,
                    &ResourceLimits::default(),
                )
                .test_value()?;
                test_assert!(provider.calls.values().all(|count| *count == 2))?;
            }
            "database" => {
                test_assert!(matches!(
                    at(&value, &["primary_host", "port"])?.as_ref(),
                    ValueKind::Unsigned(6432)
                ))?;
                test_assert!(matches!(
                    at(&value, &["auth", "username"])?.as_ref(),
                    ValueKind::String("acceptance_user")
                ))?;
            }
            "all_types" => {
                test_assert!(matches!(
                    at(&value, &["integers", "val_i32"])?.as_ref(),
                    ValueKind::Signed(-42)
                ))?;
                test_assert!(matches!(
                    at(&value, &["floats", "val_f64"])?.as_ref(),
                    ValueKind::Float64(12.5)
                ))?;
                test_assert!(matches!(
                    at(&value, &["texts", "is_active"])?.as_ref(),
                    ValueKind::Bool(false)
                ))?;
                let ValueKind::Optional(Some(token)) =
                    at(&value, &["collections", "optional_token"])?.as_ref()
                else {
                    return Err(TestFailure::new(
                        "optional_token should have an environment value",
                    ));
                };
                test_eq!(token.source(), ValueSource::EnvironmentVariable)?;
            }
            _ => return Err(TestFailure::new("unknown configuration fixture type")),
        }
    }

    Ok(())
}

#[test]
fn illegal_environment_never_uses_valid_fallback_or_exposes_secret_text() -> TestResult<()> {
    for case in [
        EnvironmentFailureCase {
            fixture: &SERVER,
            name: "TYPOL_NET_PORT",
            bytes: b"65536",
            code: DiagnosticCode::CONFIG_VALUE_ERROR,
        },
        EnvironmentFailureCase {
            fixture: &SERVER,
            name: "TYPOL_WORKER_THREADS",
            bytes: b"0",
            code: DiagnosticCode::CONFIG_VALIDATION_ERROR,
        },
        EnvironmentFailureCase {
            fixture: &SERVER,
            name: "TYPOL_CLIENT_SECRET",
            bytes: b"short",
            code: DiagnosticCode::CONFIG_VALIDATION_ERROR,
        },
        EnvironmentFailureCase {
            fixture: &DATABASE,
            name: "TYPOL_DB_HOST",
            bytes: b"999.1.2.3",
            code: DiagnosticCode::CONFIG_VALIDATION_ERROR,
        },
        EnvironmentFailureCase {
            fixture: &DATABASE,
            name: "TYPOL_DB_PORT",
            bytes: b"",
            code: DiagnosticCode::CONFIG_VALUE_ERROR,
        },
        EnvironmentFailureCase {
            fixture: &DATABASE,
            name: "TYPOL_DB_PASSWORD",
            bytes: &[0xff],
            code: DiagnosticCode::CONFIG_ENVIRONMENT_ERROR,
        },
        EnvironmentFailureCase {
            fixture: &ALL_TYPES,
            name: "TYPOL_SAMPLE_ACTIVE",
            bytes: b"false trailing",
            code: DiagnosticCode::CONFIG_VALUE_ERROR,
        },
        EnvironmentFailureCase {
            fixture: &ALL_TYPES,
            name: "TYPOL_SAMPLE_SLUG",
            bytes: b"INVALID",
            code: DiagnosticCode::CONFIG_VALIDATION_ERROR,
        },
        EnvironmentFailureCase {
            fixture: &ALL_TYPES,
            name: "TYPOL_SECRET_TOKEN",
            bytes: b"",
            code: DiagnosticCode::CONFIG_VALIDATION_ERROR,
        },
    ] {
        let schema = compile(case.fixture)?;
        let mut provider = MemoryEnvironment::required(case.fixture);
        provider.set(case.name, case.bytes);
        let errors = parse(
            case.fixture,
            &schema,
            case.fixture.config,
            &mut provider,
            &ResourceLimits::default(),
        )
        .test_error()?;
        assert_code(&errors, case.code)?;
        test_eq!(provider.calls.get(case.name).copied().test_value()?, 1)?;
        for diagnostic in errors.diagnostics() {
            test_eq!(
                diagnostic.value_source(),
                Some(ValueSource::EnvironmentVariable)
            )?;
            test_assert!(
                errors
                    .span_location(diagnostic.primary().test_value()?)
                    .is_some()
            )?;
            if case.name == case.fixture.required || case.name == "TYPOL_SECRET_TOKEN" {
                test_eq!(diagnostic.message(), "value is hidden")?;
                test_assert!(!format!("{errors:?}").contains("short"))?;
            }
        }
    }

    Ok(())
}

#[test]
fn shared_environment_is_read_once_but_each_field_checks_its_own_rules() -> TestResult<()> {
    let schema = compile(&SERVER)?;
    for (bytes, expected_fields) in [
        (b"ab".as_slice(), vec!["app_name"]),
        (b"invalid.name", vec!["app_name"]),
        (b"", vec!["app_name", "app_name", "client_id"]),
    ] {
        let mut provider = MemoryEnvironment::required(&SERVER);
        provider.set("TYPOL_APP_NAME", bytes);
        let errors = parse(
            &SERVER,
            &schema,
            SERVER.config,
            &mut provider,
            &ResourceLimits::default(),
        )
        .test_error()?;
        assert_code(&errors, DiagnosticCode::CONFIG_VALIDATION_ERROR)?;
        let fields: Vec<_> = errors
            .diagnostics()
            .iter()
            .map(|d| -> TestResult<_> {
                d.path()
                    .test_value()?
                    .segments()
                    .last()
                    .test_value()?
                    .as_field()
                    .test_value()
            })
            .collect::<TestResult<Vec<_>>>()?;
        test_eq!(fields, expected_fields)?;
        test_eq!(
            provider.calls.get("TYPOL_APP_NAME").copied().test_value()?,
            1
        )?;
    }

    Ok(())
}

#[test]
fn unauthorized_names_and_invalid_fallbacks_fail_before_environment_reads() -> TestResult<()> {
    let schema = compile(&SERVER)?;
    for case in [
        FallbackCase {
            old: "app_name: env(\"TYPOL_APP_NAME\", \"gateway-core\")",
            new: "app_name: env(\"typol_app_name\", \"gateway-core\")",
            code: DiagnosticCode::CONFIG_VALUE_ERROR,
        },
        FallbackCase {
            old: "app_name: env(\"TYPOL_APP_NAME\", \"gateway-core\")",
            new: "app_name: env(\"UNAUTHORIZED\", \"gateway-core\")",
            code: DiagnosticCode::CONFIG_VALUE_ERROR,
        },
        FallbackCase {
            old: "app_name: env(\"TYPOL_APP_NAME\", \"gateway-core\")",
            new: "app_name: env(\"TYPOL_APP_NAME\", \"INVALID\")",
            code: DiagnosticCode::CONFIG_VALIDATION_ERROR,
        },
    ] {
        let text = replace(&SERVER, case.old, case.new)?;
        let mut provider = MemoryEnvironment::required(&SERVER);
        provider.set("TYPOL_APP_NAME", b"edge-gateway");
        let mut limits = ConfigLimits::default();
        limits = limits.with_max_diagnostics(1);
        let errors = parse(&SERVER, &schema, &text, &mut provider, &limits).test_error()?;
        assert_code(&errors, case.code)?;
        test_assert!(errors.is_truncated())?;
        test_assert!(provider.calls.is_empty())?;
        let errors = parse(
            &SERVER,
            &schema,
            &text,
            &mut provider,
            &ResourceLimits::default(),
        )
        .test_error()?;
        assert_code(&errors, case.code)?;
        test_assert!(!provider.calls.contains_key("typol_app_name"))?;
        test_assert!(!provider.calls.contains_key("UNAUTHORIZED"))?;
        // The later `client_id` still authorizes reading the same name, which is read once at that location after recovery.
        test_eq!(
            provider.calls.get("TYPOL_APP_NAME").copied().test_value()?,
            1
        )?;
    }

    Ok(())
}

#[test]
fn fixture_counterexamples_reject_collections_and_complete_tree_assertions() -> TestResult<()> {
    for case in [
        Counterexample {
            fixture: &SERVER,
            old: "host: \"10.0.0.12\"",
            new: "host: \"10.0.0.11\"",
            assertion: false,
        },
        Counterexample {
            fixture: &SERVER,
            old: "\"127.0.0.1/32\"",
            new: "\"0.0.0.0/0\"",
            assertion: false,
        },
        Counterexample {
            fixture: &SERVER,
            old: "host: \"10.0.0.11\"",
            new: "host: \"0.0.0.0\"",
            assertion: true,
        },
        Counterexample {
            fixture: &SERVER,
            old: "host: \"10.0.0.10\"",
            new: "host: \"0.0.0.0\"",
            assertion: true,
        },
        Counterexample {
            fixture: &DATABASE,
            old: "host: \"db-replica-02.prod.internal\"",
            new: "host: \"db-replica-01.prod.internal\"",
            assertion: false,
        },
        Counterexample {
            fixture: &DATABASE,
            old: "read_only_routing: true",
            new: "read_only_routing: false",
            assertion: true,
        },
        Counterexample {
            fixture: &DATABASE,
            old: "min_idle_connections: 20",
            new: "min_idle_connections: 201",
            assertion: true,
        },
        Counterexample {
            fixture: &ALL_TYPES,
            old: "dynamic_scores: [99.5, 87.0, 92.3, 76.8]",
            new: "dynamic_scores: []",
            assertion: false,
        },
        Counterexample {
            fixture: &ALL_TYPES,
            old: "dynamic_scores: [99.5, 87.0, 92.3, 76.8]",
            new: "dynamic_scores: [-1.0]",
            assertion: false,
        },
        Counterexample {
            fixture: &ALL_TYPES,
            old: "dynamic_scores: [99.5, 87.0, 92.3, 76.8]",
            new: "dynamic_scores: [300.0, 300.0]",
            assertion: false,
        },
        Counterexample {
            fixture: &ALL_TYPES,
            old: "id: 2",
            new: "id: 1",
            assertion: false,
        },
    ] {
        let schema = compile(case.fixture)?;
        let text = replace(case.fixture, case.old, case.new)?;
        let mut provider = MemoryEnvironment::required(case.fixture);
        let errors = parse(
            case.fixture,
            &schema,
            &text,
            &mut provider,
            &ResourceLimits::default(),
        )
        .test_error()?;
        assert_code(&errors, DiagnosticCode::CONFIG_VALIDATION_ERROR)?;
        for diagnostic in errors.diagnostics() {
            test_assert!(
                errors
                    .span_location(diagnostic.primary().test_value()?)
                    .is_some()
            )?;
            test_assert!(!diagnostic.labels().is_empty())?;
            for label in diagnostic.labels() {
                test_assert!(errors.span_location(label.span()).is_some())?;
            }
            if case.assertion {
                test_eq!(diagnostic.value_source(), Some(ValueSource::ConfigLiteral))?;
            }
        }
    }

    Ok(())
}

#[test]
fn map_failures_keep_source_order_and_diagnostic_budget_stops_later_reads() -> TestResult<()> {
    let schema = compile(&ALL_TYPES)?;
    let text = replace(
        &ALL_TYPES,
        "labels: {\"tier\": \"production\"}",
        "labels: {\"z\": \"\", \"a\": \"\"}",
    )?;
    for budget in [0, 1, 2, 3] {
        let mut limits = ConfigLimits::default();
        limits = limits.with_max_diagnostics(budget);
        let mut provider = MemoryEnvironment::required(&ALL_TYPES);
        let errors = parse(&ALL_TYPES, &schema, &text, &mut provider, &limits).test_error()?;
        test_eq!(errors.diagnostics().len(), budget.clamp(1, 2))?;
        if budget <= 1 {
            assert_code(&errors, DiagnosticCode::CONFIG_VALIDATION_ERROR)?;
            test_assert!(errors.is_truncated())?;
            test_assert!(!provider.calls.contains_key("TYPOL_NOTIFY_TOKEN"))?;
        } else if budget == 2 {
            test_eq!(
                errors.diagnostics().last().test_value()?.code(),
                DiagnosticCode::DIAGNOSTIC_LIMIT_REACHED
            )?;
            test_assert!(errors.is_truncated())?;
            test_assert!(!provider.calls.contains_key("TYPOL_NOTIFY_TOKEN"))?;
        } else {
            assert_code(&errors, DiagnosticCode::CONFIG_VALIDATION_ERROR)?;
            test_assert!(!errors.is_truncated())?;
            let keys: Vec<_> = errors
                .diagnostics()
                .iter()
                .map(|d| -> TestResult<_> {
                    d.path()
                        .test_value()?
                        .segments()
                        .last()
                        .test_value()?
                        .as_map_key()
                        .test_value()
                })
                .collect::<TestResult<Vec<_>>>()?;
            test_eq!(keys, ["z", "a"])?;
            let positions: Vec<_> = errors
                .diagnostics()
                .iter()
                .map(|d| Ok(d.primary().test_value()?.span().start()))
                .collect::<TestResult<Vec<_>>>()?;
            test_assert!(TestCheck::item(&positions, 0)? < TestCheck::item(&positions, 1)?)?;
            test_eq!(
                provider
                    .calls
                    .get("TYPOL_NOTIFY_TOKEN")
                    .copied()
                    .test_value()?,
                1
            )?;
        }
    }

    Ok(())
}

#[test]
fn imported_schema_labels_and_provider_failures_use_public_owned_errors() -> TestResult<()> {
    let schema = compile(&SERVER)?;
    let mut provider = MemoryEnvironment::required(&SERVER);
    provider.set("TYPOL_NET_HOST", b"999.1.2.3");
    let errors = parse(
        &SERVER,
        &schema,
        SERVER.config,
        &mut provider,
        &ResourceLimits::default(),
    )
    .test_error()?;
    let diagnostic = TestCheck::item(errors.diagnostics(), 0)?;
    assert_code(&errors, DiagnosticCode::CONFIG_VALIDATION_ERROR)?;
    test_assert!(
        diagnostic
            .labels()
            .iter()
            .any(|label| label.span().source_id() == SourceId::new(1))
    )?;
    for label in diagnostic.labels() {
        test_assert!(errors.span_location(label.span()).is_some())?;
    }
    for fixture in [&SERVER, &DATABASE, &ALL_TYPES] {
        let schema = compile(fixture)?;
        for (error, code) in [
            (
                EnvironmentError::HostFailure,
                DiagnosticCode::CONFIG_ENVIRONMENT_ERROR,
            ),
            (
                EnvironmentError::AllocationFailed,
                DiagnosticCode::ALLOCATION_FAILED,
            ),
            (
                EnvironmentError::ResourceLimitExceeded,
                DiagnosticCode::RESOURCE_LIMIT_EXCEEDED,
            ),
        ] {
            let mut provider = MemoryEnvironment {
                error: Some(error),
                ..MemoryEnvironment::required(fixture)
            };
            let errors = parse(
                fixture,
                &schema,
                fixture.config,
                &mut provider,
                &ResourceLimits::default(),
            )
            .test_error()?;
            assert_code(&errors, code)?;
            if error != EnvironmentError::HostFailure {
                test_eq!(provider.calls.len(), 1)?;
            }
        }
    }

    Ok(())
}

#[test]
fn fixture_resource_boundaries_accept_exact_limit_and_reject_one_less() -> TestResult<()> {
    for fixture in [&SERVER, &DATABASE, &ALL_TYPES] {
        let schema = compile(fixture)?;
        let config_builders: [ConfigLimitBuilder; 4] = [
            (ConfigLimits::with_max_source_bytes, fixture.config.len()),
            (ConfigLimits::with_max_string_bytes, 256),
            (ConfigLimits::with_max_value_nodes, 512),
            (ConfigLimits::with_max_nesting_depth, 32),
        ];
        for (with_limit, upper) in config_builders {
            let run = |bound| {
                let config = with_limit(ConfigLimits::default(), bound);
                let resources = ResourceLimits::default();
                parse(
                    fixture,
                    &schema,
                    fixture.config,
                    &mut MemoryEnvironment::required(fixture),
                    &(&resources, &config),
                )
            };
            run(upper).test_value()?;
            let (mut low, mut high) = (0, upper);
            while low < high {
                let middle = low + (high - low) / 2;
                match run(middle) {
                    TestOpeoOutcome::Value(_) => high = middle,
                    TestOpeoOutcome::Error(errors) => {
                        assert_code(&errors, DiagnosticCode::RESOURCE_LIMIT_EXCEEDED)?;
                        low = middle + 1;
                    }
                }
            }
            test_assert!(low > 0)?;
            run(low).test_value()?;
            assert_code(
                &run(low - 1).test_error()?,
                DiagnosticCode::RESOURCE_LIMIT_EXCEEDED,
            )?;
        }
        let resource_setters = [
            ResourceLimitSetter {
                set: |limits, value| limits.regex_mut().set_max_regex_search_steps(value),
                upper: 4096,
            },
            ResourceLimitSetter {
                set: |limits, value| limits.regex_mut().set_max_regex_scratch_states(value),
                upper: 4096,
            },
        ];
        for setter in resource_setters {
            let run = |bound| {
                let config = ConfigLimits::default();
                let mut resources = ResourceLimits::default();
                (setter.set)(&mut resources, bound);
                parse(
                    fixture,
                    &schema,
                    fixture.config,
                    &mut MemoryEnvironment::required(fixture),
                    &(&resources, &config),
                )
            };
            run(setter.upper).test_value()?;
            let (mut low, mut high) = (0, setter.upper);
            while low < high {
                let middle = low + (high - low) / 2;
                match run(middle) {
                    TestOpeoOutcome::Value(_) => high = middle,
                    TestOpeoOutcome::Error(errors) => {
                        assert_code(&errors, DiagnosticCode::RESOURCE_LIMIT_EXCEEDED)?;
                        low = middle + 1;
                    }
                }
            }
            test_assert!(low > 0)?;
            run(low).test_value()?;
            assert_code(
                &run(low - 1).test_error()?,
                DiagnosticCode::RESOURCE_LIMIT_EXCEEDED,
            )?;
        }
        let limits = ConfigLimits::default().with_max_source_bytes(fixture.config.len() - 1);
        let mut provider = MemoryEnvironment::required(fixture);
        assert_code(
            &parse(fixture, &schema, fixture.config, &mut provider, &limits).test_error()?,
            DiagnosticCode::RESOURCE_LIMIT_EXCEEDED,
        )?;
        provider.assert_cached(fixture)?;
    }

    Ok(())
}

fn config_request<'a>(
    source: (&'a mut impl Read, &'a str),
    limits: impl Into<TestLimits>,
) -> ConfigParseRequest<'a> {
    let limits = limits.into();
    ConfigParseRequest::new(
        source.0,
        source.1,
        ConfigParseOptions::new(
            limits.resources(),
            limits.config(),
            ExecutionLimits::new(1_000_000_000, 268_435_456),
        ),
    )
}

fn snapshot<'a>(input: impl Into<ValueViewInput<'a>>) -> String {
    let value = input.into().value_view();
    let content = match value.as_ref() {
        ValueKind::Signed(number) => format!("i{number}"),
        ValueKind::Unsigned(number) => format!("u{number}"),
        ValueKind::Float32(number) => format!("f32:{}", number.to_bits()),
        ValueKind::Float64(number) => format!("f64:{}", number.to_bits()),
        ValueKind::String(text) => format!("{text:?}"),
        ValueKind::Bool(value) => format!("b{value}"),
        ValueKind::Optional(value) => value.map_or_else(|| "none".into(), snapshot),
        ValueKind::Array(values) => values
            .map(|entry| snapshot(entry.value()))
            .collect::<Vec<_>>()
            .join(","),
        ValueKind::Map(entries) => entries
            .map(|entry| {
                format!(
                    "{:?}:{}",
                    entry.name().unwrap_or_default(),
                    snapshot(entry.value())
                )
            })
            .collect::<Vec<_>>()
            .join(","),
        ValueKind::Structure(fields) => fields
            .map(|field| {
                format!(
                    "{}:{}",
                    field.name().unwrap_or_default(),
                    snapshot(field.value())
                )
            })
            .collect::<Vec<_>>()
            .join(","),
        ValueKind::Enumeration { variant, payload } => {
            let payload = match payload {
                ValuePayloadView::Unit => "unit".into(),
                ValuePayloadView::Single(value) => snapshot(value),
                ValuePayloadView::Structure(fields) => fields
                    .map(|field| {
                        format!(
                            "{}:{}",
                            field.name().unwrap_or_default(),
                            snapshot(field.value())
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(","),
            };
            format!(".{variant}({payload})")
        }
    };
    format!("{:?}:{content}", value.source())
}

struct FragmentSource<'a> {
    bytes: &'a [u8],
    offset: usize,
    cuts: Vec<usize>,
    next_cut: usize,
}

impl Read for FragmentSource<'_> {
    fn read<'slot, 'borrow>(
        &mut self,
        output: &mut [u8],
        source_id: SourceId,
        offset: usize,
        out: Out<'slot, 'borrow, SourceReadError>,
    ) -> OResult<'slot, usize> {
        self.offset = offset;
        while self
            .cuts
            .get(self.next_cut)
            .is_some_and(|cut| *cut <= self.offset)
        {
            self.next_cut += 1;
        }
        let end = self
            .cuts
            .get(self.next_cut)
            .copied()
            .unwrap_or(self.bytes.len());
        let count = output.len().min(end - self.offset);
        let Some(end) = self.offset.checked_add(count) else {
            return Err(out.fail(
                SourceReadError::new(SourceReadErrorKind::InternalInvariantFailure)
                    .with_input(source_id, offset),
            ));
        };
        let Some(target) = output.get_mut(..count) else {
            return Err(out.fail(
                SourceReadError::new(SourceReadErrorKind::InternalInvariantFailure)
                    .with_input(source_id, offset),
            ));
        };
        let Some(source) = self.bytes.get(self.offset..end) else {
            return Err(out.fail(
                SourceReadError::new(SourceReadErrorKind::InternalInvariantFailure)
                    .with_input(source_id, offset),
            ));
        };
        target.copy_from_slice(source);
        self.offset += count;
        Ok(count)
    }
}

#[test]
fn original_fixtures_preserve_values_origins_and_environment_cache_at_every_split() -> TestResult<()>
{
    for fixture in [&SERVER, &DATABASE, &ALL_TYPES] {
        let schema = compile(fixture)?;
        let baseline = parse(
            fixture,
            &schema,
            fixture.config,
            &mut MemoryEnvironment::required(fixture),
            &ResourceLimits::default(),
        )
        .test_value()?;
        let expected = snapshot(&baseline);
        let check = |cuts, chunk| -> TestResult<()> {
            let mut input = FragmentSource {
                bytes: fixture.config.as_bytes(),
                offset: 0,
                cuts,
                next_cut: 0,
            };
            let mut provider = MemoryEnvironment::required(fixture);
            let value = Support::parse(
                &schema,
                ConfigParseRequest::new(
                    &mut input,
                    fixture.name,
                    ConfigParseOptions::default()
                        .with_stream(StreamLimits::try_new(chunk, 1_048_576).test_value()?),
                ),
                &mut provider,
            )
            .test_value()?;
            test_eq!(snapshot(&value), expected)?;
            provider.assert_cached(fixture)?;
            Ok(())
        };
        for cut in 1..fixture.config.len() {
            check(vec![cut], 8192)?;
        }
        for chunk in [1, 2, 3, 7, 8192] {
            check((1..fixture.config.len()).collect(), chunk)?;
        }
        let cuts = (1..fixture.config.len())
            .filter(|index| (index * 1103515245 + 12345) % 17 < 3)
            .collect();
        check(cuts, 7)?;
    }

    Ok(())
}
