use std::{
    cell::Cell,
    error::Error,
    io::Error as IoError,
    mem::{align_of, size_of},
    panic::{AssertUnwindSafe, catch_unwind},
    rc::Rc,
    thread,
};
use typol_opeo_test_support::TestOpeo;

use typol_core::{
    BudgetError, ChargedString, ChargedVec, DiagnosticContext, ExecutionBudget, ExecutionLimits,
    TerminalSlot, diagnostic_context,
};

static MISSING_TEST_ITEM: DiagnosticContext = diagnostic_context!(
    "typol-core-tests",
    "budget-storage",
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
fn budget() -> Result<ExecutionBudget, BudgetError> {
    TestOpeo::call(|out| ExecutionBudget::new(ExecutionLimits::new(1_000_000, 1_048_576), out))
}

#[test]
fn exact_growth_accounts_both_blocks_and_move_keeps_ownership() -> Result<(), Box<dyn Error>> {
    let budget = budget()?;
    let initial = budget.workspace_bytes();
    let mut values =
        TestOpeo::call::<BudgetError, _>(|out| ChargedVec::<u64>::new(Some(&budget), out))?;
    TestOpeo::call::<BudgetError, _>(|out| values.try_reserve_exact(2, out))?;
    TestOpeo::call::<BudgetError, _>(|out| values.try_push(11, out))?;
    TestOpeo::call::<BudgetError, _>(|out| values.try_push(12, out))?;
    TestOpeo::call::<BudgetError, _>(|out| values.try_reserve_exact(1, out))?;
    ensure_eq!(values.capacity(), 3);
    ensure_eq!(values.as_slice(), [11, 12]);
    ensure_eq!(budget.workspace_bytes(), initial + 3 * size_of::<u64>());
    ensure_eq!(
        budget.peak_workspace_bytes(),
        initial + 5 * size_of::<u64>()
    );
    let mut moved = values;
    moved.clear();
    ensure_eq!(budget.workspace_bytes(), initial + 3 * size_of::<u64>());
    drop(moved);
    ensure_eq!(budget.workspace_bytes(), initial);
    Ok(())
}

#[test]
fn capacity_rejection_preserves_old_elements_and_exhaustion_allows_cleanup()
-> Result<(), Box<dyn Error>> {
    let initial = budget()?.workspace_bytes();
    let budget =
        TestOpeo::call(|out| ExecutionBudget::new(ExecutionLimits::new(100, initial + 23), out))?;
    let mut values =
        TestOpeo::call::<BudgetError, _>(|out| ChargedVec::<u64>::new(Some(&budget), out))?;
    TestOpeo::call::<BudgetError, _>(|out| values.try_push(42, out))?;
    ensure_eq!(
        TestOpeo::call::<BudgetError, _>(|out| values.try_reserve_exact(1, out)),
        Err(BudgetError::Resource)
    );
    ensure_eq!(values.capacity(), 1);
    ensure_eq!(values.as_slice(), [42]);
    ensure!(budget.exhausted());
    ensure_eq!(values.pop(), Some(42));
    values.clear();
    ensure_eq!(budget.workspace_bytes(), initial + 8);
    drop(values);
    ensure_eq!(budget.workspace_bytes(), initial);
    Ok(())
}

#[test]
fn nested_storage_releases_inner_owners_without_releasing_outer_capacity()
-> Result<(), Box<dyn Error>> {
    let budget = budget()?;
    let initial = budget.workspace_bytes();
    let mut values = TestOpeo::call::<BudgetError, _>(|out| ChargedVec::new(Some(&budget), out))?;
    TestOpeo::call::<BudgetError, _>(|out| values.try_reserve_exact(2, out))?;
    let chinese =
        TestOpeo::call::<BudgetError, _>(|out| ChargedString::copy("€€", Some(&budget), out))?;
    TestOpeo::call::<BudgetError, _>(|out| values.try_push(chinese, out))?;
    let emoji =
        TestOpeo::call::<BudgetError, _>(|out| ChargedString::copy("🙂", Some(&budget), out))?;
    TestOpeo::call::<BudgetError, _>(|out| values.try_push(emoji, out))?;
    let outer = 2 * size_of::<ChargedString>();
    ensure_eq!(budget.workspace_bytes(), initial + outer + 10);
    let text = values
        .pop()
        .ok_or_else(|| BudgetError::invariant(&MISSING_TEST_ITEM))?;
    ensure_eq!(budget.workspace_bytes(), initial + outer + 10);
    drop(text);
    ensure_eq!(budget.workspace_bytes(), initial + outer + 6);
    values.clear();
    ensure_eq!(budget.workspace_bytes(), initial + outer);
    drop(values);
    ensure_eq!(budget.workspace_bytes(), initial);
    Ok(())
}

#[test]
fn text_growth_preserves_utf8_and_charges_each_byte_and_move() -> Result<(), Box<dyn Error>> {
    let budget = budget()?;
    let initial = budget.workspace_bytes();
    let mut text =
        TestOpeo::call::<BudgetError, _>(|out| ChargedString::copy("€", Some(&budget), out))?;
    ensure_eq!(budget.steps(), 3);
    TestOpeo::call::<BudgetError, _>(|out| text.try_push_char('🙂', out))?;
    ensure_eq!(text.as_str(), "€🙂");
    ensure_eq!(text.capacity(), 7);
    ensure_eq!(budget.steps(), 10);
    ensure_eq!(budget.workspace_bytes(), initial + 7);
    ensure_eq!(budget.peak_workspace_bytes(), initial + 10);
    drop(text);
    ensure_eq!(budget.workspace_bytes(), initial);
    Ok(())
}

#[test]
fn step_rejection_happens_before_growth_and_keeps_text_valid() -> Result<(), Box<dyn Error>> {
    let budget =
        TestOpeo::call(|out| ExecutionBudget::new(ExecutionLimits::new(3, 1_048_576), out))?;
    let mut text =
        TestOpeo::call::<BudgetError, _>(|out| ChargedString::copy("€", Some(&budget), out))?;
    let held = budget.workspace_bytes();
    ensure_eq!(
        TestOpeo::call::<BudgetError, _>(|out| text.try_push_char('🙂', out)),
        Err(BudgetError::Resource)
    );
    ensure_eq!(text.as_str(), "€");
    ensure_eq!(text.capacity(), 3);
    ensure_eq!(budget.workspace_bytes(), held);
    ensure_eq!(budget.peak_workspace_bytes(), held);
    Ok(())
}

#[repr(align(256))]
struct Aligned(u8);

#[test]
fn exact_layout_preserves_large_alignment() -> Result<(), Box<dyn Error>> {
    let budget = budget()?;
    let initial = budget.workspace_bytes();
    let mut values = TestOpeo::call::<BudgetError, _>(|out| ChargedVec::new(Some(&budget), out))?;
    TestOpeo::call::<BudgetError, _>(|out| values.try_push(Aligned(7), out))?;
    ensure_eq!(values.as_ptr() as usize % align_of::<Aligned>(), 0);
    let first = values
        .first()
        .ok_or_else(|| BudgetError::invariant(&MISSING_TEST_ITEM))?;
    ensure_eq!(first.0, 7);
    ensure_eq!(budget.workspace_bytes(), initial + size_of::<Aligned>());
    Ok(())
}

struct Zero;

thread_local! {
    static ZERO_DROPS: Cell<usize> = const { Cell::new(0) };
}

impl Drop for Zero {
    fn drop(&mut self) {
        ZERO_DROPS.with(|count| count.set(count.get() + 1));
    }
}

#[test]
fn zero_size_elements_keep_logical_capacity_and_drop_exactly_once() -> Result<(), Box<dyn Error>> {
    ZERO_DROPS.with(|count| count.set(0));
    let budget = budget()?;
    let initial = budget.workspace_bytes();
    let mut values = TestOpeo::call::<BudgetError, _>(|out| ChargedVec::new(Some(&budget), out))?;
    for _ in 0..5 {
        TestOpeo::call::<BudgetError, _>(|out| values.try_push(Zero, out))?;
    }
    ensure_eq!(values.len(), 5);
    ensure_eq!(values.capacity(), usize::MAX);
    ensure_eq!(budget.workspace_bytes(), initial);
    drop(values.pop());
    values.truncate(1);
    drop(values);
    let drops = ZERO_DROPS.with(|count| count.get());
    ensure_eq!(drops, 5);
    Ok(())
}

#[test]
fn capacity_overflow_is_rejected_before_allocating_or_initializing() -> Result<(), Box<dyn Error>> {
    let budget = budget()?;
    let initial = budget.workspace_bytes();
    let steps = budget.steps();
    let peak = budget.peak_workspace_bytes();
    let mut values =
        TestOpeo::call::<BudgetError, _>(|out| ChargedVec::<u64>::new(Some(&budget), out))?;
    ensure_eq!(
        TestOpeo::call::<BudgetError, _>(|out| values.try_reserve_exact(usize::MAX, out)),
        Err(BudgetError::CapacityExceeded)
    );
    ensure_eq!(budget.steps(), steps);
    ensure_eq!(budget.peak_workspace_bytes(), peak);
    ensure_eq!(budget.failure(), None);
    ensure!(!budget.exhausted());
    ensure_eq!(budget.workspace_bytes(), initial);
    ensure_eq!(values.capacity(), 0);
    Ok(())
}

struct PanickingDrop {
    count: Rc<Cell<usize>>,
    panic: bool,
}

impl Drop for PanickingDrop {
    fn drop(&mut self) {
        self.count.set(self.count.get() + 1);
        assert!(!self.panic, "simulated single-element drop failure");
    }
}

#[test]
fn destructor_unwind_releases_remaining_elements_and_exact_block() -> Result<(), Box<dyn Error>> {
    let budget = budget()?;
    let initial = budget.workspace_bytes();
    let count = Rc::new(Cell::new(0));
    let mut values = TestOpeo::call::<BudgetError, _>(|out| ChargedVec::new(Some(&budget), out))?;
    for panic in [true, false, false] {
        TestOpeo::call::<BudgetError, _>(|out| {
            values.try_push(
                PanickingDrop {
                    count: count.clone(),
                    panic,
                },
                out,
            )
        })?;
    }
    ensure!(catch_unwind(AssertUnwindSafe(|| drop(values))).is_err());
    ensure_eq!(count.get(), 3);
    ensure_eq!(budget.workspace_bytes(), initial);
    Ok(())
}

#[test]
fn initialized_prefix_survives_truncate_unwind() -> Result<(), Box<dyn Error>> {
    let budget = budget()?;
    let initial = budget.workspace_bytes();
    let count = Rc::new(Cell::new(0));
    let mut values = TestOpeo::call::<BudgetError, _>(|out| ChargedVec::new(Some(&budget), out))?;
    for panic in [false, true, false] {
        TestOpeo::call::<BudgetError, _>(|out| {
            values.try_push(
                PanickingDrop {
                    count: count.clone(),
                    panic,
                },
                out,
            )
        })?;
    }
    ensure!(catch_unwind(AssertUnwindSafe(|| values.truncate(1))).is_err());
    ensure_eq!(count.get(), 2);
    ensure_eq!(values.len(), 1);
    drop(values);
    ensure_eq!(count.get(), 3);
    ensure_eq!(budget.workspace_bytes(), initial);
    Ok(())
}

#[test]
fn storage_preserves_send_and_sync_for_thread_safe_elements() -> Result<(), Box<dyn Error>> {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ChargedVec<ChargedString>>();
    let budget = budget()?;
    let initial = budget.workspace_bytes();
    let text =
        TestOpeo::call::<BudgetError, _>(|out| ChargedString::copy("€thread", Some(&budget), out))?;
    let matches = thread::spawn(move || text.as_str() == "€thread")
        .join()
        .map_err(|_| IoError::other("storage thread exited unexpectedly"))?;
    ensure!(matches);
    ensure_eq!(budget.workspace_bytes(), initial);
    Ok(())
}

#[test]
fn zero_size_capacity_growth_overflow_preserves_budget_state() -> Result<(), Box<dyn Error>> {
    ZERO_DROPS.with(|count| count.set(0));
    let budget = budget()?;
    let initial = budget.workspace_bytes();
    let mut values =
        TestOpeo::call::<BudgetError, _>(|out| ChargedVec::<Zero>::new(Some(&budget), out))?;
    TestOpeo::call::<BudgetError, _>(|out| values.try_push(Zero, out))?;
    let steps = budget.steps();
    let peak = budget.peak_workspace_bytes();
    ensure_eq!(
        TestOpeo::call::<BudgetError, _>(|out| values.try_reserve(usize::MAX, out)),
        Err(BudgetError::CapacityExceeded)
    );
    ensure_eq!(values.capacity(), usize::MAX);
    ensure_eq!(values.len(), 1);
    ensure_eq!(budget.steps(), steps);
    ensure_eq!(budget.peak_workspace_bytes(), peak);
    ensure_eq!(budget.failure(), None);
    ensure!(!budget.exhausted());
    drop(values);
    ensure_eq!(ZERO_DROPS.with(|count| count.get()), 1);
    ensure_eq!(budget.workspace_bytes(), initial);
    Ok(())
}

#[test]
fn owning_iteration_keeps_capacity_and_drops_only_remaining_elements() -> Result<(), Box<dyn Error>>
{
    let budget = budget()?;
    let initial = budget.workspace_bytes();
    let mut values = TestOpeo::call::<BudgetError, _>(|out| ChargedVec::new(Some(&budget), out))?;
    for text in ["€", "₽", "₿"] {
        let value =
            TestOpeo::call::<BudgetError, _>(|out| ChargedString::copy(text, Some(&budget), out))?;
        TestOpeo::call::<BudgetError, _>(|out| values.try_push(value, out))?;
    }
    let outer = values.capacity() * size_of::<ChargedString>();
    let mut iter = values.into_iter();
    let first = iter
        .next()
        .ok_or_else(|| BudgetError::invariant(&MISSING_TEST_ITEM))?;
    ensure_eq!(first.as_str(), "€");
    ensure_eq!(iter.len(), 2);
    ensure_eq!(budget.workspace_bytes(), initial + outer + 9);
    drop(iter);
    ensure_eq!(budget.workspace_bytes(), initial + 3);
    drop(first);
    ensure_eq!(budget.workspace_bytes(), initial);
    Ok(())
}

#[test]
fn owning_iterator_unwind_and_zero_size_cleanup_drop_every_element_once()
-> Result<(), Box<dyn Error>> {
    let budget = budget()?;
    let initial = budget.workspace_bytes();
    let count = Rc::new(Cell::new(0));
    let mut values = TestOpeo::call::<BudgetError, _>(|out| ChargedVec::new(Some(&budget), out))?;
    for panic in [false, true, false] {
        TestOpeo::call::<BudgetError, _>(|out| {
            values.try_push(
                PanickingDrop {
                    count: count.clone(),
                    panic,
                },
                out,
            )
        })?;
    }
    let mut iter = values.into_iter();
    drop(iter.next());
    ensure!(catch_unwind(AssertUnwindSafe(|| drop(iter))).is_err());
    ensure_eq!(count.get(), 3);
    ensure_eq!(budget.workspace_bytes(), initial);
    ZERO_DROPS.with(|count| count.set(0));
    let mut values = TestOpeo::call::<BudgetError, _>(|out| ChargedVec::new(Some(&budget), out))?;
    for _ in 0..3 {
        TestOpeo::call::<BudgetError, _>(|out| values.try_push(Zero, out))?;
    }
    let mut iter = values.into_iter();
    drop(iter.next());
    drop(iter);
    let drops = ZERO_DROPS.with(|count| count.get());
    ensure_eq!(drops, 3);
    ensure_eq!(budget.workspace_bytes(), initial);
    Ok(())
}

#[test]
fn terminal_slot_keeps_ownership_after_exhaustion_and_drops_once() -> Result<(), Box<dyn Error>> {
    let budget = budget()?;
    let initial = budget.workspace_bytes();
    let slot = TestOpeo::call::<BudgetError, _>(|out| TerminalSlot::new(&budget, out))?;
    let value =
        TestOpeo::call::<BudgetError, _>(|out| ChargedString::copy("€end", Some(&budget), out))?;
    let retained = budget.workspace_bytes();
    ensure_eq!(
        TestOpeo::call::<BudgetError, _>(|out| budget.consume(usize::MAX, out)),
        Err(BudgetError::Resource)
    );
    let slot = slot
        .store(value)
        .ok_or_else(|| IoError::other("terminal slot reserve capacity was lost"))?;
    ensure_eq!(
        slot.get()
            .ok_or_else(|| IoError::other("terminal slot contents are missing"))?
            .as_str(),
        "€end"
    );
    ensure_eq!(budget.workspace_bytes(), retained);
    drop(slot);
    ensure_eq!(budget.workspace_bytes(), initial);
    Ok(())
}

#[test]
fn unwritten_terminal_slot_and_zero_size_value_release_cleanly() -> Result<(), Box<dyn Error>> {
    let budget = budget()?;
    let initial = budget.workspace_bytes();
    drop(TestOpeo::call::<BudgetError, _>(|out| {
        TerminalSlot::<u64>::new(&budget, out)
    })?);
    ensure_eq!(budget.workspace_bytes(), initial);
    ZERO_DROPS.with(|count| count.set(0));
    let slot = TestOpeo::call::<BudgetError, _>(|out| TerminalSlot::new(&budget, out))?;
    let slot = slot
        .store(Zero)
        .ok_or_else(|| IoError::other("terminal slot reserve capacity was lost"))?;
    drop(slot);
    let drops = ZERO_DROPS.with(|count| count.get());
    ensure_eq!(drops, 1);
    ensure_eq!(budget.workspace_bytes(), initial);
    Ok(())
}
