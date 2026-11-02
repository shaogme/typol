use renamed_typol::{
    ConfigInput, ConfigParseOptions, Environment, EnvironmentError, EnvironmentReadContext, Typol,
    LoadedSchema, ProcessEnvironment, ProcessEnvironmentLimits, SchemaLoader, TypolError,
    TypolOptions, TypolStage, ValidatedConfigValue,
    binding::*, config::*, core::*, host::*, schema::*, semantic::*,
    opeo::{OResult, Out},
};
use std::{
    collections::{BTreeMap, HashMap},
    error::Error,
    fmt::Debug,
    fs::write,
    path::Path,
};
use typol_test_support::TestOpeo;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize, de::DeserializeOwned};

#[derive(Debug, PartialEq, Typol)]
#[typol(crate = "renamed_typol")]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
struct Endpoint {
    host: String,
    port: u16,
}

#[derive(Debug, PartialEq, Typol)]
#[typol(crate = "renamed_typol")]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
enum Protocol {
    Tcp,
    Udp,
    Tls(TlsConfig),
}

#[derive(Debug, PartialEq, Typol)]
#[typol(crate = "renamed_typol")]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
struct TlsConfig {
    cert_path: String,
    key_path: String,
    insecure_skip_verify: bool,
}

#[derive(Debug, PartialEq, Typol)]
#[typol(crate = "renamed_typol")]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
    Fatal,
}

#[derive(Debug, PartialEq, Typol)]
#[typol(crate = "renamed_typol")]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
enum OutputSink {
    Console,
    File(String),
    Remote(Endpoint),
}

#[derive(Debug, PartialEq, Typol)]
#[typol(crate = "renamed_typol")]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
struct LogConfig {
    level: LogLevel,
    sink: OutputSink,
    enable_json: bool,
    max_size_mb: u32,
}

#[derive(Debug, PartialEq, Typol)]
#[typol(crate = "renamed_typol")]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
enum StorageEngine {
    Memory,
    LocalDisk(String),
    S3 {
        bucket: String,
        region: String,
        endpoint: Option<String>,
    },
}

#[derive(Debug, PartialEq, Typol)]
#[typol(crate = "renamed_typol")]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
struct PoolConfig {
    threads: u16,
    priority: i8,
    queue_capacity: u32,
}

#[derive(Debug, PartialEq, Typol)]
#[typol(crate = "renamed_typol")]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
struct ResourceQuota {
    max_memory_bytes: u64,
    cpu_percentage: f32,
    sample_rate: f64,
    perf_budget_ns: f64,
}

#[derive(Debug, PartialEq, Typol)]
#[typol(crate = "renamed_typol")]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
enum NodeRole {
    Standalone,
    Leader,
    Follower {
        leader_endpoint: Endpoint,
        sync_timeout_ms: u32,
    },
}

#[derive(Debug, PartialEq, Typol)]
#[typol(crate = "renamed_typol")]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
struct Topology {
    node_id: u32,
    role: NodeRole,
    quorum_nodes: [u32; 3],
    peers: Vec<Endpoint>,
}

#[derive(Debug, PartialEq, Typol)]
#[typol(crate = "renamed_typol")]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
enum Credential {
    None,
    ApiToken(String),
    ClientCert(TlsConfig),
    OAuth2 {
        client_id: String,
        client_secret: String,
        token_url: String,
    },
}

#[derive(Debug, PartialEq, Typol)]
#[typol(crate = "renamed_typol")]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
struct SecurityPolicy {
    credential: Credential,
    pre_shared_key: [u8; 16],
    admin_email: Option<String>,
    backup_phone: Option<String>,
    ip_whitelist: Vec<String>,
}

#[derive(Debug, PartialEq, Typol)]
#[typol(crate = "renamed_typol")]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
struct BenchmarkStats {
    signed_8: i8,
    signed_16: i16,
    signed_32: i32,
    signed_64: i64,
    signed_128: i128,
    unsigned_8: u8,
    unsigned_16: u16,
    unsigned_32: u32,
    unsigned_64: u64,
    unsigned_128: u128,
    float_32: f32,
    float_64: f64,
}

#[derive(Debug, PartialEq, Typol)]
#[typol(crate = "renamed_typol")]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
struct ServerConfig {
    app_name: String,
    listen_endpoint: Endpoint,
    protocol: Protocol,
    logging: LogConfig,
    storage: StorageEngine,
    worker_pool: PoolConfig,
    resource_limits: ResourceQuota,
    cluster: Topology,
    security: SecurityPolicy,
    benchmark: BenchmarkStats,
    description: Option<String>,
}

