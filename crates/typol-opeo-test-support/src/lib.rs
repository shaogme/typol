#![no_std]

extern crate alloc;

use alloc::{fmt::format, format, string::String};
use core::{
    error::Error,
    fmt::{self, Arguments, Debug, Display, Formatter},
};
use opeo::{ErrSlot, OResult, Out};

#[derive(Debug)]
pub struct TestFailure {
    message: String,
}

pub type TestResult<T = ()> = Result<T, TestFailure>;

impl TestFailure {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn from_debug(error: impl Debug) -> Self {
        Self::new(format!("{error:?}"))
    }

    #[doc(hidden)]
    pub fn from_arguments(arguments: Arguments<'_>) -> Self {
        Self::new(format(arguments))
    }

    #[doc(hidden)]
    pub fn equality_mismatch<L, R>(left: &L, right: &R) -> Self
    where
        L: Debug,
        R: Debug,
    {
        Self::new(format!(
            "assertion failed: left value {left:?}, right value {right:?}"
        ))
    }

    #[doc(hidden)]
    pub fn equality_mismatch_with<L, R>(left: &L, right: &R, context: Arguments<'_>) -> Self
    where
        L: Debug,
        R: Debug,
    {
        Self::new(format!(
            "{}: left value {left:?}, right value {right:?}",
            format(context)
        ))
    }

    #[doc(hidden)]
    pub fn inequality_mismatch<L, R>(left: &L, right: &R) -> Self
    where
        L: Debug,
        R: Debug,
    {
        Self::new(format!(
            "assertion failed: left and right values should differ: {left:?}, {right:?}"
        ))
    }

    #[doc(hidden)]
    pub fn inequality_mismatch_with<L, R>(left: &L, right: &R, context: Arguments<'_>) -> Self
    where
        L: Debug,
        R: Debug,
    {
        Self::new(format!(
            "{}: left and right values should differ: {left:?}, {right:?}",
            format(context)
        ))
    }

    #[doc(hidden)]
    pub fn missing(context: &str) -> Self {
        Self::new(format!("{context}: expected value is missing"))
    }
}

impl Display for TestFailure {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for TestFailure {}

impl From<String> for TestFailure {
    fn from(message: String) -> Self {
        Self::new(message)
    }
}

impl From<&str> for TestFailure {
    fn from(message: &str) -> Self {
        Self::new(message)
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum TestOpeoOutcome<T, E> {
    Value(T),
    Error(E),
}

impl<T, E> TestOpeoOutcome<T, E> {
    pub fn map<U>(self, transform: impl FnOnce(T) -> U) -> TestOpeoOutcome<U, E> {
        match self {
            Self::Value(value) => TestOpeoOutcome::Value(transform(value)),
            Self::Error(error) => TestOpeoOutcome::Error(error),
        }
    }

    pub fn is_ok(&self) -> bool {
        matches!(self, Self::Value(_))
    }

    pub fn is_err(&self) -> bool {
        matches!(self, Self::Error(_))
    }

    pub fn value(&self) -> Option<&T> {
        match self {
            Self::Value(value) => Some(value),
            Self::Error(_) => None,
        }
    }

    pub fn error(&self) -> Option<&E> {
        match self {
            Self::Value(_) => None,
            Self::Error(error) => Some(error),
        }
    }

    pub fn test_value(self) -> TestResult<T> {
        match self {
            Self::Value(value) => Ok(value),
            Self::Error(_) => Err(TestFailure::new("OPEO call failed")),
        }
    }

    pub fn test_error(self) -> TestResult<E> {
        match self {
            Self::Value(_) => Err(TestFailure::new(
                "OPEO call did not produce the expected error",
            )),
            Self::Error(error) => Ok(error),
        }
    }
}

pub struct TestOpeo;

impl TestOpeo {
    pub fn call<E, T>(
        operation: impl for<'slot> FnOnce(Out<'slot, 'slot, E>) -> OResult<'slot, T>,
    ) -> Result<T, E> {
        let mut slot = ErrSlot::new();
        match slot.try_call(operation) {
            Ok(value) => Ok(value),
            Err(caught) => Err(caught.take()),
        }
    }

    pub fn outcome<E, T>(
        operation: impl for<'slot> FnOnce(Out<'slot, 'slot, E>) -> OResult<'slot, T>,
    ) -> TestOpeoOutcome<T, E> {
        match Self::call(operation) {
            Ok(value) => TestOpeoOutcome::Value(value),
            Err(error) => TestOpeoOutcome::Error(error),
        }
    }

    pub fn value<E: Debug, T>(
        operation: impl for<'slot> FnOnce(Out<'slot, 'slot, E>) -> OResult<'slot, T>,
    ) -> TestResult<T> {
        Self::call(operation)
            .map_err(|error| TestFailure::new(format!("OPEO call failed: {error:?}")))
    }

    pub fn error<E, T>(
        operation: impl for<'slot> FnOnce(Out<'slot, 'slot, E>) -> OResult<'slot, T>,
    ) -> TestResult<E> {
        match Self::call(operation) {
            Ok(_) => Err(TestFailure::new(
                "expected the OPEO call to fail, but it succeeded",
            )),
            Err(error) => Ok(error),
        }
    }
}

pub struct TestSlot<E> {
    slot: ErrSlot<E>,
}

impl<E> TestSlot<E> {
    pub fn new() -> Self {
        Self {
            slot: ErrSlot::new(),
        }
    }

    pub fn access<T>(
        &mut self,
        operation: impl for<'slot> FnOnce(Out<'slot, 'slot, E>) -> OResult<'slot, T>,
    ) -> Result<T, E> {
        match self.slot.try_call(operation) {
            Ok(value) => Ok(value),
            Err(caught) => Err(caught.take()),
        }
    }

    pub fn run<T>(
        &mut self,
        operation: impl for<'slot> FnOnce(Out<'slot, 'slot, E>) -> OResult<'slot, T>,
    ) -> TestOpeoOutcome<T, E> {
        match self.access(operation) {
            Ok(value) => TestOpeoOutcome::Value(value),
            Err(error) => TestOpeoOutcome::Error(error),
        }
    }

    pub fn slot(&mut self) -> &mut ErrSlot<E> {
        &mut self.slot
    }
}

impl<E> Default for TestSlot<E> {
    fn default() -> Self {
        Self::new()
    }
}

#[macro_export]
macro_rules! ensure {
    ($condition:expr $(,)?) => {
        if !$condition {
            return Err($crate::TestFailure::from_arguments(::core::format_args!(
                "assertion failed: {}",
                stringify!($condition)
            ))
            .into());
        }
    };
    ($condition:expr, failure = $failure:expr $(,)?) => {
        if !$condition {
            return Err($failure.into());
        }
    };
    ($condition:expr, $($message:tt)+) => {
        if !$condition {
            return Err($crate::TestFailure::from_arguments(::core::format_args!($($message)+)).into());
        }
    };
}

#[macro_export]
macro_rules! ensure_eq {
    ($left:expr, $right:expr $(,)?) => {{
        match (&$left, &$right) {
            (left, right) if left == right => {}
            (left, right) => {
                return Err($crate::TestFailure::equality_mismatch(left, right).into());
            }
        }
    }};
    ($left:expr, $right:expr, failure = $failure:expr $(,)?) => {{
        match (&$left, &$right) {
            (left, right) if left == right => {}
            _ => return Err($failure.into()),
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
                )
                .into());
            }
        }
    }};
}

#[macro_export]
macro_rules! ensure_ne {
    ($left:expr, $right:expr $(,)?) => {{
        match (&$left, &$right) {
            (left, right) if left != right => {}
            (left, right) => {
                return Err($crate::TestFailure::inequality_mismatch(left, right).into());
            }
        }
    }};
    ($left:expr, $right:expr, failure = $failure:expr $(,)?) => {{
        match (&$left, &$right) {
            (left, right) if left != right => {}
            _ => return Err($failure.into()),
        }
    }};
}

#[macro_export]
macro_rules! ensure_ok {
    ($result:expr $(,)?) => {{
        let result = &$result;
        if let Err(error) = result {
            return Err($crate::TestFailure::from_arguments(::core::format_args!(
                "expected success, but operation returned an error: {error:?}"
            ))
            .into());
        }
    }};
    ($result:expr, failure = $failure:expr $(,)?) => {{
        let result = &$result;
        if result.is_err() {
            return Err($failure.into());
        }
    }};
}

#[macro_export]
macro_rules! ensure_err {
    ($result:expr $(,)?) => {{
        let result = &$result;
        if result.is_ok() {
            return Err($crate::TestFailure::from_arguments(::core::format_args!(
                "expected failure, but operation succeeded: {}",
                stringify!($result)
            ))
            .into());
        }
    }};
    ($result:expr, failure = $failure:expr $(,)?) => {{
        let result = &$result;
        if result.is_ok() {
            return Err($failure.into());
        }
    }};
}

#[macro_export]
macro_rules! ensure_some {
    ($value:expr $(,)?) => {{
        let value = &$value;
        if value.is_none() {
            return Err($crate::TestFailure::from_arguments(::core::format_args!(
                "expected a value: {}",
                stringify!($value)
            ))
            .into());
        }
    }};
    ($value:expr, failure = $failure:expr $(,)?) => {{
        let value = &$value;
        if value.is_none() {
            return Err($failure.into());
        }
    }};
}

#[macro_export]
macro_rules! ensure_none {
    ($value:expr $(,)?) => {{
        let value = &$value;
        if value.is_some() {
            return Err($crate::TestFailure::from_arguments(::core::format_args!(
                "expected an empty value: {}",
                stringify!($value)
            ))
            .into());
        }
    }};
    ($value:expr, failure = $failure:expr $(,)?) => {{
        let value = &$value;
        if value.is_some() {
            return Err($failure.into());
        }
    }};
}

#[macro_export]
macro_rules! check {
    ($constructor:expr; $condition:expr $(, $($message:tt)+)? $(,)?) => {
        if !$condition {
            return Err(($constructor)(stringify!($condition)).into());
        }
    };
    ($condition:expr $(,)?) => {
        $crate::ensure!($condition)
    };
    ($condition:expr, failure = $constructor:expr $(, $($message:tt)+)? $(,)?) => {
        if !$condition {
            return Err(($constructor)(stringify!($condition)).into());
        }
    };
}

#[macro_export]
macro_rules! check_eq {
    ($constructor:expr; $left:expr, $right:expr $(,)?) => {{
        match (&$left, &$right) {
            (left, right) if left == right => {}
            _ => return Err(($constructor)(stringify!($left)).into()),
        }
    }};
    ($left:expr, $right:expr $(,)?) => {
        $crate::ensure_eq!($left, $right)
    };
    ($left:expr, $right:expr, failure = $constructor:expr $(,)?) => {{
        match (&$left, &$right) {
            (left, right) if left == right => {}
            _ => return Err(($constructor)(stringify!($left)).into()),
        }
    }};
}

#[cfg(test)]
mod tests {
    use super::{TestFailure, TestResult};
    use crate::TestOpeoOutcome;

    fn shared_macros_accept_test_results() -> TestResult {
        ensure!(true);
        ensure_eq!(1, 1);
        ensure_ne!(1, 2);
        ensure_ok!(Ok::<(), TestFailure>(()));
        ensure_err!(Err::<(), TestFailure>(TestFailure::new("expected")));
        ensure_some!(Some(1));
        ensure_none!(None::<()>);
        check!(true);
        check_eq!(1, 1);
        Ok(())
    }

    fn custom_failure() -> Result<(), &'static str> {
        ensure!(false, failure = "expected failure");
        Ok(())
    }

    fn failure_constructor(_: &'static str) -> &'static str {
        "expected custom check failure"
    }

    fn custom_check_failure() -> Result<(), &'static str> {
        check!(false, failure = failure_constructor);
        Ok(())
    }

    fn custom_check_prefix_failure() -> Result<(), &'static str> {
        check!(failure_constructor; false);
        Ok(())
    }

