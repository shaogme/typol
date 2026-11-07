#[path = "support/clippy.rs"]
mod clippy;

use typol_core::{BudgetError, ExecutionLimits, RegexExecutionBudget};
use typol_opeo_test_support::TestSlot;
use typol_policy::RegexLimits;
use typol_regex::{
    Regex, RegexCompileError, RegexCompiler, RegexMatchError, RegexResource, RegexScratch,
    RegexScratchError, RegexSyntaxErrorKind,
};

struct SearchCase {
    pattern: &'static str,
    input: &'static str,
    expected: bool,
}

impl SearchCase {
    fn new(pattern: &'static str, input: &'static str, expected: bool) -> Self {
        Self {
            pattern,
            input,
            expected,
        }
    }
}

struct RegexErrors {
    budget: TestSlot<BudgetError>,
    compile: TestSlot<RegexCompileError>,
    scratch: TestSlot<RegexScratchError>,
    matching: TestSlot<RegexMatchError>,
    active_budget: Option<RegexExecutionBudget>,
}

fn compilation_budget(errors: &mut RegexErrors) -> Option<RegexExecutionBudget> {
    clippy::Clippy::succeed_opeo(
        clippy::Clippy::regex_execution_budget(
            &mut errors.budget,
            ExecutionLimits::new(67_108_864, 67_108_864),
        ),
        "compilation budget should be created",
    )
}

impl RegexErrors {
    fn new() -> Self {
        Self {
            budget: TestSlot::default(),
            compile: TestSlot::default(),
            scratch: TestSlot::default(),
            matching: TestSlot::default(),
            active_budget: None,
        }
    }

    fn compile(
        &mut self,
        pattern: &str,
        limits: &RegexLimits,
        execution: &RegexExecutionBudget,
    ) -> clippy::Attempt<Regex, RegexCompileError> {
        clippy::Clippy::attempt(&mut self.compile, |out| {
            RegexCompiler::compile(pattern, limits, execution, out)
        })
    }

    fn scratch(
        &mut self,
        regex: &Regex,
        limits: &RegexLimits,
    ) -> clippy::Attempt<RegexScratch, RegexScratchError> {
        let Some(budget) = compilation_budget(self) else {
            return clippy::Attempt::Failure(RegexScratchError::AllocationFailed);
        };
        let result = clippy::Clippy::attempt(&mut self.scratch, |out| {
            RegexScratch::new(regex, limits, &budget, out)
        });
        if result.is_success() {
            self.active_budget = Some(budget);
        }
        result
    }

    fn scratch_budgeted(
        &mut self,
        regex: &Regex,
        limits: &RegexLimits,
        budget: &RegexExecutionBudget,
    ) -> clippy::Attempt<RegexScratch, RegexScratchError> {
        clippy::Clippy::attempt(&mut self.scratch, |out| {
            RegexScratch::new(regex, limits, budget, out)
        })
    }

    fn is_match(
        &mut self,
        regex: &Regex,
        input: &str,
        scratch: &mut RegexScratch,
    ) -> clippy::Attempt<bool, RegexMatchError> {
        let Some(budget) = self.active_budget.as_ref() else {
            return clippy::Attempt::Failure(RegexMatchError::CapacityExceeded);
        };
        clippy::Clippy::attempt(&mut self.matching, |out| {
            regex.is_match(input, scratch, budget, out)
        })
    }

    fn is_match_budgeted(
        &mut self,
        regex: &Regex,
        input: &str,
        scratch: &mut RegexScratch,
        budget: &RegexExecutionBudget,
    ) -> clippy::Attempt<bool, RegexMatchError> {
        clippy::Clippy::attempt(&mut self.matching, |out| {
            regex.is_match(input, scratch, budget, out)
        })
    }
}

fn compile_pattern(
    pattern: &str,
    limits: &RegexLimits,
    compilation: &RegexExecutionBudget,
    errors: &mut RegexErrors,
) -> Option<Regex> {
    clippy::Clippy::succeed_opeo(
        errors.compile(pattern, limits, compilation),
        "regex should compile successfully",
    )
}

