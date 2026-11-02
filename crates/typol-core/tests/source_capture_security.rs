use std::{error::Error, io::Error as IoError};
use typol_core::{
    Diagnostic, DiagnosticCode, ExcerptLimits, ExcerptStatus, LocatedSpan, SecretRange,
    SourceCapture, SourceExcerptSet, SourceId, SourceLocation, SourceSpan, Span, SpanLocation,
};

macro_rules! ensure {
    ($condition:expr $(,)?) => {
        if !$condition {
            return Err(
                IoError::other(concat!("assertion failed: ", stringify!($condition))).into(),
            );
        }
    };
}

fn located(span: SourceSpan, start: usize, end: usize) -> Result<LocatedSpan, IoError> {
    let start =
        SourceLocation::new(1, start).ok_or_else(|| IoError::other("start column is invalid"))?;
    let end = SourceLocation::new(1, end).ok_or_else(|| IoError::other("end column is invalid"))?;
    Ok(LocatedSpan::new(span, Some(SpanLocation { start, end })))
}

fn report(capture: SourceCapture, span: SourceSpan) -> Result<SourceExcerptSet, Box<dyn Error>> {
    let mut excerpts = SourceExcerptSet::new(ExcerptLimits::default(), None);
    let mut diagnostic = Diagnostic::emergency(DiagnosticCode::CONFIG_VALUE_ERROR);
    diagnostic.set_located_primary(Some(located(span, 1, 2)?));
    capture.finish(&mut excerpts, &[diagnostic]);
    Ok(excerpts)
}

fn assert_invalid_secret_range(
    source_id: u32,
    start: usize,
    end: usize,
) -> Result<(), Box<dyn Error>> {
    let source_id = SourceId::new(source_id);
    let mut capture = SourceCapture::new(source_id, ExcerptLimits::default(), None);
    capture.feed("aéz\n".as_bytes());
    let secret = capture
        .begin_secret(start)
        .ok_or_else(|| IoError::other("failed to begin secret span"))?;
    capture.end_secret(secret, end);
    capture.end_input();
    let span = SourceSpan::new(source_id, Span::from_zero_to(4));
    let excerpts = report(capture, span)?;

    ensure!(excerpts.span_status(span) == ExcerptStatus::Unavailable);
    ensure!(excerpts.records().is_empty());
    Ok(())
}

#[test]
fn completed_secret_is_masked_before_an_excerpt_becomes_observable() -> Result<(), Box<dyn Error>> {
    let source_id = SourceId::new(1);
    let input = b"password=secret\n";
    let mut capture = SourceCapture::new(source_id, ExcerptLimits::default(), None);
    capture.feed(input);
    let secret: SecretRange = capture
        .begin_secret(9)
        .ok_or_else(|| IoError::other("failed to begin secret span"))?;
    capture.end_secret(secret, 15);
    capture.end_input();
    let span = SourceSpan::new(source_id, Span::from_zero_to(15));
    let excerpts = report(capture, span)?;

    ensure!(excerpts.status() == ExcerptStatus::Complete);
    let text = excerpts
        .records()
        .first()
        .ok_or_else(|| IoError::other("excerpt line is missing"))?
        .text();
    ensure!(text.contains("<redacted>"));
    ensure!(!text.contains("secret"));
    Ok(())
}

#[test]
fn foreign_secret_handle_invalidates_capture_instead_of_revealing_input()
-> Result<(), Box<dyn Error>> {
    let first_id = SourceId::new(2);
    let second_id = SourceId::new(3);
    let mut first = SourceCapture::new(first_id, ExcerptLimits::default(), None);
    let mut second = SourceCapture::new(second_id, ExcerptLimits::default(), None);
    first.feed(b"secret\n");
    second.feed(b"secret\n");
    let foreign = first
        .begin_secret(0)
        .ok_or_else(|| IoError::other("failed to begin secret span"))?;
    let _local = second
        .begin_secret(0)
        .ok_or_else(|| IoError::other("failed to begin secret span"))?;
    second.end_secret(foreign, 6);
    second.end_input();

    let span = SourceSpan::new(second_id, Span::from_zero_to(6));
    let excerpts = report(second, span)?;
    ensure!(excerpts.span_status(span) == ExcerptStatus::Unavailable);
    ensure!(excerpts.records().is_empty());
    Ok(())
}

#[test]
fn invalid_secret_end_disables_excerpt_output() -> Result<(), Box<dyn Error>> {
    let source_id = SourceId::new(4);
    let mut capture = SourceCapture::new(source_id, ExcerptLimits::default(), None);
    capture.feed(b"token\n");
    let secret = capture
        .begin_secret(0)
        .ok_or_else(|| IoError::other("failed to begin secret span"))?;
    capture.end_secret(secret, usize::MAX - 1);
    capture.end_input();
    let span = SourceSpan::new(source_id, Span::from_zero_to(5));
    let excerpts = report(capture, span)?;

    ensure!(excerpts.span_status(span) == ExcerptStatus::Unavailable);
    ensure!(excerpts.records().is_empty());
    Ok(())
}

#[test]
fn secret_ranges_must_start_and_end_on_utf8_boundaries() -> Result<(), Box<dyn Error>> {
    assert_invalid_secret_range(5, 2, 3)?;
    assert_invalid_secret_range(8, 1, 2)?;
    Ok(())
}

#[test]
fn unfinished_capture_cannot_produce_an_excerpt() -> Result<(), Box<dyn Error>> {
    let source_id = SourceId::new(6);
    let mut capture = SourceCapture::new(source_id, ExcerptLimits::default(), None);
    capture.feed(b"public text\n");
    let span = SourceSpan::new(source_id, Span::from_zero_to(11));
    let excerpts = report(capture, span)?;

    ensure!(excerpts.span_status(span) == ExcerptStatus::Unavailable);
    ensure!(excerpts.records().is_empty());
    Ok(())
}

#[test]
fn feeding_after_capture_is_sealed_disables_excerpt_output() -> Result<(), Box<dyn Error>> {
    let source_id = SourceId::new(7);
    let mut capture = SourceCapture::new(source_id, ExcerptLimits::default(), None);
    capture.feed(b"first\n");
    capture.end_input();
    capture.feed(b"secret\n");
    let span = SourceSpan::new(source_id, Span::from_zero_to(5));
    let excerpts = report(capture, span)?;

    ensure!(excerpts.span_status(span) == ExcerptStatus::Unavailable);
    ensure!(excerpts.records().is_empty());
    Ok(())
}
