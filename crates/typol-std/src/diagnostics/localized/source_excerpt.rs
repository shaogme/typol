use opeo::{OResult, Out};
use typol_core::{ExcerptStatus, SourceSpan};
use typol_i18n::t;

use super::{
    super::{DiagnosticContext, DisplayError, renderer::FormatOutput},
    MessageTranslator,
};

#[derive(Clone, Copy)]
pub(super) struct SourceExcerptAnnotation<'a> {
    pub(super) text: &'a str,
}

pub(super) struct SourceExcerptRequest<'a, 'context> {
    pub(super) span: SourceSpan,
    pub(super) annotation: Option<SourceExcerptAnnotation<'a>>,
    pub(super) context: &'a DiagnosticContext<'context>,
    pub(super) localizer: &'a mut MessageTranslator,
}

pub(super) struct SourceExcerptRenderer;

impl SourceExcerptRenderer {
    pub(super) fn write_location<'slot>(
        span: SourceSpan,
        context: &DiagnosticContext<'_>,
        output: &mut impl FormatOutput,
        localizer: &mut MessageTranslator,
        mut out: Out<'slot, '_, DisplayError>,
    ) -> OResult<'slot, ()> {
        let Some(location) = (context.span_location)(span) else {
            let text = t! {
                name: "report.location_unavailable",
                locale: localizer.translator()
            };
            return localizer.write_translated(text, output, out.reborrow());
        };
        if let Some(source_name) = (context.source_name)(span.source_id()) {
            output.escape(source_name, false, out.reborrow())?;
        } else {
            output.append("source#", out.reborrow())?;
            output.number(span.source_id().get() as usize, out.reborrow())?;
        }
        output.append(":", out.reborrow())?;
        output.number(location.start.line(), out.reborrow())?;
        output.append(":", out.reborrow())?;
        output.number(location.start.column(), out.reborrow())?;
        output.append("-", out.reborrow())?;
        output.number(location.end.line(), out.reborrow())?;
        output.append(":", out.reborrow())?;
        output.number(location.end.column(), out.reborrow())
    }

    pub(super) fn write_source_excerpt<'slot>(
        request: SourceExcerptRequest<'_, '_>,
        output: &mut impl FormatOutput,
        mut out: Out<'slot, '_, DisplayError>,
    ) -> OResult<'slot, ()> {
        let SourceExcerptRequest {
            span,
            annotation,
            context,
            localizer,
        } = request;
        let Some(location) = (context.span_location)(span) else {
            output.append("\n  = ", out.reborrow())?;
            let text = t! {
                name: "report.source_excerpt_unavailable",
                locale: localizer.translator()
            };
            localizer.write_translated(text, output, out.reborrow())?;
            return Ok(());
        };
        let excerpts = context.excerpts;
        let mut found = false;
        for record in excerpts.records().iter().filter(|record| {
            record.source_id() == span.source_id()
                && record.line() >= location.start.line()
                && record.line() <= location.end.line()
        }) {
            found = true;
            let text = record.text();
            output.append("\n  | ", out.reborrow())?;
            output.number(record.line(), out.reborrow())?;
            output.append(" | ", out.reborrow())?;
            Self::write_source_line(text, output, out.reborrow())?;
            if let Some((start, end)) = excerpts.columns(span, record.line()) {
                output.append("\n  |   ", out.reborrow())?;
                Self::write_repeat(
                    output,
                    ' ',
                    Self::visible_column(text, start).saturating_sub(1),
                    out.reborrow(),
                )?;
                Self::write_repeat(
                    output,
                    '^',
                    Self::visible_column(text, end)
                        .saturating_sub(Self::visible_column(text, start))
                        .max(1),
                    out.reborrow(),
                )?;
                if let Some(annotation) = annotation {
                    output.append(" ", out.reborrow())?;
                    output.escape(annotation.text, false, out.reborrow())?;
                }
            }
        }
        if !found || excerpts.span_status(span) == ExcerptStatus::Unavailable {
            output.append("\n  = ", out.reborrow())?;
            let text = t! {
                name: "report.source_excerpt_unavailable",
                locale: localizer.translator()
            };
            localizer.write_translated(text, output, out.reborrow())?;
        }
        Ok(())
    }

    fn write_source_line<'slot>(
        text: &str,
        output: &mut impl FormatOutput,
        mut out: Out<'slot, '_, DisplayError>,
    ) -> OResult<'slot, ()> {
        for character in text.chars() {
            output.step(character.len_utf8(), out.reborrow())?;
            match character {
                '\t' => output.append("\\t", out.reborrow())?,
                '\0'..='\u{1f}'
                | '\u{7f}'..='\u{9f}'
                | '\u{2028}'..='\u{202e}'
                | '\u{2066}'..='\u{2069}' => {
                    for escaped in character.escape_unicode() {
                        let mut encoded = [0; 4];
                        output.append(escaped.encode_utf8(&mut encoded), out.reborrow())?;
                    }
                }
                _ => {
                    let mut encoded = [0; 4];
                    output.append(character.encode_utf8(&mut encoded), out.reborrow())?;
                }
            }
        }
        Ok(())
    }

    fn visible_column(text: &str, column: usize) -> usize {
        text.chars()
            .take(column.saturating_sub(1))
            .map(|character| match character {
                '\t' => 2,
                '\0'..='\u{1f}'
                | '\u{7f}'..='\u{9f}'
                | '\u{2028}'..='\u{202e}'
                | '\u{2066}'..='\u{2069}' => character.escape_unicode().count(),
                _ => 1,
            })
            .sum::<usize>()
            .saturating_add(1)
    }

    fn write_repeat<'slot>(
        output: &mut impl FormatOutput,
        character: char,
        count: usize,
        mut out: Out<'slot, '_, DisplayError>,
    ) -> OResult<'slot, ()> {
        let mut encoded = [0; 4];
        let character = character.encode_utf8(&mut encoded);
        for _ in 0..count {
            output.append(character, out.reborrow())?;
        }
        Ok(())
    }
}
