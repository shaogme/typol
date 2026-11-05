#![no_std]

extern crate alloc;

mod config_values;

use alloc::{format, vec::Vec};
use core::{fmt::Debug, ops::Range};
use opeo::{OResult, Out};
use typol_config::{
    ConfigParseErrors, ConfigParseRequest, ConfigParser, Environment, ValidatedConfigValue,
};
use typol_core::{ConfigValue, Read, SourceId, SourceReadError, SourceReadErrorKind};
use typol_schema::{
    SchemaCompileErrors, SchemaCompileOptions, SchemaCompiler, SchemaResolver, SchemaSource,
    SourceObserver,
};
use typol_semantic::CompiledSchema;

pub use typol_opeo_test_support::{TestFailure, TestOpeo, TestOpeoOutcome, TestResult, TestSlot};

pub use config_values::{FieldValue, MapEntry, Named, Payload, ValueFactory, ValueViewInput};

pub struct OwnedReadSource {
    bytes: Vec<u8>,
    offset: usize,
}

impl OwnedReadSource {
    pub fn new(text: &str) -> Self {
        Self {
            bytes: text.as_bytes().to_vec(),
            offset: 0,
        }
    }

    pub fn replace(&mut self, text: &str) {
        self.bytes.clear();
        self.bytes.extend_from_slice(text.as_bytes());
        self.offset = 0;
    }
}

impl Default for OwnedReadSource {
    fn default() -> Self {
        Self::new("")
    }
}

impl Read<SourceReadError> for OwnedReadSource {
    fn read<'slot>(
        &mut self,
        output: &mut [u8],
        source_id: SourceId,
        offset: usize,
        out: Out<'slot, '_, SourceReadError>,
    ) -> OResult<'slot, usize> {
        if output.is_empty() {
            return Ok(0);
        }
        let Some(start) = self.bytes.get(self.offset..) else {
            return Err(out.fail(
                SourceReadError::new(SourceReadErrorKind::InternalInvariantFailure)
                    .with_input(source_id, offset),
            ));
        };
        let count = start.len().min(output.len());
        let Some(source) = start.get(..count) else {
            return Err(out.fail(
                SourceReadError::new(SourceReadErrorKind::InternalInvariantFailure)
                    .with_input(source_id, offset),
            ));
        };
        let Some(destination) = output.get_mut(..count) else {
            return Err(out.fail(
                SourceReadError::new(SourceReadErrorKind::InternalInvariantFailure)
                    .with_input(source_id, offset),
            ));
        };
        destination.copy_from_slice(source);
        self.offset += count;
        Ok(count)
    }
}

pub struct ConfigTestSupport;

impl ConfigTestSupport {
    pub fn parse(
        schema: &CompiledSchema,
        request: ConfigParseRequest<'_>,
        environment: &mut dyn Environment,
    ) -> TestOpeoOutcome<ConfigValue, ConfigParseErrors> {
        match Self::parse_validated_config(schema, request, environment) {
            TestOpeoOutcome::Value(value) => TestOpeoOutcome::Value(value.into_config_value()),
            TestOpeoOutcome::Error(error) => TestOpeoOutcome::Error(error),
        }
    }

    pub fn parse_validated_config(
        schema: &CompiledSchema,
        request: ConfigParseRequest<'_>,
        environment: &mut dyn Environment,
    ) -> TestOpeoOutcome<ValidatedConfigValue, ConfigParseErrors> {
        TestOpeo::outcome(|out| ConfigParser::new(schema).parse(request, environment, out))
    }

    pub fn compile_schema<S: Read<SourceReadError>>(
        options: SchemaCompileOptions,
        source: SchemaSource<'_, S>,
        resolver: Option<&mut dyn SchemaResolver>,
        observer: Option<&mut dyn SourceObserver>,
    ) -> TestOpeoOutcome<CompiledSchema, SchemaCompileErrors> {
        TestOpeo::outcome(|mut out| {
            let compiler = SchemaCompiler::new(options, out.reborrow())?;
            match resolver {
                Some(resolver) => compiler.compile(source, resolver, observer, out.reborrow()),
                None => match observer {
                    Some(observer) => {
                        compiler.compile_without_imports_with_observer(source, observer, out)
                    }
                    None => compiler.compile_without_imports(source, out),
                },
            }
        })
    }
}

pub struct TestCheck;

impl TestCheck {
    pub fn equal<L, R>(left: &L, right: &R, context: &str) -> TestResult<()>
    where
        L: PartialEq<R> + Debug,
        R: Debug,
    {
        if left == right {
            Ok(())
        } else {
            Err(TestFailure::new(format!(
                "{context}: actual value {left:?}, expected value {right:?}"
            )))
        }
    }

    pub fn different<L, R>(left: &L, right: &R, context: &str) -> TestResult<()>
    where
        L: PartialEq<R> + Debug,
        R: Debug,
    {
        if left != right {
            Ok(())
        } else {
            Err(TestFailure::new(format!(
                "{context}: values unexpectedly match: {left:?}"
            )))
        }
    }

    pub fn condition(value: bool, context: &str) -> TestResult<()> {
        if value {
            Ok(())
        } else {
            Err(TestFailure::new(format!("{context}: condition is false")))
        }
    }

