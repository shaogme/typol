use super::{ConfigLimits, Parser, ParserTestSupport, foundation::Provider};
use crate::{
    ConfigParseOptions,
    diagnostics::DiagnosticFactory,
    environment::{Environment, EnvironmentError, EnvironmentReadContext},
    parser::context::ParserControl,
    session::ConfigSession,
};
use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};
use opeo::{ErrSlot, OResult, Out};
use typol_core::{
    BudgetError, Diagnostic, ExecutionLimits, SliceSource, SourceId, Span, SpanError, ValueKind,
    ValuePayloadView, ValueSource, ValueView,
};
use typol_policy::{ExcerptLimits, RegexOptions};
use typol_schema::{
    ResolverError, SchemaCompileErrors, SchemaCompileOptions, SchemaCompiler, SchemaResolveRequest,
    SchemaResolver, SchemaSource,
};
use typol_semantic::TypeKind;
use typol_test_support::*;

struct RepositoryExample {
    schema: &'static str,
    source: &'static str,
    expected_calls: usize,
    expected_field: &'static str,
}

fn present<T>(value: Option<T>) -> TestResult<T> {
    value.test_expect("schema is missing the expected member")
}

fn field<'a>(value: impl Into<ValueViewInput<'a>>, name: &str) -> TestResult<ValueView<'a>> {
    let ValueKind::Structure(mut fields) = value.into().value_view().as_ref() else {
        return Err(TestFailure::new(
            "expected the configuration value to be a struct",
        ));
    };
    Ok(fields
        .find(|field| field.name() == Some(name))
        .test_value()?
        .value())
}

#[test]
fn integer_widths_accept_exact_boundaries_and_reject_overflow() -> TestResult<()> {
    for bits in [8, 16, 32, 64, 128] {
        let signed =
            ParserTestSupport::compile(&format!("entry struct Root {{ value: i{bits}, }}"))?;
        let min = if bits == 128 {
            i128::MIN
        } else {
            -(1_i128 << (bits - 1))
        };
        let max = if bits == 128 {
            i128::MAX
        } else {
            (1_i128 << (bits - 1)) - 1
        };
        for literal in [
            min.to_string(),
            max.to_string(),
            format!("-0x{:x}", min.unsigned_abs()),
            "00042".into(),
        ] {
            let value = ParserTestSupport::parse(
                &signed,
                &format!("{{ value: {literal} }}"),
                &ConfigLimits::default(),
            )
            .test_value()?;
            let expected = if literal == "00042" {
                42
            } else if literal.starts_with('-') {
                min
            } else {
                max
            };
            test_assert!(
                matches!(field(&value, "value")?.as_ref(), ValueKind::Signed(number) if number == expected)
            )?;
        }
        for literal in [
            (max as u128 + 1).to_string(),
            format!("-{}", min.unsigned_abs() + 1),
        ] {
            test_eq!(
                ParserTestSupport::codes(
                    &ParserTestSupport::parse(
                        &signed,
                        &format!("{{ value: {literal} }}"),
                        &ConfigLimits::default()
                    )
                    .test_error()?
                ),
                [11]
            )?;
        }
        let unsigned =
            ParserTestSupport::compile(&format!("entry struct Root {{ value: u{bits}, }}"))?;
        let max = if bits == 128 {
            u128::MAX
        } else {
            (1_u128 << bits) - 1
        };
        for literal in ["0".into(), max.to_string(), format!("0x{max:x}")] {
            let value = ParserTestSupport::parse(
                &unsigned,
                &format!("{{ value: {literal} }}"),
                &ConfigLimits::default(),
            )
            .test_value()?;
            test_assert!(
                matches!(field(&value, "value")?.as_ref(), ValueKind::Unsigned(number) if number == if literal == "0" { 0 } else { max })
            )?;
        }
        let overflow = if bits == 128 {
            "340282366920938463463374607431768211456".into()
        } else {
            (max + 1).to_string()
        };
        for literal in [overflow, "-1".into(), "-0".into()] {
            test_eq!(
                ParserTestSupport::codes(
                    &ParserTestSupport::parse(
                        &unsigned,
                        &format!("{{ value: {literal} }}"),
                        &ConfigLimits::default()
                    )
                    .test_error()?
                ),
                [11]
            )?;
        }
    }

    Ok(())
}

