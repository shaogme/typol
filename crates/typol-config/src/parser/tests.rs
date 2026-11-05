mod acceptance;
mod collection;
mod environment;
mod execution;
mod foundation;
mod indexes;
mod scalar;
mod secret;

use self::foundation::Provider;
use super::{
    Parser,
    frames::ValueParser,
    recovery::{ParseFailure, Recovery},
};
use crate::{
    ConfigLimits, ConfigParseOptions, ConfigParseRequest, ConfigParser,
    diagnostics::ConfigParseErrors,
    environment::Environment,
    instance::LocationIndex,
    parser::context::{ParserControl, ParserControlRequest},
    session::ConfigSession,
};
use alloc::{format, vec, vec::Vec};
use opeo::{OResult, Out};
use typol_core::{
    ConfigValue, Diagnostic, DiagnosticCode, ExecutionLimits, Read, SliceSource, ValueKind,
    ValueSource,
};
use typol_policy::ResourceLimits;
use typol_schema::{
    ResolverError, SchemaCompileErrors, SchemaCompileOptions, SchemaCompiler, SchemaResolveRequest,
    SchemaResolver, SchemaSource,
};
use typol_semantic::{CompiledSchema, SchemaAccessError};
use typol_test_support::*;

struct NodeLimitCase {
    schema: &'static str,
    source: &'static str,
    nodes: usize,
}

struct NestingCase {
    schema: &'static str,
    source: &'static str,
    depth: usize,
}

fn access<T>(
    errors: &mut TestSlot<SchemaAccessError>,
    operation: impl for<'slot> FnOnce(Out<'slot, 'slot, SchemaAccessError>) -> OResult<'slot, T>,
) -> TestResult<T> {
    errors
        .run(operation)
        .test_value()
        .map_err(|_| TestFailure::new("schema access failed"))
}

fn present<T>(value: Option<T>) -> TestResult<T> {
    value.test_expect("schema is missing the expected member")
}

fn required<T>(
    errors: &mut TestSlot<SchemaAccessError>,
    operation: impl for<'slot> FnOnce(Out<'slot, 'slot, SchemaAccessError>) -> OResult<'slot, Option<T>>,
) -> TestResult<T> {
    present(access(errors, operation)?)
}

struct NoImports;

impl SchemaResolver for NoImports {
    fn resolve<'a, 'slot, 'borrow>(
        &'a mut self,
        _request: SchemaResolveRequest<'_>,
        out: Out<'slot, 'borrow, ResolverError>,
    ) -> OResult<'slot, SchemaSource<'a>> {
        Err(out.fail(ResolverError::NotFound))
    }
}

fn compile(text: &str) -> TestResult<CompiledSchema> {
    let mut errors = TestSlot::<SchemaCompileErrors>::default();
    match errors.run(|mut out| {
        let compiler = SchemaCompiler::new(SchemaCompileOptions::default(), out.reborrow())?;
        compiler.compile(
            SchemaSource::from_str(b"root", text),
            &mut NoImports,
            None,
            out.reborrow(),
        )
    }) {
        TestOpeoOutcome::Value(schema) => Ok(schema),
        TestOpeoOutcome::Error(errors) => Err(TestFailure::new(format!(
            "test schema compilation failed: {:?}",
            errors
                .diagnostics()
                .iter()
                .map(|error| (error.code(), error.message()))
                .collect::<Vec<_>>()
        ))),
    }
}

#[derive(Clone, Copy)]
pub struct ParserLimits {
    resources: ResourceLimits,
    config: ConfigLimits,
}

impl From<&ResourceLimits> for ParserLimits {
    fn from(resources: &ResourceLimits) -> Self {
        Self {
            resources: *resources,
            config: ConfigLimits::default()
                .with_max_environment_entries(100_000)
                .with_max_environment_bytes(268_435_456)
                .with_max_constructed_text_bytes(268_435_456)
                .with_max_display_name_bytes(1_048_576),
        }
    }
}