#[derive(Debug, PartialEq, Typol)]
#[typol(crate = "renamed_typol")]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
struct Address {
    host: String,
    port: u16,
}

#[derive(Debug, PartialEq, Typol)]
#[typol(crate = "renamed_typol")]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
enum DriverType {
    Sqlite(String),
    Postgres,
    Mysql,
    DistributedCluster {
        shard_key: String,
        read_write_splitting: bool,
        max_route_nodes: u16,
    },
}

#[derive(Debug, PartialEq, Typol)]
#[typol(crate = "renamed_typol")]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
struct Auth {
    username: String,
    password: String,
    database_name: String,
}

#[derive(Debug, PartialEq, Typol)]
#[typol(crate = "renamed_typol")]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
struct PoolLimits {
    max_connections: u32,
    min_idle_connections: u32,
    idle_timeout_ms: u64,
    connect_timeout_ms: u32,
}

#[derive(Debug, PartialEq, Typol)]
#[typol(crate = "renamed_typol")]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
enum IsolationLevel {
    ReadUncommitted,
    ReadCommitted,
    RepeatableRead,
    Serializable,
}

#[derive(Debug, PartialEq, Typol)]
#[typol(crate = "renamed_typol")]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
struct DatabaseClusterConfig {
    primary_host: Address,
    read_replicas: Vec<Address>,
    read_only_routing: bool,
    driver: DriverType,
    auth: Option<Auth>,
    pool: PoolLimits,
    default_isolation: IsolationLevel,
    slow_query_threshold_ms: u32,
}

#[derive(Debug, PartialEq, Typol)]
#[typol(crate = "renamed_typol")]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
struct IntegerSuite {
    val_i8: i8,
    val_i16: i16,
    val_i32: i32,
    val_i64: i64,
    val_i128: i128,
    val_u8: u8,
    val_u16: u16,
    val_u32: u32,
    val_u64: u64,
    val_u128: u128,
}

#[derive(Debug, PartialEq, Typol)]
#[typol(crate = "renamed_typol")]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
struct FloatSuite {
    val_f32: f32,
    val_f64: f64,
}

#[derive(Debug, PartialEq, Typol)]
#[typol(crate = "renamed_typol")]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
struct TextAndFlagSuite {
    plain_text: String,
    bounded_text: String,
    slug: String,
    contact_email: String,
    is_active: bool,
}

#[derive(Debug, PartialEq, Typol)]
#[typol(crate = "renamed_typol")]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
struct TagItem {
    id: u32,
    name: String,
    labels: HashMap<String, String>,
}

#[derive(Debug, PartialEq, Typol)]
#[typol(crate = "renamed_typol")]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
struct CollectionSuite {
    fixed_ipv4_bytes: [u8; 4],
    fixed_pair: [String; 2],
    dynamic_scores: Vec<f64>,
    dynamic_tags: Vec<TagItem>,
    optional_int: Option<i64>,
    optional_token: Option<String>,
    optional_tag: Option<TagItem>,
    disabled_feature: Option<String>,
}

#[derive(Debug, PartialEq, Typol)]
#[typol(crate = "renamed_typol")]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
struct WebhookTarget {
    url: String,
    timeout_ms: u32,
}

#[derive(Debug, PartialEq, Typol)]
#[typol(crate = "renamed_typol")]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
enum NotificationChannel {
    Mute,
    Console,
    Email(String),
    Webhook(WebhookTarget),
    CustomHttp {
        endpoint: String,
        bearer_token: String,
        retry_limit: u8,
    },
}

#[derive(Debug, PartialEq, Typol)]
#[typol(crate = "renamed_typol")]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
struct MetaTag {
    key: String,
    value: String,
}

#[derive(Debug, PartialEq, Typol)]
#[typol(crate = "renamed_typol")]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
struct AllTypesConfig {
    integers: IntegerSuite,
    floats: FloatSuite,
    texts: TextAndFlagSuite,
    collections: CollectionSuite,
    notification: NotificationChannel,
    meta: MetaTag,
}

const SECRET: &str = "fixture-secret-value";