#[test]
fn scalar_tokens_preserve_decoding_rounding_and_strict_types() -> TestResult<()> {
    let schema = ParserTestSupport::compile(
        "entry struct Root { text: string, flag: bool, small: f32, large: f64, }",
    )?;
    let value = ParserTestSupport::parse(
        &schema,
        r#"{ text: "éø\"\\\n\r\t", flag: false, small: 16777217.0, large: -0.0 }"#,
        &ConfigLimits::default(),
    )
    .test_value()?;
    test_assert!(matches!(
        field(&value, "text")?.as_ref(),
        ValueKind::String("éø\"\\\n\r\t")
    ))?;
    test_assert!(matches!(
        field(&value, "flag")?.as_ref(),
        ValueKind::Bool(false)
    ))?;
    test_assert!(
        matches!(field(&value, "small")?.as_ref(), ValueKind::Float32(number) if number.to_bits() == 16777216.0_f32.to_bits())
    )?;
    test_assert!(
        matches!(field(&value, "large")?.as_ref(), ValueKind::Float64(number) if number.to_bits() == (-0.0_f64).to_bits())
    )?;
    for (target, literal) in [
        ("string", "1"),
        ("u8", "\"1\""),
        ("u8", "1.0"),
        ("f32", "1"),
        ("bool", "1"),
        ("u8", "none"),
        ("f32", "340282400000000000000000000000000000000.0"),
    ] {
        let schema =
            ParserTestSupport::compile(&format!("entry struct Root {{ value: {target}, }}"))?;
        test_eq!(
            ParserTestSupport::codes(
                &ParserTestSupport::parse(
                    &schema,
                    &format!("{{ value: {literal} }}"),
                    &ConfigLimits::default()
                )
                .test_error()?
            ),
            [11]
        )?;
    }
    let schema = ParserTestSupport::compile("entry struct Root { value: f64, }")?;
    let overflow = format!("{{ value: {}.0 }}", "9".repeat(310));
    test_eq!(
        ParserTestSupport::codes(
            &ParserTestSupport::parse(&schema, &overflow, &ConfigLimits::default()).test_error()?
        ),
        [11]
    )?;
    for literal in ["+1.0", "1e2", "1_000", "0XFF", "1.", ".5", r#""\u0041""#] {
        test_assert!(
            ParserTestSupport::parse(
                &schema,
                &format!("{{ value: {literal} }}"),
                &ConfigLimits::default()
            )
            .is_err()
        )?;
    }

    Ok(())
}

