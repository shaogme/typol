use opeo::{ErrSlot, OResult, Out};
use typol_core::{
    ByteSource, Diagnostic, SourceCapture, SourceId, SourceReadError, SourceSpan, Span,
};

use crate::{diagnostics::EmergencyDiagnostics, resolver::SchemaInput, session::CompileBudget};

pub(super) struct BoundedSource<'a> {
    source: &'a mut dyn ByteSource<SourceReadError>,
    budget: &'a mut CompileBudget,
    capture: &'a mut SourceCapture,
    source_id: SourceId,
    source_limit: usize,
    actual_bytes: usize,
    eof: bool,
}

impl BoundedSource<'_> {
    pub(super) fn new<'a>(
        source: &'a mut dyn ByteSource<SourceReadError>,
        budget: &'a mut CompileBudget,
        capture: &'a mut SourceCapture,
        source_id: SourceId,
        source_limit: usize,
    ) -> BoundedSource<'a> {
        BoundedSource {
            source,
            budget,
            capture,
            source_id,
            source_limit,
            actual_bytes: 0,
            eof: false,
        }
    }

    pub(super) const fn actual_bytes(&self) -> usize {
        self.actual_bytes
    }

    pub(super) const fn reached_eof(&self) -> bool {
        self.eof
    }

    fn record_bytes(&mut self, count: usize) -> Option<bool> {
        self.actual_bytes = self.actual_bytes.checked_add(count)?;
        self.budget.charge_source_bytes(count)
    }
}

impl ByteSource<Diagnostic> for BoundedSource<'_> {
    fn read<'slot>(
        &mut self,
        output: &mut [u8],
        source_id: SourceId,
        offset: usize,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, usize> {
        if self.eof || output.is_empty() {
            return Ok(0);
        }
        if source_id != self.source_id {
            return Err(out.reborrow().fail(EmergencyDiagnostics::internal(
                SourceSpan::new(self.source_id, Span::point(offset)),
                schema_context!("source-input", "source-id-mismatch"),
            )));
        }
        let source_remaining = self.source_limit.saturating_sub(self.actual_bytes);
        let total_remaining = self.budget.source_bytes_remaining();
        let allowance = source_remaining.min(total_remaining);
        let request = output.len().min(allowance.saturating_add(1));
        let Some(buffer) = output.get_mut(..request) else {
            self.capture.invalidate();
            return Err(out.reborrow().fail(EmergencyDiagnostics::internal(
                SourceSpan::new(source_id, Span::point(offset)),
                schema_context!("source-input", "read-buffer-range-invalid"),
            )));
        };
        let mut source_errors = ErrSlot::<SourceReadError>::new();
        let read = source_errors
            .try_call(|source_out| self.source.read(buffer, source_id, offset, source_out));
        let count = match read {
            Ok(count) => count,
            Err(caught) => {
                let error = caught.take();
                if let Some(consumed) = error.consumed_bytes() {
                    let Some(within_total) = self.record_bytes(consumed) else {
                        self.capture.invalidate();
                        return Err(out.reborrow().fail(EmergencyDiagnostics::capacity(
                            SourceSpan::new(source_id, Span::point(offset)),
                        )));
                    };
                    if consumed > allowance || !within_total {
                        self.capture.invalidate();
                        return Err(out.reborrow().fail(EmergencyDiagnostics::resource(
                            SourceSpan::new(
                                source_id,
                                Span::point(offset.saturating_add(allowance)),
                            ),
                        )));
                    }
                }
                self.capture.invalidate();
                return Err(out.reborrow().fail(SchemaInput::source_read_diagnostic(
                    error, source_id, offset,
                )));
            }
        };
        if count > request {
            self.capture.invalidate();
            return Err(out.reborrow().fail(EmergencyDiagnostics::internal(
                SourceSpan::new(source_id, Span::point(offset)),
                schema_context!("source-input", "read-count-exceeds-request"),
            )));
        }
        let Some(within_total) = self.record_bytes(count) else {
            self.capture.invalidate();
            return Err(out
                .reborrow()
                .fail(EmergencyDiagnostics::capacity(SourceSpan::new(
                    source_id,
                    Span::point(offset),
                ))));
        };
        if count > allowance || !within_total {
            self.capture.invalidate();
            let limit_offset = offset.saturating_add(allowance);
            return Err(out
                .reborrow()
                .fail(EmergencyDiagnostics::resource(SourceSpan::new(
                    source_id,
                    Span::point(limit_offset),
                ))));
        }
        let Some(bytes) = output.get(..count) else {
            self.capture.invalidate();
            return Err(out.reborrow().fail(EmergencyDiagnostics::internal(
                SourceSpan::new(source_id, Span::point(offset)),
                schema_context!("source-input", "read-output-range-invalid"),
            )));
        };
        self.capture.feed(bytes);
        if count == 0 {
            self.eof = true;
            self.capture.end_input();
        }
        Ok(count)
    }
}