fn invoke<E, T>(
    operation: impl for<'slot> FnOnce(Out<'slot, 'slot, E>) -> OResult<'slot, T>,
) -> T
where
    E: Debug,
{
    match TestOpeo::call(operation) {
        Ok(value) => value,
        Err(error) => panic!("OPEO call failed: {error:?}"),
    }
}

fn invoke_error<E, T>(
    operation: impl for<'slot> FnOnce(Out<'slot, 'slot, E>) -> OResult<'slot, T>,
) -> E
where
    E: Debug,
{
    match TestOpeo::error(operation) {
        Ok(error) => error,
        Err(failure) => panic!("{failure}"),
    }
}

struct Resolver;

impl SchemaResolver for Resolver {
    fn resolve<'slot, 'borrow, 'src>(
        &'src mut self,
        request: SchemaResolveRequest<'_>,
        out: Out<'slot, 'borrow, ResolverError>,
    ) -> OResult<'slot, SchemaSource<'src>> {
        let from = request.from_identity;
        let name = request.module_name;
        let max = request.source_byte_limit;
        assert!(from == b"server" || from == b"database");
        assert_eq!(name, "common");
        let text = include_str!("../fixtures/common.tschem");
        if text.len() > max {
            return Err(out.fail(ResolverError::ResourceLimitExceeded));
        }
        Ok(SchemaSource::from_str(b"common", text))
    }
}

#[derive(Default)]
struct Provider {
    calls: BTreeMap<String, usize>,
    deny: bool,
}

impl Environment for Provider {
    fn read<'slot, 'borrow, 'env>(
        &'env mut self,
        name: &str,
        _: EnvironmentReadContext<'_>,
        _: Out<'slot, 'borrow, EnvironmentError>,
    ) -> OResult<'slot, Option<&'env [u8]>> {
        assert!(!self.deny, "frozen text should not read the environment");
        *self.calls.entry(name.into()).or_default() += 1;
        Ok(matches!(
            name,
            "TYPOL_CLIENT_SECRET" | "TYPOL_DB_PASSWORD" | "TYPOL_NOTIFY_TOKEN"
        )
        .then_some(SECRET.as_bytes()))
    }
}

#[cfg(not(feature = "serde"))]
trait Target: BudgetedTypol + Debug + PartialEq {}

#[cfg(not(feature = "serde"))]
impl<T: BudgetedTypol + Debug + PartialEq> Target for T {}

#[cfg(feature = "serde")]
trait Target: BudgetedTypol + Debug + PartialEq + Serialize + DeserializeOwned {}

#[cfg(feature = "serde")]
impl<T: BudgetedTypol + Debug + PartialEq + Serialize + DeserializeOwned> Target for T {}

fn read_typed<'slot, T: Target>(
    loaded: &LoadedSchema,
    input: ConfigInput<'_>,
    mut out: Out<'slot, '_, TypolError>,
) -> OResult<'slot, T> {
    let bound = loaded.bind::<T>(out.reborrow())?;
    let mut environment = ProcessEnvironment::new(ProcessEnvironmentLimits::default());
    bound.read(input, &mut environment, out)
}

fn read_value_with_process_environment<'slot>(
    loaded: &LoadedSchema,
    input: ConfigInput<'_>,
    out: Out<'slot, '_, TypolError>,
) -> OResult<'slot, ValidatedConfigValue> {
    let mut environment = ProcessEnvironment::new(ProcessEnvironmentLimits::default());
    loaded.read_value(input, &mut environment, out)
}

fn read_typed_with_environment<'slot, T: Target>(
    loaded: &LoadedSchema,
    input: ConfigInput<'_>,
    environment: &mut dyn Environment,
    mut out: Out<'slot, '_, TypolError>,
) -> OResult<'slot, T> {
    loaded
        .bind::<T>(out.reborrow())?
        .read(input, environment, out)
}

#[cfg(feature = "serde")]
fn read_serde<'slot, T: Target>(
    loaded: &LoadedSchema,
    input: ConfigInput<'_>,
    mut out: Out<'slot, '_, TypolError>,
) -> OResult<'slot, T> {
    let bound = loaded.bind::<T>(out.reborrow())?;
    let mut environment = ProcessEnvironment::new(ProcessEnvironmentLimits::default());
    bound.read_serde(input, &mut environment, out)
}

