use typol_core::{BudgetError, DiagnosticCode, ExecutionBudget, ExecutionLimits, SourceId};
use typol_syntax::{
    AstAccessError, BinaryOperator, ExprKind, MethodArgumentKind, SchemaAst, SchemaParseOptions,
    SchemaParser, SyntaxErrors, SyntaxLimits, UnaryOperator,
};

use typol_test_support::TestSlot;

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

fn new_execution() -> TestResult<ExecutionBudget> {
    TestSlot::<BudgetError>::default()
        .access(|out| ExecutionBudget::new(ExecutionLimits::new(67_108_864, 67_108_864), out))
        .test_value()
}

fn parse_with_slot(
    slot: &mut TestSlot<SyntaxErrors>,
    source: &str,
    limits: &SyntaxLimits,
    execution: &ExecutionBudget,
) -> Result<SchemaAst, SyntaxErrors> {
    let options = SchemaParseOptions::with_syntax(*limits);
    slot.access(|out| SchemaParser::parse_str(SOURCE_ID, source, &options, execution, out))
}

#[test]
fn arithmetic_comparison_and_boolean_operators_follow_schema_precedence() -> TestResult {
    let execution = new_execution()?;
    let mut syntax_errors = TestSlot::<SyntaxErrors>::default();
    let mut ast_errors = TestSlot::<AstAccessError>::default();

    let source = "entry struct Root {} @assert(1 + 2 * 3 > 7 && true || false, \"message\")";
    let schema = parse_with_slot(
        &mut syntax_errors,
        source,
        &SyntaxLimits::default(),
        &execution,
    )
    .test_value()?;
    let item = ast_errors
        .access(|out| schema.items().first(out))
        .test_value()?
        .test_value()?;
    let entry = item.entry_struct().test_value()?;
    let assertions = ast_errors
        .access(|out| entry.assertions(out))
        .test_value()?;
    let assertion = ast_errors
        .access(|out| assertions.first(out))
        .test_value()?
        .test_value()?;
    let expression = ast_errors
        .access(|out| assertion.expression(out))
        .test_value()?;

    let ExprKind::Binary {
        operator: BinaryOperator::Or,
        left,
        ..
    } = ast_errors.access(|out| expression.kind(out)).test_value()?
    else {
        return Err(TestFailure);
    };
    let ExprKind::Binary {
        operator: BinaryOperator::And,
        left,
        ..
    } = ast_errors.access(|out| left.kind(out)).test_value()?
    else {
        return Err(TestFailure);
    };
    let ExprKind::Binary {
        operator: BinaryOperator::Greater,
        left,
        ..
    } = ast_errors.access(|out| left.kind(out)).test_value()?
    else {
        return Err(TestFailure);
    };
    let ExprKind::Binary {
        operator: BinaryOperator::Add,
        right,
        ..
    } = ast_errors.access(|out| left.kind(out)).test_value()?
    else {
        return Err(TestFailure);
    };
    test_assert!(matches!(
        ast_errors.access(|out| right.kind(out)).test_value()?,
        ExprKind::Binary {
            operator: BinaryOperator::Multiply,
            ..
        }
    ));

    Ok(())
}

