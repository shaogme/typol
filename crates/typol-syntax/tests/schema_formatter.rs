use std::{error::Error, fmt::Debug, io::Error as IoError};

use typol_core::{ExecutionBudget, ExecutionLimits, SourceId};
use typol_syntax::{
    SchemaDocument, SchemaFormatError, SchemaFormatErrorKind, SchemaFormatOptions, SchemaFormatter,
    SchemaParseOptions, SchemaParser, SyntaxErrors, opeo::ErrSlot,
};

fn ensure(condition: bool, message: &str) -> Result<(), Box<dyn Error>> {
    if condition {
        Ok(())
    } else {
        Err(IoError::other(message).into())
    }
}

fn ensure_equal<T: Debug + PartialEq>(
    actual: T,
    expected: T,
    message: &str,
) -> Result<(), Box<dyn Error>> {
    if actual == expected {
        Ok(())
    } else {
        Err(IoError::other(format!(
            "{message}: actual value {actual:?}, expected value {expected:?}"
        ))
        .into())
    }
}

fn budget() -> Result<ExecutionBudget, Box<dyn Error>> {
    let mut errors = ErrSlot::new();
    errors
        .try_call(|out| ExecutionBudget::new(ExecutionLimits::new(2_000_000, 16_000_000), out))
        .map_err(|caught| IoError::other(format!("{:?}", caught.take())).into())
}

fn parse_document(source: &str) -> Result<SchemaDocument<'_>, Box<dyn Error>> {
    let budget = budget()?;
    let mut errors = ErrSlot::<SyntaxErrors>::new();
    errors
        .try_call(|out| {
            SchemaParser::parse_document(
                SourceId::new(0),
                source,
                &SchemaParseOptions::default(),
                &budget,
                out,
            )
        })
        .map_err(|caught| IoError::other(format!("{:?}", caught.take())).into())
}

fn format(source: &str) -> Result<String, Box<dyn Error>> {
    let document = parse_document(source)?;
    let mut format_errors = ErrSlot::<SchemaFormatError>::new();
    let formatted = format_errors
        .try_call(|out| SchemaFormatter::format(document, SchemaFormatOptions::default(), out))
        .map_err(|caught| IoError::other(format!("{:?}", caught.take())))?;
    Ok(formatted.into_string())
}

fn format_error(
    source: &str,
    options: SchemaFormatOptions,
) -> Result<SchemaFormatError, Box<dyn Error>> {
    let document = parse_document(source)?;
    let mut errors = ErrSlot::<SchemaFormatError>::new();
    match errors.try_call(|out| SchemaFormatter::format(document, options, out)) {
        Ok(_) => Err(IoError::other("expected schema formatting failure").into()),
        Err(caught) => Ok(caught.take()),
    }
}

#[test]
fn formatter_consumes_the_validated_document() -> Result<(), Box<dyn Error>> {
    let source = "entry struct Root { value: u8 = 1, }";
    ensure_equal(
        format(source)?,
        "entry struct Root {\n    value: u8 = 1,\n}\n".to_owned(),
        "formatted document output",
    )
}

#[test]
fn expands_declarations_values_and_lists_with_trailing_commas() -> Result<(), Box<dyn Error>> {
    let source = "type Port=[u16;4];entry struct Root{id:Port=0x0a,values:[u8]=[1,2],settings:map<string,string> = {name:\"x\",},empty:map<string,string> = {}}";
    let actual = format(source)?;
    let expected = concat!(
        "type Port = [u16; 4];\n\n",
        "entry struct Root {\n",
        "    id: Port = 0x0a,\n",
        "    values: [u8] = [\n",
        "        1,\n",
        "        2,\n",
        "    ],\n",
        "    settings: map<string, string> = {\n",
        "        name: \"x\",\n",
        "    },\n",
        "    empty: map<string, string> = {},\n",
        "}\n"
    );
    ensure_equal(
        actual.clone(),
        expected.to_owned(),
        "canonical schema output",
    )?;
    ensure_equal(format(&actual)?, actual, "schema formatter idempotence")?;
    Ok(())
}

#[test]
fn expands_arrays_nested_in_map_defaults() -> Result<(), Box<dyn Error>> {
    let source = "entry struct Root { settings: map<string, map<string, [u8]>> = {\"outer\": {\"inner\": [1, 2]}}, }";
    let actual = format(source)?;
    let expected = concat!(
        "entry struct Root {\n",
        "    settings: map<string, map<string, [u8]>> = {\n",
        "        \"outer\": {\n",
        "            \"inner\": [\n",
        "                1,\n",
        "                2,\n",
        "            ],\n",
        "        },\n",
        "    },\n",
        "}\n"
    );
    ensure_equal(
        actual.clone(),
        expected.to_owned(),
        "nested map default layout",
    )?;
    ensure_equal(format(&actual)?, actual, "nested map default idempotence")?;
    Ok(())
}

#[test]
fn preserves_comments_raw_strings_and_expression_spacing() -> Result<(), Box<dyn Error>> {
    let source = "/* 顶部 */entry struct Root{/* field */text:string=\"// /*\\n\", // 行注释\ncount:i32@min(0)=1,} @assert(self.count>0,\"ok\")";
    let actual = format(source)?;
    ensure(
        actual.contains("/* 顶部 */ entry struct Root {"),
        "block comments should stay beside their token",
    )?;
    ensure(
        actual.contains("text: string = \"// /*\\n\","),
        "string token spelling should be preserved",
    )?;
    ensure(
        actual.contains("// 行注释"),
        "line comments should be preserved",
    )?;
    ensure(
        actual.contains("count: i32 @min(0) = 1,"),
        "annotation and field spacing should be canonical",
    )?;
    ensure(
        actual.contains("@assert(self.count > 0, \"ok\")"),
        "expression spacing should be canonical",
    )?;
    ensure_equal(format(&actual)?, actual, "schema formatter idempotence")?;
    Ok(())
}

