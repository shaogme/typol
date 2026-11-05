use opeo::{OResult, Out};
use std::{
    error::Error,
    fmt::{self, Display, Formatter},
};
use typol_config::{
    ConfigDocument, ConfigEdit, ConfigEditErrorKind, ConfigFormatErrorKind, ConfigFormatOptions,
    ConfigFormatter, ConfigLimits, ConfigParseOptions, ConfigParseRequest, ConfigParser,
    ConfigPath, ConfigReplacement, ConfigSourceCommitError, ConfigSourceExpectation,
    ConfigSourceRevision, ConfigSourceWriter, EmptyEnvironment, Environment, EnvironmentError,
    EnvironmentReadContext,
};
use typol_core::{
    ConfigValue, ConfigValueBuilder, ExecutionBudget, ExecutionLimits, SliceSource, ValueSource,
};
use typol_schema::{SchemaCompileOptions, SchemaSource};
use typol_semantic::CompiledSchema;
use typol_test_support::{
    ConfigTestSupport as Support, TestExtract, TestOpeo, TestResult, test_verify as test_assert,
    test_verify_eq as test_eq,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MemoryWriteError {
    Conflict,
    Failed,
}

impl Display for MemoryWriteError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Conflict => "source revision changed",
            Self::Failed => "memory backend failed",
        })
    }
}

impl Error for MemoryWriteError {}

struct MemoryWriter {
    contents: Option<Vec<u8>>,
    revision: u64,
    fail: bool,
}

impl ConfigSourceWriter for MemoryWriter {
    type Target = str;
    type Error = MemoryWriteError;

    fn replace_atomically(
        &mut self,
        _: &Self::Target,
        expectation: ConfigSourceExpectation<'_>,
        replacement: &[u8],
        max_bytes: usize,
    ) -> Result<ConfigSourceRevision, Self::Error> {
        if replacement.len() > max_bytes {
            return Err(MemoryWriteError::Failed);
        }
        match expectation {
            ConfigSourceExpectation::ExactSnapshot(expected)
                if self.contents.as_deref() != Some(expected) =>
            {
                return Err(MemoryWriteError::Conflict);
            }
            ConfigSourceExpectation::MustBeAbsent if self.contents.is_some() => {
                return Err(MemoryWriteError::Conflict);
            }
            ConfigSourceExpectation::DocumentSnapshot => return Err(MemoryWriteError::Conflict),
            ConfigSourceExpectation::ExactSnapshot(_)
            | ConfigSourceExpectation::MustBeAbsent
            | ConfigSourceExpectation::ReplaceAny => {}
        }
        if self.fail {
            return Err(MemoryWriteError::Failed);
        }
        let mut contents = Vec::new();
        contents
            .try_reserve_exact(replacement.len())
            .map_err(|_| MemoryWriteError::Failed)?;
        contents.extend_from_slice(replacement);
        self.contents = Some(contents);
        self.revision = self.revision.saturating_add(1);
        Ok(ConfigSourceRevision::new(self.revision))
    }
}

fn compile(text: &str) -> TestResult<CompiledSchema> {
    Support::compile_schema(
        SchemaCompileOptions::default(),
        SchemaSource::from_str(b"root", text),
        None,
        None,
    )
    .test_value()
}

fn parse_document<'schema>(
    schema: &'schema CompiledSchema,
    source: &str,
) -> TestResult<typol_config::ConfigDocument<'schema>> {
    parse_document_with_options(schema, source, ConfigParseOptions::default())
}

fn parse_document_with_options<'schema>(
    schema: &'schema CompiledSchema,
    source: &str,
    options: ConfigParseOptions,
) -> TestResult<typol_config::ConfigDocument<'schema>> {
    let mut input = SliceSource::new(source.as_bytes());
    TestOpeo::value(|out| {
        ConfigParser::new(schema).parse_document(
            ConfigParseRequest::new(&mut input, "document", options),
            &mut EmptyEnvironment,
            out,
        )
    })
}

fn format_document(
    document: &ConfigDocument<'_>,
    options: ConfigFormatOptions,
) -> TestResult<String> {
    let formatted = TestOpeo::value(|out| ConfigFormatter::format(document, options, out))?;
    Ok(formatted.as_str().to_owned())
}

