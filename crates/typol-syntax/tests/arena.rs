use typol_core::{BudgetError, ExecutionBudget, ExecutionLimits, SourceId};
use typol_syntax::{
    AnnotationData, AstAccessError, BraceKey, Declaration, DefaultValueKind, ExprKind,
    MethodArgumentKind, ScalarLiteral, SchemaAst, SchemaParseOptions, SchemaParser, SyntaxErrors,
    SyntaxLimits, TypeExprKind,
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

const SOURCE_ID: SourceId = SourceId::new(7);

struct IteratorStats<'a> {
    names: Vec<&'a str>,
    lengths: Vec<usize>,
    hints: Vec<(usize, Option<usize>)>,
    exhausted: bool,
    complete: bool,
}

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
fn pooled_text_and_nested_lists_survive_source_release_and_table_growth() -> TestResult {
    let execution = new_execution()?;
    let mut syntax_errors = TestSlot::<SyntaxErrors>::default();
    let mut ast_errors = TestSlot::<AstAccessError>::default();

    let ast = {
        let mut source = String::from(
            r#"mod Outer {
                mod Inner { type T = [u8; 123]; }
                type First = Inner::T;
            }
            entry struct Root {
                text: string @regex("€\n₽") = "€source",
                number: i128 = -0x2a,
                records: [u8] = [{key: [1, 2]}, {"€\n₽": [3, 4]}],
            }
            @assert(self.text.f(|item| item == "€€\n₽"), "assertion\nmessage")
            "#,
        );
        for index in 0..1024 {
            source.push_str(&format!("type T{index} = u8;"));
        }
        parse_with_slot(
            &mut syntax_errors,
            &source,
            &SyntaxLimits::default(),
            &execution,
        )
        .test_value()?
    };
    let top_level = ast_errors
        .access(|out| ast.items().first(out))
        .test_value()?
        .test_value()?;
    let Declaration::Module(outer) = top_level.declaration().test_value()? else {
        return Err(TestFailure);
    };
    let declarations = ast_errors
        .access(|out| outer.declarations(out))
        .test_value()?;
    let inner = ast_errors
        .access(|out| declarations.first(out))
        .test_value()?
        .test_value()?;
    let Declaration::Module(inner) = inner else {
        return Err(TestFailure);
    };
    let declarations = ast_errors
        .access(|out| inner.declarations(out))
        .test_value()?;
    let alias = ast_errors
        .access(|out| declarations.first(out))
        .test_value()?
        .test_value()?;
    let Declaration::TypeAlias(alias) = alias else {
        return Err(TestFailure);
    };
    let target = ast_errors.access(|out| alias.target(out)).test_value()?;
    let kind = ast_errors.access(|out| target.kind(out)).test_value()?;
    let TypeExprKind::Array {
        length: Some(length),
        ..
    } = kind
    else {
        return Err(TestFailure);
    };
    test_assert_eq!(
        ast_errors.access(|out| length.raw(out)).test_value()?,
        "123"
    );
    let declarations = ast_errors
        .access(|out| outer.declarations(out))
        .test_value()?;
    let alias = ast_errors
        .access(|out| declarations.last(out))
        .test_value()?
        .test_value()?;
    let Declaration::TypeAlias(alias) = alias else {
        return Err(TestFailure);
    };
    let target = ast_errors.access(|out| alias.target(out)).test_value()?;
    let kind = ast_errors.access(|out| target.kind(out)).test_value()?;
    let TypeExprKind::Path(path) = kind else {
        return Err(TestFailure);
    };
    let segments = ast_errors.access(|out| path.segments(out)).test_value()?;
    let first_segment = ast_errors
        .access(|out| segments.first(out))
        .test_value()?
        .test_value()?;
    let last_segment = ast_errors
        .access(|out| segments.last(out))
        .test_value()?
        .test_value()?;
    test_assert_eq!(
        ast_errors
            .access(|out| first_segment.text(out))
            .test_value()?,
        "Inner"
    );
    test_assert_eq!(
        ast_errors
            .access(|out| last_segment.text(out))
            .test_value()?,
        "T"
    );
    let entry = ast_errors
        .access(|out| ast.items().get(1, out))
        .test_value()?
        .test_value()?;
    let root = entry.entry_struct().test_value()?;
    let fields = ast_errors.access(|out| root.fields(out)).test_value()?;
    let text = ast_errors
        .access(|out| fields.first(out))
        .test_value()?
        .test_value()?;
    let annotations = ast_errors
        .access(|out| text.annotations(out))
        .test_value()?;
    let annotation = ast_errors
        .access(|out| annotations.first(out))
        .test_value()?
        .test_value()?;
    let AnnotationData::Regex(regex) =
        ast_errors.access(|out| annotation.data(out)).test_value()?
    else {
        return Err(TestFailure);
    };
    test_assert_eq!(
        ast_errors.access(|out| regex.value(out)).test_value()?,
        "€\n₽"
    );
    let default = ast_errors
        .access(|out| text.default(out))
        .test_value()?
        .test_value()?;
    let kind = ast_errors.access(|out| default.kind(out)).test_value()?;
    let DefaultValueKind::Scalar(ScalarLiteral::String(value)) = kind else {
        return Err(TestFailure);
    };
    test_assert_eq!(
        ast_errors.access(|out| value.value(out)).test_value()?,
        "€source"
    );
    let number_field = ast_errors
        .access(|out| fields.get(1, out))
        .test_value()?
        .test_value()?;
    let default = ast_errors
        .access(|out| number_field.default(out))
        .test_value()?
        .test_value()?;
    let kind = ast_errors.access(|out| default.kind(out)).test_value()?;
    let DefaultValueKind::Scalar(ScalarLiteral::Number(number)) = kind else {
        return Err(TestFailure);
    };
    test_assert_eq!(
        ast_errors.access(|out| number.raw(out)).test_value()?,
        "0x2a"
    );
    test_assert!(number.is_negative());
    let records_field = ast_errors
        .access(|out| fields.last(out))
        .test_value()?
        .test_value()?;
    let default = ast_errors
        .access(|out| records_field.default(out))
        .test_value()?
        .test_value()?;
    let kind = ast_errors.access(|out| default.kind(out)).test_value()?;
    let DefaultValueKind::Array(records) = kind else {
        return Err(TestFailure);
    };
    let elements = ast_errors
        .access(|out| records.elements(out))
        .test_value()?;
    for index in 0..elements.len() {
        let element = ast_errors
            .access(|out| elements.get(index, out))
            .test_value()?
            .test_value()?;
        let DefaultValueKind::Braced(record) =
            ast_errors.access(|out| element.kind(out)).test_value()?
        else {
            return Err(TestFailure);
        };
        let entries = ast_errors.access(|out| record.entries(out)).test_value()?;
        let entry = ast_errors
            .access(|out| entries.first(out))
            .test_value()?
            .test_value()?;
        let key = ast_errors.access(|out| entry.key(out)).test_value()?;
        match (index, key) {
            (0, BraceKey::Identifier(key)) => {
                test_assert_eq!(ast_errors.access(|out| key.text(out)).test_value()?, "key")
            }
            (1, BraceKey::String(key)) => {
                test_assert_eq!(
                    ast_errors.access(|out| key.value(out)).test_value()?,
                    "€\n₽"
                )
            }
            _ => return Err(TestFailure),
        }
        let value = ast_errors.access(|out| entry.value(out)).test_value()?;
        let DefaultValueKind::Array(values) =
            ast_errors.access(|out| value.kind(out)).test_value()?
        else {
            return Err(TestFailure);
        };
        let values = ast_errors.access(|out| values.elements(out)).test_value()?;
        let mut numbers = Vec::new();
        for value_index in 0..values.len() {
            let value = ast_errors
                .access(|out| values.get(value_index, out))
                .test_value()?
                .test_value()?;
            let DefaultValueKind::Scalar(ScalarLiteral::Number(number)) =
                ast_errors.access(|out| value.kind(out)).test_value()?
            else {
                return Err(TestFailure);
            };
            numbers.push(ast_errors.access(|out| number.raw(out)).test_value()?);
        }
        test_assert_eq!(numbers, if index == 0 { ["1", "2"] } else { ["3", "4"] });
    }
    let assertions = ast_errors.access(|out| root.assertions(out)).test_value()?;
    let assertion = ast_errors
        .access(|out| assertions.first(out))
        .test_value()?
        .test_value()?;
    let message = ast_errors
        .access(|out| assertion.message(out))
        .test_value()?;
    test_assert_eq!(
        ast_errors.access(|out| message.value(out)).test_value()?,
        "assertion\nmessage"
    );
    let expression = ast_errors
        .access(|out| assertion.expression(out))
        .test_value()?;
    let ExprKind::MethodCall {
        argument: Some(argument),
        ..
    } = ast_errors.access(|out| expression.kind(out)).test_value()?
    else {
        return Err(TestFailure);
    };
    let MethodArgumentKind::Closure { parameter, body } =
        ast_errors.access(|out| argument.kind(out)).test_value()?
    else {
        return Err(TestFailure);
    };
    test_assert_eq!(
        ast_errors.access(|out| parameter.text(out)).test_value()?,
        "item"
    );
    let ExprKind::Binary { right, .. } = ast_errors.access(|out| body.kind(out)).test_value()?
    else {
        return Err(TestFailure);
    };
    let ExprKind::Literal(ScalarLiteral::String(text)) =
        ast_errors.access(|out| right.kind(out)).test_value()?
    else {
        return Err(TestFailure);
    };
    test_assert_eq!(
        ast_errors.access(|out| text.value(out)).test_value()?,
        "€€\n₽"
    );
    let last_item = ast_errors
        .access(|out| ast.items().last(out))
        .test_value()?
        .test_value()?;
    let Declaration::TypeAlias(last) = last_item.declaration().test_value()? else {
        return Err(TestFailure);
    };
    test_assert_eq!(
        ast_errors.access(|out| last.name(out)).test_value()?,
        "T1023"
    );

    Ok(())
}

