use alloc::vec::Vec;
use core::mem::size_of;

use crate::{Symbol, token::CommentKind};

use super::{SchemaFormatError, SchemaFormatErrorKind, SchemaFormatOptions};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum LexemeKind {
    Word,
    Number,
    String,
    LineComment,
    BlockComment,
    Symbol(Symbol),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ContainerKind {
    Inline,
    ModuleBody,
    BraceList,
    ArrayList,
    TypeArray,
    TypeAngle,
}

#[derive(Clone, Copy)]
pub(super) struct TokenSpan {
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) kind: LexemeKind,
    pub(super) container: ContainerKind,
    pub(super) pair: Option<usize>,
    pub(super) insert_comma: bool,
}

struct ScanFrame {
    token: usize,
    container: ContainerKind,
    close: Symbol,
    default_value: bool,
}

struct ScanContext<'a> {
    source: &'a str,
    tokens: &'a [TokenSpan],
    frames: &'a [ScanFrame],
    pending_default: bool,
}

pub(super) struct TokenScanner;

fn reserve_geometrically<T>(
    items: &mut Vec<T>,
    other_capacity_bytes: usize,
    workspace_limit: usize,
    capacity_limit: usize,
) -> Result<(), SchemaFormatError> {
    let item_size = size_of::<T>();
    if item_size == 0 {
        return Ok(());
    }

    let required = items
        .len()
        .checked_add(1)
        .ok_or_else(|| SchemaFormatError::new(SchemaFormatErrorKind::ResourceLimitExceeded))?;
    let available_bytes = workspace_limit
        .checked_sub(other_capacity_bytes)
        .ok_or_else(|| SchemaFormatError::new(SchemaFormatErrorKind::ResourceLimitExceeded))?;
    let maximum_capacity = (available_bytes / item_size).min(capacity_limit);
    if required > maximum_capacity {
        return Err(SchemaFormatError::new(
            SchemaFormatErrorKind::ResourceLimitExceeded,
        ));
    }
    if items.capacity() < required {
        let doubled_capacity = items.capacity().saturating_mul(2).max(1);
        let target_capacity = doubled_capacity.max(required).min(maximum_capacity);
        items
            .try_reserve_exact(target_capacity.saturating_sub(items.len()))
            .map_err(|_| SchemaFormatError::new(SchemaFormatErrorKind::AllocationFailed))?;
    }
    Ok(())
}

impl TokenScanner {
    pub(super) fn scan(
        source: &str,
        options: SchemaFormatOptions,
    ) -> Result<Vec<TokenSpan>, SchemaFormatError> {
        scan_tokens_impl(source, options)
    }

    pub(super) fn previous_significant(tokens: &[TokenSpan], before: usize) -> Option<usize> {
        previous_significant(tokens, before)
    }

    pub(super) fn token_text<'source>(
        source: &'source str,
        tokens: &[TokenSpan],
        index: usize,
    ) -> Option<&'source str> {
        token_text(source, tokens, index)
    }

    pub(super) fn inconsistent_source() -> SchemaFormatError {
        inconsistent_source()
    }
}

