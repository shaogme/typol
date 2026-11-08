use core::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

use super::{ValueId, ValueSource};
use crate::{DiagnosticContext, diagnostic_context};

pub(super) struct ArenaIdAllocator;

static NEXT_ARENA_ID: AtomicUsize = AtomicUsize::new(1);

impl ArenaIdAllocator {
    pub(super) fn reserve() -> Option<usize> {
        let mut current = NEXT_ARENA_ID.load(AtomicOrdering::Relaxed);
        loop {
            let next = current.checked_add(1)?;
            match NEXT_ARENA_ID.compare_exchange_weak(
                current,
                next,
                AtomicOrdering::Relaxed,
                AtomicOrdering::Relaxed,
            ) {
                Ok(_) => return Some(current),
                Err(actual) => current = actual,
            }
        }
    }
}

pub(super) static ARENA_ID_EXHAUSTED: DiagnosticContext =
    diagnostic_context!("typol-core", "value-arena", "arena-id-exhausted",);
pub(super) static ARENA_ID_MISMATCH: DiagnosticContext = diagnostic_context!(
    "typol-core",
    "value-arena",
    "value-id-does-not-belong-to-arena",
);
pub(super) static ARENA_BUDGET_MISMATCH: DiagnosticContext = diagnostic_context!(
    "typol-core",
    "value-arena",
    "value-budget-does-not-match-arena",
);
pub(super) static ARENA_NODE_WRITE: DiagnosticContext =
    diagnostic_context!("typol-core", "value-arena", "reserved-node-write-failed",);
pub(super) static ARENA_EDGE_WRITE: DiagnosticContext =
    diagnostic_context!("typol-core", "value-arena", "reserved-edge-write-failed",);
pub(super) static VALUE_NAME_INDEX_MISSING: DiagnosticContext =
    diagnostic_context!("typol-core", "value-arena", "named-value-index-missing",);

#[derive(Clone, Copy)]
pub struct NamedValue<'a> {
    name: &'a str,
    value: ValueId,
}

impl<'a> NamedValue<'a> {
    pub const fn new(name: &'a str, value: ValueId) -> Self {
        Self { name, value }
    }

    pub const fn name(self) -> &'a str {
        self.name
    }

    pub const fn value(self) -> ValueId {
        self.value
    }
}

#[derive(Clone, Copy)]
pub enum ValuePayload<'a> {
    Unit,
    Single(ValueId),
    Structure(&'a [NamedValue<'a>]),
}

#[derive(Clone, Copy)]
pub(super) enum NamedContainerKind {
    Map,
    Structure,
}

pub(super) struct NamedContainerSpec {
    pub(super) kind: NamedContainerKind,
    pub(super) source: ValueSource,
}

pub(super) struct ConfigValueCheckpoint {
    pub(super) nodes_len: usize,
    pub(super) edges_len: usize,
    pub(super) text_len: usize,
    pub(super) first_text_len: usize,
    pub(super) references_len: usize,
}