fn assert_search(errors: &mut RegexErrors, pattern: &str, input: &str, expected: bool) {
    let Some(compilation) = compilation_budget(errors) else {
        return;
    };
    let limits = RegexLimits::default();
    let Some(regex) = compile_pattern(pattern, &limits, &compilation, errors) else {
        return;
    };
    let Some(mut scratch) = clippy::Clippy::succeed_opeo(
        errors.scratch(&regex, &limits),
        "matcher workspace should be created",
    ) else {
        return;
    };
    assert_eq!(
        errors.is_match(&regex, input, &mut scratch),
        clippy::Attempt::Success(expected),
        "pattern {pattern:?}, input {input:?}"
    );
}

#[test]
fn follows_search_group_alternation_anchor_and_repetition_semantics() {
    let mut errors = RegexErrors::new();
    let cases = [
        SearchCase::new("", "", true),
        SearchCase::new("^$", "", true),
        SearchCase::new("^$", "x", false),
        SearchCase::new("", "arbitrary text", true),
        SearchCase::new("()", "arbitrary text", true),
        SearchCase::new("(())", "", true),
        SearchCase::new("abc", "prefix abc suffix", true),
        SearchCase::new("abc", "ab", false),
        SearchCase::new("a|bc", "xbc!", true),
        SearchCase::new("a|bc", "xyz", false),
        SearchCase::new("(ab|c)d", "xcd!", true),
        SearchCase::new("^ab$", "ab", true),
        SearchCase::new("^ab$", "xab", false),
        SearchCase::new("^ab$", "ab\n", false),
        SearchCase::new("a^b", "ab", false),
        SearchCase::new("a$b", "ab", false),
        SearchCase::new("a^b", "a^b", false),
        SearchCase::new("a$b", "a$b", false),
        SearchCase::new("^ab", "abx", true),
        SearchCase::new("^ab", "zab", false),
        SearchCase::new("ab$", "zab", true),
        SearchCase::new("ab$", "zabx", false),
        SearchCase::new("a*", "bbb", true),
        SearchCase::new("a+", "bbb", false),
        SearchCase::new("a+", "baaa", true),
        SearchCase::new("a?", "bbb", true),
        SearchCase::new("^a{2}$", "aa", true),
        SearchCase::new("^a{2}$", "a", false),
        SearchCase::new("^a{0}$", "", true),
        SearchCase::new("^a{0}$", "a", false),
        SearchCase::new("^a{1,1}$", "a", true),
        SearchCase::new("^a{0,0}$", "", true),
        SearchCase::new("^a{0,}$", "aaa", true),
        SearchCase::new("^a{0,2}$", "aa", true),
        SearchCase::new("^a{0,2}$", "aaa", false),
        SearchCase::new("^a{2,}$", "aaa", true),
        SearchCase::new("^a{2,}$", "a", false),
        SearchCase::new("^a{1,3}$", "aaa", true),
        SearchCase::new("^a{1,3}$", "aaaa", false),
        SearchCase::new("aab", "aaab", true),
        SearchCase::new("^aab$", "aaab", false),
        SearchCase::new("^(a|aa)*b$", "aaaaaaaab", true),
        SearchCase::new("^(a|aa)*b$", "aaaaaaaa", false),
        SearchCase::new("^(ab){2}$", "abab", true),
        SearchCase::new("^(ab){2}$", "ab", false),
        SearchCase::new("^(a|ab)c$", "abc", true),
        SearchCase::new("^(ab|b)c$", "abc", true),
    ];

    for case in cases {
        assert_search(&mut errors, case.pattern, case.input, case.expected);
    }

    let maximum_repetition = "a".repeat(255);
    let below_maximum = "a".repeat(254);
    assert_search(&mut errors, "^a{255}$", &maximum_repetition, true);
    assert_search(&mut errors, "^a{255}$", &below_maximum, false);
}

