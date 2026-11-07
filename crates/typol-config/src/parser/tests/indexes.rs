use super::{ConfigLimits, ParserTestSupport, foundation::Provider};
use crate::{
    ConfigParseOptions,
    environment::{Environment, EnvironmentError, EnvironmentReadContext},
    instance::InstanceWalk,
    parser::{
        context::{ParserControl, ParserControlRequest},
        frames::ValueParser,
        recovery::ParseFailure,
    },
    session::ConfigSession,
};
use alloc::{format, string::String, vec::Vec};
use opeo::{ErrSlot, OResult, Out};
use typol_core::{BudgetError, Diagnostic, ExecutionLimits, PathSegment, SliceSource, ValueKind};
use typol_policy::{ExcerptLimits, RegexOptions};
use typol_semantic::{SchemaAccessError, TypeKind};
use typol_test_support::*;

struct CountingProvider {
    names: Vec<String>,
}

impl Environment for CountingProvider {
    fn read<'env, 'slot, 'borrow>(
        &'env mut self,
        name: &str,
        _: EnvironmentReadContext<'_>,
        _: Out<'slot, 'borrow, EnvironmentError>,
    ) -> OResult<'slot, Option<&'env [u8]>> {
        self.names.push(String::from(name));
        Ok(Some(b"7"))
    }
}

#[test]
fn sorted_field_and_environment_indexes_preserve_declaration_and_input_order() -> TestResult<()> {
    let mut fields = String::new();
    let mut source = String::from("{");
    for index in 0..256 {
        fields.push_str(&format!("field{index}: u8 @env(\"E{index}\"),"));
    }
    fields.push_str("again: u8 @env(\"E255\")");
    for index in (0..256).rev() {
        source.push_str(&format!("field{index}: env(\"E{index}\"),"));
    }
    source.push_str("again: env(\"E255\")}");
    let schema = ParserTestSupport::compile(&format!("entry struct Root {{{fields}}}"))?;
    let mut provider = CountingProvider { names: Vec::new() };
    let (root, locations) = ParserTestSupport::parse_local_value(
        &schema,
        &source,
        &mut provider,
        &ConfigLimits::default(),
    )
    .test_value()?;
    let mut schema_errors = TestSlot::<SchemaAccessError>::default();
    let ValueKind::Structure(values) = root.as_ref() else {
        return Err(TestFailure::new("root should be a struct"));
    };
    for (index, value) in values.into_iter().take(256).enumerate() {
        let name = format!("field{index}");
        let fields =
            ParserTestSupport::access(&mut schema_errors, |out| schema.entry().fields(out))?;
        let (slot, field) =
            ParserTestSupport::required(&mut schema_errors, |out| fields.named(&name, out))?;
        test_eq!(slot, index)?;
        test_eq!(
            ParserTestSupport::access(&mut schema_errors, |out| field.slot(out))?,
            index
        )?;
        test_eq!(value.name(), Some(name.as_str()))?;
        test_assert!(matches!(value.value().as_ref(), ValueKind::Unsigned(7)))?;
    }
    let fields = ParserTestSupport::access(&mut schema_errors, |out| schema.entry().fields(out))?;
    let missing =
        ParserTestSupport::access(&mut schema_errors, |out| fields.named("missing", out))?;
    test_assert!(missing.is_none())?;
    test_eq!(provider.names.len(), 256)?;
    test_eq!(provider.names.first().test_value()?, "E255")?;
    let first_field = ParserTestSupport::required(&mut schema_errors, |out| {
        schema.entry().field_named("field255", out)
    })?;
    let first =
        ParserTestSupport::required(&mut schema_errors, |out| first_field.environment(out))?;
    let again_field = ParserTestSupport::required(&mut schema_errors, |out| {
        schema.entry().field_named("again", out)
    })?;
    let again =
        ParserTestSupport::required(&mut schema_errors, |out| again_field.environment(out))?;
    test_eq!(first.slot(), again.slot())?;
    test_eq!(first.slot_count(), 256)?;
    let mut walk_errors = TestSlot::<Diagnostic>::default();
    let mut walk = walk_errors
        .run(|out| InstanceWalk::new(&locations, &schema, &root, 16, out))
        .test_value()?;
    test_assert!(
        walk_errors
            .run(|out| walk.next(out))
            .test_value()?
            .is_some()
    )?;
    for index in (0..256).rev() {
        test_assert!(
            walk_errors
                .run(|out| walk.next(out))
                .test_value()?
                .is_some()
        )?;
        test_eq!(
            TestCheck::item(
                walk_errors
                    .run(|out| walk.copy_path(None, None, out))
                    .test_value()?
                    .segments(),
                0
            )?
            .as_field(),
            Some(format!("field{index}").as_str())
        )?;
    }
    test_assert!(
        walk_errors
            .run(|out| walk.next(out))
            .test_value()?
            .is_some()
    )?;
    test_eq!(
        TestCheck::item(
            walk_errors
                .run(|out| walk.copy_path(None, None, out))
                .test_value()?
                .segments(),
            0
        )?
        .as_field(),
        Some("again")
    )?;
    test_assert!(
        walk_errors
            .run(|out| walk.next(out))
            .test_value()?
            .is_none()
    )?;

    Ok(())
}

