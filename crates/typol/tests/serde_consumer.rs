#[cfg(feature = "serde")]
use std::{
    env::{temp_dir, var_os},
    fs::{copy, create_dir_all, remove_dir_all, write},
    path::PathBuf,
    process::{Command, id},
};

#[cfg(feature = "serde")]
use typol_test_support::{TestExtract, TestResult, test_verify as assert};

#[cfg(feature = "serde")]
struct Consumer {
    root: PathBuf,
}

#[cfg(feature = "serde")]
impl Consumer {
    fn new() -> TestResult<Self> {
        let root = temp_dir().join(format!("typol-serde-consumer-{}", id()));
        create_dir_all(root.join("src")).test_value()?;
        Ok(Self { root })
    }

    fn run(&self, feature: bool) -> TestResult {
        let facade = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let support = facade.parent().test_value()?.join("typol-test-support");
        let features = if feature {
            ", features = [\"std\", \"serde\"]"
        } else {
            ", features = [\"std\"]"
        };
        write(self.root.join("Cargo.toml"), format!(
            "[package]\nname = \"serde-consumer\"\nversion = \"0.0.0\"\nedition = \"2024\"\n[workspace]\n[features]\nserde = []\n[dependencies]\nrenamed_typol = {{ package = \"typol\", path = {facade:?}, default-features = false{features} }}\ntypol_test_support = {{ package = \"typol-test-support\", path = {support:?} }}\nserde = {{ version = \"=1.0.228\", default-features = false, features = [\"std\", \"derive\"] }}\n",
        )).test_value()?;
        copy(
            facade.join("../../Cargo.lock"),
            self.root.join("Cargo.lock"),
        )
        .test_value()?;
        write(
            self.root.join("src/main.rs"),
            include_str!("support/serde_consumer.rs"),
        )
        .test_value()?;
        copy(
            facade.join("tests/support/serde_read_input.rs"),
            self.root.join("src/read_input.rs"),
        )
        .test_value()?;
        let mut command = Command::new(var_os("CARGO").unwrap_or_else(|| "cargo".into()));
        command.arg(if feature { "run" } else { "check" });
        if feature {
            command.args(["--features", "serde"]);
        }
        let output = command
            .args(["--offline", "--message-format=json"])
            .current_dir(&self.root)
            .env("CARGO_TARGET_DIR", self.root.join("target"))
            .env("RUSTFLAGS", "-Dwarnings")
            .output()
            .test_value()?;
        let diagnostics = String::from_utf8_lossy(&output.stdout);
        if feature {
            assert!(
                output.status.success(),
                "{diagnostics}\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
        } else {
            assert!(
                !output.status.success(),
                "serde methods should not be available when the feature is disabled"
            );
            assert!(diagnostics.contains("serialize_serde"), "{diagnostics}");
            assert!(diagnostics.contains("deserialize_serde"), "{diagnostics}");
            assert!(diagnostics.contains("read_serde"), "{diagnostics}");
            assert!(diagnostics.contains("E0599"), "{diagnostics}");
        }
        Ok(())
    }
}

#[cfg(feature = "serde")]
#[test]
fn real_serde_derives_use_only_feature_gated_public_methods() -> TestResult {
    let consumer = Consumer::new()?;
    let runs = (|| {
        consumer.run(true)?;
        consumer.run(false)
    })();
    remove_dir_all(&consumer.root).test_value()?;
    runs?;
    Ok(())
}