#[test]
fn handles_escapes_classes_and_ascii_shortcuts() {
    let mut errors = RegexErrors::new();
    let metacharacters = r"\.\^\$\|\(\)\[\]\{\}\*\+\?\\-";
    assert_search(&mut errors, metacharacters, ".^$|()[]{}*+?\\-", true);
    assert_search(&mut errors, r"\.", "x", false);
    assert_search(&mut errors, r"\-", "-", true);
    assert_search(&mut errors, r"\n", "x\ny", true);
    assert_search(&mut errors, r"\r", "\r", true);
    assert_search(&mut errors, r"\t", "\t", true);
    assert_search(&mut errors, r"\d+", "port 4821", true);
    assert_search(&mut errors, r"\d+", "no digits", false);
    assert_search(&mut errors, r"\D+", "123", false);
    assert_search(&mut errors, r"\D+", "12x", true);
    assert_search(&mut errors, r"\w+", "abc_09", true);
    assert_search(&mut errors, r"\w+", "é", false);
    assert_search(&mut errors, r"\W+", "abc_09", false);
    assert_search(&mut errors, r"\W+", "é", true);
    assert_search(&mut errors, r"\s+", " \t\r\n", true);
    assert_search(&mut errors, r"\s+", "\u{00A0}", false);
    assert_search(&mut errors, r"\S+", " \t", false);
    assert_search(&mut errors, r"\S+", "\u{00A0}", true);
    assert_search(&mut errors, r"[-a]+", "--a-", true);
    assert_search(&mut errors, r"[a-]+", "-a", true);
    assert_search(&mut errors, r"[\^]", "^", true);
    assert_search(&mut errors, r"[\]]", "]", true);
    assert_search(&mut errors, "[^a-c]+", "abc", false);
    assert_search(&mut errors, "[^a-c]+", "abX", true);
    assert_search(&mut errors, r"[\dA-F]+", "Hex A42", true);
    assert_search(&mut errors, "[é-ê]", "ê", true);
    assert_search(&mut errors, "[é-ê]", "ñ", false);
    assert_search(&mut errors, "[^é]", "🙂", true);
    assert_search(&mut errors, "[^é]", "é", false);
}

#[test]
fn matches_unicode_scalars_without_normalization_or_case_folding() {
    let mut errors = RegexErrors::new();
    assert_search(&mut errors, "é", "xéy", true);
    assert_search(&mut errors, "é", "e\u{301}", false);
    assert_search(&mut errors, "A", "a", false);
    assert_search(&mut errors, ".", "🙂", true);
    assert_search(&mut errors, ".", "\n", false);
    assert_search(&mut errors, ".", "\r", true);

    let lower = '\u{D7FF}';
    let upper = '\u{E000}';
    let mut pattern = String::from("[");
    pattern.push(lower);
    pattern.push('-');
    pattern.push(upper);
    pattern.push(']');
    assert_search(&mut errors, &pattern, &lower.to_string(), true);
    assert_search(&mut errors, &pattern, &upper.to_string(), true);
    assert_search(&mut errors, &pattern, "𐀀", false);

    let mut complement_pattern = String::from("[^");
    complement_pattern.push(lower);
    complement_pattern.push('-');
    complement_pattern.push(upper);
    complement_pattern.push(']');
    assert_search(&mut errors, &complement_pattern, "𐀀", true);
    assert_search(&mut errors, &complement_pattern, &lower.to_string(), false);
}

