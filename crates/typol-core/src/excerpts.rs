use crate::{
    ExcerptLimits, ExecutionBudget, SourceId, SourceSpan, Span, SpanLocation,
    optional_storage::OptionalVec, storage::WorkspaceCharge,
};
use alloc::{
    alloc::{Layout, alloc},
    boxed::Box,
};
use core::{
    fmt, iter,
    mem::size_of,
    str::{from_utf8, from_utf8_unchecked},
};

const MASK: &str = "<redacted>";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExcerptStatus {
    Complete,
    Truncated,
    Unavailable,
}

pub(crate) struct ExcerptLine<'a> {
    pub(crate) source_id: SourceId,
    pub(crate) line: usize,
    pub(crate) offset: usize,
    pub(crate) raw: &'a str,
    pub(crate) redactions: &'a [Span],
    pub(crate) truncated: bool,
}

pub(crate) struct MarkerInput<'a> {
    pub(crate) offset: usize,
    pub(crate) raw: &'a str,
    pub(crate) redactions: &'a [Span],
}

pub struct SourceExcerpt {
    source_id: SourceId,
    line: usize,
    text: OptionalVec<u8>,
    truncated: bool,
}

impl SourceExcerpt {
    pub const fn source_id(&self) -> SourceId {
        self.source_id
    }

    pub const fn line(&self) -> usize {
        self.line
    }

    pub fn text(&self) -> &str {
        // SAFETY: Only complete UTF-8 characters, fixed masks, and omission markers are written to the text.
        unsafe { from_utf8_unchecked(&self.text) }
    }

    pub const fn is_truncated(&self) -> bool {
        self.truncated
    }
}

impl fmt::Debug for SourceExcerpt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SourceExcerpt")
            .field("source_id", &self.source_id)
            .field("line", &self.line)
            .field("truncated", &self.truncated)
            .finish()
    }
}

struct ExcerptMarker {
    span: SourceSpan,
    line: usize,
    start: usize,
    end: usize,
}

struct SpanStatusEntry {
    span: SourceSpan,
    status: ExcerptStatus,
}

struct SafeLine {
    text: OptionalVec<u8>,
    truncated: bool,
}

struct ExcerptData {
    charge: WorkspaceCharge,
    records: OptionalVec<SourceExcerpt>,
    markers: OptionalVec<ExcerptMarker>,
    span_statuses: OptionalVec<SpanStatusEntry>,
    limits: ExcerptLimits,
    status: ExcerptStatus,
    active_span: Option<SourceSpan>,
}

pub struct SourceExcerptSet {
    data: Option<Box<ExcerptData>>,
}

impl SourceExcerptSet {
    pub const fn unavailable() -> Self {
        Self { data: None }
    }

    pub fn new(limits: ExcerptLimits, budget: Option<&ExecutionBudget>) -> Self {
        let layout = Layout::new::<ExcerptData>();
        if !limits.enabled() || layout.size() > limits.max_bytes() {
            return Self::unavailable();
        }
        let Some(mut charge) = WorkspaceCharge::new_optional(budget) else {
            return Self::unavailable();
        };
        if !charge.add_optional(layout.size()) {
            return Self::unavailable();
        }
        let Some(records) = OptionalVec::bounded(budget, limits.max_bytes()) else {
            return Self::unavailable();
        };
        let Some(markers) = OptionalVec::bounded(budget, limits.max_bytes()) else {
            return Self::unavailable();
        };
        let Some(span_statuses) = OptionalVec::bounded(budget, limits.max_bytes()) else {
            return Self::unavailable();
        };
        // SAFETY: The layout matches the owned object; allocation failure only disables the snapshot, and `Box` becomes its sole owner after initialization.
        let pointer = unsafe { alloc(layout) }.cast::<ExcerptData>();
        if pointer.is_null() {
            return Self::unavailable();
        }
        unsafe {
            pointer.write(ExcerptData {
                charge,
                records,
                markers,
                span_statuses,
                limits,
                status: ExcerptStatus::Complete,
                active_span: None,
            });
        }
        Self {
            data: Some(unsafe { Box::from_raw(pointer) }),
        }
    }

    pub fn status(&self) -> ExcerptStatus {
        self.data
            .as_ref()
            .map_or(ExcerptStatus::Unavailable, |data| data.status())
    }

    pub fn records(&self) -> &[SourceExcerpt] {
        self.data.as_ref().map_or(&[], |data| data.records())
    }

    pub fn storage_bytes(&self) -> usize {
        self.data
            .as_ref()
            .map_or(0, |data| data.storage_bytes().unwrap_or(usize::MAX))
    }

