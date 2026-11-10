use opeo::{ErrSlot, OResult, Out};
use typol_core::{BudgetError, ChargedString, ChargedVec, ExecutionBudget, Span, TextLayout};

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
    pub(super) indent_unit: &'source str,
    pub(super) indent_levels: usize,
    pub(super) colon_spacing: &'source str,
}

struct LexicalSpan {
    end: usize,
    comment: bool,
    block_string: bool,
}

struct EmptyIndentation<'source> {
    indent: &'source str,
    unit: &'source str,
    levels: usize,
}

pub(super) struct SourceSyntax;

fn repeated_scan_steps(bytes: usize, passes: usize) -> Result<usize, ConfigEditErrorKind> {
    bytes
        .checked_mul(passes)
        .ok_or(ConfigEditErrorKind::ResourceLimitExceeded)
}

impl SourceSyntax {
    pub(super) fn document_line_break(
        source: &str,
        budget: Option<&ExecutionBudget>,
    ) -> Result<&'static str, ConfigEditErrorKind> {
        consume(budget, source.len())?;
        Ok(if source.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        })
    }

    pub(super) fn edit_line_break(
        source: &str,
        budget: Option<&ExecutionBudget>,
        layout: TextLayout,
        encodes_values: bool,
    ) -> Result<&'static str, ConfigEditErrorKind> {
        if layout == TextLayout::Pretty && encodes_values {
            Self::document_line_break(source, budget)
        } else {
            Ok("\n")
        }
    }

    pub(super) fn indent_context<'source>(
        source: &'source str,
        position: usize,
        budget: Option<&ExecutionBudget>,
    ) -> Result<(&'source str, usize), ConfigEditErrorKind> {
        let prefix = source
            .get(..position)
            .ok_or(ConfigEditErrorKind::EditConflict)?;
        consume(budget, position)?;
        let line_start = prefix
            .rfind('\n')
            .map_or(0, |index| index.saturating_add(1));
        let line_prefix = source
            .get(line_start..position)
            .ok_or(ConfigEditErrorKind::EditConflict)?;
        consume(budget, line_prefix.len())?;
        let indent_len = line_prefix
            .bytes()
            .take_while(|byte| matches!(byte, b' ' | b'\t'))
            .count();
        let indent = line_prefix
            .get(..indent_len)
            .ok_or(ConfigEditErrorKind::EditConflict)?;
        consume(budget, position)?;
        let mut cursor = 0usize;
        let mut depth = 0usize;
        let bytes = source.as_bytes();
        while cursor < position {
            if let Some(lexeme) = lexical_span(source, cursor)? {
                cursor = lexeme.end.min(position);
                continue;
            }
            let Some(byte) = bytes.get(cursor).copied() else {
                return Err(ConfigEditErrorKind::EditConflict);
            };
            if cursor >= line_start {
                if matches!(byte, b'{' | b'[' | b'(') {
                    depth = depth
                        .checked_add(1)
                        .ok_or(ConfigEditErrorKind::ResourceLimitExceeded)?;
                } else if matches!(byte, b'}' | b']' | b')') {
                    depth = depth.saturating_sub(1);
                }
            }
            cursor = cursor
                .checked_add(1)
                .ok_or(ConfigEditErrorKind::ResourceLimitExceeded)?;
        }
        Ok((indent, depth))
    }

    pub(super) fn layout<'source>(
        source: &'source str,
        bounds: SourceContainer,
        infer_empty_indent: bool,
        budget: Option<&ExecutionBudget>,
    ) -> Result<SourceLayout<'source>, ConfigEditErrorKind> {
        let content = source
            .get(bounds.open + 1..bounds.close)
            .ok_or(ConfigEditErrorKind::EditConflict)?;
        consume(budget, content.len())?;
        let bytes = source.as_bytes();
        let mut cursor = bounds.open + 1;
        let mut multiline = false;
        let mut line_break = "\n";
        while cursor < bounds.close {
            if let Some(lexeme) = lexical_span(source, cursor)? {
                if lexeme.comment {
                    let comment = source
                        .get(cursor..lexeme.end)
                        .ok_or(ConfigEditErrorKind::EditConflict)?;
                    let comment_bytes = comment.as_bytes();
                    for (index, byte) in comment_bytes.iter().copied().enumerate() {
                        if byte == b'\r' {
                            multiline = true;
                        } else if byte == b'\n' {
                            multiline = true;
                            if comment_bytes.get(index.saturating_sub(1)) == Some(&b'\r') {
                                line_break = "\r\n";
                            }
                        }
                    }
                }
                cursor = lexeme.end.min(bounds.close);
                continue;
            }
            match bytes.get(cursor).copied() {
                Some(b'\n') => {
                    multiline = true;
                    if bytes.get(cursor.saturating_sub(1)) == Some(&b'\r') {
                        line_break = "\r\n";
                    }
                }
                Some(b'\r') => multiline = true,
                Some(_) => {}
                None => return Err(ConfigEditErrorKind::EditConflict),
            }
            cursor = cursor
                .checked_add(1)
                .ok_or(ConfigEditErrorKind::ResourceLimitExceeded)?;
        }
        let indentation = if multiline && infer_empty_indent {
            Self::empty_container_indent(source, bounds.open, budget)?
        } else {
            EmptyIndentation {
                indent: "    ",
                unit: "    ",
                levels: 0,
            }
        };
        Ok(SourceLayout {
            multiline,
            line_break,
            indent: indentation.indent,
            indent_unit: indentation.unit,
            indent_levels: indentation.levels,
            colon_spacing: " ",
        })
    }

    fn empty_container_indent<'source>(
        source: &'source str,
        open: usize,
        budget: Option<&ExecutionBudget>,
    ) -> Result<EmptyIndentation<'source>, ConfigEditErrorKind> {
        let prefix = source
            .get(..open)
            .ok_or(ConfigEditErrorKind::EditConflict)?;
        consume(budget, open)?;
        let line_start = prefix
            .rfind('\n')
            .map_or(0, |index| index.saturating_add(1));
        let line_prefix = source
            .get(line_start..open)
            .ok_or(ConfigEditErrorKind::EditConflict)?;
        consume(budget, repeated_scan_steps(line_prefix.len(), 2)?)?;
        let indent_len = line_prefix
            .bytes()
            .take_while(|byte| matches!(byte, b' ' | b'\t'))
            .count();
        let indent = line_prefix
            .get(..indent_len)
            .ok_or(ConfigEditErrorKind::EditConflict)?;
        let mut indent_unit = "    ";
        if indent_len > 0 {
            let preceding_source = source
                .get(..line_start)
                .ok_or(ConfigEditErrorKind::EditConflict)?;
            consume(budget, repeated_scan_steps(preceding_source.len(), 4)?)?;
            let mut previous_start = 0usize;
            let mut block_string_end = 0usize;
            let mut lexical_end = 0usize;
            while previous_start < line_start {
                let remaining = preceding_source
                    .get(previous_start..)
                    .ok_or(ConfigEditErrorKind::EditConflict)?;
                let line_end = remaining
                    .find('\n')
                    .map_or(line_start, |index| previous_start.saturating_add(index));
                let previous_line = source
                    .get(previous_start..line_end)
                    .ok_or(ConfigEditErrorKind::EditConflict)?;
                if previous_start >= block_string_end {
                    let previous_indent_len = previous_line
                        .bytes()
                        .take_while(|byte| matches!(byte, b' ' | b'\t'))
                        .count();
                    let previous_content = previous_line
                        .get(previous_indent_len..)
                        .ok_or(ConfigEditErrorKind::EditConflict)?;
                    if previous_content
                        .bytes()
                        .any(|byte| !matches!(byte, b' ' | b'\t' | b'\r'))
                        && previous_indent_len < indent_len
                    {
                        let previous_indent = previous_line
                            .get(..previous_indent_len)
                            .ok_or(ConfigEditErrorKind::EditConflict)?;
                        if indent.starts_with(previous_indent) {
                            indent_unit = indent
                                .get(previous_indent_len..)
                                .ok_or(ConfigEditErrorKind::EditConflict)?;
                        }
                    }
                    let mut cursor = previous_start.max(lexical_end);
                    while cursor < line_end {
                        if let Some(lexeme) = lexical_span(source, cursor)? {
                            if lexeme.block_string {
                                block_string_end = lexeme.end;
                                break;
                            }
                            lexical_end = lexeme.end;
                            cursor = lexeme.end;
                            continue;
                        }
                        cursor = cursor
                            .checked_add(1)
                            .ok_or(ConfigEditErrorKind::ResourceLimitExceeded)?;
                    }
                }
                previous_start = if line_end < line_start {
                    line_end
                        .checked_add(1)
                        .ok_or(ConfigEditErrorKind::ResourceLimitExceeded)?
                } else {
                    line_start
                };
            }
        }
        let mut cursor = line_start;
        let mut same_line_depth = 0usize;
        let bytes = source.as_bytes();
        while cursor < open {
            if let Some(lexeme) = lexical_span(source, cursor)? {
                cursor = lexeme.end.min(open);
                continue;
            }
            let Some(byte) = bytes.get(cursor).copied() else {
                return Err(ConfigEditErrorKind::EditConflict);
            };
            if matches!(byte, b'{' | b'[' | b'(') {
                same_line_depth = same_line_depth
                    .checked_add(1)
                    .ok_or(ConfigEditErrorKind::ResourceLimitExceeded)?;
            } else if matches!(byte, b'}' | b']' | b')') {
                same_line_depth = same_line_depth.saturating_sub(1);
            }
            cursor = cursor
                .checked_add(1)
                .ok_or(ConfigEditErrorKind::ResourceLimitExceeded)?;
        }
        let indent_levels = same_line_depth
            .checked_add(1)
            .ok_or(ConfigEditErrorKind::ResourceLimitExceeded)?;
        Ok(EmptyIndentation {
            indent,
            unit: indent_unit,
            levels: indent_levels,
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
            PathValueKind::Scalar
            | PathValueKind::Enumeration
            | PathValueKind::Payload
            | PathValueKind::Other => {
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
                block_string: bytes.get(start..start.saturating_add(3)) == Some(b"\"\"\""),
            })
        }),
        Some(b'/') if bytes.get(start.saturating_add(1)) == Some(&b'/') => {
            skip_line_comment(source, start).map(|end| {
                Some(LexicalSpan {
                    end,
                    comment: true,
                    block_string: false,
                })
            })
        }
        Some(b'/') if bytes.get(start.saturating_add(1)) == Some(&b'*') => {
            skip_block_comment(source, start).map(|end| {
                Some(LexicalSpan {
                    end,
                    comment: true,
                    block_string: false,
                })
            })
        }
        _ => Ok(None),
    }
}

