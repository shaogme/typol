use std::{
    env::{temp_dir, var_os},
    fs::{copy, create_dir_all, remove_dir_all, write},
    path::PathBuf,
    process::{Command, id},
};
use typol_test_support::{TestExtract, TestResult, test_verify as assert};

struct Consumer {
    root: PathBuf,
}

impl Consumer {
    fn new() -> TestResult<Self> {
        let root = temp_dir().join(format!("typol-fixture-consumer-{}", id()));
        create_dir_all(root.join("src")).test_value()?;
        Ok(Self { root })
    }

    fn run(&self, feature: bool) -> TestResult {
        let facade = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let support = facade.parent().test_value()?.join("typol-test-support");
        let features = if feature { "serde" } else { "" };
        write(self.root.join("Cargo.toml"), format!(
            "[package]\nname = \"fixture-consumer\"\nversion = \"0.0.0\"\nedition = \"2024\"\n[workspace]\n[features]\ndefault = [\"std\"]\nstd = [\"renamed_typol/std\"]\nserde = [\"renamed_typol/serde\", \"dep:serde\"]\n[dependencies]\nrenamed_typol = {{ package = \"typol\", path = {facade:?}, default-features = false }}\ntypol_test_support = {{ package = \"typol-test-support\", path = {support:?} }}\nserde = {{ version = \"=1.0.228\", optional = true, default-features = false, features = [\"std\", \"derive\"] }}\n",
        )).test_value()?;
        copy(
            facade.join("../../Cargo.lock"),
            self.root.join("Cargo.lock"),
        )
        .test_value()?;
        create_dir_all(self.root.join("fixtures")).test_value()?;
        for name in [
            "server.tschem",
            "server.tconf",
            "database.tschem",
            "database.tconf",
            "all_types.tschem",
            "all_types.tconf",
            "common.tschem",
        ] {
            copy(
                facade.join("../../examples/config").join(name),
                self.root.join("fixtures").join(name),
            )
            .test_value()?;
        }
        write(
            self.root.join("src/main.rs"),
            include_str!("support/fixture_consumer.rs"),
        )
        .test_value()?;
        let mut command = Command::new(var_os("CARGO").unwrap_or_else(|| "cargo".into()));
        command
            .arg("run")
            .args(["--features", features])
            .args(["--offline", "--message-format=json"])
            .current_dir(&self.root)
            .env("CARGO_TARGET_DIR", self.root.join("target"))
            .env("RUSTFLAGS", "-Dwarnings");
        for name in [
            "TYPOL_APP_NAME",
            "TYPOL_NET_HOST",
            "TYPOL_NET_PORT",
            "TYPOL_TLS_KEY_PATH",
            "TYPOL_S3_BUCKET",
            "TYPOL_WORKER_THREADS",
            "TYPOL_MAX_MEMORY_BYTES",
            "TYPOL_NODE_ID",
            "TYPOL_ADMIN_EMAIL",
            "TYPOL_DB_HOST",
            "TYPOL_DB_PORT",
            "TYPOL_DB_USER",
            "TYPOL_DB_NAME",
            "TYPOL_SAMPLE_I32",
            "TYPOL_SAMPLE_U16",
            "TYPOL_SAMPLE_F64",
            "TYPOL_SAMPLE_SLUG",
            "TYPOL_SAMPLE_ACTIVE",
            "TYPOL_SECRET_TOKEN",
        ] {
            command.env_remove(name);
        }
        for name in [
            "TYPOL_CLIENT_SECRET",
            "TYPOL_DB_PASSWORD",
            "TYPOL_NOTIFY_TOKEN",
        ] {
            command.env(name, "fixture-secret-value");
        }
        let output = command.output().test_value()?;
        let diagnostics = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "{diagnostics}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    fn cleanup(self) -> TestResult {
        remove_dir_all(&self.root).test_value()
    }
}

#[test]
fn original_fixtures_roundtrip_through_public_derive_and_serde() -> TestResult {
    let consumer = Consumer::new()?;
    consumer.run(true)?;
    consumer.run(false)?;
    consumer.cleanup()?;
    Ok(())
}