fn scan_tokens_impl(
    source: &str,
    options: SchemaFormatOptions,
) -> Result<Vec<TokenSpan>, SchemaFormatError> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut frames = Vec::<ScanFrame>::new();
    let mut offset = 0;
    let mut type_alias = false;
    let mut pending_default_depth = None;
    while offset < bytes.len() {
        let byte = bytes.get(offset).copied().ok_or_else(inconsistent_source)?;
        if matches!(byte, b' ' | b'\t' | b'\n') {
            offset += 1;
            continue;
        }
        if byte == b'\r' {
            if bytes.get(offset + 1) != Some(&b'\n') {
                return Err(inconsistent_source());
            }
            offset += 2;
            continue;
        }

        let start = offset;
        let comment = Symbol::comment_kind(byte, bytes.get(offset + 1).copied());
        let kind = if comment == Some(CommentKind::Line) {
            offset += 2;
            while offset < bytes.len() && !matches!(bytes.get(offset), Some(b'\r' | b'\n')) {
                offset += 1;
            }
            LexemeKind::LineComment
        } else if comment == Some(CommentKind::Block) {
            offset += 2;
            loop {
                if offset + 1 >= bytes.len() {
                    return Err(inconsistent_source());
                }
                if bytes.get(offset..offset + 2) == Some(b"*/") {
                    offset += 2;
                    break;
                }
                offset += 1;
            }
            LexemeKind::BlockComment
        } else if byte == b'"' {
            offset += 1;
            loop {
                let current = bytes.get(offset).copied().ok_or_else(inconsistent_source)?;
                offset += 1;
                if current == b'\\' {
                    bytes.get(offset).ok_or_else(inconsistent_source)?;
                    offset += 1;
                } else if current == b'"' {
                    break;
                }
            }
            LexemeKind::String
        } else if byte.is_ascii_alphabetic() || byte == b'_' {
            offset += 1;
            while bytes
                .get(offset)
                .is_some_and(|value| Symbol::identifier_continue(char::from(*value)))
            {
                offset += 1;
            }
            LexemeKind::Word
        } else if byte.is_ascii_digit() {
            offset += 1;
            while bytes.get(offset).is_some_and(|value| {
                Symbol::identifier_continue(char::from(*value)) || *value == b'.'
            }) {
                offset += 1;
            }
            LexemeKind::Number
        } else {
            let (symbol, width) = scan_symbol(bytes, offset)?;
            offset = offset.checked_add(width).ok_or_else(|| {
                SchemaFormatError::new(SchemaFormatErrorKind::ResourceLimitExceeded)
            })?;
            LexemeKind::Symbol(symbol)
        };
        let container = match kind {
            LexemeKind::Symbol(Symbol::LeftBrace | Symbol::LeftBracket | Symbol::LeftParen) => {
                classify_container(
                    ScanContext {
                        source,
                        tokens: &tokens,
                        frames: &frames,
                        pending_default: pending_default_depth == Some(frames.len()),
                    },
                    kind,
                    offset,
                    type_alias,
                )
            }
            LexemeKind::Symbol(Symbol::Less) => {
                if previous_word_is(source, &tokens, "option")
                    || previous_word_is(source, &tokens, "map")
                {
                    ContainerKind::TypeAngle
                } else {
                    ContainerKind::Inline
                }
            }
            _ => ContainerKind::Inline,
        };
        if let LexemeKind::Word = kind {
            let word = source.get(start..offset).ok_or_else(inconsistent_source)?;
            if word == "type" {
                type_alias = true;
            }
        }

        let token_index = tokens.len();
        reserve_geometrically(
            &mut tokens,
            frames
                .capacity()
                .checked_mul(size_of::<ScanFrame>())
                .ok_or_else(|| {
                    SchemaFormatError::new(SchemaFormatErrorKind::ResourceLimitExceeded)
                })?,
            options.max_workspace_bytes(),
            usize::MAX,
        )?;
        tokens.push(TokenSpan {
            start,
            end: offset,
            kind,
            container,
            pair: None,
            insert_comma: false,
        });

        if let LexemeKind::Symbol(symbol) = kind {
            if let Some(close) = opening_close(symbol, container) {
                if frames.len() >= options.max_nesting_depth() {
                    return Err(SchemaFormatError::new(
                        SchemaFormatErrorKind::ResourceLimitExceeded,
                    ));
                }
                reserve_geometrically(
                    &mut frames,
                    tokens
                        .capacity()
                        .checked_mul(size_of::<TokenSpan>())
                        .ok_or_else(|| {
                            SchemaFormatError::new(SchemaFormatErrorKind::ResourceLimitExceeded)
                        })?,
                    options.max_workspace_bytes(),
                    options.max_nesting_depth(),
                )?;
                frames.push(ScanFrame {
                    token: token_index,
                    container,
                    close,
                    default_value: pending_default_depth == Some(frames.len())
                        || frames.last().is_some_and(|frame| frame.default_value),
                });
                if pending_default_depth == Some(frames.len() - 1) {
                    pending_default_depth = None;
                }
            } else if let Some(close) = closing_symbol(symbol, &frames) {
                let Some(frame) = frames.pop() else {
                    return Err(inconsistent_source());
                };
                if frame.close != close {
                    return Err(inconsistent_source());
                }
                let open_token = tokens
                    .get_mut(frame.token)
                    .ok_or_else(inconsistent_source)?;
                open_token.pair = Some(token_index);
                let close_token = tokens
                    .get_mut(token_index)
                    .ok_or_else(inconsistent_source)?;
                close_token.pair = Some(frame.token);
                close_token.container = frame.container;
                if symbol == Symbol::Semicolon
                    && !frames.iter().any(|frame| {
                        matches!(
                            frame.container,
                            ContainerKind::TypeArray | ContainerKind::TypeAngle
                        )
                    })
                {
                    type_alias = false;
                }
            } else if symbol == Symbol::Semicolon
                && !frames.iter().any(|frame| {
                    matches!(
                        frame.container,
                        ContainerKind::TypeArray | ContainerKind::TypeAngle
                    )
                })
            {
                type_alias = false;
            }
            if symbol == Symbol::Equal
                && !type_alias
                && !frames.iter().any(|frame| {
                    matches!(
                        frame.container,
                        ContainerKind::TypeArray | ContainerKind::TypeAngle
                    )
                })
            {
                pending_default_depth = Some(frames.len());
            } else if matches!(symbol, Symbol::Comma | Symbol::Semicolon)
                && pending_default_depth == Some(frames.len())
            {
                pending_default_depth = None;
            }
        }
    }
    if !frames.is_empty() {
        return Err(inconsistent_source());
    }
    add_trailing_commas(&mut tokens)?;
    Ok(tokens)
}

