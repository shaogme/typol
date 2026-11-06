use crate::{
    Diagnostic, ExcerptLimits, ExecutionBudget, LocatedSpan, SourceExcerptSet, SourceId,
    SourceSpan, Span, SpanLocation,
    excerpts::{ExcerptLine, MarkerInput},
    optional_storage::OptionalVec,
};
use core::{
    fmt,
    mem::replace,
    str::from_utf8,
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT_CAPTURE_ID: AtomicUsize = AtomicUsize::new(1);

fn reserve_capture_id() -> Option<usize> {
    let mut current = NEXT_CAPTURE_ID.load(Ordering::Relaxed);
    loop {
        let next = current.checked_add(1)?;
        match NEXT_CAPTURE_ID.compare_exchange_weak(
            current,
            next,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => return Some(current),
            Err(actual) => current = actual,
        }
    }
}

/// A secret-range handle that can be finished exactly once by the capture that created it.
pub struct SecretRange {
    capture_id: usize,
    index: usize,
    start: usize,
}

struct CapturedLine {
    bytes: OptionalVec<u8>,
    line: usize,
    offset: usize,
    pinned: bool,
    truncated: bool,
    newline: bool,
}

/// Used only by the input session to stage raw lines; public reports must pass through `finish` to apply range masking.
pub struct SourceCapture {
    lines: OptionalVec<CapturedLine>,
    redactions: OptionalVec<Span>,
    current: OptionalVec<u8>,
    source_id: SourceId,
    capture_id: usize,
    limits: ExcerptLimits,
    offset: usize,
    line_offset: usize,
    line: usize,
    line_truncated: bool,
    skip_current_line: bool,
    unavailable: bool,
    sealed: bool,
    input_complete: bool,
    pin_start: usize,
    pin_until: usize,
    lost_lines: Option<(usize, usize)>,
}

impl SourceCapture {
    pub fn new(
        source_id: SourceId,
        limits: ExcerptLimits,
        budget: Option<&ExecutionBudget>,
    ) -> Self {
        let capture_id = reserve_capture_id();
        let Some(capture_id) = capture_id else {
            return Self::disabled(source_id, limits, usize::MAX);
        };
        let storage = OptionalVec::new(budget).zip(OptionalVec::new(budget));
        let Some((lines, redactions)) = storage else {
            return Self::disabled(source_id, limits, capture_id);
        };
        let Some(current) = OptionalVec::new(budget) else {
            return Self::disabled(source_id, limits, capture_id);
        };
        Self {
            lines,
            redactions,
            current,
            source_id,
            capture_id,
            limits,
            offset: 0,
            line_offset: 0,
            line: 1,
            line_truncated: false,
            skip_current_line: false,
            unavailable: !limits.enabled(),
            sealed: false,
            input_complete: false,
            pin_start: usize::MAX,
            pin_until: 0,
            lost_lines: None,
        }
    }

    fn disabled(source_id: SourceId, limits: ExcerptLimits, capture_id: usize) -> Self {
        Self {
            lines: OptionalVec::empty(),
            redactions: OptionalVec::empty(),
            current: OptionalVec::empty(),
            source_id,
            capture_id,
            limits,
            offset: 0,
            line_offset: 0,
            line: 1,
            line_truncated: false,
            skip_current_line: false,
            unavailable: true,
            sealed: false,
            input_complete: false,
            pin_start: usize::MAX,
            pin_until: 0,
            lost_lines: None,
        }
    }

    fn storage_bytes(&self) -> Option<usize> {
        OptionalVec::<u8>::checked_storage_bytes(
            [
                self.lines.capacity_bytes(),
                self.redactions.capacity_bytes(),
                self.current.capacity_bytes(),
            ]
            .into_iter()
            .chain(self.lines.iter().map(|line| line.bytes.capacity_bytes())),
        )
    }

    fn boundary_at(&self, offset: usize) -> Option<bool> {
        if offset == self.offset {
            return self.sealed.then_some(true);
        }
        if offset > self.offset {
            return None;
        }
        for line in &self.lines {
            let Some(relative) = offset.checked_sub(line.offset) else {
                continue;
            };
            if let Some(byte) = line.bytes.get(relative) {
                return Some(*byte & 0xc0 != 0x80);
            }
            if relative == line.bytes.len() && line.newline && !line.truncated {
                return Some(true);
            }
            if relative < line.bytes.len() || line.truncated {
                return None;
            }
        }
        let relative = offset.checked_sub(self.line_offset)?;
        self.current.get(relative).map(|byte| *byte & 0xc0 != 0x80)
    }

    fn redactions_valid(&self) -> bool {
        self.redactions.iter().all(|range| {
            (self.boundary_at(range.start()) == Some(true)
                || (!self.sealed && range.end() == usize::MAX && range.start() == self.offset))
                && (range.end() == usize::MAX
                    || self.boundary_at(range.end()) == Some(true)
                    || (!self.sealed && range.end() == self.offset))
        })
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        if self.sealed {
            self.invalidate();
            return;
        }
        if self.unavailable || !self.redactions_valid() {
            return;
        }
        if self
            .lines
            .budget()
            .is_some_and(|budget| !budget.consume_optional(bytes.len()))
        {
            self.invalidate();
            return;
        }
        for byte in bytes {
            let capacity_changed = *byte == b'\n' || self.current.is_empty();
            if *byte == b'\n' {
                if self.skip_current_line {
                    self.skip_current_line = false;
                    self.line_truncated = false;
                } else {
                    self.end_line(true);
                }
                self.line += 1;
                self.line_offset = self.offset.saturating_add(1);
            } else if self.skip_current_line {
                self.offset = self.offset.saturating_add(1);
                continue;
            } else if self.current.len() < self.limits.max_line_bytes().min(self.limits.max_bytes())
            {
                if self.current.is_empty()
                    && !self.reserve_current(16.min(self.limits.max_line_bytes()))
                {
                    self.record_lost_line(self.line);
                    self.line_truncated = true;
                    self.skip_current_line = true;
                }
                if self.skip_current_line {
                    self.offset = self.offset.saturating_add(1);
                    continue;
                }
                if self.current.len() == self.current.capacity_bytes() && !self.line_truncated {
                    if let Some(required) = self.current.len().checked_add(1) {
                        if !self.reserve_current(required) {
                            self.line_truncated = true;
                        }
                    } else {
                        self.line_truncated = true;
                    }
                }
                if self.current.len() < self.current.capacity_bytes() {
                    if !self.current.push(*byte) {
                        self.invalidate();
                        return;
                    }
                } else {
                    self.line_truncated = true;
                }
            } else {
                self.line_truncated = true;
            }
            self.offset = self.offset.saturating_add(1);
            if !capacity_changed {
                continue;
            }
            while self
                .storage_bytes()
                .is_none_or(|bytes| bytes > self.limits.max_bytes())
            {
                if let Some(index) = self.lines.iter().position(|line| !line.pinned) {
                    self.lines.remove(index);
                } else {
                    self.invalidate();
                    return;
                }
            }
        }
    }

    pub fn end_input(&mut self) {
        if !self.unavailable && !self.sealed {
            if self.skip_current_line {
                self.skip_current_line = false;
            } else {
                self.end_line(false);
            }
        }
        self.sealed = true;
        self.input_complete = true;
    }

    /// When capture stops early, only the bytes already read are fixed; open-range masking on the parser side covers later bytes.
    pub fn seal(&mut self) {
        if !self.unavailable && !self.sealed {
            if self.skip_current_line {
                self.skip_current_line = false;
            } else {
                self.line_truncated = true;
                self.end_line(false);
            }
        }
        self.sealed = true;
    }

    fn end_line(&mut self, newline: bool) {
        if self.pin_start != usize::MAX && self.line < self.pin_start {
            self.record_lost_line(self.line);
            self.current.clear();
            self.line_truncated = false;
            return;
        }
        if self.lines.len() >= self.limits.max_records() {
            if let Some(index) = self.lines.iter().position(|line| !line.pinned) {
                self.lines.remove(index);
            } else {
                self.record_lost_line(self.line);
                self.current.clear();
                self.line_truncated = false;
                return;
            }
        }
        let Some(empty) = OptionalVec::new(self.lines.budget()) else {
            self.invalidate();
            return;
        };
        let bytes = replace(&mut self.current, empty);
        let record = CapturedLine {
            bytes,
            line: self.line,
            offset: self.line_offset,
            pinned: (self.pin_start == usize::MAX && self.line <= self.limits.max_records() / 2)
                || (self.line >= self.pin_start && self.line <= self.pin_until),
            truncated: self.line_truncated,
            newline,
        };
        let Some(required) = self.lines.len().checked_add(1) else {
            self.invalidate();
            return;
        };
        loop {
            let Some(other_bytes) = self
                .storage_bytes()
                .and_then(|bytes| bytes.checked_sub(self.lines.capacity_bytes()))
                .and_then(|bytes| bytes.checked_add(record.bytes.capacity_bytes()))
            else {
                self.invalidate();
                return;
            };
            self.lines
                .set_max_bytes(self.limits.max_bytes().saturating_sub(other_bytes));
            let fits = self.lines.reserve(required)
                && self
                    .storage_bytes()
                    .and_then(|bytes| bytes.checked_add(record.bytes.capacity_bytes()))
                    .is_some_and(|bytes| bytes <= self.limits.max_bytes());
            if fits {
                if !self.lines.push(record) {
                    self.invalidate();
                }
                break;
            }
            if let Some(index) = self.lines.iter().position(|line| !line.pinned) {
                self.lines.remove(index);
            } else {
                self.invalidate();
                return;
            }
        }
        self.line_truncated = false;
    }

    pub fn pin(&mut self, span: SourceSpan, location: Option<SpanLocation>) {
        if span.source_id() != self.source_id {
            return;
        }
        let Some(location) = location else {
            return;
        };
        let first = self.limits.first_line(location.start.line());
        let last = self
            .limits
            .last_line(location.start.line())
            .min(first.saturating_add(self.limits.max_records().saturating_sub(2)));
        let first_pin = self.pin_start == usize::MAX;
        if self.line <= last {
            self.pin_start = self.pin_start.min(first);
            self.pin_until = self.pin_until.max(last);
        }
        if first_pin {
            for line in self.lines.iter_mut() {
                line.pinned = line.line >= first && line.line <= last;
            }
        }
        for line in self
            .lines
            .iter_mut()
            .filter(|line| line.line >= first && line.line <= last)
        {
            line.pinned = true;
        }
    }

    fn record_lost_line(&mut self, line: usize) {
        self.lost_lines = Some(match self.lost_lines {
            Some((first, last)) => (first.min(line), last.max(line)),
            None => (line, line),
        });
    }

    fn reserve_current(&mut self, required: usize) -> bool {
        loop {
            let Some(other_bytes) = self
                .storage_bytes()
                .and_then(|bytes| bytes.checked_sub(self.current.capacity_bytes()))
            else {
                return false;
            };
            let available = self
                .limits
                .max_bytes()
                .saturating_sub(other_bytes)
                .min(self.limits.max_line_bytes());
            if required > available {
                return false;
            }
            self.current.set_max_bytes(available);
            if self.current.reserve(required) {
                return true;
            }
            if let Some(index) = self.lines.iter().position(|line| !line.pinned) {
                self.lines.remove(index);
            } else {
                return false;
            }
        }
    }

    /// An open range covers an unfinished secret value; when finished, it can shrink only to the end of a trusted span.
    pub fn begin_secret(&mut self, start: usize) -> Option<SecretRange> {
        if self.unavailable || !self.redactions_valid() {
            return None;
        }
        if start > self.offset {
            self.invalidate();
            return None;
        }
        if self.redactions.len() >= self.limits.max_records() {
            self.invalidate();
            return None;
        }
        let Some(other_bytes) = self
            .storage_bytes()
            .and_then(|bytes| bytes.checked_sub(self.redactions.capacity_bytes()))
        else {
            self.invalidate();
            return None;
        };
        self.redactions
            .set_max_bytes(self.limits.max_bytes().saturating_sub(other_bytes));
        let Some(required) = self.redactions.len().checked_add(1) else {
            self.invalidate();
            return None;
        };
        if !self.redactions.reserve(required) {
            self.invalidate();
            return None;
        }
        let index = self.redactions.len();
        if !self.redactions.push(Span::from_ordered(start, usize::MAX))
            || self
                .storage_bytes()
                .is_none_or(|bytes| bytes > self.limits.max_bytes())
        {
            self.invalidate();
            return None;
        }
        Some(SecretRange {
            capture_id: self.capture_id,
            index,
            start,
        })
    }

    pub fn end_secret(&mut self, secret: SecretRange, end: usize) {
        if secret.capture_id != self.capture_id {
            self.invalidate();
            return;
        }
        let Some(range) = self.redactions.get_mut(secret.index) else {
            self.invalidate();
            return;
        };
        if range.start() != secret.start
            || range.end() != usize::MAX
            || secret.start > end
            || end > self.offset
        {
            self.invalidate();
            return;
        }
        let completed = Span::from_ordered(secret.start, end);
        *range = completed;
    }

    pub fn invalidate(&mut self) {
        self.unavailable = true;
        self.lines.clear();
        self.current.clear();
    }

    pub fn finish(&self, excerpts: &mut SourceExcerptSet, diagnostics: &[Diagnostic]) {
        self.finish_diagnostics(excerpts, diagnostics.iter(), LocatedSpan::location);
    }

    pub fn finish_iter<'diagnostics>(
        &self,
        excerpts: &mut SourceExcerptSet,
        diagnostics: impl Clone + Iterator<Item = &'diagnostics Diagnostic>,
    ) {
        self.finish_diagnostics(excerpts, diagnostics, LocatedSpan::location);
    }

    pub fn finish_iter_with_locations<'diagnostics>(
        &self,
        excerpts: &mut SourceExcerptSet,
        diagnostics: impl Clone + Iterator<Item = &'diagnostics Diagnostic>,
        location: impl Fn(LocatedSpan) -> Option<SpanLocation>,
    ) {
        self.finish_diagnostics(excerpts, diagnostics, location);
    }

    fn finish_diagnostics<'diagnostics>(
        &self,
        excerpts: &mut SourceExcerptSet,
        diagnostics: impl Clone + Iterator<Item = &'diagnostics Diagnostic>,
        location: impl Fn(LocatedSpan) -> Option<SpanLocation>,
    ) {
        if self.unavailable || !self.sealed || !self.redactions_valid() {
            let mut found = false;
            for span in diagnostics
                .clone()
                .flat_map(|diagnostic| {
                    diagnostic
                        .located_primary()
                        .into_iter()
                        .chain(diagnostic.labels().iter().map(|label| label.located_span()))
                })
                .map(|located| located.source_span())
                .filter(|span| span.source_id() == self.source_id)
            {
                found = true;
                excerpts.mark_span_unavailable(span);
            }
            if !found {
                excerpts.mark_missing();
            }
            return;
        }
        for diagnostic in diagnostics {
            for located in diagnostic
                .located_primary()
                .into_iter()
                .chain(diagnostic.labels().iter().map(|label| label.located_span()))
            {
                let span = located.source_span();
                if span.source_id() != self.source_id {
                    continue;
                }
                if span.span().end() > self.offset {
                    if self.input_complete {
                        excerpts.mark_span_unavailable(span);
                    } else {
                        excerpts.mark_span_missing(span);
                    }
                    excerpts.mark_missing();
                    continue;
                }
                let Some(location) = location(located).or_else(|| located.location()) else {
                    excerpts.mark_span_missing(span);
                    continue;
                };
                let first = self.limits.first_line(location.start.line());
                let last = self.limits.last_line(location.start.line());
                if self
                    .lost_lines
                    .is_some_and(|(lost_first, lost_last)| lost_first <= last && lost_last >= first)
                {
                    excerpts.mark_span_missing(span);
                }
                if location.end.line() > last {
                    excerpts.mark_span_missing(span);
                }
                let mut found = false;
                for line in self
                    .lines
                    .iter()
                    .filter(|line| line.line >= first && line.line <= last)
                {
                    let mut bytes: &[u8] = &line.bytes;
                    if line.truncated {
                        if let Err(error) = from_utf8(bytes) {
                            if error.error_len().is_none() {
                                bytes = bytes.get(..error.valid_up_to()).unwrap_or_default();
                            }
                        }
                    }
                    let Ok(raw) = from_utf8(bytes) else {
                        excerpts.mark_span_missing(span);
                        continue;
                    };
                    let raw = if line.newline {
                        raw.strip_suffix('\r').unwrap_or(raw)
                    } else {
                        raw
                    };
                    excerpts.insert_line(
                        span,
                        ExcerptLine {
                            source_id: self.source_id,
                            line: line.line,
                            offset: line.offset,
                            raw,
                            redactions: &self.redactions,
                            truncated: line.truncated,
                        },
                    );
                    excerpts.add_marker(
                        span,
                        location,
                        line.line,
                        MarkerInput {
                            offset: line.offset,
                            raw,
                            redactions: &self.redactions,
                        },
                    );
                    if line.line == location.start.line() {
                        found = true;
                    }
                }
                if !found {
                    excerpts.mark_span_missing(span);
                }
            }
        }
    }
}

impl fmt::Debug for SourceCapture {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SourceCapture")
            .field("line_count", &self.lines.len())
            .field("unavailable", &self.unavailable)
            .finish()
    }
}
