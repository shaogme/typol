use typol_core::{BudgetError, DiagnosticCode, ExecutionBudget, ExecutionLimits, SourceId};
use typol_syntax::{SchemaAst, SchemaParseOptions, SchemaParser, SyntaxErrors, SyntaxLimits};
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

    fn parse(
        &mut self,
        source_id: SourceId,
        source: &str,
        limits: &SyntaxLimits,
        budget: &ExecutionBudget,
    ) -> Result<SchemaAst, SyntaxErrors> {
        let options = SchemaParseOptions::with_syntax(*limits);
        self.syntax
            .access(|out| SchemaParser::parse_str(source_id, source, &options, budget, out))
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

macro_rules! test_assert_eq {
    ($left:expr, $right:expr $(, $($message:tt)*)?) => {{
        let left = &$left;
        let right = &$right;
        if left != right {
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

trait TestError<T> {
    fn test_error(self) -> TestResult<T>;
}

impl<T, E> TestError<E> for Result<T, E> {
    fn test_error(self) -> TestResult<E> {
        self.err().ok_or(TestFailure)
    }
}

const SOURCE_ID: SourceId = SourceId::new(7);

#[test]
fn reserved_words_and_extra_separators_are_rejected_at_the_offending_token() -> TestResult {
    let mut errors = TestErrors::new();
    let execution = errors
        .execution(ExecutionLimits::new(67_108_864, 67_108_864))
        .test_value()?;

    let source = "entry struct string {}";
    let diagnostics = errors
        .parse(SOURCE_ID, source, &SyntaxLimits::default(), &execution)
        .test_error()?;
    let diagnostics = diagnostics.diagnostics();
    test_assert_eq!(diagnostics.len(), 1);
    test_assert_eq!(
        diagnostics.first().test_value()?.code(),
        DiagnosticCode::SCHEMA_SYNTAX_ERROR
    );
    test_assert_eq!(
        diagnostics
            .first()
            .test_value()?
            .primary()
            .test_value()?
            .span()
            .start(),
        source.find("string").test_value()?
    );

    let source = "entry struct Root { value: u8,, }";
    let diagnostics = errors
        .parse(SOURCE_ID, source, &SyntaxLimits::default(), &execution)
        .test_error()?;
    let diagnostics = diagnostics.diagnostics();
    test_assert_eq!(
        diagnostics.first().test_value()?.code(),
        DiagnosticCode::SCHEMA_SYNTAX_ERROR
    );
    test_assert_eq!(
        diagnostics
            .first()
            .test_value()?
            .primary()
            .test_value()?
            .span()
            .start(),
        source.find(",,").test_value()? + 1
    );

    Ok(())
}

#[test]
fn parser_recovers_multiple_errors_in_source_order_without_returning_an_ast() -> TestResult {
    let mut errors = TestErrors::new();
    let execution = errors
        .execution(ExecutionLimits::new(67_108_864, 67_108_864))
        .test_value()?;

    let source = "struct A { first: } struct B { second: } entry struct Root {}";
    let diagnostics = errors
        .parse(SOURCE_ID, source, &SyntaxLimits::default(), &execution)
        .test_error()?;
    let diagnostics = diagnostics.diagnostics();

    test_assert_eq!(diagnostics.len(), 2);
    test_assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code() == DiagnosticCode::SCHEMA_SYNTAX_ERROR)
    );
    let starts = diagnostics
        .iter()
        .map(|diagnostic| -> TestResult<_> {
            Ok(diagnostic.primary().test_value()?.span().start())
        })
        .collect::<TestResult<Vec<_>>>()?;
    test_assert!(starts.windows(2).all(|pair| {
        pair.first()
            .zip(pair.get(1))
            .is_some_and(|(left, right)| left < right)
    }));

    Ok(())
}

#[test]
fn diagnostic_limit_reserves_the_last_slot_for_a_summary() -> TestResult {
    let mut errors = TestErrors::new();
    let execution = errors
        .execution(ExecutionLimits::new(67_108_864, 67_108_864))
        .test_value()?;

    let mut limits = SyntaxLimits::default();
    limits.set_max_diagnostics(3);
    let diagnostics = errors
        .parse(
            SOURCE_ID,
            "not_schema ? ? entry struct Root {}",
            &limits,
            &execution,
        )
        .test_error()?;
    let diagnostics = diagnostics.diagnostics();

    test_assert_eq!(diagnostics.len(), 3);
    test_assert_eq!(
        diagnostics.first().test_value()?.code(),
        DiagnosticCode::SCHEMA_SYNTAX_ERROR
    );
    test_assert_eq!(
        diagnostics.get(1).test_value()?.code(),
        DiagnosticCode::LEXICAL_ERROR
    );
    test_assert_eq!(
        diagnostics.get(2).test_value()?.code(),
        DiagnosticCode::DIAGNOSTIC_LIMIT_REACHED
    );

    Ok(())
}

#[test]
fn source_and_nesting_limits_stop_schema_parsing_with_resource_diagnostics() -> TestResult {
    let mut errors = TestErrors::new();
    let execution = errors
        .execution(ExecutionLimits::new(67_108_864, 67_108_864))
        .test_value()?;

    let mut source_limits = SyntaxLimits::default();
    source_limits.set_max_source_bytes(3);
    let diagnostics = errors
        .parse(SOURCE_ID, "entry", &source_limits, &execution)
        .test_error()?;
    let diagnostics = diagnostics.diagnostics();
    test_assert_eq!(diagnostics.len(), 1);
    test_assert_eq!(
        diagnostics.first().test_value()?.code(),
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
    );
    let span = diagnostics
        .first()
        .test_value()?
        .primary()
        .test_value()?
        .span();
    test_assert_eq!((span.start(), span.end()), (3, 3));

    let mut nesting_limits = SyntaxLimits::default();
    nesting_limits.set_max_nesting_depth(1);
    let diagnostics = errors
        .parse(
            SOURCE_ID,
            "entry struct Root { field: option<u8>, }",
            &nesting_limits,
            &execution,
        )
        .test_error()?;
    let diagnostics = diagnostics.diagnostics();
    test_assert_eq!(diagnostics.len(), 1);
    test_assert_eq!(
        diagnostics.first().test_value()?.code(),
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
    );

    Ok(())
}

#[test]
fn fatal_causes_take_the_reserved_slot_and_zero_budget_accepts_valid_input() -> TestResult {
    let mut errors = TestErrors::new();
    let execution = errors
        .execution(ExecutionLimits::new(67_108_864, 67_108_864))
        .test_value()?;

    for budget in [0, 1, 2, 100] {
        let mut limits = SyntaxLimits::default();
        limits.set_max_diagnostics(budget);
        test_assert!(
            errors
                .parse(SOURCE_ID, "entry struct Root {}", &limits, &execution)
                .is_ok()
        );
        limits.set_max_string_bytes(1);
        let syntax_errors = errors
            .parse(
                SOURCE_ID,
                "type A = string @regex(\"abc\");",
                &limits,
                &execution,
            )
            .test_error()?;
        test_assert_eq!(syntax_errors.diagnostics().len(), 1);
        test_assert_eq!(
            syntax_errors.diagnostics().first().test_value()?.code(),
            DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
        );
        let syntax_errors = errors
            .parse(SOURCE_ID, "? ? ?", &limits, &execution)
            .test_error()?;
        let diagnostics = syntax_errors.diagnostics();
        test_assert!(diagnostics.len() <= budget.max(1));
        test_assert_eq!(
            diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.code() == DiagnosticCode::DIAGNOSTIC_LIMIT_REACHED)
                .count(),
            usize::from(budget <= 2)
        );
    }
    let mut limits = SyntaxLimits::default();
    limits.set_max_diagnostics(2);
    limits.set_max_string_bytes(1);
    let errors = errors
        .parse(
            SOURCE_ID,
            "? type A = string @regex(\"abc\"); ?",
            &limits,
            &execution,
        )
        .test_error()?;
    test_assert_eq!(
        errors
            .diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.code().get())
            .collect::<Vec<_>>(),
        [3, 1]
    );

    Ok(())
}

#[test]
fn parser_recovers_from_multiple_failed_members_inside_each_container() -> TestResult {
    let mut errors = TestErrors::new();
    let execution = errors
        .execution(ExecutionLimits::new(67_108_864, 67_108_864))
        .test_value()?;

    let fields = "entry struct Root { first: , second: , okay: u8, }";
    let diagnostics = errors
        .parse(SOURCE_ID, fields, &SyntaxLimits::default(), &execution)
        .test_error()?;
    let diagnostics = diagnostics.diagnostics();
    test_assert_eq!(diagnostics.len(), 2);
    let starts = diagnostics
        .iter()
        .map(|diagnostic| -> TestResult<_> {
            Ok(diagnostic.primary().test_value()?.span().start())
        })
        .collect::<TestResult<Vec<_>>>()?;
    test_assert!(starts.windows(2).all(|pair| {
        pair.first()
            .zip(pair.get(1))
            .is_some_and(|(left, right)| left < right)
    }));

    let variants = "enum Broken { First(), Second(), Good } entry struct Root {}";
    let diagnostics = errors
        .parse(SOURCE_ID, variants, &SyntaxLimits::default(), &execution)
        .test_error()?;
    let diagnostics = diagnostics.diagnostics();
    test_assert_eq!(diagnostics.len(), 2);
    test_assert!(
        diagnostics
            .iter()
            .all(|diagnostic| { diagnostic.code() == DiagnosticCode::SCHEMA_SYNTAX_ERROR })
    );

    let module = "mod M { struct Broken { field: } type Good = u8; enum Empty {} struct Later { value: } } entry struct Root {}";
    let diagnostics = errors
        .parse(SOURCE_ID, module, &SyntaxLimits::default(), &execution)
        .test_error()?;
    let diagnostics = diagnostics.diagnostics();
    test_assert_eq!(diagnostics.len(), 3);
    let starts = diagnostics
        .iter()
        .map(|diagnostic| -> TestResult<_> {
            Ok(diagnostic.primary().test_value()?.span().start())
        })
        .collect::<TestResult<Vec<_>>>()?;
    test_assert!(starts.windows(2).all(|pair| {
        pair.first()
            .zip(pair.get(1))
            .is_some_and(|(left, right)| left < right)
    }));

    let module_member =
        "mod M { type Broken = ; type Good = u8; struct Later { value: } } entry struct Root {}";
    let diagnostics = errors
        .parse(
            SOURCE_ID,
            module_member,
            &SyntaxLimits::default(),
            &execution,
        )
        .test_error()?;
    let diagnostics = diagnostics.diagnostics();
    test_assert_eq!(diagnostics.len(), 2);
    let starts = diagnostics
        .iter()
        .map(|diagnostic| -> TestResult<_> {
            Ok(diagnostic.primary().test_value()?.span().start())
        })
        .collect::<TestResult<Vec<_>>>()?;
    let first_separator = module_member.find(';').test_value()?;
    let later_member = module_member.find("struct Later").test_value()?;
    test_assert_eq!(starts.first().copied(), Some(first_separator));
    test_assert!(starts.get(1).is_some_and(|start| *start > later_member));

    Ok(())
}

#[test]
fn resource_failures_inside_members_are_not_recovered_as_syntax_errors() -> TestResult {
    let mut errors = TestErrors::new();
    let execution = errors
        .execution(ExecutionLimits::new(67_108_864, 67_108_864))
        .test_value()?;
    let mut limits = SyntaxLimits::default();
    limits.set_max_nesting_depth(1);

    let diagnostics = errors
        .parse(
            SOURCE_ID,
            "struct A { field: option<u8>, } struct Later { broken: }",
            &limits,
            &execution,
        )
        .test_error()?;
    let diagnostics = diagnostics.diagnostics();
    test_assert_eq!(diagnostics.len(), 1);
    test_assert_eq!(
        diagnostics.first().test_value()?.code(),
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
    );

    Ok(())
}

#[test]
fn nested_recovery_leaves_the_outer_close_and_later_declarations_available() -> TestResult {
    let mut errors = TestErrors::new();
    let execution = errors
        .execution(ExecutionLimits::new(67_108_864, 67_108_864))
        .test_value()?;

    for source in [
        "struct A { x: [u8 } struct B { y: }",
        "enum A { X([u8 } struct B { y: }",
        "mod A { type T = [u8 } struct B { y: }",
    ] {
        let errors = errors
            .parse(SOURCE_ID, source, &SyntaxLimits::default(), &execution)
            .test_error()?;
        let declaration_start = source.find("struct B").test_value()?;
        let mut later_diagnostic_found = false;
        for diagnostic in errors.diagnostics() {
            let start = diagnostic.primary().test_value()?.span().start();
            if start > declaration_start {
                later_diagnostic_found = true;
                break;
            }
        }
        test_assert!(
            later_diagnostic_found,
            "later declarations must still be diagnosed: {source}"
        );
    }

    Ok(())
}

#[test]
fn completed_declarations_before_lexical_errors_keep_the_original_error() -> TestResult {
    let mut errors = TestErrors::new();
    let execution = errors
        .execution(ExecutionLimits::new(67_108_864, 67_108_864))
        .test_value()?;

    for source in ["entry struct Root {} ?", "type A = string; ?"] {
        let errors = errors
            .parse(SOURCE_ID, source, &SyntaxLimits::default(), &execution)
            .test_error()?;
        test_assert_eq!(errors.diagnostics().len(), 1);
        test_assert_eq!(
            errors.diagnostics().first().test_value()?.code(),
            DiagnosticCode::LEXICAL_ERROR
        );
        test_assert_eq!(
            errors
                .diagnostics()
                .first()
                .test_value()?
                .primary()
                .test_value()?
                .span()
                .start(),
            source.find('?').test_value()?
        );
    }

    Ok(())
}
