use super::arena::{EdgeRange, NodeKind, PayloadKind, TextRange, ValueEdge, ValueNode};
use crate::{ChargedString, ChargedVec, ExecutionBudget, ValueSource};
use core::fmt;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ValueId {
    pub(super) arena_id: usize,
    pub(super) index: usize,
}

/// A root value owned by a flat arena; dropping nodes does not recursively traverse child values.
pub struct ConfigValue {
    arena_id: usize,
    root_index: usize,
    root: ValueNode,
    nodes: ChargedVec<ValueNode>,
    edges: ChargedVec<ValueEdge>,
    text: ChargedVec<ChargedString>,
}

pub(super) struct ConfigValueParts {
    pub(super) arena_id: usize,
    pub(super) root_index: usize,
    pub(super) root: ValueNode,
    pub(super) nodes: ChargedVec<ValueNode>,
    pub(super) edges: ChargedVec<ValueEdge>,
    pub(super) text: ChargedVec<ChargedString>,
}

#[derive(Clone, Copy)]
pub struct ValueView<'a> {
    owner: &'a ConfigValue,
    index: usize,
    node: &'a ValueNode,
}

#[derive(Clone, Copy)]
pub struct ValueEntries<'a> {
    owner: &'a ConfigValue,
    range: EdgeRange,
}

#[derive(Clone, Copy)]
pub struct ValueEntryView<'a> {
    name: Option<&'a str>,
    value: ValueView<'a>,
}

#[derive(Clone, Copy)]
pub struct NamedValueEntryView<'a>(&'a str, ValueView<'a>);

