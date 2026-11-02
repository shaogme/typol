use crate::ValueSource;

#[derive(Clone, Copy)]
pub(super) struct TextRange {
    pub(super) segment: usize,
    pub(super) start: usize,
    pub(super) end: usize,
}

#[derive(Clone, Copy)]
pub(super) struct EdgeRange {
    pub(super) start: usize,
    pub(super) end: usize,
}

#[derive(Clone, Copy)]
pub(super) enum PayloadKind {
    Unit,
    Single(usize),
    Structure(EdgeRange),
}

#[derive(Clone, Copy)]
pub(super) enum NodeKind {
    Signed(i128),
    Unsigned(u128),
    Float32(f32),
    Float64(f64),
    String(TextRange),
    Bool(bool),
    Optional(Option<usize>),
    Array(EdgeRange),
    Map(EdgeRange),
    Structure(EdgeRange),
    Enumeration {
        variant: TextRange,
        payload: PayloadKind,
    },
}

pub(super) struct ValueNode {
    pub(super) kind: NodeKind,
    pub(super) source: ValueSource,
}

pub(super) struct ValueEdge {
    pub(super) name: Option<TextRange>,
    pub(super) child: usize,
}

pub(super) struct ArenaRebase;

impl ArenaRebase {
    pub(super) fn text_range(range: TextRange, offset: usize) -> Option<TextRange> {
        Some(TextRange {
            segment: range.segment.checked_add(offset)?,
            start: range.start,
            end: range.end,
        })
    }

    pub(super) fn edge_range(range: EdgeRange, offset: usize) -> Option<EdgeRange> {
        Some(EdgeRange {
            start: range.start.checked_add(offset)?,
            end: range.end.checked_add(offset)?,
        })
    }

    pub(super) fn node_kind(
        kind: NodeKind,
        node_offset: usize,
        edge_offset: usize,
        text_offset: usize,
    ) -> Option<NodeKind> {
        Some(match kind {
            NodeKind::Signed(value) => NodeKind::Signed(value),
            NodeKind::Unsigned(value) => NodeKind::Unsigned(value),
            NodeKind::Float32(value) => NodeKind::Float32(value),
            NodeKind::Float64(value) => NodeKind::Float64(value),
            NodeKind::String(range) => NodeKind::String(Self::text_range(range, text_offset)?),
            NodeKind::Bool(value) => NodeKind::Bool(value),
            NodeKind::Optional(child) => NodeKind::Optional(match child {
                Some(index) => Some(index.checked_add(node_offset)?),
                None => None,
            }),
            NodeKind::Array(range) => NodeKind::Array(Self::edge_range(range, edge_offset)?),
            NodeKind::Map(range) => NodeKind::Map(Self::edge_range(range, edge_offset)?),
            NodeKind::Structure(range) => {
                NodeKind::Structure(Self::edge_range(range, edge_offset)?)
            }
            NodeKind::Enumeration { variant, payload } => NodeKind::Enumeration {
                variant: Self::text_range(variant, text_offset)?,
                payload: match payload {
                    PayloadKind::Unit => PayloadKind::Unit,
                    PayloadKind::Single(index) => {
                        PayloadKind::Single(index.checked_add(node_offset)?)
                    }
                    PayloadKind::Structure(range) => {
                        PayloadKind::Structure(Self::edge_range(range, edge_offset)?)
                    }
                },
            },
        })
    }
}
