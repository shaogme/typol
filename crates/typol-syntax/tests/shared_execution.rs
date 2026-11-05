use typol_core::{
    BudgetError, Diagnostic, DiagnosticCode, ExecutionBudget, ExecutionLimits, Read, SliceSource,
    SourceId,
};
use typol_syntax::{
    AnnotationData, AstAccessError, InputConfig, LexEvent, SchemaAst, SchemaParseOptions,
    SchemaParser, StreamLimits, SyntaxErrors, SyntaxLimits, TokenCursor,
};
use typol_test_support::TestSlot;

struct TestCursor<'source> {
    cursor: TokenCursor<'source>,
    errors: TestSlot<Diagnostic>,
}

impl<'source> TestCursor<'source> {
    fn new(
        source_id: SourceId,
        source: &'source mut dyn Read<Diagnostic>,
        limits: &SyntaxLimits,
        stream: StreamLimits,
        execution: &ExecutionBudget,
    ) -> Result<Self, Diagnostic> {
        let input_limits = limits.input_limits();
        let mut errors = TestSlot::default();
        let cursor = errors.access(|out| {
            TokenCursor::new(
                InputConfig {
                    source_id,
                    source,
                    resources: &input_limits,
                    stream,
                    budget: execution,
                },
                out,
            )
        })?;
        Ok(Self { cursor, errors })
    }

