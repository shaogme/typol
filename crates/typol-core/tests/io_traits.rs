use typol_core::{FmtWriteSink, Write, WriteError};

#[test]
fn string_and_vector_sinks_append_complete_fragments() {
    let mut text = String::from("config");
    assert_eq!(text.write_all(b" text"), Ok(()));
    assert_eq!(text, "config text");

    let mut bytes = vec![1, 2];
    assert_eq!(bytes.write_all(&[3, 4]), Ok(()));
    assert_eq!(bytes, [1, 2, 3, 4]);
}

#[test]
fn string_sink_rejects_non_utf8_fragments_without_mutating_output() {
    let mut text = String::from("safe");
    assert_eq!(
        text.write_all(&[0xff]),
        Err(WriteError::InternalInvariantFailure)
    );
    assert_eq!(text, "safe");
}

#[test]
fn fmt_sink_adapts_text_writers() {
    let mut text = String::new();
    let mut sink = FmtWriteSink::new(&mut text);
    assert_eq!(sink.write_all(b"formatted"), Ok(()));
    assert_eq!(text, "formatted");
}