#[test]
fn reports_resource_failures_without_echoing_pattern_or_input() {
    let mut errors = RegexErrors::new();
    let Some(compilation) = compilation_budget(&mut errors) else {
        return;
    };
    let mut limits = RegexLimits::default();
    limits.set_max_regex_pattern_bytes(1);
    let compile_result = errors.compile("sensitive", &limits, &compilation);
    assert!(
        matches!(
            &compile_result,
            clippy::Attempt::Failure(RegexCompileError::ResourceLimitExceeded(
                RegexResource::PatternBytes
            ))
        ),
        "exceeding the pattern byte limit should return the corresponding resource error"
    );
    let Some(compile_error) = clippy::Clippy::failure_opeo(
        compile_result,
        "exceeding the pattern byte limit should return a compile error",
    ) else {
        return;
    };
    assert!(!format!("{compile_error:?}").contains("sensitive"));

    let limits = RegexLimits::default();
    let Some(regex) = compile_pattern("^secret$", &limits, &compilation, &mut errors) else {
        return;
    };
    let zero_search = clippy::Clippy::attempt(&mut errors.budget, |out| {
        RegexExecutionBudget::new(67_108_864, 0, 67_108_864, out)
    });
    let Some(zero_search) =
        clippy::Clippy::succeed_opeo(zero_search, "zero-search budget should be created")
    else {
        return;
    };
    let Some(mut zero_scratch) = clippy::Clippy::succeed_opeo(
        errors.scratch_budgeted(&regex, &limits, &zero_search),
        "matcher workspace should be created",
    ) else {
        return;
    };
    let match_result =
        errors.is_match_budgeted(&regex, "private-input", &mut zero_scratch, &zero_search);
    assert!(
        matches!(
            &match_result,
            clippy::Attempt::Failure(RegexMatchError::ResourceLimitExceeded(
                RegexResource::SearchSteps
            ))
        ),
        "zero-step budget should report a search resource error"
    );
    let Some(match_error) =
        clippy::Clippy::failure_opeo(match_result, "zero-step budget should return a match error")
    else {
        return;
    };
    let diagnostic = format!("{match_error:?}");
    assert!(!diagnostic.contains("secret"));
    assert!(!diagnostic.contains("private-input"));
}

#[test]
fn enforces_compile_scratch_and_search_boundaries() {
    let mut errors = RegexErrors::new();
    let Some(compilation) = compilation_budget(&mut errors) else {
        return;
    };
    let mut limits = RegexLimits::default();
    limits.set_max_regex_pattern_bytes(2);
    assert!(matches!(
        errors.compile("é", &limits, &compilation),
        clippy::Attempt::Success(_)
    ));
    limits.set_max_regex_pattern_bytes(1);
    assert!(matches!(
        errors.compile("é", &limits, &compilation),
        clippy::Attempt::Failure(RegexCompileError::ResourceLimitExceeded(
            RegexResource::PatternBytes
        ))
    ));

    let mut limits = RegexLimits::default();
    limits.set_max_regex_nesting_depth(64);
    let at_depth_limit = format!("{}a{}", "(".repeat(64), ")".repeat(64));
    assert!(matches!(
        errors.compile(&at_depth_limit, &limits, &compilation),
        clippy::Attempt::Success(_)
    ));
    let over_depth = format!("{}a{}", "(".repeat(65), ")".repeat(65));
    assert!(matches!(
        errors.compile(&over_depth, &limits, &compilation),
        clippy::Attempt::Failure(RegexCompileError::ResourceLimitExceeded(
            RegexResource::NestingDepth
        ))
    ));

    let mut limits = RegexLimits::default();
    limits.set_max_regex_instructions_per_pattern(2);
    limits.set_max_regex_ranges_per_pattern(1);
    let Some(regex) = compile_pattern("a", &limits, &compilation, &mut errors) else {
        return;
    };
    assert_eq!(regex.instruction_count(), 2);
    assert_eq!(regex.range_count(), 1);
    assert_eq!(regex.scratch_state_count(), 2);
    limits.set_max_regex_instructions_per_pattern(1);
    assert!(matches!(
        errors.compile("a", &limits, &compilation),
        clippy::Attempt::Failure(RegexCompileError::ResourceLimitExceeded(
            RegexResource::InstructionCount
        ))
    ));
    limits.set_max_regex_instructions_per_pattern(2);
    limits.set_max_regex_ranges_per_pattern(0);
    assert!(matches!(
        errors.compile("a", &limits, &compilation),
        clippy::Attempt::Failure(RegexCompileError::ResourceLimitExceeded(
            RegexResource::CharacterRangeCount
        ))
    ));

    let limits = RegexLimits::default();
    let Some(regex) = compile_pattern("a", &limits, &compilation, &mut errors) else {
        return;
    };
    let mut exact_scratch_limit = limits;
    exact_scratch_limit.set_max_regex_scratch_states(2);
    let Some(_scratch) = clippy::Clippy::succeed_opeo(
        errors.scratch(&regex, &exact_scratch_limit),
        "boundary scratch capacity should be accepted",
    ) else {
        return;
    };
    exact_scratch_limit.set_max_regex_scratch_states(1);
    assert!(matches!(
        errors.scratch(&regex, &exact_scratch_limit),
        clippy::Attempt::Failure(RegexScratchError::ResourceLimitExceeded(
            RegexResource::ScratchStateCount
        ))
    ));

    let search_three = clippy::Clippy::succeed_opeo(
        clippy::Clippy::attempt(&mut errors.budget, |out| {
            RegexExecutionBudget::new(67_108_864, 3, 67_108_864, out)
        }),
        "three-step search budget should be created",
    );
    let Some(search_three) = search_three else {
        return;
    };
    let Some(mut scratch_three) = clippy::Clippy::succeed_opeo(
        errors.scratch_budgeted(&regex, &limits, &search_three),
        "scratch should be created for a three-step search",
    ) else {
        return;
    };
    assert_eq!(
        errors.is_match_budgeted(&regex, "a", &mut scratch_three, &search_three),
        clippy::Attempt::Success(true)
    );
    assert_eq!(
        errors.is_match_budgeted(&regex, "b", &mut scratch_three, &search_three),
        clippy::Attempt::Success(false)
    );

    let search_two = clippy::Clippy::succeed_opeo(
        clippy::Clippy::attempt(&mut errors.budget, |out| {
            RegexExecutionBudget::new(67_108_864, 2, 67_108_864, out)
        }),
        "two-step search budget should be created",
    );
    let Some(search_two) = search_two else {
        return;
    };
    let Some(mut scratch_two) = clippy::Clippy::succeed_opeo(
        errors.scratch_budgeted(&regex, &limits, &search_two),
        "scratch should be created for a two-step search",
    ) else {
        return;
    };
    assert_eq!(
        errors.is_match_budgeted(&regex, "a", &mut scratch_two, &search_two),
        clippy::Attempt::Failure(RegexMatchError::ResourceLimitExceeded(
            RegexResource::SearchSteps
        ))
    );
}

