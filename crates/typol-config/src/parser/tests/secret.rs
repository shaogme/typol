use super::{ConfigLimits, ParserTestSupport, foundation::Provider};
use crate::{
    ConfigParseOptions, ConfigParseRequest, ConfigParser,
    diagnostics::ConfigParseErrors,
    environment::{Environment, EnvironmentError, EnvironmentReadContext},
};
use alloc::{format, string::String, vec::Vec};
use opeo::{OResult, Out};
use typol_core::{
    ConfigValue, DiagnosticCode, ExecutionLimits, SliceSource, ValueKind, ValueSource,
};
use typol_policy::ResourceLimits;
use typol_schema::{
    ResolverError, SchemaCompileErrors, SchemaCompileOptions, SchemaCompiler, SchemaResolveRequest,
    SchemaResolver, SchemaSource,
};
use typol_semantic::{CompiledSchema, SchemaAccessError, TypeKind};
use typol_test_support::*;

struct HiddenValueCase {
    source: &'static str,
    bytes: Option<&'static [u8]>,
    code: u32,
    origin: ValueSource,
}

struct Imports;

impl SchemaResolver for Imports {
    fn resolve<'a, 'slot, 'borrow>(
        &'a mut self,
        _request: SchemaResolveRequest<'_>,
        _: Out<'slot, 'borrow, ResolverError>,
    ) -> OResult<'slot, SchemaSource<'a>> {
        Ok(SchemaSource::from_str(
            b"common",
            "type Secret = u8 @secret @min(2);",
        ))
    }
}

struct SecretEnvironment {
    bytes: Vec<u8>,
}

impl Environment for SecretEnvironment {
    fn read<'env, 'slot, 'borrow>(
        &'env mut self,
        name: &str,
        _: EnvironmentReadContext<'_>,
        _: Out<'slot, 'borrow, EnvironmentError>,
    ) -> OResult<'slot, Option<&'env [u8]>> {
        Ok((name == "X").then_some(self.bytes.as_slice()))
    }
}

fn run(
    schema: &CompiledSchema,
    source: &str,
    bytes: Option<&'static [u8]>,
    limits: &ConfigLimits,
) -> TestOpeoOutcome<ConfigValue, ConfigParseErrors> {
    let mut provider = Provider::new(Ok(bytes));
    parse_with_environment(schema, source, &mut provider, limits)
}

fn parse_with_environment(
    schema: &CompiledSchema,
    source: &str,
    provider: &mut dyn Environment,
    limits: &ConfigLimits,
) -> TestOpeoOutcome<ConfigValue, ConfigParseErrors> {
    let mut errors = TestSlot::default();
    match errors.run(|out| {
        ConfigParser::new(schema).parse(
            ConfigParseRequest::new(
                &mut SliceSource::new(source.as_bytes()),
                "secret-test.tconf",
                session_options(limits),
            ),
            provider,
            out,
        )
    }) {
        TestOpeoOutcome::Value(value) => TestOpeoOutcome::Value(value.into_config_value()),
        TestOpeoOutcome::Error(error) => TestOpeoOutcome::Error(error),
    }
}

fn parse(schema: &CompiledSchema, source: &str) -> TestOpeoOutcome<ConfigValue, ConfigParseErrors> {
    run(schema, source, None, &ConfigLimits::default())
}

fn assert_hidden(errors: &ConfigParseErrors, lengths: &[usize]) -> TestResult<()> {
    test_eq!(errors.diagnostics().len(), lengths.len())?;
    for (error, length) in errors.diagnostics().iter().zip(lengths) {
        test_eq!(error.message(), "value is hidden")?;
        let path = error.path().test_value()?;
        test_eq!(path.len(), *length)?;
        test_assert!(
            path.segments()
                .iter()
                .all(|segment| segment.as_map_key().is_none() && segment.as_index().is_none())
        )?;
        for label in error.labels() {
            test_assert!(!label.message().contains("confidential"))?;
        }
        test_assert!(!format!("{error:?}").contains("confidential"))?;
    }
    test_assert!(!format!("{errors:?}").contains("confidential"))?;
    Ok(())
}