    fn custom_check_eq_failure() -> Result<(), &'static str> {
        check_eq!(1, 2, failure = failure_constructor);
        Ok(())
    }

    fn custom_check_eq_prefix_failure() -> Result<(), &'static str> {
        check_eq!(failure_constructor; 1, 2);
        Ok(())
    }

    #[test]
    fn shared_macros_preserve_result_types() -> TestResult {
        shared_macros_accept_test_results()?;
        let mapped = TestOpeoOutcome::<_, TestFailure>::Value(4).map(|value| value + 1);
        match mapped {
            TestOpeoOutcome::Value(5) => {}
            _ => return Err(TestFailure::new("OPEO outcome mapping changed")),
        }
        match custom_failure() {
            Err("expected failure") => {}
            _ => return Err(TestFailure::new("custom ensure conversion changed")),
        }
        match custom_check_failure() {
            Err("expected custom check failure") => {}
            _ => return Err(TestFailure::new("custom check conversion changed")),
        }
        match custom_check_prefix_failure() {
            Err("expected custom check failure") => {}
            _ => return Err(TestFailure::new("custom check prefix conversion changed")),
        }
        match custom_check_eq_failure() {
            Err("expected custom check failure") => {}
            _ => return Err(TestFailure::new("custom equality conversion changed")),
        }
        match custom_check_eq_prefix_failure() {
            Err("expected custom check failure") => Ok(()),
            _ => Err(TestFailure::new(
                "custom equality prefix conversion changed",
            )),
        }
    }
}
