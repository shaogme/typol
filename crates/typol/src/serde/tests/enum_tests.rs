use super::support::{Choice, CustomError, SerdeTestSupport as Support};
use crate::{diagnostics::CodecErrors, test_support::TestValues, value::EnumPayload};
use typol_test_support::{
    TestOpeo, TestResult, test_verify as assert, test_verify_eq as assert_eq,
};

fn secret_is_hidden(errors: &CodecErrors) -> bool {
    !format!("{errors:?}{errors}").contains("secret")
}

#[test]
fn enum_payloads_round_trip_and_wrong_variants_reject() -> TestResult {
    let schema = Support::compile(
        "enum Choice { Unit, Single(string), Structure { first: u8, } } entry struct Root { value: Choice, }",
    )?;
    for value in [
        Choice::Unit,
        Choice::Single("secret payload".to_owned()),
        Choice::Structure { first: 7 },
    ] {
        assert_eq!(
            Support::decode::<Choice>(&schema, &Support::encode(&schema, &value)?.test_value()?)?
                .test_value()?,
            value
        );
    }
    let payload = TestOpeo::value(|out| EnumPayload::single(TestValues::signed(1), None, out))?;
    let tree = TestValues::enumeration_unmetered("Unit".to_owned(), payload);
    assert_eq!(
        Support::code(
            Support::decode::<Choice>(&schema, &tree)?
                .err()
                .test_value()?
        )?,
        15
    );
    let tree = TestValues::enumeration_unmetered("secret variant".to_owned(), EnumPayload::unit());
    let error = Support::decode::<Choice>(&schema, &tree)?
        .err()
        .test_value()?;
    assert!(secret_is_hidden(&error));
    assert_eq!(Support::code(error)?, 15);
    let tree = TestValues::enumeration_unmetered(
        "Unit".to_owned(),
        TestOpeo::value(|out| {
            EnumPayload::single(
                TestValues::string_unmetered("secret payload".to_owned()),
                None,
                out,
            )
        })?,
    );
    let error = Support::decode::<Choice>(&schema, &tree)?
        .err()
        .test_value()?;
    assert!(secret_is_hidden(&error));
    assert_eq!(Support::code(error)?, 15);
    let schema = Support::compile("enum Other { Unit } entry struct Root { value: Other, }")?;
    assert_eq!(
        Support::code(
            Support::encode(&schema, &Choice::Unit)?
                .err()
                .test_value()?
        )?,
        15
    );

    Ok(())
}

#[test]
fn custom_error_text_is_discarded_without_formatting() -> TestResult {
    let schema = Support::compile("entry struct Root { value: string @secret, }")?;
    let error = Support::encode(&schema, &CustomError)?.err().test_value()?;
    assert!(secret_is_hidden(&error));
    assert_eq!(Support::code(error)?, 15);
    let error = Support::decode::<CustomError>(
        &schema,
        &TestValues::string_unmetered("secret original value".to_owned()),
    )?
    .err()
    .test_value()?;
    assert!(secret_is_hidden(&error));
    assert_eq!(Support::code(error)?, 15);

    Ok(())
}
