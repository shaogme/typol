#[cfg(test)]
mod tests;

use opeo::{OResult, Out, ResultOutExt};
use typol_core::{BudgetError, DiagnosticContext, ExecutionBudget, diagnostic_context};

static TEXT_VALIDATION_INVARIANT: DiagnosticContext = diagnostic_context!(
    "typol-semantic",
    "text-validation",
    "address-or-text-boundary-invalid",
);

pub struct TextChecks<'a> {
    budget: Option<&'a ExecutionBudget>,
}

impl<'a> TextChecks<'a> {
    pub const fn new(budget: Option<&'a ExecutionBudget>) -> Self {
        Self { budget }
    }

    fn byte<'slot, E>(
        &self,
        value: &str,
        index: usize,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, Option<u8>>
    where
        E: From<BudgetError>,
    {
        let Some(byte) = value.as_bytes().get(index).copied() else {
            return Ok(None);
        };
        if let Some(budget) = self.budget {
            budget.consume(1, out.reborrow())?;
        }
        Ok(Some(byte))
    }

    fn required_byte<'slot, E>(
        &self,
        value: &str,
        index: usize,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, u8>
    where
        E: From<BudgetError>,
    {
        let byte = self.byte(value, index, out.reborrow())?;
        byte.ok_or(BudgetError::invariant(&TEXT_VALIDATION_INVARIANT))
            .store_err_into(out.reborrow())
    }

    fn find<'slot, E>(
        &self,
        value: &str,
        delimiter: u8,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, Option<usize>>
    where
        E: From<BudgetError>,
    {
        for index in 0..value.len() {
            if self.byte(value, index, out.reborrow())? == Some(delimiter) {
                return Ok(Some(index));
            }
        }
        Ok(None)
    }

