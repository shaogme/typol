#[path = "support/clippy.rs"]
mod clippy;

use std::{env, process::Command, thread};
use typol_core::{BudgetError, ExecutionLimits};
use typol_opeo_test_support::TestSlot;
use typol_policy::RegexLimits;
use typol_regex::{
    RegexCompileError, RegexCompiler, RegexResource, RegexScratch, RegexScratchError,
};

const LIMIT: usize = 67_108_864;

#[test]
fn compiled_program_retains_payloads_and_shares_cumulative_work() {
    let mut budget_errors = TestSlot::<BudgetError>::default();
    let Some(budget) = clippy::Clippy::succeed_opeo(
        clippy::Clippy::regex_execution_budget(
            &mut budget_errors,
            ExecutionLimits::new(LIMIT, LIMIT),
        ),
        "compilation budget should be created",
    ) else {
        return;
    };
    let initial = budget.workspace_bytes();
    let limits = RegexLimits::default();
    let mut compile_errors = TestSlot::<RegexCompileError>::default();
    let Some(regex) = clippy::Clippy::succeed_opeo(
        clippy::Clippy::attempt(&mut compile_errors, |out| {
            RegexCompiler::compile("(ab|[a-z]){2,8}", &limits, &budget, out)
        }),
        "initial compilation should succeed",
    ) else {
        return;
    };
    assert!(budget.workspace_bytes() > initial);
    let before = budget.steps();
    let Some(second) = clippy::Clippy::succeed_opeo(
        clippy::Clippy::attempt(&mut compile_errors, |out| {
            RegexCompiler::compile("(ab|[a-z]){2,8}", &limits, &budget, out)
        }),
        "repeated compilation should succeed",
    ) else {
        return;
    };
    assert_eq!(budget.steps(), before * 2);
    drop(regex);
    assert!(budget.workspace_bytes() > initial);
    drop(second);
    assert_eq!(budget.workspace_bytes(), initial);
}

#[test]
fn parsing_and_repeat_expansion_stop_at_the_shared_step_boundary() {
    let mut budget_errors = TestSlot::<BudgetError>::default();
    let mut compile_errors = TestSlot::<RegexCompileError>::default();
    for pattern in ["(ab|[a-z]){2,8}", "a{255}"] {
        let Some(budget) = clippy::Clippy::succeed_opeo(
            clippy::Clippy::regex_execution_budget(
                &mut budget_errors,
                ExecutionLimits::new(LIMIT, LIMIT),
            ),
            "compilation budget should be created",
        ) else {
            return;
        };
        let Some(regex) = clippy::Clippy::succeed_opeo(
            clippy::Clippy::attempt(&mut compile_errors, |out| {
                RegexCompiler::compile(pattern, &RegexLimits::default(), &budget, out)
            }),
            "baseline compilation should succeed",
        ) else {
            return;
        };
        drop(regex);
        let required = budget.steps();
        let Some(limited) = clippy::Clippy::succeed_opeo(
            clippy::Clippy::regex_execution_budget(
                &mut budget_errors,
                ExecutionLimits::new(required.saturating_sub(1), LIMIT),
            ),
            "limited budget should be created",
        ) else {
            return;
        };
        let initial = limited.workspace_bytes();
        assert!(matches!(
            clippy::Clippy::attempt(&mut compile_errors, |out| {
                RegexCompiler::compile(pattern, &RegexLimits::default(), &limited, out)
            }),
            clippy::Attempt::Failure(RegexCompileError::ResourceLimitExceeded(
                RegexResource::CompilationBudget
            ))
        ));
        assert!(limited.exhausted());
        assert_eq!(limited.workspace_bytes(), initial);
        assert!(matches!(
            clippy::Clippy::attempt(&mut compile_errors, |out| {
                RegexCompiler::compile("a", &RegexLimits::default(), &limited, out)
            }),
            clippy::Attempt::Failure(RegexCompileError::ResourceLimitExceeded(
                RegexResource::CompilationBudget
            ))
        ));
    }
}

#[test]
fn scratch_uses_the_same_precise_capacity_budget() {
    let mut budget_errors = TestSlot::<BudgetError>::default();
    let Some(budget) = clippy::Clippy::succeed_opeo(
        clippy::Clippy::regex_execution_budget(
            &mut budget_errors,
            ExecutionLimits::new(LIMIT, LIMIT),
        ),
        "scratch budget should be created",
    ) else {
        return;
    };
    let limits = RegexLimits::default();
    let mut compile_errors = TestSlot::<RegexCompileError>::default();
    let Some(regex) = clippy::Clippy::succeed_opeo(
        clippy::Clippy::attempt(&mut compile_errors, |out| {
            RegexCompiler::compile("[a-z]+", &limits, &budget, out)
        }),
        "character-class regex should compile successfully",
    ) else {
        return;
    };
    let before = budget.workspace_bytes();
    let mut scratch_errors = TestSlot::<RegexScratchError>::default();
    let Some(scratch) = clippy::Clippy::succeed_opeo(
        clippy::Clippy::attempt(&mut scratch_errors, |out| {
            RegexScratch::new(&regex, &limits, &budget, out)
        }),
        "scratch space should be created",
    ) else {
        return;
    };
    assert_eq!(
        budget.workspace_bytes() - before,
        regex.scratch_state_count() * (3 * size_of::<usize>() + 2 * size_of::<bool>())
    );
    drop(scratch);
    assert_eq!(budget.workspace_bytes(), before);
}

