use std::{collections::HashMap, path::PathBuf};

use typol::{
    ConfigInput, Environment, LoadedSchema, ProcessEnvironment, ProcessEnvironmentLimits,
    SchemaLoader, Typol, TypolError, TypolOptions,
    opeo::{OResult, Out},
};

#[derive(PartialEq, Typol)]
pub struct Endpoint {
    host: String,
    port: u16,
}

impl Endpoint {
    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

#[derive(PartialEq, Typol)]
struct TlsOptions {
    certificate: String,
    private_key: String,
    verify_peer: bool,
}

#[derive(PartialEq, Typol)]
enum Protocol {
    Http,
    Https(TlsOptions),
}

#[derive(PartialEq, Typol)]
enum Storage {
    Memory,
    Local(String),
    ObjectStore {
        bucket: String,
        access_token: String,
        region: String,
    },
}

#[derive(PartialEq, Typol)]
pub struct WorkerPool {
    min_threads: u16,
    max_threads: u16,
}

impl WorkerPool {
    pub fn min_threads(&self) -> u16 {
        self.min_threads
    }

    pub fn max_threads(&self) -> u16 {
        self.max_threads
    }
}

#[derive(PartialEq, Typol)]
pub struct RetryPolicy {
    attempts: u8,
    backoff_ms: [u32; 3],
    multiplier: f64,
}

impl RetryPolicy {
    pub fn attempts(&self) -> u8 {
        self.attempts
    }

    pub fn backoff_ms(&self) -> &[u32; 3] {
        &self.backoff_ms
    }

    pub fn multiplier(&self) -> f64 {
        self.multiplier
    }
}

#[derive(PartialEq, Typol)]
pub struct ShowcaseConfig {
    service_name: String,
    endpoint: Endpoint,
    protocol: Protocol,
    storage: Storage,
    worker_pool: WorkerPool,
    retry: RetryPolicy,
    labels: HashMap<String, String>,
    peers: Vec<Endpoint>,
    features: Vec<String>,
    optional_note: Option<String>,
}

impl ShowcaseConfig {
    pub fn service_name(&self) -> &str {
        &self.service_name
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    pub fn worker_pool(&self) -> &WorkerPool {
        &self.worker_pool
    }

    pub fn retry(&self) -> &RetryPolicy {
        &self.retry
    }

    pub fn labels(&self) -> &HashMap<String, String> {
        &self.labels
    }

    pub fn peers(&self) -> &[Endpoint] {
        &self.peers
    }

    pub fn features(&self) -> &[String] {
        &self.features
    }

    pub fn optional_note(&self) -> Option<&str> {
        self.optional_note.as_deref()
    }

    pub fn uses_tls(&self) -> bool {
        matches!(&self.protocol, Protocol::Https(_))
    }

    pub fn uses_object_storage(&self) -> bool {
        matches!(&self.storage, Storage::ObjectStore { .. })
    }
}

pub fn load_schema<'slot, 'borrow>(
    out: Out<'slot, 'borrow, TypolError>,
) -> OResult<'slot, LoadedSchema> {
    SchemaLoader::new(TypolOptions::default()).compile_file(&schema_path(), out)
}

pub fn load_demo_config<'slot, 'borrow>(
    mut out: Out<'slot, 'borrow, TypolError>,
) -> OResult<'slot, ShowcaseConfig> {
    let loaded = load_schema(out.reborrow())?;
    let bound = loaded.bind::<ShowcaseConfig>(out.reborrow())?;
    let mut environment = ProcessEnvironment::new(ProcessEnvironmentLimits::default());
    bound.read(
        ConfigInput::file(config_path().as_path()),
        &mut environment,
        out,
    )
}

pub fn load_demo_config_with_environment<'slot, 'borrow>(
    environment: &mut dyn Environment,
    mut out: Out<'slot, 'borrow, TypolError>,
) -> OResult<'slot, ShowcaseConfig> {
    let loaded = load_schema(out.reborrow())?;
    loaded.bind::<ShowcaseConfig>(out.reborrow())?.read(
        ConfigInput::file(config_path().as_path()),
        environment,
        out.reborrow(),
    )
}

pub fn load_text_with_environment<'slot, 'borrow>(
    text: &str,
    environment: &mut dyn Environment,
    mut out: Out<'slot, 'borrow, TypolError>,
) -> OResult<'slot, ShowcaseConfig> {
    let loaded = load_schema(out.reborrow())?;
    loaded.bind::<ShowcaseConfig>(out.reborrow())?.read(
        ConfigInput::text(text).named("内存配置"),
        environment,
        out.reborrow(),
    )
}

fn schema_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("schemas/application.tschem")
}

fn config_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config/demo.tconf")
}
