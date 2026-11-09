use alloc::vec::Vec;
use core::mem::size_of;
use opeo::{ErrSlot, OResult, Out};

use crate::{
    AstAccessError, BracedDefault, Declaration, DefaultValue, DefaultValueKind,
    EnumDefaultPayloadKind, FieldDeclList, SchemaAst, StructDecl, Symbol, TopLevelItem, TypeExpr,
    TypeExprKind, VariantPayloadKind, token::CommentKind,
};
use typol_core::SourceSpan;

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
    EnumPayload,
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
}

struct AstContext<'tokens> {
    tokens: &'tokens mut [TokenSpan],
    consistent: bool,
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
        ast: &SchemaAst,
        options: SchemaFormatOptions,
    ) -> Result<Vec<TokenSpan>, SchemaFormatError> {
        scan_tokens_impl(source, ast, options)
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
    ast: &SchemaAst,
    options: SchemaFormatOptions,
) -> Result<Vec<TokenSpan>, SchemaFormatError> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut frames = Vec::<ScanFrame>::new();
    let mut offset = 0;
    let ast_bytes = ast
        .memory_bytes()
        .ok_or_else(|| SchemaFormatError::new(SchemaFormatErrorKind::ResourceLimitExceeded))?;
    if ast_bytes > options.max_workspace_bytes() {
        return Err(SchemaFormatError::new(
            SchemaFormatErrorKind::ResourceLimitExceeded,
        ));
    }
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
            offset = scan_string(bytes, offset)?;
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
            LexemeKind::Symbol(Symbol::Less)
                if previous_word_is(source, &tokens, "option")
                    || previous_word_is(source, &tokens, "map") =>
            {
                ContainerKind::TypeAngle
            }
            _ => ContainerKind::Inline,
        };

        let token_index = tokens.len();
        reserve_geometrically(
            &mut tokens,
            ast_bytes
                .checked_add(
                    frames
                        .capacity()
                        .checked_mul(size_of::<ScanFrame>())
                        .ok_or_else(|| {
                            SchemaFormatError::new(SchemaFormatErrorKind::ResourceLimitExceeded)
                        })?,
                )
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
                    ast_bytes
                        .checked_add(
                            tokens
                                .capacity()
                                .checked_mul(size_of::<TokenSpan>())
                                .ok_or_else(|| {
                                    SchemaFormatError::new(
                                        SchemaFormatErrorKind::ResourceLimitExceeded,
                                    )
                                })?,
                        )
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
                });
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
            }
        }
    }
    if !frames.is_empty() {
        return Err(inconsistent_source());
    }
    let mut access_errors = ErrSlot::<AstAccessError>::new();
    let mut context = AstContext {
        tokens: &mut tokens,
        consistent: true,
    };
    if access_errors
        .try_call(|out| context.apply(ast, out))
        .is_err()
        || !context.consistent
    {
        return Err(inconsistent_source());
    }
    add_trailing_commas(&mut tokens)?;
    Ok(tokens)
}

