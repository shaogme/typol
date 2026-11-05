mod format_scan;

use core::{error::Error, fmt, mem::size_of};
use opeo::{OResult, Out};
use typol_core::{BudgetError, ChargedString, ChargedVec, ExecutionBudget, Write, WriteError};

use self::format_scan::{ScanError, ScannedToken, SourceScanner, TokenKind};

const DEFAULT_MAX_OUTPUT_BYTES: usize = 16_777_216;
const DEFAULT_MAX_WORKSPACE_BYTES: usize = 16_777_216;
const DEFAULT_MAX_NESTING_DEPTH: usize = 128;

/// Stable resource and structure categories for configuration formatting failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigFormatErrorKind {
    ResourceLimitExceeded,
    AllocationFailed,
    InvalidStructure,
}

/// A formatting failure that never exposes configuration source text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConfigFormatError {
    kind: ConfigFormatErrorKind,
}

/// Limits for formatter-owned output and explicit nesting workspaces.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConfigFormatOptions {
    max_output_bytes: usize,
    max_workspace_bytes: usize,
    max_nesting_depth: usize,
}

/// A fully formatted configuration string owned independently from its source document.
pub struct FormattedConfig {
    source: ChargedString,
}

pub(crate) struct FormatRequest<'source, 'budget> {
    pub(crate) source: &'source str,
    pub(crate) budget: Option<&'budget ExecutionBudget>,
    pub(crate) source_limit: usize,
    pub(crate) constructed_text_limit: usize,
    pub(crate) options: ConfigFormatOptions,
}

#[derive(Clone, Copy)]
struct FormatFrame {
    opener: u8,
    content: bool,
    item: bool,
    after_comma: bool,
}

pub(crate) struct FormatterEngine<'source, 'budget> {
    source: &'source str,
    budget: Option<&'budget ExecutionBudget>,
    newline: &'static str,
    output: ChargedString,
    stack: ChargedVec<FormatFrame>,
    options: ConfigFormatOptions,
    output_limit: usize,
    previous: Option<TokenKind>,
    previous_comment_standalone: bool,
}

impl ConfigFormatError {
    const fn new(kind: ConfigFormatErrorKind) -> Self {
        Self { kind }
    }

    pub const fn kind(self) -> ConfigFormatErrorKind {
        self.kind
    }
}

impl From<BudgetError> for ConfigFormatError {
    fn from(error: BudgetError) -> Self {
        Self::new(match error {
            BudgetError::Resource | BudgetError::CapacityExceeded => {
                ConfigFormatErrorKind::ResourceLimitExceeded
            }
            BudgetError::Allocation => ConfigFormatErrorKind::AllocationFailed,
            BudgetError::InvariantViolation(_) => ConfigFormatErrorKind::InvalidStructure,
        })
    }
}

impl fmt::Display for ConfigFormatError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.kind {
            ConfigFormatErrorKind::ResourceLimitExceeded => {
                "configuration formatting resource limit exceeded"
            }
            ConfigFormatErrorKind::AllocationFailed => "configuration formatting allocation failed",
            ConfigFormatErrorKind::InvalidStructure => {
                "configuration formatting encountered invalid structure"
            }
        })
    }
}

impl Error for ConfigFormatError {}

impl ConfigFormatOptions {
    pub const fn new() -> Self {
        Self {
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
            max_workspace_bytes: DEFAULT_MAX_WORKSPACE_BYTES,
            max_nesting_depth: DEFAULT_MAX_NESTING_DEPTH,
        }
    }

    pub const fn with_max_output_bytes(mut self, max_output_bytes: usize) -> Self {
        self.max_output_bytes = max_output_bytes;
        self
    }

    pub const fn with_max_workspace_bytes(mut self, max_workspace_bytes: usize) -> Self {
        self.max_workspace_bytes = max_workspace_bytes;
        self
    }

    pub const fn with_max_nesting_depth(mut self, max_nesting_depth: usize) -> Self {
        self.max_nesting_depth = max_nesting_depth;
        self
    }

