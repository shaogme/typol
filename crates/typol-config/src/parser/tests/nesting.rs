use super::ParserTestSupport;
use crate::ConfigLimits;
use alloc::format;
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
        let schema = ParserTestSupport::compile(case.schema)?;
        let mut limits = ConfigLimits::default();
        limits = limits.with_max_value_nodes(case.nodes);
        test_assert!(ParserTestSupport::parse(&schema, case.source, &limits).is_ok())?;
        limits = limits.with_max_value_nodes(case.nodes - 1);
        let errors = ParserTestSupport::parse(&schema, case.source, &limits).test_error()?;
        test_eq!(ParserTestSupport::codes(&errors), [1])?;
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
        let schema = ParserTestSupport::compile(case.schema)?;
        let mut limits = ConfigLimits::default();
        limits = limits.with_max_nesting_depth(case.depth);
        test_assert!(ParserTestSupport::parse(&schema, case.source, &limits).is_ok())?;
        limits = limits.with_max_nesting_depth(case.depth - 1);
        let errors = ParserTestSupport::parse(&schema, case.source, &limits).test_error()?;
        test_eq!(ParserTestSupport::codes(&errors), [1])?;
    }

    Ok(())
}

#[test]
fn recovery_scanning_also_obeys_nesting_limit() -> TestResult<()> {
    let schema = ParserTestSupport::compile("entry struct Root { a: u8, b: u8, }")?;
    let mut limits = ConfigLimits::default();
    limits = limits.with_max_nesting_depth(2);
    let errors =
        ParserTestSupport::parse(&schema, "{ a: [[[1]]], b: 999 }", &limits).test_error()?;
    test_eq!(ParserTestSupport::codes(&errors), [11, 1])?;

    Ok(())
}

#[test]
fn enum_default_and_explicit_structure_share_depth_accounting() -> TestResult<()> {
    let schema = ParserTestSupport::compile(
        "enum E { Obj { a: [u8], }, } entry struct Root { e: E = .Obj { a: [1] }, }",
    )?;
    let mut limits = ConfigLimits::default();
    limits = limits.with_max_nesting_depth(4);
    for source in ["{}", "{ e: .Obj { a: [1] } }"] {
        if let TestOpeoOutcome::Error(errors) = ParserTestSupport::parse(&schema, source, &limits) {
            return Err(TestFailure::new(format!("{source}: {errors}")));
        }
    }
    limits = limits.with_max_nesting_depth(3);
    for source in ["{}", "{ e: .Obj { a: [1] } }"] {
        let errors = ParserTestSupport::parse(&schema, source, &limits).test_error()?;
        test_eq!(ParserTestSupport::codes(&errors), [1])?;
    }

    Ok(())
}
