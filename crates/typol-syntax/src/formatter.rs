mod scan;

use alloc::{string::String, vec::Vec};
use core::{error::Error, fmt, mem::size_of};
use opeo::{OResult, Out};

use self::scan::{ContainerKind, LexemeKind, TokenScanner, TokenSpan};

use crate::{SchemaAst, token::Symbol};

const INDENT_WIDTH: usize = 4;
const INDENT_SPACES: &str = "                                                                ";

/// A successfully parsed Schema AST bound to the exact source text it came from.
pub struct SchemaDocument<'source> {
    source: &'source str,
    ast: SchemaAst,
    source_limit: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchemaFormatErrorKind {
    ResourceLimitExceeded,
    AllocationFailed,
    InconsistentSource,
    OutputSinkFailure,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchemaFormatError {
    kind: SchemaFormatErrorKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchemaFormatOptions {
    max_output_bytes: usize,
    max_workspace_bytes: usize,
    max_nesting_depth: usize,
}

#[derive(Debug)]
pub struct FormattedSchema {
    text: String,
}

pub struct SchemaFormatter;

struct FormatFrame {
    container: ContainerKind,
    has_declaration: bool,
}

struct OutputBuffer {
    text: String,
    limit: usize,
    newline: &'static str,
}

impl<'source> SchemaDocument<'source> {
    pub(crate) const fn new(source: &'source str, ast: SchemaAst, source_limit: usize) -> Self {
        Self {
            source,
            ast,
            source_limit,
        }
    }

    pub const fn source(&self) -> &'source str {
        self.source
    }

    pub const fn ast(&self) -> &SchemaAst {
        &self.ast
    }

    pub const fn source_limit(&self) -> usize {
        self.source_limit
    }
}

impl SchemaFormatError {
    pub(crate) const fn new(kind: SchemaFormatErrorKind) -> Self {
        Self { kind }
    }

    pub const fn kind(self) -> SchemaFormatErrorKind {
        self.kind
    }
}

impl fmt::Display for SchemaFormatError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.kind {
            SchemaFormatErrorKind::ResourceLimitExceeded => {
                "schema formatting resource limit exceeded"
            }
            SchemaFormatErrorKind::AllocationFailed => "schema formatting allocation failed",
            SchemaFormatErrorKind::InconsistentSource => {
                "schema formatting source structure is inconsistent"
            }
            SchemaFormatErrorKind::OutputSinkFailure => "schema formatting output sink failed",
        })
    }
}

impl Error for SchemaFormatError {}

impl SchemaFormatOptions {
    pub const fn new(
        max_output_bytes: usize,
        max_workspace_bytes: usize,
        max_nesting_depth: usize,
    ) -> Self {
        Self {
            max_output_bytes,
            max_workspace_bytes,
            max_nesting_depth,
        }
    }

    pub const fn max_output_bytes(&self) -> usize {
        self.max_output_bytes
    }

    pub const fn max_workspace_bytes(&self) -> usize {
        self.max_workspace_bytes
    }

    pub const fn max_nesting_depth(&self) -> usize {
        self.max_nesting_depth
    }

    pub const fn with_max_output_bytes(mut self, value: usize) -> Self {
        self.max_output_bytes = value;
        self
    }

    pub const fn with_max_workspace_bytes(mut self, value: usize) -> Self {
        self.max_workspace_bytes = value;
        self
    }

    pub const fn with_max_nesting_depth(mut self, value: usize) -> Self {
        self.max_nesting_depth = value;
        self
    }
}

impl Default for SchemaFormatOptions {
    fn default() -> Self {
        Self::new(1_048_576, 4_194_304, 128)
    }
}

impl FormattedSchema {
    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn into_string(self) -> String {
        self.text
    }

    pub fn write_to(self, output: &mut impl fmt::Write) -> Result<(), SchemaFormatError> {
        output
            .write_str(&self.text)
            .map_err(|_| SchemaFormatError::new(SchemaFormatErrorKind::OutputSinkFailure))
    }
}

