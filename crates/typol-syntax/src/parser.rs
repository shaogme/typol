#[macro_use]
mod context;
mod annotation;
mod declaration;
mod expression;
mod recovery;
mod type_expr;
mod value;

use crate::{
    InputConfig, Keyword, SchemaDocument, StreamLimits, Symbol, SyntaxErrors, SyntaxLimits,
    TokenKind,
    ast::SchemaAst,
    parser::{
        context::{ParserControl, ParserState},
        declaration::DeclarationParser,
        recovery::RecoveryParser,
    },
    storage::TopLevelItem,
};
use core::fmt::{Debug, Formatter, Result as FmtResult};
use opeo::{Caught, ErrSlot, OResult, Out};
use typol_core::{
    ChargedVec, Diagnostic, ExecutionBudget, Read, SliceSource, SourceId, SourceLocation,
    SourceSpan, Span, diagnostic_context,
};
use typol_i18n::{Language, Translator, t};

struct TopLevelItemPush {
    item: TopLevelItem,
    span: SourceSpan,
}

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
    translator: Translator,
}

impl SchemaParseOptions {
    /// Creates parse options from syntax and token stream limits.
    pub const fn new(syntax: SyntaxLimits, stream: StreamLimits) -> Self {
        Self {
            syntax,
            stream,
            translator: Translator::new(Language::English),
        }
    }

    pub const fn with_translator(mut self, translator: Translator) -> Self {
        self.translator = translator;
        self
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

    pub const fn translator(&self) -> Translator {
        self.translator
    }
}

impl Default for SchemaParseOptions {
    fn default() -> Self {
        Self::new(SyntaxLimits::default(), StreamLimits::default())
    }
}

enum ParseAttempt<'slot, T> {
    Success(T),
    Failure(Caught<'slot, Diagnostic>),
}

struct ParseRequest<'src, 'options, 'budget> {
    source_id: SourceId,
    source: &'src mut dyn Read<Diagnostic>,
    options: &'options SchemaParseOptions,
    budget: &'budget ExecutionBudget,
}

