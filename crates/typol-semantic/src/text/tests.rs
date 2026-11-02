use alloc::{format, string::String};
use core::fmt::{Debug, Formatter, Result as FmtResult};

use crate::TextValidation;
use typol_core::BudgetError;
use typol_test_support::TestSlot;

enum TestError {
    Budget(BudgetError),
    Assertion(String),
}

impl Debug for TestError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        match self {
            Self::Budget(error) => write!(formatter, "Budget({error:?})"),
            Self::Assertion(message) => write!(formatter, "Assertion({message})"),
        }
    }
}

impl From<BudgetError> for TestError {
    fn from(error: BudgetError) -> Self {
        Self::Budget(error)
    }
}

type TestResult<T> = Result<T, TestError>;

macro_rules! check {
    ($condition:expr $(, $($message:tt)+)? $(,)?) => {{
        if !$condition {
            return Err(TestError::Assertion(alloc::format!(
                "assertion failed: {}",
                stringify!($condition)
            )));
        }
    }};
}

macro_rules! check_eq {
    ($actual:expr, $expected:expr $(,)?) => {{
        let actual = &$actual;
        let expected = &$expected;
        if actual != expected {
            return Err(TestError::Assertion(alloc::format!(
                "assertion failed: {}; actual={actual:?}, expected={expected:?}",
                stringify!($actual)
            )));
        }
    }};
}

#[test]
fn strict_ip_literals_cover_compression_and_embedded_ipv4() -> TestResult<()> {
    let mut errors = TestSlot::<TestError>::default();
    for text in ["0.0.0.0", "255.255.255.255", "192.0.2.9"] {
        check!(
            errors
                .access(|out| TextValidation::parse_ipv4::<TestError>(text, out))?
                .is_some(),
            "{text}"
        );
    }
    for text in [
        "01.2.3.4",
        "+1.2.3.4",
        "256.2.3.4",
        "1.2.3",
        "1.2.3.4.5",
        "1..2.3",
        "1.2.3.4 ",
    ] {
        check!(
            errors
                .access(|out| TextValidation::parse_ipv4::<TestError>(text, out))?
                .is_none(),
            "{text}"
        );
    }
    check_eq!(
        errors.access(|out| TextValidation::parse_ipv6::<TestError>("2001:db8::1", out))?,
        errors
            .access(|out| TextValidation::parse_ipv6::<TestError>("2001:0DB8:0:0:0:0:0:1", out))?
    );
    check_eq!(
        errors.access(|out| TextValidation::parse_ipv6::<TestError>("::ffff:192.0.2.1", out))?,
        errors.access(|out| TextValidation::parse_ipv6::<TestError>(
            "0:0:0:0:0:ffff:c000:201",
            out
        ))?
    );
    for text in [
        "::",
        "::1",
        "1::",
        "1:2:3:4:5:6:7:8",
        "1:2:3:4:5:6:192.0.2.1",
    ] {
        check!(
            errors
                .access(|out| TextValidation::parse_ipv6::<TestError>(text, out))?
                .is_some(),
            "{text}"
        );
    }
    for text in [
        ":::1",
        "1::2::3",
        "1:2:3:4:5:6:7",
        "1:2:3:4:5:6:7:8:9",
        "1:2:3:4:5:6:7::8",
        "::ffff:01.2.3.4",
        "192.0.2.1::",
        "::1%eth0",
        "[::1]",
        "::12345",
        "::g",
        "::1:",
    ] {
        check!(
            errors
                .access(|out| TextValidation::parse_ipv6::<TestError>(text, out))?
                .is_none(),
            "{text}"
        );
    }
    Ok(())
}