#[test]
fn formatter_expands_containers_and_preserves_source_expressions_and_order() -> TestResult<()> {
    let schema = compile(
        r#"
        enum Mode { Ready, Named(string), Details { name: string, }, }
        entry struct Root {
            title: string,
            values: [u8],
            settings: map<string, u8>,
            mode: Mode,
            details: Mode,
            port: u16 @env("PORT"),
        }
        "#,
    )?;
    let source = r#"{title:"中,}/*",values:[1,2],settings:{"b":2,"a":1},mode:.Named("x, //"),details:.Details{name:"owner"},port:env("PORT",3)}"#;
    let document = parse_document(&schema, source)?;
    let formatted = format_document(&document, ConfigFormatOptions::default())?;
    test_eq!(
        formatted,
        concat!(
            "{\n",
            "    title: \"中,}/*\",\n",
            "    values: [\n",
            "        1,\n",
            "        2,\n",
            "    ],\n",
            "    settings: {\n",
            "        \"b\": 2,\n",
            "        \"a\": 1,\n",
            "    },\n",
            "    mode: .Named(\"x, //\"),\n",
            "    details: .Details {\n",
            "        name: \"owner\",\n",
            "    },\n",
            "    port: env(\"PORT\", 3),\n",
            "}\n"
        )
    );
    let formatted_document = parse_document(&schema, &formatted)?;
    test_eq!(
        format_document(&formatted_document, ConfigFormatOptions::default())?,
        formatted
    );
    Ok(())
}

#[test]
fn formatter_preserves_comments_and_crlf_while_ending_with_a_newline() -> TestResult<()> {
    let schema = compile("entry struct Root { first: u8, second: [u8] }")?;
    let source = "{/* leading */first:1,/* between */second:[/* item */2]}";
    let document = parse_document(&schema, source)?;
    let formatted = format_document(&document, ConfigFormatOptions::default())?;
    test_assert!(formatted.contains("/* leading */"));
    test_assert!(formatted.contains("/* between */"));
    test_assert!(formatted.contains("/* item */"));
    let formatted_document = parse_document(&schema, &formatted)?;
    test_eq!(
        format_document(&formatted_document, ConfigFormatOptions::default())?,
        formatted
    );

    let line_comments = "{ // root\nfirst:1, // first\nsecond:[2] // trailing\n}";
    let line_document = parse_document(&schema, line_comments)?;
    let formatted_lines = format_document(&line_document, ConfigFormatOptions::default())?;
    test_assert!(formatted_lines.contains("// root\n"));
    test_assert!(formatted_lines.contains("first: 1, // first\n"));
    test_assert!(formatted_lines.contains("], // trailing\n"));
    let formatted_line_document = parse_document(&schema, &formatted_lines)?;
    test_eq!(
        format_document(&formatted_line_document, ConfigFormatOptions::default())?,
        formatted_lines
    );

    let crlf = "{\r\nfirst:1,\r\nsecond:[],\r\n}";
    let crlf_document = parse_document(&schema, crlf)?;
    let formatted_crlf = format_document(&crlf_document, ConfigFormatOptions::default())?;
    test_assert!(formatted_crlf.contains("\r\n"));
    test_assert!(!formatted_crlf.replace("\r\n", "").contains('\n'));
    test_assert!(formatted_crlf.ends_with("}\r\n"));
    Ok(())
}

#[test]
fn formatter_keeps_root_comments_at_column_zero_and_removes_comment_tail_space() -> TestResult<()> {
    let schema = compile("entry struct Root { first: u8, second: [u8] }")?;
    let root_line_comment = "// note\n{first:1,second:[]}";
    let line_document = parse_document(&schema, root_line_comment)?;
    let formatted_line = format_document(&line_document, ConfigFormatOptions::default())?;
    test_eq!(
        formatted_line,
        "// note\n{\n    first: 1,\n    second: [],\n}\n"
    );

    let root_block_comment = "/* note */\n{first:1,second:[]}";
    let block_document = parse_document(&schema, root_block_comment)?;
    let formatted_block = format_document(&block_document, ConfigFormatOptions::default())?;
    test_eq!(
        formatted_block,
        "/* note */\n{\n    first: 1,\n    second: [],\n}\n"
    );

    let trailing_comment_schema = compile("entry struct Root { value: u8 }")?;
    let trailing_comment_document = parse_document(&trailing_comment_schema, "{value:1 /*note*/}")?;
    let formatted_trailing_comment =
        format_document(&trailing_comment_document, ConfigFormatOptions::default())?;
    test_eq!(formatted_trailing_comment, "{\n    value: 1, /*note*/\n}\n");
    let reparsed = parse_document(&trailing_comment_schema, &formatted_trailing_comment)?;
    test_eq!(
        format_document(&reparsed, ConfigFormatOptions::default())?,
        formatted_trailing_comment
    );
    Ok(())
}