#[derive(Clone, Copy)]
pub enum ValuePayloadView<'a> {
    Unit,
    Single(ValueView<'a>),
    Structure(ValueEntries<'a>),
}

pub enum ValueKind<'a> {
    Signed(i128),
    Unsigned(u128),
    Float32(f32),
    Float64(f64),
    String(&'a str),
    Bool(bool),
    Optional(Option<ValueView<'a>>),
    Array(ValueEntries<'a>),
    Map(ValueEntries<'a>),
    Structure(ValueEntries<'a>),
    Enumeration {
        variant: &'a str,
        payload: ValuePayloadView<'a>,
    },
}

impl ConfigValue {
    pub(super) fn from_parts(parts: ConfigValueParts) -> Self {
        Self {
            arena_id: parts.arena_id,
            root_index: parts.root_index,
            root: parts.root,
            nodes: parts.nodes,
            edges: parts.edges,
            text: parts.text,
        }
    }

    pub(super) fn into_parts(self) -> ConfigValueParts {
        ConfigValueParts {
            arena_id: self.arena_id,
            root_index: self.root_index,
            root: self.root,
            nodes: self.nodes,
            edges: self.edges,
            text: self.text,
        }
    }

    pub fn root(&self) -> ValueView<'_> {
        ValueView {
            owner: self,
            index: self.root_index,
            node: &self.root,
        }
    }

    pub fn as_ref(&self) -> ValueKind<'_> {
        self.root().as_ref()
    }

    pub fn source(&self) -> ValueSource {
        self.root().source()
    }

    pub fn budget(&self) -> Option<&ExecutionBudget> {
        self.nodes.budget()
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    fn view(&self, index: usize) -> Option<ValueView<'_>> {
        let node = self.nodes.get(index)?;
        Some(ValueView {
            owner: self,
            index,
            node,
        })
    }

    fn text_range(&self, range: TextRange) -> Option<&str> {
        self.text
            .get(range.segment)
            .and_then(|text| text.get(range.start..range.end))
    }
}

impl<'a> ValueView<'a> {
    pub const fn source(self) -> ValueSource {
        self.node.source
    }

    pub fn as_ref(self) -> ValueKind<'a> {
        match self.node.kind {
            NodeKind::Signed(value) => ValueKind::Signed(value),
            NodeKind::Unsigned(value) => ValueKind::Unsigned(value),
            NodeKind::Float32(value) => ValueKind::Float32(value),
            NodeKind::Float64(value) => ValueKind::Float64(value),
            NodeKind::String(range) => {
                ValueKind::String(self.owner.text_range(range).unwrap_or_default())
            }
            NodeKind::Bool(value) => ValueKind::Bool(value),
            NodeKind::Optional(child) => {
                ValueKind::Optional(child.and_then(|index| self.owner.view(index)))
            }
            NodeKind::Array(range) => ValueKind::Array(ValueEntries::new(self.owner, range)),
            NodeKind::Map(range) => ValueKind::Map(ValueEntries::new(self.owner, range)),
            NodeKind::Structure(range) => {
                ValueKind::Structure(ValueEntries::new(self.owner, range))
            }
            NodeKind::Enumeration { variant, payload } => {
                let variant = self.owner.text_range(variant).unwrap_or_default();
                let payload = match payload {
                    PayloadKind::Unit => ValuePayloadView::Unit,
                    PayloadKind::Single(index) => self
                        .owner
                        .view(index)
                        .map_or(ValuePayloadView::Unit, ValuePayloadView::Single),
                    PayloadKind::Structure(range) => {
                        ValuePayloadView::Structure(ValueEntries::new(self.owner, range))
                    }
                };
                ValueKind::Enumeration { variant, payload }
            }
        }
    }

    pub const fn id(self) -> ValueId {
        ValueId {
            arena_id: self.owner.arena_id,
            index: self.index,
        }
    }
}

impl<'a> ValueEntries<'a> {
    fn new(owner: &'a ConfigValue, range: EdgeRange) -> Self {
        Self { owner, range }
    }

    /// Read an entry by its zero-based container slot without changing the iterator cursor.
    pub fn get(self, index: usize) -> Option<ValueEntryView<'a>> {
        let edge_index = self.range.start.checked_add(index)?;
        if edge_index >= self.range.end {
            return None;
        }
        let edge = self.owner.edges.get(edge_index)?;
        let value = self.owner.view(edge.child)?;
        let name = edge.name.and_then(|range| self.owner.text_range(range));
        Some(ValueEntryView { name, value })
    }
}

impl<'a> Iterator for ValueEntries<'a> {
    type Item = ValueEntryView<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.range.start >= self.range.end {
            return None;
        }
        let edge_index = self.range.start;
        self.range.start += 1;
        let edge = self.owner.edges.get(edge_index)?;
        let value = self.owner.view(edge.child)?;
        let name = edge.name.and_then(|range| self.owner.text_range(range));
        Some(ValueEntryView { name, value })
    }

    fn nth(&mut self, n: usize) -> Option<Self::Item> {
        if n >= self.len() {
            self.range.start = self.range.end;
            return None;
        }
        self.range.start += n;
        self.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.range.end.saturating_sub(self.range.start);
        (remaining, Some(remaining))
    }
}

impl DoubleEndedIterator for ValueEntries<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.range.start >= self.range.end {
            return None;
        }
        self.range.end -= 1;
        let edge = self.owner.edges.get(self.range.end)?;
        let value = self.owner.view(edge.child)?;
        let name = edge.name.and_then(|range| self.owner.text_range(range));
        Some(ValueEntryView { name, value })
    }

    fn nth_back(&mut self, n: usize) -> Option<Self::Item> {
        if n >= self.len() {
            self.range.end = self.range.start;
            return None;
        }
        self.range.end -= n;
        self.next_back()
    }
}

impl ExactSizeIterator for ValueEntries<'_> {}

impl<'a> ValueEntryView<'a> {
    pub const fn name(&self) -> Option<&'a str> {
        self.name
    }

    pub fn as_named(&self) -> Option<NamedValueEntryView<'a>> {
        Some(NamedValueEntryView(self.name?, self.value))
    }

    pub const fn value(&self) -> ValueView<'a> {
        self.value
    }
}

impl<'a> NamedValueEntryView<'a> {
    pub const fn name(&self) -> &'a str {
        self.0
    }

    pub const fn value(&self) -> ValueView<'a> {
        self.1
    }
}

impl fmt::Debug for ConfigValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConfigValue")
            .field("node_count", &self.nodes.len())
            .field("edge_count", &self.edges.len())
            .field("source", &self.source())
            .finish()
    }
}
