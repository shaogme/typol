use std::sync::{Mutex, PoisonError};

use typol_core::{BudgetError, ExecutionBudget, ExecutionLimits};
use typol_opeo_test_support::TestSlot;
use typol_regex::opeo::{OResult, Out};

pub(crate) static TEST_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum Attempt<T, E> {
    Success(T),
    Failure(E),
}

impl<T, E> Attempt<T, E> {
    pub(crate) fn as_ref(&self) -> Attempt<&T, &E> {
        match self {
            Self::Success(value) => Attempt::Success(value),
            Self::Failure(error) => Attempt::Failure(error),
        }
    }

    pub(crate) fn is_success(&self) -> bool {
        matches!(self.as_ref(), Attempt::Success(_))
    }

    pub(crate) fn is_failure(&self) -> bool {
        matches!(self, Self::Failure(_))
    }

    pub(crate) fn into_option(self) -> Option<T> {
        match self {
            Self::Success(value) => Some(value),
            Self::Failure(_) => None,
        }
    }

    pub(crate) fn into_failure(self) -> Option<E> {
        match self {
            Self::Success(_) => None,
            Self::Failure(error) => Some(error),
        }
    }

    pub(crate) fn failure_matches(&self, predicate: impl FnOnce(&E) -> bool) -> bool {
        match self {
            Self::Success(_) => false,
            Self::Failure(error) => predicate(error),
        }
    }

    pub(crate) fn map_success<U>(self, transform: impl FnOnce(T) -> U) -> Attempt<U, E> {
        match self {
            Self::Success(value) => Attempt::Success(transform(value)),
            Self::Failure(error) => Attempt::Failure(error),
        }
    }

    pub(crate) fn map_failure<F>(self, transform: impl FnOnce(E) -> F) -> Attempt<T, F> {
        match self {
            Self::Success(value) => Attempt::Success(value),
            Self::Failure(error) => Attempt::Failure(transform(error)),
        }
    }
}

pub(crate) struct Clippy;

impl Clippy {
    pub(crate) fn attempt<T, E>(
        errors: &mut TestSlot<E>,
        operation: impl for<'slot> FnOnce(Out<'slot, 'slot, E>) -> OResult<'slot, T>,
    ) -> Attempt<T, E> {
        match errors.access(operation) {
            Ok(value) => Attempt::Success(value),
            Err(error) => Attempt::Failure(error),
        }
    }

    pub(crate) fn execution_budget(
        errors: &mut TestSlot<BudgetError>,
        limits: ExecutionLimits,
    ) -> Attempt<ExecutionBudget, BudgetError> {
        Self::attempt(errors, |out| ExecutionBudget::new(limits, out))
    }

    pub(crate) fn succeed_opeo<T, E>(attempt: Attempt<T, E>, message: &str) -> Option<T> {
        assert!(attempt.is_success(), "{message}");
        attempt.into_option()
    }

    pub(crate) fn failure_opeo<T, E>(attempt: Attempt<T, E>, message: &str) -> Option<E> {
        assert!(attempt.is_failure(), "{message}");
        attempt.into_failure()
    }
}

#[cfg(test)]
mod tests {
    use super::{Attempt, Clippy, PoisonError, TEST_LOCK};

    #[test]
    fn attempt_helpers_preserve_success_and_failure_values() {
        let _test_guard = TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let success = Attempt::<usize, &str>::Success(4);
        assert!(success.is_success());
        assert_eq!(success.as_ref(), Attempt::Success(&4));
        assert_eq!(
            Clippy::succeed_opeo(success, "success result should be preserved"),
            Some(4)
        );
        assert_eq!(
            Attempt::<usize, &str>::Success(4)
                .map_success(|value| value + 1)
                .into_option(),
            Some(5)
        );

        let failure = Attempt::<usize, &str>::Failure("expected failure");
        assert!(failure.is_failure());
        assert!(failure.failure_matches(|error| *error == "expected failure"));
        assert_eq!(
            failure.map_failure(str::to_owned).into_failure(),
            Some(String::from("expected failure"))
        );
        assert_eq!(
            Clippy::failure_opeo(
                Attempt::<usize, &str>::Failure("expected failure"),
                "failure result should be preserved",
            ),
            Some("expected failure")
        );
    }
}
