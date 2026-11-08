use typol_core::{BudgetError, DiagnosticCode, ExecutionBudget, ExecutionLimits, SourceId};
use typol_syntax::{
    AnnotationData, AnnotationKind, AstAccessError, BraceKey, BuiltinType, Declaration,
    DefaultValueKind, EnumDefaultPayloadKind, ExprKind, ScalarLiteral, SchemaParseOptions,
    SchemaParser, SizeArguments, StreamLimits, SyntaxErrors, SyntaxLimits, TypeExprKind,
    VariantPayloadKind,
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

#[test]
fn declarations_keep_their_source_order_and_structured_payload_shapes() -> TestResult {
    let mut errors = TestSlot::<AstAccessError>::default();
    let mut budget_errors = TestSlot::<BudgetError>::default();
    let mut syntax_errors = TestSlot::<SyntaxErrors>::default();
    let execution = budget_errors
        .access(|out| ExecutionBudget::new(ExecutionLimits::new(67_108_864, 67_108_864), out))
        .test_value()?;

    let source = r#"
        extern mod shared;
        mod network { struct Endpoint { host: string, } }
        type Port = u16 @range(1, 65535);
        struct Local { value: bool, }
        enum Choice { Empty, Text(option<string> @secret), Record { id: u32, }, }
        entry struct Root { endpoint: network::Endpoint, }
    "#;
    let schema = syntax_errors
        .access(|out| {
            SchemaParser::parse_str(
                SOURCE_ID,
                source,
                &SchemaParseOptions::default(),
                &execution,
                out,
            )
        })
        .test_value()?;

    test_assert_eq!(schema.items().len(), 6);
    let first_item = errors
        .access(|out| schema.items().first(out))
        .test_value()?
        .test_value()?;
    let shared_module = first_item.extern_mod().test_value()?;
    test_assert_eq!(
        errors.access(|out| shared_module.name(out)).test_value()?,
        "shared"
    );
    let module_item = errors
        .access(|out| schema.items().get(1, out))
        .test_value()?
        .test_value()?;
    let module = match module_item.declaration().test_value()? {
        Declaration::Module(module) => module,
        _ => return Err(TestFailure),
    };
    test_assert_eq!(
        errors.access(|out| module.name(out)).test_value()?,
        "network"
    );
    let module_declarations = errors.access(|out| module.declarations(out)).test_value()?;
    test_assert_eq!(module_declarations.len(), 1);
    let alias_item = errors
        .access(|out| schema.items().get(2, out))
        .test_value()?
        .test_value()?;
    let alias = match alias_item.declaration().test_value()? {
        Declaration::TypeAlias(alias) => alias,
        _ => return Err(TestFailure),
    };
    test_assert_eq!(errors.access(|out| alias.name(out)).test_value()?, "Port");
    let struct_item = errors
        .access(|out| schema.items().get(3, out))
        .test_value()?
        .test_value()?;
    let structure = match struct_item.declaration().test_value()? {
        Declaration::Struct(structure) => structure,
        _ => return Err(TestFailure),
    };
    test_assert_eq!(
        errors.access(|out| structure.name(out)).test_value()?,
        "Local"
    );
    let enum_item = errors
        .access(|out| schema.items().get(4, out))
        .test_value()?
        .test_value()?;
    let enumeration = match enum_item.declaration().test_value()? {
        Declaration::Enum(enumeration) => enumeration,
        _ => return Err(TestFailure),
    };
    let variants = errors
        .access(|out| enumeration.variants(out))
        .test_value()?;
    test_assert_eq!(variants.len(), 3);
    let unit = errors
        .access(|out| variants.first(out))
        .test_value()?
        .test_value()?;
    let unit_payload = errors.access(|out| unit.payload(out)).test_value()?;
    test_assert!(matches!(
        errors.access(|out| unit_payload.kind(out)).test_value()?,
        VariantPayloadKind::Unit
    ));
    let single = errors
        .access(|out| variants.get(1, out))
        .test_value()?
        .test_value()?;
    let single_payload = errors.access(|out| single.payload(out)).test_value()?;
    test_assert!(matches!(
        errors.access(|out| single_payload.kind(out)).test_value()?,
        VariantPayloadKind::Single(_)
    ));
    let structure_variant = errors
        .access(|out| variants.get(2, out))
        .test_value()?
        .test_value()?;
    let structure_payload = errors
        .access(|out| structure_variant.payload(out))
        .test_value()?;
    test_assert!(matches!(
        errors
            .access(|out| structure_payload.kind(out))
            .test_value()?,
        VariantPayloadKind::Structure(_)
    ));
    let entry_item = errors
        .access(|out| schema.items().get(5, out))
        .test_value()?
        .test_value()?;
    let entry = entry_item.entry_struct().test_value()?;
    test_assert_eq!(errors.access(|out| entry.name(out)).test_value()?, "Root");

    Ok(())
}

#[test]
fn schema_string_defaults_store_the_dedented_block_value_and_full_span() -> TestResult {
    let mut errors = TestSlot::<AstAccessError>::default();
    let mut budget_errors = TestSlot::<BudgetError>::default();
    let mut syntax_errors = TestSlot::<SyntaxErrors>::default();
    let execution = budget_errors
        .access(|out| ExecutionBudget::new(ExecutionLimits::new(67_108_864, 67_108_864), out))
        .test_value()?;
    let source = "entry struct Root { text: string = \"\"\"\n    first\n    second\n    \"\"\", }";
    let schema = syntax_errors
        .access(|out| {
            SchemaParser::parse_str(
                SOURCE_ID,
                source,
                &SchemaParseOptions::default(),
                &execution,
                out,
            )
        })
        .test_value()?;
    let item = errors
        .access(|out| schema.items().first(out))
        .test_value()?
        .test_value()?;
    let entry = item.entry_struct().test_value()?;
    let fields = errors.access(|out| entry.fields(out)).test_value()?;
    let field = errors
        .access(|out| fields.first(out))
        .test_value()?
        .test_value()?;
    let default = errors
        .access(|out| field.default(out))
        .test_value()?
        .test_value()?;
    let DefaultValueKind::Scalar(ScalarLiteral::String(value)) =
        errors.access(|out| default.kind(out)).test_value()?
    else {
        return Err(TestFailure);
    };
    test_assert_eq!(
        errors.access(|out| value.value(out)).test_value()?,
        "first\nsecond"
    );
    let span = value.span().span();
    let token_start = source.find("\"\"\"").test_value()?;
    let token_end = source.rfind("\"\"\"").test_value()? + 3;
    test_assert_eq!((span.start(), span.end()), (token_start, token_end));
    Ok(())
}

#[test]
fn type_expressions_retain_builtin_container_array_and_path_forms() -> TestResult {
    let mut errors = TestSlot::<AstAccessError>::default();
    let mut budget_errors = TestSlot::<BudgetError>::default();
    let mut syntax_errors = TestSlot::<SyntaxErrors>::default();
    let execution = budget_errors
        .access(|out| ExecutionBudget::new(ExecutionLimits::new(67_108_864, 67_108_864), out))
        .test_value()?;

    let source = r#"
        entry struct Root {
            i8_value: i8,
            i16_value: i16,
            i32_value: i32,
            i64_value: i64,
            i128_value: i128,
            u8_value: u8,
            u16_value: u16,
            u32_value: u32,
            u64_value: u64,
            u128_value: u128,
            f32_value: f32,
            f64_value: f64,
            string_value: string,
            bool_value: bool,
            optional: option<string>,
            list: [u8],
            fixed: [string; 4],
            dictionary: map<string, option<bool>>,
            reference: network::Endpoint,
        }
    "#;
    let schema = syntax_errors
        .access(|out| {
            SchemaParser::parse_str(
                SOURCE_ID,
                source,
                &SchemaParseOptions::default(),
                &execution,
                out,
            )
        })
        .test_value()?;
    let entry_item = errors
        .access(|out| schema.items().first(out))
        .test_value()?
        .test_value()?;
    let entry = entry_item.entry_struct().test_value()?;
    let fields = errors.access(|out| entry.fields(out)).test_value()?;
    let builtins = [
        BuiltinType::I8,
        BuiltinType::I16,
        BuiltinType::I32,
        BuiltinType::I64,
        BuiltinType::I128,
        BuiltinType::U8,
        BuiltinType::U16,
        BuiltinType::U32,
        BuiltinType::U64,
        BuiltinType::U128,
        BuiltinType::F32,
        BuiltinType::F64,
        BuiltinType::String,
        BuiltinType::Bool,
    ];

    for (index, expected) in builtins.into_iter().enumerate() {
        let field = errors
            .access(|out| fields.get(index, out))
            .test_value()?
            .test_value()?;
        let field_type = errors.access(|out| field.ty(out)).test_value()?;
        let kind = errors.access(|out| field_type.kind(out)).test_value()?;
        test_assert!(matches!(
            kind,
            TypeExprKind::Builtin(actual) if actual == expected
        ));
    }
    let optional_field = errors
        .access(|out| fields.get(14, out))
        .test_value()?
        .test_value()?;
    let optional_type = errors.access(|out| optional_field.ty(out)).test_value()?;
    test_assert!(matches!(
        errors.access(|out| optional_type.kind(out)).test_value()?,
        TypeExprKind::Option(_)
    ));
    let list_field = errors
        .access(|out| fields.get(15, out))
        .test_value()?
        .test_value()?;
    let list_type = errors.access(|out| list_field.ty(out)).test_value()?;
    test_assert!(matches!(
        errors.access(|out| list_type.kind(out)).test_value()?,
        TypeExprKind::Array { length: None, .. }
    ));
    let fixed_field = errors
        .access(|out| fields.get(16, out))
        .test_value()?
        .test_value()?;
    let fixed_type = errors.access(|out| fixed_field.ty(out)).test_value()?;
    match errors.access(|out| fixed_type.kind(out)).test_value()? {
        TypeExprKind::Array {
            length: Some(length),
            ..
        } => test_assert_eq!(errors.access(|out| length.raw(out)).test_value()?, "4"),
        _ => return Err(TestFailure),
    }
    let map_field = errors
        .access(|out| fields.get(17, out))
        .test_value()?
        .test_value()?;
    let map_type = errors.access(|out| map_field.ty(out)).test_value()?;
    test_assert!(matches!(
        errors.access(|out| map_type.kind(out)).test_value()?,
        TypeExprKind::Map { .. }
    ));
    let path_field = errors
        .access(|out| fields.get(18, out))
        .test_value()?
        .test_value()?;
    let path_type = errors.access(|out| path_field.ty(out)).test_value()?;
    match errors.access(|out| path_type.kind(out)).test_value()? {
        TypeExprKind::Path(path) => {
            let segments = errors.access(|out| path.segments(out)).test_value()?;
            let first = errors
                .access(|out| segments.get(0, out))
                .test_value()?
                .test_value()?;
            let second = errors
                .access(|out| segments.get(1, out))
                .test_value()?
                .test_value()?;
            test_assert_eq!(
                errors.access(|out| first.text(out)).test_value()?,
                "network"
            );
            test_assert_eq!(
                errors.access(|out| second.text(out)).test_value()?,
                "Endpoint"
            );
        }
        _ => return Err(TestFailure),
    }

    Ok(())
}

#[test]
fn defaults_retain_scalar_array_brace_key_and_enum_forms() -> TestResult {
    let mut errors = TestSlot::<AstAccessError>::default();
    let mut budget_errors = TestSlot::<BudgetError>::default();
    let mut syntax_errors = TestSlot::<SyntaxErrors>::default();
    let execution = budget_errors
        .access(|out| ExecutionBudget::new(ExecutionLimits::new(67_108_864, 67_108_864), out))
        .test_value()?;

    let source = r#"
        entry struct Root {
            negative: i16 = -0x2a,
            enabled: bool = true,
            optional: option<string> = none,
            values: [i32] = [1, 2,],
            dictionary: map<string, string> = {"tier": "prod",},
            unit: Choice = .Empty,
            single: Choice = .Text("hello"),
            record: Choice = .Record { id: 7, },
        }
    "#;
    let schema = syntax_errors
        .access(|out| {
            SchemaParser::parse_str(
                SOURCE_ID,
                source,
                &SchemaParseOptions::default(),
                &execution,
                out,
            )
        })
        .test_value()?;
    let entry_item = errors
        .access(|out| schema.items().first(out))
        .test_value()?
        .test_value()?;
    let entry = entry_item.entry_struct().test_value()?;
    let fields = errors.access(|out| entry.fields(out)).test_value()?;
    macro_rules! default_kind {
        ($index:expr) => {{
            let field = errors
                .access(|out| fields.get($index, out))
                .test_value()?
                .test_value()?;
            let default = errors
                .access(|out| field.default(out))
                .test_value()?
                .test_value()?;
            errors.access(|out| default.kind(out)).test_value()?
        }};
    }

    match default_kind!(0) {
        DefaultValueKind::Scalar(ScalarLiteral::Number(number)) => {
            test_assert_eq!(errors.access(|out| number.raw(out)).test_value()?, "0x2a");
            test_assert!(number.is_negative());
        }
        _ => return Err(TestFailure),
    }
    test_assert!(matches!(default_kind!(1), DefaultValueKind::Scalar(_)));
    test_assert!(matches!(default_kind!(2), DefaultValueKind::None));
    match default_kind!(3) {
        DefaultValueKind::Array(array) => {
            let elements = errors.access(|out| array.elements(out)).test_value()?;
            test_assert_eq!(elements.len(), 2);
        }
        _ => return Err(TestFailure),
    }
    match default_kind!(4) {
        DefaultValueKind::Braced(braced) => {
            let entries = errors.access(|out| braced.entries(out)).test_value()?;
            let entry = errors
                .access(|out| entries.first(out))
                .test_value()?
                .test_value()?;
            test_assert!(matches!(
                errors.access(|out| entry.key(out)).test_value()?,
                BraceKey::String(_)
            ));
        }
        _ => return Err(TestFailure),
    }
    for (index, expected_variant) in ["Empty", "Text", "Record"].into_iter().enumerate() {
        let DefaultValueKind::Enum(value) = default_kind!(index + 5) else {
            return Err(TestFailure);
        };
        test_assert_eq!(
            errors.access(|out| value.variant(out)).test_value()?,
            expected_variant
        );
        let payload = errors.access(|out| value.payload(out)).test_value()?;
        let payload = errors.access(|out| payload.kind(out)).test_value()?;
        match (index, payload) {
            (0, EnumDefaultPayloadKind::Unit) => {}
            (1, EnumDefaultPayloadKind::Single(_)) => {}
            (2, EnumDefaultPayloadKind::Structure(structure)) => {
                let entries = errors.access(|out| structure.entries(out)).test_value()?;
                let entry = errors
                    .access(|out| entries.first(out))
                    .test_value()?
                    .test_value()?;
                test_assert!(matches!(
                    errors.access(|out| entry.key(out)).test_value()?,
                    BraceKey::Identifier(_)
                ));
            }
            _ => return Err(TestFailure),
        }
    }

    Ok(())
}

#[test]
fn annotations_and_assertions_preserve_order_and_expression_nodes() -> TestResult {
    let mut errors = TestSlot::<AstAccessError>::default();
    let mut budget_errors = TestSlot::<BudgetError>::default();
    let mut syntax_errors = TestSlot::<SyntaxErrors>::default();
    let execution = budget_errors
        .access(|out| ExecutionBudget::new(ExecutionLimits::new(67_108_864, 67_108_864), out))
        .test_value()?;

    let source = r#"
        entry struct Root {
            values: [i32] @size(min = 1, max = 3) @unique @secret,
        }
        @assert(self.values.all(|value| value > 0), "values must be positive")
    "#;
    let schema = syntax_errors
        .access(|out| {
            SchemaParser::parse_str(
                SOURCE_ID,
                source,
                &SchemaParseOptions::default(),
                &execution,
                out,
            )
        })
        .test_value()?;
    let entry_item = errors
        .access(|out| schema.items().first(out))
        .test_value()?
        .test_value()?;
    let entry = entry_item.entry_struct().test_value()?;
    let fields = errors.access(|out| entry.fields(out)).test_value()?;
    let first_field = errors
        .access(|out| fields.first(out))
        .test_value()?
        .test_value()?;
    let annotations = errors
        .access(|out| first_field.annotations(out))
        .test_value()?;

    test_assert_eq!(annotations.len(), 3);
    let first_annotation = errors
        .access(|out| annotations.first(out))
        .test_value()?
        .test_value()?;
    test_assert_eq!(
        errors
            .access(|out| first_annotation.kind(out))
            .test_value()?,
        AnnotationKind::Size
    );
    let AnnotationData::Size(SizeArguments::Named(bounds)) = errors
        .access(|out| first_annotation.data(out))
        .test_value()?
    else {
        return Err(TestFailure);
    };
    let named_bounds = errors
        .access(|mut out| {
            let mut values = Vec::new();
            let arguments = {
                let iter = bounds.iter(out.reborrow());
                iter.collect::<Vec<_>>()
            };
            for argument in arguments {
                let argument = argument?;
                let name = argument.name().text(out.reborrow())?;
                values.push(name);
            }
            Ok(values)
        })
        .test_value()?;
    test_assert_eq!(named_bounds, ["min", "max"]);
    for (index, expected) in [AnnotationKind::Unique, AnnotationKind::Secret]
        .into_iter()
        .enumerate()
    {
        let annotation = errors
            .access(|out| annotations.get(index + 1, out))
            .test_value()?
            .test_value()?;
        test_assert_eq!(
            errors.access(|out| annotation.kind(out)).test_value()?,
            expected
        );
    }
    let assertions = errors.access(|out| entry.assertions(out)).test_value()?;
    test_assert_eq!(assertions.len(), 1);
    let assertion = errors
        .access(|out| assertions.first(out))
        .test_value()?
        .test_value()?;
    let message = errors.access(|out| assertion.message(out)).test_value()?;
    test_assert_eq!(
        errors.access(|out| message.value(out)).test_value()?,
        "values must be positive"
    );
    let expression = errors
        .access(|out| assertion.expression(out))
        .test_value()?;
    test_assert!(matches!(
        errors.access(|out| expression.kind(out)).test_value()?,
        ExprKind::MethodCall { .. }
    ));

    Ok(())
}

#[test]
fn all_annotation_forms_are_parsed_with_their_argument_categories() -> TestResult {
    let mut errors = TestSlot::<AstAccessError>::default();
    let mut budget_errors = TestSlot::<BudgetError>::default();
    let mut syntax_errors = TestSlot::<SyntaxErrors>::default();
    let execution = budget_errors
        .access(|out| ExecutionBudget::new(ExecutionLimits::new(67_108_864, 67_108_864), out))
        .test_value()?;

    let source = r#"
        entry struct Root {
            integer: i32 @min(-2) @max(9) @range(-2, 9),
            text: string @length(1, 8) @regex("^[a-z]+$") @host @email
                @cidr(v4_only = false) @env("TEXT"),
            values: [i32] @size(0, 3) @unique(by = "value")
                @all(self.integer > 0, "positive") @sum(min = 0, max = 9) @secret,
        }
    "#;
    let schema = syntax_errors
        .access(|out| {
            SchemaParser::parse_str(
                SOURCE_ID,
                source,
                &SchemaParseOptions::default(),
                &execution,
                out,
            )
        })
        .test_value()?;
    let entry_item = errors
        .access(|out| schema.items().first(out))
        .test_value()?
        .test_value()?;
    let entry = entry_item.entry_struct().test_value()?;
    let fields = errors.access(|out| entry.fields(out)).test_value()?;
    let kinds = errors
        .access(|mut out| {
            let mut kinds = Vec::new();
            for field_index in 0..fields.len() {
                let Some(field) = fields.get(field_index, out.reborrow())? else {
                    return Ok(Err(TestFailure));
                };
                let annotations = field.annotations(out.reborrow())?;
                for annotation_index in 0..annotations.len() {
                    let Some(annotation) = annotations.get(annotation_index, out.reborrow())?
                    else {
                        return Ok(Err(TestFailure));
                    };
                    kinds.push(annotation.kind(out.reborrow())?);
                }
            }
            Ok(Ok(kinds))
        })
        .test_value()?
        .test_value()?;

    test_assert_eq!(
        kinds,
        [
            AnnotationKind::Min,
            AnnotationKind::Max,
            AnnotationKind::Range,
            AnnotationKind::Length,
            AnnotationKind::Regex,
            AnnotationKind::Host,
            AnnotationKind::Email,
            AnnotationKind::Cidr,
            AnnotationKind::Env,
            AnnotationKind::Size,
            AnnotationKind::Unique,
            AnnotationKind::All,
            AnnotationKind::Sum,
            AnnotationKind::Secret,
        ]
    );
    macro_rules! annotation_data {
        ($field_index:expr, $annotation_index:expr) => {{
            let field = errors
                .access(|out| fields.get($field_index, out))
                .test_value()?
                .test_value()?;
            let annotations = errors.access(|out| field.annotations(out)).test_value()?;
            let annotation = errors
                .access(|out| annotations.get($annotation_index, out))
                .test_value()?
                .test_value()?;
            errors.access(|out| annotation.data(out)).test_value()?
        }};
    }
    test_assert!(matches!(
        annotation_data!(0, 2),
        AnnotationData::Range { .. }
    ));
    test_assert!(matches!(annotation_data!(1, 4), AnnotationData::Cidr(_)));
    test_assert!(matches!(annotation_data!(2, 2), AnnotationData::All(_)));
    let AnnotationData::Sum(bounds) = annotation_data!(2, 3) else {
        return Err(TestFailure);
    };
    let count = errors
        .access(|mut out| {
            let mut count = 0;
            let values = {
                let iter = bounds.iter(out.reborrow());
                iter.collect::<Vec<_>>()
            };
            for value in values {
                let _ = value?;
                count += 1;
            }
            Ok(count)
        })
        .test_value()?;
    test_assert_eq!(count, 2);

    Ok(())
}

#[test]
fn ast_owns_names_after_the_source_buffer_is_dropped() -> TestResult {
    let mut errors = TestSlot::<AstAccessError>::default();
    let mut budget_errors = TestSlot::<BudgetError>::default();
    let mut syntax_errors = TestSlot::<SyntaxErrors>::default();
    let execution = budget_errors
        .access(|out| ExecutionBudget::new(ExecutionLimits::new(67_108_864, 67_108_864), out))
        .test_value()?;

    let schema = {
        let source = String::from("entry struct Root { label: string = \"owned\", }");
        syntax_errors
            .access(|out| {
                SchemaParser::parse_str(
                    SOURCE_ID,
                    &source,
                    &SchemaParseOptions::default(),
                    &execution,
                    out,
                )
            })
            .test_value()?
    };
    let entry_item = errors
        .access(|out| schema.items().first(out))
        .test_value()?
        .test_value()?;
    let entry = entry_item.entry_struct().test_value()?;
    test_assert_eq!(errors.access(|out| entry.name(out)).test_value()?, "Root");
    let fields = errors.access(|out| entry.fields(out)).test_value()?;
    let field = errors
        .access(|out| fields.first(out))
        .test_value()?
        .test_value()?;
    test_assert_eq!(errors.access(|out| field.name(out)).test_value()?, "label");

    Ok(())
}

#[test]
fn empty_brace_defaults_and_trailing_commas_keep_their_syntax_shape() -> TestResult {
    let mut errors = TestSlot::<AstAccessError>::default();
    let mut budget_errors = TestSlot::<BudgetError>::default();
    let mut syntax_errors = TestSlot::<SyntaxErrors>::default();
    let execution = budget_errors
        .access(|out| ExecutionBudget::new(ExecutionLimits::new(67_108_864, 67_108_864), out))
        .test_value()?;

    let source = r#"
        enum Choice { Empty, }
        entry struct Root {
            map_value: map<string, i32> = {},
            object_value: Choice = .Empty,
            items: [i32] = [1, 2,],
        }
    "#;
    let schema = syntax_errors
        .access(|out| {
            SchemaParser::parse_str(
                SOURCE_ID,
                source,
                &SchemaParseOptions::default(),
                &execution,
                out,
            )
        })
        .test_value()?;
    let entry_item = errors
        .access(|out| schema.items().get(1, out))
        .test_value()?
        .test_value()?;
    let entry = entry_item.entry_struct().test_value()?;
    let fields = errors.access(|out| entry.fields(out)).test_value()?;
    let field = errors
        .access(|out| fields.first(out))
        .test_value()?
        .test_value()?;
    let default = errors
        .access(|out| field.default(out))
        .test_value()?
        .test_value()?;
    let DefaultValueKind::Braced(braced) = errors.access(|out| default.kind(out)).test_value()?
    else {
        return Err(TestFailure);
    };
    let entries = errors.access(|out| braced.entries(out)).test_value()?;
    test_assert!(entries.is_empty());

    Ok(())
}

#[test]
fn schema_parse_options_control_window_and_token_limits() -> TestResult {
    let mut budget_errors = TestSlot::<BudgetError>::default();
    let mut syntax_errors = TestSlot::<SyntaxErrors>::default();
    let execution = budget_errors
        .access(|out| ExecutionBudget::new(ExecutionLimits::new(67_108_864, 67_108_864), out))
        .test_value()?;
    let syntax = SyntaxLimits::default();
    let windowed = SchemaParseOptions::new(syntax, StreamLimits::try_new(1, 64).test_value()?);

    syntax_errors
        .access(|out| {
            SchemaParser::parse_str(
                SOURCE_ID,
                "entry struct Root {}",
                &windowed,
                &execution,
                out,
            )
        })
        .test_value()?;

    let token_limited = SchemaParseOptions::new(syntax, StreamLimits::try_new(8, 4).test_value()?);
    let diagnostics = syntax_errors
        .access(|out| {
            SchemaParser::parse_str(
                SOURCE_ID,
                "entry struct Root {}",
                &token_limited,
                &execution,
                out,
            )
        })
        .err()
        .test_value()?;
    test_assert_eq!(
        diagnostics.diagnostics().first().test_value()?.code(),
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
    );

    Ok(())
}
