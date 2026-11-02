use crate::{DisplayError, DisplayFailurePoint};
use core::ops::Range;
use std::{
    fmt::{self, Display, LowerExp, Write},
    str::from_utf8,
};

use opeo::{OResult, Out, ResultOutExt};
use typol_core::{ChargedVec, ExecutionBudget};

const LITERAL_CAPACITY: usize = 768;

pub(crate) struct Literal {
    bytes: [u8; LITERAL_CAPACITY],
    length: usize,
}

impl Literal {
    pub(crate) fn number<'slot>(
        value: impl Display,
        mut out: Out<'slot, '_, DisplayError>,
    ) -> OResult<'slot, Self> {
        let mut result = Self {
            bytes: [0; 768],
            length: 0,
        };
        write!(&mut result, "{value}")
            .map_err(|_| DisplayError::ResourceLimitExceeded)
            .store_err_into(out.reborrow())?;
        Ok(result)
    }

    pub(crate) fn float<'slot>(
        value: impl LowerExp,
        check: impl for<'call> FnOnce(usize, Out<'slot, 'call, DisplayError>) -> OResult<'slot, ()>,
        mut out: Out<'slot, '_, DisplayError>,
    ) -> OResult<'slot, Self> {
        let mut result = Self {
            bytes: [0; LITERAL_CAPACITY],
            length: 0,
        };
        write!(&mut result, "{value:e}")
            .map_err(|_| DisplayError::internal(DisplayFailurePoint::NumberFormatting))
            .store_err_into(out.reborrow())?;
        // Rust’s target-width `LowerExp` provides the shortest round-trip significant digits; expansion leaves those digits unchanged.
        if let Some(position) = result.text(out.reborrow())?.find(['e', 'E']) {
            let exponent_start = position
                .checked_add(1)
                .ok_or(DisplayError::internal(DisplayFailurePoint::FloatExpansion))
                .store_err_into(out.reborrow())?;
            let exponent_text = result.text(out.reborrow())?;
            let exponent_digits = exponent_text
                .get(exponent_start..)
                .ok_or(DisplayError::internal(DisplayFailurePoint::FloatExpansion))
                .store_err_into(out.reborrow())?;
            let exponent: i32 = exponent_digits
                .parse()
                .map_err(|_| DisplayError::internal(DisplayFailurePoint::FloatExpansion))
                .store_err_into(out.reborrow())?;
            let negative = result.byte(0, out.reborrow())? == b'-';
            let start = usize::from(negative);
            let mantissa_text = result.text(out.reborrow())?;
            let mantissa = mantissa_text
                .get(start..position)
                .ok_or(DisplayError::internal(DisplayFailurePoint::FloatExpansion));
            let mantissa = mantissa.store_err_into(out.reborrow())?;
            let point = mantissa.find('.').unwrap_or(mantissa.len());
            let decimal = i32::try_from(point)
                .map_err(|_| DisplayError::internal(DisplayFailurePoint::FloatExpansion))
                .and_then(|point| {
                    point
                        .checked_add(exponent)
                        .ok_or(DisplayError::internal(DisplayFailurePoint::FloatExpansion))
                })
                .store_err_into(out.reborrow())?;
            let count = mantissa.bytes().filter(|byte| *byte != b'.').count();
            let length = if decimal <= 0 {
                2usize
                    .checked_add(decimal.unsigned_abs() as usize)
                    .and_then(|length| length.checked_add(count))
                    .ok_or(DisplayError::internal(DisplayFailurePoint::FloatExpansion))
                    .store_err_into(out.reborrow())?
            } else if decimal as usize >= count {
                (decimal as usize)
                    .checked_add(2)
                    .ok_or(DisplayError::internal(DisplayFailurePoint::FloatExpansion))
                    .store_err_into(out.reborrow())?
            } else {
                count
                    .checked_add(1)
                    .ok_or(DisplayError::internal(DisplayFailurePoint::FloatExpansion))
                    .store_err_into(out.reborrow())?
            };
            let length = length
                .checked_add(start)
                .filter(|length| *length <= LITERAL_CAPACITY)
                .ok_or(DisplayError::internal(DisplayFailurePoint::FloatExpansion))
                .store_err_into(out.reborrow())?;
            check(length, out.reborrow())?;
            let mut target = start;
            for source in start..position {
                let byte = result.byte(source, out.reborrow())?;
                if byte != b'.' {
                    result.set_byte(target, byte, out.reborrow())?;
                    target = target
                        .checked_add(1)
                        .ok_or(DisplayError::internal(DisplayFailurePoint::FloatExpansion))
                        .store_err_into(out.reborrow())?;
                }
            }
            let digits_end = start
                .checked_add(count)
                .ok_or(DisplayError::internal(DisplayFailurePoint::FloatExpansion))
                .store_err_into(out.reborrow())?;
            // First compact the significant digits, then shift them according to the decimal-point position; `copy_within` handles overlapping moves.
            if decimal <= 0 {
                let zeroes_start = start
                    .checked_add(2)
                    .ok_or(DisplayError::internal(DisplayFailurePoint::FloatExpansion))
                    .store_err_into(out.reborrow())?;
                let offset = zeroes_start
                    .checked_add(decimal.unsigned_abs() as usize)
                    .ok_or(DisplayError::internal(DisplayFailurePoint::FloatExpansion))
                    .store_err_into(out.reborrow())?;
                result.copy_within(start..digits_end, offset, out.reborrow())?;
                result.set_byte(start, b'0', out.reborrow())?;
                result.set_byte(
                    start
                        .checked_add(1)
                        .ok_or(DisplayError::internal(DisplayFailurePoint::FloatExpansion))
                        .store_err_into(out.reborrow())?,
                    b'.',
                    out.reborrow(),
                )?;
                result.fill(zeroes_start..offset, b'0', out.reborrow())?;
            } else if decimal as usize >= count {
                let point = start
                    .checked_add(decimal as usize)
                    .ok_or(DisplayError::internal(DisplayFailurePoint::FloatExpansion))
                    .store_err_into(out.reborrow())?;
                result.fill(digits_end..point, b'0', out.reborrow())?;
                result.set_byte(point, b'.', out.reborrow())?;
                result.set_byte(
                    point
                        .checked_add(1)
                        .ok_or(DisplayError::internal(DisplayFailurePoint::FloatExpansion))
                        .store_err_into(out.reborrow())?,
                    b'0',
                    out.reborrow(),
                )?;
            } else {
                let point = start
                    .checked_add(decimal as usize)
                    .ok_or(DisplayError::internal(DisplayFailurePoint::FloatExpansion))
                    .store_err_into(out.reborrow())?;
                let after_point = point
                    .checked_add(1)
                    .ok_or(DisplayError::internal(DisplayFailurePoint::FloatExpansion))
                    .store_err_into(out.reborrow())?;
                result.copy_within(point..digits_end, after_point, out.reborrow())?;
                result.set_byte(point, b'.', out.reborrow())?;
            }
            result.length = length;
        } else {
            let suffix = usize::from(!result.text(out.reborrow())?.contains('.')) * 2;
            let length = result
                .length
                .checked_add(suffix)
                .ok_or(DisplayError::internal(DisplayFailurePoint::FloatExpansion))
                .store_err_into(out.reborrow())?;
            check(length, out.reborrow())?;
            if suffix > 0 {
                result
                    .write_str(".0")
                    .map_err(|_| DisplayError::internal(DisplayFailurePoint::FloatExpansion))
                    .store_err_into(out.reborrow())?;
            }
        }
        Ok(result)
    }

    pub(crate) fn text<'slot>(
        &self,
        mut out: Out<'slot, '_, DisplayError>,
    ) -> OResult<'slot, &str> {
        let bytes = self
            .bytes
            .get(..self.length)
            .ok_or(DisplayError::internal(
                DisplayFailurePoint::LiteralBufferAccess,
            ))
            .store_err_into(out.reborrow())?;
        let text = from_utf8(bytes)
            .map_err(|_| DisplayError::internal(DisplayFailurePoint::LiteralUtf8Validation))
            .store_err_into(out.reborrow())?;
        Ok(text)
    }

    fn byte<'slot>(
        &self,
        index: usize,
        mut out: Out<'slot, '_, DisplayError>,
    ) -> OResult<'slot, u8> {
        let byte = self
            .bytes
            .get(index)
            .copied()
            .ok_or(DisplayError::internal(
                DisplayFailurePoint::LiteralBufferAccess,
            ))
            .store_err_into(out.reborrow())?;
        Ok(byte)
    }

    fn set_byte<'slot>(
        &mut self,
        index: usize,
        value: u8,
        mut out: Out<'slot, '_, DisplayError>,
    ) -> OResult<'slot, ()> {
        let byte = self
            .bytes
            .get_mut(index)
            .ok_or(DisplayError::internal(
                DisplayFailurePoint::LiteralBufferAccess,
            ))
            .store_err_into(out.reborrow())?;
        *byte = value;
        Ok(())
    }

    fn fill<'slot>(
        &mut self,
        range: Range<usize>,
        value: u8,
        mut out: Out<'slot, '_, DisplayError>,
    ) -> OResult<'slot, ()> {
        let bytes = self
            .bytes
            .get_mut(range)
            .ok_or(DisplayError::internal(
                DisplayFailurePoint::LiteralBufferAccess,
            ))
            .store_err_into(out.reborrow())?;
        bytes.fill(value);
        Ok(())
    }

    fn copy_within<'slot>(
        &mut self,
        source: Range<usize>,
        destination: usize,
        mut out: Out<'slot, '_, DisplayError>,
    ) -> OResult<'slot, ()> {
        let source_bytes = self
            .bytes
            .get(source.clone())
            .ok_or(DisplayError::internal(
                DisplayFailurePoint::LiteralBufferAccess,
            ))
            .store_err_into(out.reborrow())?;
        let destination_end = destination
            .checked_add(source_bytes.len())
            .ok_or(DisplayError::internal(
                DisplayFailurePoint::LiteralBufferAccess,
            ))
            .store_err_into(out.reborrow())?;
        let _ = self
            .bytes
            .get_mut(destination..destination_end)
            .ok_or(DisplayError::internal(
                DisplayFailurePoint::LiteralBufferAccess,
            ))
            .store_err_into(out.reborrow())?;
        self.bytes.copy_within(source, destination);
        Ok(())
    }
}

