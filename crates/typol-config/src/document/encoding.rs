use core::{
    fmt::{self, LowerExp, Write as FmtWrite},
    str,
};
use opeo::ErrSlot;
use typol_core::{
    BudgetError, ChargedString, ChargedVec, ExecutionBudget, ValueEntries, ValueKind,
    ValuePayloadView, ValueView,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigEditErrorKind {
    InvalidPath,
    ValueNotExplicit,
    EncodingFailed,
    EditConflict,
    InvalidContainer,
    EntryAlreadyExists,
    ValidationFailed,
    ResourceLimitExceeded,
    AllocationFailed,
}

pub(super) struct ScalarEncoder;

pub(super) struct EntryEncoding<'value, 'source> {
    pub(super) name: &'source str,
    pub(super) value: ValueView<'value>,
    pub(super) map_key: bool,
    pub(super) colon_spacing: &'source str,
    pub(super) budget: Option<&'source ExecutionBudget>,
    pub(super) limit: usize,
}

struct FixedText {
    bytes: [u8; 64],
    length: usize,
}

struct EntriesEncoding<'value> {
    entries: ValueEntries<'value>,
    open: &'static str,
    close: &'static str,
    limit: usize,
}

enum EncodeFrame<'value> {
    Value(ValueView<'value>),
    Entries(ValueEntries<'value>, bool),
    Text(&'value str),
    Quoted(&'value str),
}

impl ScalarEncoder {
    pub(super) fn encode_value(
        value: ValueView<'_>,
        budget: Option<&ExecutionBudget>,
        limit: usize,
    ) -> Result<ChargedString, ConfigEditErrorKind> {
        let mut errors = ErrSlot::<BudgetError>::new();
        let mut output = errors
            .try_call(|out| ChargedString::with_capacity(0, budget, out))
            .map_err(|caught| budget_error_kind(caught.take()))?;
        Self::encode_value_into(&mut output, value, budget, limit)?;
        Ok(output)
    }

    pub(super) fn encode_value_into(
        output: &mut ChargedString,
        value: ValueView<'_>,
        budget: Option<&ExecutionBudget>,
        limit: usize,
    ) -> Result<(), ConfigEditErrorKind> {
        let mut errors = ErrSlot::<BudgetError>::new();
        let mut frames = errors
            .try_call(|out| ChargedVec::new(budget, out))
            .map_err(|caught| budget_error_kind(caught.take()))?;
        errors
            .try_call(|out| frames.try_push(EncodeFrame::Value(value), out))
            .map_err(|caught| budget_error_kind(caught.take()))?;
        while let Some(frame) = frames.pop() {
            match frame {
                EncodeFrame::Text(text) => append_text(output, text, limit)?,
                EncodeFrame::Quoted(text) => append_escaped(output, text, limit)?,
                EncodeFrame::Entries(entries, map_keys) => {
                    append_text(output, "{", limit)?;
                    push_frame(&mut frames, EncodeFrame::Text("}"))?;
                    push_entries(&mut frames, entries, map_keys)?;
                }
                EncodeFrame::Value(value) => match value.as_ref() {
                    ValueKind::Signed(number) => {
                        append_format(output, format_args!("{number}"), limit)?;
                    }
                    ValueKind::Unsigned(number) => {
                        append_format(output, format_args!("{number}"), limit)?;
                    }
                    ValueKind::Float32(number) => append_float(output, number, limit)?,
                    ValueKind::Float64(number) => append_float(output, number, limit)?,
                    ValueKind::String(text) => append_escaped(output, text, limit)?,
                    ValueKind::Bool(value) => {
                        append_text(output, if value { "true" } else { "false" }, limit)?;
                    }
                    ValueKind::Optional(Some(value)) => {
                        push_frame(&mut frames, EncodeFrame::Value(value))?;
                    }
                    ValueKind::Optional(None) => append_text(output, "none", limit)?,
                    ValueKind::Array(entries) => {
                        encode_entries(
                            output,
                            &mut frames,
                            EntriesEncoding {
                                entries,
                                open: "[",
                                close: "]",
                                limit,
                            },
                        )?;
                    }
                    ValueKind::Map(entries) => {
                        append_text(output, "{", limit)?;
                        push_frame(&mut frames, EncodeFrame::Text("}"))?;
                        push_entries(&mut frames, entries, true)?;
                    }
                    ValueKind::Structure(entries) => {
                        push_frame(&mut frames, EncodeFrame::Entries(entries, false))?;
                    }
                    ValueKind::Enumeration { variant, payload } => {
                        append_text(output, ".", limit)?;
                        append_text(output, variant, limit)?;
                        match payload {
                            ValuePayloadView::Unit => {}
                            ValuePayloadView::Single(value) => {
                                append_text(output, "(", limit)?;
                                push_frame(&mut frames, EncodeFrame::Text(")"))?;
                                push_frame(&mut frames, EncodeFrame::Value(value))?;
                            }
                            ValuePayloadView::Structure(entries) => {
                                append_text(output, " ", limit)?;
                                push_frame(&mut frames, EncodeFrame::Entries(entries, false))?;
                            }
                        }
                    }
                },
            }
        }
        Ok(())
    }

    pub(super) fn encode_entry_into(
        output: &mut ChargedString,
        request: EntryEncoding<'_, '_>,
    ) -> Result<(), ConfigEditErrorKind> {
        if request.map_key {
            append_escaped(output, request.name, request.limit)?;
        } else {
            append_text(output, request.name, request.limit)?;
        }
        append_text(output, ":", request.limit)?;
        append_text(output, request.colon_spacing, request.limit)?;
        Self::encode_value_into(output, request.value, request.budget, request.limit)
    }
}

fn push_frame<'value>(
    frames: &mut ChargedVec<EncodeFrame<'value>>,
    frame: EncodeFrame<'value>,
) -> Result<(), ConfigEditErrorKind> {
    let mut errors = ErrSlot::<BudgetError>::new();
    errors
        .try_call(|out| frames.try_push(frame, out))
        .map_err(|caught| budget_error_kind(caught.take()))
}

fn encode_entries<'value>(
    output: &mut ChargedString,
    frames: &mut ChargedVec<EncodeFrame<'value>>,
    request: EntriesEncoding<'value>,
) -> Result<(), ConfigEditErrorKind> {
    append_text(output, request.open, request.limit)?;
    push_frame(frames, EncodeFrame::Text(request.close))?;
    push_entries(frames, request.entries, false)
}

fn push_entries<'value>(
    frames: &mut ChargedVec<EncodeFrame<'value>>,
    entries: ValueEntries<'value>,
    map_keys: bool,
) -> Result<(), ConfigEditErrorKind> {
    for index in (0..entries.len()).rev() {
        let Some(entry) = entries.get(index) else {
            return Err(ConfigEditErrorKind::AllocationFailed);
        };
        push_frame(frames, EncodeFrame::Value(entry.value()))?;
        if let Some(name) = entry.name() {
            push_frame(frames, EncodeFrame::Text(": "))?;
            push_frame(
                frames,
                if map_keys {
                    EncodeFrame::Quoted(name)
                } else {
                    EncodeFrame::Text(name)
                },
            )?;
        }
        if index > 0 {
            push_frame(frames, EncodeFrame::Text(", "))?;
        }
    }
    Ok(())
}