#[cfg(feature = "serde")]
fn read_serde_with_environment<'slot, T: Target>(
    loaded: &LoadedSchema,
    input: ConfigInput<'_>,
    environment: &mut dyn Environment,
    mut out: Out<'slot, '_, TypolError>,
) -> OResult<'slot, T> {
    loaded
        .bind::<T>(out.reborrow())?
        .read_serde(input, environment, out)
}

fn request<'a>(input: &'a mut SliceSource<'_>) -> ConfigParseRequest<'a> {
    ConfigParseRequest::new(input, "raw acceptance config", ConfigParseOptions::default())
}

fn compile(name: &str, text: &str) -> CompiledSchema {
    let compiler = invoke(|out| {
        SchemaCompiler::new(SchemaCompileOptions::default(), out)
    });
    invoke(|out| {
        compiler.compile(
            SchemaSource::from_str(name.as_bytes(), text),
            &mut Resolver,
            None,
            out,
        )
    })
}

fn reordered(text: &str) -> String {
    let content = &text[1..text.len() - 1];
    let mut quoted = false;
    let mut escaped = false;
    let mut depth = 0;
    let mut start = 0;
    let mut fields = Vec::new();
    for (index, character) in content.char_indices() {
        if quoted {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                quoted = false;
            }
            continue;
        }
        match character {
            '"' => quoted = true,
            '{' | '[' | '(' => depth += 1,
            '}' | ']' | ')' => depth -= 1,
            ',' if depth == 0 => {
                fields.push(content[start..index].trim());
                start = index + 1;
            }
            _ => {}
        }
    }
    fields.push(content[start..].trim());
    fields.reverse();
    format!("{{{}}}", fields.join(", "))
}

fn roundtrip<T: Target>(binding: &TypedSchema<'_, T>, input: &str, calls: usize) -> T {
    let mut provider = Provider::default();
    let value = invoke(|out| binding.deserialize(
            request(&mut SliceSource::new((input).as_bytes())),
            &mut provider,
            ConversionLimits::default(),
            out,
        ));
    assert_eq!(provider.calls.len(), calls);
    assert!(provider.calls.values().all(|count| *count == 1));
    let text = invoke(|out| binding.serialize(&value, CodecOptions::default(), out));
    assert!(text.contains(SECRET));
    assert!(!text.contains("env("));
    let decoded = invoke(|out| binding.deserialize(
            request(&mut SliceSource::new(text.as_bytes())),
            &mut Provider {
                deny: true,
                ..Provider::default()
            },
            ConversionLimits::default(),
            out,
        ));
    assert_eq!(value, decoded);
    let reordered = reordered(&text);
    assert_eq!(
        value,
        invoke(|out| binding.deserialize(
                request(&mut SliceSource::new(reordered.as_bytes())),
                &mut Provider {
                    deny: true,
                    ..Provider::default()
                },
                ConversionLimits::default(),
                out,
            ))
    );

    assert_eq!(
        text,
        invoke(|out| binding.serialize(&decoded, CodecOptions::default(), out))
    );
    let defaults = CodecOptions::default();
    assert_eq!(
        text,
        invoke(|out| binding.serialize(&value, defaults.with_max_output_bytes(text.len()), out))
    );
    let errors = invoke_error(|out| {
        binding.serialize(&value, defaults.with_max_output_bytes(text.len() - 1), out)
    });
    assert_eq!(
        errors.diagnostics().last().unwrap().code(),
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
    );
    #[cfg(feature = "serde")]
    {
        let mut provider = Provider::default();
        let serde_value = invoke(|out| {
            binding.deserialize_serde(
                request(&mut SliceSource::new(input.as_bytes())),
                &mut provider,
                ConversionLimits::default(),
                out,
            )
        });
        assert_eq!(value, serde_value);
        assert_eq!(
            value,
            invoke(|out| binding.deserialize_serde(
                    request(&mut SliceSource::new(reordered.as_bytes())),
                    &mut Provider {
                        deny: true,
                        ..Provider::default()
                    },
                    ConversionLimits::default(),
                    out,
                ))
        );

        assert_eq!(provider.calls.len(), calls);
        assert!(provider.calls.values().all(|count| *count == 1));
        assert_eq!(
            text,
            invoke(|out| binding.serialize_serde(&serde_value, CodecOptions::default(), out))
        );
        assert_eq!(
            value,
            invoke(|out| binding.deserialize_serde(
                    request(&mut SliceSource::new(text.as_bytes())),
                    &mut Provider {
                        deny: true,
                        ..Provider::default()
                    },
                    ConversionLimits::default(),
                    out,
                ))
        );
    }
    value
}

