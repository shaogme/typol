use typol_core::{BudgetError, DiagnosticCode, ExecutionBudget, ExecutionLimits, SourceId};
use typol_syntax::{
    AnnotationData, AstAccessError, Declaration, SchemaAst, SchemaParseOptions, SchemaParser,
    SizeArguments, SyntaxErrors, SyntaxLimits, VariantRestrictionMode,
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

struct BoundDetails<Bound, Name, Raw> {
    bound: Bound,
    name: Name,
    raw: Raw,
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

macro_rules! annotation_data {
    ($slot:ident, $alias:expr, $index:expr) => {{
        let target = $slot.access(|out| $alias.target(out)).test_value()?;
        let annotations = $slot.access(|out| target.annotations(out)).test_value()?;
        let annotation = $slot
            .access(|out| annotations.get($index, out))
            .test_value()?
            .test_value()?;
        $slot.access(|out| annotation.data(out)).test_value()?
    }};
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
fn typed_payloads_own_text_and_preserve_bound_order_and_positions() -> TestResult {
    let execution = new_execution()?;
    let mut syntax_errors = TestSlot::<SyntaxErrors>::default();
    let mut ast_errors = TestSlot::<AstAccessError>::default();

    let schema = {
        let source = String::from(
            r#"type A = i128 @min(-170141183460469231731687303715884105728) @max(0x2a) @range(-1, 2);
            type B = string @length(0, 2) @regex("a\n€") @cidr(v4_only = false) @env("TOKEN");
            type C = [u8] @size(max = 8, min = 2) @unique(by = "value.id") @all(item > 0, "rule\n") @sum(max = -0x2a);
            type D = [u8] @size(0, 8) @unique;"#,
        );
        let schema = parse_with_slot(
            &mut syntax_errors,
            &source,
            &SyntaxLimits::default(),
            &execution,
        )
        .test_value()?;
        let item = ast_errors
            .access(|out| schema.items().get(2, out))
            .test_value()?
            .test_value()?;
        let Some(Declaration::TypeAlias(alias)) = item.declaration() else {
            return Err(TestFailure);
        };
        let target = ast_errors.access(|out| alias.target(out)).test_value()?;
        let annotations = ast_errors
            .access(|out| target.annotations(out))
            .test_value()?;
        let annotation = ast_errors
            .access(|out| annotations.first(out))
            .test_value()?
            .test_value()?;
        let AnnotationData::Size(SizeArguments::Named(bounds)) =
            ast_errors.access(|out| annotation.data(out)).test_value()?
        else {
            return Err(TestFailure);
        };
        let bound_details = ast_errors
            .access(|mut out| {
                let values = bounds.iter(out.reborrow()).collect::<Vec<_>>();
                let mut details = Vec::new();
                for value in values {
                    let bound = value?;
                    let name = bound.name().text(out.reborrow())?;
                    let raw = bound.value().raw(out.reborrow())?;
                    details.push(BoundDetails { bound, name, raw });
                }
                Ok(details)
            })
            .test_value()?;
        for details in bound_details {
            let name_span = details.bound.name().span().span();
            test_assert_eq!(&source[name_span.start()..name_span.end()], details.name);
            let number_span = details.bound.value().span().span();
            test_assert_eq!(&source[number_span.start()..number_span.end()], details.raw);
            let parameter_span = details.bound.span().span();
            test_assert_eq!(parameter_span.start(), name_span.start());
            test_assert_eq!(parameter_span.end(), number_span.end());
            test_assert_eq!(details.bound.span().source_id(), SOURCE_ID);
        }
        schema
    };
    let aliases = ast_errors
        .access(|mut out| {
            let items = schema.items().iter(out.reborrow()).collect::<Vec<_>>();
            let mut aliases = Vec::new();
            for item in items {
                let item = item?;
                if let Some(Declaration::TypeAlias(alias)) = item.declaration() {
                    aliases.push(alias);
                }
            }
            Ok(aliases)
        })
        .test_value()?;
    let alias_a = aliases.first().test_value()?;
    let alias_b = aliases.get(1).test_value()?;
    let alias_c = aliases.get(2).test_value()?;
    let alias_d = aliases.get(3).test_value()?;
    let AnnotationData::Min(number) = annotation_data!(ast_errors, alias_a, 0) else {
        return Err(TestFailure);
    };
    test_assert!(number.is_negative());
    test_assert_eq!(
        ast_errors.access(|out| number.raw(out)).test_value()?,
        "170141183460469231731687303715884105728"
    );
    let AnnotationData::Max(number) = annotation_data!(ast_errors, alias_a, 1) else {
        return Err(TestFailure);
    };
    test_assert_eq!(
        ast_errors.access(|out| number.raw(out)).test_value()?,
        "0x2a"
    );
    let AnnotationData::Regex(pattern) = annotation_data!(ast_errors, alias_b, 1) else {
        return Err(TestFailure);
    };
    test_assert_eq!(
        ast_errors.access(|out| pattern.value(out)).test_value()?,
        "a\n€"
    );
    let AnnotationData::Cidr(argument) = annotation_data!(ast_errors, alias_b, 2) else {
        return Err(TestFailure);
    };
    test_assert_eq!(
        ast_errors
            .access(|out| argument.name().text(out))
            .test_value()?,
        "v4_only"
    );
    test_assert!(!argument.value().value());
    let AnnotationData::Env(name) = annotation_data!(ast_errors, alias_b, 3) else {
        return Err(TestFailure);
    };
    test_assert_eq!(
        ast_errors.access(|out| name.value(out)).test_value()?,
        "TOKEN"
    );
    let AnnotationData::Size(SizeArguments::Named(bounds)) =
        annotation_data!(ast_errors, alias_c, 0)
    else {
        return Err(TestFailure);
    };
    let bound_names = ast_errors
        .access(|mut out| {
            let bounds = bounds.iter(out.reborrow()).collect::<Vec<_>>();
            let mut names = Vec::new();
            for bound in bounds {
                let bound = bound?;
                names.push(bound.name().text(out.reborrow())?);
            }
            Ok(names)
        })
        .test_value()?;
    test_assert_eq!(bound_names, ["max", "min"]);
    let AnnotationData::Unique(Some(by)) = annotation_data!(ast_errors, alias_c, 1) else {
        return Err(TestFailure);
    };
    test_assert_eq!(
        ast_errors
            .access(|out| by.value().value(out))
            .test_value()?,
        "value.id"
    );
    let AnnotationData::All(assertion) = annotation_data!(ast_errors, alias_c, 2) else {
        return Err(TestFailure);
    };
    let message = ast_errors
        .access(|out| assertion.message(out))
        .test_value()?;
    test_assert_eq!(
        ast_errors.access(|out| message.value(out)).test_value()?,
        "rule\n"
    );
    let AnnotationData::Sum(bounds) = annotation_data!(ast_errors, alias_c, 3) else {
        return Err(TestFailure);
    };
    let number = ast_errors
        .access(|mut out| {
            let mut iter = bounds.iter(out.reborrow());
            let Some(argument) = iter.next() else {
                return Ok(None);
            };
            let argument = argument?;
            Ok(Some(argument.value()))
        })
        .test_value()?
        .test_value()?;
    test_assert!(number.is_negative());
    test_assert_eq!(
        ast_errors.access(|out| number.raw(out)).test_value()?,
        "0x2a"
    );
    test_assert!(matches!(
        annotation_data!(ast_errors, alias_d, 0),
        AnnotationData::Size(SizeArguments::Positional { .. })
    ));
    test_assert!(matches!(
        annotation_data!(ast_errors, alias_d, 1),
        AnnotationData::Unique(None)
    ));

    Ok(())
}

#[test]
fn all_annotation_grammar_rejections_remain_syntax_errors() -> TestResult {
    let execution = new_execution()?;
    let mut syntax_errors = TestSlot::<SyntaxErrors>::default();

    for annotation in [
        "@unknown",
        "@min()",
        "@min(1, 2)",
        "@min(\"1\")",
        "@max(min = 1)",
        "@range(1)",
        "@range(1, 2, 3)",
        "@length(-1, 2)",
        "@length(0x1, 2)",
        "@length(1.5, 2)",
        "@regex()",
        "@regex(pattern = \"a\")",
        "@regex(1)",
        "@host()",
        "@email(\"a\")",
        "@secret(true)",
        "@env(1)",
        "@cidr()",
        "@cidr(v4_only = 1)",
        "@cidr(v6_only = true)",
        "@size()",
        "@size(1)",
        "@size(-1, 2)",
        "@size(1, max = 2)",
        "@size(min = 1, 2)",
        "@size(min = 0x1)",
        "@size(min = 1, max = 2, min = 0)",
        "@unique()",
        "@unique(by = 1)",
        "@unique(key = \"id\")",
        "@all(true)",
        "@all(true, 1)",
        "@all(true, \"rule\", false)",
        "@sum()",
        "@sum(0, 1)",
        "@sum(value = 1)",
        "@sum(min = 0, max = 1, min = 2)",
        "@variants()",
        "@variants(block = [\"A\"])",
        "@variants(allow = [])",
        "@variants(deny = [])",
        "@variants(allow = [1])",
        "@variants(allow = [\"A\";])",
        "@variants(allow = [\"A\"]",
    ] {
        let source = format!("type A = u8 {annotation};");
        let errors = parse_with_slot(
            &mut syntax_errors,
            &source,
            &SyntaxLimits::default(),
            &execution,
        )
        .test_error()?;
        test_assert_eq!(
            errors.diagnostics().first().test_value()?.code(),
            DiagnosticCode::SCHEMA_SYNTAX_ERROR,
            "{annotation}"
        );
    }

    Ok(())
}

#[test]
fn variant_restrictions_keep_the_mode_names_and_literal_spans() -> TestResult {
    let execution = new_execution()?;
    let mut syntax_errors = TestSlot::<SyntaxErrors>::default();
    let mut ast_errors = TestSlot::<AstAccessError>::default();
    let source = r#"type Choice = string @variants(allow = ["Alpha", "Beta",]);
        type Excluded = string @variants(deny = ["Legacy"]);"#;
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
    let Some(Declaration::TypeAlias(alias)) = item.declaration() else {
        return Err(TestFailure);
    };
    let AnnotationData::Variants { mode, names } = annotation_data!(ast_errors, alias, 0) else {
        return Err(TestFailure);
    };
    test_assert_eq!(mode, VariantRestrictionMode::Allow);
    let literals = ast_errors
        .access(|mut out| {
            let values = names.iter(out.reborrow()).collect::<Vec<_>>();
            let mut literals = Vec::new();
            for value in values {
                literals.push(value?);
            }
            Ok(literals)
        })
        .test_value()?;
    test_assert_eq!(literals.len(), 2);
    for (literal, expected) in literals.into_iter().zip(["Alpha", "Beta"]) {
        test_assert_eq!(
            ast_errors.access(|out| literal.value(out)).test_value()?,
            expected
        );
        let span = literal.span().span();
        let raw = format!("\"{expected}\"");
        test_assert_eq!(&source[span.start()..span.end()], raw.as_str());
    }

    let excluded_item = ast_errors
        .access(|out| schema.items().get(1, out))
        .test_value()?
        .test_value()?;
    let Some(Declaration::TypeAlias(excluded)) = excluded_item.declaration() else {
        return Err(TestFailure);
    };
    let AnnotationData::Variants { mode, names } = annotation_data!(ast_errors, excluded, 0) else {
        return Err(TestFailure);
    };
    test_assert_eq!(mode, VariantRestrictionMode::Deny);
    test_assert_eq!(names.len(), 1);

    Ok(())
}

#[test]
fn repeated_bounds_are_preserved_for_semantic_validation() -> TestResult {
    let execution = new_execution()?;
    let mut syntax_errors = TestSlot::<SyntaxErrors>::default();
    let mut ast_errors = TestSlot::<AstAccessError>::default();

    for annotation in ["@size(min = 1, min = 2)", "@sum(max = 1, max = 2)"] {
        let source = format!("type A = [u8] {annotation};");
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
        let Some(Declaration::TypeAlias(alias)) = item.declaration() else {
            return Err(TestFailure);
        };
        let data = annotation_data!(ast_errors, alias, 0);
        let bounds = match data {
            AnnotationData::Size(SizeArguments::Named(bounds)) | AnnotationData::Sum(bounds) => {
                bounds
            }
            _ => return Err(TestFailure),
        };
        let arguments = ast_errors
            .access(|mut out| {
                let values = bounds.iter(out.reborrow()).collect::<Vec<_>>();
                let mut arguments = Vec::new();
                for value in values {
                    arguments.push(value?);
                }
                Ok(arguments)
            })
            .test_value()?;
        test_assert_eq!(arguments.len(), 2);
        let first = arguments.first().test_value()?;
        let second = arguments.get(1).test_value()?;
        test_assert_eq!(
            ast_errors
                .access(|out| first.name().text(out))
                .test_value()?,
            ast_errors
                .access(|out| second.name().text(out))
                .test_value()?
        );
        test_assert!(
            arguments.first().test_value()?.span().span().end()
                < arguments.get(1).test_value()?.span().span().start()
        );
    }

    Ok(())
}

#[test]
fn payload_debug_hides_boolean_text_numbers_expressions_and_names() -> TestResult {
    let execution = new_execution()?;
    let mut syntax_errors = TestSlot::<SyntaxErrors>::default();
    let mut ast_errors = TestSlot::<AstAccessError>::default();

    let source = r#"type A = string @regex("hidden-pattern") @env("HIDDEN_ENV") @cidr(v4_only = false);
    type B = [u8] @size(min = 98765) @unique(by = "hidden.path") @all(hidden_field == true, "hidden-message");"#;
    let schema = parse_with_slot(
        &mut syntax_errors,
        source,
        &SyntaxLimits::default(),
        &execution,
    )
    .test_value()?;
    let debug_values = ast_errors
        .access(|mut out| {
            let items = schema.items().iter(out.reborrow()).collect::<Vec<_>>();
            let mut values = Vec::new();
            for item in items {
                let item = item?;
                let Some(Declaration::TypeAlias(alias)) = item.declaration() else {
                    continue;
                };
                let target = alias.target(out.reborrow())?;
                let annotations = target.annotations(out.reborrow())?;
                let annotations = annotations.iter(out.reborrow()).collect::<Vec<_>>();
                for annotation in annotations {
                    let annotation = annotation?;
                    let data = annotation.data(out.reborrow())?;
                    values.push(format!("{:?} {:?}", annotation, data));
                }
            }
            Ok(values)
        })
        .test_value()?;
    for debug in debug_values {
        for text in ["false", "true", "v4_only", "98765", "hidden", "HIDDEN_ENV"] {
            test_assert!(
                !debug.contains(text),
                "automatic Debug output must not reveal raw arguments: {text}"
            );
        }
    }

    Ok(())
}