#[test]
fn named_aliases_and_every_type_use_keep_secret_identity() -> TestResult<()> {
    let schema = ParserTestSupport::compile(
        "type A = string @secret; type B = A; entry struct Root { direct: B, array: [B], dict: map<string, B>, optional: option<B>, }",
    )?;
    let mut schema_errors = TestSlot::<SchemaAccessError>::default();
    let direct = ParserTestSupport::required(&mut schema_errors, |out| {
        schema.entry().field_named("direct", out)
    })?;
    let alias = ParserTestSupport::access(&mut schema_errors, |out| direct.ty(out))?;
    test_assert!(ParserTestSupport::access(&mut schema_errors, |out| alias.is_secret(out))?)?;
    for name in ["array", "dict", "optional"] {
        let field = ParserTestSupport::required(&mut schema_errors, |out| {
            schema.entry().field_named(name, out)
        })?;
        let info = ParserTestSupport::access(&mut schema_errors, |out| field.ty(out))?;
        let ty = match ParserTestSupport::access(&mut schema_errors, |out| info.kind(out))? {
            TypeKind::Array { element, .. } => element,
            TypeKind::Map { value } => value,
            TypeKind::Option { inner } => inner,
            _ => return Err(TestFailure::new("field should be a container type")),
        };
        let info = ty;
        test_assert!(ParserTestSupport::access(&mut schema_errors, |out| info.is_secret(out))?)?;
        test_assert!(
            ParserTestSupport::access(&mut schema_errors, |out| info.name(out))?.is_none()
        )?;
        let constraints =
            ParserTestSupport::access(&mut schema_errors, |out| info.constraints(out))?;
        test_assert!(ParserTestSupport::access(&mut schema_errors, |out| {
            constraints.is_empty(out)
        })?)?;
    }

    Ok(())
}

#[test]
fn container_boundaries_hide_keys_indices_and_payload_suffixes() -> TestResult<()> {
    for (declaration, source) in [
        (
            "value: map<string, [u8]> @secret",
            r#"{ value: { "confidential-key": [999] } }"#,
        ),
        ("value: [option<u8>] @secret", "{ value: [999] }"),
        (
            "value: option<map<string, u8>> @secret",
            r#"{ value: { "confidential-key": 999 } }"#,
        ),
        ("value: E @secret", "{ value: .Single(999) }"),
    ] {
        let schema = ParserTestSupport::compile(&format!(
            "enum E {{ Single(u8), }} entry struct Root {{ {declaration}, }}"
        ))?;
        assert_hidden(&parse(&schema, source).test_error()?, &[1])?;
    }

    Ok(())
}