fn reject<T: Target>(binding: &TypedSchema<'_, T>, value: &T) -> CodecErrors {
    let errors = invoke_error(|out| {
        binding.serialize(value, CodecOptions::default(), out)
    });
    assert_eq!(errors.stage(), CodecStage::Validation);
    assert!(errors.diagnostics().iter().all(|d| d.primary().is_none()));
    assert!(!format!("{errors:?}{errors}").contains(SECRET));
    for label in errors.diagnostics().iter().flat_map(|d| d.labels()) {
        assert!(errors.span_location(label.span()).is_some());
    }
    #[cfg(feature = "serde")]
    {
        let serde_errors = invoke_error(|out| {
            binding.serialize_serde(value, CodecOptions::default(), out)
        });
        assert_eq!(serde_errors.stage(), CodecStage::Validation);
        assert!(!format!("{serde_errors:?}{serde_errors}").contains(SECRET));
        let codes = |e: &CodecErrors| e.diagnostics().iter().map(|d| d.code()).collect::<Vec<_>>();
        assert_eq!(codes(&errors), codes(&serde_errors));
    }
    errors
}

fn server() {
    let schema = compile("server", include_str!("../fixtures/server.tschem"));
    let binding = invoke(|out| TypedSchema::<ServerConfig>::bind(&schema, BindOptions::default(), out));
    let input = include_str!("../fixtures/server.tconf");
    let mut value = roundtrip(&binding, input, 10);
    assert_eq!(value.benchmark.signed_128, i128::MIN);
    assert_eq!(value.benchmark.unsigned_128, u128::MAX);
    assert_eq!(value.security.backup_phone, None);
    assert_eq!(value.worker_pool.queue_capacity, 10000);
    let bits = [0.0_f64, -0.0, f64::from_bits(1), f64::MAX, f64::MIN];
    for number in bits {
        value.resource_limits.perf_budget_ns = number;
        value.benchmark.float_32 = f32::from_bits(1);
        value.benchmark.float_64 = -0.0;
        let text = invoke(|out| binding.serialize(&value, CodecOptions::default(), out));
        let decoded = invoke(|out| binding.deserialize(
                request(&mut SliceSource::new(text.as_bytes())),
                &mut Provider {
                    deny: true,
                    ..Provider::default()
                },
                ConversionLimits::default(),
                out,
            ));
        assert_eq!(
            decoded.resource_limits.perf_budget_ns.to_bits(),
            number.to_bits()
        );
        assert_eq!(
            decoded.benchmark.float_32.to_bits(),
            value.benchmark.float_32.to_bits()
        );
        assert_eq!(
            decoded.benchmark.float_64.to_bits(),
            value.benchmark.float_64.to_bits()
        );
        #[cfg(feature = "serde")]
        {
            assert_eq!(
                text,
                invoke(|out| binding.serialize_serde(&value, CodecOptions::default(), out))
            );
            let decoded = invoke(|out| binding.deserialize_serde(
                    request(&mut SliceSource::new(text.as_bytes())),
                    &mut Provider {
                        deny: true,
                        ..Provider::default()
                    },
                    ConversionLimits::default(),
                    out,
                ));
            assert_eq!(
                decoded.resource_limits.perf_budget_ns.to_bits(),
                number.to_bits()
            );
            assert_eq!(
                decoded.benchmark.float_32.to_bits(),
                value.benchmark.float_32.to_bits()
            );
            assert_eq!(
                decoded.benchmark.float_64.to_bits(),
                value.benchmark.float_64.to_bits()
            );
        }
    }
    value.worker_pool.threads = 0;
    reject(&binding, &value);
    value.worker_pool.threads = 64;
    value.cluster.quorum_nodes = [1, 1, 2];
    reject(&binding, &value);
    value.cluster.quorum_nodes = [1, 2, 3];
    value.protocol = Protocol::Tcp;
    reject(&binding, &value);
    value.protocol = Protocol::Tls(TlsConfig {
        cert_path: "cert".into(),
        key_path: "key".into(),
        insecure_skip_verify: false,
    });
    value.cluster.role = NodeRole::Leader;
    reject(&binding, &value);
    value.cluster.role = NodeRole::Follower {
        leader_endpoint: Endpoint {
            host: value.listen_endpoint.host.clone(),
            port: 9000,
        },
        sync_timeout_ms: 5000,
    };
    reject(&binding, &value);
    value.cluster.role = NodeRole::Standalone;
    if let Credential::OAuth2 { client_id, .. } = &mut value.security.credential {
        *client_id = "wrong".into();
    }
    let errors = reject(&binding, &value);
    let spans: Vec<_> = errors
        .diagnostics()
        .iter()
        .flat_map(|d| d.labels())
        .map(|l| l.span())
        .collect();
    drop(binding);
    drop(schema);
    for span in spans {
        assert!(errors.span_location(span).is_some());
    }
}