impl From<&ConfigLimits> for ParserLimits {
    fn from(config: &ConfigLimits) -> Self {
        Self {
            resources: ResourceLimits::default(),
            config: *config,
        }
    }
}

impl From<&(&ResourceLimits, &ConfigLimits)> for ParserLimits {
    fn from(limits: &(&ResourceLimits, &ConfigLimits)) -> Self {
        Self {
            resources: *limits.0,
            config: *limits.1,
        }
    }
}

impl ParserLimits {
    fn resources(&self) -> ResourceLimits {
        self.resources
    }

    fn config(&self) -> ConfigLimits {
        self.config
    }
}

fn parse(
    schema: &CompiledSchema,
    source: &str,
    limits: impl Into<ParserLimits>,
) -> TestOpeoOutcome<ConfigValue, ConfigParseErrors> {
    let mut errors = TestSlot::default();
    match errors.run(|out| {
        ConfigParser::new(schema).parse(
            config_request((&mut SliceSource::new(source.as_bytes()), "test"), limits),
            &mut Provider::new(Ok(None)),
            out,
        )
    }) {
        TestOpeoOutcome::Value(value) => TestOpeoOutcome::Value(value.into_config_value()),
        TestOpeoOutcome::Error(error) => TestOpeoOutcome::Error(error),
    }
}

fn codes(errors: &[Diagnostic]) -> Vec<u32> {
    errors.iter().map(|error| error.code().get()).collect()
}

#[test]
fn independent_fields_recover_without_duplicate_missing_errors() -> TestResult<()> {
    let schema = compile("entry struct Root { a: u8, b: bool, c: u8, }")?;
    let source = "{ a: 256, b: 1, c: 2 }";
    let errors = parse(&schema, source, &ConfigLimits::default()).test_error()?;
    test_eq!(codes(&errors), [11, 11])?;
    test_eq!(
        TestCheck::item(errors.diagnostics(), 0)?
            .primary()
            .test_value()?
            .span()
            .start(),
        source.find("256").test_value()?
    )?;
    test_eq!(
        TestCheck::item(
            TestCheck::item(errors.diagnostics(), 1)?
                .path()
                .test_value()?
                .segments(),
            0
        )?
        .as_field(),
        Some("b")
    )?;

    Ok(())
}

#[test]
fn missing_fields_are_reported_only_after_normal_close() -> TestResult<()> {
    let schema = compile("entry struct Root { a: u8, b: bool, }")?;
    let errors = parse(&schema, "{}", &ConfigLimits::default()).test_error()?;
    test_eq!(codes(&errors), [11, 11])?;
    test_eq!(
        TestCheck::item(
            TestCheck::item(errors.diagnostics(), 0)?
                .path()
                .test_value()?
                .segments(),
            0
        )?
        .as_field(),
        Some("a")
    )?;
    test_eq!(
        TestCheck::item(
            TestCheck::item(errors.diagnostics(), 1)?
                .path()
                .test_value()?
                .segments(),
            0
        )?
        .as_field(),
        Some("b")
    )?;
    test_assert!(
        errors
            .diagnostics()
            .iter()
            .all(|error| error.value_source().is_none())
    )?;
    for source in ["{", "{ a: 2", "{ a: 2 ]"] {
        let errors = parse(&schema, source, &ConfigLimits::default()).test_error()?;
        test_assert!(
            errors
                .iter()
                .all(|error| error.code() == DiagnosticCode::CONFIG_SYNTAX_ERROR)
        )?;
    }

    Ok(())
}

#[test]
fn recovery_skips_nested_wrong_values_and_preserves_source_order() -> TestResult<()> {
    let schema = compile("entry struct Root { a: u8, b: bool, c: u8, }")?;
    let source = "{ a: { unknown: [1, 2, { x: 3 }] }, b: 7, c: 999 }";
    let errors = parse(&schema, source, &ConfigLimits::default()).test_error()?;
    test_eq!(codes(&errors), [11, 11, 11])?;
    let positions: Vec<_> = errors
        .diagnostics()
        .iter()
        .map(|error| error.primary().map(|span| span.span().start()).test_value())
        .collect::<TestResult<Vec<_>>>()?;
    test_assert!(positions.windows(2).all(|pair| {
        pair.first()
            .zip(pair.get(1))
            .is_some_and(|(left, right)| left < right)
    }))?;

    Ok(())
}