#[test]
fn reuses_scratch_across_regexes_and_rejects_insufficient_capacity() {
    let mut errors = RegexErrors::new();
    let Some(compilation) = compilation_budget(&mut errors) else {
        return;
    };
    let limits = RegexLimits::default();
    let Some(small) = compile_pattern("a", &limits, &compilation, &mut errors) else {
        return;
    };
    let Some(large) = compile_pattern("a{3}", &limits, &compilation, &mut errors) else {
        return;
    };
    let Some(mut scratch) = clippy::Clippy::succeed_opeo(
        errors.scratch(&large, &limits),
        "large regex workspace should be created",
    ) else {
        return;
    };

    assert_eq!(
        errors.is_match(&small, "cat", &mut scratch),
        clippy::Attempt::Success(true)
    );
    assert_eq!(
        errors.is_match(&large, "aaa", &mut scratch),
        clippy::Attempt::Success(true)
    );
    assert_eq!(
        errors.is_match(&small, "dog", &mut scratch),
        clippy::Attempt::Success(false)
    );

    let Some(mut small_scratch) = clippy::Clippy::succeed_opeo(
        errors.scratch(&small, &limits),
        "small regex workspace should be created",
    ) else {
        return;
    };
    assert!(matches!(
        errors.is_match(&large, "aaa", &mut small_scratch),
        clippy::Attempt::Failure(RegexMatchError::ResourceLimitExceeded(
            RegexResource::ScratchStateCount
        ))
    ));
}

#[test]
fn syntax_errors_keep_only_kind_and_pattern_span() {
    let mut errors = RegexErrors::new();
    let Some(compilation) = compilation_budget(&mut errors) else {
        return;
    };
    let result = errors.compile("classified\\q", &RegexLimits::default(), &compilation);
    assert!(
        matches!(
            &result,
            clippy::Attempt::Failure(RegexCompileError::Syntax(_))
        ),
        "invalid escape should return a syntax error"
    );
    let Some(RegexCompileError::Syntax(syntax)) =
        clippy::Clippy::failure_opeo(result, "invalid escape should return a syntax error")
    else {
        return;
    };
    assert_eq!(syntax.kind(), RegexSyntaxErrorKind::InvalidEscape);
    assert_eq!(syntax.span().start(), 10);
    assert_eq!(syntax.span().end(), 12);
    assert!(!format!("{syntax:?}").contains("classified"));
}