fn database() {
    let schema = compile("database", include_str!("../fixtures/database.tschem"));
    let binding = invoke(|out| TypedSchema::<DatabaseClusterConfig>::bind(&schema, BindOptions::default(), out));
    let mut value = roundtrip(&binding, include_str!("../fixtures/database.tconf"), 5);
    assert_eq!(value.default_isolation, IsolationLevel::ReadCommitted);
    assert_eq!(value.pool.idle_timeout_ms, 600000);
    value.slow_query_threshold_ms = 1;
    reject(&binding, &value);
    value.slow_query_threshold_ms = 500;
    value.pool.min_idle_connections = 201;
    reject(&binding, &value);
    value.pool.min_idle_connections = 20;
    value.read_only_routing = false;
    reject(&binding, &value);
    value.read_only_routing = true;
    value.read_replicas[1].host = value.read_replicas[0].host.clone();
    reject(&binding, &value);
    value.read_replicas.clear();
    value.auth = None;
    let text = invoke(|out| binding.serialize(&value, CodecOptions::default(), out));
    assert!(text.contains("auth: none"));
    assert_eq!(
        value,
        invoke(|out| binding.deserialize(
                request(&mut SliceSource::new(text.as_bytes())),
                &mut Provider {
                    deny: true,
                    ..Provider::default()
                },
                ConversionLimits::default(),
                out,
            ))
    );
}

fn all_types() {
    let schema = compile("all_types", include_str!("../fixtures/all_types.tschem"));
    let binding = invoke(|out| TypedSchema::<AllTypesConfig>::bind(&schema, BindOptions::default(), out));
    let mut value = roundtrip(&binding, include_str!("../fixtures/all_types.tconf"), 7);
    assert_eq!(value.texts.plain_text, "default-text");
    assert_eq!(value.collections.fixed_pair, ["primary", "secondary"]);
    assert_eq!(value.collections.optional_token, None);
    assert_eq!(value.collections.disabled_feature, None);
    assert_eq!(value.meta.value, "true");
    let labels = &mut value.collections.dynamic_tags[0].labels;
    labels.clear();
    for key in ["😀", "ö", "a", "z"] {
        labels.insert(key.into(), key.into());
    }
    let first = invoke(|out| binding.serialize(&value, CodecOptions::default(), out));
    value.collections.dynamic_tags[0].labels.clear();
    for key in ["z", "a", "ö", "😀"] {
        value.collections.dynamic_tags[0]
            .labels
            .insert(key.into(), key.into());
    }
    assert_eq!(
        first,
        invoke(|out| binding.serialize(&value, CodecOptions::default(), out))
    );
    #[cfg(feature = "serde")]
    assert_eq!(
        first,
        invoke(|out| binding.serialize_serde(&value, CodecOptions::default(), out))
    );
    value.collections.dynamic_scores = vec![200.0, 200.0, 200.0];
    reject(&binding, &value);
    value.collections.dynamic_scores = vec![1.0];
    value.collections.dynamic_tags[1].id = value.collections.dynamic_tags[0].id;
    reject(&binding, &value);
    value.collections.dynamic_tags[1].id = 2;
    value.collections.optional_token = Some("short".into());
    reject(&binding, &value);
    value.collections.optional_token = None;
    value.collections.dynamic_tags[0]
        .labels
        .insert("hidden".into(), String::new());
    reject(&binding, &value);
}