impl FixedText {
    fn new() -> Self {
        Self {
            bytes: [0; 64],
            length: 0,
        }
    }

    fn as_str(&self) -> Result<&str, ConfigEditErrorKind> {
        let text = self
            .bytes
            .get(..self.length)
            .ok_or(ConfigEditErrorKind::EncodingFailed)?;
        str::from_utf8(text).map_err(|_| ConfigEditErrorKind::EncodingFailed)
    }
}

impl FmtWrite for FixedText {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        let Some(end) = self.length.checked_add(value.len()) else {
            return Err(fmt::Error);
        };
        let Some(destination) = self.bytes.get_mut(self.length..end) else {
            return Err(fmt::Error);
        };
        destination.copy_from_slice(value.as_bytes());
        self.length = end;
        Ok(())
    }
}

fn append_float(
    output: &mut ChargedString,
    value: impl LowerExp,
    limit: usize,
) -> Result<(), ConfigEditErrorKind> {
    let mut scientific = FixedText::new();
    scientific
        .write_fmt(format_args!("{value:e}"))
        .map_err(|_| ConfigEditErrorKind::EncodingFailed)?;
    let (mantissa, exponent) = scientific
        .as_str()?
        .split_once('e')
        .ok_or(ConfigEditErrorKind::EncodingFailed)?;
    let exponent = exponent
        .parse::<i32>()
        .map_err(|_| ConfigEditErrorKind::EncodingFailed)?;
    let negative = mantissa.starts_with('-');
    let unsigned = mantissa.strip_prefix('-').unwrap_or(mantissa);
    let decimal = unsigned.find('.').unwrap_or(unsigned.len());
    let decimal = i32::try_from(decimal)
        .map_err(|_| ConfigEditErrorKind::ResourceLimitExceeded)?
        .checked_add(exponent)
        .ok_or(ConfigEditErrorKind::ResourceLimitExceeded)?;
    let mut digits = FixedText::new();
    for character in unsigned.chars().filter(|character| *character != '.') {
        digits
            .write_char(character)
            .map_err(|_| ConfigEditErrorKind::EncodingFailed)?;
    }
    if negative {
        append_text(output, "-", limit)?;
    }
    if decimal <= 0 {
        append_text(output, "0.", limit)?;
        let zeros = usize::try_from(decimal.unsigned_abs())
            .map_err(|_| ConfigEditErrorKind::ResourceLimitExceeded)?;
        append_zeros(output, zeros, limit)?;
        append_text(output, digits.as_str()?, limit)
    } else {
        let decimal =
            usize::try_from(decimal).map_err(|_| ConfigEditErrorKind::ResourceLimitExceeded)?;
        let digits = digits.as_str()?;
        if decimal >= digits.len() {
            append_text(output, digits, limit)?;
            append_zeros(output, decimal - digits.len(), limit)?;
            append_text(output, ".0", limit)
        } else {
            let Some(whole) = digits.get(..decimal) else {
                return Err(ConfigEditErrorKind::EncodingFailed);
            };
            let Some(fraction) = digits.get(decimal..) else {
                return Err(ConfigEditErrorKind::EncodingFailed);
            };
            append_text(output, whole, limit)?;
            append_text(output, ".", limit)?;
            append_text(output, fraction, limit)
        }
    }
}

