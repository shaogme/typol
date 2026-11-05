use opeo::ErrSlot;
use typol_core::{BudgetError, ChargedString, ExecutionBudget, Span};

use super::{ConfigEditErrorKind, path::PathValueKind};

#[derive(Clone, Copy)]
pub(super) struct SourceContainer {
    pub(super) open: usize,
    pub(super) close: usize,
}

pub(super) struct SourceItemBounds {
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) previous_comma: Option<usize>,
    pub(super) next_comma: Option<usize>,
}

#[derive(Clone, Copy)]
pub(super) struct SourceLayout<'source> {
    pub(super) multiline: bool,
    pub(super) line_break: &'static str,
    pub(super) indent: &'source str,
    pub(super) colon_spacing: &'source str,
}

struct LexicalSpan {
    end: usize,
    comment: bool,
}

pub(super) struct SourceSyntax;

impl SourceSyntax {
    pub(super) fn layout<'source>(
        source: &'source str,
        bounds: SourceContainer,
        reference: Option<Span>,
    ) -> Result<SourceLayout<'source>, ConfigEditErrorKind> {
        let content = source
            .get(bounds.open + 1..bounds.close)
            .ok_or(ConfigEditErrorKind::EditConflict)?;
        let multiline = content.contains('\n') || content.contains('\r');
        let line_break = if content.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        };
        let indent = reference
            .map(|span| Self::item_indent(source, bounds, span))
            .transpose()?
            .flatten()
            .unwrap_or("    ");
        let colon_spacing = reference
            .map(|span| Self::item_colon_spacing(source, bounds, span))
            .transpose()?
            .unwrap_or(" ");
        Ok(SourceLayout {
            multiline,
            line_break,
            indent,
            colon_spacing,
        })
    }

    pub(super) fn comments_in(
        source: &str,
        start: usize,
        end: usize,
        budget: Option<&ExecutionBudget>,
    ) -> Result<ChargedString, ConfigEditErrorKind> {
        let range = source
            .get(start..end)
            .ok_or(ConfigEditErrorKind::EditConflict)?;
        let mut errors = ErrSlot::<BudgetError>::new();
        let mut comments = errors
            .try_call(|out| ChargedString::with_capacity(0, budget, out))
            .map_err(|caught| edit_budget_error(caught.take()))?;
        let mut cursor = start;
        while cursor < end {
            if let Some(lexeme) = lexical_span(source, cursor)? {
                if lexeme.comment {
                    let text = source
                        .get(cursor..lexeme.end)
                        .ok_or(ConfigEditErrorKind::EditConflict)?;
                    append_charged(&mut comments, text, range.len())?;
                }
                cursor = lexeme.end;
            } else {
                cursor = cursor
                    .checked_add(1)
                    .ok_or(ConfigEditErrorKind::ResourceLimitExceeded)?;
            }
        }
        Ok(comments)
    }

    pub(super) fn scan_container(
        source: &str,
        span: Span,
        kind: PathValueKind,
    ) -> Result<SourceContainer, ConfigEditErrorKind> {
        let (opening, closing) = match kind {
            PathValueKind::Structure | PathValueKind::Map => (b'{', b'}'),
            PathValueKind::Array => (b'[', b']'),
            PathValueKind::Scalar | PathValueKind::Enumeration | PathValueKind::Other => {
                return Err(ConfigEditErrorKind::InvalidContainer);
            }
        };
        let bytes = source.as_bytes();
        let mut cursor = span.start();
        let mut open = None;
        while cursor < span.end() {
            if let Some(lexeme) = lexical_span(source, cursor)? {
                cursor = lexeme.end;
                continue;
            }
            if bytes.get(cursor) == Some(&opening) {
                open = Some(cursor);
                break;
            }
            cursor = cursor
                .checked_add(1)
                .ok_or(ConfigEditErrorKind::ResourceLimitExceeded)?;
        }
        let open = open.ok_or(ConfigEditErrorKind::InvalidContainer)?;
        cursor = open
            .checked_add(1)
            .ok_or(ConfigEditErrorKind::ResourceLimitExceeded)?;
        let mut nested = 0usize;
        while cursor < span.end() {
            if let Some(lexeme) = lexical_span(source, cursor)? {
                cursor = lexeme.end;
                continue;
            }
            let Some(byte) = bytes.get(cursor).copied() else {
                return Err(ConfigEditErrorKind::EditConflict);
            };
            if matches!(byte, b'{' | b'[' | b'(') {
                nested = nested
                    .checked_add(1)
                    .ok_or(ConfigEditErrorKind::ResourceLimitExceeded)?;
            } else if matches!(byte, b'}' | b']' | b')') {
                if nested == 0 {
                    if byte == closing {
                        return Ok(SourceContainer {
                            open,
                            close: cursor,
                        });
                    }
                } else {
                    nested -= 1;
                }
            }
            cursor = cursor
                .checked_add(1)
                .ok_or(ConfigEditErrorKind::ResourceLimitExceeded)?;
        }
        Err(ConfigEditErrorKind::InvalidContainer)
    }

    pub(super) fn locate_item(
        source: &str,
        bounds: SourceContainer,
        target: Span,
    ) -> Result<SourceItemBounds, ConfigEditErrorKind> {
        let mut start = bounds.open + 1;
        let mut previous_comma = None;
        let mut located = None;
        visit_top_level_commas(source, bounds, |comma| {
            if target.start() >= start && target.end() <= comma {
                located = Some(SourceItemBounds {
                    start,
                    end: comma,
                    previous_comma,
                    next_comma: Some(comma),
                });
                false
            } else {
                start = comma + 1;
                previous_comma = Some(comma);
                true
            }
        })?;
        if let Some(item) = located {
            return Ok(item);
        }
        if target.start() >= start && target.end() <= bounds.close {
            return Ok(SourceItemBounds {
                start,
                end: bounds.close,
                previous_comma,
                next_comma: None,
            });
        }
        Err(ConfigEditErrorKind::EditConflict)
    }

    pub(super) fn trailing_comma(
        source: &str,
        bounds: SourceContainer,
        after: usize,
    ) -> Result<Option<usize>, ConfigEditErrorKind> {
        let mut trailing = None;
        visit_top_level_commas(source, bounds, |comma| {
            if comma >= after {
                trailing = Some(comma);
            }
            true
        })?;
        Ok(trailing)
    }

    fn item_indent(
        source: &str,
        bounds: SourceContainer,
        span: Span,
    ) -> Result<Option<&str>, ConfigEditErrorKind> {
        let item = Self::locate_item(source, bounds, span)?;
        let prefix = source
            .get(item.start..span.start())
            .ok_or(ConfigEditErrorKind::EditConflict)?;
        let line_start = prefix
            .rfind('\n')
            .map_or(0, |index| index.saturating_add(1));
        let line_prefix = prefix
            .get(line_start..)
            .ok_or(ConfigEditErrorKind::EditConflict)?;
        let indent_len = line_prefix
            .bytes()
            .take_while(|byte| matches!(byte, b' ' | b'\t'))
            .count();
        if indent_len == 0 {
            return Ok(None);
        }
        Ok(line_prefix.get(..indent_len))
    }

    fn item_colon_spacing(
        source: &str,
        bounds: SourceContainer,
        span: Span,
    ) -> Result<&str, ConfigEditErrorKind> {
        let item = Self::locate_item(source, bounds, span)?;
        let bytes = source.as_bytes();
        let mut cursor = item.start;
        while cursor < span.start() {
            if let Some(lexeme) = lexical_span(source, cursor)? {
                cursor = lexeme.end;
                continue;
            }
            if bytes.get(cursor) == Some(&b':') {
                let start = cursor
                    .checked_add(1)
                    .ok_or(ConfigEditErrorKind::ResourceLimitExceeded)?;
                let mut end = start;
                while matches!(bytes.get(end), Some(b' ' | b'\t')) {
                    end = end
                        .checked_add(1)
                        .ok_or(ConfigEditErrorKind::ResourceLimitExceeded)?;
                }
                return source
                    .get(start..end)
                    .ok_or(ConfigEditErrorKind::EditConflict);
            }
            cursor = cursor
                .checked_add(1)
                .ok_or(ConfigEditErrorKind::ResourceLimitExceeded)?;
        }
        Ok(" ")
    }
}

