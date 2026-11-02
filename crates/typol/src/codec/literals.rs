use crate::diagnostics::{CodecErrors, CodecStage, Failure};
use core::{
    fmt::{self, Display, LowerExp, Write},
    ops::Range,
    str::from_utf8,
};
use opeo::{OResult, Out, ResultOutExt};
use typol_core::{DiagnosticContext, diagnostic_context};

static LITERAL_INVARIANT: DiagnosticContext =
    diagnostic_context!("typol", "encoding", "numeric-literal-invariant");

pub(crate) struct Literal {
    bytes: [u8; 768],
    length: usize,
}

impl Literal {
    pub(crate) fn number<'slot>(
        value: impl Display,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, Self> {
        let mut result = Self {
            bytes: [0; 768],
            length: 0,
        };
        write!(&mut result, "{value}")
            .map_err(|_| {
                CodecErrors::emergency(CodecStage::Encoding, Failure::internal(LITERAL_INVARIANT))
            })
            .store_err_into(out.reborrow())?;
        Ok(result)
    }

    pub(crate) fn float<'slot>(
        value: impl LowerExp,
        check: impl for<'check_slot, 'check_borrow> FnOnce(
            usize,
            Out<'check_slot, 'check_borrow, CodecErrors>,
        ) -> OResult<'check_slot, ()>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, Self> {
        let mut result = Self {
            bytes: [0; 768],
            length: 0,
        };
        write!(&mut result, "{value:e}")
            .map_err(|_| Failure::internal(LITERAL_INVARIANT))
            .store_err_into(out.reborrow())?;
        // `LowerExp` preserves the shortest round-trippable significant digits; expansion only moves the decimal point and adds zeros.
        if let Some(position) = result.text(out.reborrow())?.find(['e', 'E']) {
            let exponent_start = position
                .checked_add(1)
                .ok_or(Failure::internal(LITERAL_INVARIANT))
                .store_err_into(out.reborrow())?;
            let text = result.text(out.reborrow())?;
            let exponent_text = text
                .get(exponent_start..)
                .ok_or(Failure::internal(LITERAL_INVARIANT))
                .store_err_into(out.reborrow())?;
            let exponent: i32 = exponent_text
                .parse()
                .map_err(|_| Failure::internal(LITERAL_INVARIANT))
                .store_err_into(out.reborrow())?;
            let negative = result.byte(0, out.reborrow())? == b'-';
            let start = usize::from(negative);
            let mantissa = result.range(start..position, out.reborrow())?;
            let point = mantissa
                .iter()
                .position(|byte| *byte == b'.')
                .unwrap_or(mantissa.len());
            let decimal = i32::try_from(point)
                .map_err(|_| Failure::internal(LITERAL_INVARIANT))
                .store_err_into(out.reborrow())?;
            let decimal = decimal
                .checked_add(exponent)
                .ok_or(Failure::internal(LITERAL_INVARIANT))
                .store_err_into(out.reborrow())?;
            let count = mantissa.iter().filter(|byte| **byte != b'.').count();
            let body_length = if decimal <= 0 {
                let zeros = usize::try_from(decimal.unsigned_abs())
                    .map_err(|_| Failure::internal(LITERAL_INVARIANT))
                    .store_err_into(out.reborrow())?;
                2usize
                    .checked_add(zeros)
                    .and_then(|length| length.checked_add(count))
                    .ok_or(Failure::internal(LITERAL_INVARIANT))
                    .store_err_into(out.reborrow())?
            } else {
                let decimal = usize::try_from(decimal)
                    .map_err(|_| Failure::internal(LITERAL_INVARIANT))
                    .store_err_into(out.reborrow())?;
                if decimal >= count {
                    decimal
                        .checked_add(2)
                        .ok_or(Failure::internal(LITERAL_INVARIANT))
                        .store_err_into(out.reborrow())?
                } else {
                    count
                        .checked_add(1)
                        .ok_or(Failure::internal(LITERAL_INVARIANT))
                        .store_err_into(out.reborrow())?
                }
            };
            let length = start
                .checked_add(body_length)
                .filter(|length| *length <= Self::CAPACITY)
                .ok_or(Failure::internal(LITERAL_INVARIANT))
                .store_err_into(out.reborrow())?;
            check(length, out.reborrow())?;

            let mut target = start;
            for source in start..position {
                let byte = result.byte(source, out.reborrow())?;
                if byte != b'.' {
                    result.set_byte(target, byte, out.reborrow())?;
                    target = target
                        .checked_add(1)
                        .ok_or(Failure::internal(LITERAL_INVARIANT))
                        .store_err_into(out.reborrow())?;
                }
            }
            let source_end = start
                .checked_add(count)
                .ok_or(Failure::internal(LITERAL_INVARIANT))
                .store_err_into(out.reborrow())?;
            if decimal <= 0 {
                let zeros = usize::try_from(decimal.unsigned_abs())
                    .map_err(|_| Failure::internal(LITERAL_INVARIANT))
                    .store_err_into(out.reborrow())?;
                let offset = start
                    .checked_add(2)
                    .and_then(|value| value.checked_add(zeros))
                    .ok_or(Failure::internal(LITERAL_INVARIANT))
                    .store_err_into(out.reborrow())?;
                result.copy_within(start..source_end, offset, out.reborrow())?;
                let decimal_point = start
                    .checked_add(1)
                    .ok_or(Failure::internal(LITERAL_INVARIANT))
                    .store_err_into(out.reborrow())?;
                result.set_byte(start, b'0', out.reborrow())?;
                result.set_byte(decimal_point, b'.', out.reborrow())?;
                let padding_start = start
                    .checked_add(2)
                    .ok_or(Failure::internal(LITERAL_INVARIANT))
                    .store_err_into(out.reborrow())?;
                result.fill(padding_start..offset, b'0', out.reborrow())?;
            } else {
                let decimal = usize::try_from(decimal)
                    .map_err(|_| Failure::internal(LITERAL_INVARIANT))
                    .store_err_into(out.reborrow())?;
                let point = start
                    .checked_add(decimal)
                    .ok_or(Failure::internal(LITERAL_INVARIANT))
                    .store_err_into(out.reborrow())?;
                if decimal >= count {
                    result.fill(source_end..point, b'0', out.reborrow())?;
                    result.set_byte(point, b'.', out.reborrow())?;
                    let suffix = point
                        .checked_add(1)
                        .ok_or(Failure::internal(LITERAL_INVARIANT))
                        .store_err_into(out.reborrow())?;
                    result.set_byte(suffix, b'0', out.reborrow())?;
                } else {
                    let shifted = point
                        .checked_add(1)
                        .ok_or(Failure::internal(LITERAL_INVARIANT))
                        .store_err_into(out.reborrow())?;
                    result.copy_within(point..source_end, shifted, out.reborrow())?;
                    result.set_byte(point, b'.', out.reborrow())?;
                }
            }
            result.length = length;
        } else {
            let suffix = usize::from(!result.text(out.reborrow())?.contains('.')) * 2;
            let length = result
                .length
                .checked_add(suffix)
                .filter(|length| *length <= Self::CAPACITY)
                .ok_or(Failure::internal(LITERAL_INVARIANT))
                .store_err_into(out.reborrow())?;
            check(length, out.reborrow())?;
            if suffix > 0 {
                result
                    .write_str(".0")
                    .map_err(|_| Failure::internal(LITERAL_INVARIANT))
                    .store_err_into(out.reborrow())?;
            }
        }
        Ok(result)
    }

    pub(crate) fn text<'slot>(&self, mut out: Out<'slot, '_, CodecErrors>) -> OResult<'slot, &str> {
        let bytes = self
            .bytes
            .get(..self.length)
            .ok_or(Failure::internal(LITERAL_INVARIANT));
        let bytes = bytes.store_err_into(out.reborrow())?;
        let text = from_utf8(bytes)
            .map_err(|_| Failure::internal(LITERAL_INVARIANT))
            .store_err_into(out.reborrow())?;
        Ok(text)
    }

    pub(crate) fn escape<'slot, 'borrow>(
        character: char,
        out: Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, Option<&'static str>> {
        match character {
            '"' => Ok(Some("\\\"")),
            '\\' => Ok(Some("\\\\")),
            '\n' => Ok(Some("\\n")),
            '\r' => Ok(Some("\\r")),
            '\t' => Ok(Some("\\t")),
            '\u{0}'..='\u{1f}' | '\u{7f}'..='\u{9f}' => Err(out.fail(CodecErrors::emergency(
                CodecStage::Encoding,
                Failure::Encoding,
            ))),
            _ => Ok(None),
        }
    }

    fn byte<'slot>(
        &self,
        index: usize,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, u8> {
        let byte = self
            .bytes
            .get(index)
            .copied()
            .ok_or(Failure::internal(LITERAL_INVARIANT))
            .store_err_into(out.reborrow())?;
        Ok(byte)
    }

    fn range<'slot>(
        &self,
        range: Range<usize>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, &[u8]> {
        let range = self
            .bytes
            .get(range)
            .ok_or(Failure::internal(LITERAL_INVARIANT))
            .store_err_into(out.reborrow())?;
        Ok(range)
    }

    fn set_byte<'slot>(
        &mut self,
        index: usize,
        byte: u8,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ()> {
        let target = self
            .bytes
            .get_mut(index)
            .ok_or(Failure::internal(LITERAL_INVARIANT))
            .store_err_into(out.reborrow())?;
        *target = byte;
        Ok(())
    }

    fn fill<'slot>(
        &mut self,
        range: Range<usize>,
        byte: u8,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ()> {
        let target = self
            .bytes
            .get_mut(range)
            .ok_or(Failure::internal(LITERAL_INVARIANT))
            .store_err_into(out.reborrow())?;
        target.fill(byte);
        Ok(())
    }

    fn copy_within<'slot>(
        &mut self,
        range: Range<usize>,
        target: usize,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ()> {
        let length = range
            .end
            .checked_sub(range.start)
            .ok_or(Failure::internal(LITERAL_INVARIANT))
            .store_err_into(out.reborrow())?;
        let end = target
            .checked_add(length)
            .ok_or(Failure::internal(LITERAL_INVARIANT))
            .store_err_into(out.reborrow())?;
        self.bytes
            .get(range.clone())
            .ok_or(Failure::internal(LITERAL_INVARIANT))
            .store_err_into(out.reborrow())?;
        self.bytes
            .get(target..end)
            .ok_or(Failure::internal(LITERAL_INVARIANT))
            .store_err_into(out.reborrow())?;
        let bytes = self
            .bytes
            .get_mut(..)
            .ok_or(Failure::internal(LITERAL_INVARIANT))
            .store_err_into(out.reborrow())?;
        bytes.copy_within(range, target);
        Ok(())
    }
}

impl Write for Literal {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let end = self
            .length
            .checked_add(text.len())
            .filter(|end| *end <= self.bytes.len())
            .ok_or(fmt::Error)?;
        let target = self.bytes.get_mut(self.length..end).ok_or(fmt::Error)?;
        target.copy_from_slice(text.as_bytes());
        self.length = end;
        Ok(())
    }
}

impl Literal {
    const CAPACITY: usize = 768;
}