fn classify_container(
    context: ScanContext<'_>,
    kind: LexemeKind,
    open_end: usize,
    type_alias: bool,
) -> ContainerKind {
    let ScanContext {
        source,
        tokens,
        frames,
        pending_default,
    } = context;
    let LexemeKind::Symbol(symbol) = kind else {
        return ContainerKind::Inline;
    };
    let previous = previous_significant(tokens, tokens.len());
    let parent = frames.last().map(|frame| frame.container);
    let empty = next_non_space_is_close(source, open_end, symbol);
    match symbol {
        Symbol::LeftParen => ContainerKind::Inline,
        Symbol::LeftBrace => {
            if empty {
                return ContainerKind::Inline;
            }
            if previous_previous_word_is(source, tokens, "mod") {
                ContainerKind::ModuleBody
            } else {
                ContainerKind::BraceList
            }
        }
        Symbol::LeftBracket => {
            if pending_default || frames.iter().any(|frame| frame.default_value) {
                ContainerKind::ArrayList
            } else if parent.is_some_and(|container| {
                matches!(
                    container,
                    ContainerKind::TypeArray | ContainerKind::TypeAngle
                )
            }) || previous.is_some_and(|index| {
                tokens
                    .get(index)
                    .is_some_and(|token| token.kind == LexemeKind::Symbol(Symbol::Colon))
            }) || (type_alias
                && previous.is_some_and(|index| {
                    tokens
                        .get(index)
                        .is_some_and(|token| token.kind == LexemeKind::Symbol(Symbol::Equal))
                }))
            {
                ContainerKind::TypeArray
            } else if empty {
                ContainerKind::Inline
            } else {
                ContainerKind::ArrayList
            }
        }
        _ => ContainerKind::Inline,
    }
}

