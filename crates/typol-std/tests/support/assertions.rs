use std::io;

pub struct AssertionError;

impl AssertionError {
    pub fn from_message(message: impl Into<String>) -> io::Error {
        io::Error::other(message.into())
    }
}

#[macro_export]
macro_rules! ensure {
    ($condition:expr $(,)?) => {
        if !$condition {
            return Err($crate::assertions::AssertionError::from_message(concat!(
                "assertion failed: ",
                stringify!($condition)
            ))
            .into());
        }
    };
    ($condition:expr, $($argument:tt)+) => {
        if !$condition {
            return Err($crate::assertions::AssertionError::from_message(format!($($argument)+)).into());
        }
    };
}

#[macro_export]
macro_rules! ensure_eq {
    ($left:expr, $right:expr $(,)?) => {{
        let left = &$left;
        let right = &$right;
        if left != right {
            return Err($crate::assertions::AssertionError::from_message(format!(
                "assertion failed: {} and {} differ (actual {:?}, expected {:?})",
                stringify!($left),
                stringify!($right),
                left,
                right,
            ))
            .into());
        }
    }};
    ($left:expr, $right:expr, $($argument:tt)+) => {{
        let left = &$left;
        let right = &$right;
        if left != right {
            return Err($crate::assertions::AssertionError::from_message(format!($($argument)+)).into());
        }
    }};
}

#[macro_export]
macro_rules! ensure_ne {
    ($left:expr, $right:expr $(,)?) => {
        ensure!($left != $right)
    };
}

#[macro_export]
macro_rules! ensure_ok {
    ($result:expr $(,)?) => {{
        let result = &$result;
        if let Err(error) = result {
            return Err($crate::assertions::AssertionError::from_message(format!(
                "expected success, but operation returned an error: {error:?}"
            ))
            .into());
        }
    }};
}

#[macro_export]
macro_rules! ensure_err {
    ($result:expr $(,)?) => {{
        let result = &$result;
        if result.is_ok() {
            return Err($crate::assertions::AssertionError::from_message(concat!(
                "expected failure, but operation succeeded: ",
                stringify!($result)
            ))
            .into());
        }
    }};
}

#[macro_export]
macro_rules! ensure_some {
    ($value:expr $(,)?) => {{
        if ($value).is_none() {
            return Err($crate::assertions::AssertionError::from_message(concat!(
                "expected a value: ",
                stringify!($value)
            ))
            .into());
        }
    }};
}

#[macro_export]
macro_rules! ensure_none {
    ($value:expr $(,)?) => {{
        if ($value).is_some() {
            return Err($crate::assertions::AssertionError::from_message(concat!(
                "expected an empty value: ",
                stringify!($value)
            ))
            .into());
        }
    }};
}
