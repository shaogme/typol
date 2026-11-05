use alloc::{string::String, vec::Vec};
use core::cmp::Ordering;
use opeo::{ErrSlot, OResult, Out};
use typol_core::{
    ChargedString, ChargedVec, ConfigValue, Diagnostic, DiagnosticCode, ExecutionBudget, Span,
    ValueKind, ValuePayloadView, ValueSource, ValueView,
};
use typol_semantic::{FieldView, SchemaAccessError, VariantView};

use crate::{
    ConfigParseErrors,
    instance::{LocationIndex, Member},
};

/// A typed path into a parsed configuration value.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ConfigPathSegment {
    Field(String),
    MapKey(String),
    Index(usize),
    Variant(String),
    Payload,
}

/// A sequence of schema-aware path segments into one configuration value.
#[derive(Clone)]
pub struct ConfigPath {
    segments: Vec<ConfigPathSegment>,
}

/// One scalar source replacement in a batch edit.
pub struct ConfigReplacement<'a> {
    path: &'a ConfigPath,
    value: &'a ConfigValue,
}

enum IndexedPathSegment {
    Field(ChargedString),
    MapKey(ChargedString),
    Index(usize),
    Variant(ChargedString),
    Payload,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum PathValueKind {
    Scalar,
    Array,
    Map,
    Structure,
    Enumeration,
    Other,
}

struct PathIndexEntry {
    parent: usize,
    segment: Option<IndexedPathSegment>,
    span: Option<Span>,
    kind: PathValueKind,
}

#[derive(Clone, Copy)]
pub(super) struct PathNodeInfo {
    pub(super) span: Option<Span>,
    pub(super) kind: PathValueKind,
}

pub(super) struct SourcePathIndex {
    entries: ChargedVec<PathIndexEntry>,
    order: ChargedVec<usize>,
}

pub(super) enum PathLookup {
    Missing,
    Explicit(Span),
}

impl ConfigPath {
    pub const fn new() -> Self {
        Self {
            segments: Vec::new(),
        }
    }

    pub fn field(mut self, name: impl Into<String>) -> Self {
        self.segments.push(ConfigPathSegment::Field(name.into()));
        self
    }

    pub fn map_key(mut self, key: impl Into<String>) -> Self {
        self.segments.push(ConfigPathSegment::MapKey(key.into()));
        self
    }

    pub fn index(mut self, index: usize) -> Self {
        self.segments.push(ConfigPathSegment::Index(index));
        self
    }

    pub fn variant(mut self, name: impl Into<String>) -> Self {
        self.segments.push(ConfigPathSegment::Variant(name.into()));
        self
    }

    pub fn payload(mut self) -> Self {
        self.segments.push(ConfigPathSegment::Payload);
        self
    }

    pub(super) fn parent_path(&self) -> Option<Self> {
        let length = self.segments.len().checked_sub(1)?;
        Some(Self {
            segments: self.segments.get(..length)?.to_vec(),
        })
    }

    pub(super) fn last(&self) -> Option<&ConfigPathSegment> {
        self.segments.last()
    }

    pub(super) fn pushed(&self, segment: ConfigPathSegment) -> Self {
        let mut path = self.clone();
        path.segments.push(segment);
        path
    }
}

impl Default for ConfigPath {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> ConfigReplacement<'a> {
    pub const fn new(path: &'a ConfigPath, value: &'a ConfigValue) -> Self {
        Self { path, value }
    }

    pub(super) const fn path(&self) -> &'a ConfigPath {
        self.path
    }

    pub(super) const fn value(&self) -> &'a ConfigValue {
        self.value
    }
}