fn skip_string(source: &str, start: usize) -> Result<usize, ConfigEditErrorKind> {
    let bytes = source.as_bytes();
    if bytes.get(start..start.saturating_add(3)) == Some(b"\"\"\"") {
        let mut cursor = start
            .checked_add(3)
            .ok_or(ConfigEditErrorKind::ResourceLimitExceeded)?;
        match bytes.get(cursor) {
            Some(b'\n') => cursor += 1,
            Some(b'\r') if bytes.get(cursor.saturating_add(1)) == Some(&b'\n') => cursor += 2,
            _ => return Err(ConfigEditErrorKind::EditConflict),
        }
        while cursor < bytes.len() {
            let mut content = cursor;
            while matches!(bytes.get(content), Some(b' ' | b'\t')) {
                content = content
                    .checked_add(1)
                    .ok_or(ConfigEditErrorKind::ResourceLimitExceeded)?;
            }
            if bytes.get(content..content.saturating_add(3)) == Some(b"\"\"\"") {
                return content
                    .checked_add(3)
                    .ok_or(ConfigEditErrorKind::ResourceLimitExceeded);
            }
            while bytes.get(cursor).is_some_and(|byte| *byte != b'\n') {
                cursor = cursor
                    .checked_add(1)
                    .ok_or(ConfigEditErrorKind::ResourceLimitExceeded)?;
            }
            if cursor < bytes.len() {
                cursor = cursor
                    .checked_add(1)
                    .ok_or(ConfigEditErrorKind::ResourceLimitExceeded)?;
            }
        }
        return Err(ConfigEditErrorKind::EditConflict);
    }
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

#[cfg(test)]
mod tests {
    use super::{ConfigEditErrorKind, SourceSyntax};
    use alloc::format;
    use typol_core::{ExecutionBudget, ExecutionLimits};
    use typol_test_support::{
        TestFailure, TestOpeo, TestResult, test_verify as test_assert, test_verify_eq as test_eq,
    };

    fn expected_scan_steps(bytes: usize, passes: usize) -> TestResult<usize> {
        bytes
            .checked_mul(passes)
            .ok_or_else(|| TestFailure::new("scan budget overflowed"))
    }

    fn budget_with_steps(max_steps: usize) -> TestResult<ExecutionBudget> {
        TestOpeo::value(|out| ExecutionBudget::new(ExecutionLimits::new(max_steps, 1_048_576), out))
    }

    #[test]
    fn indent_context_charges_prefix_indentation_and_depth_scans() -> TestResult<()> {
        let source = " ".repeat(4096);
        let position = source.len();
        let limit = expected_scan_steps(position, 3)?;
        let budget = budget_with_steps(limit)?;

        let context = SourceSyntax::indent_context(&source, position, Some(&budget))
            .map_err(|_| TestFailure::new("two charged scans did not fit"))?;

        test_eq!(context, (source.as_str(), 0));
        test_eq!(budget.steps(), limit);
        Ok(())
    }

    #[test]
    fn indent_context_rejects_budget_that_covers_only_one_source_pass() -> TestResult<()> {
        let source = "x".repeat(4096);
        let position = source.len();
        let budget = budget_with_steps(position)?;

        let result = SourceSyntax::indent_context(&source, position, Some(&budget));

        test_eq!(result, Err(ConfigEditErrorKind::ResourceLimitExceeded));
        test_eq!(budget.steps(), position);
        Ok(())
    }

    #[test]
    fn empty_container_indent_charges_prefix_and_repeated_line_scans() -> TestResult<()> {
        let preceding = "x".repeat(4096);
        let source = format!("{preceding}\n  {{");
        let open = source.len() - 1;
        let line_start = preceding.len() + 1;
        let line_prefix_len = open - line_start;
        let limit =
            open + expected_scan_steps(line_prefix_len, 2)? + expected_scan_steps(line_start, 4)?;
        let budget = budget_with_steps(limit)?;

        let indentation = SourceSyntax::empty_container_indent(&source, open, Some(&budget))
            .map_err(|_| TestFailure::new("charged indentation scans did not fit"))?;

        test_eq!(indentation.indent, "  ");
        test_eq!(indentation.levels, 1);
        test_eq!(budget.steps(), limit);
        Ok(())
    }

    #[test]
    fn empty_container_indent_rejects_budget_that_covers_only_old_charge() -> TestResult<()> {
        let preceding = "x".repeat(4096);
        let source = format!("{preceding}\n  {{");
        let open = source.len() - 1;
        let line_start = preceding.len() + 1;
        let line_prefix_len = open - line_start;
        let old_charge = line_prefix_len + line_start;
        let budget = budget_with_steps(old_charge)?;

        let result = SourceSyntax::empty_container_indent(&source, open, Some(&budget));

        test_assert!(matches!(
            result,
            Err(ConfigEditErrorKind::ResourceLimitExceeded)
        ));
        test_eq!(budget.steps(), open);
        Ok(())
    }
}