    pub fn columns(&self, span: SourceSpan, line: usize) -> Option<(usize, usize)> {
        self.data.as_ref()?.columns(span, line)
    }

    pub fn span_status(&self, span: SourceSpan) -> ExcerptStatus {
        self.data
            .as_ref()
            .map_or(ExcerptStatus::Unavailable, |data| data.span_status(span))
    }

    pub fn mark_missing(&mut self) {
        if let Some(data) = self.data.as_mut() {
            data.mark_missing();
        }
    }

    pub(crate) fn mark_span_missing(&mut self, span: SourceSpan) {
        if let Some(data) = self.data.as_mut() {
            data.mark_span_status(span, ExcerptStatus::Truncated);
        }
    }

    pub(crate) fn mark_span_unavailable(&mut self, span: SourceSpan) {
        if let Some(data) = self.data.as_mut() {
            data.mark_span_status(span, ExcerptStatus::Unavailable);
        }
    }

    pub fn copy_span(&mut self, other: &Self, span: SourceSpan) {
        if let Some(data) = self.data.as_mut() {
            if let Some(other) = other.data.as_ref() {
                data.copy_span(other, span);
            } else {
                data.mark_span_status(span, ExcerptStatus::Unavailable);
                data.mark_missing();
            }
        }
    }

    pub(crate) fn insert_line(&mut self, span: SourceSpan, input: ExcerptLine<'_>) {
        if let Some(data) = self.data.as_mut() {
            let previous = data.active_span.replace(span);
            data.insert_line(input);
            data.active_span = previous;
        }
    }

    pub(crate) fn add_marker(
        &mut self,
        span: SourceSpan,
        location: SpanLocation,
        line: usize,
        input: MarkerInput<'_>,
    ) {
        if let Some(data) = self.data.as_mut() {
            let previous = data.active_span.replace(span);
            data.add_marker(span, location, line, input);
            data.active_span = previous;
        }
    }
}

impl ExcerptData {
    pub fn status(&self) -> ExcerptStatus {
        if self.records.is_empty() && self.status == ExcerptStatus::Truncated {
            ExcerptStatus::Unavailable
        } else {
            self.status
        }
    }

    pub fn records(&self) -> &[SourceExcerpt] {
        &self.records
    }

    pub fn storage_bytes(&self) -> Option<usize> {
        OptionalVec::<u8>::checked_storage_bytes(
            iter::once(self.charge.bytes())
                .chain([
                    self.records.capacity_bytes(),
                    self.markers.capacity_bytes(),
                    self.span_statuses.capacity_bytes(),
                ])
                .chain(
                    self.records
                        .iter()
                        .map(|record| record.text.capacity_bytes()),
                ),
        )
    }

    pub fn columns(&self, span: SourceSpan, line: usize) -> Option<(usize, usize)> {
        self.markers
            .iter()
            .find(|marker| marker.span == span && marker.line == line)
            .map(|marker| (marker.start, marker.end))
    }

    pub fn span_status(&self, span: SourceSpan) -> ExcerptStatus {
        self.span_statuses
            .iter()
            .find(|entry| entry.span == span)
            .map_or(ExcerptStatus::Unavailable, |entry| entry.status)
    }

    fn mark_span_status(&mut self, span: SourceSpan, status: ExcerptStatus) {
        if let Some(entry) = self
            .span_statuses
            .iter_mut()
            .find(|entry| entry.span == span)
        {
            entry.status = merge_status(entry.status, status);
        } else {
            if self.span_statuses.len() >= self.limits.max_records() {
                self.status = ExcerptStatus::Truncated;
                return;
            }
            let other_bytes = self
                .storage_bytes()
                .and_then(|bytes| bytes.checked_sub(self.span_statuses.capacity_bytes()));
            let Some(other_bytes) = other_bytes else {
                self.status = ExcerptStatus::Truncated;
                return;
            };
            self.span_statuses
                .set_max_bytes(self.limits.max_bytes().saturating_sub(other_bytes));
            if !self.span_statuses.push(SpanStatusEntry { span, status }) {
                self.status = ExcerptStatus::Truncated;
                return;
            }
        }
        if status != ExcerptStatus::Complete && self.status == ExcerptStatus::Complete {
            self.status = ExcerptStatus::Truncated;
        }
    }

    pub fn mark_missing(&mut self) {
        if self.status == ExcerptStatus::Complete {
            self.status = ExcerptStatus::Truncated;
        }
        if let Some(span) = self.active_span {
            self.mark_span_status(span, ExcerptStatus::Truncated);
        }
    }