impl SchemaFormatter {
    /// Produces a canonical representation while preserving every source token and comment.
    pub fn format<'slot>(
        document: SchemaDocument<'_>,
        options: SchemaFormatOptions,
        out: Out<'slot, '_, SchemaFormatError>,
    ) -> OResult<'slot, FormattedSchema> {
        let SchemaDocument {
            source,
            ast,
            source_limit,
        } = document;
        drop(ast);
        match Self::format_source(source, source_limit, options) {
            Ok(formatted) => Ok(formatted),
            Err(error) => Err(out.fail(error)),
        }
    }

    fn format_source(
        source: &str,
        source_limit: usize,
        options: SchemaFormatOptions,
    ) -> Result<FormattedSchema, SchemaFormatError> {
        if source.len() > source_limit {
            return Err(SchemaFormatError::new(
                SchemaFormatErrorKind::ResourceLimitExceeded,
            ));
        }
        let newline = if source.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        };
        let tokens = TokenScanner::scan(source, options)?;
        let text = render_tokens(source, &tokens, tokens.capacity(), options, newline)?;
        Ok(FormattedSchema { text })
    }
}

fn reserve_format_frame(
    frames: &mut Vec<FormatFrame>,
    token_capacity: usize,
    options: SchemaFormatOptions,
) -> Result<(), SchemaFormatError> {
    let required = frames
        .len()
        .checked_add(1)
        .ok_or_else(|| SchemaFormatError::new(SchemaFormatErrorKind::ResourceLimitExceeded))?;
    let token_bytes = token_capacity
        .checked_mul(size_of::<TokenSpan>())
        .ok_or_else(|| SchemaFormatError::new(SchemaFormatErrorKind::ResourceLimitExceeded))?;
    let available_bytes = options
        .max_workspace_bytes()
        .checked_sub(token_bytes)
        .ok_or_else(|| SchemaFormatError::new(SchemaFormatErrorKind::ResourceLimitExceeded))?;
    let maximum_capacity =
        (available_bytes / size_of::<FormatFrame>()).min(options.max_nesting_depth());
    if required > maximum_capacity {
        return Err(SchemaFormatError::new(
            SchemaFormatErrorKind::ResourceLimitExceeded,
        ));
    }
    if frames.capacity() < required {
        let doubled_capacity = frames.capacity().saturating_mul(2).max(1);
        let target_capacity = doubled_capacity.max(required).min(maximum_capacity);
        frames
            .try_reserve_exact(target_capacity.saturating_sub(frames.len()))
            .map_err(|_| SchemaFormatError::new(SchemaFormatErrorKind::AllocationFailed))?;
    }
    Ok(())
}

impl OutputBuffer {
    fn new(limit: usize, newline: &'static str) -> Self {
        Self {
            text: String::new(),
            limit,
            newline,
        }
    }

    fn append(&mut self, text: &str) -> Result<(), SchemaFormatError> {
        let length = self
            .text
            .len()
            .checked_add(text.len())
            .filter(|length| *length <= self.limit)
            .ok_or_else(|| SchemaFormatError::new(SchemaFormatErrorKind::ResourceLimitExceeded))?;
        if self.text.capacity() < length {
            let doubled_capacity = self.text.capacity().saturating_mul(2).max(1);
            let target_capacity = doubled_capacity.max(length).min(self.limit);
            self.text
                .try_reserve_exact(target_capacity.saturating_sub(self.text.len()))
                .map_err(|_| SchemaFormatError::new(SchemaFormatErrorKind::AllocationFailed))?;
        }
        self.text.push_str(text);
        Ok(())
    }

    fn append_normalized_line_endings(&mut self, text: &str) -> Result<(), SchemaFormatError> {
        let bytes = text.as_bytes();
        let mut segment_start = 0;
        let mut offset = 0;
        while offset < bytes.len() {
            if !matches!(bytes.get(offset), Some(b'\r' | b'\n')) {
                offset += 1;
                continue;
            }
            self.append(
                text.get(segment_start..offset)
                    .ok_or_else(TokenScanner::inconsistent_source)?,
            )?;
            self.append(self.newline)?;
            if bytes.get(offset) == Some(&b'\r') && bytes.get(offset + 1) == Some(&b'\n') {
                offset += 2;
            } else {
                offset += 1;
            }
            segment_start = offset;
        }
        self.append(
            text.get(segment_start..)
                .ok_or_else(TokenScanner::inconsistent_source)?,
        )
    }

    fn is_line_start(&self) -> bool {
        self.text.ends_with('\n') || self.text.ends_with('\r')
    }

    fn ends_with_space(&self) -> bool {
        self.text.ends_with(' ')
    }