#[test]
fn nested_container_failure_propagates_without_partial_values() -> TestResult<()> {
    let schema =
        compile("struct Child { a: u8, b: bool, } entry struct Root { child: Child, c: u8, }")?;
    let errors = parse(
        &schema,
        "{ child: { a: 999, b: 1 }, c: 999 }",
        &ConfigLimits::default(),
    )
    .test_error()?;
    test_eq!(codes(&errors), [11, 11, 11])?;

    Ok(())
}

#[test]
fn mismatched_delimiter_stops_recovery() -> TestResult<()> {
    let schema = compile("entry struct Root { a: u8, b: bool, }")?;
    let source = "{ a: [1, 2 }, b: 9 }";
    let errors = parse(&schema, source, &ConfigLimits::default()).test_error()?;
    test_eq!(codes(&errors), [11, 10])?;
    let later_field = source.find("b:").test_value()?;
    for error in errors.diagnostics().iter() {
        test_assert!(error.primary().test_value()?.span().start() < later_field)?;
    }

    Ok(())
}

#[test]
fn unknown_and_duplicate_fields_can_recover() -> TestResult<()> {
    let schema = compile("entry struct Root { a: u8, b: bool, }")?;
    let errors = parse(
        &schema,
        "{ x: { y: [1, 2] }, a: 999, a: 1, b: 1 }",
        &ConfigLimits::default(),
    )
    .test_error()?;
    test_eq!(codes(&errors), [11, 11, 11, 11])?;

    Ok(())
}

#[test]
fn array_paths_count_failed_items() -> TestResult<()> {
    let schema = compile("entry struct Root { a: [u8], }")?;
    let errors = parse(&schema, "{ a: [999, 1, 999] }", &ConfigLimits::default()).test_error()?;
    test_eq!(codes(&errors), [11, 11])?;
    test_eq!(
        TestCheck::item(
            TestCheck::item(errors.diagnostics(), 0)?
                .path()
                .test_value()?
                .segments(),
            1
        )?
        .as_index(),
        Some(0)
    )?;
    test_eq!(
        TestCheck::item(
            TestCheck::item(errors.diagnostics(), 1)?
                .path()
                .test_value()?
                .segments(),
            1
        )?
        .as_index(),
        Some(2)
    )?;

    Ok(())
}

#[test]
fn map_errors_follow_source_order_and_track_failed_keys() -> TestResult<()> {
    let schema = compile("entry struct Root { m: map<string, u8>, }")?;
    let source = r#"{ m: { "z\"": 999, "a": 999, "z\"": 1 } }"#;
    let errors = parse(&schema, source, &ConfigLimits::default()).test_error()?;
    test_eq!(codes(&errors), [11, 11, 11])?;
    test_eq!(
        TestCheck::item(
            TestCheck::item(errors.diagnostics(), 0)?
                .path()
                .test_value()?
                .segments(),
            1
        )?
        .as_map_key(),
        Some("z\"")
    )?;
    test_eq!(
        TestCheck::item(
            TestCheck::item(errors.diagnostics(), 1)?
                .path()
                .test_value()?
                .segments(),
            1
        )?
        .as_map_key(),
        Some("a")
    )?;
    test_eq!(
        TestCheck::item(
            TestCheck::item(errors.diagnostics(), 2)?
                .path()
                .test_value()?
                .segments(),
            1
        )?
        .as_map_key(),
        Some("z\"")
    )?;

    Ok(())
}