    pub(crate) fn insert_line(&mut self, input: ExcerptLine<'_>) {
        if self
            .records
            .iter()
            .any(|record| record.source_id == input.source_id && record.line == input.line)
        {
            return;
        }
        if !self.limits.enabled() || self.records.len() >= self.limits.max_records() {
            self.mark_missing();
            return;
        }
        let Some(storage_bytes) = self.storage_bytes() else {
            self.mark_missing();
            return;
        };
        let available = self
            .limits
            .max_bytes()
            .saturating_sub(storage_bytes)
            .saturating_sub(size_of::<SourceExcerpt>());
        let line_limits = self
            .limits
            .with_max_line_bytes(self.limits.max_line_bytes().min(available));
        if !line_limits.enabled() {
            self.mark_missing();
            return;
        }
        let Some(safe) = safe_line(&input, line_limits, self.records.budget()) else {
            self.mark_missing();
            return;
        };
        let SafeLine { text, truncated } = safe;
        let Some(other_bytes) = self
            .storage_bytes()
            .and_then(|bytes| bytes.checked_sub(self.records.capacity_bytes()))
            .and_then(|bytes| bytes.checked_add(text.capacity_bytes()))
        else {
            self.mark_missing();
            return;
        };
        self.records
            .set_max_bytes(self.limits.max_bytes().saturating_sub(other_bytes));
        if !self.records.push(SourceExcerpt {
            source_id: input.source_id,
            line: input.line,
            text,
            truncated,
        }) || self
            .storage_bytes()
            .is_none_or(|bytes| bytes > self.limits.max_bytes())
        {
            self.mark_missing();
            return;
        }
        if truncated {
            self.mark_missing();
        }
    }

    pub(crate) fn add_marker(
        &mut self,
        span: SourceSpan,
        location: SpanLocation,
        line: usize,
        input: MarkerInput<'_>,
    ) {
        if line < location.start.line() || line > location.end.line() {
            return;
        }
        let Some(record) = self
            .records
            .iter()
            .find(|record| record.source_id == span.source_id() && record.line == line)
        else {
            self.mark_missing();
            return;
        };
        let record_truncated = record.truncated;
        if self
            .markers
            .iter()
            .any(|marker| marker.span == span && marker.line == line)
        {
            return;
        }
        if self.markers.len() >= self.limits.max_records() {
            self.mark_missing();
            return;
        }
        let start = if line == location.start.line() {
            mapped_column(
                input.raw,
                input.offset,
                span.span().start(),
                input.redactions,
            )
        } else {
            1
        };
        let end = if line == location.end.line() {
            mapped_column(input.raw, input.offset, span.span().end(), input.redactions)
        } else {
            record.text().chars().count() + 1
        };
        let max_column = record.text().chars().count() + 1;
        let Some(other_bytes) = self
            .storage_bytes()
            .and_then(|bytes| bytes.checked_sub(self.markers.capacity_bytes()))
        else {
            self.mark_missing();
            return;
        };
        self.markers
            .set_max_bytes(self.limits.max_bytes().saturating_sub(other_bytes));
        if !self.markers.push(ExcerptMarker {
            span,
            line,
            start: start.min(max_column),
            end: end.min(max_column),
        }) || self
            .storage_bytes()
            .is_none_or(|bytes| bytes > self.limits.max_bytes())
        {
            self.mark_missing();
        } else {
            self.mark_span_status(
                span,
                if record_truncated {
                    ExcerptStatus::Truncated
                } else {
                    ExcerptStatus::Complete
                },
            );
        }
    }

    pub fn copy_span(&mut self, other: &Self, span: SourceSpan) {
        self.active_span = Some(span);
        self.copy_span_for_span(other, span);
        self.active_span = None;
    }