    fn bump(&mut self) -> Result<LexEvent, Diagnostic> {
        self.errors.access(|out| self.cursor.bump(out))
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

const LIMIT: usize = 67_108_864;

fn execution(steps: usize, bytes: usize) -> TestResult<ExecutionBudget> {
    TestSlot::<BudgetError>::default()
        .access(|out| ExecutionBudget::new(ExecutionLimits::new(steps, bytes), out))
        .test_value()
}

fn parse_with_slot(
    errors: &mut TestSlot<SyntaxErrors>,
    source_id: SourceId,
    source: &str,
    limits: &SyntaxLimits,
    budget: &ExecutionBudget,
) -> Result<SchemaAst, SyntaxErrors> {
    let options = SchemaParseOptions::with_syntax(*limits);
    errors.access(|out| SchemaParser::parse_str(source_id, source, &options, budget, out))
}

#[test]
fn zero_steps_rejects_even_an_empty_source_without_retained_storage() -> TestResult {
    let budget = execution(0, LIMIT)?;
    let mut syntax_errors = TestSlot::<SyntaxErrors>::default();
    let initial = budget.workspace_bytes();
    let errors = parse_with_slot(
        &mut syntax_errors,
        SourceId::new(7),
        "",
        &SyntaxLimits::default(),
        &budget,
    )
    .test_error()?;
    test_assert_eq!(
        errors.diagnostics().next_back().test_value()?.code(),
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
    );
    test_assert!(budget.exhausted());
    drop(errors);
    test_assert_eq!(budget.workspace_bytes(), initial);

    Ok(())
}

#[test]
fn multiple_sources_share_steps_and_keep_the_first_tree_charged() -> TestResult {
    let source = "entry struct Root { x: option<u8> = none }";
    let limits = SyntaxLimits::default();
    let mut syntax_errors = TestSlot::<SyntaxErrors>::default();
    let probe = execution(LIMIT, LIMIT)?;
    drop(
        parse_with_slot(
            &mut syntax_errors,
            SourceId::new(0),
            source,
            &limits,
            &probe,
        )
        .test_value()?,
    );
    let one = probe.steps();
    let budget = execution(one + one / 2, LIMIT)?;
    let initial = budget.workspace_bytes();
    let mut ast_errors = TestSlot::<AstAccessError>::default();
    let first = parse_with_slot(
        &mut syntax_errors,
        SourceId::new(0),
        source,
        &limits,
        &budget,
    )
    .test_value()?;
    let retained = budget.workspace_bytes();
    test_assert!(retained > initial);
    let errors = parse_with_slot(
        &mut syntax_errors,
        SourceId::new(1),
        source,
        &limits,
        &budget,
    )
    .test_error()?;
    test_assert_eq!(
        errors.diagnostics().next_back().test_value()?.code(),
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
    );
    test_assert!(budget.steps() > one);
    drop(errors);
    test_assert_eq!(budget.workspace_bytes(), retained);
    let item = ast_errors
        .access(|out| first.items().first(out))
        .test_value()?
        .test_value()?;
    let entry = item.entry_struct().test_value()?;
    test_assert_eq!(
        ast_errors.access(|out| entry.name(out)).test_value()?,
        "Root"
    );
    drop(first);
    test_assert_eq!(budget.workspace_bytes(), initial);

    Ok(())
}

#[test]
fn escaped_payload_keeps_its_exact_charge_after_leaving_the_token() -> TestResult {
    let budget = execution(LIMIT, LIMIT)?;
    let initial = budget.workspace_bytes();
    let mut byte_input_1 = SliceSource::new((r#""\n€""#).as_bytes());
    let mut cursor = TestCursor::new(
        SourceId::new(0),
        &mut byte_input_1,
        &SyntaxLimits::default(),
        StreamLimits::default(),
        &budget,
    )
    .test_value()?;
    let LexEvent::Token(token) = cursor.bump().test_value()? else {
        return Err(TestFailure);
    };
    let text = token.into_decoded_string().test_value()?;
    drop(cursor);
    test_assert_eq!(budget.workspace_bytes(), initial + 4);
    test_assert_eq!(text.as_str(), "\n€");
    test_assert!(text.budget().is_some_and(|owner| owner.same_call(&budget)));
    drop(text);
    test_assert_eq!(budget.workspace_bytes(), initial);

    Ok(())
}

#[test]
fn tree_outlives_source_and_keeps_all_text_and_tables_charged() -> TestResult {
    let budget = execution(LIMIT, LIMIT)?;
    let mut syntax_errors = TestSlot::<SyntaxErrors>::default();
    let mut ast_errors = TestSlot::<AstAccessError>::default();
    let initial = budget.workspace_bytes();
    let ast = {
        let source = String::from(r#"entry struct Root { text: string @regex("\n€") = "plain" }"#);
        parse_with_slot(
            &mut syntax_errors,
            SourceId::new(0),
            &source,
            &SyntaxLimits::default(),
            &budget,
        )
        .test_value()?
    };
    let retained = budget.workspace_bytes();
    test_assert!(retained > initial);
    let item = ast_errors
        .access(|out| ast.items().first(out))
        .test_value()?
        .test_value()?;
    let entry = item.entry_struct().test_value()?;
    let fields = ast_errors.access(|out| entry.fields(out)).test_value()?;
    let field = ast_errors
        .access(|out| fields.first(out))
        .test_value()?
        .test_value()?;
    let annotations = ast_errors
        .access(|out| field.annotations(out))
        .test_value()?;
    let annotation = ast_errors
        .access(|out| annotations.first(out))
        .test_value()?
        .test_value()?;
    let data = ast_errors.access(|out| annotation.data(out)).test_value()?;
    let AnnotationData::Regex(regex) = data else {
        return Err(TestFailure);
    };
    test_assert_eq!(
        ast_errors.access(|out| regex.value(out)).test_value()?,
        "\n€"
    );
    test_assert_eq!(budget.workspace_bytes(), retained);
    drop(ast);
    test_assert_eq!(budget.workspace_bytes(), initial);

    Ok(())
}

#[test]
fn diagnostics_limit_does_not_trigger_limit_sized_preallocation() -> TestResult {
    let initial = execution(LIMIT, LIMIT)?.workspace_bytes();
    let capacity =
        initial + StreamLimits::default().chunk_bytes() * 2 + size_of::<Diagnostic>() * 2;
    let budget = execution(LIMIT, capacity)?;
    let mut syntax_errors = TestSlot::<SyntaxErrors>::default();
    let mut limits = SyntaxLimits::default();
    limits.set_max_diagnostics(usize::MAX);
    drop(parse_with_slot(&mut syntax_errors, SourceId::new(0), "", &limits, &budget).test_value()?);
    test_assert_eq!(budget.workspace_bytes(), initial);
    test_assert!(budget.peak_workspace_bytes() <= capacity);

    Ok(())
}

#[test]
fn parser_work_can_exhaust_a_budget_that_is_sufficient_for_lexing() -> TestResult {
    let source = "entry struct Root { first: u8, second: string @regex(\"a\\n\") }";
    let limits = SyntaxLimits::default();
    let mut syntax_errors = TestSlot::<SyntaxErrors>::default();
    let probe = execution(LIMIT, LIMIT)?;
    let mut byte_input_2 = SliceSource::new((source).as_bytes());
    let mut cursor = TestCursor::new(
        SourceId::new(0),
        &mut byte_input_2,
        &limits,
        StreamLimits::default(),
        &probe,
    )
    .test_value()?;
    while !matches!(cursor.bump().test_value()?, LexEvent::Eof(_)) {}
    let lexical = probe.steps();
    let budget = execution(lexical, LIMIT)?;
    let errors = parse_with_slot(
        &mut syntax_errors,
        SourceId::new(0),
        source,
        &limits,
        &budget,
    )
    .test_error()?;
    test_assert_eq!(
        errors.diagnostics().next_back().test_value()?.code(),
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
    );

    Ok(())
}

#[test]
fn recovery_preserves_previous_errors_and_appends_a_resource_terminal() -> TestResult {
    let source = "? ".repeat(100);
    let mut found = false;
    let mut syntax_errors = TestSlot::<SyntaxErrors>::default();
    for steps in 8192 + source.len()..8192 + source.len() + 200 {
        let budget = execution(steps, LIMIT)?;
        let initial = budget.workspace_bytes();
        let errors = parse_with_slot(
            &mut syntax_errors,
            SourceId::new(0),
            &source,
            &SyntaxLimits::default(),
            &budget,
        )
        .test_error()?;
        let diagnostics = errors.diagnostics();
        if diagnostics.len() >= 2
            && diagnostics.iter().next_back().test_value()?.code()
                == DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
        {
            test_assert_eq!(
                diagnostics.first().test_value()?.code(),
                DiagnosticCode::LEXICAL_ERROR
            );
            found = true;
        }
        drop(errors);
        test_assert_eq!(budget.workspace_bytes(), initial);
        if found {
            break;
        }
    }
    test_assert!(
        found,
        "ordinary errors must be preserved before the recovery budget is exhausted"
    );

    Ok(())
}
