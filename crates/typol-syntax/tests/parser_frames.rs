use std::{
    env,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use typol_core::{BudgetError, DiagnosticCode, ExecutionBudget, ExecutionLimits, SourceId};
use typol_syntax::{
    AstAccessError, BinaryOperator, Declaration, DefaultValueKind, EnumDefaultPayloadKind,
    ExprKind, MethodArgumentKind, SchemaAst, SchemaParseOptions, SchemaParser, SyntaxErrors,
    SyntaxLimits, TypeExprKind,
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

struct ValueNestingCase {
    opening: &'static str,
    atom: &'static str,
    closing: &'static str,
    layers: usize,
}

struct ExpressionNestingCase {
    opening: &'static str,
    closing: &'static str,
    layers: usize,
}

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

fn parse(
    errors: &mut TestErrors,
    source: &str,
    limits: &SyntaxLimits,
    execution: &ExecutionBudget,
) -> Result<SchemaAst, SyntaxErrors> {
    errors.parse(SOURCE_ID, source, limits, execution)
}

fn check_limit(source: &str, limit: usize, required: usize, errors: &mut TestErrors) -> TestResult {
    let execution = errors
        .execution(ExecutionLimits::new(67_108_864, 67_108_864))
        .test_value()?;

    let mut limits = SyntaxLimits::default();
    limits.set_max_nesting_depth(limit);
    let result = parse(errors, source, &limits, &execution);
    if required <= limit {
        test_assert!(
            result.is_ok(),
            "limit={limit}, required={required}, source={source}"
        );
    } else {
        test_assert!(
            result.is_err(),
            "limit={limit}, required={required}, source={source}"
        );
        let errors = result.test_error()?;
        test_assert_eq!(
            errors.diagnostics().next_back().test_value()?.code(),
            DiagnosticCode::RESOURCE_LIMIT_EXCEEDED,
            "limit={limit}, required={required}, source={source}"
        );
    }

    Ok(())
}

#[test]
fn syntax_frames_and_completed_expression_heights_have_independent_limits() -> TestResult {
    let mut errors = TestErrors::new();
    for limit in [0_usize, 1, 2, 8, 128] {
        for depth in [limit.saturating_sub(1), limit, limit + 1] {
            for (open, close) in [("option<", ">"), ("map<string,", ">"), ("[", "]")] {
                check_limit(
                    &format!("type A = {}u8{};", open.repeat(depth), close.repeat(depth)),
                    limit,
                    depth + 1,
                    &mut errors,
                )?;
            }
            check_limit(
                &format!(
                    "{}type A = u8;{}",
                    "mod M {".repeat(depth),
                    "}".repeat(depth)
                ),
                limit,
                depth + 1,
                &mut errors,
            )?;
            for case in [
                ValueNestingCase {
                    opening: "[",
                    atom: "0",
                    closing: "]",
                    layers: 2,
                },
                ValueNestingCase {
                    opening: "{key:",
                    atom: "0",
                    closing: "}",
                    layers: 3,
                },
                ValueNestingCase {
                    opening: ".V(",
                    atom: "0",
                    closing: ")",
                    layers: 3,
                },
                ValueNestingCase {
                    opening: ".V{key:",
                    atom: "0",
                    closing: "}",
                    layers: 5,
                },
            ] {
                check_limit(
                    &format!(
                        "struct A {{ x: u8 = {}{}{} }}",
                        case.opening.repeat(depth),
                        case.atom,
                        case.closing.repeat(depth)
                    ),
                    limit,
                    depth * case.layers + 3,
                    &mut errors,
                )?;
            }
            for case in [
                ExpressionNestingCase {
                    opening: "(",
                    closing: ")",
                    layers: 0,
                },
                ExpressionNestingCase {
                    opening: "x.f(",
                    closing: ")",
                    layers: 2,
                },
                ExpressionNestingCase {
                    opening: "x.f(|v|",
                    closing: ")",
                    layers: 2,
                },
                ExpressionNestingCase {
                    opening: "!",
                    closing: "",
                    layers: 1,
                },
            ] {
                check_limit(
                    &format!(
                        "struct A {{}} @assert({}true{}, \"rule\")",
                        case.opening.repeat(depth),
                        case.closing.repeat(depth)
                    ),
                    limit,
                    (depth + 1).max(depth * case.layers + 3),
                    &mut errors,
                )?;
            }
            check_limit(
                &format!("struct A {{}} @assert(1{}, \"rule\")", " + 1".repeat(depth)),
                limit,
                depth + 3,
                &mut errors,
            )?;
            check_limit(
                &format!("struct A {{}} @assert(x{}, \"rule\")", ".f".repeat(depth)),
                limit,
                depth + 3,
                &mut errors,
            )?;
            check_limit(
                &format!("struct A {{}} @assert(x{}, \"rule\")", ".f()".repeat(depth)),
                limit,
                depth + 3,
                &mut errors,
            )?;
        }
    }

    Ok(())
}

#[test]
fn mixed_type_frames_preserve_array_lengths_and_recovery_releases_prefix_frames() -> TestResult {
    let mut errors = TestErrors::new();
    let execution = errors
        .execution(ExecutionLimits::new(67_108_864, 67_108_864))
        .test_value()?;

    check_limit(
        "type A = option<map<string, [option<u8>; 3]>>;",
        5,
        5,
        &mut errors,
    )?;
    for source in [
        "struct A { bad: option<[u8, good: option<u8>, } struct B { broken: }",
        "struct A { bad: u8 @all(!, \"rule\"), good: u8 @all(!true, \"rule\"), } struct B { broken: }",
        "mod M { mod N { struct A { bad: } type Good = option<u8>; } type Outer = u8; } struct B { broken: }",
        "struct A { bad: u8 = [} struct B { broken: }",
    ] {
        let mut limits = SyntaxLimits::default();
        limits.set_max_nesting_depth(8);
        let syntax_errors = parse(&mut errors, source, &limits, &execution).test_error()?;
        test_assert!(
            syntax_errors
                .diagnostics()
                .iter()
                .all(|error| error.code() == DiagnosticCode::SCHEMA_SYNTAX_ERROR)
        );
        let broken_start = source.rfind("broken").test_value()?;
        let mut recovered_broken_declaration = false;
        for error in syntax_errors.diagnostics() {
            let start = error.primary().test_value()?.span().start();
            if start >= broken_start {
                recovered_broken_declaration = true;
                break;
            }
        }
        test_assert!(
            recovered_broken_declaration,
            "recovery must reach the next independent declaration: {source}"
        );
    }

    Ok(())
}

#[test]
fn completed_child_frames_keep_comparison_scope_and_postfix_binding() -> TestResult {
    let mut errors = TestErrors::new();
    let mut ast_errors = TestSlot::<AstAccessError>::default();
    let execution = errors
        .execution(ExecutionLimits::new(67_108_864, 67_108_864))
        .test_value()?;

    for expression in [
        "(a < b) == (c < d)",
        "a < b && c < d || e < f",
        "x.f(a < b).g(|v| v < c) == (d < e)",
        "-x.f(1 + 2).value * 3",
        "((x)).f().value == 1",
    ] {
        let source = format!("struct A {{}} @assert({expression}, \"rule\")");
        test_assert!(
            parse(&mut errors, &source, &SyntaxLimits::default(), &execution).is_ok(),
            "{expression}"
        );
    }
    for expression in ["a < b == c", "x.f(a < b < c)", "(a < b == c)"] {
        let source = format!("struct A {{}} @assert({expression}, \"rule\")");
        test_assert_eq!(
            parse(&mut errors, &source, &SyntaxLimits::default(), &execution)
                .test_error()?
                .diagnostics()
                .first()
                .test_value()?
                .code(),
            DiagnosticCode::SCHEMA_SYNTAX_ERROR
        );
    }
    let source = "entry struct A {} @assert((1 + 2) * 3 - 4 - 5, \"rule\")";
    let schema = parse(&mut errors, source, &SyntaxLimits::default(), &execution).test_value()?;
    let item = ast_errors
        .access(|out| schema.items().first(out))
        .test_value()?
        .test_value()?;
    let root = item.entry_struct().test_value()?;
    let assertions = ast_errors.access(|out| root.assertions(out)).test_value()?;
    let assertion = ast_errors
        .access(|out| assertions.first(out))
        .test_value()?
        .test_value()?;
    let expression = ast_errors
        .access(|out| assertion.expression(out))
        .test_value()?;
    let kind = ast_errors.access(|out| expression.kind(out)).test_value()?;
    let ExprKind::Binary {
        operator: BinaryOperator::Subtract,
        left,
        ..
    } = kind
    else {
        return Err(TestFailure);
    };
    test_assert!(matches!(
        ast_errors.access(|out| left.kind(out)).test_value()?,
        ExprKind::Binary {
            operator: BinaryOperator::Subtract,
            ..
        }
    ));

    Ok(())
}

#[test]
fn parser_stack_child() -> TestResult {
    if env::var_os("TYPOL_FRAME_STACK_CHILD").is_none() {
        return Ok(());
    }
    thread::Builder::new()
        .stack_size(160 * 1024)
        .spawn(|| -> TestResult {
            let mut errors = TestErrors::new();
            let mut ast_errors = TestSlot::<AstAccessError>::default();
            let execution = errors
                .execution(ExecutionLimits::new(67_108_864, 67_108_864))
                .test_value()?;

            let depth = 8192;
            let mut limits = SyntaxLimits::default();
            limits.set_max_nesting_depth(depth * 5 + 4);
            // A fixed, small stack also covers reading, debugging, and dropping a successfully built deep tree.
            check_deep_owned_trees(depth, &limits, &execution, &mut errors, &mut ast_errors)?;
            let source = format!(
                "struct A {{}} @assert({}true{}, \"rule\")",
                "(".repeat(depth),
                ")".repeat(depth)
            );
            let schema = parse(&mut errors, &source, &limits, &execution).test_value()?;
            drop(schema);
            for source in [
                format!("type A = {}", "option<".repeat(depth)),
                format!("type A = {}", "map<string,".repeat(depth)),
                format!("type A = {}", "[".repeat(depth)),
                "mod M {".repeat(depth),
                format!("struct A {{ x: u8 = {}", "[".repeat(depth)),
                format!("struct A {{ x: u8 = {}", "{key:".repeat(depth)),
                format!("struct A {{ x: u8 = {}", ".V(".repeat(depth)),
                format!("struct A {{}} @assert({}", "!".repeat(depth)),
                format!("struct A {{}} @assert({}", "x.f(|v|".repeat(depth)),
            ] {
                let syntax_errors =
                    parse(&mut errors, &source, &limits, &execution).test_error()?;
                test_assert_eq!(
                    syntax_errors.diagnostics().next_back().test_value()?.code(),
                    DiagnosticCode::SCHEMA_SYNTAX_ERROR
                );
            }

            Ok(())
        })
        .test_value()?
        .join()
        .test_value()??;

    Ok(())
}

fn check_deep_owned_trees(
    depth: usize,
    limits: &SyntaxLimits,
    execution: &ExecutionBudget,
    errors: &mut TestErrors,
    ast_errors: &mut TestSlot<AstAccessError>,
) -> TestResult {
    check_deep_type_trees(depth, limits, execution, errors, ast_errors)?;
    check_deep_module_tree(depth, limits, execution, errors, ast_errors)?;
    check_deep_default_trees(depth, limits, execution, errors, ast_errors)?;
    check_deep_expression_trees(depth, limits, execution, errors, ast_errors)
}

fn check_deep_type_trees(
    depth: usize,
    limits: &SyntaxLimits,
    execution: &ExecutionBudget,
    errors: &mut TestErrors,
    ast_errors: &mut TestSlot<AstAccessError>,
) -> TestResult {
    for (open, close) in [("option<", ">"), ("map<string,", ">"), ("[", "]")] {
        let source = format!("type A = {}u8{};", open.repeat(depth), close.repeat(depth));
        let ast = parse(errors, &source, limits, execution).test_value()?;
        let item = ast_errors
            .access(|out| ast.items().first(out))
            .test_value()?
            .test_value()?;
        let Some(Declaration::TypeAlias(alias)) = item.declaration() else {
            return Err(TestFailure);
        };
        let mut ty = ast_errors.access(|out| alias.target(out)).test_value()?;
        let mut count = 0;
        loop {
            ty = match ast_errors.access(|out| ty.kind(out)).test_value()? {
                TypeExprKind::Option(inner) => inner,
                TypeExprKind::Map { value } => value,
                TypeExprKind::Array { element, .. } => element,
                TypeExprKind::Builtin(_) => break,
                _ => return Err(TestFailure),
            };
            count += 1;
        }
        test_assert_eq!(count, depth);
        drop(ast);
    }
    Ok(())
}

fn check_deep_module_tree(
    depth: usize,
    limits: &SyntaxLimits,
    execution: &ExecutionBudget,
    errors: &mut TestErrors,
    ast_errors: &mut TestSlot<AstAccessError>,
) -> TestResult {
    let source = format!(
        "{}type A = u8;{}",
        "mod M {".repeat(depth),
        "}".repeat(depth)
    );
    let ast = parse(errors, &source, limits, execution).test_value()?;
    let item = ast_errors
        .access(|out| ast.items().first(out))
        .test_value()?
        .test_value()?;
    let mut declaration = item.declaration().test_value()?;
    let mut count = 0;
    while let Declaration::Module(module) = declaration {
        let declarations = ast_errors
            .access(|out| module.declarations(out))
            .test_value()?;
        declaration = ast_errors
            .access(|out| declarations.first(out))
            .test_value()?
            .test_value()?;
        count += 1;
    }
    test_assert_eq!(count, depth);
    drop(ast);
    Ok(())
}

fn check_deep_default_trees(
    depth: usize,
    limits: &SyntaxLimits,
    execution: &ExecutionBudget,
    errors: &mut TestErrors,
    ast_errors: &mut TestSlot<AstAccessError>,
) -> TestResult {
    for (open, close) in [("[", "]"), ("{key:", "}"), (".V(", ")"), (".V{key:", "}")] {
        let source = format!(
            "entry struct A {{ x: u8 = {}0{} }}",
            open.repeat(depth),
            close.repeat(depth)
        );
        let ast = parse(errors, &source, limits, execution).test_value()?;
        let item = ast_errors
            .access(|out| ast.items().first(out))
            .test_value()?
            .test_value()?;
        let root = item.entry_struct().test_value()?;
        let fields = ast_errors.access(|out| root.fields(out)).test_value()?;
        let field = ast_errors
            .access(|out| fields.first(out))
            .test_value()?
            .test_value()?;
        let mut value = ast_errors
            .access(|out| field.default(out))
            .test_value()?
            .test_value()?;
        let mut count = 0;
        loop {
            value = match ast_errors.access(|out| value.kind(out)).test_value()? {
                DefaultValueKind::Array(array) => {
                    let elements = ast_errors.access(|out| array.elements(out)).test_value()?;
                    ast_errors
                        .access(|out| elements.first(out))
                        .test_value()?
                        .test_value()?
                }
                DefaultValueKind::Braced(braced) => {
                    let entries = ast_errors.access(|out| braced.entries(out)).test_value()?;
                    let entry = ast_errors
                        .access(|out| entries.first(out))
                        .test_value()?
                        .test_value()?;
                    ast_errors.access(|out| entry.value(out)).test_value()?
                }
                DefaultValueKind::Enum(enumeration) => {
                    let payload = ast_errors
                        .access(|out| enumeration.payload(out))
                        .test_value()?;
                    match ast_errors.access(|out| payload.kind(out)).test_value()? {
                        EnumDefaultPayloadKind::Single(value) => value,
                        EnumDefaultPayloadKind::Structure(braced) => {
                            let entries =
                                ast_errors.access(|out| braced.entries(out)).test_value()?;
                            let entry = ast_errors
                                .access(|out| entries.first(out))
                                .test_value()?
                                .test_value()?;
                            ast_errors.access(|out| entry.value(out)).test_value()?
                        }
                        _ => return Err(TestFailure),
                    }
                }
                DefaultValueKind::Scalar(_) => break,
                _ => return Err(TestFailure),
            };
            count += 1;
        }
        test_assert_eq!(count, depth);
        drop(ast);
    }
    Ok(())
}

fn check_deep_expression_trees(
    depth: usize,
    limits: &SyntaxLimits,
    execution: &ExecutionBudget,
    errors: &mut TestErrors,
    ast_errors: &mut TestSlot<AstAccessError>,
) -> TestResult {
    for expression in [
        format!("{}true", "!".repeat(depth)),
        format!("1{}", " + 1".repeat(depth)),
        format!("x{}", ".f()".repeat(depth)),
        format!("{}true{}", "x.f(|v|".repeat(depth), ")".repeat(depth)),
    ] {
        let source = format!("entry struct A {{}} @assert({expression}, \"rule\")");
        let ast = parse(errors, &source, limits, execution).test_value()?;
        let item = ast_errors
            .access(|out| ast.items().first(out))
            .test_value()?
            .test_value()?;
        let root = item.entry_struct().test_value()?;
        let assertions = ast_errors.access(|out| root.assertions(out)).test_value()?;
        let assertion = ast_errors
            .access(|out| assertions.first(out))
            .test_value()?
            .test_value()?;
        let mut expression = ast_errors
            .access(|out| assertion.expression(out))
            .test_value()?;
        let mut count = 0;
        loop {
            expression = match ast_errors.access(|out| expression.kind(out)).test_value()? {
                ExprKind::Unary { operand, .. } => operand,
                ExprKind::Binary { left, .. } => left,
                ExprKind::MethodCall {
                    receiver, argument, ..
                } => match argument {
                    Some(argument) => {
                        match ast_errors.access(|out| argument.kind(out)).test_value()? {
                            MethodArgumentKind::Closure { body, .. } => body,
                            _ => return Err(TestFailure),
                        }
                    }
                    None => receiver,
                },
                ExprKind::Literal(_) | ExprKind::Name(_) => break,
                _ => return Err(TestFailure),
            };
            count += 1;
        }
        test_assert_eq!(count, depth);
        test_assert!(format!("{ast:?}").contains("SchemaAst"));
        drop(ast);
    }

    Ok(())
}

#[test]
fn deep_parsing_uses_heap_frames_in_a_bounded_stack_process() -> TestResult {
    let mut child = Command::new(env::current_exe().test_value()?)
        .args(["--exact", "parser_stack_child"])
        .env("TYPOL_FRAME_STACK_CHILD", "1")
        .stdout(Stdio::null())
        .spawn()
        .test_value()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait().test_value()? {
            test_assert!(
                status.success(),
                "explicit-frame parser child process failed"
            );
            return Ok(());
        }
        if Instant::now() >= deadline {
            child.kill().test_value()?;
            child.wait().test_value()?;
            return Err(TestFailure);
        }
        thread::sleep(Duration::from_millis(10));
    }
}
