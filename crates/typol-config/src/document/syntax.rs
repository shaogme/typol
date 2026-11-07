use opeo::{ErrSlot, OResult, Out};
use typol_core::{BudgetError, ChargedString, ChargedVec, ExecutionBudget, Span};

use super::{ConfigEditError, ConfigEditErrorKind, path::PathValueKind};

#[derive(Clone, Copy)]
pub(super) struct SourceContainer {
    pub(super) open: usize,
    pub(super) close: usize,
}

#[derive(Clone, Copy)]
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
        budget: Option<&ExecutionBudget>,
    ) -> Result<SourceLayout<'source>, ConfigEditErrorKind> {
        let content = source
            .get(bounds.open + 1..bounds.close)
            .ok_or(ConfigEditErrorKind::EditConflict)?;
        consume(budget, content.len())?;
        let multiline = content.contains('\n') || content.contains('\r');
        let line_break = if content.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        };
        Ok(SourceLayout {
            multiline,
            line_break,
            indent: "    ",
            colon_spacing: " ",
        })
    }

    pub(super) fn entry_bounds<'slot>(
        source: &str,
        bounds: SourceContainer,
        children: &[Span],
        budget: Option<&ExecutionBudget>,
        mut out: Out<'slot, '_, ConfigEditError>,
    ) -> OResult<'slot, ChargedVec<SourceItemBounds>> {
        let mut entries = ChargedVec::new(budget, out.reborrow())?;
        entries.try_reserve_exact(children.len(), out.reborrow())?;
        let content_len = bounds.close.checked_sub(bounds.open + 1).ok_or_else(|| {
            out.reborrow()
                .fail(ConfigEditError::new(ConfigEditErrorKind::EditConflict))
        })?;
        consume_budget(budget, content_len, out.reborrow())?;
        let mut entry_index = 0usize;
        let mut item_start = bounds.open + 1;
        let mut previous_comma = None;
        let bytes = source.as_bytes();
        let mut cursor = item_start;
        let mut nested = 0usize;
        while cursor < bounds.close {
            if let Some(lexeme) = lexical_span(source, cursor)
                .map_err(|kind| out.reborrow().fail(ConfigEditError::new(kind)))?
            {
                cursor = lexeme.end;
                continue;
            }
            let Some(byte) = bytes.get(cursor).copied() else {
                return Err(out.fail(ConfigEditError::new(ConfigEditErrorKind::EditConflict)));
            };
            if byte == b',' && nested == 0 {
                while let Some(span) = children.get(entry_index).copied() {
                    if span.start() < item_start {
                        return Err(
                            out.fail(ConfigEditError::new(ConfigEditErrorKind::EditConflict))
                        );
                    }
                    if span.end() > cursor {
                        break;
                    }
                    entries.try_push(
                        SourceItemBounds {
                            start: item_start,
                            end: cursor,
                            previous_comma,
                            next_comma: Some(cursor),
                        },
                        out.reborrow(),
                    )?;
                    entry_index += 1;
                }
                item_start = cursor.checked_add(1).ok_or_else(|| {
                    out.reborrow().fail(ConfigEditError::new(
                        ConfigEditErrorKind::ResourceLimitExceeded,
                    ))
                })?;
                previous_comma = Some(cursor);
            } else if matches!(byte, b'{' | b'[' | b'(') {
                nested = nested.checked_add(1).ok_or_else(|| {
                    out.reborrow().fail(ConfigEditError::new(
                        ConfigEditErrorKind::ResourceLimitExceeded,
                    ))
                })?;
            } else if matches!(byte, b'}' | b']' | b')') && nested > 0 {
                nested -= 1;
            }
            cursor = cursor.checked_add(1).ok_or_else(|| {
                out.reborrow().fail(ConfigEditError::new(
                    ConfigEditErrorKind::ResourceLimitExceeded,
                ))
            })?;
        }
        let mut invalid = false;
        while let Some(span) = children.get(entry_index).copied() {
            if span.start() < item_start || span.end() > bounds.close {
                invalid = true;
                break;
            }
            entries.try_push(
                SourceItemBounds {
                    start: item_start,
                    end: bounds.close,
                    previous_comma,
                    next_comma: None,
                },
                out.reborrow(),
            )?;
            entry_index = entry_index.saturating_add(1);
        }
        if invalid || entry_index != children.len() {
            return Err(out.fail(ConfigEditError::new(ConfigEditErrorKind::EditConflict)));
        }
        Ok(entries)
    }

    pub(super) fn entry_layout<'source>(
        source: &'source str,
        span: Span,
        item: SourceItemBounds,
        budget: Option<&ExecutionBudget>,
    ) -> Result<(Option<&'source str>, &'source str), ConfigEditErrorKind> {
        let prefix = source
            .get(item.start..span.start())
            .ok_or(ConfigEditErrorKind::EditConflict)?;
        let scan_bytes = prefix
            .len()
            .checked_mul(2)
            .ok_or(ConfigEditErrorKind::ResourceLimitExceeded)?;
        consume(budget, scan_bytes)?;
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
        let indent = if indent_len == 0 {
            None
        } else {
            Some(
                line_prefix
                    .get(..indent_len)
                    .ok_or(ConfigEditErrorKind::EditConflict)?,
            )
        };
        let bytes = source.as_bytes();
        let mut cursor = item.start;
        let colon_spacing = loop {
            if cursor >= span.start() {
                break " ";
            }
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
                break source
                    .get(start..end)
                    .ok_or(ConfigEditErrorKind::EditConflict)?;
            }
            cursor = cursor
                .checked_add(1)
                .ok_or(ConfigEditErrorKind::ResourceLimitExceeded)?;
        };
        Ok((indent, colon_spacing))
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
        consume(budget, range.len())?;
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
        budget: Option<&ExecutionBudget>,
    ) -> Result<SourceContainer, ConfigEditErrorKind> {
        consume(budget, span.len())?;
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
}

fn consume(budget: Option<&ExecutionBudget>, steps: usize) -> Result<(), ConfigEditErrorKind> {
    let Some(budget) = budget else {
        return Ok(());
    };
    let mut errors = ErrSlot::<BudgetError>::new();
    errors
        .try_call(|out| budget.consume(steps, out))
        .map_err(|caught| edit_budget_error(caught.take()))
}

fn consume_budget<'slot>(
    budget: Option<&ExecutionBudget>,
    steps: usize,
    out: Out<'slot, '_, ConfigEditError>,
) -> OResult<'slot, ()> {
    let Some(budget) = budget else {
        return Ok(());
    };
    budget.consume(steps, out)
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
        BudgetError::Resource
        | BudgetError::RegexSteps
        | BudgetError::RegexSearchSteps
        | BudgetError::RegexWorkspace
        | BudgetError::CapacityExceeded => ConfigEditErrorKind::ResourceLimitExceeded,
        BudgetError::Allocation | BudgetError::InvariantViolation(_) => {
            ConfigEditErrorKind::AllocationFailed
        }
    }
}