fn try_diagnostic<'slot, T>(
    errors: &'slot mut ErrSlot<Diagnostic>,
    operation: impl for<'call> FnOnce(Out<'call, 'call, Diagnostic>) -> OResult<'call, T>,
) -> ParseAttempt<'slot, T> {
    match errors.try_call(operation) {
        Ok(value) => ParseAttempt::Success(value),
        Err(error) => ParseAttempt::Failure(error),
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
        let mut errors = ErrSlot::<Diagnostic>::new();
        let parser = match SchemaParser::create_parser(request, &mut errors) {
            ParseAttempt::Success(parser) => parser,
            ParseAttempt::Failure(error) => {
                return Err(out.reborrow().fail(SyntaxErrors::inline(error.take())));
            }
        };
        SchemaParser::parse_file(parser, &mut errors, out.reborrow())
    }

    fn create_parser<'src, 'errors>(
        request: ParseRequest<'src, '_, '_>,
        errors: &'errors mut ErrSlot<Diagnostic>,
    ) -> ParseAttempt<'errors, ParserState<'src>> {
        let ParseRequest {
            source_id,
            source,
            options,
            budget,
        } = request;
        let input_limits = options.syntax.input_limits();
        try_diagnostic(errors, |out| {
            ParserControl::create(
                InputConfig {
                    source_id,
                    source,
                    resources: &input_limits,
                    stream: options.stream,
                    budget,
                },
                &options.syntax,
                options.translator,
                out,
            )
        })
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

    /// Parses source text without resolving imports or performing static semantic checks.
    pub fn parse_document<'source, 'slot>(
        source_id: SourceId,
        source: &'source str,
        options: &SchemaParseOptions,
        budget: &ExecutionBudget,
        out: Out<'slot, '_, SyntaxErrors>,
    ) -> OResult<'slot, SchemaDocument<'source>> {
        let ast = Self::parse_str(source_id, source, options, budget, out)?;
        Ok(SchemaDocument::new(
            source,
            ast,
            options.syntax.max_source_bytes(),
        ))
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
                return Err(out.reborrow().fail(SyntaxErrors::inline(error.take())));
            }
        };
        SchemaParser::parse_top_level_items(&mut parser, errors, &mut items, out.reborrow())?;

        SchemaParser::finish_file(parser, errors, items, out)
    }

    fn finish_file<'slot>(
        mut parser: ParserState<'_>,
        errors: &mut ErrSlot<Diagnostic>,
        items: ChargedVec<TopLevelItem>,
        mut out: Out<'slot, '_, SyntaxErrors>,
    ) -> OResult<'slot, SchemaParseOutput> {
        let known_position = ParserControl::eof_span(&parser);
        if let Some(errors) = parser.diagnostics.finish() {
            return Err(out.reborrow().fail(errors));
        }
        let Some(source_len) = parser.actual_source_len else {
            let error = ParserControl::internal_error(
                diagnostic_context!("typol-syntax", "parser", "eof-not-confirmed"),
                known_position,
                parser.translator,
            );
            return Err(out.reborrow().fail(SyntaxErrors::inline(error)));
        };
        let file_span = SourceSpan::new(parser.source_id, Span::from_zero_to(source_len));
        match try_diagnostic(errors, |out| {
            parser.ast.finish_source(source_len, file_span, out)
        }) {
            ParseAttempt::Success(()) => {
                let positions = parser.cursor.into_positions(source_len);
                let ast = SchemaAst::finish(parser.ast.into_inner(), items, file_span);
                Ok(SchemaParseOutput { ast, positions })
            }
            ParseAttempt::Failure(error) => {
                Err(out.reborrow().fail(SyntaxErrors::inline(error.take())))
            }
        }
    }

    fn parse_top_level_items<'slot>(
        parser: &mut ParserState<'_>,
        errors: &mut ErrSlot<Diagnostic>,
        items: &mut ChargedVec<TopLevelItem>,
        mut out: Out<'slot, '_, SyntaxErrors>,
    ) -> OResult<'slot, ()> {
        while !parser.stopped {
            let Some(info) = SchemaParser::peek_top_level_item(parser, errors, out.reborrow())?
            else {
                continue;
            };
            if info.kind == TokenKind::Eof {
                break;
            }

            let failed_at = info.span.span().start();
            let Some(item) = SchemaParser::parse_top_level_item_with_recovery(
                parser,
                errors,
                failed_at,
                out.reborrow(),
            )?
            else {
                continue;
            };
            SchemaParser::push_top_level_item(
                parser,
                errors,
                items,
                TopLevelItemPush {
                    item,
                    span: info.span,
                },
                out.reborrow(),
            )?;
        }
        Ok(())
    }

    fn peek_top_level_item<'slot>(
        parser: &mut ParserState<'_>,
        errors: &mut ErrSlot<Diagnostic>,
        mut out: Out<'slot, '_, SyntaxErrors>,
    ) -> OResult<'slot, Option<context::TokenInfo>> {
        let peek_result = match try_diagnostic(errors, |out| ParserControl::peek(parser, out)) {
            ParseAttempt::Success(info) => Ok(info),
            ParseAttempt::Failure(error) => Err(error.take()),
        };
        match peek_result {
            Ok(info) => Ok(Some(info)),
            Err(diagnostic) => {
                let failed_at = diagnostic
                    .primary()
                    .map(|span| span.span().start())
                    .unwrap_or_else(|| ParserControl::eof_span(parser).span().start());
                SchemaParser::recover_top_level_item(
                    parser,
                    errors,
                    diagnostic,
                    failed_at,
                    out.reborrow(),
                )?;
                Ok(None)
            }
        }
    }

    fn parse_top_level_item_with_recovery<'slot>(
        parser: &mut ParserState<'_>,
        errors: &mut ErrSlot<Diagnostic>,
        failed_at: usize,
        mut out: Out<'slot, '_, SyntaxErrors>,
    ) -> OResult<'slot, Option<TopLevelItem>> {
        let parse_result = match try_diagnostic(errors, |out| {
            SchemaParser::parse_top_level_item(parser, out)
        }) {
            ParseAttempt::Success(item) => Ok(item),
            ParseAttempt::Failure(error) => Err(error.take()),
        };
        match parse_result {
            Ok(item) => Ok(Some(item)),
            Err(diagnostic) => {
                SchemaParser::recover_top_level_item(
                    parser,
                    errors,
                    diagnostic,
                    failed_at,
                    out.reborrow(),
                )?;
                Ok(None)
            }
        }
    }

    fn recover_top_level_item<'slot>(
        parser: &mut ParserState<'_>,
        errors: &mut ErrSlot<Diagnostic>,
        diagnostic: Diagnostic,
        failed_at: usize,
        mut out: Out<'slot, '_, SyntaxErrors>,
    ) -> OResult<'slot, ()> {
        SchemaParser::push_diagnostic(parser, errors, diagnostic, out.reborrow())?;
        let recovery_error = match try_diagnostic(errors, |out| {
            RecoveryParser::recover_top_level(parser, failed_at, out)
        }) {
            ParseAttempt::Success(()) => None,
            ParseAttempt::Failure(error) => Some(error.take()),
        };
        if let Some(error) = recovery_error {
            SchemaParser::push_diagnostic(parser, errors, error, out.reborrow())?;
        }
        Ok(())
    }

    fn push_top_level_item<'slot>(
        parser: &mut ParserState<'_>,
        errors: &mut ErrSlot<Diagnostic>,
        items: &mut ChargedVec<TopLevelItem>,
        request: TopLevelItemPush,
        mut out: Out<'slot, '_, SyntaxErrors>,
    ) -> OResult<'slot, ()> {
        let TopLevelItemPush { item, span } = request;
        let push_error = match try_diagnostic(errors, |out| {
            ParserControl::try_push(parser, items, item, span, out)
        }) {
            ParseAttempt::Success(()) => None,
            ParseAttempt::Failure(error) => Some(error.take()),
        };
        if let Some(error) = push_error {
            SchemaParser::push_diagnostic(parser, errors, error, out.reborrow())?;
        }
        Ok(())
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
            ParseAttempt::Failure(error) => {
                Err(out.reborrow().fail(SyntaxErrors::inline(error.take())))
            }
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
                    t! {
                        name: "diagnostic.syntax.expected_schema_declaration",
                        locale: parser.translator,
                    },
                    out.reborrow(),
                )?;
                Err(out.reborrow().fail(diagnostic))
            }
        }
    }
}