#[test]
fn containers_options_and_variants_keep_their_declared_shapes_and_order() -> TestResult<()> {
    let schema = ParserTestSupport::compile(
        r#"
        struct Child { id: u8, name: string = "default", }
        enum E { Unit, Single(Child), Object { id: u8 = 7, missing: option<string>, }, }
        entry struct Root {
            child: option<Child>, variant: option<E>, values: [u8; 2],
            dict: map<string, u8>, defaulted: option<u8> = 9, omitted: option<string>,
        }
    "#,
    )?;
    let value = ParserTestSupport::parse(&schema, r#"{ dict: { "z": 2, "a": 1, }, values: [2, 1,], variant: .Object {}, child: { id: 3 }, defaulted: none, }"#, &ConfigLimits::default()).test_value()?;
    let ValueKind::Structure(fields) = value.as_ref() else {
        return Err(TestFailure::new("expected a struct"));
    };
    test_eq!(
        fields
            .into_iter()
            .map(|field| field.as_named().map(|named| named.name()))
            .collect::<Option<Vec<_>>>()
            .test_value()?,
        ["child", "variant", "values", "dict", "defaulted", "omitted"]
    )?;
    let ValueKind::Map(entries) = field(&value, "dict")?.as_ref() else {
        return Err(TestFailure::new("expected a map"));
    };
    test_eq!(
        entries
            .into_iter()
            .map(|entry| entry.as_named().map(|named| named.name()))
            .collect::<Option<Vec<_>>>()
            .test_value()?,
        ["z", "a"]
    )?;
    let ValueKind::Array(mut values) = field(&value, "values")?.as_ref() else {
        return Err(TestFailure::new("expected an array"));
    };
    test_assert!(matches!(
        values.next().test_value()?.value().as_ref(),
        ValueKind::Unsigned(2)
    ))?;
    test_assert!(matches!(
        values.next().test_value()?.value().as_ref(),
        ValueKind::Unsigned(1)
    ))?;
    for name in ["defaulted", "omitted"] {
        test_assert!(matches!(
            field(&value, name)?.as_ref(),
            ValueKind::Optional(None)
        ))?;
    }
    let ValueKind::Optional(Some(child)) = field(&value, "child")?.as_ref() else {
        return Err(TestFailure::new("expected promotion to Some"));
    };
    test_eq!(field(child, "name")?.source(), ValueSource::SchemaDefault)?;
    let ValueKind::Optional(Some(variant)) = field(&value, "variant")?.as_ref() else {
        return Err(TestFailure::new("expected promotion to Some"));
    };
    let ValueKind::Enumeration {
        variant,
        payload: ValuePayloadView::Structure(mut fields),
    } = variant.as_ref()
    else {
        return Err(TestFailure::new("expected an anonymous-struct payload"));
    };
    test_eq!(variant, "Object")?;
    test_assert!(matches!(
        fields.next().test_value()?.value().as_ref(),
        ValueKind::Unsigned(7)
    ))?;
    test_assert!(matches!(
        fields.next().test_value()?.value().as_ref(),
        ValueKind::Optional(None)
    ))?;

    let schema = ParserTestSupport::compile(
        "enum E { Unit, Single(u8), Object { id: u8, }, } entry struct Root { e: E, }",
    )?;
    for source in [
        "{ e: .Unit }",
        "{ e: .Single(1) }",
        "{ e: .Object { id: 1 } }",
    ] {
        test_assert!(ParserTestSupport::parse(&schema, source, &ConfigLimits::default()).is_ok())?;
    }
    for source in [
        "{ e: .Unknown }",
        "{ e: .Unit(1) }",
        "{ e: .Single }",
        "{ e: .Single { id: 1 } }",
        "{ e: .Object(1) }",
        "{ e: .Object { other: 1 } }",
    ] {
        test_assert!(ParserTestSupport::parse(&schema, source, &ConfigLimits::default()).is_err())?;
    }
    for (target, source) in [("[u8; 0]", "[]"), ("[u8]", "[]"), ("map<string, u8>", "{}")] {
        let schema =
            ParserTestSupport::compile(&format!("entry struct Root {{ value: {target}, }}"))?;
        test_assert!(
            ParserTestSupport::parse(
                &schema,
                &format!("{{ value: {source} }}"),
                &ConfigLimits::default()
            )
            .is_ok()
        )?;
    }
    for (target, source) in [
        ("[u8; 2]", "[1]"),
        ("[u8; 2]", "[1, 2, 3]"),
        ("[u8; 4294967295]", "[]"),
        ("map<string, u8>", "{ key: 1 }"),
        ("u8", "{}"),
    ] {
        let schema =
            ParserTestSupport::compile(&format!("entry struct Root {{ value: {target}, }}"))?;
        test_assert!(
            ParserTestSupport::parse(
                &schema,
                &format!("{{ value: {source} }}"),
                &ConfigLimits::default()
            )
            .is_err()
        )?;
    }

    Ok(())
}

fn assert_default_tree(value: ValueView<'_>) -> TestResult<()> {
    test_eq!(value.source(), ValueSource::SchemaDefault)?;
    match value.as_ref() {
        ValueKind::Optional(Some(inner)) => assert_default_tree(inner)?,
        ValueKind::Array(values) => {
            for value in values {
                assert_default_tree(value.value())?;
            }
        }
        ValueKind::Map(entries) => {
            for entry in entries {
                assert_default_tree(entry.value())?;
            }
        }
        ValueKind::Structure(fields) => {
            for field in fields {
                assert_default_tree(field.value())?;
            }
        }
        ValueKind::Enumeration { payload, .. } => match payload {
            ValuePayloadView::Unit => {}
            ValuePayloadView::Single(inner) => assert_default_tree(inner)?,
            ValuePayloadView::Structure(fields) => {
                for field in fields {
                    assert_default_tree(field.value())?;
                }
            }
        },
        _ => {}
    }
    Ok(())
}

#[test]
fn copied_defaults_own_every_shape_and_survive_schema_and_source_drop() -> TestResult<()> {
    let value = {
        let schema = ParserTestSupport::compile(
            r#"
            struct Child { text: string = "éø", }
            enum E { Unit, Single(string), Object { child: Child, }, }
            entry struct Root {
                signed: i128 = -170141183460469231731687303715884105728,
                unsigned: u128 = 340282366920938463463374607431768211455,
                small: f32 = 1.25, large: f64 = -2.5, flag: bool = true,
                text: string = "éø", absent: option<string> = none,
                present: option<Child> = {}, array: [string] = ["first", "second"],
                dict: map<string, [u8]> = { "z": [2], "a": [1] },
                child: Child = {}, unit: E = .Unit, single: E = .Single("payload"),
                object: E = .Object { child: {} },
            }
        "#,
        )?;
        let source = String::from("{}");
        let value =
            ParserTestSupport::parse(&schema, &source, &ConfigLimits::default()).test_value()?;
        let ValueKind::Structure(fields) = value.as_ref() else {
            return Err(TestFailure::new("expected a struct"));
        };
        for copied in fields {
            assert_default_tree(copied.value())?;
        }
        let root = schema.entry();
        let mut schema_errors = TestSlot::default();
        let TypeKind::Struct { fields: ids } =
            ParserTestSupport::access(&mut schema_errors, |out| root.kind(out))?
        else {
            return Err(TestFailure::new("expected a struct"));
        };
        let original_field = ids.get(5).test_value()?;
        let original = present(ParserTestSupport::access(&mut schema_errors, |out| {
            original_field.default_value(out)
        })?)?;
        let ValueKind::String(original) = original.as_ref() else {
            return Err(TestFailure::new("expected a string"));
        };
        let ValueKind::String(copied) = field(&value, "text")?.as_ref() else {
            return Err(TestFailure::new("expected a string"));
        };
        test_ne!(original.as_ptr(), copied.as_ptr())?;
        value
    };
    test_assert!(matches!(
        field(&value, "signed")?.as_ref(),
        ValueKind::Signed(i128::MIN)
    ))?;
    test_assert!(matches!(
        field(&value, "unsigned")?.as_ref(),
        ValueKind::Unsigned(u128::MAX)
    ))?;
    test_assert!(matches!(
        field(&value, "text")?.as_ref(),
        ValueKind::String("éø")
    ))?;
    test_assert!(
        matches!(field(&value, "small")?.as_ref(), ValueKind::Float32(number) if number.to_bits() == 1.25_f32.to_bits())
    )?;
    test_assert!(
        matches!(field(&value, "large")?.as_ref(), ValueKind::Float64(number) if number.to_bits() == (-2.5_f64).to_bits())
    )?;
    test_assert!(matches!(
        field(&value, "flag")?.as_ref(),
        ValueKind::Bool(true)
    ))?;
    test_assert!(matches!(
        field(&value, "absent")?.as_ref(),
        ValueKind::Optional(None)
    ))?;
    let ValueKind::Optional(Some(present)) = field(&value, "present")?.as_ref() else {
        return Err(TestFailure::new("expected Some"));
    };
    test_assert!(matches!(
        field(present, "text")?.as_ref(),
        ValueKind::String("éø")
    ))?;
    test_assert!(matches!(
        field(field(&value, "child")?, "text")?.as_ref(),
        ValueKind::String("éø")
    ))?;
    let ValueKind::Array(mut array) = field(&value, "array")?.as_ref() else {
        return Err(TestFailure::new("expected an array"));
    };
    test_eq!(array.len(), 2)?;
    test_assert!(matches!(
        array.next().test_value()?.value().as_ref(),
        ValueKind::String("first")
    ))?;
    test_assert!(matches!(
        array.next().test_value()?.value().as_ref(),
        ValueKind::String("second")
    ))?;
    test_assert!(matches!(
        field(&value, "unit")?.as_ref(),
        ValueKind::Enumeration {
            variant: "Unit",
            payload: ValuePayloadView::Unit
        }
    ))?;
    let ValueKind::Enumeration {
        variant: "Object",
        payload: ValuePayloadView::Structure(mut fields),
    } = field(&value, "object")?.as_ref()
    else {
        return Err(TestFailure::new("expected an anonymous-struct enum"));
    };
    let child = fields.next().test_value()?;
    test_eq!(child.name(), Some("child"))?;
    test_assert!(matches!(
        field(child, "text")?.as_ref(),
        ValueKind::String("éø")
    ))?;
    let ValueKind::Map(mut entries) = field(&value, "dict")?.as_ref() else {
        return Err(TestFailure::new("expected a map"));
    };
    for expected in [2, 1] {
        let entry = entries.next().test_value()?;
        let ValueKind::Array(mut values) = entry.value().as_ref() else {
            return Err(TestFailure::new("expected an array"));
        };
        test_eq!(values.len(), 1)?;
        test_assert!(
            matches!(values.next().test_value()?.value().as_ref(), ValueKind::Unsigned(number) if number == expected)
        )?;
    }
    test_assert!(
        matches!(field(&value, "single")?.as_ref(), ValueKind::Enumeration { variant: "Single", payload: ValuePayloadView::Single(inner) } if matches!(inner.as_ref(), ValueKind::String("payload")))
    )?;

    Ok(())
}

#[derive(Default)]
struct Common {
    source: OwnedReadSource,
}

impl SchemaResolver for Common {
    fn resolve<'a, 'slot, 'borrow>(
        &'a mut self,
        request: SchemaResolveRequest<'_>,
        out: Out<'slot, 'borrow, ResolverError>,
    ) -> OResult<'slot, SchemaSource<'a>> {
        let name = request.module_name;
        if name == "common" {
            let text = include_str!("../../../../../examples/config/common.tschem");
            self.source.replace(text);
            Ok(SchemaSource::from_source(b"common", &mut self.source))
        } else {
            Err(out.fail(ResolverError::NotFound))
        }
    }
}