fn append_charged(
    output: &mut ChargedString,
    text: &str,
    limit: usize,
) -> Result<(), ConfigEditErrorKind> {
    if output
        .len()
        .checked_add(text.len())
        .filter(|length| *length <= limit)
        .is_none()
    {
        return Err(ConfigEditErrorKind::ResourceLimitExceeded);
    }
    let mut errors = ErrSlot::<BudgetError>::new();
    errors
        .try_call(|out| output.try_push_str(text, out))
        .map_err(|caught| edit_budget_error(caught.take()))?;
    Ok(())
}

fn visit_top_level_commas(
    source: &str,
    bounds: SourceContainer,
    mut visit: impl FnMut(usize) -> bool,
) -> Result<(), ConfigEditErrorKind> {
    let bytes = source.as_bytes();
    let mut cursor = bounds.open + 1;
    let mut nested = 0usize;
    while cursor < bounds.close {
        if let Some(lexeme) = lexical_span(source, cursor)? {
            cursor = lexeme.end;
            continue;
        }
        let Some(byte) = bytes.get(cursor).copied() else {
            return Err(ConfigEditErrorKind::EditConflict);
        };
        if byte == b',' && nested == 0 {
            if !visit(cursor) {
                return Ok(());
            }
        } else if matches!(byte, b'{' | b'[' | b'(') {
            nested = nested
                .checked_add(1)
                .ok_or(ConfigEditErrorKind::ResourceLimitExceeded)?;
        } else if matches!(byte, b'}' | b']' | b')') && nested > 0 {
            nested -= 1;
        }
        cursor = cursor
            .checked_add(1)
            .ok_or(ConfigEditErrorKind::ResourceLimitExceeded)?;
    }
    Ok(())
}