impl SourcePathIndex {
    pub(super) fn new<'slot, 'schema>(
        locations: &LocationIndex<'schema>,
        value: &ConfigValue,
        budget: &ExecutionBudget,
        mut out: Out<'slot, '_, ConfigParseErrors>,
    ) -> OResult<'slot, Self> {
        let mut entries = ChargedVec::new(Some(budget), out.reborrow())?;
        entries.try_push(
            PathIndexEntry {
                parent: 0,
                segment: None,
                span: None,
                kind: PathValueKind::Other,
            },
            out.reborrow(),
        )?;
        let mut path_nodes = ChargedVec::new(Some(budget), out.reborrow())?;
        path_nodes.resize(locations.len(), 0, out.reborrow())?;
        let mut values = ChargedVec::new(Some(budget), out.reborrow())?;
        values.resize(locations.len(), None, out.reborrow())?;
        let mut root_found = false;
        for (node_id, node) in locations.nodes() {
            let node_index = node_id.index();
            if matches!(node.member(), Member::Root) {
                let Some(root) = values.get_mut(node_index) else {
                    return Err(out.fail(internal_document_error()));
                };
                *root = Some(value.root());
                let Some(root_entry) = entries.get_mut(0) else {
                    return Err(out.fail(internal_document_error()));
                };
                root_entry.span = node.span().map(|span| span.span());
                root_entry.kind = path_value_kind(value.root());
                root_found = true;
                continue;
            }
            let Some(parent_id) = node.parent() else {
                return Err(out.fail(internal_document_error()));
            };
            let Some(parent_path) = path_nodes.get(parent_id.index()).copied() else {
                return Err(out.fail(internal_document_error()));
            };
            let Some(parent_value) = values.get(parent_id.index()).copied().flatten() else {
                return Err(out.fail(internal_document_error()));
            };
            let (child_value, segments) =
                child_value(parent_value, node.member(), budget, out.reborrow())?;
            let mut path_id = parent_path;
            let mut insertion_parent = parent_path;
            let parent_span = entries.get(parent_path).and_then(|entry| entry.span);
            for segment in segments.into_iter().flatten() {
                path_id = entries.len();
                entries.try_push(
                    PathIndexEntry {
                        parent: insertion_parent,
                        segment: Some(segment),
                        span: None,
                        kind: PathValueKind::Other,
                    },
                    out.reborrow(),
                )?;
                if matches!(
                    entries
                        .get(path_id)
                        .and_then(|entry| entry.segment.as_ref()),
                    Some(IndexedPathSegment::Variant(_))
                ) {
                    if let ValueKind::Enumeration {
                        payload: ValuePayloadView::Structure(_),
                        ..
                    } = parent_value.as_ref()
                    {
                        if let Some(entry) = entries.get_mut(path_id) {
                            entry.span = parent_span;
                            entry.kind = PathValueKind::Structure;
                        }
                    }
                }
                insertion_parent = path_id;
            }
            let Some(path_entry) = entries.get_mut(path_id) else {
                return Err(out.fail(internal_document_error()));
            };
            if let Some(span) = node.span()
                && !matches!(
                    child_value.source(),
                    ValueSource::SchemaDefault | ValueSource::RustValue
                )
            {
                path_entry.span = Some(span.span());
            }
            path_entry.kind = path_value_kind(child_value);
            let Some(saved_path) = path_nodes.get_mut(node_index) else {
                return Err(out.fail(internal_document_error()));
            };
            *saved_path = path_id;
            let Some(saved_value) = values.get_mut(node_index) else {
                return Err(out.fail(internal_document_error()));
            };
            *saved_value = Some(child_value);
        }
        if !root_found {
            return Err(out.fail(internal_document_error()));
        }
        let mut order = ChargedVec::new(Some(budget), out.reborrow())?;
        for index in 1..entries.len() {
            order.try_push(index, out.reborrow())?;
        }
        order.sort_by(
            |left, right, _sort_out| {
                let ordering = match (entries.get(*left), entries.get(*right)) {
                    (Some(left), Some(right)) => left
                        .parent
                        .cmp(&right.parent)
                        .then_with(|| compare_indexed(&left.segment, &right.segment)),
                    _ => Ordering::Equal,
                };
                Ok(ordering)
            },
            out.reborrow(),
        )?;
        Ok(Self { entries, order })
    }

    pub(super) fn lookup(&self, path: &ConfigPath) -> PathLookup {
        let Some(info) = self.info(path) else {
            return PathLookup::Missing;
        };
        match info.span {
            Some(span) => PathLookup::Explicit(span),
            None => PathLookup::Missing,
        }
    }

    pub(super) fn info(&self, path: &ConfigPath) -> Option<PathNodeInfo> {
        let id = self.resolve_id(path)?;
        let entry = self.entries.get(id)?;
        Some(PathNodeInfo {
            span: entry.span,
            kind: entry.kind,
        })
    }

    pub(super) fn direct_children(
        &self,
        path: &ConfigPath,
    ) -> Option<impl Iterator<Item = PathNodeInfo> + '_> {
        let parent_id = self.resolve_id(path)?;
        Some(
            self.entries
                .iter()
                .enumerate()
                .filter_map(move |(id, entry)| {
                    (id != parent_id && entry.parent == parent_id).then_some(PathNodeInfo {
                        span: entry.span,
                        kind: entry.kind,
                    })
                }),
        )
    }

    fn resolve_id(&self, path: &ConfigPath) -> Option<usize> {
        let mut parent = 0;
        for segment in &path.segments {
            let order = self.order.as_slice();
            let found = order
                .binary_search_by(|index| {
                    let Some(entry) = self.entries.get(*index) else {
                        return Ordering::Less;
                    };
                    entry
                        .parent
                        .cmp(&parent)
                        .then_with(|| compare_query(&entry.segment, segment))
                })
                .ok()?;
            parent = *order.get(found)?;
        }
        Some(parent)
    }

    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }
}