fn append_zeros(
    output: &mut ChargedString,
    mut count: usize,
    limit: usize,
) -> Result<(), ConfigEditErrorKind> {
    const ZEROS: &str = "0000000000000000000000000000000000000000000000000000000000000000";
    while count > 0 {
        let length = count.min(ZEROS.len());
        append_text(output, &ZEROS[..length], limit)?;
        count -= length;
    }
    Ok(())
}

fn append_format(
    output: &mut ChargedString,
    arguments: fmt::Arguments<'_>,
    limit: usize,
) -> Result<(), ConfigEditErrorKind> {
    let mut formatted = FixedText::new();
    formatted
        .write_fmt(arguments)
        .map_err(|_| ConfigEditErrorKind::EncodingFailed)?;
    append_text(output, formatted.as_str()?, limit)
}

fn append_escaped(
    output: &mut ChargedString,
    text: &str,
    limit: usize,
) -> Result<(), ConfigEditErrorKind> {
    append_text(output, "\"", limit)?;
    for character in text.chars() {
        match character {
            '\\' => append_text(output, "\\\\", limit)?,
            '"' => append_text(output, "\\\"", limit)?,
            '\n' => append_text(output, "\\n", limit)?,
            '\r' => append_text(output, "\\r", limit)?,
            '\t' => append_text(output, "\\t", limit)?,
            character if character.is_control() => {
                return Err(ConfigEditErrorKind::EncodingFailed);
            }
            character => {
                let mut encoded = [0; 4];
                append_text(output, character.encode_utf8(&mut encoded), limit)?;
            }
        }
    }
    append_text(output, "\"", limit)
}

fn append_text(
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
        .map_err(|caught| budget_error_kind(caught.take()))?;
    Ok(())
}

fn budget_error_kind(error: BudgetError) -> ConfigEditErrorKind {
    match error {
        BudgetError::Resource | BudgetError::CapacityExceeded => {
            ConfigEditErrorKind::ResourceLimitExceeded
        }
        BudgetError::Allocation | BudgetError::InvariantViolation(_) => {
            ConfigEditErrorKind::AllocationFailed
        }
    }
}