#[test]
fn binary_operators_are_left_associative_and_unary_operators_bind_tightly() -> TestResult {
    let execution = new_execution()?;
    let mut syntax_errors = TestSlot::<SyntaxErrors>::default();
    let mut ast_errors = TestSlot::<AstAccessError>::default();

    let source = "entry struct Root {} @assert(-1 - 2 - 3 < 0 && !false, \"message\")";
    let schema = parse_with_slot(
        &mut syntax_errors,
        source,
        &SyntaxLimits::default(),
        &execution,
    )
    .test_value()?;
    let item = ast_errors
        .access(|out| schema.items().first(out))
        .test_value()?
        .test_value()?;
    let entry = item.entry_struct().test_value()?;
    let assertions = ast_errors
        .access(|out| entry.assertions(out))
        .test_value()?;
    let assertion = ast_errors
        .access(|out| assertions.first(out))
        .test_value()?
        .test_value()?;
    let expression = ast_errors
        .access(|out| assertion.expression(out))
        .test_value()?;

    let ExprKind::Binary {
        operator: BinaryOperator::And,
        left,
        right,
    } = ast_errors.access(|out| expression.kind(out)).test_value()?
    else {
        return Err(TestFailure);
    };
    test_assert!(matches!(
        ast_errors.access(|out| right.kind(out)).test_value()?,
        ExprKind::Unary {
            operator: UnaryOperator::Not,
            ..
        }
    ));
    let ExprKind::Binary {
        operator: BinaryOperator::Less,
        left,
        ..
    } = ast_errors.access(|out| left.kind(out)).test_value()?
    else {
        return Err(TestFailure);
    };
    let ExprKind::Binary {
        operator: BinaryOperator::Subtract,
        left: nested,
        ..
    } = ast_errors.access(|out| left.kind(out)).test_value()?
    else {
        return Err(TestFailure);
    };
    test_assert!(matches!(
        ast_errors.access(|out| nested.kind(out)).test_value()?,
        ExprKind::Binary {
            operator: BinaryOperator::Subtract,
            ..
        }
    ));

    Ok(())
}

#[test]
fn every_binary_operator_has_a_structured_ast_variant() -> TestResult {
    let execution = new_execution()?;
    let mut syntax_errors = TestSlot::<SyntaxErrors>::default();
    let mut ast_errors = TestSlot::<AstAccessError>::default();

    let cases = [
        ("1 * 2", BinaryOperator::Multiply),
        ("1 / 2", BinaryOperator::Divide),
        ("1 + 2", BinaryOperator::Add),
        ("1 - 2", BinaryOperator::Subtract),
        ("1 == 2", BinaryOperator::Equal),
        ("1 != 2", BinaryOperator::NotEqual),
        ("1 < 2", BinaryOperator::Less),
        ("1 <= 2", BinaryOperator::LessEqual),
        ("1 > 2", BinaryOperator::Greater),
        ("1 >= 2", BinaryOperator::GreaterEqual),
        ("true && false", BinaryOperator::And),
        ("true || false", BinaryOperator::Or),
    ];

    for (expression, expected) in cases {
        let source = format!("entry struct Root {{}} @assert({expression}, \"message\")");
        let schema = parse_with_slot(
            &mut syntax_errors,
            &source,
            &SyntaxLimits::default(),
            &execution,
        )
        .test_value()?;
        let item = ast_errors
            .access(|out| schema.items().first(out))
            .test_value()?
            .test_value()?;
        let entry = item.entry_struct().test_value()?;
        let assertions = ast_errors
            .access(|out| entry.assertions(out))
            .test_value()?;
        let assertion = ast_errors
            .access(|out| assertions.first(out))
            .test_value()?
            .test_value()?;
        let expression = ast_errors
            .access(|out| assertion.expression(out))
            .test_value()?;
        test_assert!(matches!(
            ast_errors.access(|out| expression.kind(out)).test_value()?,
            ExprKind::Binary { operator, .. } if operator == expected
        ));
    }

    Ok(())
}

