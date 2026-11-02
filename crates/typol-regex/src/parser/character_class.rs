use crate::{
    PatternSpan, RegexCompileError, RegexSyntaxErrorKind, char_class::CharClass, parser::Parser,
};
use opeo::{OResult, Out};

pub(super) struct CharacterClassParser;

impl CharacterClassParser {
    pub(super) fn is_regex_metachar(ch: char) -> bool {
        matches!(
            ch,
            '.' | '^' | '$' | '|' | '(' | ')' | '[' | ']' | '{' | '}' | '*' | '+' | '?' | '\\'
        )
    }

    pub(super) fn is_quantifier_start(ch: Option<char>) -> bool {
        matches!(ch, Some('*' | '+' | '?' | '{'))
    }

    pub(super) fn parse<'slot>(
        parser: &mut Parser<'_>,
        mut out: Out<'slot, '_, RegexCompileError>,
    ) -> OResult<'slot, (CharClass, PatternSpan)> {
        let start = parser.position();
        parser.advance_char(out.reborrow())?;
        let negated = if parser.current_char(out.reborrow())? == Some('^') {
            parser.advance_char(out.reborrow())?;
            true
        } else {
            false
        };

        if parser.current_char(out.reborrow())? == Some(']') {
            parser.advance_char(out.reborrow())?;
            return Err(out.fail(parser.syntax_error(
                RegexSyntaxErrorKind::EmptyCharacterClass,
                start,
                parser.position(),
            )));
        }

        let mut builder = parser.new_class_builder(out.reborrow())?;
        let mut has_item = false;
        loop {
            match parser.current_char(out.reborrow())? {
                None => {
                    return Err(out.fail(parser.syntax_error(
                        RegexSyntaxErrorKind::UnclosedCharacterClass,
                        parser.pattern().len(),
                        parser.pattern().len(),
                    )));
                }
                Some(']') => {
                    parser.advance_char(out.reborrow())?;
                    if !has_item {
                        return Err(out.fail(parser.syntax_error(
                            RegexSyntaxErrorKind::EmptyCharacterClass,
                            start,
                            parser.position(),
                        )));
                    }
                    let class = parser.finish_class(builder, negated, out.reborrow())?;
                    let span = PatternSpan::new(start, parser.position());
                    break Ok((class, span));
                }
                Some('\\') => {
                    let slash_start = parser.position();
                    parser.advance_char(out.reborrow())?;
                    let escaped = parser.parse_escape_for_class(slash_start, out.reborrow())?;
                    if let Some(ch) = escaped.literal() {
                        builder.add_scalar(ch, out.reborrow())?;
                    } else if let Some(class) = escaped.class() {
                        builder.add_class(class, out.reborrow())?;
                    }
                    has_item = true;
                    if parser.current_char(out.reborrow())? == Some('-')
                        && parser.peek_after_current(out.reborrow())? != Some(']')
                    {
                        return Err(out.fail(parser.syntax_error(
                            RegexSyntaxErrorKind::InvalidCharacterRange,
                            parser.position(),
                            parser.position() + 1,
                        )));
                    }
                }
                Some('-')
                    if !has_item || parser.peek_after_current(out.reborrow())? == Some(']') =>
                {
                    parser.advance_char(out.reborrow())?;
                    builder.add_scalar('-', out.reborrow())?;
                    has_item = true;
                }
                Some('-') => {
                    let end = parser.position() + 1;
                    return Err(out.fail(parser.syntax_error(
                        RegexSyntaxErrorKind::InvalidCharacterRange,
                        parser.position(),
                        end,
                    )));
                }
                Some(start_char) => {
                    let range_start = parser.position();
                    parser.advance_char(out.reborrow())?;
                    if parser.current_char(out.reborrow())? != Some('-') {
                        builder.add_scalar(start_char, out.reborrow())?;
                        has_item = true;
                        continue;
                    }

                    match parser.peek_after_current(out.reborrow())? {
                        Some(']') => {
                            builder.add_scalar(start_char, out.reborrow())?;
                            has_item = true;
                        }
                        Some('\\') | Some('-') => {
                            let end = parser.position() + 1;
                            return Err(out.fail(parser.syntax_error(
                                RegexSyntaxErrorKind::InvalidCharacterRange,
                                range_start,
                                end,
                            )));
                        }
                        Some(_) => {
                            parser.advance_char(out.reborrow())?;
                            let Some(end_char) = parser.advance_char(out.reborrow())? else {
                                return Err(out.fail(parser.syntax_error(
                                    RegexSyntaxErrorKind::UnclosedCharacterClass,
                                    parser.pattern().len(),
                                    parser.pattern().len(),
                                )));
                            };
                            if start_char > end_char {
                                return Err(out.fail(parser.syntax_error(
                                    RegexSyntaxErrorKind::InvalidCharacterRange,
                                    range_start,
                                    parser.position(),
                                )));
                            }
                            builder.add_range(
                                start_char as u32,
                                end_char as u32,
                                out.reborrow(),
                            )?;
                            has_item = true;
                        }
                        None => {
                            return Err(out.fail(parser.syntax_error(
                                RegexSyntaxErrorKind::UnclosedCharacterClass,
                                parser.pattern().len(),
                                parser.pattern().len(),
                            )));
                        }
                    }
                }
            }
        }
    }
}
