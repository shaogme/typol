use std::{error::Error, io::Error as IoError};
use typol_core::{
    BudgetError, ChargedString, ChargedVec, DiagnosticContext, ExecutionBudget, ExecutionLimits,
    diagnostic_context,
};
use typol_opeo_test_support::TestOpeo;

static MISSING_TEST_ITEM: DiagnosticContext = diagnostic_context!(
    "typol-core-tests",
    "evaluation-storage",
    "expected-item-missing",
);

macro_rules! ensure {
    ($condition:expr $(,)?) => {
        if !$condition {
            return Err(IoError::other(concat!("assertion failed: ", stringify!($condition))).into());
        }
    };
    ($condition:expr, $($argument:tt)+) => {
        if !$condition {
            return Err(IoError::other(format!($($argument)+)).into());
        }
    };
}

macro_rules! ensure_eq {
    ($left:expr, $right:expr $(,)?) => {{
        let left = &$left;
        let right = &$right;
        if left != right {
            return Err(IoError::other(format!(
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
            return Err(IoError::other(format!($($argument)+)).into());
        }
    }};
}

#[test]
fn metered_storage_keeps_exact_capacity_through_iteration_and_drop() -> Result<(), Box<dyn Error>> {
    let budget =
        TestOpeo::call(|out| ExecutionBudget::new(ExecutionLimits::new(10_000, 10_000), out))?;
    let initial = budget.workspace_bytes();
    let mut values = TestOpeo::call::<BudgetError, _>(|out| ChargedVec::new(Some(&budget), out))?;
    TestOpeo::call::<BudgetError, _>(|out| values.try_reserve_exact(8, out))?;
    TestOpeo::call::<BudgetError, _>(|out| values.resize(8, 7u64, out))?;
    ensure_eq!(budget.workspace_bytes() - initial, size_of::<[u64; 8]>());
    let mut values = values.into_iter();
    ensure_eq!(values.next(), Some(7));
    ensure_eq!(budget.workspace_bytes() - initial, size_of::<[u64; 8]>());
    drop(values);
    ensure_eq!(budget.workspace_bytes(), initial);
    Ok(())
}

#[test]
fn sorting_propagates_exhaustion_and_releases_storage_after_failure() -> Result<(), Box<dyn Error>>
{
    let budget = TestOpeo::call(|out| ExecutionBudget::new(ExecutionLimits::new(5, 10_000), out))?;
    let initial = budget.workspace_bytes();
    let mut values = TestOpeo::call::<BudgetError, _>(|out| ChargedVec::new(Some(&budget), out))?;
    TestOpeo::call::<BudgetError, _>(|out| values.try_reserve_exact(4, out))?;
    for value in [4, 3, 2, 1] {
        TestOpeo::call::<BudgetError, _>(|out| values.try_push(value, out))?;
    }
    ensure_eq!(
        TestOpeo::call::<BudgetError, _>(|out| values.sort_by(|a, b, _out| Ok(a.cmp(b)), out)),
        Err(BudgetError::Resource)
    );
    ensure!(budget.exhausted());
    drop(values);
    ensure_eq!(budget.workspace_bytes(), initial);
    Ok(())
}

#[test]
fn capacity_is_rejected_before_growth_and_old_items_survive() -> Result<(), Box<dyn Error>> {
    let budget =
        TestOpeo::call(|out| ExecutionBudget::new(ExecutionLimits::new(10_000, 128), out))?;
    let initial = budget.workspace_bytes();
    let mut values = TestOpeo::call::<BudgetError, _>(|out| ChargedVec::new(Some(&budget), out))?;
    TestOpeo::call::<BudgetError, _>(|out| values.try_reserve_exact(4, out))?;
    TestOpeo::call::<BudgetError, _>(|out| values.resize(4, 3u64, out))?;
    ensure_eq!(
        TestOpeo::call::<BudgetError, _>(|out| values.try_reserve_exact(8, out)),
        Err(BudgetError::Resource)
    );
    ensure_eq!(&*values, &[3, 3, 3, 3]);
    ensure!(budget.peak_workspace_bytes() <= 128);
    drop(values);
    ensure_eq!(budget.workspace_bytes(), initial);
    Ok(())
}

#[test]
fn standard_container_credentials_cover_exact_blocks_and_growth_overlap()
-> Result<(), Box<dyn Error>> {
    let budget =
        TestOpeo::call(|out| ExecutionBudget::new(ExecutionLimits::new(10_000, 10_000), out))?;
    let initial = budget.workspace_bytes();
    let mut values = TestOpeo::call::<BudgetError, _>(|out| ChargedVec::new(Some(&budget), out))?;
    TestOpeo::call::<BudgetError, _>(|out| values.try_reserve_exact(2, out))?;
    TestOpeo::call::<BudgetError, _>(|out| values.try_push(1u64, out))?;
    TestOpeo::call::<BudgetError, _>(|out| values.try_push(2u64, out))?;
    TestOpeo::call::<BudgetError, _>(|out| values.try_reserve_exact(3, out))?;
    ensure_eq!(values.capacity(), 5);
    ensure_eq!(budget.workspace_bytes(), initial + 40);
    ensure_eq!(budget.peak_workspace_bytes(), initial + 16 + 40);
    let text =
        TestOpeo::call::<BudgetError, _>(|out| ChargedString::copy("€€🙂", Some(&budget), out))?;
    ensure_eq!(text.capacity(), 10);
    ensure_eq!(budget.workspace_bytes(), initial + 50);
    drop(values);
    drop(text);
    ensure_eq!(budget.workspace_bytes(), initial);
    Ok(())
}

#[test]
fn standard_container_sort_charges_moves_even_when_comparison_is_free() -> Result<(), Box<dyn Error>>
{
    let budget = TestOpeo::call(|out| ExecutionBudget::new(ExecutionLimits::new(5, 10_000), out))?;
    let initial = budget.workspace_bytes();
    let mut values = TestOpeo::call::<BudgetError, _>(|out| ChargedVec::new(Some(&budget), out))?;
    TestOpeo::call::<BudgetError, _>(|out| values.try_reserve_exact(4, out))?;
    for value in [4u64, 3, 2, 1] {
        TestOpeo::call::<BudgetError, _>(|out| values.try_push(value, out))?;
    }
    ensure_eq!(
        TestOpeo::call::<BudgetError, _>(|out| values.sort_by(|a, b, _out| Ok(a.cmp(b)), out)),
        Err(BudgetError::Resource)
    );
    ensure!(budget.exhausted());
    drop(values);
    ensure_eq!(budget.workspace_bytes(), initial);
    Ok(())
}

#[test]
fn standard_container_exact_blocks_handle_alignment_and_zero_size() -> Result<(), Box<dyn Error>> {
    #[repr(align(64))]
    struct Aligned(u64);

    let budget =
        TestOpeo::call(|out| ExecutionBudget::new(ExecutionLimits::new(10_000, 10_000), out))?;
    let initial = budget.workspace_bytes();
    let mut values = TestOpeo::call::<BudgetError, _>(|out| ChargedVec::new(Some(&budget), out))?;
    TestOpeo::call::<BudgetError, _>(|out| values.try_reserve_exact(2, out))?;
    TestOpeo::call::<BudgetError, _>(|out| values.try_push(Aligned(7), out))?;
    ensure_eq!(
        values
            .first()
            .ok_or_else(|| BudgetError::invariant(&MISSING_TEST_ITEM))?
            .0,
        7
    );
    ensure_eq!(values.as_ptr() as usize % 64, 0);
    ensure_eq!(budget.workspace_bytes(), initial + 128);
    let mut empty = TestOpeo::call::<BudgetError, _>(|out| ChargedVec::new(Some(&budget), out))?;
    TestOpeo::call::<BudgetError, _>(|out| empty.try_reserve_exact(8, out))?;
    for _ in 0..8 {
        TestOpeo::call::<BudgetError, _>(|out| empty.try_push((), out))?;
    }
    ensure_eq!(empty.len(), 8);
    ensure_eq!(budget.workspace_bytes(), initial + 128);
    drop(empty);
    drop(values);
    ensure_eq!(budget.workspace_bytes(), initial);
    Ok(())
}

#[test]
fn unmetered_and_budgeted_vectors_share_the_same_sorting_path() -> Result<(), Box<dyn Error>> {
    let mut values = ChargedVec::unmetered();
    for value in [4, 1, 3, 2] {
        TestOpeo::call::<BudgetError, _>(|out| values.try_push(value, out))?;
    }
    TestOpeo::call::<BudgetError, _>(|out| {
        values.sort_by(|left, right, _out| Ok(left.cmp(right)), out)
    })?;
    ensure_eq!(values.as_slice(), [1, 2, 3, 4]);
    Ok(())
}