#[test]
fn secret_element_aliases_hide_map_keys_and_array_indices() -> TestResult<()> {
    for (target, literal) in [
        ("[B]", "[999]"),
        ("map<string, B>", r#"{ "confidential-key": 999 }"#),
        ("[option<B>]", "[999]"),
        ("option<B>", "999"),
    ] {
        let schema = ParserTestSupport::compile(&format!(
            "type A = u8 @secret; type B = A; entry struct Root {{ value: {target}, }}"
        ))?;
        assert_hidden(
            &parse(&schema, &format!("{{ value: {literal} }}")).test_error()?,
            &[1],
        )?;
    }

    Ok(())
}

#[test]
fn ordinary_objects_keep_public_paths_and_only_hide_secret_fields() -> TestResult<()> {
    let schema = ParserTestSupport::compile(
        "struct Child { secret: u8 @secret, public: u8, } entry struct Root { child: Child, }",
    )?;
    let errors = parse(&schema, "{ child: { secret: 999, public: 999 } }").test_error()?;
    let errors = errors.diagnostics();
    test_eq!(TestCheck::item(errors, 0)?.message(), "value is hidden")?;
    test_ne!(TestCheck::item(errors, 1)?.message(), "value is hidden")?;
    for (error, name) in errors.iter().zip(["secret", "public"]) {
        let path = error.path().test_value()?;
        test_eq!(path.len(), 2)?;
        test_eq!(TestCheck::item(path.segments(), 1)?.as_field(), Some(name))?;
    }

    Ok(())
}

#[test]
fn single_payload_and_anonymous_variant_fields_obey_secret_boundaries() -> TestResult<()> {
    let schema = ParserTestSupport::compile(
        "enum E { Single(u8 @secret), Object { secret: u8 @secret, }, } entry struct Root { value: E, }",
    )?;
    assert_hidden(
        &parse(&schema, "{ value: .Single(999) }").test_error()?,
        &[1],
    )?;
    assert_hidden(
        &parse(&schema, "{ value: .Object { secret: 999 } }").test_error()?,
        &[3],
    )?;

    Ok(())
}

#[test]
fn literal_environment_fallback_and_invalid_utf8_errors_are_hidden() -> TestResult<()> {
    let schema = ParserTestSupport::compile(
        r#"entry struct Root { value: string @secret @length(16, 64) @env("X"), }"#,
    )?;
    for case in [
        HiddenValueCase {
            source: r#"{ value: "confidential" }"#,
            bytes: None,
            code: 12,
            origin: ValueSource::ConfigLiteral,
        },
        HiddenValueCase {
            source: r#"{ value: env("X") }"#,
            bytes: Some(&b"confidential"[..]),
            code: 12,
            origin: ValueSource::EnvironmentVariable,
        },
        HiddenValueCase {
            source: r#"{ value: env("X", "confidential") }"#,
            bytes: None,
            code: 12,
            origin: ValueSource::ExplicitFallback,
        },
        HiddenValueCase {
            source: r#"{ value: env("X") }"#,
            bytes: Some(&b"\xff"[..]),
            code: 13,
            origin: ValueSource::EnvironmentVariable,
        },
    ] {
        let errors =
            run(&schema, case.source, case.bytes, &ConfigLimits::default()).test_error()?;
        assert_hidden(&errors, &[1])?;
        test_eq!(
            TestCheck::item(errors.diagnostics(), 0)?.code().get(),
            case.code
        )?;
        test_eq!(
            TestCheck::item(errors.diagnostics(), 0)?.value_source(),
            Some(case.origin)
        )?;
    }

    Ok(())
}

#[test]
fn default_secret_subtrees_keep_schema_locations_and_mixed_assertion_source() -> TestResult<()> {
    let schema = ParserTestSupport::compile(
        "struct Child { n: u8, } @assert(self.n > root.limit, \"default rule\") entry struct Root { limit: u8, value: map<string, Child> @secret = { \"confidential-key\": { n: 1 } }, }",
    )?;
    let errors = parse(&schema, "{ limit: 2 }").test_error()?;
    assert_hidden(&errors, &[1])?;
    test_eq!(
        TestCheck::item(errors.diagnostics(), 0)?.code(),
        DiagnosticCode::CONFIG_VALIDATION_ERROR
    )?;
    test_eq!(
        TestCheck::item(errors.diagnostics(), 0)?.value_source(),
        Some(ValueSource::SchemaDefault)
    )?;
    let span = TestCheck::item(errors.diagnostics(), 0)?
        .primary()
        .test_value()?;
    test_ne!(Some(span.source_id()), errors.config_source_id())?;
    test_assert!(errors.span_location(span).is_some())?;
    test_assert!(
        errors
            .span_location(
                TestCheck::item(TestCheck::item(errors.diagnostics(), 0)?.labels(), 0)?.span()
            )
            .is_some()
    )?;

    Ok(())
}

#[test]
fn errors_own_unicode_crlf_positions_and_imported_schema_labels() -> TestResult<()> {
    let mut schema_errors = TestSlot::<SchemaCompileErrors>::default();
    let schema = schema_errors
        .run(|mut out| {
            let compiler = SchemaCompiler::new(SchemaCompileOptions::default(), out.reborrow())?;
            compiler.compile(
                SchemaSource::from_str(
                    b"root",
                    "extern mod common; entry struct Root { value: common::Secret, }",
                ),
                &mut Imports,
                None,
                out.reborrow(),
            )
        })
        .test_value()?;
    let errors = {
        let source = String::from("{\r\n/*é*/ value: 1\r\n}");
        parse(&schema, &source).test_error()?
    };
    assert_hidden(&errors, &[1])?;
    test_eq!(errors.config_source_name(), Some("secret-test.tconf"))?;
    let error = TestCheck::item(errors.diagnostics(), 0)?;
    let location = errors
        .span_location(error.primary().test_value()?)
        .test_value()?;
    test_eq!((location.start.line(), location.start.column()), (2, 14))?;
    let label = TestCheck::item(error.labels(), 0)?.span();
    test_ne!(Some(label.source_id()), errors.config_source_id())?;
    test_assert!(errors.span_location(label).is_some())?;

    Ok(())
}

#[test]
fn successful_secret_values_are_owned_and_unchanged() -> TestResult<()> {
    let value = {
        let schema = ParserTestSupport::compile(
            r#"entry struct Root { literal: string @secret, environment: string @secret @env("X"), fallback: string @secret @env("Y"), defaulted: string @secret = "confidential-default", }"#,
        )?;
        let source = String::from(
            r#"{ literal: "confidential-literal", environment: env("X"), fallback: env("Y", "confidential-fallback") }"#,
        );
        let mut provider = SecretEnvironment {
            bytes: b"confidential-environment".to_vec(),
        };
        let limits = ConfigLimits::default();
        parse_with_environment(&schema, &source, &mut provider, &limits).test_value()?
    };
    let ValueKind::Structure(fields) = value.as_ref() else {
        return Err(TestFailure::new(
            "configuration root node should be a struct",
        ));
    };
    for (field, expected) in fields.into_iter().zip([
        "confidential-literal",
        "confidential-environment",
        "confidential-fallback",
        "confidential-default",
    ]) {
        test_assert!(
            matches!(field.value().as_ref(), ValueKind::String(text) if text == expected)
        )?;
    }
    test_assert!(!format!("{value:?}").contains("confidential"))?;

    Ok(())
}

#[test]
fn secret_errors_share_budget_and_fatal_paths_are_safe() -> TestResult<()> {
    let schema =
        ParserTestSupport::compile("entry struct Root { value: map<string, u8> @secret, }")?;
    for budget in [0, 1, 2, 4] {
        let mut limits = ConfigLimits::default();
        limits = limits.with_max_diagnostics(budget);
        let errors = run(
            &schema,
            r#"{ value: { "confidential-a": 999, "confidential-b": 999 } }"#,
            None,
            &limits,
        )
        .test_error()?;
        test_eq!(errors.len(), if budget <= 1 { 1 } else { 2 })?;
        test_assert!(
            errors
                .iter()
                .all(|error| error.path().is_none_or(|path| path.len() == 1))
        )?;
        test_assert!(!format!("{errors:?} {errors}").contains("confidential"))?;
        if budget <= 1 {
            test_eq!(
                errors.last().test_value()?.code(),
                DiagnosticCode::CONFIG_VALUE_ERROR
            )?;
            test_assert!(errors.is_truncated())?;
        } else if budget == 2 {
            test_eq!(
                errors.last().test_value()?.code(),
                DiagnosticCode::DIAGNOSTIC_LIMIT_REACHED
            )?;
            test_assert!(errors.is_truncated())?;
        } else {
            test_assert!(!errors.is_truncated())?;
        }
    }
    let mut limits = ConfigLimits::default();
    limits = limits.with_max_value_nodes(1);
    let errors = run(
        &schema,
        r#"{ value: { "confidential-key": 1 } }"#,
        None,
        &limits,
    )
    .test_error()?;
    test_eq!(
        errors.last().test_value()?.code(),
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
    )?;
    test_eq!(errors.last().test_value()?.message(), "value is hidden")?;
    test_assert!(
        errors
            .iter()
            .all(|error| error.path().is_none_or(|path| path.len() <= 1))
    )?;

    Ok(())
}

#[test]
fn secret_single_payload_assertions_hide_messages_without_a_payload_path() -> TestResult<()> {
    for payload in ["u8 @secret", "Secret"] {
        let schema = ParserTestSupport::compile(&format!(
            "type Secret = u8 @secret; enum E {{ Single({payload}) @assert(self > 1, \"payload rule\"), }} entry struct Root {{ value: E, }}"
        ))?;
        let errors = parse(&schema, "{ value: .Single(1) }").test_error()?;
        assert_hidden(&errors, &[1])?;
        test_eq!(
            TestCheck::item(errors.diagnostics(), 0)?.value_source(),
            Some(ValueSource::ConfigLiteral)
        )?;
    }

    Ok(())
}

fn session_options(limits: &ConfigLimits) -> ConfigParseOptions {
    ConfigParseOptions::new(
        ResourceLimits::default(),
        *limits,
        ExecutionLimits::new(1_000_000_000, 268_435_456),
    )
}