#[test]
fn regex_stack_child() {
    if env::var_os("TYPOL_REGEX_STACK_CHILD").is_none() {
        return;
    }
    let spawned = thread::Builder::new().stack_size(128 * 1024).spawn(|| {
        let depth = 4096;
        let mut limits = RegexLimits::default();
        limits.set_max_regex_nesting_depth(depth + 16);
        limits.set_max_regex_pattern_bytes(depth * 2 + 1);
        let pattern = format!("{}a{}", "(".repeat(depth), ")".repeat(depth));
        let mut budget_errors = TestSlot::<BudgetError>::default();
        let mut compile_errors = TestSlot::<RegexCompileError>::default();
        let Some(budget) = clippy::Clippy::succeed_opeo(
            clippy::Clippy::regex_execution_budget(
                &mut budget_errors,
                ExecutionLimits::new(LIMIT, LIMIT),
            ),
            "deep compilation budget should be created",
        ) else {
            return;
        };
        let initial = budget.workspace_bytes();
        let Some(regex) = clippy::Clippy::succeed_opeo(
            clippy::Clippy::attempt(&mut compile_errors, |out| {
                RegexCompiler::compile(&pattern, &limits, &budget, out)
            }),
            "regex at the depth limit should compile successfully",
        ) else {
            return;
        };
        drop(regex);
        assert_eq!(budget.workspace_bytes(), initial);
        limits.set_max_regex_nesting_depth(128);
        assert!(matches!(
            clippy::Clippy::attempt(&mut compile_errors, |out| {
                RegexCompiler::compile(&pattern, &limits, &budget, out)
            }),
            clippy::Attempt::Failure(RegexCompileError::ResourceLimitExceeded(
                RegexResource::NestingDepth
            ))
        ));
        limits.set_max_regex_nesting_depth(depth + 16);
        limits.set_max_regex_pattern_bytes(depth * 2 + 1);
        let Some(small) = clippy::Clippy::succeed_opeo(
            clippy::Clippy::regex_execution_budget(
                &mut budget_errors,
                ExecutionLimits::new(30_000, LIMIT),
            ),
            "small budget should be created",
        ) else {
            return;
        };
        assert!(matches!(
            clippy::Clippy::attempt(&mut compile_errors, |out| {
                RegexCompiler::compile(&pattern, &limits, &small, out)
            }),
            clippy::Attempt::Failure(RegexCompileError::ResourceLimitExceeded(
                RegexResource::CompilationBudget
            ))
        ));
    });
    assert!(
        spawned.is_ok(),
        "small-stack child-process thread should start"
    );
    let Some(handle) = spawned.ok() else {
        return;
    };
    let joined = handle.join();
    assert!(
        joined.is_ok(),
        "small-stack child-process thread should exit normally"
    );
}

#[test]
fn deep_regex_compilation_and_cleanup_have_bounded_host_stack() {
    let executable = env::current_exe();
    assert!(
        executable.is_ok(),
        "test executable path should be available"
    );
    let Some(executable) = executable.ok() else {
        return;
    };
    let status = Command::new(executable)
        .args(["--exact", "regex_stack_child"])
        .env("TYPOL_REGEX_STACK_CHILD", "1")
        .status();
    assert!(
        status.is_ok(),
        "deep-compilation child process should start"
    );
    let Some(status) = status.ok() else {
        return;
    };
    assert!(status.success());
}

#[test]
fn independent_capacity_limit_rejects_compilation_and_releases_partial_ast() {
    let mut budget_errors = TestSlot::<BudgetError>::default();
    let Some(budget) = clippy::Clippy::succeed_opeo(
        clippy::Clippy::regex_execution_budget(
            &mut budget_errors,
            ExecutionLimits::new(LIMIT, 512),
        ),
        "capacity budget should be created",
    ) else {
        return;
    };
    let initial = budget.workspace_bytes();
    let mut compile_errors = TestSlot::<RegexCompileError>::default();
    assert!(matches!(
        clippy::Clippy::attempt(&mut compile_errors, |out| {
            RegexCompiler::compile("(ab|[a-z]){2,8}", &RegexLimits::default(), &budget, out)
        }),
        clippy::Attempt::Failure(RegexCompileError::ResourceLimitExceeded(
            RegexResource::WorkspaceBytes
        ))
    ));
    assert!(budget.exhausted());
    assert!(budget.peak_workspace_bytes() <= 512);
    assert_eq!(budget.workspace_bytes(), initial);
}
