use typol_test_support::{ConfigTestSupport as Support, TestOpeoOutcome};

use std::string::String;
use typol_config::{
    ConfigLimits, ConfigParseOptions, ConfigParseRequest, Environment, EnvironmentError,
    EnvironmentReadContext,
    opeo::{OResult, Out},
};
use typol_core::{ExecutionLimits, SliceSource};
use typol_policy::{ExcerptLimits, RegexOptions};
use typol_schema::{SchemaCompileOptions, SchemaSource};
use typol_test_support::*;

struct Provider {
    calls: usize,
    names: Vec<String>,
}

impl Environment for Provider {
    fn read<'env, 'slot, 'borrow>(
        &'env mut self,
        name: &str,
        _: EnvironmentReadContext<'_>,
        _: Out<'slot, 'borrow, EnvironmentError>,
    ) -> OResult<'slot, Option<&'env [u8]>> {
        self.names.push(name.to_owned());
        self.calls += 1;
        Ok(None)
    }
}

#[test]
fn deterministic_value_mutations_terminate_with_owned_safe_results_under_small_budgets()
-> TestResult<()> {
    let schema = Support::compile_schema(
        SchemaCompileOptions::default(),
        SchemaSource::from_str(
            b"mutations",
            r#"
            enum E { Unit, One(u8), Object { value: u8 } }
            entry struct Root {
                values: [option<u8>],
                keys: map<string, string> @secret,
                variant: E,
                flag: option<bool> @env("X")
            }
        "#,
        ),
        None,
        None,
    )
    .test_value()?;
    let valid = r#"{values: [1, none, 2], keys: {"éøPRIVATE-MARKER": "PRIVATE-MARKER"}, variant: .Object {value: 1}, flag: env("X", true)}"#;
    let offsets: Vec<_> = valid
        .char_indices()
        .map(|(index, _)| index)
        .chain([valid.len()])
        .collect();
    let insertions = [
        "",
        ",",
        "}",
        "]",
        "{",
        "[",
        "(",
        "none",
        ".Unit",
        "\"",
        "/*",
        "env(\"X\")",
    ];
    let mut seed = 0x97a3_1937u32;
    for _ in 0..512 {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let index = seed as usize % (offsets.len() - 1);
        let start = *TestCheck::item(&offsets, index)?;
        let end = *TestCheck::item(&offsets, index + 1)?;
        let insertion_index = (seed >> 8) as usize % insertions.len();
        let insertion = TestCheck::item(&insertions, insertion_index)?;
        let prefix = valid.get(..start).test_value()?;
        let suffix = valid.get(end..).test_value()?;
        let mut text = String::with_capacity(prefix.len() + insertion.len() + suffix.len());
        text.push_str(prefix);
        text.push_str(insertion);
        text.push_str(suffix);
        for (steps, workspace) in [(0, 0), (256, 65_536), (100_000, 1_048_576)] {
            let source = text.clone();
            let mut provider = Provider {
                calls: 0,
                names: Vec::new(),
            };
            let result = Support::parse(
                &schema,
                ConfigParseRequest::new(
                    &mut SliceSource::new(source.as_bytes()),
                    "mutations",
                    ConfigParseOptions::new(
                        ExcerptLimits::default(),
                        RegexOptions::default(),
                        ConfigLimits::default(),
                        ExecutionLimits::new(steps, workspace),
                    ),
                ),
                &mut provider,
            );
            test_assert!(provider.calls <= 1)?;
            test_assert!(provider.names.iter().all(|name| name == "X"))?;
            test_assert!(!format!("{result:?}").contains("PRIVATE-MARKER"))?;
            drop(source);
            if let TestOpeoOutcome::Error(errors) = result {
                test_assert!(!errors.diagnostics().is_empty())?;
                test_assert!(errors.diagnostics().len() <= 100)?;
            }
        }
    }

    Ok(())
}