#[test]
fn dns_labels_and_total_lengths_are_bounded_without_ipv4_fallback() -> TestResult<()> {
    let mut errors = TestSlot::<TestError>::default();
    for text in [
        "localhost",
        "Example.COM.",
        "a-b.example",
        "1a.2.3.4",
        "192.0.2.1.",
    ] {
        check!(
            errors.access(|out| TextValidation::valid_host::<TestError>(text, out))?,
            "{text}"
        );
    }
    for text in [
        "",
        ".",
        "example..com",
        "example.com..",
        "-a.com",
        "a-.com",
        "a_b.com",
        "é.com",
        "a:80",
        "http://a.com",
        " a.com",
        "01.2.3.4",
        "01.2.3.4.",
        "256.2.3.4",
        "256.2.3.4.",
    ] {
        check!(
            !errors.access(|out| TextValidation::valid_host::<TestError>(text, out))?,
            "{text}"
        );
    }
    check!(errors.access(|out| TextValidation::valid_host::<TestError>(&"a".repeat(63), out))?);
    check!(!errors.access(|out| TextValidation::valid_host::<TestError>(&"a".repeat(64), out))?);
    let host = format!(
        "{}.{}.{}.{}",
        "a".repeat(63),
        "b".repeat(63),
        "c".repeat(63),
        "d".repeat(61)
    );
    check_eq!(host.len(), 253);
    check!(errors.access(|out| TextValidation::valid_host::<TestError>(&host, out))?);
    check!(errors.access(|out| TextValidation::valid_host::<TestError>(&format!("{host}."), out))?);
    check!(
        !errors.access(|out| TextValidation::valid_host::<TestError>(&format!("{host}d"), out))?
    );
    Ok(())
}

#[test]
fn email_ascii_subset_and_cidr_prefixes_have_independent_counterexamples() -> TestResult<()> {
    let mut errors = TestSlot::<TestError>::default();
    for text in ["a+b@example.com", "a_b@example.com", "a.b-c@EXAMPLE.com"] {
        check!(
            errors.access(|out| TextValidation::valid_email::<TestError>(text, out))?,
            "{text}"
        );
    }
    for text in [
        "a@localhost",
        ".a@example.com",
        "a.@example.com",
        "a..b@example.com",
        "a@@example.com",
        "a@-example.com",
        "a@example-.com",
        "a@exam_ple.com",
        "a@example..com",
        "a@example.com.",
        "a b@example.com",
        "a!b@example.com",
        "é@example.com",
        "@example.com",
        "a@",
    ] {
        check!(
            !errors.access(|out| TextValidation::valid_email::<TestError>(text, out))?,
            "{text}"
        );
    }
    check!(
        errors.access(|out| TextValidation::valid_email::<TestError>(
            &format!("{}@example.com", "a".repeat(64)),
            out
        ))?
    );
    check!(
        !errors.access(|out| TextValidation::valid_email::<TestError>(
            &format!("{}@example.com", "a".repeat(65)),
            out
        ))?
    );
    check!(
        errors.access(|out| TextValidation::valid_email::<TestError>(
            &format!("a@{}.com", "a".repeat(63)),
            out
        ))?
    );
    check!(
        !errors.access(|out| TextValidation::valid_email::<TestError>(
            &format!("a@{}.com", "a".repeat(64)),
            out
        ))?
    );
    let domain = format!(
        "{}.{}.{}.{}",
        "a".repeat(63),
        "b".repeat(63),
        "c".repeat(63),
        "d".repeat(61)
    );
    check!(
        errors
            .access(|out| TextValidation::valid_email::<TestError>(&format!("a@{domain}"), out))?
    );
    check!(
        !errors
            .access(|out| TextValidation::valid_email::<TestError>(&format!("a@{domain}d"), out))?
    );
    for text in [
        "192.0.2.9/0",
        "192.0.2.9/24",
        "192.0.2.9/32",
        "192.0.2.9/00024",
    ] {
        check!(
            errors.access(|out| TextValidation::valid_cidr::<TestError>(text, true, out))?,
            "{text}"
        );
    }
    for text in ["::1/0", "::1/128", "2001:db8::9/64", "::ffff:192.0.2.9/128"] {
        check!(
            errors.access(|out| TextValidation::valid_cidr::<TestError>(text, false, out))?,
            "{text}"
        );
        check!(
            !errors.access(|out| TextValidation::valid_cidr::<TestError>(text, true, out))?,
            "{text}"
        );
    }
    for text in [
        "192.0.2.9/33",
        "::1/129",
        "192.0.2.9/-1",
        "192.0.2.9/+1",
        "192.0.2.9/",
        "01.2.3.4/24",
        "256.2.3.4/24",
        "host/24",
        "[::1]/128",
        "::1%eth0/128",
        "::1/128/0",
        "192.0.2.9/２４",
        "192.0.2.9/24 ",
    ] {
        check!(
            !errors.access(|out| TextValidation::valid_cidr::<TestError>(text, false, out))?,
            "{text}"
        );
    }
    Ok(())
}
