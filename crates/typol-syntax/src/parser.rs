#[macro_use]
mod context;
mod annotation;
mod declaration;
mod expression;
mod recovery;
mod type_expr;
mod value;

use crate::{
    InputConfig, Keyword, StreamLimits, Symbol, SyntaxErrors, SyntaxLimits, TokenKind,
    ast::SchemaAst,
    parser::{
        context::{ParserControl, ParserState},
        declaration::DeclarationParser,
        recovery::RecoveryParser,
    },
    storage::TopLevelItem,
};
use core::fmt::{Debug, Formatter, Result as FmtResult};
use opeo::{ErrSlot, OResult, Out};
use typol_core::{
    ChargedVec, Diagnostic, ExecutionBudget, Read, SliceSource, SourceId, SourceLocation,
    SourceSpan, Span, diagnostic_context,
};

/// UTF-8 boundary positions recorded while parsing, without retaining source text.
pub struct SourcePositionIndex {
    entries: ChargedVec<(usize, u64)>,
    extended_positions: ChargedVec<(usize, SourceLocation)>,
    source_len: usize,
}

impl SourcePositionIndex {
    pub(crate) fn new(
        entries: ChargedVec<(usize, u64)>,
        extended_positions: ChargedVec<(usize, SourceLocation)>,
        source_len: usize,
    ) -> Self {
        Self {
            entries,
            extended_positions,
            source_len,
        }
    }

    pub const fn source_len(&self) -> usize {
        self.source_len
    }

    pub fn location(&self, offset: usize) -> Option<SourceLocation> {
        let entry = self
            .entries
            .binary_search_by_key(&offset, |entry| entry.0)
            .ok()
            .and_then(|index| self.entries.get(index))?;
        if entry.1 == 0 {
            return self
                .extended_positions
                .binary_search_by_key(&offset, |position| position.0)
                .ok()
                .and_then(|index| self.extended_positions.get(index))
                .map(|position| position.1);
        }
        let line = usize::try_from(entry.1 >> 32).ok()?;
        let column = usize::try_from(entry.1 as u32).ok()?;
        SourceLocation::new(line, column)
    }

    pub fn is_char_boundary(&self, offset: usize) -> bool {
        self.location(offset).is_some()
    }

    pub fn is_valid_span(&self, start: usize, end: usize) -> bool {
        start <= end
            && end <= self.source_len
            && self.is_char_boundary(start)
            && self.is_char_boundary(end)
    }
}

impl Debug for SourcePositionIndex {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        formatter
            .debug_struct("SourcePositionIndex")
            .field("entry_count", &self.entries.len())
            .field("extended_position_count", &self.extended_positions.len())
            .field("source_len", &self.source_len)
            .finish()
    }
}

/// A successfully parsed AST and its UTF-8 boundary position index.
pub struct SchemaParseOutput {
    ast: SchemaAst,
    positions: SourcePositionIndex,
}

impl SchemaParseOutput {
    pub fn ast(&self) -> &SchemaAst {
        &self.ast
    }

    pub fn positions(&self) -> &SourcePositionIndex {
        &self.positions
    }

    pub fn into_parts(self) -> (SchemaAst, SourcePositionIndex) {
        (self.ast, self.positions)
    }
}

/// Resource settings for Schema syntax and token streaming.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchemaParseOptions {
    syntax: SyntaxLimits,
    stream: StreamLimits,
}

impl SchemaParseOptions {
    /// Creates parse options from syntax and token stream limits.
    pub const fn new(syntax: SyntaxLimits, stream: StreamLimits) -> Self {
        Self { syntax, stream }
    }

    /// Uses the default token stream limits with the provided Schema syntax limits.
    pub fn with_syntax(syntax: SyntaxLimits) -> Self {
        Self::new(syntax, StreamLimits::default())
    }

    /// Returns syntax tree and parser limits.
    pub const fn syntax(&self) -> &SyntaxLimits {
        &self.syntax
    }

    /// Returns token window and token length limits.
    pub const fn stream(&self) -> StreamLimits {
        self.stream
    }
}

impl Default for SchemaParseOptions {
    fn default() -> Self {
        Self::new(SyntaxLimits::default(), StreamLimits::default())
    }
}

enum ParseAttempt<T> {
    Success(T),
    Failure(Diagnostic),
}

struct ParseRequest<'src, 'options, 'budget> {
    source_id: SourceId,
    source: &'src mut dyn Read<Diagnostic>,
    options: &'options SchemaParseOptions,
    budget: &'budget ExecutionBudget,
}

