use super::edit::ConfigEditErrorKind;
use core::{
    fmt::{self, LowerExp, Write as FmtWrite},
    str,
};
use opeo::ErrSlot;
use typol_core::{
    BudgetError, ChargedString, ChargedVec, ExecutionBudget, TextLayout, ValueEntries, ValueKind,
    ValuePayloadView, ValueScalar, ValueSource, ValueView,
};

pub(super) struct ScalarEncoder;

#[derive(Clone, Copy)]
pub(super) struct ValueEncodingContext<'indent> {
    pub(super) layout: TextLayout,
    pub(super) line_break: &'static str,
    pub(super) indent: &'indent str,
    pub(super) indent_unit: &'indent str,
    pub(super) indent_levels: usize,
    pub(super) depth: usize,
}

pub(super) struct EntryEncoding<'value, 'source, 'indent> {
    pub(super) name: &'source str,
    pub(super) value: ValueView<'value>,
    pub(super) map_key: bool,
    pub(super) colon_spacing: &'source str,
    pub(super) context: ValueEncodingContext<'indent>,
    pub(super) budget: Option<&'source ExecutionBudget>,
    pub(super) limit: usize,
}

#[derive(Clone, Copy)]
struct EncodeOptions<'budget, 'indent> {
    context: ValueEncodingContext<'indent>,
    budget: Option<&'budget ExecutionBudget>,
    limit: usize,
}

#[derive(Clone, Copy)]
struct ContainerEncoding<'value> {
    entries: ValueEntries<'value>,
    map_keys: bool,
    close: &'static str,
    depth: usize,
}

struct FixedText {
    bytes: [u8; 64],
    length: usize,
}

