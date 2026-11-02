use std::str::from_utf8;

use opeo::{ErrSlot, OResult, Out, ResultOutExt};
use typol_core::{BudgetError, ChargedVec, ExecutionBudget, InstancePath, PathSegment};

use super::{DisplayError, DisplayFailurePoint, RenderOptions};

pub(super) trait FormatOutput {
    fn step<'slot, 'borrow>(
        &self,
        count: usize,
        out: Out<'slot, 'borrow, DisplayError>,
    ) -> OResult<'slot, ()>;

    fn append<'slot, 'borrow>(
        &mut self,
        text: &str,
        out: Out<'slot, 'borrow, DisplayError>,
    ) -> OResult<'slot, ()>;

    fn number<'slot>(
        &mut self,
        mut value: usize,
        mut out: Out<'slot, '_, DisplayError>,
    ) -> OResult<'slot, ()> {
        let mut digits = [0; usize::BITS as usize];
        let mut start = digits.len();
        loop {
            self.step(1, out.reborrow())?;
            start = start
                .checked_sub(1)
                .ok_or(DisplayError::internal(
                    DisplayFailurePoint::OutputNumberFormatting,
                ))
                .store_err_into(out.reborrow())?;
            let digit = digits
                .get_mut(start)
                .ok_or(DisplayError::internal(
                    DisplayFailurePoint::OutputNumberFormatting,
                ))
                .store_err_into(out.reborrow())?;
            *digit = b'0' + (value % 10) as u8;
            value /= 10;
            if value == 0 {
                break;
            }
        }
        let bytes = digits
            .get(start..)
            .ok_or(DisplayError::internal(
                DisplayFailurePoint::OutputNumberFormatting,
            ))
            .store_err_into(out.reborrow())?;
        let text = from_utf8(bytes)
            .map_err(|_| DisplayError::internal(DisplayFailurePoint::OutputNumberFormatting))
            .store_err_into(out.reborrow())?;
        self.append(text, out.reborrow())
    }

    fn escape<'slot>(
        &mut self,
        text: &str,
        quote: bool,
        mut out: Out<'slot, '_, DisplayError>,
    ) -> OResult<'slot, ()> {
        for character in text.chars() {
            self.step(character.len_utf8(), out.reborrow())?;
            match character {
                '\\' => self.append("\\\\", out.reborrow())?,
                '\n' => self.append("\\n", out.reborrow())?,
                '\r' => self.append("\\r", out.reborrow())?,
                '\t' => self.append("\\t", out.reborrow())?,
                '"' if quote => self.append("\\\"", out.reborrow())?,
                '\0'..='\u{1f}'
                | '\u{7f}'..='\u{9f}'
                | '\u{2028}'..='\u{202e}'
                | '\u{2066}'..='\u{2069}' => {
                    for escaped in character.escape_unicode() {
                        let mut encoded = [0; 4];
                        self.append(escaped.encode_utf8(&mut encoded), out.reborrow())?;
                    }
                }
                _ => {
                    let mut encoded = [0; 4];
                    self.append(character.encode_utf8(&mut encoded), out.reborrow())?;
                }
            }
        }
        Ok(())
    }

    fn path<'slot>(
        &mut self,
        path: &InstancePath,
        mut out: Out<'slot, '_, DisplayError>,
    ) -> OResult<'slot, ()> {
        self.append("$", out.reborrow())?;
        for segment in path.segments() {
            self.step(1, out.reborrow())?;
            match segment {
                PathSegment::Field(name) => {
                    self.append(".", out.reborrow())?;
                    if InstancePath::is_schema_identifier(name) {
                        self.escape(name, false, out.reborrow())?;
                    } else {
                        self.append("\"", out.reborrow())?;
                        self.escape(name, true, out.reborrow())?;
                        self.append("\"", out.reborrow())?;
                    }
                }
                PathSegment::Index(index) => {
                    self.append("[", out.reborrow())?;
                    self.number(*index, out.reborrow())?;
                    self.append("]", out.reborrow())?;
                }
                PathSegment::MapKey(key) => {
                    self.append("[\"", out.reborrow())?;
                    self.escape(key, true, out.reborrow())?;
                    self.append("\"]", out.reborrow())?;
                }
                PathSegment::EnumPayload(variant) => {
                    self.append("::", out.reborrow())?;
                    if InstancePath::is_schema_identifier(variant) {
                        self.escape(variant, false, out.reborrow())?;
                    } else {
                        self.append("\"", out.reborrow())?;
                        self.escape(variant, true, out.reborrow())?;
                        self.append("\"", out.reborrow())?;
                    }
                }
            }
        }
        Ok(())
    }
}

pub(super) struct RenderBuffer {
    bytes: ChargedVec<u8>,
    budget: ExecutionBudget,
    max_output_bytes: usize,
}

impl RenderBuffer {
    pub(super) fn new<'slot>(
        options: RenderOptions,
        mut out: Out<'slot, '_, DisplayError>,
    ) -> OResult<'slot, Self> {
        let mut budget_errors = ErrSlot::<BudgetError>::new();
        let budget = match budget_errors
            .try_call(|budget_out| ExecutionBudget::new(options.execution(), budget_out))
        {
            Ok(budget) => budget,
            Err(caught) => return Err(out.fail(DisplayError::from(caught.take()))),
        };
        let bytes = ChargedVec::new(Some(&budget), out.reborrow())?;
        Ok(Self {
            bytes,
            budget,
            max_output_bytes: options.max_output_bytes(),
        })
    }

    pub(super) fn finish<'slot>(
        self,
        mut out: Out<'slot, '_, DisplayError>,
    ) -> OResult<'slot, String> {
        self.budget.consume(self.bytes.len(), out.reborrow())?;
        let text = String::from_utf8(self.bytes.into_unmetered_vec())
            .map_err(|_| DisplayError::internal(DisplayFailurePoint::OutputUtf8Validation))
            .store_err_into(out.reborrow())?;
        Ok(text)
    }
}

impl FormatOutput for RenderBuffer {
    fn step<'slot>(
        &self,
        count: usize,
        mut out: Out<'slot, '_, DisplayError>,
    ) -> OResult<'slot, ()> {
        self.budget.consume(count, out.reborrow())?;
        Ok(())
    }

    fn append<'slot>(
        &mut self,
        text: &str,
        mut out: Out<'slot, '_, DisplayError>,
    ) -> OResult<'slot, ()> {
        self.budget
            .construct_text(text.len(), self.max_output_bytes, out.reborrow())?;
        self.budget.consume(text.len(), out.reborrow())?;
        self.bytes.try_reserve(text.len(), out.reborrow())?;
        for byte in text.bytes() {
            self.bytes.try_push(byte, out.reborrow())?;
        }
        Ok(())
    }
}