    pub const fn max_output_bytes(&self) -> usize {
        self.max_output_bytes
    }

    pub const fn max_workspace_bytes(&self) -> usize {
        self.max_workspace_bytes
    }

    pub const fn max_nesting_depth(&self) -> usize {
        self.max_nesting_depth
    }
}

impl Default for ConfigFormatOptions {
    fn default() -> Self {
        Self::new()
    }
}

impl FormattedConfig {
    /// Borrows the complete formatted text.
    pub fn as_str(&self) -> &str {
        self.source.as_str()
    }

    /// Consumes the formatted result after writing all of it to the supplied sink.
    pub fn write_to(self, output: &mut dyn Write) -> Result<(), WriteError> {
        output.write_all(self.source.as_str().as_bytes())
    }
}

impl FormatterEngine<'_, '_> {
    pub(crate) fn format_source<'slot>(
        request: FormatRequest<'_, '_>,
        mut out: Out<'slot, '_, ConfigFormatError>,
    ) -> OResult<'slot, FormattedConfig> {
        if request.source.len() > request.source_limit {
            return Err(out.fail(ConfigFormatError::new(
                ConfigFormatErrorKind::ResourceLimitExceeded,
            )));
        }
        if let Some(budget) = request.budget {
            budget.consume(request.source.len(), out.reborrow())?;
        }
        let output_limit = request
            .options
            .max_output_bytes
            .min(request.source_limit)
            .min(request.constructed_text_limit);
        let newline = if request.source.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        };
        let output = ChargedString::with_capacity(0, request.budget, out.reborrow())?;
        let stack = ChargedVec::new(request.budget, out.reborrow())?;
        let mut engine = FormatterEngine {
            source: request.source,
            budget: request.budget,
            newline,
            output,
            stack,
            options: request.options,
            output_limit,
            previous: None,
            previous_comment_standalone: false,
        };
        engine.run(out.reborrow())?;
        if !engine.output.as_str().ends_with('\n') {
            engine.append_text(newline, out.reborrow())?;
        }
        if engine.output.len() > request.constructed_text_limit {
            return Err(out.fail(ConfigFormatError::new(
                ConfigFormatErrorKind::ResourceLimitExceeded,
            )));
        }
        if let Some(budget) = request.budget {
            budget.construct_text(
                engine.output.len(),
                request.constructed_text_limit,
                out.reborrow(),
            )?;
        }
        Ok(FormattedConfig {
            source: engine.output,
        })
    }

    fn run<'slot>(&mut self, mut out: Out<'slot, '_, ConfigFormatError>) -> OResult<'slot, ()> {
        let mut cursor = 0usize;
        while let Some(token) = self.next_scanned_token(&mut cursor, out.reborrow())? {
            self.process(token, out.reborrow())?;
        }
        if !self.stack.is_empty() {
            return Err(out.fail(ConfigFormatError::new(
                ConfigFormatErrorKind::InvalidStructure,
            )));
        }
        Ok(())
    }

    fn next_scanned_token<'slot>(
        &self,
        cursor: &mut usize,
        mut out: Out<'slot, '_, ConfigFormatError>,
    ) -> OResult<'slot, Option<ScannedToken>> {
        let previous_cursor = *cursor;
        let result = SourceScanner::next(self.source, cursor);
        let Some(scanned_bytes) = cursor.checked_sub(previous_cursor) else {
            return Err(out.fail(ConfigFormatError::new(
                ConfigFormatErrorKind::InvalidStructure,
            )));
        };
        if let Some(budget) = self.budget {
            budget.consume(scanned_bytes, out.reborrow())?;
        }
        match result {
            Ok(token) => Ok(token),
            Err(error) => Err(out.fail(Self::scan_error(error))),
        }
    }

    fn process<'slot>(
        &mut self,
        token: ScannedToken,
        mut out: Out<'slot, '_, ConfigFormatError>,
    ) -> OResult<'slot, ()> {
        match token.kind {
            TokenKind::LineComment | TokenKind::BlockComment => {
                self.write_comment(token, out.reborrow())
            }
            TokenKind::Open(opener) => self.open(opener, token, out.reborrow()),
            TokenKind::Close(closer) => self.close(closer, token, out.reborrow()),
            TokenKind::Comma => self.comma(token, out.reborrow()),
            TokenKind::Colon => {
                self.mark_item();
                self.prepare_token(token, out.reborrow())?;
                self.append_source(token.start, token.end, out.reborrow())?;
                self.previous = Some(TokenKind::Colon);
                self.previous_comment_standalone = false;
                Ok(())
            }
            TokenKind::Atom | TokenKind::String => {
                self.mark_item();
                self.prepare_token(token, out.reborrow())?;
                self.append_source(token.start, token.end, out.reborrow())?;
                self.previous = Some(token.kind);
                self.previous_comment_standalone = false;
                Ok(())
            }
        }
    }

    fn open<'slot>(
        &mut self,
        opener: u8,
        token: ScannedToken,
        mut out: Out<'slot, '_, ConfigFormatError>,
    ) -> OResult<'slot, ()> {
        self.mark_item();
        self.prepare_token(token, out.reborrow())?;
        self.append_source(token.start, token.end, out.reborrow())?;
        let content = !self.container_is_empty(token.end, opener, out.reborrow())?;
        self.push_frame(opener, content, out.reborrow())?;
        if content && is_container(opener) {
            self.append_newline(out.reborrow())?;
        }
        self.previous = Some(token.kind);
        self.previous_comment_standalone = false;
        Ok(())
    }

    fn close<'slot>(
        &mut self,
        closer: u8,
        token: ScannedToken,
        mut out: Out<'slot, '_, ConfigFormatError>,
    ) -> OResult<'slot, ()> {
        let Some(frame) = self.stack.as_slice().last().copied() else {
            return Err(out.fail(ConfigFormatError::new(
                ConfigFormatErrorKind::InvalidStructure,
            )));
        };
        if matching_close(frame.opener) != Some(closer) {
            return Err(out.fail(ConfigFormatError::new(
                ConfigFormatErrorKind::InvalidStructure,
            )));
        }
        if is_container(frame.opener) {
            if frame.item && !frame.after_comma {
                self.prepare_comma(out.reborrow())?;
                self.append_text(",", out.reborrow())?;
            }
            if frame.content {
                self.append_newline(out.reborrow())?;
                let depth = self.stack.len().checked_sub(1).ok_or_else(|| {
                    out.reborrow().fail(ConfigFormatError::new(
                        ConfigFormatErrorKind::InvalidStructure,
                    ))
                })?;
                self.append_indent(depth, out.reborrow())?;
            }
        } else {
            self.prepare_closing_paren(out.reborrow())?;
        }
        self.append_source(token.start, token.end, out.reborrow())?;
        self.stack.pop().ok_or_else(|| {
            out.reborrow().fail(ConfigFormatError::new(
                ConfigFormatErrorKind::InvalidStructure,
            ))
        })?;
        self.previous = Some(token.kind);
        self.previous_comment_standalone = false;
        Ok(())
    }

    fn comma<'slot>(
        &mut self,
        token: ScannedToken,
        mut out: Out<'slot, '_, ConfigFormatError>,
    ) -> OResult<'slot, ()> {
        let Some(frame) = self.stack.as_slice().last().copied() else {
            return Err(out.fail(ConfigFormatError::new(
                ConfigFormatErrorKind::InvalidStructure,
            )));
        };
        if is_container(frame.opener) {
            if !frame.item || frame.after_comma {
                return Err(out.fail(ConfigFormatError::new(
                    ConfigFormatErrorKind::InvalidStructure,
                )));
            }
            self.prepare_comma(out.reborrow())?;
            self.append_source(token.start, token.end, out.reborrow())?;
            if let Some(frame) = self.stack.as_mut_slice().last_mut() {
                frame.item = false;
                frame.after_comma = true;
                frame.content = true;
            }
            if self.has_same_line_comment(token.end, out.reborrow())? {
                self.append_text(" ", out.reborrow())?;
            } else {
                self.append_newline(out.reborrow())?;
            }
        } else {
            self.prepare_comma(out.reborrow())?;
            self.append_source(token.start, token.end, out.reborrow())?;
            self.append_text(" ", out.reborrow())?;
        }
        self.previous = Some(TokenKind::Comma);
        self.previous_comment_standalone = false;
        Ok(())
    }

    fn write_comment<'slot>(
        &mut self,
        token: ScannedToken,
        mut out: Out<'slot, '_, ConfigFormatError>,
    ) -> OResult<'slot, ()> {
        self.insert_comma_before_closing_comment(token.end, out.reborrow())?;
        let opening_boundary = matches!(
            self.previous,
            Some(TokenKind::Open(b'{') | TokenKind::Open(b'['))
        );
        let standalone = token.newline_before
            || opening_boundary
            || self.previous.is_none()
            || self.previous_comment_standalone;
        if standalone {
            if !self.output.is_empty() {
                self.append_newline(out.reborrow())?;
            }
            self.append_indent(self.stack.len(), out.reborrow())?;
        } else {
            self.append_space(out.reborrow())?;
        }
        self.append_source(token.start, token.end, out.reborrow())?;
        let same_line_after_comma = matches!(
            (self.previous, self.stack.as_slice().last()),
            (Some(TokenKind::Comma), Some(frame)) if is_container(frame.opener)
        ) && !token.newline_before;
        let before_container_close = if token.kind == TokenKind::BlockComment && !standalone {
            let mut cursor = token.end;
            matches!(
                self.next_scanned_token(&mut cursor, out.reborrow())?,
                Some(ScannedToken {
                    kind: TokenKind::Close(b'}' | b']'),
                    ..
                })
            )
        } else {
            false
        };
        if token.kind == TokenKind::LineComment || standalone || same_line_after_comma {
            self.append_newline(out.reborrow())?;
        } else if !before_container_close {
            self.append_text(" ", out.reborrow())?;
        }
        if let Some(frame) = self.stack.as_mut_slice().last_mut() {
            if is_container(frame.opener) {
                frame.content = true;
            }
        }
        self.previous = Some(token.kind);
        self.previous_comment_standalone =
            standalone || token.kind == TokenKind::LineComment || same_line_after_comma;
        Ok(())
    }

    fn has_same_line_comment<'slot>(
        &self,
        start: usize,
        mut out: Out<'slot, '_, ConfigFormatError>,
    ) -> OResult<'slot, bool> {
        let mut cursor = start;
        let Some(token) = self.next_scanned_token(&mut cursor, out.reborrow())? else {
            return Ok(false);
        };
        Ok(!token.newline_before
            && matches!(token.kind, TokenKind::LineComment | TokenKind::BlockComment))
    }

    fn insert_comma_before_closing_comment<'slot>(
        &mut self,
        start: usize,
        mut out: Out<'slot, '_, ConfigFormatError>,
    ) -> OResult<'slot, ()> {
        let Some(frame) = self.stack.as_slice().last().copied() else {
            return Ok(());
        };
        if !is_container(frame.opener) || !frame.item || frame.after_comma {
            return Ok(());
        }
        let Some(closer) = matching_close(frame.opener) else {
            return Err(out.fail(ConfigFormatError::new(
                ConfigFormatErrorKind::InvalidStructure,
            )));
        };
        let mut cursor = start;
        loop {
            let Some(token) = self.next_scanned_token(&mut cursor, out.reborrow())? else {
                return Ok(());
            };
            match token.kind {
                TokenKind::LineComment | TokenKind::BlockComment => continue,
                TokenKind::Close(found) if found == closer => {
                    self.prepare_comma(out.reborrow())?;
                    self.append_text(",", out.reborrow())?;
                    if let Some(frame) = self.stack.as_mut_slice().last_mut() {
                        frame.item = false;
                        frame.after_comma = true;
                    }
                    return Ok(());
                }
                _ => return Ok(()),
            }
        }
    }

    fn prepare_token<'slot>(
        &mut self,
        token: ScannedToken,
        mut out: Out<'slot, '_, ConfigFormatError>,
    ) -> OResult<'slot, ()> {
        if self.output.as_str().ends_with('\n') {
            self.append_indent(self.stack.len(), out.reborrow())?;
        }
        let needs_space = matches!(self.previous, Some(TokenKind::Colon))
            || matches!(self.previous, Some(TokenKind::Comma))
                && matches!(self.stack.as_slice().last(), Some(frame) if frame.opener == b'(')
            || matches!(self.previous, Some(TokenKind::BlockComment))
                && !self.previous_comment_standalone
            || matches!(self.previous, Some(TokenKind::Atom | TokenKind::String))
                && matches!(token.kind, TokenKind::Atom | TokenKind::String)
            || matches!(self.previous, Some(TokenKind::Atom))
                && matches!(token.kind, TokenKind::Open(b'{'))
            || matches!(self.previous, Some(TokenKind::Close(b'}' | b']' | b')')))
                && matches!(token.kind, TokenKind::Atom | TokenKind::String);
        if needs_space {
            self.append_space(out.reborrow())?;
        }
        Ok(())
    }

    fn prepare_comma<'slot>(
        &mut self,
        mut out: Out<'slot, '_, ConfigFormatError>,
    ) -> OResult<'slot, ()> {
        if self.output.as_str().ends_with('\n') {
            self.append_indent(self.stack.len(), out.reborrow())?;
        }
        if matches!(self.previous, Some(TokenKind::BlockComment))
            && !self.output.as_str().ends_with(' ')
        {
            self.append_space(out.reborrow())?;
        }
        Ok(())
    }

    fn prepare_closing_paren<'slot>(
        &mut self,
        mut out: Out<'slot, '_, ConfigFormatError>,
    ) -> OResult<'slot, ()> {
        if self.output.as_str().ends_with('\n') {
            let depth = self.stack.len().checked_sub(1).ok_or_else(|| {
                out.reborrow().fail(ConfigFormatError::new(
                    ConfigFormatErrorKind::InvalidStructure,
                ))
            })?;
            self.append_indent(depth, out.reborrow())?;
        } else if matches!(self.previous, Some(TokenKind::BlockComment))
            && !self.previous_comment_standalone
        {
            self.append_space(out.reborrow())?;
        }
        Ok(())
    }

    fn mark_item(&mut self) {
        if let Some(frame) = self.stack.as_mut_slice().last_mut() {
            frame.content = true;
            frame.item = true;
            frame.after_comma = false;
        }
    }

    fn push_frame<'slot>(
        &mut self,
        opener: u8,
        content: bool,
        mut out: Out<'slot, '_, ConfigFormatError>,
    ) -> OResult<'slot, ()> {
        let depth = self.stack.len().checked_add(1).ok_or_else(|| {
            out.reborrow().fail(ConfigFormatError::new(
                ConfigFormatErrorKind::ResourceLimitExceeded,
            ))
        })?;
        let workspace_bytes = depth.checked_mul(size_of::<FormatFrame>()).ok_or_else(|| {
            out.reborrow().fail(ConfigFormatError::new(
                ConfigFormatErrorKind::ResourceLimitExceeded,
            ))
        })?;
        if depth > self.options.max_nesting_depth
            || workspace_bytes > self.options.max_workspace_bytes
        {
            return Err(out.fail(ConfigFormatError::new(
                ConfigFormatErrorKind::ResourceLimitExceeded,
            )));
        }
        self.stack.try_push(
            FormatFrame {
                opener,
                content,
                item: false,
                after_comma: false,
            },
            out,
        )
    }

    fn container_is_empty<'slot>(
        &self,
        start: usize,
        opener: u8,
        mut out: Out<'slot, '_, ConfigFormatError>,
    ) -> OResult<'slot, bool> {
        if !is_container(opener) {
            return Ok(true);
        }
        let bytes = self.source.as_bytes();
        let mut cursor = start;
        let mut scanned_bytes = 0usize;
        while let Some(byte) = bytes.get(cursor).copied() {
            scanned_bytes = scanned_bytes.checked_add(1).ok_or_else(|| {
                out.reborrow().fail(ConfigFormatError::new(
                    ConfigFormatErrorKind::ResourceLimitExceeded,
                ))
            })?;
            if !matches!(byte, b' ' | b'\t' | b'\r' | b'\n') {
                if let Some(budget) = self.budget {
                    budget.consume(scanned_bytes, out.reborrow())?;
                }
                return Ok(byte == matching_close(opener).unwrap_or_default());
            }
            cursor = cursor.checked_add(1).ok_or_else(|| {
                out.reborrow().fail(ConfigFormatError::new(
                    ConfigFormatErrorKind::ResourceLimitExceeded,
                ))
            })?;
        }
        if let Some(budget) = self.budget {
            budget.consume(scanned_bytes, out.reborrow())?;
        }
        Err(out.fail(ConfigFormatError::new(
            ConfigFormatErrorKind::InvalidStructure,
        )))
    }

    fn append_source<'slot>(
        &mut self,
        start: usize,
        end: usize,
        out: Out<'slot, '_, ConfigFormatError>,
    ) -> OResult<'slot, ()> {
        let Some(text) = self.source.get(start..end) else {
            return Err(out.fail(ConfigFormatError::new(
                ConfigFormatErrorKind::InvalidStructure,
            )));
        };
        self.append_text(text, out)
    }

    fn append_text<'slot>(
        &mut self,
        text: &str,
        mut out: Out<'slot, '_, ConfigFormatError>,
    ) -> OResult<'slot, ()> {
        let Some(length) = self.output.len().checked_add(text.len()) else {
            return Err(out.fail(ConfigFormatError::new(
                ConfigFormatErrorKind::ResourceLimitExceeded,
            )));
        };
        if length > self.output_limit {
            return Err(out.fail(ConfigFormatError::new(
                ConfigFormatErrorKind::ResourceLimitExceeded,
            )));
        }
        self.output.try_push_str(text, out.reborrow())
    }

    fn append_space<'slot>(
        &mut self,
        out: Out<'slot, '_, ConfigFormatError>,
    ) -> OResult<'slot, ()> {
        if self.output.as_str().ends_with([' ', '\t', '\n', '\r']) {
            return Ok(());
        }
        self.append_text(" ", out)
    }

    fn append_newline<'slot>(
        &mut self,
        out: Out<'slot, '_, ConfigFormatError>,
    ) -> OResult<'slot, ()> {
        if self.output.as_str().ends_with('\n') {
            return Ok(());
        }
        self.append_text(self.newline, out)
    }

    fn append_indent<'slot>(
        &mut self,
        depth: usize,
        mut out: Out<'slot, '_, ConfigFormatError>,
    ) -> OResult<'slot, ()> {
        if !self.output.as_str().ends_with('\n') {
            return Ok(());
        }
        let spaces = depth.checked_mul(4).ok_or_else(|| {
            out.reborrow().fail(ConfigFormatError::new(
                ConfigFormatErrorKind::ResourceLimitExceeded,
            ))
        })?;
        if spaces > self.output_limit.saturating_sub(self.output.len()) {
            return Err(out.fail(ConfigFormatError::new(
                ConfigFormatErrorKind::ResourceLimitExceeded,
            )));
        }
        for _ in 0..depth {
            self.append_text("    ", out.reborrow())?;
        }
        Ok(())
    }

    fn scan_error(error: ScanError) -> ConfigFormatError {
        ConfigFormatError::new(match error {
            ScanError::InvalidStructure => ConfigFormatErrorKind::InvalidStructure,
            ScanError::ResourceLimitExceeded => ConfigFormatErrorKind::ResourceLimitExceeded,
        })
    }
}

fn is_container(opener: u8) -> bool {
    matches!(opener, b'{' | b'[')
}

fn matching_close(opener: u8) -> Option<u8> {
    match opener {
        b'{' => Some(b'}'),
        b'[' => Some(b']'),
        b'(' => Some(b')'),
        _ => None,
    }
}