#[test]
fn field_and_method_chains_keep_closure_arguments_structured() -> TestResult {
    let execution = new_execution()?;
    let mut syntax_errors = TestSlot::<SyntaxErrors>::default();
    let mut ast_errors = TestSlot::<AstAccessError>::default();

    let source = r#"entry struct Root {} @assert(self.items.all(|item| item.value > 0) && !self.value.is_none(), "message")"#;
    let schema = parse_with_slot(
        &mut syntax_errors,
        source,
        &SyntaxLimits::default(),
        &execution,
    )
    .test_value()?;
    let item = ast_errors
        .access(|out| schema.items().first(out))
        .test_value()?
        .test_value()?;
    let entry = item.entry_struct().test_value()?;
    let assertions = ast_errors
        .access(|out| entry.assertions(out))
        .test_value()?;
    let assertion = ast_errors
        .access(|out| assertions.first(out))
        .test_value()?
        .test_value()?;
    let expression = ast_errors
        .access(|out| assertion.expression(out))
        .test_value()?;
    let ExprKind::Binary { left, right, .. } =
        ast_errors.access(|out| expression.kind(out)).test_value()?
    else {
        return Err(TestFailure);
    };

    let ExprKind::MethodCall {
        receiver,
        method,
        argument: Some(argument),
    } = ast_errors.access(|out| left.kind(out)).test_value()?
    else {
        return Err(TestFailure);
    };
    test_assert_eq!(
        ast_errors.access(|out| method.text(out)).test_value()?,
        "all"
    );
    test_assert!(matches!(
        ast_errors.access(|out| receiver.kind(out)).test_value()?,
        ExprKind::FieldAccess { field, .. } if ast_errors.access(|out| field.text(out)).test_value()? == "items"
    ));
    test_assert!(matches!(
        ast_errors.access(|out| argument.kind(out)).test_value()?,
        MethodArgumentKind::Closure { parameter, body }
            if ast_errors.access(|out| parameter.text(out)).test_value()? == "item" && matches!(ast_errors.access(|out| body.kind(out)).test_value()?, ExprKind::Binary { .. })
    ));

    let ExprKind::Unary {
        operator: UnaryOperator::Not,
        operand,
    } = ast_errors.access(|out| right.kind(out)).test_value()?
    else {
        return Err(TestFailure);
    };
    test_assert!(matches!(
        ast_errors.access(|out| operand.kind(out)).test_value()?,
        ExprKind::MethodCall { method, argument: None, .. } if ast_errors.access(|out| method.text(out)).test_value()? == "is_none"
    ));

    Ok(())
}

#[test]
fn methods_accept_expression_and_closure_arguments() -> TestResult {
    let execution = new_execution()?;
    let mut syntax_errors = TestSlot::<SyntaxErrors>::default();
    let mut ast_errors = TestSlot::<AstAccessError>::default();

    let source = r#"entry struct Root {} @assert(self.label.matches("x") && self.values.any(|value| value == 2), "message")"#;
    let schema = parse_with_slot(
        &mut syntax_errors,
        source,
        &SyntaxLimits::default(),
        &execution,
    )
    .test_value()?;
    let item = ast_errors
        .access(|out| schema.items().first(out))
        .test_value()?
        .test_value()?;
    let entry = item.entry_struct().test_value()?;
    let assertions = ast_errors
        .access(|out| entry.assertions(out))
        .test_value()?;
    let assertion = ast_errors
        .access(|out| assertions.first(out))
        .test_value()?
        .test_value()?;
    let expression = ast_errors
        .access(|out| assertion.expression(out))
        .test_value()?;
    let ExprKind::Binary { left, right, .. } =
        ast_errors.access(|out| expression.kind(out)).test_value()?
    else {
        return Err(TestFailure);
    };

    test_assert!(matches!(
        ast_errors.access(|out| left.kind(out)).test_value()?,
        ExprKind::MethodCall {
            method,
            argument: Some(argument),
            ..
        } if ast_errors.access(|out| method.text(out)).test_value()? == "matches" && matches!(ast_errors.access(|out| argument.kind(out)).test_value()?, MethodArgumentKind::Expression(_))
    ));
    test_assert!(matches!(
        ast_errors.access(|out| right.kind(out)).test_value()?,
        ExprKind::MethodCall {
            method,
            argument: Some(argument),
            ..
        } if ast_errors.access(|out| method.text(out)).test_value()? == "any" && matches!(ast_errors.access(|out| argument.kind(out)).test_value()?, MethodArgumentKind::Closure { .. })
    ));

    Ok(())
}

#[test]
fn malformed_expression_grammar_is_rejected_with_schema_diagnostics() -> TestResult {
    let execution = new_execution()?;
    let mut syntax_errors = TestSlot::<SyntaxErrors>::default();

    for expression in ["1 < 2 < 3", "1 +", "self.items.all(|item| item > )"] {
        let source = format!(
            "entry struct Root {{}} @assert({}, \"message\")",
            expression
        );
        let diagnostics = parse_with_slot(
            &mut syntax_errors,
            &source,
            &SyntaxLimits::default(),
            &execution,
        )
        .test_error()?;
        let diagnostics = diagnostics.diagnostics();
        test_assert!(!diagnostics.is_empty());
        test_assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code() == DiagnosticCode::SCHEMA_SYNTAX_ERROR)
        );
    }

    Ok(())
}