    fn copy_span_for_span(&mut self, other: &Self, span: SourceSpan) {
        if other.span_status(span) != ExcerptStatus::Complete {
            self.mark_span_status(span, other.span_status(span));
            self.mark_missing();
        }
        let mut found = false;
        for marker in other.markers.iter().filter(|marker| marker.span == span) {
            found = true;
            let first = self.limits.first_line(marker.line);
            let last = self.limits.last_line(marker.line);
            for record in other.records.iter().filter(|record| {
                record.source_id == span.source_id() && record.line >= first && record.line <= last
            }) {
                self.insert_line(ExcerptLine {
                    source_id: record.source_id,
                    line: record.line,
                    offset: 0,
                    raw: record.text(),
                    redactions: &[],
                    truncated: record.truncated,
                });
            }
            if self
                .markers
                .iter()
                .any(|saved| saved.span == span && saved.line == marker.line)
            {
                continue;
            }
            let Some(record) = self
                .records
                .iter()
                .find(|record| record.source_id == span.source_id() && record.line == marker.line)
            else {
                self.mark_missing();
                continue;
            };
            let max_column = record.text().chars().count() + 1;
            if self.markers.len() >= self.limits.max_records() {
                self.mark_missing();
                continue;
            }
            let Some(other_bytes) = self
                .storage_bytes()
                .and_then(|bytes| bytes.checked_sub(self.markers.capacity_bytes()))
            else {
                self.mark_missing();
                continue;
            };
            self.markers
                .set_max_bytes(self.limits.max_bytes().saturating_sub(other_bytes));
            if !self.markers.push(ExcerptMarker {
                span,
                line: marker.line,
                start: marker.start.min(max_column),
                end: marker.end.min(max_column),
            }) || self
                .storage_bytes()
                .is_none_or(|bytes| bytes > self.limits.max_bytes())
            {
                self.mark_missing();
            }
        }
        if !found {
            self.mark_missing();
        } else if other.span_status(span) == ExcerptStatus::Complete {
            self.mark_span_status(span, ExcerptStatus::Complete);
        }
    }
}

fn merge_status(left: ExcerptStatus, right: ExcerptStatus) -> ExcerptStatus {
    match (left, right) {
        (ExcerptStatus::Unavailable, _) | (_, ExcerptStatus::Unavailable) => {
            ExcerptStatus::Unavailable
        }
        (ExcerptStatus::Truncated, _) | (_, ExcerptStatus::Truncated) => ExcerptStatus::Truncated,
        _ => ExcerptStatus::Complete,
    }
}

impl fmt::Debug for SourceExcerptSet {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SourceExcerptSet")
            .field("status", &self.status())
            .field("record_count", &self.records().len())
            .finish()
    }
}

fn covering(ranges: &[Span], offset: usize) -> Option<usize> {
    ranges
        .iter()
        .filter(|range| range.start() <= offset && offset < range.end())
        .map(|range| range.end())
        .max()
}

fn safe_line(
    input: &ExcerptLine<'_>,
    limits: ExcerptLimits,
    budget: Option<&ExecutionBudget>,
) -> Option<SafeLine> {
    let limit = limits.max_line_bytes();
    let ellipsis = limits.omission();
    let affected = input
        .redactions
        .iter()
        .filter(|range| {
            range.start() < input.offset.saturating_add(input.raw.len())
                && range.end() > input.offset
        })
        .count();
    let capacity = limit.min(
        input
            .raw
            .len()
            .saturating_add(affected.saturating_mul(MASK.len()))
            .saturating_add(ellipsis.len()),
    );
    let mut text = OptionalVec::bounded(budget, limit)?;
    if !text.reserve(capacity) {
        return None;
    }
    let mut offset = 0;
    let mut truncated = input.truncated;
    while offset < input.raw.len() {
        let (chunk, next) = if let Some(end) = covering(input.redactions, input.offset + offset) {
            (MASK, end.saturating_sub(input.offset).min(input.raw.len()))
        } else {
            let ch = input.raw.get(offset..)?.chars().next()?;
            (
                input.raw.get(offset..offset + ch.len_utf8())?,
                offset + ch.len_utf8(),
            )
        };
        let reserve = if next < input.raw.len() || input.truncated {
            ellipsis.len()
        } else {
            0
        };
        if text
            .len()
            .saturating_add(chunk.len())
            .saturating_add(reserve)
            > limit
        {
            truncated = true;
            break;
        }
        if !text.extend(chunk.as_bytes()) {
            return None;
        }
        offset = next;
    }
    if truncated && text.len() + ellipsis.len() <= limit && !text.extend(ellipsis.as_bytes()) {
        return None;
    }
    from_utf8(&text).ok()?;
    Some(SafeLine { text, truncated })
}

fn mapped_column(raw: &str, line_offset: usize, position: usize, ranges: &[Span]) -> usize {
    let mut offset = 0;
    let mut column = 1;
    while offset < raw.len() && line_offset + offset < position {
        if let Some(end) = covering(ranges, line_offset + offset) {
            column += MASK.chars().count();
            offset = end.saturating_sub(line_offset).min(raw.len());
        } else if let Some(ch) = raw.get(offset..).and_then(|suffix| suffix.chars().next()) {
            offset += ch.len_utf8();
            column += 1;
        } else {
            break;
        }
    }
    column
}