fn path_value_kind(value: ValueView<'_>) -> PathValueKind {
    match value.as_ref() {
        ValueKind::Signed(_)
        | ValueKind::Unsigned(_)
        | ValueKind::Float32(_)
        | ValueKind::Float64(_)
        | ValueKind::String(_)
        | ValueKind::Bool(_) => PathValueKind::Scalar,
        ValueKind::Array(_) => PathValueKind::Array,
        ValueKind::Map(_) => PathValueKind::Map,
        ValueKind::Structure(_) => PathValueKind::Structure,
        ValueKind::Enumeration { .. } => PathValueKind::Enumeration,
        ValueKind::Optional(_) => PathValueKind::Other,
    }
}

fn child_value<'slot, 'value>(
    parent: ValueView<'value>,
    member: Member<'_>,
    budget: &ExecutionBudget,
    mut out: Out<'slot, '_, ConfigParseErrors>,
) -> OResult<'slot, (ValueView<'value>, [Option<IndexedPathSegment>; 2])> {
    match member {
        Member::Root => Err(out.fail(internal_document_error())),
        Member::Field(field) => {
            let slot = field_slot(field, out.reborrow())?;
            let ValueKind::Structure(fields) = parent.as_ref() else {
                return Err(out.fail(internal_document_error()));
            };
            let Some(field_value) = fields.get(slot) else {
                return Err(out.fail(internal_document_error()));
            };
            let Some(name) = field_value.name() else {
                return Err(out.fail(internal_document_error()));
            };
            Ok((
                field_value.value(),
                [
                    Some(indexed_name(name, false, budget, out.reborrow())?),
                    None,
                ],
            ))
        }
        Member::Index(index) => {
            let ValueKind::Array(values) = parent.as_ref() else {
                return Err(out.fail(internal_document_error()));
            };
            let Some(value) = values.get(index) else {
                return Err(out.fail(internal_document_error()));
            };
            Ok((
                value.value(),
                [Some(IndexedPathSegment::Index(index)), None],
            ))
        }
        Member::MapItem(index) => {
            let ValueKind::Map(values) = parent.as_ref() else {
                return Err(out.fail(internal_document_error()));
            };
            let Some(value) = values.get(index) else {
                return Err(out.fail(internal_document_error()));
            };
            let Some(name) = value.name() else {
                return Err(out.fail(internal_document_error()));
            };
            Ok((
                value.value(),
                [
                    Some(indexed_name(name, true, budget, out.reborrow())?),
                    None,
                ],
            ))
        }
        Member::Optional => {
            let ValueKind::Optional(Some(value)) = parent.as_ref() else {
                return Err(out.fail(internal_document_error()));
            };
            Ok((value, [None, None]))
        }
        Member::Payload(_variant) => {
            let ValueKind::Enumeration {
                variant,
                payload: ValuePayloadView::Single(value),
            } = parent.as_ref()
            else {
                return Err(out.fail(internal_document_error()));
            };
            let variant = indexed_variant(variant, budget, out.reborrow())?;
            Ok((value, [Some(variant), Some(IndexedPathSegment::Payload)]))
        }
        Member::VariantField { variant, field } => {
            let slot = field_slot(field, out.reborrow())?;
            let ValueKind::Enumeration {
                payload: ValuePayloadView::Structure(fields),
                ..
            } = parent.as_ref()
            else {
                return Err(out.fail(internal_document_error()));
            };
            let Some(field_value) = fields.get(slot) else {
                return Err(out.fail(internal_document_error()));
            };
            let Some(name) = field_value.name() else {
                return Err(out.fail(internal_document_error()));
            };
            let variant = variant_name(variant, budget, out.reborrow())?;
            Ok((
                field_value.value(),
                [
                    Some(variant),
                    Some(indexed_name(name, false, budget, out.reborrow())?),
                ],
            ))
        }
    }
}

fn field_slot<'slot>(
    field: FieldView<'_>,
    out: Out<'slot, '_, ConfigParseErrors>,
) -> OResult<'slot, usize> {
    let mut errors = ErrSlot::<SchemaAccessError>::new();
    match errors.try_call(|schema_out| field.slot(schema_out)) {
        Ok(slot) => Ok(slot),
        Err(_) => Err(out.fail(internal_document_error())),
    }
}