#[test]
fn direct_collection_slots_preserve_holes_and_incomplete_members() -> TestResult<()> {
    let schema = ParserTestSupport::compile("entry struct Root { values: [u8] }")?;
    let limits = ConfigLimits::default();
    let mut provider = Provider::new(Ok(None));
    let options = session_options(&limits);
    let mut budget_errors = ErrSlot::<BudgetError>::new();
    let mut session_errors = TestSlot::<Diagnostic>::default();
    let session = session_errors
        .run(|out| ConfigSession::new(&mut provider, &options, &mut budget_errors, out))
        .test_value()?;
    let mut parser_input_1 = SliceSource::new(("{values: [999, 1, 999, 2]}").as_bytes());
    let source_id = schema.next_source_id().test_value()?;
    let mut context_errors = TestSlot::<Diagnostic>::default();
    let mut context = context_errors
        .run(|out| {
            ParserControl::create(
                ParserControlRequest {
                    schema: &schema,
                    source: &mut parser_input_1,
                    source_id,
                    limits: &limits,
                    session,
                },
                out,
            )
        })
        .test_value()?;
    let mut frame_errors = ErrSlot::<ParseFailure>::new();
    let mut item_errors = ErrSlot::<ParseFailure>::new();
    let mut parse_errors = TestSlot::<ParseFailure>::default();
    test_assert!(
        parse_errors
            .run(|out| {
                ValueParser::parse(
                    &mut context,
                    schema.entry(),
                    &mut frame_errors,
                    &mut item_errors,
                    out,
                )
            })
            .is_err()
    )?;
    let mut access_errors = TestSlot::<SchemaAccessError>::default();
    let mut parent = None;
    for (id, node) in context.locations.nodes() {
        if matches!(
            access_errors.run(|out| node.expected().kind(out)).value(),
            Some(TypeKind::Array { .. })
        ) {
            parent = Some(id);
            break;
        }
    }
    let parent = parent.test_value()?;
    test_assert!(context.locations.item(parent, 0).is_none())?;
    test_assert!(context.locations.item(parent, 2).is_none())?;
    test_assert!(context.locations.item(parent, 4).is_none())?;
    for index in [1, 3] {
        test_assert!(
            context
                .locations
                .item(parent, index)
                .test_value()?
                .complete()
        )?;
    }

    Ok(())
}