#[test]
fn a_single_diagnostic_slot_preserves_the_first_error() -> TestResult<()> {
    let schema = compile("entry struct Root { a: u8, b: bool, }")?;
    for (budget, expected) in [
        (0, vec![11]),
        (1, vec![11]),
        (2, vec![11, 5]),
        (3, vec![11, 3]),
    ] {
        let mut limits = ConfigLimits::default();
        limits = limits.with_max_diagnostics(budget);
        let errors = parse(&schema, "{ a: 999, b: ` }", &limits).test_error()?;
        test_eq!(codes(&errors), expected)?;
        test_eq!(errors.is_truncated(), budget <= 2)?;
        if budget <= 1 {
            test_assert!(format!("{errors}").contains("subsequent errors were not collected"))?;
        }
    }

    Ok(())
}

#[test]
fn diagnostic_limit_stops_before_later_lexer_error() -> TestResult<()> {
    let schema = compile("entry struct Root { a: [u8], }")?;
    let mut limits = ConfigLimits::default();
    limits = limits.with_max_diagnostics(3);
    let errors = parse(&schema, "{ a: [999, 999, 999, `] }", &limits).test_error()?;
    test_eq!(codes(&errors), [11, 11, 5])?;

    Ok(())
}

#[test]
fn resource_failure_is_fatal_and_shares_the_budget() -> TestResult<()> {
    let schema = compile("entry struct Root { a: u8, b: [u8], }")?;
    let mut limits = ConfigLimits::default();
    limits = limits.with_max_value_nodes(2);
    let errors = parse(&schema, "{ a: 999, b: [999] }", &limits).test_error()?;
    test_eq!(codes(&errors), [11, 1])?;
    limits = limits.with_max_diagnostics(2);
    let errors = parse(&schema, "{ a: 999, b: [999] }", &limits).test_error()?;
    test_eq!(codes(&errors), [11, 1])?;

    Ok(())
}

#[test]
fn omitted_optional_and_map_keys_count_as_nodes() -> TestResult<()> {
    for case in [
        NodeLimitCase {
            schema: "entry struct Root { a: option<u8>, }",
            source: "{}",
            nodes: 2,
        },
        NodeLimitCase {
            schema: "entry struct Root { m: map<string, u8>, }",
            source: r#"{ m: { "x": 1 } }"#,
            nodes: 4,
        },
        NodeLimitCase {
            schema: "entry struct Root { a: option<u8>, }",
            source: "{ a: 1 }",
            nodes: 3,
        },
        NodeLimitCase {
            schema: "entry struct Root { a: [u8] = [1, 2], }",
            source: "{}",
            nodes: 4,
        },
    ] {
        let schema = compile(case.schema)?;
        let mut limits = ConfigLimits::default();
        limits = limits.with_max_value_nodes(case.nodes);
        test_assert!(parse(&schema, case.source, &limits).is_ok())?;
        limits = limits.with_max_value_nodes(case.nodes - 1);
        let errors = parse(&schema, case.source, &limits).test_error()?;
        test_eq!(codes(&errors), [1])?;
    }

    Ok(())
}

#[test]
fn nesting_counts_optional_wrappers_and_default_containers() -> TestResult<()> {
    for case in [
        NestingCase {
            schema: "entry struct Root { a: option<[u8]>, }",
            source: "{ a: [1] }",
            depth: 3,
        },
        NestingCase {
            schema: "entry struct Root { a: [[u8]] = [[1]], }",
            source: "{}",
            depth: 3,
        },
        NestingCase {
            schema: "enum E { S { a: [u8], }, } entry struct Root { e: E, }",
            source: "{ e: .S { a: [1] } }",
            depth: 4,
        },
    ] {
        let schema = compile(case.schema)?;
        let mut limits = ConfigLimits::default();
        limits = limits.with_max_nesting_depth(case.depth);
        test_assert!(parse(&schema, case.source, &limits).is_ok())?;
        limits = limits.with_max_nesting_depth(case.depth - 1);
        test_eq!(
            codes(&parse(&schema, case.source, &limits).test_error()?),
            [1]
        )?;
    }

    Ok(())
}