    pub fn item<T>(values: &[T], index: usize) -> TestResult<&T> {
        values.get(index).ok_or_else(|| {
            TestFailure::new(format!("container is missing test value at index {index}"))
        })
    }

    pub fn range<T>(values: &[T], range: Range<usize>) -> TestResult<&[T]> {
        let context = format!(
            "container is missing test range {}..{}",
            range.start, range.end
        );
        values.get(range).ok_or_else(|| TestFailure::new(context))
    }
}

pub trait TestExtract<T> {
    fn test_value(self) -> TestResult<T>;
}

impl<T, E> TestExtract<T> for Result<T, E>
where
    E: Debug,
{
    fn test_value(self) -> TestResult<T> {
        self.map_err(|error| TestFailure::new(format!("operation failed: {error:?}")))
    }
}

impl<T> TestExtract<T> for Option<T> {
    fn test_value(self) -> TestResult<T> {
        self.ok_or_else(|| TestFailure::missing("expected a value to be present"))
    }
}

pub trait TestExpect<T> {
    fn test_expect(self, context: &str) -> TestResult<T>;
}

impl<T> TestExpect<T> for Option<T> {
    fn test_expect(self, context: &str) -> TestResult<T> {
        self.ok_or_else(|| TestFailure::missing(context))
    }
}

pub trait TestError<E> {
    fn test_error(self) -> TestResult<E>;
}

impl<T, E> TestError<E> for Result<T, E>
where
    T: Debug,
{
    fn test_error(self) -> TestResult<E> {
        match self {
            Err(error) => Ok(error),
            Ok(value) => Err(TestFailure::new(format!(
                "expected operation to fail, but got successful value: {value:?}"
            ))),
        }
    }
}

pub trait TestValueExt<T> {
    type Error;

    fn into_test_value(self) -> TestResult<T>;
}

impl<T, E> TestValueExt<T> for Result<T, E>
where
    E: Debug,
{
    type Error = E;

    fn into_test_value(self) -> TestResult<T> {
        self.map_err(|error| TestFailure::new(format!("unexpected error: {error:?}")))
    }
}

impl<T> TestValueExt<T> for Option<T> {
    type Error = ();

    fn into_test_value(self) -> TestResult<T> {
        self.ok_or_else(|| TestFailure::missing("expected a value to be present"))
    }
}

pub trait TestResultExt<T> {
    type Error;

    fn into_test_error(self) -> TestResult<Self::Error>;
}

impl<T, E> TestResultExt<T> for Result<T, E>
where
    T: Debug,
{
    type Error = E;

    fn into_test_error(self) -> TestResult<Self::Error> {
        match self {
            Err(error) => Ok(error),
            Ok(_) => Err(TestFailure::new(
                "expected operation to fail, but it succeeded",
            )),
        }
    }
}

#[macro_export]
macro_rules! test_assert {
    ($condition:expr $(,)?) => {
        $crate::TestCheck::condition($condition, stringify!($condition))
    };
}

#[macro_export]
macro_rules! test_eq {
    ($left:expr, $right:expr $(,)?) => {
        $crate::TestCheck::equal(&$left, &$right, stringify!($left == $right))
    };
}

#[macro_export]
macro_rules! test_ne {
    ($left:expr, $right:expr $(,)?) => {
        $crate::TestCheck::different(&$left, &$right, stringify!($left != $right))
    };
}

#[macro_export]
macro_rules! test_verify {
    ($condition:expr $(,)?) => {
        if !$condition {
            return Err($crate::TestFailure::from_arguments(::core::format_args!(
                "check failed: {}",
                stringify!($condition)
            )));
        }
    };
    ($condition:expr, $($message:tt)+) => {
        if !$condition {
            return Err($crate::TestFailure::from_arguments(::core::format_args!($($message)+)));
        }
    };
}

#[macro_export]
macro_rules! test_verify_eq {
    ($left:expr, $right:expr $(,)?) => {{
        match (&$left, &$right) {
            (left, right) if left == right => {}
            (left, right) => {
                return Err($crate::TestFailure::equality_mismatch(left, right));
            }
        }
    }};
    ($left:expr, $right:expr, $($message:tt)+) => {{
        match (&$left, &$right) {
            (left, right) if left == right => {}
            (left, right) => {
                return Err($crate::TestFailure::equality_mismatch_with(
                    left,
                    right,
                    ::core::format_args!($($message)+),
                ));
            }
        }
    }};
}

#[macro_export]
macro_rules! test_verify_ne {
    ($left:expr, $right:expr $(,)?) => {{
        match (&$left, &$right) {
            (left, right) if left != right => {}
            (left, right) => {
                return Err($crate::TestFailure::inequality_mismatch(left, right));
            }
        }
    }};
    ($left:expr, $right:expr, $($message:tt)+) => {{
        match (&$left, &$right) {
            (left, right) if left != right => {}
            (left, right) => {
                return Err($crate::TestFailure::inequality_mismatch_with(
                    left,
                    right,
                    ::core::format_args!($($message)+),
                ));
            }
        }
    }};
}