impl AstContext<'_> {
    fn apply<'slot>(
        &mut self,
        ast: &SchemaAst,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, ()> {
        let items = ast.items();
        for index in 0..items.len() {
            let Some(item) = items.get(index, out.reborrow())? else {
                return Err(out.fail(AstAccessError::InvalidNodeId { id: index }));
            };
            self.visit_item(item, out.reborrow())?;
        }
        Ok(())
    }

    fn visit_item<'slot>(
        &mut self,
        item: TopLevelItem<'_>,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, ()> {
        match item {
            TopLevelItem::ExternMod(_) => Ok(()),
            TopLevelItem::Declaration(declaration) => {
                self.visit_declaration(declaration, out.reborrow())
            }
            TopLevelItem::EntryStruct(item) => self.visit_struct(item, out.reborrow()),
        }
    }

    fn visit_declaration<'slot>(
        &mut self,
        declaration: Declaration<'_>,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, ()> {
        match declaration {
            Declaration::Module(module) => {
                let name = module.name_node(out.reborrow())?;
                self.mark_search(
                    name.span().span().end(),
                    module.span().span().end(),
                    Symbol::LeftBrace,
                    ContainerKind::ModuleBody,
                );
                let declarations = module.declarations(out.reborrow())?;
                for index in 0..declarations.len() {
                    let Some(declaration) = declarations.get(index, out.reborrow())? else {
                        return Err(out.fail(AstAccessError::InvalidNodeId { id: index }));
                    };
                    self.visit_declaration(declaration, out.reborrow())?;
                }
                Ok(())
            }
            Declaration::TypeAlias(alias) => {
                let target = alias.target(out.reborrow())?;
                self.visit_type(target, out.reborrow())
            }
            Declaration::Struct(item) => self.visit_struct(item, out.reborrow()),
            Declaration::Enum(item) => {
                let name = item.name_node(out.reborrow())?;
                self.mark_search(
                    name.span().span().end(),
                    item.span().span().end(),
                    Symbol::LeftBrace,
                    ContainerKind::BraceList,
                );
                let variants = item.variants(out.reborrow())?;
                for index in 0..variants.len() {
                    let Some(variant) = variants.get(index, out.reborrow())? else {
                        return Err(out.fail(AstAccessError::InvalidNodeId { id: index }));
                    };
                    let payload = variant.payload(out.reborrow())?;
                    let payload_span = payload.span();
                    match payload.kind(out.reborrow())? {
                        VariantPayloadKind::Unit => {}
                        VariantPayloadKind::Single(payload) => {
                            let ty = payload.ty(out.reborrow())?;
                            self.visit_type(ty, out.reborrow())?;
                        }
                        VariantPayloadKind::Structure(payload) => {
                            self.mark_exact(
                                payload_span,
                                Symbol::LeftBrace,
                                ContainerKind::BraceList,
                            );
                            self.visit_fields(payload.fields(out.reborrow())?, out.reborrow())?;
                        }
                    }
                }
                Ok(())
            }
        }
    }

    fn visit_struct<'slot>(
        &mut self,
        item: StructDecl<'_>,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, ()> {
        let name = item.name_node(out.reborrow())?;
        self.mark_search(
            name.span().span().end(),
            item.span().span().end(),
            Symbol::LeftBrace,
            ContainerKind::BraceList,
        );
        self.visit_fields(item.fields(out.reborrow())?, out.reborrow())
    }

    fn visit_fields<'slot>(
        &mut self,
        fields: FieldDeclList<'_>,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, ()> {
        for index in 0..fields.len() {
            let Some(field) = fields.get(index, out.reborrow())? else {
                return Err(out.fail(AstAccessError::InvalidNodeId { id: index }));
            };
            self.visit_type(field.ty(out.reborrow())?, out.reborrow())?;
            if let Some(default) = field.default(out.reborrow())? {
                self.visit_default(default, out.reborrow())?;
            }
        }
        Ok(())
    }

    fn visit_type<'slot>(
        &mut self,
        ty: TypeExpr<'_>,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, ()> {
        match ty.kind(out.reborrow())? {
            TypeExprKind::Builtin(_) | TypeExprKind::Path(_) => Ok(()),
            TypeExprKind::Option(inner) | TypeExprKind::Map { value: inner } => {
                let span = ty.span().span();
                self.mark_search(
                    span.start(),
                    span.end(),
                    Symbol::Less,
                    ContainerKind::TypeAngle,
                );
                self.visit_type(inner, out.reborrow())
            }
            TypeExprKind::Array { element, .. } => {
                self.mark_exact(ty.span(), Symbol::LeftBracket, ContainerKind::TypeArray);
                self.visit_type(element, out.reborrow())
            }
        }
    }

    fn visit_default<'slot>(
        &mut self,
        value: DefaultValue<'_>,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, ()> {
        match value.kind(out.reborrow())? {
            DefaultValueKind::Scalar(_) | DefaultValueKind::None => Ok(()),
            DefaultValueKind::Array(array) => {
                self.mark_exact(value.span(), Symbol::LeftBracket, ContainerKind::ArrayList);
                let elements = array.elements(out.reborrow())?;
                for index in 0..elements.len() {
                    let Some(element) = elements.get(index, out.reborrow())? else {
                        return Err(out.fail(AstAccessError::InvalidNodeId { id: index }));
                    };
                    self.visit_default(element, out.reborrow())?;
                }
                Ok(())
            }
            DefaultValueKind::Braced(braced) => {
                self.visit_braced(braced, value.span(), out.reborrow())
            }
            DefaultValueKind::Enum(enumeration) => {
                let payload = enumeration.payload(out.reborrow())?;
                match payload.kind(out.reborrow())? {
                    EnumDefaultPayloadKind::Unit => Ok(()),
                    EnumDefaultPayloadKind::Single(value) => {
                        self.mark_exact(
                            payload.span(),
                            Symbol::LeftParen,
                            ContainerKind::EnumPayload,
                        );
                        self.visit_default(value, out.reborrow())
                    }
                    EnumDefaultPayloadKind::Structure(braced) => {
                        self.visit_braced(braced, payload.span(), out.reborrow())
                    }
                }
            }
        }
    }

    fn visit_braced<'slot>(
        &mut self,
        braced: BracedDefault<'_>,
        span: SourceSpan,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, ()> {
        self.mark_exact(span, Symbol::LeftBrace, ContainerKind::BraceList);
        let entries = braced.entries(out.reborrow())?;
        for index in 0..entries.len() {
            let Some(entry) = entries.get(index, out.reborrow())? else {
                return Err(out.fail(AstAccessError::InvalidNodeId { id: index }));
            };
            self.visit_default(entry.value(out.reborrow())?, out.reborrow())?;
        }
        Ok(())
    }

    fn mark_exact(&mut self, span: SourceSpan, opener: Symbol, kind: ContainerKind) {
        self.mark_search(span.span().start(), span.span().end(), opener, kind);
    }

    fn mark_search(&mut self, start: usize, end: usize, opener: Symbol, kind: ContainerKind) {
        let search_start = self.tokens.partition_point(|token| token.start < start);
        let search_end = self.tokens.partition_point(|token| token.start < end);
        let Some(search_tokens) = self.tokens.get(search_start..search_end) else {
            self.consistent = false;
            return;
        };
        let Some(offset) = search_tokens.iter().position(|token| {
            token.start >= start && token.start < end && token_symbol(token.kind) == Some(opener)
        }) else {
            self.consistent = false;
            return;
        };
        let Some(index) = search_start.checked_add(offset) else {
            self.consistent = false;
            return;
        };
        let Some(close) = self.tokens.get(index).and_then(|token| token.pair) else {
            self.consistent = false;
            return;
        };
        let expected_close = match opener {
            Symbol::LeftBrace => Symbol::RightBrace,
            Symbol::LeftBracket => Symbol::RightBracket,
            Symbol::LeftParen => Symbol::RightParen,
            Symbol::Less => Symbol::Greater,
            _ => {
                self.consistent = false;
                return;
            }
        };
        if self
            .tokens
            .get(close)
            .is_none_or(|token| token_symbol(token.kind) != Some(expected_close))
        {
            self.consistent = false;
            return;
        }
        let selected_kind = if close == index.saturating_add(1)
            && matches!(
                kind,
                ContainerKind::ModuleBody | ContainerKind::BraceList | ContainerKind::ArrayList
            ) {
            ContainerKind::Inline
        } else {
            kind
        };
        if let Some(token) = self.tokens.get_mut(index) {
            token.container = selected_kind;
        }
        if let Some(token) = self.tokens.get_mut(close) {
            token.container = selected_kind;
        }
    }
}

