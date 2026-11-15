use crate::ConfigEditErrorKind;
use typol_core::CapacityExceeded;

pub(crate) struct LexicalSpan {
    pub(crate) end: usize,
    pub(crate) comment: bool,
    pub(crate) block_string: bool,
}

pub(crate) struct SyntaxScanner;

impl SyntaxScanner {
    pub(crate) fn lexical_span(
        source: &str,
        start: usize,
    ) -> Result<Option<LexicalSpan>, ConfigEditErrorKind> {
        let bytes = source.as_bytes();
        match bytes.get(start).copied() {
            Some(b'"') => Self::skip_string(source, start).map(|end| {
                Some(LexicalSpan {
                    end,
                    comment: false,
                    block_string: bytes.get(start..start.saturating_add(3)) == Some(b"\"\"\""),
                })
            }),
            Some(b'/') if bytes.get(start.saturating_add(1)) == Some(&b'/') => {
                Self::skip_line_comment(source, start).map(|end| {
                    Some(LexicalSpan {
                        end,
                        comment: true,
                        block_string: false,
                    })
                })
            }
            Some(b'/') if bytes.get(start.saturating_add(1)) == Some(&b'*') => {
                Self::skip_block_comment(source, start).map(|end| {
                    Some(LexicalSpan {
                        end,
                        comment: true,
                        block_string: false,
                    })
                })
            }
            _ => Ok(None),
        }
    }

    fn skip_string(source: &str, start: usize) -> Result<usize, ConfigEditErrorKind> {
        let bytes = source.as_bytes();
        if bytes.get(start..start.saturating_add(3)) == Some(b"\"\"\"") {
            let mut cursor = start
                .checked_add(3)
                .ok_or(ConfigEditErrorKind::CapacityExceeded(
                    CapacityExceeded::ConfigEdit,
                ))?;
            match bytes.get(cursor) {
                Some(b'\n') => cursor += 1,
                Some(b'\r') if bytes.get(cursor.saturating_add(1)) == Some(&b'\n') => cursor += 2,
                _ => return Err(ConfigEditErrorKind::EditConflict),
            }
            while cursor < bytes.len() {
                let mut content = cursor;
                while matches!(bytes.get(content), Some(b' ' | b'\t')) {
                    content =
                        content
                            .checked_add(1)
                            .ok_or(ConfigEditErrorKind::CapacityExceeded(
                                CapacityExceeded::ConfigEdit,
                            ))?;
                }
                if bytes.get(content..content.saturating_add(3)) == Some(b"\"\"\"") {
                    return content
                        .checked_add(3)
                        .ok_or(ConfigEditErrorKind::CapacityExceeded(
                            CapacityExceeded::ConfigEdit,
                        ));
                }
                while bytes.get(cursor).is_some_and(|byte| *byte != b'\n') {
                    cursor = cursor
                        .checked_add(1)
                        .ok_or(ConfigEditErrorKind::CapacityExceeded(
                            CapacityExceeded::ConfigEdit,
                        ))?;
                }
                if cursor < bytes.len() {
                    cursor = cursor
                        .checked_add(1)
                        .ok_or(ConfigEditErrorKind::CapacityExceeded(
                            CapacityExceeded::ConfigEdit,
                        ))?;
                }
            }
            return Err(ConfigEditErrorKind::EditConflict);
        }
        let mut cursor = start
            .checked_add(1)
            .ok_or(ConfigEditErrorKind::CapacityExceeded(
                CapacityExceeded::ConfigEdit,
            ))?;
        while let Some(byte) = bytes.get(cursor).copied() {
            if byte == b'\\' {
                cursor = cursor
                    .checked_add(2)
                    .ok_or(ConfigEditErrorKind::CapacityExceeded(
                        CapacityExceeded::ConfigEdit,
                    ))?;
            } else if byte == b'"' {
                return cursor
                    .checked_add(1)
                    .ok_or(ConfigEditErrorKind::CapacityExceeded(
                        CapacityExceeded::ConfigEdit,
                    ));
            } else {
                cursor = cursor
                    .checked_add(1)
                    .ok_or(ConfigEditErrorKind::CapacityExceeded(
                        CapacityExceeded::ConfigEdit,
                    ))?;
            }
        }
        Err(ConfigEditErrorKind::EditConflict)
    }

    fn skip_line_comment(source: &str, start: usize) -> Result<usize, ConfigEditErrorKind> {
        let bytes = source.as_bytes();
        let mut cursor = start
            .checked_add(2)
            .ok_or(ConfigEditErrorKind::CapacityExceeded(
                CapacityExceeded::ConfigEdit,
            ))?;
        while let Some(byte) = bytes.get(cursor).copied() {
            cursor = cursor
                .checked_add(1)
                .ok_or(ConfigEditErrorKind::CapacityExceeded(
                    CapacityExceeded::ConfigEdit,
                ))?;
            if byte == b'\n' {
                return Ok(cursor);
            }
            if byte == b'\r' {
                if bytes.get(cursor) == Some(&b'\n') {
                    cursor = cursor
                        .checked_add(1)
                        .ok_or(ConfigEditErrorKind::CapacityExceeded(
                            CapacityExceeded::ConfigEdit,
                        ))?;
                }
                return Ok(cursor);
            }
        }
        Ok(cursor)
    }

    fn skip_block_comment(source: &str, start: usize) -> Result<usize, ConfigEditErrorKind> {
        let bytes = source.as_bytes();
        let mut cursor = start
            .checked_add(2)
            .ok_or(ConfigEditErrorKind::CapacityExceeded(
                CapacityExceeded::ConfigEdit,
            ))?;
        while cursor < bytes.len() {
            if bytes.get(cursor) == Some(&b'*')
                && bytes.get(cursor.saturating_add(1)) == Some(&b'/')
            {
                return cursor
                    .checked_add(2)
                    .ok_or(ConfigEditErrorKind::CapacityExceeded(
                        CapacityExceeded::ConfigEdit,
                    ));
            }
            cursor = cursor
                .checked_add(1)
                .ok_or(ConfigEditErrorKind::CapacityExceeded(
                    CapacityExceeded::ConfigEdit,
                ))?;
        }
        Err(ConfigEditErrorKind::EditConflict)
    }
}