#[test]
fn rejects_syntax_outside_the_schema_grammar() {
    let mut errors = RegexErrors::new();
    let Some(compilation) = compilation_budget(&mut errors) else {
        return;
    };
    let unsupported = [
        r"\1",
        r"\b",
        r"\p{L}",
        "(?=a)",
        "(?<=a)",
        "(?P<name>a)",
        "(?i)a",
        r"(a)\1",
        "a??",
        "a*?",
        "[a-\\d]",
        "[z-a]",
        "a|",
        "|a",
        "[]",
        ")",
        "(",
    ];

    for pattern in unsupported {
        assert!(
            matches!(
                errors.compile(pattern, &RegexLimits::default(), &compilation),
                clippy::Attempt::Failure(RegexCompileError::Syntax(_))
            ),
            "out-of-spec or invalid syntax must be rejected at compile time: {pattern:?}"
        );
    }
}

#[test]
fn regex_budget_separates_cumulative_steps_and_single_search_limit() {
    let mut errors = RegexErrors::new();
    let Some(compilation) = compilation_budget(&mut errors) else {
        return;
    };
    let limits = RegexLimits::default();
    let Some(regex) = compile_pattern("^x$", &limits, &compilation, &mut errors) else {
        return;
    };
    let Some(generous) = clippy::Clippy::succeed_opeo(
        clippy::Clippy::regex_execution_budget(
            &mut errors.budget,
            ExecutionLimits::new(100_000, 1_048_576),
        ),
        "loose Regex budget should be created",
    ) else {
        return;
    };
    let Some(mut scratch) = clippy::Clippy::succeed_opeo(
        errors.scratch_budgeted(&regex, &limits, &generous),
        "scratch space should be created with a loose budget",
    ) else {
        return;
    };
    assert_eq!(
        errors.is_match_budgeted(&regex, "x", &mut scratch, &generous),
        clippy::Attempt::Success(true)
    );
    let single_search_work = generous.steps();
    assert!(single_search_work > 0);

    let Some(shared) = clippy::Clippy::succeed_opeo(
        clippy::Clippy::regex_execution_budget(
            &mut errors.budget,
            ExecutionLimits::new(single_search_work, 1_048_576),
        ),
        "shared Regex budget should be created",
    ) else {
        return;
    };
    let Some(mut scratch) = clippy::Clippy::succeed_opeo(
        errors.scratch_budgeted(&regex, &limits, &shared),
        "scratch space should be created with a shared budget",
    ) else {
        return;
    };
    assert_eq!(
        errors.is_match_budgeted(&regex, "x", &mut scratch, &shared),
        clippy::Attempt::Success(true)
    );
    assert_eq!(
        errors.is_match_budgeted(&regex, "x", &mut scratch, &shared),
        clippy::Attempt::Failure(RegexMatchError::ResourceLimitExceeded(
            RegexResource::ExecutionSteps
        ))
    );

    let zero_search = clippy::Clippy::attempt(&mut errors.budget, |out| {
        RegexExecutionBudget::new(100_000, 0, 1_048_576, out)
    });
    let Some(zero_search) =
        clippy::Clippy::succeed_opeo(zero_search, "zero-search budget should be created")
    else {
        return;
    };
    let Some(mut scratch) = clippy::Clippy::succeed_opeo(
        errors.scratch_budgeted(&regex, &limits, &zero_search),
        "scratch space should be created with a zero-search budget",
    ) else {
        return;
    };
    assert_eq!(
        errors.is_match_budgeted(&regex, "x", &mut scratch, &zero_search),
        clippy::Attempt::Failure(RegexMatchError::ResourceLimitExceeded(
            RegexResource::SearchSteps
        ))
    );
}