fn indexed_name<'slot>(
    name: &str,
    map_key: bool,
    budget: &ExecutionBudget,
    mut out: Out<'slot, '_, ConfigParseErrors>,
) -> OResult<'slot, IndexedPathSegment> {
    let name = ChargedString::copy(name, Some(budget), out.reborrow())?;
    Ok(if map_key {
        IndexedPathSegment::MapKey(name)
    } else {
        IndexedPathSegment::Field(name)
    })
}

fn indexed_variant<'slot>(
    name: &str,
    budget: &ExecutionBudget,
    mut out: Out<'slot, '_, ConfigParseErrors>,
) -> OResult<'slot, IndexedPathSegment> {
    let name = ChargedString::copy(name, Some(budget), out.reborrow())?;
    Ok(IndexedPathSegment::Variant(name))
}

fn variant_name<'slot>(
    variant: VariantView<'_>,
    budget: &ExecutionBudget,
    mut out: Out<'slot, '_, ConfigParseErrors>,
) -> OResult<'slot, IndexedPathSegment> {
    let mut errors = ErrSlot::<SchemaAccessError>::new();
    let name = match errors.try_call(|schema_out| variant.name(schema_out)) {
        Ok(name) => name,
        _ => return Err(out.fail(internal_document_error())),
    };
    indexed_variant(name, budget, out.reborrow())
}

fn compare_indexed(
    left: &Option<IndexedPathSegment>,
    right: &Option<IndexedPathSegment>,
) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => match (left, right) {
            (IndexedPathSegment::Field(left), IndexedPathSegment::Field(right)) => {
                left.as_str().cmp(right.as_str())
            }
            (IndexedPathSegment::MapKey(left), IndexedPathSegment::MapKey(right)) => {
                left.as_str().cmp(right.as_str())
            }
            (IndexedPathSegment::Index(left), IndexedPathSegment::Index(right)) => left.cmp(right),
            (IndexedPathSegment::Variant(left), IndexedPathSegment::Variant(right)) => {
                left.as_str().cmp(right.as_str())
            }
            (IndexedPathSegment::Payload, IndexedPathSegment::Payload) => Ordering::Equal,
            (IndexedPathSegment::Field(_), _) => Ordering::Less,
            (IndexedPathSegment::MapKey(_), IndexedPathSegment::Field(_)) => Ordering::Greater,
            (IndexedPathSegment::MapKey(_), _) => Ordering::Less,
            (
                IndexedPathSegment::Index(_),
                IndexedPathSegment::Field(_) | IndexedPathSegment::MapKey(_),
            ) => Ordering::Greater,
            (IndexedPathSegment::Index(_), _) => Ordering::Less,
            (IndexedPathSegment::Variant(_), IndexedPathSegment::Payload) => Ordering::Less,
            (IndexedPathSegment::Variant(_), _) => Ordering::Greater,
            (IndexedPathSegment::Payload, _) => Ordering::Greater,
        },
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn compare_query(indexed: &Option<IndexedPathSegment>, query: &ConfigPathSegment) -> Ordering {
    match (indexed.as_ref(), query) {
        (Some(IndexedPathSegment::Field(left)), ConfigPathSegment::Field(right)) => {
            left.as_str().cmp(right)
        }
        (Some(IndexedPathSegment::MapKey(left)), ConfigPathSegment::MapKey(right)) => {
            left.as_str().cmp(right)
        }
        (Some(IndexedPathSegment::Index(left)), ConfigPathSegment::Index(right)) => left.cmp(right),
        (Some(IndexedPathSegment::Variant(left)), ConfigPathSegment::Variant(right)) => {
            left.as_str().cmp(right)
        }
        (Some(IndexedPathSegment::Payload), ConfigPathSegment::Payload) => Ordering::Equal,
        (Some(IndexedPathSegment::Field(_)), _) => Ordering::Less,
        (Some(IndexedPathSegment::MapKey(_)), ConfigPathSegment::Field(_)) => Ordering::Greater,
        (Some(IndexedPathSegment::MapKey(_)), _) => Ordering::Less,
        (
            Some(IndexedPathSegment::Index(_)),
            ConfigPathSegment::Field(_) | ConfigPathSegment::MapKey(_),
        ) => Ordering::Greater,
        (Some(IndexedPathSegment::Index(_)), _) => Ordering::Less,
        (Some(IndexedPathSegment::Variant(_)), ConfigPathSegment::Payload) => Ordering::Less,
        (Some(IndexedPathSegment::Variant(_)), _) => Ordering::Greater,
        (Some(IndexedPathSegment::Payload), _) => Ordering::Greater,
        (None, _) => Ordering::Less,
    }
}

fn internal_document_error() -> ConfigParseErrors {
    ConfigParseErrors::emergency(Diagnostic::emergency(DiagnosticCode::INTERNAL_ERROR), None)
}