#[test]
fn recovery_scanning_also_obeys_nesting_limit() -> TestResult<()> {
    let schema = compile("entry struct Root { a: u8, b: u8, }")?;
    let mut limits = ConfigLimits::default();
    limits = limits.with_max_nesting_depth(2);
    let errors = parse(&schema, "{ a: [[[1]]], b: 999 }", &limits).test_error()?;
    test_eq!(codes(&errors), [11, 1])?;

    Ok(())
}

#[test]
fn source_and_string_limits_apply_before_value_publication() -> TestResult<()> {
    let schema = compile("entry struct Root { name: string = \"long\", }")?;
    let mut limits = ConfigLimits::default();
    limits = limits.with_max_source_bytes(1);
    test_eq!(codes(&parse(&schema, "{}", &limits).test_error()?), [1])?;
    limits = limits.with_max_diagnostics(1);
    test_eq!(codes(&parse(&schema, "{}", &limits).test_error()?), [1])?;
    limits = ConfigLimits::default();
    limits = limits.with_max_string_bytes(3);
    for source in ["{}", "{ name: \"x\" }"] {
        test_eq!(codes(&parse(&schema, source, &limits).test_error()?), [1])?;
    }
    let enum_schema = compile("enum E { Long, } entry struct Root { e: E, }")?;
    for source in ["{ e: .Long }", "{ e: .Unknown }"] {
        test_eq!(
            codes(&parse(&enum_schema, source, &limits).test_error()?),
            [1]
        )?;
    }
    let schema = compile("entry struct Root { m: map<string, string>, }")?;
    for source in [r#"{ m: { "long": "x" } }"#, r#"{ m: { "x": "long" } }"#] {
        test_eq!(codes(&parse(&schema, source, &limits).test_error()?), [1])?;
    }

    Ok(())
}

#[test]
fn invalid_environment_position_and_fallback_do_not_call_provider() -> TestResult<()> {
    let schema = compile("entry struct Root { port: u8 @env(\"PORT\"), a: [u8] }")?;
    let mut provider = Provider::new(Ok(Some(b"7")));
    let mut parse_errors = TestSlot::default();
    for source in [
        "{ port: env(\"PORT\", none), a: [] }",
        "{ port: env(\"PORT\", 999), a: [] }",
        "{ port: env(\"PORT\", 1, 2), a: [] }",
        "{ port: 1, a: [env(\"PORT\")] }",
    ] {
        test_assert!(
            parse_errors
                .run(|out| {
                    ConfigParser::new(&schema).parse(
                        config_request(
                            (&mut SliceSource::new(source.as_bytes()), "test"),
                            &ConfigLimits::default(),
                        ),
                        &mut provider,
                        out,
                    )
                })
                .is_err()
        )?;
    }
    test_eq!(provider.calls(), 0)?;

    Ok(())
}