#[test]
fn preserves_numeric_spelling_and_compacts_unary_negative_values() -> Result<(), Box<dyn Error>> {
    let source = "entry struct Root { decimal: u16 = 0009, hexadecimal: u16 = 0x0f, ratio: f32 = 1.00, } @assert(self.decimal >= -1, \"valid\")";
    let actual = format(source)?;
    ensure(
        actual.contains("decimal: u16 = 0009,"),
        "decimal token spelling should be preserved",
    )?;
    ensure(
        actual.contains("hexadecimal: u16 = 0x0f,"),
        "hexadecimal token spelling should be preserved",
    )?;
    ensure(
        actual.contains("ratio: f32 = 1.00,"),
        "float token spelling should be preserved",
    )?;
    ensure(
        actual.contains("self.decimal >= -1"),
        "unary minus should stay attached to its numeric operand",
    )?;
    ensure_equal(format(&actual)?, actual, "negative expression idempotence")?;
    Ok(())
}

#[test]
fn handles_modules_enums_nested_types_and_utf8_line_endings() -> Result<(), Box<dyn Error>> {
    let source = "mod net{type Addr=option<map<string,[u8]>>;enum State{Ready,Failed(string),Custom{code:i32}}}entry struct Root{state:net::State =.Ready,}";
    let lf = format(source)?;
    ensure(
        lf.contains("mod net {\n    type Addr = option<map<string, [u8]>>;\n\n    enum State {"),
        "nested modules and type aliases should expand",
    )?;
    ensure(
        lf.contains("        Failed(string),\n"),
        "enum tuple payload should remain compact",
    )?;
    ensure(
        lf.contains("        Custom {\n            code: i32,\n        },\n"),
        "enum struct payload should expand",
    )?;
    ensure(
        lf.ends_with("}\n"),
        "formatted source should end with newline",
    )?;
    let crlf = source.replace('}', "}\r\n");
    let crlf = format(&crlf)?;
    ensure(crlf.contains("\r\n"), "CRLF input should retain CRLF")?;
    ensure(
        !crlf.replace("\r\n", "").contains('\n'),
        "CRLF output should not contain bare LF",
    )?;
    ensure_equal(format(&crlf)?, crlf, "CRLF formatter idempotence")?;
    Ok(())
}

#[test]
fn normalizes_line_endings_inside_block_comments() -> Result<(), Box<dyn Error>> {
    let source = "/* first\nsecond */\r\nentry struct Root {}";
    let actual = format(source)?;
    ensure(
        actual.contains("/* first\r\nsecond */"),
        "block comment line endings should match the selected CRLF style",
    )?;
    ensure(
        !actual.replace("\r\n", "").contains('\n'),
        "formatted CRLF source should not retain bare LF inside comments",
    )?;
    ensure_equal(
        format(&actual)?,
        actual,
        "block comment newline idempotence",
    )?;
    Ok(())
}

#[test]
fn rejects_output_workspace_and_depth_limits_without_partial_result() -> Result<(), Box<dyn Error>>
{
    let source = "entry struct Root { value: [u8] = [1, 2], }";
    let output_error = format_error(
        source,
        SchemaFormatOptions::default().with_max_output_bytes(3),
    )?;
    ensure_equal(
        output_error.kind(),
        SchemaFormatErrorKind::ResourceLimitExceeded,
        "output limit error kind",
    )?;
    let workspace_error = format_error(
        source,
        SchemaFormatOptions::default().with_max_workspace_bytes(1),
    )?;
    ensure_equal(
        workspace_error.kind(),
        SchemaFormatErrorKind::ResourceLimitExceeded,
        "workspace limit error kind",
    )?;
    let depth_error = format_error(
        source,
        SchemaFormatOptions::default().with_max_nesting_depth(1),
    )?;
    ensure_equal(
        depth_error.kind(),
        SchemaFormatErrorKind::ResourceLimitExceeded,
        "nesting limit error kind",
    )?;
    Ok(())
}

#[test]
fn parse_only_document_rejects_invalid_syntax_but_not_static_semantics()
-> Result<(), Box<dyn Error>> {
    let budget = budget()?;
    let mut syntax_errors = ErrSlot::<SyntaxErrors>::new();
    let document = syntax_errors
        .try_call(|out| {
            SchemaParser::parse_document(
                SourceId::new(0),
                "entry struct Root { value: Unknown = 1, }",
                &SchemaParseOptions::default(),
                &budget,
                out,
            )
        })
        .map_err(|caught| IoError::other(format!("{:?}", caught.take())))?;
    ensure_equal(document.ast().items().len(), 1, "parse-only AST item count")?;

    let mut syntax_errors = ErrSlot::<SyntaxErrors>::new();
    let invalid = syntax_errors.try_call(|out| {
        SchemaParser::parse_document(
            SourceId::new(0),
            "entry struct Root { value: }",
            &SchemaParseOptions::default(),
            &budget,
            out,
        )
    });
    ensure(
        invalid.is_err(),
        "parse-only document should reject invalid syntax",
    )?;
    Ok(())
}