fn try_diagnostic<T>(
    errors: &mut ErrSlot<Diagnostic>,
    operation: impl for<'slot> FnOnce(Out<'slot, 'slot, Diagnostic>) -> OResult<'slot, T>,
) -> ParseAttempt<T> {
    match errors.try_call(operation) {
        Ok(value) => ParseAttempt::Success(value),
        Err(error) => ParseAttempt::Failure(error.take()),
    }
}

/// All parser storage and work use the same budget supplied by the caller.
pub struct SchemaParser;

impl SchemaParser {
    pub fn parse<'slot>(
        source_id: SourceId,
        source: &mut dyn Read<Diagnostic>,
        options: &SchemaParseOptions,
        budget: &ExecutionBudget,
        out: Out<'slot, '_, SyntaxErrors>,
    ) -> OResult<'slot, SchemaAst> {
        let parsed = Self::parse_internal(
            ParseRequest {
                source_id,
                source,
                options,
                budget,
            },
            out,
        )?;
        Ok(parsed.into_parts().0)
    }

    pub fn parse_with_positions<'slot>(
        source_id: SourceId,
        source: &mut dyn Read<Diagnostic>,
        options: &SchemaParseOptions,
        budget: &ExecutionBudget,
        out: Out<'slot, '_, SyntaxErrors>,
    ) -> OResult<'slot, SchemaParseOutput> {
        Self::parse_internal(
            ParseRequest {
                source_id,
                source,
                options,
                budget,
            },
            out,
        )
    }

    fn parse_internal<'slot>(
        request: ParseRequest<'_, '_, '_>,
        mut out: Out<'slot, '_, SyntaxErrors>,
    ) -> OResult<'slot, SchemaParseOutput> {
        let ParseRequest {
            source_id,
            source,
            options,
            budget,
        } = request;
        let input_limits = options.syntax.input_limits();
        let mut errors = ErrSlot::<Diagnostic>::new();
        let parser = match try_diagnostic(&mut errors, |out| {
            ParserControl::create(
                InputConfig {
                    source_id,
                    source,
                    resources: &input_limits,
                    stream: options.stream,
                    budget,
                },
                &options.syntax,
                out,
            )
        }) {
            ParseAttempt::Success(parser) => parser,
            ParseAttempt::Failure(error) => {
                return Err(out.reborrow().fail(SyntaxErrors::inline(error)));
            }
        };
        SchemaParser::parse_file(parser, &mut errors, out.reborrow())
    }

    pub fn parse_str<'slot>(
        source_id: SourceId,
        source: &str,
        options: &SchemaParseOptions,
        budget: &ExecutionBudget,
        out: Out<'slot, '_, SyntaxErrors>,
    ) -> OResult<'slot, SchemaAst> {
        let mut input = SliceSource::new(source.as_bytes());
        SchemaParser::parse(source_id, &mut input, options, budget, out)
    }
}