#[derive(Default)]
struct FixtureEnvironment {
    names: Vec<String>,
}

impl Environment for FixtureEnvironment {
    fn read<'env, 'slot, 'borrow>(
        &'env mut self,
        name: &str,
        _: EnvironmentReadContext<'_>,
        out: Out<'slot, 'borrow, EnvironmentError>,
    ) -> OResult<'slot, Option<&'env [u8]>> {
        if self.names.try_reserve(1).is_err() {
            return Err(out.fail(EnvironmentError::AllocationFailed));
        }
        let mut copy = String::new();
        if copy.try_reserve_exact(name.len()).is_err() {
            return Err(out.fail(EnvironmentError::AllocationFailed));
        }
        copy.push_str(name);
        self.names.push(copy);
        Ok(match name {
            "TYPOL_CLIENT_SECRET" | "TYPOL_DB_PASSWORD" | "TYPOL_NOTIFY_TOKEN" => {
                Some(b"fixture-secret-value")
            }
            "TYPOL_APP_NAME" => Some(b"gateway-core"),
            _ => None,
        })
    }
}

#[test]
fn repository_examples_pass_the_complete_private_semantic_pipeline() -> TestResult<()> {
    let mut schema_errors = TestSlot::<SchemaCompileErrors>::default();
    let mut budget_errors = ErrSlot::<BudgetError>::new();
    let mut session_errors = TestSlot::<Diagnostic>::default();
    let mut parse_errors = TestSlot::default();
    for example in [
        RepositoryExample {
            schema: include_str!("../../../../../examples/config/all_types.tschem"),
            source: include_str!("../../../../../examples/config/all_types.tconf"),
            expected_calls: 7,
            expected_field: "integers",
        },
        RepositoryExample {
            schema: include_str!("../../../../../examples/config/database.tschem"),
            source: include_str!("../../../../../examples/config/database.tconf"),
            expected_calls: 5,
            expected_field: "primary_host",
        },
        RepositoryExample {
            schema: include_str!("../../../../../examples/config/server.tschem"),
            source: include_str!("../../../../../examples/config/server.tconf"),
            expected_calls: 10,
            expected_field: "app_name",
        },
    ] {
        let limits = ConfigLimits::default();
        let schema = schema_errors
            .run(|mut out| {
                let compiler =
                    SchemaCompiler::new(SchemaCompileOptions::default(), out.reborrow())?;
                compiler.compile(
                    SchemaSource::from_str(b"root", example.schema),
                    &mut Common::default(),
                    None,
                    out.reborrow(),
                )
            })
            .test_value()?;
        let mut provider = FixtureEnvironment::default();
        let options = session_options(&limits);
        let session = session_errors
            .run(|out| ConfigSession::new(&mut provider, &options, &mut budget_errors, out))
            .test_value()?;
        let mut parser_input_1 = SliceSource::new(example.source.as_bytes());
        let parser = parse_errors
            .run(|out| Parser::new(&schema, &mut parser_input_1, &limits, session, out))
            .test_value()?;
        let value = parse_errors.run(|out| parser.parse(out)).test_value()?;
        test_eq!(provider.names.len(), example.expected_calls)?;
        test_eq!(
            provider
                .names
                .iter()
                .filter(|name| name.as_str() == "TYPOL_APP_NAME")
                .count(),
            usize::from(example.expected_field == "app_name")
        )?;
        field(&value, example.expected_field)?;
        let mut absent = Provider::new(Ok(None));
        let options = session_options(&limits);
        let session = session_errors
            .run(|out| ConfigSession::new(&mut absent, &options, &mut budget_errors, out))
            .test_value()?;
        let mut parser_input_2 = SliceSource::new(example.source.as_bytes());
        let parser = parse_errors
            .run(|out| Parser::new(&schema, &mut parser_input_2, &limits, session, out))
            .test_value()?;
        let errors = parse_errors.run(|out| parser.parse(out)).test_error()?;
        test_assert!(errors.iter().any(|error| error.code().get() == 11))?;
    }

    Ok(())
}