#[test]
fn formatter_enforces_output_and_workspace_limits_without_returning_partial_text() -> TestResult<()>
{
    let schema = compile("entry struct Root { value: u8 }")?;
    let document = parse_document(&schema, "{value:1}")?;
    let output_error = TestOpeo::error(|out| {
        ConfigFormatter::format(
            &document,
            ConfigFormatOptions::default().with_max_output_bytes(1),
            out,
        )
    })?;
    test_eq!(
        output_error.kind(),
        ConfigFormatErrorKind::ResourceLimitExceeded
    );

    let workspace_error = TestOpeo::error(|out| {
        ConfigFormatter::format(
            &document,
            ConfigFormatOptions::default().with_max_workspace_bytes(0),
            out,
        )
    })?;
    test_eq!(
        workspace_error.kind(),
        ConfigFormatErrorKind::ResourceLimitExceeded
    );
    Ok(())
}

struct FailingEnvironment;

impl Environment for FailingEnvironment {
    fn read<'env, 'slot, 'borrow>(
        &'env mut self,
        _name: &str,
        _context: EnvironmentReadContext<'_>,
        out: Out<'slot, 'borrow, EnvironmentError>,
    ) -> OResult<'slot, Option<&'env [u8]>> {
        Err(out.fail(EnvironmentError::HostFailure))
    }
}

fn unsigned(value: u128) -> TestResult<ConfigValue> {
    let budget = TestOpeo::value(|out| {
        ExecutionBudget::new(ExecutionLimits::new(1_000_000, 1_000_000), out)
    })?;
    TestOpeo::value(|mut out| {
        let mut builder = ConfigValueBuilder::new(Some(&budget), out.reborrow())?;
        let root = builder.add_unsigned(value, ValueSource::RustValue, out.reborrow())?;
        builder.finish(root, out)
    })
}

fn text(value: &str) -> TestResult<ConfigValue> {
    let budget = TestOpeo::value(|out| {
        ExecutionBudget::new(ExecutionLimits::new(1_000_000, 1_000_000), out)
    })?;
    TestOpeo::value(|mut out| {
        let mut builder = ConfigValueBuilder::new(Some(&budget), out.reborrow())?;
        let root = builder.add_string(value, ValueSource::RustValue, out.reborrow())?;
        builder.finish(root, out)
    })
}

fn float32(value: f32) -> TestResult<ConfigValue> {
    let budget = TestOpeo::value(|out| {
        ExecutionBudget::new(ExecutionLimits::new(1_000_000, 1_000_000), out)
    })?;
    TestOpeo::value(|mut out| {
        let mut builder = ConfigValueBuilder::new(Some(&budget), out.reborrow())?;
        let root = builder.add_float32(value, ValueSource::RustValue, out.reborrow())?;
        builder.finish(root, out)
    })
}

fn unsigned_array(first: u128, second: u128) -> TestResult<ConfigValue> {
    let budget = TestOpeo::value(|out| {
        ExecutionBudget::new(ExecutionLimits::new(1_000_000, 1_000_000), out)
    })?;
    TestOpeo::value(|mut out| {
        let mut builder = ConfigValueBuilder::new(Some(&budget), out.reborrow())?;
        let first = builder.add_unsigned(first, ValueSource::RustValue, out.reborrow())?;
        let second = builder.add_unsigned(second, ValueSource::RustValue, out.reborrow())?;
        let array = builder.add_array(&[first, second], ValueSource::RustValue, out.reborrow())?;
        builder.finish(array, out)
    })
}

