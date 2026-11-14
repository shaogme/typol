use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    ptr::null_mut,
};

#[derive(Clone, Copy)]
pub(super) struct State {
    pub(super) active: bool,
    fail_at: Option<usize>,
    persistent: bool,
    pub(super) attempts: usize,
    pub(super) live_bytes: isize,
    pub(super) largest_allocation: usize,
}

thread_local! {
    static STATE: Cell<State> = const { Cell::new(State {
        active: false, fail_at: None, persistent: false, attempts: 0, live_bytes: 0,
        largest_allocation: 0,
    }) };
}

struct CheckedAllocator;

fn should_fail() -> bool {
    STATE
        .try_with(|state| {
            let mut current = state.get();
            if !current.active {
                return false;
            }
            let fail = current.fail_at.is_some_and(|index| {
                index == current.attempts || (current.persistent && index < current.attempts)
            });
            current.attempts += 1;
            state.set(current);
            fail
        })
        .unwrap_or(false)
}

fn account(bytes: isize) {
    let _ = STATE.try_with(|state| {
        let mut current = state.get();
        if current.active {
            current.live_bytes += bytes;
            state.set(current);
        }
    });
}

fn record_allocation(size: usize) {
    let _ = STATE.try_with(|state| {
        let mut current = state.get();
        if current.active {
            current.largest_allocation = current.largest_allocation.max(size);
            state.set(current);
        }
    });
}

// SAFETY: Returns null only during the failure window on the current test thread; all other allocations use their original layouts.
unsafe impl GlobalAlloc for CheckedAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record_allocation(layout.size());
        if should_fail() {
            return null_mut();
        }
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            account(layout.size() as isize);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        account(-(layout.size() as isize));
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record_allocation(size);
        if should_fail() {
            return null_mut();
        }
        let pointer = unsafe { System.realloc(pointer, layout, size) };
        if !pointer.is_null() {
            account(size as isize - layout.size() as isize);
        }
        pointer
    }
}

#[global_allocator]
static ALLOCATOR: CheckedAllocator = CheckedAllocator;

pub(super) struct Scope;

impl Scope {
    pub(super) fn start(fail_at: Option<usize>, persistent: bool) -> Self {
        STATE.with(|state| {
            state.set(State {
                active: true,
                fail_at,
                persistent,
                attempts: 0,
                live_bytes: 0,
                largest_allocation: 0,
            })
        });
        Self
    }

    pub(super) fn finish(self) -> State {
        Self::current()
    }

    pub(super) fn current() -> State {
        STATE.with(Cell::get)
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        STATE.with(|state| {
            let mut current = state.get();
            current.active = false;
            state.set(current);
        });
    }
}