#[test]
fn views_resolve_with_their_own_tree_and_lists_keep_double_ended_order() -> TestResult {
    let execution = new_execution()?;
    let mut syntax_errors = TestSlot::<SyntaxErrors>::default();
    let mut ast_errors = TestSlot::<AstAccessError>::default();

    let limits = SyntaxLimits::default();
    let first = parse_with_slot(
        &mut syntax_errors,
        "entry struct First { a: u8, b: u8, c: u8 }",
        &limits,
        &execution,
    )
    .test_value()?;
    let second = parse_with_slot(
        &mut syntax_errors,
        "entry struct Other { x: bool }",
        &limits,
        &execution,
    )
    .test_value()?;
    let first_item = ast_errors
        .access(|out| first.items().first(out))
        .test_value()?
        .test_value()?;
    let first_root = first_item.entry_struct().test_value()?;
    let fields = ast_errors
        .access(|out| first_root.fields(out))
        .test_value()?;
    let second_item = ast_errors
        .access(|out| second.items().first(out))
        .test_value()?
        .test_value()?;
    let second_root = second_item.entry_struct().test_value()?;
    let other = ast_errors
        .access(|out| second_root.fields(out))
        .test_value()?;
    let other_field = ast_errors
        .access(|out| other.first(out))
        .test_value()?
        .test_value()?;
    test_assert_eq!(
        ast_errors
            .access(|out| other_field.name(out))
            .test_value()?,
        "x",
    );
    let first_field = ast_errors
        .access(|out| fields.first(out))
        .test_value()?
        .test_value()?;
    test_assert_eq!(
        ast_errors
            .access(|out| first_field.name(out))
            .test_value()?,
        "a"
    );
    test_assert!(
        ast_errors
            .access(|out| fields.get(usize::MAX, out))
            .test_value()?
            .is_none()
    );
    let stats = ast_errors
        .access(|mut out| {
            let mut iter = fields.iter(out.reborrow());
            let mut lengths = vec![iter.size_hint().0];
            let first = iter.next();
            lengths.push(iter.size_hint().0);
            let hints = vec![iter.size_hint()];
            let last = iter.next_back();
            let middle = iter.next_back();
            let final_len = iter.size_hint().0;
            let exhausted =
                iter.next().is_none() && iter.next_back().is_none() && iter.next().is_none();
            lengths.push(final_len);
            let (names, complete) = if let (Some(first), Some(last)) = (first, last) {
                if let Some(middle) = middle {
                    let first = first?;
                    let last = last?;
                    let middle = middle?;
                    (
                        vec![
                            first.name(out.reborrow())?,
                            last.name(out.reborrow())?,
                            middle.name(out.reborrow())?,
                        ],
                        true,
                    )
                } else {
                    (Vec::new(), false)
                }
            } else {
                (Vec::new(), false)
            };
            Ok(IteratorStats {
                names,
                lengths,
                hints,
                exhausted,
                complete,
            })
        })
        .test_value()?;
    test_assert_eq!(stats.names, ["a", "c", "b"]);
    test_assert_eq!(stats.lengths, [3, 2, 0]);
    test_assert_eq!(stats.hints, [(2, Some(2))]);
    test_assert!(stats.exhausted);
    test_assert!(stats.complete);
    let empty = parse_with_slot(&mut syntax_errors, "", &limits, &execution).test_value()?;
    test_assert!(empty.items().is_empty());
    test_assert!(
        ast_errors
            .access(|out| empty.items().first(out))
            .test_value()?
            .is_none()
    );
    test_assert!(
        ast_errors
            .access(|out| empty.items().last(out))
            .test_value()?
            .is_none()
    );
    let empty_iter_len = ast_errors
        .access(|out| Ok(empty.items().iter(out).size_hint().0))
        .test_value()?;
    test_assert_eq!(empty_iter_len, 0);

    Ok(())
}