#[test]
fn retained_source_round_trips_and_batch_replaces_only_value_spans() -> TestResult {
    let schema = compile("entry struct Root { port: u16, name: string }")?;
    let original = "{\r\n  port: 8080, // port comment\r\n  name: \"private-name\"\r\n}\r\n";
    let mut document = parse_document(&schema, original)?;
    let mut output = String::new();
    document.write_to(&mut output).test_value()?;
    test_eq!(output, original);
    test_assert!(!format!("{document:?}").contains("private-name"));

    let port = ConfigPath::new().field("port");
    let name = ConfigPath::new().field("name");
    let port_value = unsigned(9090)?;
    let name_value = text("updated")?;
    let replacements = [
        ConfigReplacement::new(&port, &port_value),
        ConfigReplacement::new(&name, &name_value),
    ];
    TestOpeo::value(|out| document.replace_values(&replacements, &mut EmptyEnvironment, out))?;

    test_eq!(
        document.source(),
        "{\r\n  port: 9090, // port comment\r\n  name: \"updated\"\r\n}\r\n"
    );
    Ok(())
}

#[test]
fn failed_batch_validation_preserves_document_source() -> TestResult {
    let schema = compile("entry struct Root { port: u16 }")?;
    let original = "{ port: 8080 }";
    let mut document = parse_document(&schema, original)?;
    let path = ConfigPath::new().field("port");
    let invalid = text("not-a-number")?;
    let replacement = unsigned(9000)?;
    let conflicts = [
        ConfigReplacement::new(&path, &replacement),
        ConfigReplacement::new(&path, &replacement),
    ];
    let conflict =
        TestOpeo::error(|out| document.replace_values(&conflicts, &mut EmptyEnvironment, out))?;
    test_eq!(conflict.kind(), ConfigEditErrorKind::EditConflict);
    test_eq!(document.source(), original);

    let error =
        TestOpeo::error(|out| document.set_value(&path, &invalid, &mut EmptyEnvironment, out))?;
    test_eq!(error.kind(), ConfigEditErrorKind::ValidationFailed);
    test_eq!(document.source(), original);
    Ok(())
}

#[test]
fn string_encoding_rejects_unrepresentable_control_characters_before_validation() -> TestResult {
    let schema = compile("entry struct Root { value: string }")?;
    let original = "{value: \"safe\"}";
    let path = ConfigPath::new().field("value");
    let mut document = parse_document(&schema, original)?;
    for invalid in ["nul\0value", "c1\u{0085}value"] {
        let replacement = text(invalid)?;
        let error = TestOpeo::error(|out| {
            document.set_value(&path, &replacement, &mut EmptyEnvironment, out)
        })?;
        test_eq!(error.kind(), ConfigEditErrorKind::EncodingFailed);
        test_eq!(document.source(), original);
    }
    Ok(())
}

#[test]
fn validation_failure_does_not_expose_a_secret_source_value() -> TestResult {
    let schema = compile("entry struct Root { password: string @secret, port: u8 }")?;
    let original = "{password: \"do-not-expose-this\", port: 1}";
    let mut document = parse_document(&schema, original)?;
    let path = ConfigPath::new().field("port");
    let replacement = text("invalid")?;
    let error =
        TestOpeo::error(|out| document.set_value(&path, &replacement, &mut EmptyEnvironment, out))?;
    let display = format!("{error}");
    let debug = format!("{error:?}");
    let source_debug = format!("{:?}", Error::source(&error));
    test_eq!(error.kind(), ConfigEditErrorKind::ValidationFailed);
    test_assert!(!display.contains("do-not-expose-this"));
    test_assert!(!debug.contains("do-not-expose-this"));
    test_assert!(!source_debug.contains("do-not-expose-this"));
    test_eq!(document.source(), original);
    Ok(())
}

