use std::{
    env,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use typol_core::{BudgetError, ExecutionBudget, ExecutionLimits, SourceId};
use typol_syntax::{
    SchemaParseOptions, SchemaParser, SyntaxErrors,
    opeo::{OResult, Out},
};
use typol_test_support::TestSlot;

struct TestErrors {
    budget: TestSlot<BudgetError>,
    syntax: TestSlot<SyntaxErrors>,
}

impl TestErrors {
    fn new() -> Self {
        Self {
            budget: TestSlot::default(),
            syntax: TestSlot::default(),
        }
    }

    fn execution(&mut self, limits: ExecutionLimits) -> Result<ExecutionBudget, BudgetError> {
        self.budget.access(|out| ExecutionBudget::new(limits, out))
    }

    fn syntax<T>(
        &mut self,
        operation: impl for<'slot> FnOnce(Out<'slot, 'slot, SyntaxErrors>) -> OResult<'slot, T>,
    ) -> Result<T, SyntaxErrors> {
        self.syntax.access(operation)
    }
}

#[derive(Debug)]
struct TestFailure;

macro_rules! test_assert {
    ($condition:expr $(, $($message:tt)*)?) => {{
        if !$condition {
            return Err(TestFailure);
        }
    }};
}

type TestResult<T = ()> = Result<T, TestFailure>;

trait TestValue<T> {
    fn test_value(self) -> TestResult<T>;
}

impl<T, E> TestValue<T> for Result<T, E> {
    fn test_value(self) -> TestResult<T> {
        self.map_err(|_| TestFailure)
    }
}

impl<T> TestValue<T> for Option<T> {
    fn test_value(self) -> TestResult<T> {
        self.ok_or(TestFailure)
    }
}

#[test]
fn recovery_child() -> TestResult {
    let mut errors = TestErrors::new();
    let execution = errors
        .execution(ExecutionLimits::new(67_108_864, 67_108_864))
        .test_value()?;

    let Ok(source) = env::var("TYPOL_RECOVERY_SOURCE") else {
        return Ok(());
    };
    if source == "mixed" {
        let fragments = [
            "[", "]", "(", ")", "{", "}", ",", "?", "u8", "ẞ", "\"", "struct",
        ];
        let mut state = 7_u64;
        for _ in 0..4096 {
            let mut input = String::from("struct A { x: ");
            for _ in 0..12 {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                let index = ((state >> 32) as usize) % fragments.len();
                input.push_str(fragments.get(index).test_value()?);
                input.push(' ');
            }
            input.push_str("} struct B { y: }");
            if let Err(errors) = errors.syntax(|out| {
                SchemaParser::parse_str(
                    SourceId::new(7),
                    &input,
                    &SchemaParseOptions::default(),
                    &execution,
                    out,
                )
            }) {
                test_assert!(!errors.diagnostics().is_empty());
                test_assert!(errors.diagnostics().len() <= 100);
                for diagnostic in errors.diagnostics() {
                    let span = diagnostic.primary().test_value()?.span();
                    test_assert!(span.start() <= span.end() && span.end() <= input.len());
                    test_assert!(
                        input.is_char_boundary(span.start()) && input.is_char_boundary(span.end())
                    );
                }
            }
        }
    } else {
        test_assert!(
            errors
                .syntax(|out| {
                    SchemaParser::parse_str(
                        SourceId::new(7),
                        &source,
                        &SchemaParseOptions::default(),
                        &execution,
                        out,
                    )
                })
                .is_err()
        );
    }

    Ok(())
}

#[test]
fn malformed_nested_input_terminates_in_a_separate_process() -> TestResult {
    for source in [
        "mixed",
        "struct A { x: [u8 }",
        "enum A { X([u8 }",
        "struct A { x: [u8",
        "enum A { X(u8",
        "struct A { x: u8 = {",
        "mod A { struct B { x: [(u8 } }",
    ] {
        let mut child = Command::new(env::current_exe().test_value()?)
            .args(["--exact", "recovery_child"])
            .env("TYPOL_RECOVERY_SOURCE", source)
            .stdout(Stdio::null())
            .spawn()
            .test_value()?;
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(status) = child.try_wait().test_value()? {
                test_assert!(status.success(), "recovery child process failed: {source}");
                break;
            }
            if Instant::now() >= deadline {
                child.kill().test_value()?;
                child.wait().test_value()?;
                return Err(TestFailure);
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    Ok(())
}