impl SchemaParser {
    fn parse_file<'slot>(
        mut parser: ParserState<'_>,
        errors: &mut ErrSlot<Diagnostic>,
        mut out: Out<'slot, '_, SyntaxErrors>,
    ) -> OResult<'slot, SchemaParseOutput> {
        let initial_span = SourceSpan::new(parser.source_id, Span::empty());
        let mut items = match try_diagnostic(errors, |out| {
            ParserControl::new_vec(&parser, initial_span, out)
        }) {
            ParseAttempt::Success(items) => items,
            ParseAttempt::Failure(error) => {
                return Err(out.reborrow().fail(SyntaxErrors::inline(error)));
            }
        };

        while !parser.stopped {
            let info = match try_diagnostic(errors, |out| ParserControl::peek(&mut parser, out)) {
                ParseAttempt::Success(info) => info,
                ParseAttempt::Failure(diagnostic) => {
                    let failed_at = diagnostic
                        .primary()
                        .map(|span| span.span().start())
                        .unwrap_or_else(|| ParserControl::eof_span(&parser).span().start());
                    SchemaParser::push_diagnostic(&mut parser, errors, diagnostic, out.reborrow())?;
                    match try_diagnostic(errors, |out| {
                        RecoveryParser::recover_top_level(&mut parser, failed_at, out)
                    }) {
                        ParseAttempt::Success(()) => {}
                        ParseAttempt::Failure(error) => SchemaParser::push_diagnostic(
                            &mut parser,
                            errors,
                            error,
                            out.reborrow(),
                        )?,
                    }
                    continue;
                }
            };
            if info.kind == TokenKind::Eof {
                break;
            }

            let failed_at = info.span.span().start();
            match try_diagnostic(errors, |out| {
                SchemaParser::parse_top_level_item(&mut parser, out)
            }) {
                ParseAttempt::Success(item) => {
                    match try_diagnostic(errors, |out| {
                        ParserControl::try_push(&mut items, item, info.span, out)
                    }) {
                        ParseAttempt::Success(()) => {}
                        ParseAttempt::Failure(error) => SchemaParser::push_diagnostic(
                            &mut parser,
                            errors,
                            error,
                            out.reborrow(),
                        )?,
                    }
                }
                ParseAttempt::Failure(error) => {
                    SchemaParser::push_diagnostic(&mut parser, errors, error, out.reborrow())?;
                    match try_diagnostic(errors, |out| {
                        RecoveryParser::recover_top_level(&mut parser, failed_at, out)
                    }) {
                        ParseAttempt::Success(()) => {}
                        ParseAttempt::Failure(error) => SchemaParser::push_diagnostic(
                            &mut parser,
                            errors,
                            error,
                            out.reborrow(),
                        )?,
                    }
                }
            }
        }

        let known_position = ParserControl::eof_span(&parser);
        if let Some(errors) = parser.diagnostics.finish() {
            return Err(out.reborrow().fail(errors));
        }
        let Some(source_len) = parser.actual_source_len else {
            let error = ParserControl::internal_error(
                diagnostic_context!("typol-syntax", "parser", "eof-not-confirmed"),
                known_position,
            );
            return Err(out.reborrow().fail(SyntaxErrors::inline(error)));
        };
        let file_span = SourceSpan::new(parser.source_id, Span::from_zero_to(source_len));
        match try_diagnostic(errors, |out| {
            parser.ast.finish_source(source_len, file_span, out)
        }) {
            ParseAttempt::Success(()) => {
                let positions = parser.cursor.into_positions(source_len);
                let ast = SchemaAst::finish(parser.ast, items, file_span);
                Ok(SchemaParseOutput { ast, positions })
            }
            ParseAttempt::Failure(error) => Err(out.reborrow().fail(SyntaxErrors::inline(error))),
        }
    }

    fn push_diagnostic<'slot>(
        parser: &mut ParserState<'_>,
        errors: &mut ErrSlot<Diagnostic>,
        diagnostic: Diagnostic,
        mut out: Out<'slot, '_, SyntaxErrors>,
    ) -> OResult<'slot, ()> {
        match try_diagnostic(errors, |out| {
            ParserControl::push_diagnostic(parser, diagnostic, out)
        }) {
            ParseAttempt::Success(()) => Ok(()),
            ParseAttempt::Failure(error) => Err(out.reborrow().fail(SyntaxErrors::inline(error))),
        }
    }

    fn parse_top_level_item<'slot>(
        parser: &mut ParserState<'_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, TopLevelItem> {
        let info = ParserControl::peek(parser, out.reborrow())?;
        match info.kind {
            TokenKind::Keyword(Keyword::Extern) => {
                ParserControl::bump(parser, out.reborrow())?;
                ParserControl::expect_keyword(parser, Keyword::Mod, out.reborrow())?;
                let name = ParserControl::take_name(parser, out.reborrow())?;
                ParserControl::expect_symbol(parser, Symbol::Semicolon, out.reborrow())?;
                let span = ParserControl::span_from_start(parser, info.span, out.reborrow())?;
                let declaration = parser.ast.extern_mod_decl_new(name, span, out.reborrow())?;
                Ok(TopLevelItem::ExternMod(declaration))
            }
            TokenKind::Keyword(Keyword::Entry) => {
                ParserControl::bump(parser, out.reborrow())?;
                let entry_start = info.span;
                ParserControl::expect_keyword(parser, Keyword::Struct, out.reborrow())?;
                let declaration = DeclarationParser::parse_struct_after_keyword(
                    parser,
                    entry_start,
                    out.reborrow(),
                )?;
                Ok(TopLevelItem::EntryStruct(declaration))
            }
            TokenKind::Keyword(Keyword::Mod | Keyword::Type | Keyword::Struct | Keyword::Enum) => {
                let declaration = DeclarationParser::parse_declaration(parser, out.reborrow())?;
                Ok(TopLevelItem::Declaration(declaration))
            }
            _ => {
                let diagnostic = ParserControl::syntax_error(
                    parser,
                    info.span,
                    "this Schema file item is not allowed here",
                    out.reborrow(),
                )?;
                Err(out.reborrow().fail(diagnostic))
            }
        }
    }
}