#[test]
fn wide_defaults_use_depth_bounded_continuations_and_visit_each_instance_once() -> TestResult<()> {
    let mut peaks = Vec::new();
    let mut shape_skips = Vec::new();
    let mut budget_errors = ErrSlot::<BudgetError>::new();
    let mut session_errors = TestSlot::<Diagnostic>::default();
    let mut context_errors = TestSlot::<Diagnostic>::default();
    let mut parse_errors = TestSlot::<ParseFailure>::default();
    let mut frame_errors = ErrSlot::<ParseFailure>::new();
    let mut item_errors = ErrSlot::<ParseFailure>::new();
    let mut walk_errors = TestSlot::<Diagnostic>::default();
    for width in [64, 512] {
        let array = (0..width)
            .map(|_| "{b: 2, a: 1}")
            .collect::<Vec<_>>()
            .join(",");
        let map = (0..width)
            .map(|index| format!("\"k{index}\": .Obj {{b: 2, a: 1}}"))
            .collect::<Vec<_>>()
            .join(",");
        let schema = ParserTestSupport::compile(&format!(
            "struct Child {{a: u8, b: u8}} enum E {{ Obj {{a: u8, b: u8}} }} entry struct Root {{array: [option<Child>] = [{array}], mapping: map<string, E> = {{{map}}}}}"
        ))?;
        let limits = ConfigLimits::default();
        let mut provider = Provider::new(Ok(None));
        let options = session_options(&limits);
        let session = session_errors
            .run(|out| ConfigSession::new(&mut provider, &options, &mut budget_errors, out))
            .test_value()?;
        let mut parser_input_2 = SliceSource::new(("{}").as_bytes());
        let source_id = schema.next_source_id().test_value()?;
        let mut context = context_errors
            .run(|out| {
                ParserControl::create(
                    ParserControlRequest {
                        schema: &schema,
                        source: &mut parser_input_2,
                        source_id,
                        limits: &limits,
                        session,
                    },
                    out,
                )
            })
            .test_value()?;
        let root = parse_errors
            .run(|out| {
                ValueParser::parse(
                    &mut context,
                    schema.entry(),
                    &mut frame_errors,
                    &mut item_errors,
                    out,
                )
            })
            .test_value()?
            .value;
        peaks.push(context.default_work_peak);
        shape_skips.push(context.subtree_shape_skips);
        let mut walk = walk_errors
            .run(|out| InstanceWalk::new(&context.locations, &schema, &root, 16, out))
            .test_value()?;
        let mut visited = 0;
        while let Some((node, _)) = walk_errors.run(|out| walk.next(out)).test_value()? {
            test_assert!(node.complete())?;
            visited += 1;
        }
        test_eq!(visited, 3 + width * 7)?;
        test_eq!(visited, context.locations.nodes().count())?;
    }
    test_eq!(*TestCheck::item(&peaks, 0)?, *TestCheck::item(&peaks, 1)?)?;
    for (index, width) in [64, 512].iter().enumerate() {
        test_eq!(*TestCheck::item(&shape_skips, index)?, 2 + width * 7)?;
    }
    test_assert!(*TestCheck::item(&peaks, 1)? < 32)?;

    Ok(())
}

#[test]
fn anonymous_variants_keep_source_preorder_then_default_declaration_order() -> TestResult<()> {
    let schema = ParserTestSupport::compile(
        r#"struct Child { n: u8 } @assert(false, "child") enum E { Obj { first: Child = {n: 1}, second: Child, third: Child = {n: 3} } @assert(false, "variant") } entry struct Root { e: E } @assert(false, "root")"#,
    )?;
    let errors = ParserTestSupport::parse(
        &schema,
        "{e: .Obj {second: {n: 2}}}",
        &ConfigLimits::default(),
    )
    .test_error()?;
    let mut messages: Vec<_> = errors.iter().map(|error| error.message()).collect();
    messages.sort_unstable();
    test_eq!(messages, ["child", "child", "child", "root", "variant"])?;
    let mut names: Vec<_> = errors
        .iter()
        .filter(|error| error.message() == "child")
        .map(|error| -> TestResult<_> {
            let path = error.path().test_value()?.segments();
            TestCheck::item(path, 2)?.as_field().test_value()
        })
        .collect::<TestResult<Vec<_>>>()?;
    names.sort_unstable();
    test_eq!(names, ["first", "second", "third"])?;
    for error in errors.iter().filter(|error| error.message() == "child") {
        let path = error.path().test_value()?.segments();
        test_eq!(TestCheck::item(path, 0)?.as_field(), Some("e"))?;
        test_eq!(TestCheck::item(path, 1)?.as_enum_payload(), Some("Obj"))?;
        test_assert!(TestCheck::item(path, 2)?.as_field().is_some())?;
        test_eq!(
            path.iter()
                .filter(|segment| matches!(segment, PathSegment::EnumPayload(_)))
                .count(),
            1
        )?;
    }

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