#[test]
fn source_limits_follow_consumption_and_root_requires_single_object() -> TestResult<()> {
    let schema = ParserTestSupport::compile("entry struct Root {}")?;
    let mut limits = ConfigLimits::default();
    limits = limits.with_max_source_bytes(1);
    test_eq!(
        ParserTestSupport::codes(&ParserTestSupport::parse(&schema, "`x", &limits).test_error()?),
        [3]
    )?;
    limits = limits.with_max_source_bytes(2);
    test_assert!(ParserTestSupport::parse(&schema, "{}", &limits).is_ok())?;
    for source in ["", "[]", ".Unit", "true", "{} {}", "{} 1"] {
        test_assert!(ParserTestSupport::parse(&schema, source, &ConfigLimits::default()).is_err())?;
    }

    Ok(())
}

#[test]
fn newline_comments_unicode_columns_and_two_source_routing_share_core_rules() -> TestResult<()> {
    let limits = ConfigLimits::default();
    let mut schema_errors = TestSlot::<SchemaCompileErrors>::default();
    let schema = schema_errors
        .run(|mut out| {
            let compiler = SchemaCompiler::new(SchemaCompileOptions::default(), out.reborrow())?;
            compiler.compile(
                SchemaSource::from_str(
                    b"root",
                    "extern mod common; entry struct Root { value: u8, }",
                ),
                &mut Common::default(),
                None,
                out.reborrow(),
            )
        })
        .test_value()?;
    test_eq!(schema.next_source_id(), Some(SourceId::new(2)))?;
    let mut budget_errors = ErrSlot::new();
    let mut session_errors = TestSlot::<Diagnostic>::default();
    let mut parse_errors = TestSlot::default();
    let mut schema_access_errors = TestSlot::default();
    let mut span_errors = TestSlot::<SpanError>::default();
    for newline in ["\n", "\r\n"] {
        let source = format!("{{{newline}/*é*/ value: 999,{newline}}}");
        let mut provider = Provider::new(Ok(None));
        let options = session_options(&limits);
        let session = session_errors
            .run(|out| ConfigSession::new(&mut provider, &options, &mut budget_errors, out))
            .test_value()?;
        let mut parser_input_3 = SliceSource::new(source.as_bytes());
        let parser = parse_errors
            .run(|out| Parser::new(&schema, &mut parser_input_3, &limits, session, out))
            .test_value()?;
        let start = source.find("999").unwrap_or_default();
        let config_span = span_errors
            .run(|out| Span::new(start, start.saturating_add(3), out))
            .value()
            .copied()
            .unwrap_or(Span::empty());
        let config_span = DiagnosticFactory::source_span(SourceId::new(2), config_span);
        let root = schema.entry();
        let TypeKind::Struct { fields } =
            ParserTestSupport::access(&mut schema_access_errors, |out| root.kind(out))?
        else {
            return Err(TestFailure::new("expected a struct"));
        };
        let field = present(fields.get(0))?;
        let schema_span =
            ParserTestSupport::access(&mut schema_access_errors, |out| field.span(out))?;
        test_eq!(
            ParserControl::span_location(&parser.context, schema_span.into()),
            schema.span_location(schema_span)
        )?;
        test_assert!(
            ParserControl::span_location(
                &parser.context,
                DiagnosticFactory::source_span(SourceId::new(99), Span::empty())
            )
            .is_none()
        )?;
        let errors = parse_errors.run(|out| parser.parse(out)).test_error()?;
        test_eq!(
            TestCheck::item(&errors, 0)?.primary(),
            Some(config_span.source_span())
        )?;
        let location = errors
            .span_location(config_span.source_span())
            .test_value()?;
        test_eq!((location.start.line(), location.start.column()), (2, 14))?;
        let valid = format!("// comment{newline}{{ /*block comment*/ value: 1, }}{newline}");
        test_assert!(ParserTestSupport::parse(&schema, &valid, &limits).is_ok())?;
    }
    for source in [
        "{\rvalue: 1 }",
        "// comment\r\n{ value: 1 }\r",
        "/* comment\r */{ value: 1 }",
        "{\u{a0}value: 1 }",
        "{ value: \"\r\" }",
    ] {
        test_eq!(
            ParserTestSupport::codes(
                &ParserTestSupport::parse(&schema, source, &limits).test_error()?
            ),
            [3]
        )?;
    }
    let source = "{ value:";
    let errors = ParserTestSupport::parse(&schema, source, &limits).test_error()?;
    test_eq!(
        TestCheck::item(&errors, 0)?
            .primary()
            .test_value()?
            .span()
            .start(),
        source.len()
    )?;
    test_assert!(
        TestCheck::item(&errors, 0)?
            .primary()
            .test_value()?
            .span()
            .is_empty()
    )?;

    Ok(())
}

