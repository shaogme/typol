use crate::{
    descriptor::{DescribeContext, DescriptorNode, DescriptorProvider, TypeDescriptor, TypeShape},
    diagnostics::{CodecErrors, CodecStage, ErrorBuffer, Failure},
    options::{BindOptions, BindingLimits},
    session::SessionBudget,
    test_support::{TestOpeo, TestOpeoOutcome},
};
use opeo::{OResult, Out};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    any::TypeId,
    cell::Cell,
    collections::BTreeMap,
    env::{current_exe, var_os},
    process::Command,
    ptr::null_mut,
    thread_local,
};
use typol_core::DiagnosticCode;
use typol_model::BuiltinType;
use typol_test_support::{
    TestExtract, TestResult, test_verify as assert, test_verify_eq as assert_eq,
};

pub(crate) struct TestAssertions;

fn scalar<'slot, 'borrow>(
    context: &mut DescribeContext<'_>,
    depth: usize,
    out: Out<'slot, 'borrow, CodecErrors>,
) -> OResult<'slot, TypeDescriptor> {
    context.scalar::<u8>(BuiltinType::U8, depth, out)
}

fn provider() -> DescriptorProvider {
    DescriptorProvider::new::<u8>(scalar)
}

#[derive(Clone, Copy)]
pub(crate) struct AllocationState {
    active: bool,
    fail_at: Option<usize>,
    persistent: bool,
    attempts: usize,
    live_bytes: isize,
    peak_bytes: isize,
}

impl AllocationState {
    #[cfg(feature = "serde")]
    pub(crate) const fn attempts(&self) -> usize {
        self.attempts
    }
}

thread_local! {
    static ALLOCATION: Cell<AllocationState> = const { Cell::new(AllocationState {
        active: false, fail_at: None, persistent: false, attempts: 0, live_bytes: 0, peak_bytes: 0,
    }) };
}

struct TestAllocator;

fn should_fail() -> bool {
    ALLOCATION
        .try_with(|state| {
            let mut current = state.get();
            if !current.active {
                return false;
            }
            let fail = current.fail_at.is_some_and(|at| {
                at == current.attempts || (current.persistent && at < current.attempts)
            });
            current.attempts += 1;
            state.set(current);
            fail
        })
        .unwrap_or(false)
}

fn account(bytes: isize) {
    let _ = ALLOCATION.try_with(|state| {
        let mut current = state.get();
        if current.active {
            current.live_bytes += bytes;
            current.peak_bytes = current.peak_bytes.max(current.live_bytes);
            state.set(current);
        }
    });
}

unsafe impl GlobalAlloc for TestAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if should_fail() {
            return null_mut();
        }
        // SAFETY: Passes the allocation layout unchanged to the system allocator and returns its managed storage.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            account(layout.size() as isize);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        account(-(layout.size() as isize));
        // SAFETY: The pointer and layout come from the corresponding system allocation.
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if should_fail() {
            return null_mut();
        }
        // SAFETY: Preserves the system allocator's original pointer and layout contract.
        let result = unsafe { System.realloc(pointer, layout, new_size) };
        if !result.is_null() {
            account(new_size as isize - layout.size() as isize);
        }
        result
    }
}

#[global_allocator]
static ALLOCATOR: TestAllocator = TestAllocator;

pub(crate) struct AllocationScope;

impl AllocationScope {
    pub(crate) fn start(fail_at: Option<usize>, persistent: bool) -> Self {
        ALLOCATION.with(|state| {
            state.set(AllocationState {
                active: true,
                fail_at,
                persistent,
                attempts: 0,
                live_bytes: 0,
                peak_bytes: 0,
            })
        });
        Self
    }

    pub(crate) fn finish(self) -> AllocationState {
        ALLOCATION.with(Cell::get)
    }
}

impl Drop for AllocationScope {
    fn drop(&mut self) {
        ALLOCATION.with(|state| {
            let mut current = state.get();
            current.active = false;
            state.set(current);
        });
    }
}

