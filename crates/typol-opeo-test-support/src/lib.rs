#![no_std]

extern crate alloc;

use alloc::{fmt::format, format, string::String};
use core::fmt::{self, Arguments, Debug, Display, Formatter};
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