enum EncodeFrame<'value> {
    Value(ValueView<'value>, usize),
    Entries(ContainerEncoding<'value>),
    Text(&'static str),
    FieldText(&'value str, bool),
    Indent(usize),
    PrettyNext(ContainerEncoding<'value>, usize),
}

impl ScalarEncoder {
    pub(super) fn encode_value(
        value: ValueView<'_>,
        context: ValueEncodingContext<'_>,
        budget: Option<&ExecutionBudget>,
        limit: usize,
    ) -> Result<ChargedString, ConfigEditErrorKind> {
        let mut errors = ErrSlot::<BudgetError>::new();
        let mut output = errors
            .try_call(|out| ChargedString::with_capacity(0, budget, out))
            .map_err(|caught| budget_error_kind(caught.take()))?;
        Self::encode_value_into(&mut output, value, context, budget, limit)?;
        Ok(output)
    }

    pub(super) fn encode_value_into(
        output: &mut ChargedString,
        value: ValueView<'_>,
        context: ValueEncodingContext<'_>,
        budget: Option<&ExecutionBudget>,
        limit: usize,
    ) -> Result<(), ConfigEditErrorKind> {
        let options = EncodeOptions {
            context,
            budget,
            limit,
        };
        let mut errors = ErrSlot::<BudgetError>::new();
        let mut frames = errors
            .try_call(|out| ChargedVec::new(budget, out))
            .map_err(|caught| budget_error_kind(caught.take()))?;
        push_frame(&mut frames, EncodeFrame::Value(value, context.depth))?;
        while let Some(frame) = frames.pop() {
            consume(options.budget, 1)?;
            match frame {
                EncodeFrame::Text(text) => append_text(output, text, options.limit)?,
                EncodeFrame::FieldText(text, map_key) => {
                    if map_key {
                        append_user_escaped(output, text, options.budget, options.limit)?;
                    } else {
                        append_user_text(output, text, options.budget, options.limit)?;
                    }
                }
                EncodeFrame::Indent(depth) => {
                    append_indent(output, depth, options)?;
                }
                EncodeFrame::Entries(container) => {
                    push_compact_entries(&mut frames, container, options)?;
                }
                EncodeFrame::PrettyNext(container, index) => {
                    append_text(output, ",", options.limit)?;
                    append_text(output, options.context.line_break, options.limit)?;
                    if index == container.entries.len() {
                        append_indent(output, container.depth, options)?;
                        append_text(output, container.close, options.limit)?;
                    } else {
                        append_indent(output, container.depth.saturating_add(1), options)?;
                        append_pretty_entry_prefix(output, container, index, options)?;
                        push_frame(
                            &mut frames,
                            EncodeFrame::PrettyNext(container, index.saturating_add(1)),
                        )?;
                        let Some(entry) = container.entries.get(index) else {
                            return Err(ConfigEditErrorKind::EncodingFailed);
                        };
                        push_frame(
                            &mut frames,
                            EncodeFrame::Value(entry.value(), container.depth.saturating_add(1)),
                        )?;
                    }
                }
                EncodeFrame::Value(value, depth) => {
                    if let Some(reference) = value.environment_reference() {
                        append_environment(
                            output,
                            reference.name(),
                            reference.fallback(),
                            options,
                        )?;
                    } else if value.source() == ValueSource::EnvironmentVariable {
                        return Err(ConfigEditErrorKind::EncodingFailed);
                    } else {
                        match value.as_ref() {
                            ValueKind::Signed(number) => {
                                append_format(output, format_args!("{number}"), options.limit)?;
                            }
                            ValueKind::Unsigned(number) => {
                                append_format(output, format_args!("{number}"), options.limit)?;
                            }
                            ValueKind::Float32(number) => {
                                append_float(output, number, options.limit)?
                            }
                            ValueKind::Float64(number) => {
                                append_float(output, number, options.limit)?
                            }
                            ValueKind::String(text) => {
                                append_user_escaped(output, text, options.budget, options.limit)?;
                            }
                            ValueKind::Bool(value) => {
                                append_text(
                                    output,
                                    if value { "true" } else { "false" },
                                    options.limit,
                                )?;
                            }
                            ValueKind::Optional(Some(value)) => {
                                if let Some(reference) = value.environment_reference() {
                                    append_environment(
                                        output,
                                        reference.name(),
                                        reference.fallback(),
                                        options,
                                    )?;
                                } else {
                                    push_frame(&mut frames, EncodeFrame::Value(value, depth))?;
                                }
                            }
                            ValueKind::Optional(None) => {
                                append_text(output, "none", options.limit)?
                            }
                            ValueKind::Array(entries) => start_value_container(
                                output,
                                &mut frames,
                                ContainerEncoding {
                                    entries,
                                    map_keys: false,
                                    close: "]",
                                    depth,
                                },
                                "[",
                                options,
                            )?,
                            ValueKind::Map(entries) => start_value_container(
                                output,
                                &mut frames,
                                ContainerEncoding {
                                    entries,
                                    map_keys: true,
                                    close: "}",
                                    depth,
                                },
                                "{",
                                options,
                            )?,
                            ValueKind::Structure(entries) => start_value_container(
                                output,
                                &mut frames,
                                ContainerEncoding {
                                    entries,
                                    map_keys: false,
                                    close: "}",
                                    depth,
                                },
                                "{",
                                options,
                            )?,
                            ValueKind::Enumeration { variant, payload } => {
                                append_text(output, ".", options.limit)?;
                                append_user_text(output, variant, options.budget, options.limit)?;
                                match payload {
                                    ValuePayloadView::Unit => {}
                                    ValuePayloadView::Single(value) => {
                                        append_text(output, "(", options.limit)?;
                                        if options.context.layout == TextLayout::Pretty {
                                            append_text(
                                                output,
                                                options.context.line_break,
                                                options.limit,
                                            )?;
                                            append_indent(
                                                output,
                                                depth.saturating_add(1),
                                                options,
                                            )?;
                                            push_frame(&mut frames, EncodeFrame::Text(")"))?;
                                            push_frame(&mut frames, EncodeFrame::Indent(depth))?;
                                            push_frame(
                                                &mut frames,
                                                EncodeFrame::Text(options.context.line_break),
                                            )?;
                                            push_frame(
                                                &mut frames,
                                                EncodeFrame::Value(value, depth.saturating_add(1)),
                                            )?;
                                        } else {
                                            push_frame(&mut frames, EncodeFrame::Text(")"))?;
                                            push_frame(
                                                &mut frames,
                                                EncodeFrame::Value(value, depth),
                                            )?;
                                        }
                                    }
                                    ValuePayloadView::Structure(entries) => {
                                        append_text(output, " ", options.limit)?;
                                        append_text(output, "{", options.limit)?;
                                        start_container(
                                            output,
                                            &mut frames,
                                            ContainerEncoding {
                                                entries,
                                                map_keys: false,
                                                close: "}",
                                                depth,
                                            },
                                            options,
                                        )?;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    pub(super) fn encode_entry_into(
        output: &mut ChargedString,
        request: EntryEncoding<'_, '_, '_>,
    ) -> Result<(), ConfigEditErrorKind> {
        let options = EncodeOptions {
            context: request.context,
            budget: request.budget,
            limit: request.limit,
        };
        if request.map_key {
            append_user_escaped(output, request.name, options.budget, options.limit)?;
        } else {
            append_user_text(output, request.name, options.budget, options.limit)?;
        }
        append_text(output, ":", options.limit)?;
        consume(options.budget, request.colon_spacing.len())?;
        append_text(output, request.colon_spacing, options.limit)?;
        Self::encode_value_into(
            output,
            request.value,
            options.context,
            options.budget,
            options.limit,
        )
    }
}

fn start_value_container<'value>(
    output: &mut ChargedString,
    frames: &mut ChargedVec<EncodeFrame<'value>>,
    container: ContainerEncoding<'value>,
    open: &'static str,
    options: EncodeOptions<'_, '_>,
) -> Result<(), ConfigEditErrorKind> {
    append_text(output, open, options.limit)?;
    start_container(output, frames, container, options)
}

fn start_container<'value>(
    output: &mut ChargedString,
    frames: &mut ChargedVec<EncodeFrame<'value>>,
    container: ContainerEncoding<'value>,
    options: EncodeOptions<'_, '_>,
) -> Result<(), ConfigEditErrorKind> {
    if options.context.layout == TextLayout::Compact {
        push_frame(frames, EncodeFrame::Entries(container))?;
    } else if container.entries.len() == 0 {
        append_text(output, container.close, options.limit)?;
    } else {
        consume(options.budget, container.entries.len())?;
        append_text(output, options.context.line_break, options.limit)?;
        append_indent(output, container.depth.saturating_add(1), options)?;
        append_pretty_entry_prefix(output, container, 0, options)?;
        push_frame(frames, EncodeFrame::PrettyNext(container, 1))?;
        let Some(entry) = container.entries.get(0) else {
            return Err(ConfigEditErrorKind::EncodingFailed);
        };
        push_frame(
            frames,
            EncodeFrame::Value(entry.value(), container.depth.saturating_add(1)),
        )?;
    }
    Ok(())
}

fn append_pretty_entry_prefix(
    output: &mut ChargedString,
    container: ContainerEncoding<'_>,
    index: usize,
    options: EncodeOptions<'_, '_>,
) -> Result<(), ConfigEditErrorKind> {
    let Some(entry) = container.entries.get(index) else {
        return Err(ConfigEditErrorKind::EncodingFailed);
    };
    if let Some(name) = entry.name() {
        if container.map_keys {
            append_user_escaped(output, name, options.budget, options.limit)?;
        } else {
            append_user_text(output, name, options.budget, options.limit)?;
        }
        append_text(output, ": ", options.limit)?;
    }
    Ok(())
}

fn append_environment(
    output: &mut ChargedString,
    name: &str,
    fallback: Option<&ValueScalar<'_>>,
    options: EncodeOptions<'_, '_>,
) -> Result<(), ConfigEditErrorKind> {
    append_text(output, "env(", options.limit)?;
    append_user_escaped(output, name, options.budget, options.limit)?;
    if let Some(fallback) = fallback {
        append_text(output, ", ", options.limit)?;
        match fallback {
            ValueScalar::Signed(value) => {
                append_format(output, format_args!("{value}"), options.limit)?
            }
            ValueScalar::Unsigned(value) => {
                append_format(output, format_args!("{value}"), options.limit)?;
            }
            ValueScalar::Float32(value) => append_float(output, *value, options.limit)?,
            ValueScalar::Float64(value) => append_float(output, *value, options.limit)?,
            ValueScalar::String(value) => {
                append_user_escaped(output, value, options.budget, options.limit)?;
            }
            ValueScalar::Bool(value) => {
                append_text(output, if *value { "true" } else { "false" }, options.limit)?;
            }
        }
    }
    append_text(output, ")", options.limit)
}

fn push_compact_entries<'value>(
    frames: &mut ChargedVec<EncodeFrame<'value>>,
    container: ContainerEncoding<'value>,
    options: EncodeOptions<'_, '_>,
) -> Result<(), ConfigEditErrorKind> {
    consume(options.budget, container.entries.len())?;
    push_frame(frames, EncodeFrame::Text(container.close))?;
    for index in (0..container.entries.len()).rev() {
        let Some(entry) = container.entries.get(index) else {
            return Err(ConfigEditErrorKind::EncodingFailed);
        };
        push_frame(frames, EncodeFrame::Value(entry.value(), container.depth))?;
        if let Some(name) = entry.name() {
            push_frame(frames, EncodeFrame::Text(": "))?;
            push_frame(frames, EncodeFrame::FieldText(name, container.map_keys))?;
        }
        if index > 0 {
            push_frame(frames, EncodeFrame::Text(", "))?;
        }
    }
    Ok(())
}

fn append_indent(
    output: &mut ChargedString,
    depth: usize,
    options: EncodeOptions<'_, '_>,
) -> Result<(), ConfigEditErrorKind> {
    let base_indent_bytes = options
        .context
        .indent_unit
        .len()
        .checked_mul(options.context.indent_levels)
        .ok_or(ConfigEditErrorKind::ResourceLimitExceeded)?;
    let pretty_indent_bytes = depth
        .checked_mul(4)
        .ok_or(ConfigEditErrorKind::ResourceLimitExceeded)?;
    let length = options
        .context
        .indent
        .len()
        .checked_add(base_indent_bytes)
        .and_then(|length| length.checked_add(pretty_indent_bytes))
        .ok_or(ConfigEditErrorKind::ResourceLimitExceeded)?;
    consume(options.budget, length)?;
    append_text(output, options.context.indent, options.limit)?;
    for _ in 0..options.context.indent_levels {
        append_text(output, options.context.indent_unit, options.limit)?;
    }
    for _ in 0..depth {
        append_text(output, "    ", options.limit)?;
    }
    Ok(())
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

fn append_user_escaped(
    output: &mut ChargedString,
    text: &str,
    budget: Option<&ExecutionBudget>,
    limit: usize,
) -> Result<(), ConfigEditErrorKind> {
    consume(budget, text.len())?;
    append_escaped(output, text, limit)
}

fn append_user_text(
    output: &mut ChargedString,
    text: &str,
    budget: Option<&ExecutionBudget>,
    limit: usize,
) -> Result<(), ConfigEditErrorKind> {
    consume(budget, text.len())?;
    append_text(output, text, limit)
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

fn consume(budget: Option<&ExecutionBudget>, steps: usize) -> Result<(), ConfigEditErrorKind> {
    let Some(budget) = budget else {
        return Ok(());
    };
    let mut errors = ErrSlot::<BudgetError>::new();
    errors
        .try_call(|out| budget.consume(steps, out))
        .map_err(|caught| budget_error_kind(caught.take()))
}

fn budget_error_kind(error: BudgetError) -> ConfigEditErrorKind {
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