    fn ascii<'slot, E>(&self, value: &str, mut out: Out<'slot, '_, E>) -> OResult<'slot, bool>
    where
        E: From<BudgetError>,
    {
        for index in 0..value.len() {
            if !self.required_byte(value, index, out.reborrow())?.is_ascii() {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub fn without_root<'s, 'slot, E>(
        &self,
        value: &'s str,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, &'s str>
    where
        E: From<BudgetError>,
    {
        if !value.is_empty() && self.byte(value, value.len() - 1, out.reborrow())? == Some(b'.') {
            value
                .get(..value.len() - 1)
                .ok_or(BudgetError::invariant(&TEXT_VALIDATION_INVARIANT))
                .store_err_into(out.reborrow())
        } else {
            Ok(value)
        }
    }

    pub fn ipv4<'slot, E>(
        &self,
        value: &str,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, Option<[u8; 4]>>
    where
        E: From<BudgetError>,
    {
        let mut address = [0; 4];
        let mut group = 0;
        let mut number = 0u16;
        let mut digits = 0;
        let mut zero = false;
        for index in 0..value.len() {
            let byte = self.required_byte(value, index, out.reborrow())?;
            if byte == b'.' {
                if digits == 0 || group == 3 {
                    return Ok(None);
                }
                *address
                    .get_mut(group)
                    .ok_or(BudgetError::invariant(&TEXT_VALIDATION_INVARIANT))
                    .store_err_into(out.reborrow())? = number as u8;
                group += 1;
                number = 0;
                digits = 0;
            } else {
                if !byte.is_ascii_digit() || (digits > 0 && zero) {
                    return Ok(None);
                }
                if digits == 0 {
                    zero = byte == b'0';
                }
                number = number * 10 + u16::from(byte - b'0');
                digits += 1;
                if number > 255 || digits > 3 {
                    return Ok(None);
                }
            }
        }
        if group != 3 || digits == 0 {
            return Ok(None);
        }
        *address
            .get_mut(3)
            .ok_or(BudgetError::invariant(&TEXT_VALIDATION_INVARIANT))
            .store_err_into(out.reborrow())? = number as u8;
        Ok(Some(address))
    }

    pub fn ipv6<'slot, E>(
        &self,
        value: &str,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, Option<[u16; 8]>>
    where
        E: From<BudgetError>,
    {
        if self.find(value, b':', out.reborrow())?.is_none()
            || self.find(value, b'%', out.reborrow())?.is_some()
            || !self.ascii(value, out.reborrow())?
        {
            return Ok(None);
        }
        let mut double = None;
        for index in 0..value.len().saturating_sub(1) {
            if self.byte(value, index, out.reborrow())? == Some(b':')
                && self.byte(value, index + 1, out.reborrow())? == Some(b':')
            {
                if double.is_some() {
                    return Ok(None);
                }
                double = Some(index);
            }
        }
        let (left_text, right_text) = match double {
            Some(index) => {
                let suffix = index
                    .checked_add(2)
                    .ok_or(BudgetError::invariant(&TEXT_VALIDATION_INVARIANT))
                    .store_err_into(out.reborrow())?;
                (
                    value
                        .get(..index)
                        .ok_or(BudgetError::invariant(&TEXT_VALIDATION_INVARIANT))
                        .store_err_into(out.reborrow())?,
                    value
                        .get(suffix..)
                        .ok_or(BudgetError::invariant(&TEXT_VALIDATION_INVARIANT))
                        .store_err_into(out.reborrow())?,
                )
            }
            None => (value, ""),
        };
        let mut left = [0; 8];
        let Some(left_len) = self.groups(left_text, &mut left, double.is_none(), out.reborrow())?
        else {
            return Ok(None);
        };
        let mut right = [0; 8];
        let Some(right_len) = self.groups(right_text, &mut right, true, out.reborrow())? else {
            return Ok(None);
        };
        if double.is_none() {
            return Ok((left_len == 8).then_some(left));
        }
        if left_len + right_len >= 8 {
            return Ok(None);
        }
        let mut result = [0; 8];
        let left_source = left
            .get(..left_len)
            .ok_or(BudgetError::invariant(&TEXT_VALIDATION_INVARIANT))
            .store_err_into(out.reborrow())?;
        result
            .get_mut(..left_len)
            .ok_or(BudgetError::invariant(&TEXT_VALIDATION_INVARIANT))
            .store_err_into(out.reborrow())?
            .copy_from_slice(left_source);
        let right_start = 8_usize
            .checked_sub(right_len)
            .ok_or(BudgetError::invariant(&TEXT_VALIDATION_INVARIANT))
            .store_err_into(out.reborrow())?;
        let right_source = right
            .get(..right_len)
            .ok_or(BudgetError::invariant(&TEXT_VALIDATION_INVARIANT))
            .store_err_into(out.reborrow())?;
        result
            .get_mut(right_start..)
            .ok_or(BudgetError::invariant(&TEXT_VALIDATION_INVARIANT))
            .store_err_into(out.reborrow())?
            .copy_from_slice(right_source);
        Ok(Some(result))
    }

    fn groups<'slot, E>(
        &self,
        text: &str,
        output: &mut [u16; 8],
        allow_ipv4: bool,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, Option<usize>>
    where
        E: From<BudgetError>,
    {
        if text.is_empty() {
            return Ok(Some(0));
        }
        let mut start = 0;
        let mut count = 0;
        loop {
            let remaining = text
                .get(start..)
                .ok_or(BudgetError::invariant(&TEXT_VALIDATION_INVARIANT))
                .store_err_into(out.reborrow())?;
            let end = match self.find(remaining, b':', out.reborrow())? {
                Some(offset) => start
                    .checked_add(offset)
                    .ok_or(BudgetError::invariant(&TEXT_VALIDATION_INVARIANT))
                    .store_err_into(out.reborrow())?,
                None => text.len(),
            };
            let part = text
                .get(start..end)
                .ok_or(BudgetError::invariant(&TEXT_VALIDATION_INVARIANT))
                .store_err_into(out.reborrow())?;
            if part.is_empty() {
                return Ok(None);
            }
            if self.find(part, b'.', out.reborrow())?.is_some() {
                if !allow_ipv4 || end != text.len() || count > 6 {
                    return Ok(None);
                }
                let Some(address) = self.ipv4(part, out.reborrow())? else {
                    return Ok(None);
                };
                let first = address
                    .get(..2)
                    .ok_or(BudgetError::invariant(&TEXT_VALIDATION_INVARIANT))
                    .store_err_into(out.reborrow())?;
                let second = address
                    .get(2..)
                    .ok_or(BudgetError::invariant(&TEXT_VALIDATION_INVARIANT))
                    .store_err_into(out.reborrow())?;
                let [first_high, first_low] = first else {
                    return Err(
                        out.fail(E::from(BudgetError::invariant(&TEXT_VALIDATION_INVARIANT)))
                    );
                };
                let [second_high, second_low] = second else {
                    return Err(
                        out.fail(E::from(BudgetError::invariant(&TEXT_VALIDATION_INVARIANT)))
                    );
                };
                *output
                    .get_mut(count)
                    .ok_or(BudgetError::invariant(&TEXT_VALIDATION_INVARIANT))
                    .store_err_into(out.reborrow())? =
                    u16::from_be_bytes([*first_high, *first_low]);
                let next = count
                    .checked_add(1)
                    .ok_or(BudgetError::invariant(&TEXT_VALIDATION_INVARIANT))
                    .store_err_into(out.reborrow())?;
                *output
                    .get_mut(next)
                    .ok_or(BudgetError::invariant(&TEXT_VALIDATION_INVARIANT))
                    .store_err_into(out.reborrow())? =
                    u16::from_be_bytes([*second_high, *second_low]);
                count += 2;
            } else {
                if count >= 8 || part.len() > 4 {
                    return Ok(None);
                }
                let mut number = 0u16;
                for index in 0..part.len() {
                    let byte = self.required_byte(part, index, out.reborrow())?;
                    let digit = match byte {
                        b'0'..=b'9' => byte - b'0',
                        b'a'..=b'f' => byte - b'a' + 10,
                        b'A'..=b'F' => byte - b'A' + 10,
                        _ => return Ok(None),
                    };
                    number = number * 16 + u16::from(digit);
                }
                *output
                    .get_mut(count)
                    .ok_or(BudgetError::invariant(&TEXT_VALIDATION_INVARIANT))
                    .store_err_into(out.reborrow())? = number;
                count += 1;
            }
            if end == text.len() {
                break;
            }
            start = end
                .checked_add(1)
                .ok_or(BudgetError::invariant(&TEXT_VALIDATION_INVARIANT))
                .store_err_into(out.reborrow())?;
        }
        Ok(Some(count))
    }

    fn dns<'slot, E>(&self, value: &str, mut out: Out<'slot, '_, E>) -> OResult<'slot, bool>
    where
        E: From<BudgetError>,
    {
        let mut start = 0;
        while start < value.len() {
            let remaining = value
                .get(start..)
                .ok_or(BudgetError::invariant(&TEXT_VALIDATION_INVARIANT))
                .store_err_into(out.reborrow())?;
            let end = match self.find(remaining, b'.', out.reborrow())? {
                Some(offset) => start
                    .checked_add(offset)
                    .ok_or(BudgetError::invariant(&TEXT_VALIDATION_INVARIANT))
                    .store_err_into(out.reborrow())?,
                None => value.len(),
            };
            let label = value
                .get(start..end)
                .ok_or(BudgetError::invariant(&TEXT_VALIDATION_INVARIANT))
                .store_err_into(out.reborrow())?;
            if label.is_empty() || label.len() > 63 {
                return Ok(false);
            }
            let first = self.required_byte(label, 0, out.reborrow())?;
            let last = self.required_byte(label, label.len() - 1, out.reborrow())?;
            if !first.is_ascii_alphanumeric() || !last.is_ascii_alphanumeric() {
                return Ok(false);
            }
            for index in 0..label.len() {
                let byte = self.required_byte(label, index, out.reborrow())?;
                if !byte.is_ascii_alphanumeric() && byte != b'-' {
                    return Ok(false);
                }
            }
            if end == value.len() {
                return Ok(true);
            }
            start = end
                .checked_add(1)
                .ok_or(BudgetError::invariant(&TEXT_VALIDATION_INVARIANT))
                .store_err_into(out.reborrow())?;
        }
        Ok(false)
    }

    pub fn host<'slot, E>(&self, value: &str, mut out: Out<'slot, '_, E>) -> OResult<'slot, bool>
    where
        E: From<BudgetError>,
    {
        if self.ipv6(value, out.reborrow())?.is_some() {
            return Ok(true);
        }
        let value = self.without_root(value, out.reborrow())?;
        let mut parts = 1;
        let mut numeric = true;
        let mut empty = true;
        for index in 0..value.len() {
            let byte = self.required_byte(value, index, out.reborrow())?;
            if byte == b'.' {
                if empty {
                    numeric = false;
                }
                parts += 1;
                empty = true;
            } else {
                numeric &= byte.is_ascii_digit();
                empty = false;
            }
        }
        if parts == 4 && numeric && !empty {
            return Ok(self.ipv4(value, out.reborrow())?.is_some());
        }
        if value.is_empty() || value.len() > 253 || !self.ascii(value, out.reborrow())? {
            return Ok(false);
        }
        self.dns(value, out)
    }

    pub fn email<'slot, E>(&self, value: &str, mut out: Out<'slot, '_, E>) -> OResult<'slot, bool>
    where
        E: From<BudgetError>,
    {
        if !self.ascii(value, out.reborrow())? {
            return Ok(false);
        }
        let Some(at) = self.find(value, b'@', out.reborrow())? else {
            return Ok(false);
        };
        let domain_start = at
            .checked_add(1)
            .ok_or(BudgetError::invariant(&TEXT_VALIDATION_INVARIANT))
            .store_err_into(out.reborrow())?;
        let local = value
            .get(..at)
            .ok_or(BudgetError::invariant(&TEXT_VALIDATION_INVARIANT))
            .store_err_into(out.reborrow())?;
        let domain = value
            .get(domain_start..)
            .ok_or(BudgetError::invariant(&TEXT_VALIDATION_INVARIANT))
            .store_err_into(out.reborrow())?;
        if self.find(domain, b'@', out.reborrow())?.is_some()
            || local.is_empty()
            || local.len() > 64
        {
            return Ok(false);
        }
        if self.byte(local, 0, out.reborrow())? == Some(b'.')
            || self.byte(local, local.len() - 1, out.reborrow())? == Some(b'.')
        {
            return Ok(false);
        }
        let mut previous = false;
        for index in 0..local.len() {
            let byte = self.required_byte(local, index, out.reborrow())?;
            if (previous && byte == b'.')
                || (!byte.is_ascii_alphanumeric() && !b"._+-".contains(&byte))
            {
                return Ok(false);
            }
            previous = byte == b'.';
        }
        if domain.len() > 253
            || (!domain.is_empty()
                && self.byte(domain, domain.len() - 1, out.reborrow())? == Some(b'.'))
            || self.find(domain, b'.', out.reborrow())?.is_none()
        {
            return Ok(false);
        }
        self.dns(domain, out)
    }

    pub fn cidr<'slot, E>(
        &self,
        value: &str,
        v4_only: bool,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, bool>
    where
        E: From<BudgetError>,
    {
        let Some(slash) = self.find(value, b'/', out.reborrow())? else {
            return Ok(false);
        };
        let prefix_start = slash
            .checked_add(1)
            .ok_or(BudgetError::invariant(&TEXT_VALIDATION_INVARIANT))
            .store_err_into(out.reborrow())?;
        let address = value
            .get(..slash)
            .ok_or(BudgetError::invariant(&TEXT_VALIDATION_INVARIANT))
            .store_err_into(out.reborrow())?;
        let prefix = value
            .get(prefix_start..)
            .ok_or(BudgetError::invariant(&TEXT_VALIDATION_INVARIANT))
            .store_err_into(out.reborrow())?;
        if prefix.is_empty() {
            return Ok(false);
        }
        let mut number = 0u16;
        for index in 0..prefix.len() {
            let byte = self.required_byte(prefix, index, out.reborrow())?;
            if !byte.is_ascii_digit() {
                return Ok(false);
            }
            let Some(next) = number
                .checked_mul(10)
                .and_then(|value| value.checked_add(u16::from(byte - b'0')))
            else {
                return Ok(false);
            };
            number = next;
        }
        if self.ipv4(address, out.reborrow())?.is_some() {
            Ok(number <= 32)
        } else {
            Ok(!v4_only && self.ipv6(address, out.reborrow())?.is_some() && number <= 128)
        }
    }
}

pub struct TextValidation;

impl TextValidation {
    pub fn valid_host<'slot, 'borrow, E>(
        value: &str,
        out: Out<'slot, 'borrow, E>,
    ) -> OResult<'slot, bool>
    where
        E: From<BudgetError>,
    {
        TextChecks::new(None).host(value, out)
    }

    pub fn valid_email<'slot, 'borrow, E>(
        value: &str,
        out: Out<'slot, 'borrow, E>,
    ) -> OResult<'slot, bool>
    where
        E: From<BudgetError>,
    {
        TextChecks::new(None).email(value, out)
    }

    pub fn valid_cidr<'slot, 'borrow, E>(
        value: &str,
        v4_only: bool,
        out: Out<'slot, 'borrow, E>,
    ) -> OResult<'slot, bool>
    where
        E: From<BudgetError>,
    {
        TextChecks::new(None).cidr(value, v4_only, out)
    }

    pub fn parse_ipv4<'slot, 'borrow, E>(
        value: &str,
        out: Out<'slot, 'borrow, E>,
    ) -> OResult<'slot, Option<[u8; 4]>>
    where
        E: From<BudgetError>,
    {
        TextChecks::new(None).ipv4(value, out)
    }

    pub fn parse_ipv6<'slot, 'borrow, E>(
        value: &str,
        out: Out<'slot, 'borrow, E>,
    ) -> OResult<'slot, Option<[u16; 8]>>
    where
        E: From<BudgetError>,
    {
        TextChecks::new(None).ipv6(value, out)
    }
}