fn allocation_workload<'slot>(mut out: Out<'slot, '_, CodecErrors>) -> OResult<'slot, ()> {
    let call = match TestOpeo::outcome(|budget_out| {
        SessionBudget::binding(BindOptions::default(), budget_out)
    }) {
        TestOpeoOutcome::Value(call) => call,
        TestOpeoOutcome::Error(error) => {
            return Err(out.fail(CodecErrors::emergency(CodecStage::Binding, error.into())));
        }
    };
    let mut ctx = DescribeContext::new(BindingLimits::default(), &call, out.reborrow())?;
    let name = ctx.name("Root", out.reborrow())?;
    let field_name = ctx.name("field", out.reborrow())?;
    let field = ctx.insert(
        DescriptorNode::Field {
            name: field_name,
            value: provider(),
        },
        0,
        out.reborrow(),
    )?;
    let fields = ctx.members(&[field], out.reborrow())?;
    ctx.insert(
        DescriptorNode::Type {
            identity: TypeId::of::<()>(),
            name: Some(name),
            shape: TypeShape::Structure(fields),
        },
        0,
        out.reborrow(),
    )?;
    let mut errors = call.budget_codec(
        CodecStage::Binding,
        |budget_out| ErrorBuffer::new(CodecStage::Binding, 3, &call, budget_out),
        out.reborrow(),
    )?;
    errors.push(Failure::BindingMismatch);
    errors.push(Failure::BindingMismatch);
    errors.push(Failure::BindingMismatch);
    Err(out.fail(errors.finish()))
}

fn allocation_workload_result() -> TestOpeoOutcome<(), CodecErrors> {
    TestOpeo::outcome(allocation_workload)
}

#[test]
fn every_recoverable_allocation_failure_is_atomic_including_persistent_oom() -> TestResult {
    let scope = AllocationScope::start(None, false);
    drop(allocation_workload_result());
    let baseline = scope.finish();
    assert_eq!(baseline.live_bytes, 0);
    assert!(baseline.attempts > 5);
    for persistent in [false, true] {
        for fail_at in 0..baseline.attempts {
            let scope = AllocationScope::start(Some(fail_at), persistent);
            let result = allocation_workload_result();
            let code = match result {
                TestOpeoOutcome::Value(()) => None,
                TestOpeoOutcome::Error(errors) => errors
                    .diagnostics()
                    .last()
                    .map(|diagnostic| diagnostic.code()),
            };
            let actual = scope.finish();
            assert_eq!(
                code,
                Some(DiagnosticCode::ALLOCATION_FAILED),
                "failure point {fail_at}, persistent rejection {persistent}"
            );
            assert_eq!(
                actual.live_bytes, 0,
                "failure point {fail_at}, persistent rejection {persistent}"
            );
        }
    }
    Ok(())
}

impl TestAssertions {
    pub(crate) fn assert_atomic_allocations(workload: impl Fn() -> TestResult) -> TestResult {
        let scope = AllocationScope::start(None, false);
        workload()?;
        let baseline = scope.finish();
        assert_eq!(baseline.live_bytes, 0);
        assert!(baseline.attempts > 0);
        for persistent in [false, true] {
            for fail_at in 0..baseline.attempts {
                let scope = AllocationScope::start(Some(fail_at), persistent);
                workload()?;
                let actual = scope.finish();
                assert_eq!(
                    actual.live_bytes, 0,
                    "failure point {fail_at}, persistent rejection {persistent}"
                );
            }
        }
        Ok(())
    }
}

#[test]
fn btree_node_oom_is_an_isolated_host_boundary() -> TestResult {
    const CHILD: &str = "TYPOL_PHASE63_BTREE_OOM_CHILD";
    if var_os(CHILD).is_some() {
        let key = String::from("k");
        let mut target = BTreeMap::new();
        let _scope = AllocationScope::start(Some(0), true);
        target.insert(key, 1u8);
        return Ok(());
    }
    let output = Command::new(current_exe().test_value()?)
        .args([
            "--exact",
            "tests::allocation::btree_node_oom_is_an_isolated_host_boundary",
        ])
        .env(CHILD, "1")
        .output()
        .test_value()?;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("memory allocation"));
    Ok(())
}