    fn line_breaks(&self) -> usize {
        let bytes = self.text.as_bytes();
        let newline = self.newline.as_bytes();
        let mut count = 0;
        let mut end = bytes.len();
        while end >= newline.len() {
            let Some(start) = end.checked_sub(newline.len()) else {
                break;
            };
            if bytes.get(start..end) != Some(newline) {
                break;
            }
            count += 1;
            end = start;
        }
        count
    }

    fn ensure_line_breaks(&mut self, count: usize) -> Result<(), SchemaFormatError> {
        let existing = self.line_breaks();
        for _ in existing..count {
            self.append(self.newline)?;
        }
        Ok(())
    }

    fn indent(&mut self, level: usize) -> Result<(), SchemaFormatError> {
        if self.is_line_start() {
            let mut spaces = level.checked_mul(INDENT_WIDTH).ok_or_else(|| {
                SchemaFormatError::new(SchemaFormatErrorKind::ResourceLimitExceeded)
            })?;
            while spaces > 0 {
                let chunk = spaces.min(64);
                self.append(&INDENT_SPACES[..chunk])?;
                spaces -= chunk;
            }
        }
        Ok(())
    }
}

fn render_tokens(
    source: &str,
    tokens: &[TokenSpan],
    token_capacity: usize,
    options: SchemaFormatOptions,
    newline: &'static str,
) -> Result<String, SchemaFormatError> {
    let mut output = OutputBuffer::new(options.max_output_bytes(), newline);
    let mut frames = Vec::<FormatFrame>::new();
    let mut root_has_declaration = false;
    let mut previous = None;
    let mut index = 0;
    while index < tokens.len() {
        let token = tokens
            .get(index)
            .ok_or_else(TokenScanner::inconsistent_source)?;
        let raw = source
            .get(token.start..token.end)
            .ok_or_else(TokenScanner::inconsistent_source)?;
        if is_declaration_start(source, tokens, index, &frames) {
            let has_declaration = if let Some(frame) = frames.last_mut() {
                frame.has_declaration
            } else {
                root_has_declaration
            };
            if has_declaration {
                output.ensure_line_breaks(2)?;
            }
            if let Some(frame) = frames.last_mut() {
                frame.has_declaration = true;
            } else {
                root_has_declaration = true;
            }
        }

        if is_closing(token) {
            let frame = frames.pop().ok_or_else(TokenScanner::inconsistent_source)?;
            if frame.container != token.container {
                return Err(TokenScanner::inconsistent_source());
            }
            if is_expanded(frame.container) {
                output.ensure_line_breaks(1)?;
                output.indent(indent_level(&frames))?;
            } else {
                write_separator(tokens, &mut output, previous, index, &frames)?;
            }
            output.append(raw)?;
            if token.insert_comma {
                output.append(",")?;
                if frames
                    .last()
                    .is_some_and(|frame| is_expanded(frame.container))
                {
                    output.ensure_line_breaks(1)?;
                }
            }
            previous = Some(index);
            index += 1;
            continue;
        }

        match token.kind {
            LexemeKind::LineComment => {
                write_separator(tokens, &mut output, previous, index, &frames)?;
                output.indent(indent_level(&frames))?;
                output.append(raw)?;
                output.ensure_line_breaks(1)?;
                previous = None;
            }
            LexemeKind::BlockComment => {
                write_separator(tokens, &mut output, previous, index, &frames)?;
                output.indent(indent_level(&frames))?;
                output.append_normalized_line_endings(raw)?;
                previous = Some(index);
            }
            LexemeKind::Symbol(
                Symbol::LeftBrace | Symbol::LeftBracket | Symbol::LeftParen | Symbol::Less,
            ) if token.pair.is_some() => {
                write_separator(tokens, &mut output, previous, index, &frames)?;
                output.indent(indent_level(&frames))?;
                output.append(raw)?;
                if frames.len() >= options.max_nesting_depth() {
                    return Err(SchemaFormatError::new(
                        SchemaFormatErrorKind::ResourceLimitExceeded,
                    ));
                }
                reserve_format_frame(&mut frames, token_capacity, options)?;
                frames.push(FormatFrame {
                    container: token.container,
                    has_declaration: false,
                });
                if is_expanded(token.container) {
                    output.ensure_line_breaks(1)?;
                }
                previous = Some(index);
            }
            LexemeKind::Symbol(Symbol::Comma) => {
                output.append(raw)?;
                if frames
                    .last()
                    .is_some_and(|frame| is_expanded(frame.container))
                {
                    output.ensure_line_breaks(1)?;
                } else {
                    output.append(" ")?;
                }
                previous = Some(index);
            }
            LexemeKind::Symbol(Symbol::Semicolon) => {
                output.append(raw)?;
                if !frames
                    .last()
                    .is_some_and(|frame| frame.container == ContainerKind::TypeArray)
                {
                    output.ensure_line_breaks(1)?;
                }
                previous = Some(index);
            }
            _ => {
                write_separator(tokens, &mut output, previous, index, &frames)?;
                output.indent(indent_level(&frames))?;
                output.append(raw)?;
                previous = Some(index);
            }
        }
        if token.insert_comma {
            output.append(",")?;
            if frames
                .last()
                .is_some_and(|frame| is_expanded(frame.container))
            {
                output.ensure_line_breaks(1)?;
            }
            previous = Some(index);
        }
        index += 1;
    }
    output.ensure_line_breaks(1)?;
    Ok(output.text)
}

