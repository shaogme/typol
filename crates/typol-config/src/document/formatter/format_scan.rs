#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TokenKind {
    Open(u8),
    Close(u8),
    Colon,
    Comma,
    Atom,
    String,
    LineComment,
    BlockComment,
}

#[derive(Clone, Copy)]
pub(crate) struct ScannedToken {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) kind: TokenKind,
    pub(crate) newline_before: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ScanError {
    InvalidStructure,
    ResourceLimitExceeded,
}

pub(crate) struct SourceScanner;

impl SourceScanner {
    pub(crate) fn next(
        source: &str,
        cursor: &mut usize,
    ) -> Result<Option<ScannedToken>, ScanError> {
        let bytes = source.as_bytes();
        let mut newline_before = false;
        while let Some(byte) = bytes.get(*cursor).copied() {
            if !is_whitespace(byte) {
                break;
            }
            newline_before |= matches!(byte, b'\n' | b'\r');
            *cursor = cursor
                .checked_add(1)
                .ok_or(ScanError::ResourceLimitExceeded)?;
        }
        let Some(start) = Some(*cursor).filter(|start| *start < bytes.len()) else {
            return Ok(None);
        };
        let first = bytes
            .get(start)
            .copied()
            .ok_or(ScanError::InvalidStructure)?;
        let kind = match first {
            b'{' | b'[' | b'(' => TokenKind::Open(first),
            b'}' | b']' | b')' => TokenKind::Close(first),
            b':' => TokenKind::Colon,
            b',' => TokenKind::Comma,
            b'"' => {
                *cursor = scan_string(source, start)?;
                return Ok(Some(ScannedToken {
                    start,
                    end: *cursor,
                    kind: TokenKind::String,
                    newline_before,
                }));
            }
            b'/' if bytes.get(start.saturating_add(1)) == Some(&b'/') => {
                *cursor = scan_line_comment(bytes, start)?;
                return Ok(Some(ScannedToken {
                    start,
                    end: *cursor,
                    kind: TokenKind::LineComment,
                    newline_before,
                }));
            }
            b'/' if bytes.get(start.saturating_add(1)) == Some(&b'*') => {
                *cursor = scan_block_comment(bytes, start)?;
                return Ok(Some(ScannedToken {
                    start,
                    end: *cursor,
                    kind: TokenKind::BlockComment,
                    newline_before,
                }));
            }
            _ => TokenKind::Atom,
        };
        if matches!(kind, TokenKind::Atom) {
            *cursor = scan_atom(bytes, start)?;
        } else {
            *cursor = cursor
                .checked_add(1)
                .ok_or(ScanError::ResourceLimitExceeded)?;
        }
        Ok(Some(ScannedToken {
            start,
            end: *cursor,
            kind,
            newline_before,
        }))
    }

    pub(crate) const fn closes(token: ScannedToken, delimiter: u8) -> bool {
        matches!(token.kind, TokenKind::Close(found) if found == delimiter)
    }

    pub(super) const fn is_container(opener: u8) -> bool {
        matches!(opener, b'{' | b'[')
    }

    pub(super) const fn matching_close(opener: u8) -> Option<u8> {
        match opener {
            b'{' => Some(b'}'),
            b'[' => Some(b']'),
            b'(' => Some(b')'),
            _ => None,
        }
    }
}

fn is_whitespace(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r')
}

fn is_delimiter(byte: u8) -> bool {
    matches!(byte, b'{' | b'}' | b'[' | b']' | b'(' | b')' | b':' | b',')
}

fn scan_atom(bytes: &[u8], start: usize) -> Result<usize, ScanError> {
    let mut cursor = start;
    while let Some(byte) = bytes.get(cursor).copied() {
        let starts_comment =
            byte == b'/' && matches!(bytes.get(cursor.saturating_add(1)), Some(b'/' | b'*'));
        if is_whitespace(byte) || is_delimiter(byte) || byte == b'"' || starts_comment {
            break;
        }
        cursor = cursor
            .checked_add(1)
            .ok_or(ScanError::ResourceLimitExceeded)?;
    }
    if cursor == start {
        return Err(ScanError::InvalidStructure);
    }
    Ok(cursor)
}

fn scan_string(source: &str, start: usize) -> Result<usize, ScanError> {
    let bytes = source.as_bytes();
    if bytes.get(start..start.saturating_add(3)) == Some(b"\"\"\"") {
        let mut cursor = start
            .checked_add(3)
            .ok_or(ScanError::ResourceLimitExceeded)?;
        match bytes.get(cursor) {
            Some(b'\n') => cursor += 1,
            Some(b'\r') if bytes.get(cursor.saturating_add(1)) == Some(&b'\n') => cursor += 2,
            _ => return Err(ScanError::InvalidStructure),
        }
        while cursor < bytes.len() {
            let mut content = cursor;
            while matches!(bytes.get(content), Some(b' ' | b'\t')) {
                content = content
                    .checked_add(1)
                    .ok_or(ScanError::ResourceLimitExceeded)?;
            }
            if bytes.get(content..content.saturating_add(3)) == Some(b"\"\"\"") {
                return content
                    .checked_add(3)
                    .ok_or(ScanError::ResourceLimitExceeded);
            }
            while bytes.get(cursor).is_some_and(|byte| *byte != b'\n') {
                cursor = cursor
                    .checked_add(1)
                    .ok_or(ScanError::ResourceLimitExceeded)?;
            }
            if cursor < bytes.len() {
                cursor = cursor
                    .checked_add(1)
                    .ok_or(ScanError::ResourceLimitExceeded)?;
            }
        }
        return Err(ScanError::InvalidStructure);
    }
    let mut cursor = start
        .checked_add(1)
        .ok_or(ScanError::ResourceLimitExceeded)?;
    while let Some(byte) = bytes.get(cursor).copied() {
        if byte == b'\\' {
            cursor = cursor
                .checked_add(2)
                .ok_or(ScanError::ResourceLimitExceeded)?;
        } else if byte == b'"' {
            return cursor
                .checked_add(1)
                .ok_or(ScanError::ResourceLimitExceeded);
        } else {
            cursor = cursor
                .checked_add(1)
                .ok_or(ScanError::ResourceLimitExceeded)?;
        }
    }
    Err(ScanError::InvalidStructure)
}

fn scan_line_comment(bytes: &[u8], start: usize) -> Result<usize, ScanError> {
    let mut cursor = start
        .checked_add(2)
        .ok_or(ScanError::ResourceLimitExceeded)?;
    while let Some(byte) = bytes.get(cursor).copied() {
        if matches!(byte, b'\n' | b'\r') {
            return Ok(cursor);
        }
        cursor = cursor
            .checked_add(1)
            .ok_or(ScanError::ResourceLimitExceeded)?;
    }
    Ok(cursor)
}

fn scan_block_comment(bytes: &[u8], start: usize) -> Result<usize, ScanError> {
    let mut cursor = start
        .checked_add(2)
        .ok_or(ScanError::ResourceLimitExceeded)?;
    while cursor < bytes.len() {
        if bytes.get(cursor) == Some(&b'*') && bytes.get(cursor.saturating_add(1)) == Some(&b'/') {
            return cursor
                .checked_add(2)
                .ok_or(ScanError::ResourceLimitExceeded);
        }
        cursor = cursor
            .checked_add(1)
            .ok_or(ScanError::ResourceLimitExceeded)?;
    }
    Err(ScanError::InvalidStructure)
}
