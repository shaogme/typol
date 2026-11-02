use typol_core::{BudgetError, DiagnosticCode, ExecutionBudget, ExecutionLimits, SourceId};
use typol_syntax::{AstAccessError, SchemaParseOptions, SchemaParser, SyntaxErrors, SyntaxLimits};

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

struct ResourceCase {
    input: &'static str,
    height: usize,
    nodes: usize,
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

#[test]
fn every_arena_node_counts_and_whole_tree_heights_match_parent_edges() -> TestResult {
    let mut errors = TestSlot::<AstAccessError>::default();
    let mut budget_errors = TestSlot::<BudgetError>::default();
    let mut syntax_errors = TestSlot::<SyntaxErrors>::default();
    let execution = budget_errors
        .access(|out| ExecutionBudget::new(ExecutionLimits::new(67_108_864, 67_108_864), out))
        .test_value()?;

    for case in [
        ResourceCase {
            input: "",
            height: 0,
            nodes: 0,
        },
        ResourceCase {
            input: "extern mod M;",
            height: 1,
            nodes: 2,
        },
        ResourceCase {
            input: "type A = u8;",
            height: 1,
            nodes: 3,
        },
        ResourceCase {
            input: "type A = option<map<string,[u8; 2]>>;",
            height: 4,
            nodes: 8,
        },
        ResourceCase {
            input: "type A = M::T;",
            height: 3,
            nodes: 6,
        },
        ResourceCase {
            input: "mod M { type A = u8; }",
            height: 2,
            nodes: 5,
        },
        ResourceCase {
            input: "struct A {}",
            height: 1,
            nodes: 2,
        },
        ResourceCase {
            input: "struct A { x: u8 = 0 }",
            height: 3,
            nodes: 7,
        },
        ResourceCase {
            input: "struct A { x: u8 = [0] }",
            height: 5,
            nodes: 9,
        },
        ResourceCase {
            input: "struct A { x: u8 = {k:0} }",
            height: 6,
            nodes: 11,
        },
        ResourceCase {
            input: "struct A { x: u8 = .V(0) }",
            height: 6,
            nodes: 11,
        },
        ResourceCase {
            input: "struct A { x: u8 = .V{k:0} }",
            height: 8,
            nodes: 14,
        },
        ResourceCase {
            input: "enum E { U, S(u8), R { x: u8 } }",
            height: 4,
            nodes: 15,
        },
        ResourceCase {
            input: "struct A {} @assert(true, \"rule\")",
            height: 3,
            nodes: 7,
        },
        ResourceCase {
            input: "struct A {} @assert(x.f(|v| !v), \"rule\")",
            height: 6,
            nodes: 14,
        },
        ResourceCase {
            input: "struct A { x: u8 @all(true, \"rule\") }",
            height: 5,
            nodes: 11,
        },
    ] {
        let ResourceCase {
            input: source,
            height,
            nodes: count,
        } = case;
        let mut limits = SyntaxLimits::default();
        let ast = syntax_errors
            .access(|out| {
                SchemaParser::parse_str(
                    SourceId::new(7),
                    source,
                    &SchemaParseOptions::with_syntax(limits),
                    &execution,
                    out,
                )
            })
            .test_value()?;
        test_assert_eq!(
            errors
                .access(|out| ast.structure_height(out))
                .test_value()?,
            height,
            "{source}"
        );
        test_assert_eq!(ast.node_count(), count, "{source}");
        limits.set_max_schema_ast_nodes(count);
        limits.set_max_nesting_depth(height);
        test_assert!(
            syntax_errors
                .access(|out| {
                    SchemaParser::parse_str(
                        SourceId::new(7),
                        source,
                        &SchemaParseOptions::with_syntax(limits),
                        &execution,
                        out,
                    )
                })
                .is_ok(),
            "{source}"
        );
        for (node_limit, depth_limit) in [
            (count.saturating_sub(1), height),
            (count, height.saturating_sub(1)),
        ] {
            if count == 0 {
                continue;
            }
            limits.set_max_schema_ast_nodes(node_limit);
            limits.set_max_nesting_depth(depth_limit);
            let errors = syntax_errors
                .access(|out| {
                    SchemaParser::parse_str(
                        SourceId::new(7),
                        source,
                        &SchemaParseOptions::with_syntax(limits),
                        &execution,
                        out,
                    )
                })
                .test_error()?;
            test_assert_eq!(
                errors.diagnostics().next_back().test_value()?.code(),
                DiagnosticCode::RESOURCE_LIMIT_EXCEEDED,
                "{source}"
            );
        }
    }

    Ok(())
}

#[test]
fn node_budget_is_independent_of_diagnostic_budget_and_includes_failed_constructions() -> TestResult
{
    let mut budget_errors = TestSlot::<BudgetError>::default();
    let mut syntax_errors = TestSlot::<SyntaxErrors>::default();
    let execution = budget_errors
        .access(|out| ExecutionBudget::new(ExecutionLimits::new(67_108_864, 67_108_864), out))
        .test_value()?;

    for diagnostics in [0, 1, 2, 100] {
        let mut limits = SyntaxLimits::default();
        limits.set_max_schema_ast_nodes(0);
        limits.set_max_diagnostics(diagnostics);
        let error = syntax_errors
            .access(|out| {
                SchemaParser::parse_str(
                    SourceId::new(7),
                    "type A = u8;",
                    &SchemaParseOptions::with_syntax(limits),
                    &execution,
                    out,
                )
            })
            .test_error()?;
        test_assert_eq!(
            error.diagnostics().next_back().test_value()?.code(),
            DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
        );
        test_assert_eq!(
            error
                .diagnostics()
                .first()
                .test_value()?
                .primary()
                .test_value()?
                .span()
                .start(),
            5
        );
    }
    let mut limits = SyntaxLimits::default();
    limits.set_max_schema_ast_nodes(3);
    let errors = syntax_errors
        .access(|out| {
            SchemaParser::parse_str(
                SourceId::new(7),
                "struct A { bad: u8 = , } type B = u8;",
                &SchemaParseOptions::with_syntax(limits),
                &execution,
                out,
            )
        })
        .test_error()?;
    test_assert_eq!(
        errors.diagnostics().first().test_value()?.code(),
        DiagnosticCode::SCHEMA_SYNTAX_ERROR
    );
    test_assert_eq!(
        errors.diagnostics().next_back().test_value()?.code(),
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
    );

    Ok(())
}

#[test]
fn typed_annotation_payloads_have_independent_count_and_height_boundaries() -> TestResult {
    let mut errors = TestSlot::<AstAccessError>::default();
    let mut budget_errors = TestSlot::<BudgetError>::default();
    let mut syntax_errors = TestSlot::<SyntaxErrors>::default();
    let execution = budget_errors
        .access(|out| ExecutionBudget::new(ExecutionLimits::new(67_108_864, 67_108_864), out))
        .test_value()?;

    for case in [
        ResourceCase {
            input: "@min(0)",
            height: 2,
            nodes: 6,
        },
        ResourceCase {
            input: "@range(0, 1)",
            height: 2,
            nodes: 7,
        },
        ResourceCase {
            input: "@length(0, 1)",
            height: 2,
            nodes: 7,
        },
        ResourceCase {
            input: "@regex(\"x\")",
            height: 2,
            nodes: 6,
        },
        ResourceCase {
            input: "@host",
            height: 1,
            nodes: 5,
        },
        ResourceCase {
            input: "@email",
            height: 1,
            nodes: 5,
        },
        ResourceCase {
            input: "@cidr(v4_only = false)",
            height: 2,
            nodes: 7,
        },
        ResourceCase {
            input: "@env(\"X\")",
            height: 2,
            nodes: 6,
        },
        ResourceCase {
            input: "@size(0, 1)",
            height: 2,
            nodes: 7,
        },
        ResourceCase {
            input: "@size(min = 0, max = 1)",
            height: 2,
            nodes: 9,
        },
        ResourceCase {
            input: "@unique",
            height: 1,
            nodes: 5,
        },
        ResourceCase {
            input: "@unique(by = \"x\")",
            height: 2,
            nodes: 7,
        },
        ResourceCase {
            input: "@sum(min = 0)",
            height: 2,
            nodes: 7,
        },
        ResourceCase {
            input: "@secret",
            height: 1,
            nodes: 5,
        },
    ] {
        let ResourceCase {
            input: annotation,
            height,
            nodes,
        } = case;
        let source = format!("type A = u8 {annotation};");
        let mut limits = SyntaxLimits::default();
        limits.set_max_schema_ast_nodes(nodes);
        limits.set_max_nesting_depth(height);
        let ast = syntax_errors
            .access(|out| {
                SchemaParser::parse_str(
                    SourceId::new(7),
                    &source,
                    &SchemaParseOptions::with_syntax(limits),
                    &execution,
                    out,
                )
            })
            .test_value()?;
        test_assert_eq!(ast.node_count(), nodes, "{annotation}");
        test_assert_eq!(
            errors
                .access(|out| ast.structure_height(out))
                .test_value()?,
            height,
            "{annotation}"
        );
        limits.set_max_nesting_depth(height - 1);
        test_assert_eq!(
            syntax_errors
                .access(|out| {
                    SchemaParser::parse_str(
                        SourceId::new(7),
                        &source,
                        &SchemaParseOptions::with_syntax(limits),
                        &execution,
                        out,
                    )
                })
                .test_error()?
                .diagnostics()
                .next_back()
                .test_value()?
                .code(),
            DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
        );
    }
    test_assert_eq!(SyntaxLimits::default().max_schema_ast_nodes(), 100_000);

    Ok(())
}