fn add_trailing_commas(tokens: &mut [TokenSpan]) -> Result<(), SchemaFormatError> {
    let mut index = 0;
    while index < tokens.len() {
        let Some(token) = tokens.get(index).copied() else {
            return Err(inconsistent_source());
        };
        if !matches!(
            token.container,
            ContainerKind::BraceList | ContainerKind::ArrayList
        ) {
            index += 1;
            continue;
        }
        let close = token.pair.ok_or_else(inconsistent_source)?;
        tokens.get(close).ok_or_else(inconsistent_source)?;
        if close == index + 1 {
            index += 1;
            continue;
        }
        let mut last = close;
        while last > index + 1 {
            let candidate_index = last - 1;
            let candidate = tokens
                .get(candidate_index)
                .ok_or_else(inconsistent_source)?;
            if matches!(
                candidate.kind,
                LexemeKind::LineComment | LexemeKind::BlockComment
            ) {
                last = candidate_index;
                continue;
            }
            if candidate.kind == LexemeKind::Symbol(Symbol::Comma) {
                break;
            }
            let token = tokens
                .get_mut(candidate_index)
                .ok_or_else(inconsistent_source)?;
            token.insert_comma = true;
            break;
        }
        index += 1;
    }
    Ok(())
}

fn previous_significant(tokens: &[TokenSpan], before: usize) -> Option<usize> {
    let mut index = before;
    while index > 0 {
        index -= 1;
        let token = tokens.get(index)?;
        if !matches!(
            token.kind,
            LexemeKind::LineComment | LexemeKind::BlockComment
        ) {
            return Some(index);
        }
    }
    None
}

fn previous_word_is(source: &str, tokens: &[TokenSpan], expected: &str) -> bool {
    previous_significant(tokens, tokens.len())
        .is_some_and(|index| token_text(source, tokens, index) == Some(expected))
}

fn previous_previous_word_is(source: &str, tokens: &[TokenSpan], expected: &str) -> bool {
    let Some(previous) = previous_significant(tokens, tokens.len()) else {
        return false;
    };
    previous_significant(tokens, previous)
        .is_some_and(|index| token_text(source, tokens, index) == Some(expected))
}

fn token_text<'source>(
    source: &'source str,
    tokens: &[TokenSpan],
    index: usize,
) -> Option<&'source str> {
    let token = tokens.get(index)?;
    source.get(token.start..token.end)
}

fn next_non_space_is_close(source: &str, mut offset: usize, open: Symbol) -> bool {
    let close = match open {
        Symbol::LeftBrace => b'}',
        Symbol::LeftBracket => b']',
        Symbol::LeftParen => b')',
        _ => return false,
    };
    let bytes = source.as_bytes();
    while bytes
        .get(offset)
        .is_some_and(|byte| matches!(*byte, b' ' | b'\t' | b'\r' | b'\n'))
    {
        offset += 1;
    }
    bytes.get(offset) == Some(&close)
}

fn opening_close(symbol: Symbol, container: ContainerKind) -> Option<Symbol> {
    match symbol {
        Symbol::LeftParen => Some(Symbol::RightParen),
        Symbol::LeftBrace => Some(Symbol::RightBrace),
        Symbol::LeftBracket => Some(Symbol::RightBracket),
        Symbol::Less if container == ContainerKind::TypeAngle => Some(Symbol::Greater),
        _ => None,
    }
}

fn closing_symbol(symbol: Symbol, frames: &[ScanFrame]) -> Option<Symbol> {
    match symbol {
        Symbol::RightParen | Symbol::RightBrace | Symbol::RightBracket => Some(match symbol {
            Symbol::RightParen => Symbol::RightParen,
            Symbol::RightBrace => Symbol::RightBrace,
            Symbol::RightBracket => Symbol::RightBracket,
            _ => return None,
        }),
        Symbol::Greater
            if frames
                .last()
                .is_some_and(|frame| frame.container == ContainerKind::TypeAngle) =>
        {
            Some(Symbol::Greater)
        }
        _ => None,
    }
}

fn scan_symbol(bytes: &[u8], offset: usize) -> Result<(Symbol, usize), SchemaFormatError> {
    let byte = bytes.get(offset).copied().ok_or_else(inconsistent_source)?;
    let next = bytes.get(offset + 1).copied();
    Symbol::from_ascii(byte, next).ok_or_else(inconsistent_source)
}

fn inconsistent_source() -> SchemaFormatError {
    SchemaFormatError::new(SchemaFormatErrorKind::InconsistentSource)
}