fn lexical_span(source: &str, start: usize) -> Result<Option<LexicalSpan>, ConfigEditErrorKind> {
    let bytes = source.as_bytes();
    match bytes.get(start).copied() {
        Some(b'"') => skip_string(source, start).map(|end| {
            Some(LexicalSpan {
                end,
                comment: false,
            })
        }),
        Some(b'/') if bytes.get(start.saturating_add(1)) == Some(&b'/') => {
            skip_line_comment(source, start).map(|end| Some(LexicalSpan { end, comment: true }))
        }
        Some(b'/') if bytes.get(start.saturating_add(1)) == Some(&b'*') => {
            skip_block_comment(source, start).map(|end| Some(LexicalSpan { end, comment: true }))
        }
        _ => Ok(None),
    }
}

fn skip_string(source: &str, start: usize) -> Result<usize, ConfigEditErrorKind> {
    let bytes = source.as_bytes();
    let mut cursor = start
        .checked_add(1)
        .ok_or(ConfigEditErrorKind::ResourceLimitExceeded)?;
    while let Some(byte) = bytes.get(cursor).copied() {
        if byte == b'\\' {
            cursor = cursor
                .checked_add(2)
                .ok_or(ConfigEditErrorKind::ResourceLimitExceeded)?;
        } else if byte == b'"' {
            return cursor
                .checked_add(1)
                .ok_or(ConfigEditErrorKind::ResourceLimitExceeded);
        } else {
            cursor = cursor
                .checked_add(1)
                .ok_or(ConfigEditErrorKind::ResourceLimitExceeded)?;
        }
    }
    Err(ConfigEditErrorKind::EditConflict)
}

fn skip_line_comment(source: &str, start: usize) -> Result<usize, ConfigEditErrorKind> {
    let bytes = source.as_bytes();
    let mut cursor = start
        .checked_add(2)
        .ok_or(ConfigEditErrorKind::ResourceLimitExceeded)?;
    while let Some(byte) = bytes.get(cursor).copied() {
        cursor = cursor
            .checked_add(1)
            .ok_or(ConfigEditErrorKind::ResourceLimitExceeded)?;
        if byte == b'\n' {
            return Ok(cursor);
        }
        if byte == b'\r' {
            if bytes.get(cursor) == Some(&b'\n') {
                cursor = cursor
                    .checked_add(1)
                    .ok_or(ConfigEditErrorKind::ResourceLimitExceeded)?;
            }
            return Ok(cursor);
        }
    }
    Ok(cursor)
}

fn skip_block_comment(source: &str, start: usize) -> Result<usize, ConfigEditErrorKind> {
    let bytes = source.as_bytes();
    let mut cursor = start
        .checked_add(2)
        .ok_or(ConfigEditErrorKind::ResourceLimitExceeded)?;
    while cursor < bytes.len() {
        if bytes.get(cursor) == Some(&b'*') && bytes.get(cursor.saturating_add(1)) == Some(&b'/') {
            return cursor
                .checked_add(2)
                .ok_or(ConfigEditErrorKind::ResourceLimitExceeded);
        }
        cursor = cursor
            .checked_add(1)
            .ok_or(ConfigEditErrorKind::ResourceLimitExceeded)?;
    }
    Err(ConfigEditErrorKind::EditConflict)
}

fn edit_budget_error(error: BudgetError) -> ConfigEditErrorKind {
    match error {
        BudgetError::Resource | BudgetError::CapacityExceeded => {
            ConfigEditErrorKind::ResourceLimitExceeded
        }
        BudgetError::Allocation | BudgetError::InvariantViolation(_) => {
            ConfigEditErrorKind::AllocationFailed
        }
    }
}