fn token_symbol(kind: LexemeKind) -> Option<Symbol> {
    if let LexemeKind::Symbol(symbol) = kind {
        Some(symbol)
    } else {
        None
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
        let mut found_item = false;
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
            found_item = true;
            break;
        }
        if !found_item {
            index += 1;
            continue;
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

fn token_text<'source>(
    source: &'source str,
    tokens: &[TokenSpan],
    index: usize,
) -> Option<&'source str> {
    let token = tokens.get(index)?;
    source.get(token.start..token.end)
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

fn scan_string(bytes: &[u8], start: usize) -> Result<usize, SchemaFormatError> {
    if bytes.get(start..start.saturating_add(3)) == Some(b"\"\"\"") {
        let mut cursor = start
            .checked_add(3)
            .ok_or_else(|| SchemaFormatError::new(SchemaFormatErrorKind::ResourceLimitExceeded))?;
        match bytes.get(cursor) {
            Some(b'\n') => cursor += 1,
            Some(b'\r') if bytes.get(cursor.saturating_add(1)) == Some(&b'\n') => cursor += 2,
            _ => return Err(inconsistent_source()),
        }
        while cursor < bytes.len() {
            let mut content = cursor;
            while matches!(bytes.get(content), Some(b' ' | b'\t')) {
                content = content.checked_add(1).ok_or_else(|| {
                    SchemaFormatError::new(SchemaFormatErrorKind::ResourceLimitExceeded)
                })?;
            }
            if bytes.get(content..content.saturating_add(3)) == Some(b"\"\"\"") {
                return content.checked_add(3).ok_or_else(|| {
                    SchemaFormatError::new(SchemaFormatErrorKind::ResourceLimitExceeded)
                });
            }
            while bytes.get(cursor).is_some_and(|byte| *byte != b'\n') {
                cursor = cursor.checked_add(1).ok_or_else(|| {
                    SchemaFormatError::new(SchemaFormatErrorKind::ResourceLimitExceeded)
                })?;
            }
            if cursor < bytes.len() {
                cursor = cursor.checked_add(1).ok_or_else(|| {
                    SchemaFormatError::new(SchemaFormatErrorKind::ResourceLimitExceeded)
                })?;
            }
        }
        return Err(inconsistent_source());
    }
    let mut cursor = start
        .checked_add(1)
        .ok_or_else(|| SchemaFormatError::new(SchemaFormatErrorKind::ResourceLimitExceeded))?;
    while let Some(byte) = bytes.get(cursor).copied() {
        if byte == b'\\' {
            cursor = cursor.checked_add(2).ok_or_else(|| {
                SchemaFormatError::new(SchemaFormatErrorKind::ResourceLimitExceeded)
            })?;
        } else if byte == b'"' {
            return cursor.checked_add(1).ok_or_else(|| {
                SchemaFormatError::new(SchemaFormatErrorKind::ResourceLimitExceeded)
            });
        } else {
            cursor = cursor.checked_add(1).ok_or_else(|| {
                SchemaFormatError::new(SchemaFormatErrorKind::ResourceLimitExceeded)
            })?;
        }
    }
    Err(inconsistent_source())
}

fn inconsistent_source() -> SchemaFormatError {
    SchemaFormatError::new(SchemaFormatErrorKind::InconsistentSource)
}