#[test]
fn specification_config_blocks_use_the_private_streaming_parser() -> TestResult<()> {
    let schemas = [
        "struct Endpoint { host: string, port: u16, } entry struct Root { primary_host: Endpoint, read_replicas: [Endpoint], }",
        "entry struct Root { labels: map<string, string>, }",
        "struct Tls { cert_path: string, key_path: string, } struct Endpoint { host: string, port: u16, } enum Protocol { Tcp, } enum Transport { Tls(Tls), } enum Role { Follower { leader_endpoint: Endpoint, sync_timeout_ms: u32, }, } entry struct Root { protocol: Protocol, tls: Transport, role: Role, }",
        "struct Tag { id: u32, name: string, } entry struct Root { optional_int: option<i32>, optional_tag: option<Tag>, }",
    ];
    let specification =
        include_str!("../../../../../docs/src/content/docs/specification/config-dsl.md");
    let blocks = specification.split("```tconf\n").skip(1);
    let mut count = 0;
    for (text, schema_text) in blocks.zip(schemas) {
        let source = text.split("```").next().test_value()?;
        let schema = ParserTestSupport::compile(schema_text)?;
        ParserTestSupport::parse(&schema, source, &ConfigLimits::default()).test_value()?;
        count += 1;
    }
    test_eq!(count, schemas.len())?;
    test_eq!(specification.matches("```tconf\n").count(), schemas.len())?;

    Ok(())
}

fn session_options(limits: &ConfigLimits) -> ConfigParseOptions {
    ConfigParseOptions::new(
        ExcerptLimits::default(),
        RegexOptions::default(),
        *limits,
        ExecutionLimits::new(1_000_000_000, 268_435_456),
    )
}