#[test]
fn validation_errors_recover_and_skip_root_assertions() -> TestResult<()> {
    let schema =
        compile(r#"entry struct Root { a: u8 @min(3), b: u8 @min(3) } @assert(false, "root")"#)?;
    let errors = parse(&schema, "{a: 1, b: 2}", &ConfigLimits::default()).test_error()?;
    test_eq!(codes(&errors), [12, 12])?;
    test_assert!(errors.iter().all(|error| error.message() != "root"))?;

    Ok(())
}

#[test]
fn enum_payload_failure_recovery_keeps_metadata() -> TestResult<()> {
    let schema =
        compile("enum E { One(u8 @min(3)), Obj { x: bool } } entry struct Root { e: E, n: u8 }")?;
    let errors = parse(&schema, "{e: .One(2), n: 1}", &ConfigLimits::default()).test_error()?;
    test_eq!(codes(&errors), [12])?;
    test_eq!(
        TestCheck::item(
            TestCheck::item(errors.diagnostics(), 0)?
                .path()
                .test_value()?
                .segments(),
            1
        )?
        .as_enum_payload(),
        Some("One")
    )?;
    for source in ["{e: .One(999), n: 999}", "{e: .Obj {x: 1}, n: 999}"] {
        test_eq!(
            codes(&parse(&schema, source, &ConfigLimits::default()).test_error()?),
            [11, 11]
        )?;
    }

    Ok(())
}

#[test]
fn diagnostics_and_debug_do_not_contain_failed_text() -> TestResult<()> {
    let schema = compile("entry struct Root { a: u8 @secret, }")?;
    let errors = parse(
        &schema,
        "{ a: \"secret-content\" }",
        &ConfigLimits::default(),
    )
    .test_error()?;
    test_assert!(
        errors
            .iter()
            .all(|error| !error.message().contains("secret-content"))
    )?;
    test_assert!(!format!("{errors:?}").contains("secret-content"))?;

    Ok(())
}

#[test]
fn missing_environment_defers_to_object_default_optional_and_required_rules() -> TestResult<()> {
    let limits = ConfigLimits::default();
    for (text, expected_default) in [
        (r#"entry struct Root { port: u8 @env("PORT") = 2 }"#, true),
        (
            r#"entry struct Root { port: option<u8> @env("PORT") }"#,
            false,
        ),
    ] {
        let schema = compile(text)?;
        let value = parse(&schema, r#"{port: env("PORT")}"#, &limits).test_value()?;
        let ValueKind::Structure(mut fields) = value.as_ref() else {
            return Err(TestFailure::new("root value should be a struct"));
        };
        let field = fields.next().test_value()?;
        if expected_default {
            test_assert!(matches!(field.value().as_ref(), ValueKind::Unsigned(2)))?;
            test_eq!(field.value().source(), ValueSource::SchemaDefault)?;
        } else {
            test_assert!(matches!(field.value().as_ref(), ValueKind::Optional(None)))?;
        }
    }
    let schema = compile(r#"entry struct Root { port: u8 @env("PORT") }"#)?;
    let source = r#"{port: env("PORT")}"#;
    let errors = parse(&schema, source, &limits).test_error()?;
    test_eq!(codes(&errors), [11])?;
    test_eq!(
        TestCheck::item(errors.diagnostics(), 0)?
            .primary()
            .test_value()?
            .span()
            .start(),
        source.len() - 1
    )?;
    test_assert!(
        TestCheck::item(errors.diagnostics(), 0)?
            .value_source()
            .is_none()
    )?;
    test_eq!(
        codes(&parse(&schema, r#"{port: env("PORT"), port: 2}"#, &limits).test_error()?),
        [11, 11]
    )?;

    Ok(())
}

#[test]
fn enum_default_and_explicit_structure_share_depth_accounting() -> TestResult<()> {
    let schema =
        compile("enum E { Obj { a: [u8], }, } entry struct Root { e: E = .Obj { a: [1] }, }")?;
    let mut limits = ConfigLimits::default();
    limits = limits.with_max_nesting_depth(4);
    for source in ["{}", "{ e: .Obj { a: [1] } }"] {
        if let TestOpeoOutcome::Error(errors) = parse(&schema, source, &limits) {
            return Err(TestFailure::new(format!("{source}: {errors}")));
        }
    }
    limits = limits.with_max_nesting_depth(3);
    for source in ["{}", "{ e: .Obj { a: [1] } }"] {
        test_eq!(codes(&parse(&schema, source, &limits).test_error()?), [1])?;
    }

    Ok(())
}

#[test]
fn lexer_failure_during_recovery_stops_the_call() -> TestResult<()> {
    let schema = compile("entry struct Root { a: u8, b: u8, }")?;
    let errors = parse(&schema, "{ a: [1, `], b: 999 }", &ConfigLimits::default()).test_error()?;
    test_eq!(codes(&errors), [11, 3])?;

    Ok(())
}

fn parse_local_value<'schema>(
    schema: &'schema CompiledSchema,
    source: &str,
    provider: &mut dyn Environment,
    limits: impl Into<ParserLimits>,
) -> TestResult<(ConfigValue, LocationIndex<'schema>)> {
    let limits = limits.into();
    let options = session_options(limits);
    let config_limits = limits.config();
    let mut budget_errors = TestSlot::default();
    let mut session_errors = TestSlot::default();
    let session = session_errors
        .run(|out| ConfigSession::new(provider, &options, budget_errors.slot(), out))
        .test_value()?;
    let source_id = schema.next_source_id().test_value()?;
    let mut parser_input_1 = SliceSource::new(source.as_bytes());
    let mut context_errors = TestSlot::default();
    let mut context = context_errors
        .run(|out| {
            ParserControl::create(
                ParserControlRequest {
                    schema,
                    source: &mut parser_input_1,
                    source_id,
                    limits: &config_limits,
                    session,
                },
                out,
            )
        })
        .test_value()?;
    let mut parse_errors = TestSlot::<ParseFailure>::default();
    let mut frame_errors = TestSlot::default();
    let mut item_errors = TestSlot::default();
    match parse_errors.run(|out| {
        ValueParser::parse(
            &mut context,
            schema.entry(),
            frame_errors.slot(),
            item_errors.slot(),
            out,
        )
    }) {
        TestOpeoOutcome::Value(parsed) => Ok((parsed.value, context.locations)),
        TestOpeoOutcome::Error(failure) => {
            Recovery::record_failure(&mut context, failure);
            let errors =
                ConfigParseErrors::from_buffer(context.diagnostics, schema, source_id, None);
            Err(TestFailure::new(format!(
                "local test parsing failed: {errors:?}"
            )))
        }
    }
}

fn config_request<'a>(
    source: (&'a mut impl Read, &'a str),
    limits: impl Into<ParserLimits>,
) -> ConfigParseRequest<'a> {
    let limits = limits.into();
    ConfigParseRequest::new(
        source.0,
        source.1,
        ConfigParseOptions::new(
            limits.resources(),
            limits.config(),
            ExecutionLimits::new(1_000_000_000, 268_435_456),
        ),
    )
}

fn session_options(limits: impl Into<ParserLimits>) -> ConfigParseOptions {
    let limits = limits.into();
    ConfigParseOptions::new(
        limits.resources(),
        limits.config(),
        ExecutionLimits::new(1_000_000_000, 268_435_456),
    )
}

pub struct ParserTestSupport;

impl ParserTestSupport {
    pub fn access<T>(
        errors: &mut TestSlot<SchemaAccessError>,
        operation: impl for<'slot> FnOnce(Out<'slot, 'slot, SchemaAccessError>) -> OResult<'slot, T>,
    ) -> TestResult<T> {
        access(errors, operation)
    }

    pub fn required<T>(
        errors: &mut TestSlot<SchemaAccessError>,
        operation: impl for<'slot> FnOnce(
            Out<'slot, 'slot, SchemaAccessError>,
        ) -> OResult<'slot, Option<T>>,
    ) -> TestResult<T> {
        required(errors, operation)
    }

    pub fn compile(text: &str) -> TestResult<CompiledSchema> {
        compile(text)
    }

    pub fn parse(
        schema: &CompiledSchema,
        source: &str,
        limits: impl Into<ParserLimits>,
    ) -> TestOpeoOutcome<ConfigValue, ConfigParseErrors> {
        parse(schema, source, limits)
    }

    pub fn parse_local_value<'schema>(
        schema: &'schema CompiledSchema,
        source: &str,
        provider: &mut dyn Environment,
        limits: impl Into<ParserLimits>,
    ) -> TestResult<(ConfigValue, LocationIndex<'schema>)> {
        parse_local_value(schema, source, provider, limits)
    }

    pub fn codes(errors: &[Diagnostic]) -> Vec<u32> {
        codes(errors)
    }

    pub fn config_request<'a>(
        source: (&'a mut impl Read, &'a str),
        limits: impl Into<ParserLimits>,
    ) -> ConfigParseRequest<'a> {
        config_request(source, limits)
    }
}
