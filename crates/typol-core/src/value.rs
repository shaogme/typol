mod arena;
mod copy;
mod errors;
mod views;

use arena::{ArenaRebase, EdgeRange, NodeKind, PayloadKind, TextRange, ValueEdge, ValueNode};
pub use copy::ValueCopyLimits;
pub use errors::{AllocationError, ValueError, ValueErrorKind, ValueSource};
use views::ConfigValueParts;
pub use views::{
    ConfigValue, NamedValueEntryView, ValueEntries, ValueEntryView, ValueId, ValueKind,
    ValuePayloadView, ValueView,
};

use crate::{
    BudgetError, DiagnosticContext, ExecutionBudget, diagnostic_context, storage::WorkspaceString,
    workspace::WorkspaceVec,
};
use core::{
    cmp::Ordering,
    sync::atomic::{AtomicUsize, Ordering as AtomicOrdering},
};
use opeo::{OResult, Out, ResultOutExt};

static NEXT_ARENA_ID: AtomicUsize = AtomicUsize::new(1);

fn reserve_arena_id() -> Option<usize> {
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

static ARENA_ID_EXHAUSTED: DiagnosticContext =
    diagnostic_context!("typol-core", "value-arena", "arena-id-exhausted",);
static ARENA_ID_MISMATCH: DiagnosticContext = diagnostic_context!(
    "typol-core",
    "value-arena",
    "value-id-does-not-belong-to-arena",
);
static ARENA_BUDGET_MISMATCH: DiagnosticContext = diagnostic_context!(
    "typol-core",
    "value-arena",
    "value-budget-does-not-match-arena",
);
static ARENA_NODE_WRITE: DiagnosticContext =
    diagnostic_context!("typol-core", "value-arena", "reserved-node-write-failed",);
static ARENA_EDGE_WRITE: DiagnosticContext =
    diagnostic_context!("typol-core", "value-arena", "reserved-edge-write-failed",);
static VALUE_NAME_INDEX_MISSING: DiagnosticContext =
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
enum NamedContainerKind {
    Map,
    Structure,
}

struct NamedContainerSpec {
    kind: NamedContainerKind,
    source: ValueSource,
}

/// Builds value nodes in a single arena; the final value owns all text, edges, and nodes.
pub struct ConfigValueBuilder {
    arena_id: usize,
    nodes: WorkspaceVec<ValueNode>,
    edges: WorkspaceVec<ValueEdge>,
    text: WorkspaceVec<WorkspaceString>,
}

impl ConfigValueBuilder {
    /// Consume an existing value and continue building in its arena, preserving child indexes.
    pub fn from_value(value: ConfigValue) -> (Self, ValueId) {
        let ConfigValueParts {
            arena_id,
            root_index,
            root: _,
            nodes,
            edges,
            text,
        } = value.into_parts();
        (
            Self {
                arena_id,
                nodes,
                edges,
                text,
            },
            ValueId {
                arena_id,
                index: root_index,
            },
        )
    }

    pub fn new<'slot>(
        budget: Option<&ExecutionBudget>,
        mut out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, Self> {
        let Some(arena_id) = reserve_arena_id() else {
            return Err(out.fail(ValueError::internal(ARENA_ID_EXHAUSTED)));
        };
        let nodes = WorkspaceVec::new(budget, out.reborrow())?;
        let edges = WorkspaceVec::new(budget, out.reborrow())?;
        let mut text: WorkspaceVec<WorkspaceString> = WorkspaceVec::new(budget, out.reborrow())?;
        text.try_push(
            WorkspaceString::with_capacity(0, budget, out.reborrow())?,
            out.reborrow(),
        )?;
        Ok(Self {
            arena_id,
            nodes,
            edges,
            text,
        })
    }

    pub fn append_value<'slot>(
        &mut self,
        value: ConfigValue,
        mut out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ValueId> {
        let same_budget = match (self.nodes.budget(), value.budget()) {
            (Some(left), Some(right)) => left.same_call(right),
            (None, None) => true,
            _ => false,
        };
        if !same_budget {
            return Err(out.fail(ValueError::internal(ARENA_BUDGET_MISMATCH)));
        }
        let ConfigValueParts {
            arena_id: _,
            root_index,
            root: _,
            nodes,
            edges,
            text,
        } = value.into_parts();
        let node_offset = self.nodes.len();
        let edge_offset = self.edges.len();
        let text_offset = self.text.len();
        self.nodes.try_reserve(nodes.len(), out.reborrow())?;
        self.edges.try_reserve(edges.len(), out.reborrow())?;
        self.text.try_reserve(text.len(), out.reborrow())?;
        self.text.extend(text, out.reborrow())?;
        for edge in edges {
            let child = edge.child.checked_add(node_offset).ok_or_else(|| {
                out.reborrow()
                    .fail(ValueError::from(BudgetError::CapacityExceeded))
            })?;
            let name = match edge.name {
                Some(range) => {
                    Some(ArenaRebase::text_range(range, text_offset).ok_or_else(|| {
                        out.reborrow()
                            .fail(ValueError::from(BudgetError::CapacityExceeded))
                    })?)
                }
                None => None,
            };
            self.edges
                .try_push(ValueEdge { name, child }, out.reborrow())?;
        }
        for node in nodes {
            let kind = ArenaRebase::node_kind(node.kind, node_offset, edge_offset, text_offset)
                .ok_or_else(|| {
                    out.reborrow()
                        .fail(ValueError::from(BudgetError::CapacityExceeded))
                })?;
            self.nodes.try_push(
                ValueNode {
                    kind,
                    source: node.source,
                },
                out.reborrow(),
            )?;
        }
        let index = root_index.checked_add(node_offset).ok_or_else(|| {
            out.reborrow()
                .fail(ValueError::from(BudgetError::CapacityExceeded))
        })?;
        Ok(ValueId {
            arena_id: self.arena_id,
            index,
        })
    }

    /// Copies a value subtree with an explicit logical nesting limit and preserves its source tags.
    ///
    /// The root has depth zero; optional children, container entries, and single enum payloads
    /// add one level. Enum structure payload fields add two levels because their anonymous
    /// structure contributes an additional level. A depth equal to the limit is accepted.
    /// Depth violations return `ResourceLimitExceeded`; depth arithmetic overflow returns
    /// `CapacityExceeded`. Frame and child storage uses the builder's execution budget when set.
    pub fn append_view<'slot>(
        &mut self,
        value: ValueView<'_>,
        limits: ValueCopyLimits,
        mut out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ValueId> {
        let budget = self
            .nodes
            .budget()
            .map(|budget| budget.try_clone(out.reborrow()))
            .transpose()?;
        copy::ValueViewCopier::new(budget).copy_to(self, value, limits, out)
    }

    pub fn add_signed<'slot>(
        &mut self,
        value: i128,
        source: ValueSource,
        out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ValueId> {
        self.add_simple(NodeKind::Signed(value), source, out)
    }

    pub fn add_unsigned<'slot>(
        &mut self,
        value: u128,
        source: ValueSource,
        out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ValueId> {
        self.add_simple(NodeKind::Unsigned(value), source, out)
    }

    pub fn add_float32<'slot>(
        &mut self,
        value: f32,
        source: ValueSource,
        out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ValueId> {
        self.add_simple(NodeKind::Float32(value), source, out)
    }

    pub fn add_float64<'slot>(
        &mut self,
        value: f64,
        source: ValueSource,
        out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ValueId> {
        self.add_simple(NodeKind::Float64(value), source, out)
    }

    pub fn add_bool<'slot>(
        &mut self,
        value: bool,
        source: ValueSource,
        out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ValueId> {
        self.add_simple(NodeKind::Bool(value), source, out)
    }

    pub fn add_string<'slot>(
        &mut self,
        value: &str,
        source: ValueSource,
        mut out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ValueId> {
        let index = self.prepare_node(out.reborrow())?;
        let range = self.append_text(value, out.reborrow())?;
        self.commit_node(index, NodeKind::String(range), source, out.reborrow())
    }

    pub fn add_optional<'slot>(
        &mut self,
        value: Option<ValueId>,
        source: ValueSource,
        mut out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ValueId> {
        let child = match value {
            Some(value) => Some(self.validate_id(value, out.reborrow())?),
            None => None,
        };
        self.add_simple(NodeKind::Optional(child), source, out)
    }

    pub fn add_array<'slot>(
        &mut self,
        values: &[ValueId],
        source: ValueSource,
        mut out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ValueId> {
        for value in values {
            self.validate_id(*value, out.reborrow())?;
        }
        let index = self.prepare_node(out.reborrow())?;
        self.edges.try_reserve(values.len(), out.reborrow())?;
        let start = self.edges.len();
        for value in values {
            if let Some(budget) = self.edges.budget() {
                budget.consume(1, out.reborrow())?;
            }
            if !self.edges.push_reserved(ValueEdge {
                name: None,
                child: value.index,
            }) {
                return Err(out.fail(ValueError::internal(ARENA_EDGE_WRITE)));
            }
        }
        let range = EdgeRange {
            start,
            end: self.edges.len(),
        };
        self.commit_node(index, NodeKind::Array(range), source, out.reborrow())
    }

    pub fn add_map<'slot>(
        &mut self,
        values: &[NamedValue<'_>],
        source: ValueSource,
        mut out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ValueId> {
        self.add_named_container(
            NamedContainerSpec {
                kind: NamedContainerKind::Map,
                source,
            },
            values,
            out.reborrow(),
        )
    }

    pub fn add_structure<'slot>(
        &mut self,
        fields: &[NamedValue<'_>],
        source: ValueSource,
        mut out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ValueId> {
        self.add_named_container(
            NamedContainerSpec {
                kind: NamedContainerKind::Structure,
                source,
            },
            fields,
            out.reborrow(),
        )
    }

    pub fn add_enumeration<'slot>(
        &mut self,
        variant: &str,
        payload: ValuePayload<'_>,
        source: ValueSource,
        mut out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ValueId> {
        let index = self.prepare_node(out.reborrow())?;
        let variant = self.append_text(variant, out.reborrow())?;
        let payload = match payload {
            ValuePayload::Unit => PayloadKind::Unit,
            ValuePayload::Single(child) => {
                PayloadKind::Single(self.validate_id(child, out.reborrow())?)
            }
            ValuePayload::Structure(fields) => {
                let range = self.append_named_edges(
                    fields,
                    ValueErrorKind::DuplicateFieldName,
                    out.reborrow(),
                )?;
                PayloadKind::Structure(range)
            }
        };
        self.commit_node(
            index,
            NodeKind::Enumeration { variant, payload },
            source,
            out.reborrow(),
        )
    }

    pub fn finish<'slot>(
        self,
        root: ValueId,
        out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ConfigValue> {
        if root.arena_id != self.arena_id || root.index >= self.nodes.len() {
            return Err(out.fail(ValueError::internal(ARENA_ID_MISMATCH)));
        }
        let Some(root_node) = self.nodes.get(root.index) else {
            return Err(out.fail(ValueError::internal(ARENA_ID_MISMATCH)));
        };
        Ok(ConfigValue::from_parts(ConfigValueParts {
            arena_id: self.arena_id,
            root_index: root.index,
            root: ValueNode {
                kind: root_node.kind,
                source: root_node.source,
            },
            nodes: self.nodes,
            edges: self.edges,
            text: self.text,
        }))
    }

    fn add_simple<'slot>(
        &mut self,
        kind: NodeKind,
        source: ValueSource,
        mut out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ValueId> {
        let index = self.prepare_node(out.reborrow())?;
        self.commit_node(index, kind, source, out.reborrow())
    }

    fn add_named_container<'slot>(
        &mut self,
        spec: NamedContainerSpec,
        values: &[NamedValue<'_>],
        mut out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ValueId> {
        let index = self.prepare_node(out.reborrow())?;
        let duplicate_kind = match spec.kind {
            NamedContainerKind::Map => ValueErrorKind::DuplicateMapKey,
            NamedContainerKind::Structure => ValueErrorKind::DuplicateFieldName,
        };
        let range = self.append_named_edges(values, duplicate_kind, out.reborrow())?;
        let kind = match spec.kind {
            NamedContainerKind::Map => NodeKind::Map(range),
            NamedContainerKind::Structure => NodeKind::Structure(range),
        };
        self.commit_node(index, kind, spec.source, out.reborrow())
    }

    fn append_named_edges<'slot>(
        &mut self,
        values: &[NamedValue<'_>],
        duplicate_kind: ValueErrorKind,
        mut out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, EdgeRange> {
        self.validate_named_values(values, duplicate_kind, out.reborrow())?;
        self.edges.try_reserve(values.len(), out.reborrow())?;
        let start = self.edges.len();
        for value in values {
            let name = self.append_text(value.name, out.reborrow())?;
            let child = value.value.index;
            if let Some(budget) = self.edges.budget() {
                budget.consume(1, out.reborrow())?;
            }
            if !self.edges.push_reserved(ValueEdge {
                name: Some(name),
                child,
            }) {
                return Err(out.fail(ValueError::internal(ARENA_EDGE_WRITE)));
            }
        }
        Ok(EdgeRange {
            start,
            end: self.edges.len(),
        })
    }

    fn validate_named_values<'slot>(
        &self,
        values: &[NamedValue<'_>],
        duplicate_kind: ValueErrorKind,
        mut out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ()> {
        if values.len() <= 2 {
            for (index, value) in values.iter().enumerate() {
                self.validate_id(value.value, out.reborrow())?;
                for (previous_index, previous) in values.iter().take(index).enumerate() {
                    let equal = match self.nodes.budget() {
                        Some(budget) => {
                            budget.compare_text(value.name, previous.name, out.reborrow())?
                                == Ordering::Equal
                        }
                        None => value.name == previous.name,
                    };
                    if equal {
                        return Err(out.fail(ValueError::duplicate(
                            duplicate_kind,
                            previous_index,
                            index,
                        )));
                    }
                }
            }
            return Ok(());
        }

        // Sorting name indices reduces comparisons to O(n log n) and preserves the earliest duplicate in input order.
        let mut indexes = WorkspaceVec::new(self.nodes.budget(), out.reborrow())?;
        indexes.try_reserve_exact(values.len(), out.reborrow())?;
        let mut invalid_id = None;
        for (index, value) in values.iter().enumerate() {
            if !self.id_is_valid(value.value) && invalid_id.is_none() {
                invalid_id = Some(index);
            }
            indexes.try_push(index, out.reborrow())?;
        }

        let budget = self.nodes.budget();
        indexes.sort_by(
            |left_index, right_index, mut sort_out| {
                let left = values
                    .get(*left_index)
                    .ok_or_else(|| ValueError::internal(VALUE_NAME_INDEX_MISSING))
                    .store_err_into(sort_out.reborrow())?;
                let right = values
                    .get(*right_index)
                    .ok_or_else(|| ValueError::internal(VALUE_NAME_INDEX_MISSING))
                    .store_err_into(sort_out.reborrow())?;
                let order = match budget {
                    Some(budget) => {
                        budget.compare_text(left.name, right.name, sort_out.reborrow())?
                    }
                    None => left.name.cmp(right.name),
                };
                Ok(order.then_with(|| left_index.cmp(right_index)))
            },
            out.reborrow(),
        )?;

        let mut duplicate = None;
        for pair in indexes.as_slice().windows(2) {
            let [left_index, right_index] = pair else {
                continue;
            };
            if let Some(budget) = budget {
                budget.consume(1, out.reborrow())?;
            }
            let left = values
                .get(*left_index)
                .ok_or_else(|| ValueError::internal(VALUE_NAME_INDEX_MISSING))
                .store_err_into(out.reborrow())?;
            let right = values
                .get(*right_index)
                .ok_or_else(|| ValueError::internal(VALUE_NAME_INDEX_MISSING))
                .store_err_into(out.reborrow())?;
            let equal = match budget {
                Some(budget) => {
                    budget.compare_text(left.name, right.name, out.reborrow())? == Ordering::Equal
                }
                None => left.name == right.name,
            };
            if equal && duplicate.is_none_or(|(_, second)| *right_index < second) {
                duplicate = Some((*left_index, *right_index));
            }
        }

        if invalid_id.is_some_and(|index| duplicate.is_none_or(|(_, second)| index <= second)) {
            return Err(out.fail(ValueError::internal(ARENA_ID_MISMATCH)));
        }
        if let Some((first, second)) = duplicate {
            return Err(out.fail(ValueError::duplicate(duplicate_kind, first, second)));
        }
        Ok(())
    }

    fn append_text<'slot>(
        &mut self,
        value: &str,
        mut out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, TextRange> {
        let Some(text) = self.text.get_mut(0) else {
            return Err(out.fail(ValueError::from(BudgetError::CapacityExceeded)));
        };
        let start = text.len();
        text.try_push_str(value, out.reborrow())?;
        Ok(TextRange {
            segment: 0,
            start,
            end: text.len(),
        })
    }

    fn validate_id<'slot>(
        &self,
        value: ValueId,
        out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, usize> {
        if !self.id_is_valid(value) {
            return Err(out.fail(ValueError::internal(ARENA_ID_MISMATCH)));
        }
        Ok(value.index)
    }

    fn id_is_valid(&self, value: ValueId) -> bool {
        value.arena_id == self.arena_id && value.index < self.nodes.len()
    }

    fn prepare_node<'slot>(
        &mut self,
        mut out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, usize> {
        self.nodes.try_reserve(1, out.reborrow())?;
        if let Some(budget) = self.nodes.budget() {
            budget.consume(1, out.reborrow())?;
        }
        Ok(self.nodes.len())
    }

    fn commit_node<'slot>(
        &mut self,
        index: usize,
        kind: NodeKind,
        source: ValueSource,
        out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ValueId> {
        if index != self.nodes.len() || !self.nodes.push_reserved(ValueNode { kind, source }) {
            return Err(out.fail(ValueError::internal(ARENA_NODE_WRITE)));
        }
        Ok(ValueId {
            arena_id: self.arena_id,
            index,
        })
    }
}
