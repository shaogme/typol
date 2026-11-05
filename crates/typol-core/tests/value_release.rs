use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    error::Error,
    io::Error as IoError,
    ptr,
};
use typol_core::{
    ConfigValueBuilder, ExecutionBudget, ExecutionLimits, ValueCopyLimits, ValueKind, ValuePayload,
    ValuePayloadView, ValueSource,
};
use typol_opeo_test_support::TestOpeo;

#[derive(Clone, Copy)]
struct Observation {
    target: *const u8,
    budget: *const ExecutionBudget,
    minimum: usize,
    released: bool,
    charged: bool,
}

thread_local! {
    static OBSERVATION: Cell<Option<Observation>> = const { Cell::new(None) };
}

struct ReleaseObserver;

unsafe impl GlobalAlloc for ReleaseObserver {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        let _ = OBSERVATION.try_with(|cell| {
            if let Some(mut observation) = cell.get() {
                if observation.target != pointer {
                    return;
                }
                // Safety: observation is enabled only during this thread's value drop, while the test owns the budget.
                let budget = unsafe { &*observation.budget };
                observation.released = true;
                observation.charged = budget.workspace_bytes() >= observation.minimum;
                cell.set(Some(observation));
            }
        });
        unsafe { System.dealloc(pointer, layout) };
    }
}

#[global_allocator]
static ALLOCATOR: ReleaseObserver = ReleaseObserver;

#[test]
fn deep_value_drop_is_iterative_and_keeps_text_charged_until_deallocation()
-> Result<(), Box<dyn Error>> {
    let budget = TestOpeo::call(|out| {
        ExecutionBudget::new(ExecutionLimits::new(1_000_000, 16_777_216), out)
    })?;
    let initial = budget.workspace_bytes();
    let text = "arena text must remain charged";
    let mut builder = TestOpeo::call(|out| ConfigValueBuilder::new(Some(&budget), out))?;
    let mut root = TestOpeo::call(|out| builder.add_string(text, ValueSource::ConfigLiteral, out))?;
    for index in 0..512 {
        root = if index % 2 == 0 {
            TestOpeo::call(|out| builder.add_optional(Some(root), ValueSource::RustValue, out))?
        } else {
            TestOpeo::call(|out| {
                builder.add_enumeration(
                    "wrapper",
                    ValuePayload::Single(root),
                    ValueSource::SchemaDefault,
                    out,
                )
            })?
        };
    }
    let value = TestOpeo::call(|out| builder.finish(root, out))?;
    let mut copied_builder = TestOpeo::call(|out| ConfigValueBuilder::new(Some(&budget), out))?;
    let copied_root = TestOpeo::call(|out| {
        copied_builder.append_view(value.root(), ValueCopyLimits::new(512), out)
    })?;
    let copied = TestOpeo::call(|out| copied_builder.finish(copied_root, out))?;
    let mut view = copied.root();
    loop {
        match view.as_ref() {
            ValueKind::Optional(Some(child)) => view = child,
            ValueKind::Enumeration {
                payload: ValuePayloadView::Single(child),
                ..
            } => view = child,
            ValueKind::String(stored) => {
                if stored != text {
                    return Err(IoError::other("arena text does not match").into());
                }
                break;
            }
            _ => return Err(IoError::other("arena wrapper chain ended unexpectedly").into()),
        }
    }
    let target = match view.as_ref() {
        ValueKind::String(stored) => stored.as_ptr(),
        _ => ptr::null(),
    };
    OBSERVATION.with(|cell| {
        cell.set(Some(Observation {
            target,
            budget: &budget,
            minimum: initial + text.len(),
            released: false,
            charged: false,
        }))
    });
    drop(copied);
    let observation = OBSERVATION
        .with(|cell| cell.replace(None))
        .ok_or_else(|| IoError::other("drop observation data is missing"))?;
    if target.is_null() || !observation.released || !observation.charged {
        return Err(IoError::other(
            "budget token did not remain alive while copied arena text was released",
        )
        .into());
    }
    drop(value);
    if budget.workspace_bytes() != initial {
        return Err(
            IoError::other("workspace balance was not restored after arena release").into(),
        );
    }
    Ok(())
}