impl Write for Literal {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        if !text.is_ascii() {
            return Err(fmt::Error);
        }
        let end = self
            .length
            .checked_add(text.len())
            .filter(|end| *end <= self.bytes.len())
            .ok_or(fmt::Error)?;
        self.bytes
            .get_mut(self.length..end)
            .ok_or(fmt::Error)?
            .copy_from_slice(text.as_bytes());
        self.length = end;
        Ok(())
    }
}

pub(crate) struct TextBuffer {
    bytes: ChargedVec<u8>,
    budget: ExecutionBudget,
    limit: usize,
    string_limit: usize,
}

impl TextBuffer {
    pub(crate) fn new<'slot>(
        budget: &ExecutionBudget,
        limit: usize,
        string_limit: usize,
        mut out: Out<'slot, '_, DisplayError>,
    ) -> OResult<'slot, Self> {
        let budget = budget.try_clone(out.reborrow())?;
        let bytes = ChargedVec::new(Some(&budget), out.reborrow())?;
        Ok(Self {
            bytes,
            budget,
            limit,
            string_limit,
        })
    }

    pub(crate) fn write<'slot>(
        &mut self,
        text: &str,
        mut out: Out<'slot, '_, DisplayError>,
    ) -> OResult<'slot, ()> {
        self.budget
            .construct_text(text.len(), self.limit, out.reborrow())?;
        self.budget.consume(text.len(), out.reborrow())?;
        self.bytes.try_reserve(text.len(), out.reborrow())?;
        for byte in text.bytes() {
            self.bytes.try_push(byte, out.reborrow())?;
        }
        Ok(())
    }

    pub(crate) fn string<'slot>(
        &mut self,
        text: &str,
        mut out: Out<'slot, '_, DisplayError>,
    ) -> OResult<'slot, ()> {
        if text.len() > self.string_limit {
            return Err(out.fail(DisplayError::ResourceLimitExceeded));
        }
        self.write("\"", out.reborrow())?;
        self.escape(text, out.reborrow())?;
        self.write("\"", out.reborrow())
    }

    pub(crate) fn escape<'slot>(
        &mut self,
        text: &str,
        mut out: Out<'slot, '_, DisplayError>,
    ) -> OResult<'slot, ()> {
        for c in text.chars() {
            self.budget.consume(c.len_utf8(), out.reborrow())?;
            match c {
                '\\' => self.write("\\\\", out.reborrow())?,
                '"' => self.write("\\\"", out.reborrow())?,
                '\n' => self.write("\\n", out.reborrow())?,
                '\r' => self.write("\\r", out.reborrow())?,
                '\t' => self.write("\\t", out.reborrow())?,
                '\0'..='\u{1f}'
                | '\u{7f}'..='\u{9f}'
                | '\u{2028}'..='\u{202e}'
                | '\u{2066}'..='\u{2069}' => {
                    for escaped in c.escape_unicode() {
                        self.write(escaped.encode_utf8(&mut [0; 4]), out.reborrow())?;
                    }
                }
                _ => self.write(c.encode_utf8(&mut [0; 4]), out.reborrow())?,
            }
        }
        Ok(())
    }

    pub(crate) fn number<'slot>(
        &mut self,
        value: impl Display,
        mut out: Out<'slot, '_, DisplayError>,
    ) -> OResult<'slot, ()> {
        let text = Literal::number(value, out.reborrow())?;
        let text = text.text(out.reborrow())?;
        self.budget.consume(text.len(), out.reborrow())?;
        self.write(text, out.reborrow())
    }

    pub(crate) fn float<'slot>(
        &mut self,
        value: impl LowerExp,
        mut out: Out<'slot, '_, DisplayError>,
    ) -> OResult<'slot, ()> {
        let text = Literal::float(
            value,
            |length, check_out| {
                self.budget
                    .consume_with(length, check_out, DisplayError::from)
            },
            out.reborrow(),
        )?;
        let text = text.text(out.reborrow())?;
        self.budget.consume(text.len(), out.reborrow())?;
        self.write(text, out.reborrow())
    }

    pub(crate) fn finish<'slot>(
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