#[test]
fn environment_provider_failure_rolls_back_the_edit() -> TestResult {
    let schema = compile(r#"entry struct Root { port: u8 @env("PORT"), label: string }"#)?;
    let original = "{port: env(\"PORT\", 1), label: \"before\"}";
    let mut document = parse_document(&schema, original)?;
    let path = ConfigPath::new().field("label");
    let replacement = text("after")?;
    let error = TestOpeo::error(|out| {
        document.set_value(&path, &replacement, &mut FailingEnvironment, out)
    })?;
    test_eq!(error.kind(), ConfigEditErrorKind::ValidationFailed);
    test_eq!(document.source(), original);
    Ok(())
}

#[test]
fn candidate_source_limit_is_enforced_before_document_commit() -> TestResult {
    let schema = compile("entry struct Root { value: u16 }")?;
    let original = "{value: 1}";
    let options = ConfigParseOptions::default()
        .with_config(ConfigLimits::default().with_max_source_bytes(original.len()));
    let mut document = parse_document_with_options(&schema, original, options)?;
    let path = ConfigPath::new().field("value");
    let replacement = unsigned(10_000)?;
    let error =
        TestOpeo::error(|out| document.set_value(&path, &replacement, &mut EmptyEnvironment, out))?;
    test_eq!(error.kind(), ConfigEditErrorKind::ResourceLimitExceeded);
    test_eq!(document.source(), original);
    Ok(())
}

#[test]
fn defaulted_values_are_not_given_fabricated_source_spans() -> TestResult {
    let schema = compile("entry struct Root { port: u16 = 8080 }")?;
    let mut document = parse_document(&schema, "{}")?;
    let path = ConfigPath::new().field("port");
    let replacement = unsigned(9000)?;
    let error =
        TestOpeo::error(|out| document.set_value(&path, &replacement, &mut EmptyEnvironment, out))?;
    test_eq!(error.kind(), ConfigEditErrorKind::InvalidPath);
    test_eq!(document.source(), "{}");
    Ok(())
}

#[test]
fn float_and_anonymous_enum_paths_use_parser_metadata() -> TestResult {
    let schema =
        compile("enum Choice { Object { ratio: f32 } } entry struct Root { choice: Choice }")?;
    let mut document = parse_document(&schema, "{choice: .Object {ratio: 1.0}}")?;
    let path = ConfigPath::new()
        .field("choice")
        .variant("Object")
        .field("ratio");
    let value = float32(-0.0)?;
    TestOpeo::value(|out| document.set_value(&path, &value, &mut EmptyEnvironment, out))?;
    test_eq!(document.source(), "{choice: .Object {ratio: -0.0}}");
    Ok(())
}

#[test]
fn writer_conflicts_and_failures_leave_document_and_target_unchanged() -> TestResult {
    let schema = compile("entry struct Root { port: u16 }")?;
    let original = "{port: 8080}";
    let mut document = parse_document(&schema, original)?;
    let mut writer = MemoryWriter {
        contents: Some(b"{port: 9000}".to_vec()),
        revision: 3,
        fail: false,
    };
    let expectation = document.source_expectation();
    let conflict = document.write_source("config", &mut writer, expectation);
    test_assert!(matches!(
        conflict,
        Err(ConfigSourceCommitError::Writer(MemoryWriteError::Conflict))
    ));
    test_eq!(document.source(), original);
    test_eq!(writer.contents.as_deref(), Some(b"{port: 9000}".as_slice()));

    writer.contents = Some(original.as_bytes().to_vec());
    writer.fail = true;
    let expectation = document.source_expectation();
    let failure = document.write_source("config", &mut writer, expectation);
    test_assert!(matches!(
        failure,
        Err(ConfigSourceCommitError::Writer(MemoryWriteError::Failed))
    ));
    test_eq!(document.source(), original);
    test_eq!(writer.contents.as_deref(), Some(original.as_bytes()));
    Ok(())
}

#[test]
fn successful_source_commit_advances_the_document_snapshot() -> TestResult {
    let schema = compile("entry struct Root { port: u16 }")?;
    let original = "{port: 1}";
    let mut document = parse_document(&schema, original)?;
    let mut writer = MemoryWriter {
        contents: Some(original.as_bytes().to_vec()),
        revision: 0,
        fail: false,
    };
    let path = ConfigPath::new().field("port");

    let replacement = unsigned(2)?;
    TestOpeo::value(|out| document.set_value(&path, &replacement, &mut EmptyEnvironment, out))?;
    let expectation = document.source_expectation();
    let revision = document
        .write_source("config", &mut writer, expectation)
        .test_value()?;
    test_eq!(revision.value(), 1);
    test_eq!(writer.contents.as_deref(), Some(b"{port: 2}".as_slice()));

    let replacement = unsigned(3)?;
    TestOpeo::value(|out| document.set_value(&path, &replacement, &mut EmptyEnvironment, out))?;
    let expectation = document.source_expectation();
    let revision = document
        .write_source("config", &mut writer, expectation)
        .test_value()?;
    test_eq!(revision.value(), 2);
    test_eq!(writer.contents.as_deref(), Some(b"{port: 3}".as_slice()));
    Ok(())
}

#[test]
fn structural_edit_batch_inserts_fields_map_entries_and_array_items() -> TestResult {
    let schema =
        compile("entry struct Root { port: u16 = 8080, labels: map<string, u16>, items: [u16] }")?;
    let mut document = parse_document(&schema, "{labels: {\"one\":1}, items: [1, 2]}")?;
    let port = ConfigPath::new().field("port");
    let labels = ConfigPath::new().field("labels");
    let items = ConfigPath::new().field("items");
    let port_value = unsigned(9000)?;
    let label_value = unsigned(2)?;
    let item_value = unsigned(9)?;
    let edits = [
        ConfigEdit::InsertField {
            path: &port,
            value: &port_value,
        },
        ConfigEdit::InsertMapEntry {
            container: &labels,
            key: "ø",
            value: &label_value,
        },
        ConfigEdit::InsertArrayItem {
            container: &items,
            index: 1,
            value: &item_value,
        },
    ];
    TestOpeo::value(|out| document.apply_edits(&edits, &mut EmptyEnvironment, out))?;
    test_eq!(
        document.source(),
        "{labels: {\"one\":1, \"ø\":2}, items: [1, 9, 2], port: 9000}"
    );
    Ok(())
}

#[test]
fn structural_edit_batch_can_remove_and_append_the_last_array_item() -> TestResult {
    let schema = compile("entry struct Root { items: [u16] }")?;
    let items = ConfigPath::new().field("items");
    let last = items.clone().index(1);
    let value = unsigned(3)?;

    for (source, expected) in [
        ("{items:[1,2]}", "{items:[1, 3]}"),
        ("{items:[1,2,]}", "{items:[1, 3,]}"),
    ] {
        let mut document = parse_document(&schema, source)?;
        let edits = [
            ConfigEdit::Remove { path: &last },
            ConfigEdit::InsertArrayItem {
                container: &items,
                index: 2,
                value: &value,
            },
        ];
        TestOpeo::value(|out| document.apply_edits(&edits, &mut EmptyEnvironment, out))?;
        test_eq!(document.source(), expected);
    }
    Ok(())
}

#[test]
fn inserted_map_keys_escape_quotes_and_backslashes() -> TestResult {
    let schema = compile("entry struct Root { values: map<string, u16> }")?;
    let mut document = parse_document(&schema, "{values: {}}")?;
    let values = ConfigPath::new().field("values");
    let replacement = unsigned(7)?;
    let edit = ConfigEdit::InsertMapEntry {
        container: &values,
        key: "quote\"and\\slash",
        value: &replacement,
    };
    TestOpeo::value(|out| document.apply_edits(&[edit], &mut EmptyEnvironment, out))?;
    test_eq!(
        document.source(),
        "{values: {\"quote\\\"and\\\\slash\": 7}}"
    );
    Ok(())
}

#[test]
fn structural_insertions_follow_multiline_crlf_indentation() -> TestResult {
    let schema = compile("entry struct Root { port: u16 = 8080, items: [u16] }")?;
    let source = "{\r\n  items: [\r\n    1,\r\n    2,\r\n  ],\r\n}";
    let mut document = parse_document(&schema, source)?;
    let port = ConfigPath::new().field("port");
    let items = ConfigPath::new().field("items");
    let port_value = unsigned(9000)?;
    let item_value = unsigned(9)?;
    let edits = [
        ConfigEdit::InsertField {
            path: &port,
            value: &port_value,
        },
        ConfigEdit::InsertArrayItem {
            container: &items,
            index: 1,
            value: &item_value,
        },
    ];
    TestOpeo::value(|out| document.apply_edits(&edits, &mut EmptyEnvironment, out))?;
    test_eq!(
        document.source(),
        "{\r\n  items: [\r\n    1,\r\n    9,\r\n    2,\r\n  ],\r\n  port: 9000,\r\n}"
    );
    Ok(())
}

#[test]
fn structural_insertion_uses_the_nearest_multiline_item_layout() -> TestResult {
    let schema = compile("entry struct Root { first: u16, second: u16, third: u16 = 0 }")?;
    let mut document = parse_document(&schema, "{first:1,\n  second:2}")?;
    let third = ConfigPath::new().field("third");
    let value = unsigned(3)?;
    let edit = ConfigEdit::InsertField {
        path: &third,
        value: &value,
    };

    TestOpeo::value(|out| document.apply_edits(&[edit], &mut EmptyEnvironment, out))?;
    test_eq!(document.source(), "{first:1,\n  second:2,\n  third:3}");
    Ok(())
}

#[test]
fn array_insertion_handles_empty_containers_and_composite_values() -> TestResult {
    let schema = compile("entry struct Root { items: [[u16]] }")?;
    let mut document = parse_document(&schema, "{items: []}")?;
    let items = ConfigPath::new().field("items");
    let value = unsigned_array(3, 4)?;
    let edit = ConfigEdit::InsertArrayItem {
        container: &items,
        index: 0,
        value: &value,
    };
    TestOpeo::value(|out| document.apply_edits(&[edit], &mut EmptyEnvironment, out))?;
    test_eq!(document.source(), "{items: [[3, 4]]}");
    Ok(())
}

#[test]
fn removal_migrates_comments_and_keeps_the_remaining_document_valid() -> TestResult {
    let schema = compile("entry struct Root { first: u16, second: option<u16>, third: u16 }")?;
    let source = "{\n  first: 1, // before the removed field\n  second: 2, /* inside the removed field */\n  third: 3,\n}";
    let mut document = parse_document(&schema, source)?;
    let second = ConfigPath::new().field("second");
    TestOpeo::value(|out| document.remove(&second, &mut EmptyEnvironment, out))?;
    test_assert!(!document.source().contains("second"));
    test_assert!(document.source().contains("// before the removed field\n"));
    test_assert!(document.source().contains("/* inside the removed field */"));
    test_assert!(document.source().contains("third: 3"));
    Ok(())
}

#[test]
fn removal_preserves_comments_at_first_middle_and_last_positions() -> TestResult {
    let schema = compile(
        "entry struct Root { first: option<u16>, second: option<u16>, third: option<u16> }",
    )?;
    let source = "{\n  // before first\n  first: 1, /* after first */\n  /* before second */ second: 2, // after second\n  third: 3 // after third\n}";
    for field in ["first", "second", "third"] {
        let mut document = parse_document(&schema, source)?;
        let path = ConfigPath::new().field(field);
        TestOpeo::value(|out| document.remove(&path, &mut EmptyEnvironment, out))?;
        test_assert!(!document.source().contains(&format!("{field}:")));
        test_assert!(document.source().contains("// before first\n"));
        test_assert!(document.source().contains("/* after first */"));
        test_assert!(document.source().contains("/* before second */"));
        test_assert!(document.source().contains("// after second\n"));
        test_assert!(document.source().contains("// after third\n"));
    }
    Ok(())
}

#[test]
fn structural_edit_failures_leave_the_document_unchanged() -> TestResult {
    let schema = compile("entry struct Root { values: [u16] }")?;
    let original = "{values: [1]}";
    let mut document = parse_document(&schema, original)?;
    let values = ConfigPath::new().field("values");
    let value = unsigned(2)?;
    let edit = ConfigEdit::InsertArrayItem {
        container: &values,
        index: 2,
        value: &value,
    };
    let error = TestOpeo::error(|out| document.apply_edits(&[edit], &mut EmptyEnvironment, out))?;
    test_eq!(error.kind(), ConfigEditErrorKind::InvalidPath);
    test_eq!(document.source(), original);

    let invalid = text("bad")?;
    let edit = ConfigEdit::InsertArrayItem {
        container: &values,
        index: 1,
        value: &invalid,
    };
    let error = TestOpeo::error(|out| document.apply_edits(&[edit], &mut EmptyEnvironment, out))?;
    test_eq!(error.kind(), ConfigEditErrorKind::ValidationFailed);
    test_eq!(document.source(), original);
    Ok(())
}