fn write_separator(
    tokens: &[TokenSpan],
    output: &mut OutputBuffer,
    previous: Option<usize>,
    current_index: usize,
    frames: &[FormatFrame],
) -> Result<(), SchemaFormatError> {
    let Some(previous_index) = previous else {
        return Ok(());
    };
    if output.is_line_start() {
        return Ok(());
    }
    let previous = tokens
        .get(previous_index)
        .ok_or_else(TokenScanner::inconsistent_source)?;
    let current = tokens
        .get(current_index)
        .ok_or_else(TokenScanner::inconsistent_source)?;
    let previous_unary = is_unary_minus(tokens, previous_index);
    let current_unary = is_unary_minus(tokens, current_index);
    if !output.ends_with_space()
        && needs_space_before(previous, current, frames, previous_unary, current_unary)
    {
        output.append(" ")?;
    }
    Ok(())
}

fn needs_space_before(
    previous: &TokenSpan,
    current: &TokenSpan,
    frames: &[FormatFrame],
    previous_unary: bool,
    current_unary: bool,
) -> bool {
    if matches!(
        current.kind,
        LexemeKind::LineComment | LexemeKind::BlockComment
    ) {
        return true;
    }
    let previous_symbol = symbol(previous.kind);
    if current_unary {
        return match previous_symbol {
            Some(Symbol::LeftParen | Symbol::LeftBracket | Symbol::LeftBrace) => false,
            Some(_) => true,
            None => matches!(previous.kind, LexemeKind::BlockComment),
        };
    }
    if previous_unary {
        return false;
    }
    if let Some(current_symbol) = symbol(current.kind) {
        return match current_symbol {
            Symbol::RightParen
            | Symbol::RightBrace
            | Symbol::RightBracket
            | Symbol::Comma
            | Symbol::Semicolon
            | Symbol::Colon
            | Symbol::PathSeparator => false,
            Symbol::Dot => previous_symbol.is_some_and(is_operator),
            Symbol::At => true,
            Symbol::LeftParen => false,
            Symbol::LeftBracket => {
                matches!(previous_symbol, Some(Symbol::Equal | Symbol::Colon))
                    || (previous_symbol == Some(Symbol::Comma)
                        && frames
                            .last()
                            .is_some_and(|frame| frame.container == ContainerKind::TypeAngle))
            }
            Symbol::LeftBrace => {
                previous_symbol.is_some_and(is_operator)
                    || previous_symbol == Some(Symbol::Colon)
                    || is_wordlike(previous.kind)
                    || is_closing(previous)
            }
            Symbol::Less if current.container == ContainerKind::TypeAngle => false,
            Symbol::Greater if current.container == ContainerKind::TypeAngle => false,
            Symbol::Bang => previous_symbol.is_some_and(is_operator),
            Symbol::Pipe => false,
            Symbol::Equal
            | Symbol::EqualEqual
            | Symbol::NotEqual
            | Symbol::Less
            | Symbol::LessEqual
            | Symbol::Greater
            | Symbol::GreaterEqual
            | Symbol::AndAnd
            | Symbol::OrOr
            | Symbol::Plus
            | Symbol::Minus
            | Symbol::Star
            | Symbol::Slash => !matches!(previous_symbol, Some(Symbol::LeftParen | Symbol::Comma)),
        };
    }
    if previous_symbol == Some(Symbol::Less) && previous.container == ContainerKind::TypeAngle {
        return false;
    }
    if previous_symbol.is_some_and(is_operator)
        || matches!(
            previous_symbol,
            Some(Symbol::Colon | Symbol::Comma | Symbol::Equal)
        )
    {
        return true;
    }
    if matches!(
        previous_symbol,
        Some(Symbol::At | Symbol::Dot | Symbol::PathSeparator | Symbol::Bang)
    ) {
        return false;
    }
    if previous_symbol == Some(Symbol::Semicolon)
        && frames
            .last()
            .is_some_and(|frame| frame.container == ContainerKind::TypeArray)
    {
        return true;
    }
    matches!(previous.kind, LexemeKind::BlockComment)
        || is_wordlike(previous.kind)
        || is_closing(previous)
}

