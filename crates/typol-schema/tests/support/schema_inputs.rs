use typol_core::{ExecutionBudget, ExecutionLimits, SourceId};
use typol_schema::SchemaLimits;
use typol_syntax::{SchemaAst, SchemaParseOptions, SchemaParser};

pub fn parse(source_id: u32, source: &str, limits: &SchemaLimits) -> SchemaAst {
    let execution = ExecutionBudget::new(ExecutionLimits::new(67_108_864, 67_108_864)).unwrap();

    let options = SchemaParseOptions::new(*limits.syntax(), limits.stream_limits());
    SchemaParser::parse_str(SourceId::new(source_id), source, &options, &execution)
        .expect("test schema syntax should be valid")
}