fn loaded_fixture<T: Target>(name: &str) -> T {
    let schema_path = format!("fixtures/{name}.tschem");
    let config_path = format!("fixtures/{name}.tconf");
    let loaded = invoke(|out| SchemaLoader::new(TypolOptions::default()).compile_file(Path::new(&schema_path), out));
    let binding = invoke(|out| TypedSchema::<T>::bind(loaded.schema(), BindOptions::default(), out));
    let missing = Path::new("fixtures/never-read.tconf");
    let mut denied = Provider {
        deny: true,
        ..Provider::default()
    };
    let binding_error = invoke_error(|out| read_typed_with_environment::<u8>(&loaded, ConfigInput::file(missing), &mut denied, out));
    assert_eq!(binding_error.stage(), TypolStage::Binding);
    let read_error = invoke_error(|out| read_typed_with_environment::<T>(&loaded, ConfigInput::file(missing), &mut denied, out));
    assert_eq!(read_error.stage(), TypolStage::ConfigRead);
    assert!(denied.calls.is_empty());
    let expected = invoke(|out| read_typed_with_environment::<T>(&loaded, ConfigInput::file(Path::new(&config_path)), &mut Provider::default(), out));
    let value = invoke(|out| read_typed::<T>(&loaded, ConfigInput::file(Path::new(&config_path)), out));
    assert_eq!(value, expected);
    let dynamic = invoke(|out| read_value_with_process_environment(&loaded, ConfigInput::file(Path::new(&config_path)), out));
    let display = invoke(|out| SafeValueFormatter::format(loaded.schema(), dynamic.as_config_value(), DisplayOptions::default(), out));
    assert!(display.contains("********"));
    assert!(!display.contains(SECRET));
    assert!(!format!("{loaded:?}{dynamic:?}").contains(SECRET));
    let invalid_path = format!("fixtures/{name}-invalid.tconf");
    write(&invalid_path, "{}").unwrap();
    for errors in [
        invoke_error(|out| read_value_with_process_environment(&loaded, ConfigInput::file(Path::new(&invalid_path)), out)),
        invoke_error(|out| read_typed_with_environment::<T>(&loaded, ConfigInput::file(Path::new(&invalid_path)), &mut denied, out)),
    ] {
        let mut safe = format!(
            "{errors:?}{errors}{}",
            invoke(|out| {
                errors.render(Translator::new(Language::English), RenderOptions::default(), out)
            })
        );
        for diagnostic in errors.diagnostics().iter() {
            safe.push_str(&format!(
                "{diagnostic:?}{}{:?}{:?}",
                diagnostic.message(),
                diagnostic.path(),
                diagnostic.labels()
            ));
        }
        let mut source = errors.source();
        while let Some(error) = source {
            safe.push_str(&format!("{error:?}{error}"));
            source = error.source();
        }
        assert!(!safe.contains(SECRET));
    }
    let encoded = invoke(|out| binding.serialize(&value, CodecOptions::default(), out));
    assert!(encoded.contains(SECRET));
    assert!(!encoded.contains("env("));
    let frozen = format!("fixtures/{name}-frozen.tconf");
    write(&frozen, &encoded).unwrap();
    assert_eq!(
        value,
        invoke(|out| read_typed_with_environment::<T>(
                &loaded,
                ConfigInput::file(Path::new(&frozen)),
                &mut Provider {
                    deny: true,
                    ..Provider::default()
                },
                out,
            ))
    );
    #[cfg(feature = "serde")]
    {
        assert_eq!(
            value,
            invoke(|out| read_serde::<T>(&loaded, ConfigInput::file(Path::new(&config_path)), out))
        );
        assert_eq!(
            value,
            invoke(|out| read_serde_with_environment::<T>(&loaded, ConfigInput::file(Path::new(&config_path)), &mut Provider::default(), out))
        );
        assert_eq!(
            value,
            invoke(|out| read_serde_with_environment::<T>(
                    &loaded,
                    ConfigInput::file(Path::new(&frozen)),
                    &mut Provider {
                        deny: true,
                        ..Provider::default()
                    },
                    out,
                ))
        );
        assert_eq!(
            encoded,
            invoke(|out| binding.serialize_serde(&value, CodecOptions::default(), out))
        );
    }
    drop(binding);
    drop(loaded);
    value
}

fn main() {
    server();
    database();
    all_types();
    let server = loaded_fixture::<ServerConfig>("server");
    assert_eq!(server.worker_pool.queue_capacity, 10000);
    assert_eq!(server.security.backup_phone, None);
    let database = loaded_fixture::<DatabaseClusterConfig>("database");
    assert_eq!(database.pool.idle_timeout_ms, 600000);
    let all_types = loaded_fixture::<AllTypesConfig>("all_types");
    assert_eq!(all_types.texts.plain_text, "default-text");
    assert_eq!(all_types.collections.optional_token, None);
}