fn is_unary_minus(tokens: &[TokenSpan], index: usize) -> bool {
    let Some(token) = tokens.get(index) else {
        return false;
    };
    if token.kind != LexemeKind::Symbol(Symbol::Minus) {
        return false;
    }
    let Some(previous) = TokenScanner::previous_significant(tokens, index) else {
        return true;
    };
    let Some(previous) = tokens.get(previous) else {
        return false;
    };
    matches!(
        symbol(previous.kind),
        Some(
            Symbol::LeftParen
                | Symbol::LeftBrace
                | Symbol::LeftBracket
                | Symbol::Comma
                | Symbol::Colon
                | Symbol::Equal
                | Symbol::EqualEqual
                | Symbol::NotEqual
                | Symbol::Less
                | Symbol::LessEqual
                | Symbol::Greater
                | Symbol::GreaterEqual
                | Symbol::AndAnd
                | Symbol::OrOr
                | Symbol::Plus
                | Symbol::Minus
                | Symbol::Star
                | Symbol::Slash
                | Symbol::Bang
        )
    )
}

fn is_operator(symbol: Symbol) -> bool {
    matches!(
        symbol,
        Symbol::Equal
            | Symbol::EqualEqual
            | Symbol::NotEqual
            | Symbol::Less
            | Symbol::LessEqual
            | Symbol::Greater
            | Symbol::GreaterEqual
            | Symbol::AndAnd
            | Symbol::OrOr
            | Symbol::Plus
            | Symbol::Minus
            | Symbol::Star
            | Symbol::Slash
    )
}

fn is_declaration_start(
    source: &str,
    tokens: &[TokenSpan],
    index: usize,
    frames: &[FormatFrame],
) -> bool {
    if !frames.is_empty()
        && !frames
            .last()
            .is_some_and(|frame| frame.container == ContainerKind::ModuleBody)
    {
        return false;
    }
    let Some(token) = tokens.get(index) else {
        return false;
    };
    let Some(word) = source.get(token.start..token.end) else {
        return false;
    };
    if !matches!(
        word,
        "extern" | "mod" | "type" | "struct" | "entry" | "enum"
    ) {
        return false;
    }
    let previous = TokenScanner::previous_significant(tokens, index);
    if word == "mod"
        && previous.is_some_and(|previous| {
            TokenScanner::token_text(source, tokens, previous) == Some("extern")
        })
    {
        return false;
    }
    if word == "struct"
        && previous.is_some_and(|previous| {
            TokenScanner::token_text(source, tokens, previous) == Some("entry")
        })
    {
        return false;
    }
    true
}

fn indent_level(frames: &[FormatFrame]) -> usize {
    frames
        .iter()
        .filter(|frame| is_expanded(frame.container))
        .count()
}

fn is_expanded(container: ContainerKind) -> bool {
    matches!(
        container,
        ContainerKind::ModuleBody | ContainerKind::BraceList | ContainerKind::ArrayList
    )
}

fn is_closing(token: &TokenSpan) -> bool {
    token.pair.is_some()
        && matches!(
            token.kind,
            LexemeKind::Symbol(
                Symbol::RightParen | Symbol::RightBrace | Symbol::RightBracket | Symbol::Greater
            )
        )
}

fn symbol(kind: LexemeKind) -> Option<Symbol> {
    if let LexemeKind::Symbol(symbol) = kind {
        Some(symbol)
    } else {
        None
    }
}

fn is_wordlike(kind: LexemeKind) -> bool {
    matches!(
        kind,
        LexemeKind::Word | LexemeKind::Number | LexemeKind::String
    )
}
