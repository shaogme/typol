use typol::{
    EmptyEnvironment, Environment, EnvironmentError, EnvironmentReadContext, TypolError,
    TypolStage, WriteOptions,
    io::ReadRequest,
    opeo::{ErrSlot, OResult, Out},
};
use typol_showcase::{
    ShowcaseConfig, load_demo_config_with_environment, load_schema, load_text_with_environment,
};

struct DemoEnvironment;

impl Environment for DemoEnvironment {
    fn read<'env, 'slot, 'borrow>(
        &'env mut self,
        name: &str,
        _: EnvironmentReadContext<'_>,
        _out: Out<'slot, 'borrow, EnvironmentError>,
    ) -> OResult<'slot, Option<&'env [u8]>> {
        let value: Option<&[u8]> = match name {
            "SHOWCASE_SERVICE_NAME" => Some(b"metrics-api"),
            "SHOWCASE_HOST" => Some(b"api.example.test"),
            "SHOWCASE_PORT" => Some(b"9443"),
            "SHOWCASE_PRIVATE_KEY" => Some(b"test-private-key-0123456789"),
            "SHOWCASE_STORAGE_TOKEN" => Some(b"test-storage-token-0123456789"),
            _ => None,
        };
        Ok(value)
    }
}

#[test]
fn local_schema_import_and_derive_macro_load_the_documented_config() {
    let mut environment = DemoEnvironment;
    let mut errors = ErrSlot::<TypolError>::new();
    let config = errors
        .try_call(|out| load_demo_config_with_environment(&mut environment, out))
        .unwrap();

    assert_eq!(config.service_name(), "metrics-api");
    assert_eq!(config.endpoint().host(), "api.example.test");
    assert_eq!(config.endpoint().port(), 9443);
    assert_eq!(config.worker_pool().min_threads(), 2);
    assert_eq!(config.worker_pool().max_threads(), 8);
    assert_eq!(config.retry().attempts(), 3);
    assert_eq!(config.retry().backoff_ms(), &[0, 100, 500]);
    assert_eq!(config.retry().multiplier(), 2.0);
    assert!(config.uses_tls());
    assert!(config.uses_object_storage());
    assert_eq!(config.labels().len(), 2);
    assert_eq!(
        config.labels().get("team").map(String::as_str),
        Some("platform")
    );
    assert_eq!(config.peers().len(), 2);
    assert_eq!(config.features()[0], "metrics");
    assert_eq!(config.features()[1], "health");
    assert_eq!(config.optional_note(), None);
}

#[test]
fn missing_environment_values_and_fields_use_fallbacks_and_schema_defaults() {
    let mut errors = ErrSlot::<TypolError>::new();
    let demo = errors
        .try_call(|out| load_demo_config_with_environment(&mut EmptyEnvironment, out))
        .unwrap();
    assert_eq!(demo.service_name(), "typol-demo");
    assert_eq!(demo.endpoint().host(), "127.0.0.1");
    assert_eq!(demo.endpoint().port(), 8443);

    let minimal = r#"{
        endpoint: {},
        peers: [],
        features: ["health"],
    }"#;
    let config = errors
        .try_call(|out| load_text_with_environment(minimal, &mut EmptyEnvironment, out))
        .unwrap();

    assert_eq!(config.service_name(), "typol-demo");
    assert_eq!(config.endpoint().host(), "127.0.0.1");
    assert_eq!(config.endpoint().port(), 8080);
    assert_eq!(config.worker_pool().min_threads(), 2);
    assert_eq!(config.worker_pool().max_threads(), 8);
    assert_eq!(config.retry().attempts(), 3);
    assert!(config.labels().is_empty());
    assert!(!config.uses_tls());
    assert!(!config.uses_object_storage());
    assert_eq!(config.optional_note(), None);
}

#[test]
fn derived_binding_round_trips_config_and_checks_collection_constraints() {
    let mut load_errors = ErrSlot::<TypolError>::new();
    let loaded = load_errors.try_call(load_schema).unwrap();
    let binding = load_errors
        .try_call(|out| loaded.bind::<ShowcaseConfig>(out))
        .unwrap();
    let mut environment = DemoEnvironment;
    let config = load_errors
        .try_call(|out| {
            binding.read(
                ReadRequest::from_text(include_str!("../config/demo.tconf")),
                &mut environment,
                out,
            )
        })
        .unwrap();
    let mut encoded = String::new();
    load_errors
        .try_call(|out| binding.write(&config, &mut encoded, WriteOptions::default(), out))
        .unwrap();

    let mut source = encoded.as_bytes();
    let decoded = load_errors
        .try_call(|out| binding.read_io(&mut source, "往返配置", &mut DemoEnvironment, out))
        .unwrap();

    assert!(decoded == config);

    let duplicate_peer = r#"{
        endpoint: { host: "127.0.0.1", port: 8443 },
        peers: [
            { host: "10.0.0.11", port: 8443 },
            { host: "10.0.0.11", port: 9443 },
        ],
        features: ["metrics"],
    }"#;
    let Err(errors) = load_errors
        .try_call(|out| load_text_with_environment(duplicate_peer, &mut DemoEnvironment, out))
    else {
        panic!("重复的 peer 主机名应被拒绝");
    };
    assert_eq!(errors.get().stage(), TypolStage::Decoding);
}

#[test]
fn struct_assertions_reject_invalid_thread_bounds() {
    let invalid = r#"{
        endpoint: { host: "127.0.0.1", port: 8443 },
        worker_pool: { min_threads: 8, max_threads: 4 },
        peers: [],
        features: ["health"],
    }"#;
    let mut errors = ErrSlot::<TypolError>::new();
    let Err(errors) =
        errors.try_call(|out| load_text_with_environment(invalid, &mut DemoEnvironment, out))
    else {
        panic!("无效的线程范围应被拒绝");
    };

    assert_eq!(errors.get().stage(), TypolStage::Decoding);
    assert!(!errors.get().diagnostics().is_empty());
}
