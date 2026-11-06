use opeo::{OResult, Out};
use typol_core::{ChargedVec, ExecutionBudget, Span};

use super::{
    super::{
        encoding::ConfigEditErrorKind,
        path::{PathValueKind, SourcePathIndex},
        syntax::{SourceContainer, SourceItemBounds, SourceLayout, SourceSyntax},
    },
    error::ConfigEditError,
};

pub(super) struct AnalyzedChild<'source> {
    span: Span,
    item: SourceItemBounds,
    indent: Option<&'source str>,
    colon_spacing: &'source str,
}

pub(super) struct ContainerAnalysis<'source> {
    id: usize,
    kind: PathValueKind,
    bounds: SourceContainer,
    layout: SourceLayout<'source>,
    children: ChargedVec<AnalyzedChild<'source>>,
    indented_children: ChargedVec<usize>,
}

pub(super) struct RemovalEntry {
    container_id: usize,
    span: Span,
}

pub(super) struct PreparedEdits<'source> {
    containers: ChargedVec<ContainerAnalysis<'source>>,
    removals: ChargedVec<RemovalEntry>,
}

impl<'source> ContainerAnalysis<'source> {
    pub(super) fn new<'slot>(
        id: usize,
        source: &'source str,
        index: &SourcePathIndex,
        budget: Option<&ExecutionBudget>,
        mut out: Out<'slot, '_, ConfigEditError>,
    ) -> OResult<'slot, Self> {
        let Some(info) = index.info_by_id(id) else {
            return Err(out.fail(ConfigEditError::new(ConfigEditErrorKind::InvalidPath)));
        };
        if !matches!(
            info.kind,
            PathValueKind::Structure | PathValueKind::Map | PathValueKind::Array
        ) {
            return Err(out.fail(ConfigEditError::new(ConfigEditErrorKind::InvalidContainer)));
        }
        let Some(span) = info.span else {
            return Err(out.fail(ConfigEditError::new(ConfigEditErrorKind::ValueNotExplicit)));
        };
        let bounds = SourceSyntax::scan_container(source, span, info.kind, budget)
            .map_err(|kind| out.reborrow().fail(ConfigEditError::new(kind)))?;
        let layout = SourceSyntax::layout(source, bounds, budget)
            .map_err(|kind| out.reborrow().fail(ConfigEditError::new(kind)))?;
        consume_search(budget, index.len(), out.reborrow())?;
        consume_search(budget, index.len(), out.reborrow())?;
        let Some(indexed_children) = index.direct_children_by_id(id) else {
            return Err(out.fail(ConfigEditError::new(ConfigEditErrorKind::InvalidPath)));
        };
        let mut spans = ChargedVec::new(budget, out.reborrow())?;
        for child in indexed_children {
            if let Some(budget) = budget {
                budget.consume(1, out.reborrow())?;
            }
            if let Some(span) = child.span {
                spans.try_push(span, out.reborrow())?;
            }
        }
        spans.sort_by(
            |left, right, mut sort_out| {
                if let Some(budget) = budget {
                    budget.consume(1, sort_out.reborrow())?;
                }
                Ok(left
                    .start()
                    .cmp(&right.start())
                    .then_with(|| left.end().cmp(&right.end())))
            },
            out.reborrow(),
        )?;
        let items =
            SourceSyntax::entry_bounds(source, bounds, spans.as_slice(), budget, out.reborrow())?;
        let mut children = ChargedVec::new(budget, out.reborrow())?;
        let mut indented_children = ChargedVec::new(budget, out.reborrow())?;
        children.try_reserve_exact(spans.len(), out.reborrow())?;
        indented_children.try_reserve_exact(spans.len(), out.reborrow())?;
        for (index, span) in spans.iter().copied().enumerate() {
            let Some(item) = items.get(index).copied() else {
                return Err(out.fail(ConfigEditError::new(ConfigEditErrorKind::EditConflict)));
            };
            let (indent, colon_spacing) = SourceSyntax::entry_layout(source, span, item, budget)
                .map_err(|kind| out.reborrow().fail(ConfigEditError::new(kind)))?;
            if indent.is_some() {
                indented_children.try_push(index, out.reborrow())?;
            }
            children.try_push(
                AnalyzedChild {
                    span,
                    item,
                    indent,
                    colon_spacing,
                },
                out.reborrow(),
            )?;
        }
        Ok(Self {
            id,
            kind: info.kind,
            bounds,
            layout,
            children,
            indented_children,
        })
    }

    pub(super) const fn id(&self) -> usize {
        self.id
    }

    pub(super) const fn kind(&self) -> PathValueKind {
        self.kind
    }

    pub(super) const fn open(&self) -> usize {
        self.bounds.open
    }

    pub(super) fn child_count(&self) -> usize {
        self.children.len()
    }

    pub(super) fn child_span(&self, index: usize) -> Option<Span> {
        self.children.get(index).map(|child| child.span)
    }

    pub(super) fn last_span(&self) -> Option<Span> {
        self.children.last().map(|child| child.span)
    }

    pub(super) fn trailing_comma(&self) -> Option<usize> {
        self.children.last().and_then(|child| child.item.next_comma)
    }

    pub(super) fn layout_for_insertion<'slot>(
        &self,
        index: usize,
        budget: Option<&ExecutionBudget>,
        out: Out<'slot, '_, ConfigEditError>,
    ) -> OResult<'slot, Option<SourceLayout<'source>>> {
        let children = self.children.as_slice();
        if index > children.len() {
            return Ok(None);
        }
        let indent_positions = self.indented_children.as_slice();
        consume_search(budget, indent_positions.len(), out)?;
        let split = indent_positions.partition_point(|position| *position < index);
        let reference_index = if indent_positions.get(split) == Some(&index) {
            Some(index)
        } else {
            let previous = split
                .checked_sub(1)
                .and_then(|position| indent_positions.get(position).copied());
            let next = indent_positions.get(split).copied();
            match (previous, next) {
                (Some(previous), Some(next))
                    if next.saturating_sub(index) < index.saturating_sub(previous) =>
                {
                    Some(next)
                }
                (Some(previous), _) => Some(previous),
                (None, Some(next)) => Some(next),
                (None, None) => None,
            }
        };
        let fallback = children
            .get(index)
            .map(|_| index)
            .or_else(|| children.len().checked_sub(1));
        let reference = reference_index
            .or(fallback)
            .and_then(|child_index| children.get(child_index));
        let mut layout = self.layout;
        if let Some(reference) = reference {
            layout.indent = reference.indent.unwrap_or("    ");
            layout.colon_spacing = reference.colon_spacing;
        }
        Ok(Some(layout))
    }

    pub(super) fn item_bounds<'slot>(
        &self,
        span: Span,
        budget: Option<&ExecutionBudget>,
        out: Out<'slot, '_, ConfigEditError>,
    ) -> OResult<'slot, Option<SourceItemBounds>> {
        let children = self.children.as_slice();
        consume_search(budget, children.len(), out)?;
        let index = children.partition_point(|child| child.span.start() < span.start());
        Ok(children
            .get(index)
            .filter(|child| child.span == span)
            .map(|child| child.item))
    }
}

impl RemovalEntry {
    pub(super) const fn new(container_id: usize, span: Span) -> Self {
        Self { container_id, span }
    }

    pub(super) const fn container_id(&self) -> usize {
        self.container_id
    }

    pub(super) const fn span(&self) -> Span {
        self.span
    }
}

impl<'source> PreparedEdits<'source> {
    pub(super) const fn new(
        containers: ChargedVec<ContainerAnalysis<'source>>,
        removals: ChargedVec<RemovalEntry>,
    ) -> Self {
        Self {
            containers,
            removals,
        }
    }

    pub(super) fn container<'borrow, 'slot>(
        &'borrow self,
        id: usize,
        budget: Option<&ExecutionBudget>,
        out: Out<'slot, '_, ConfigEditError>,
    ) -> OResult<'slot, Option<&'borrow ContainerAnalysis<'source>>> {
        let containers = self.containers.as_slice();
        consume_search(budget, containers.len(), out)?;
        let index = containers.binary_search_by_key(&id, |container| container.id);
        let Ok(index) = index else {
            return Ok(None);
        };
        Ok(containers.get(index))
    }

    pub(super) fn removes<'slot>(
        &self,
        container_id: usize,
        span: Span,
        budget: Option<&ExecutionBudget>,
        out: Out<'slot, '_, ConfigEditError>,
    ) -> OResult<'slot, bool> {
        let removals = self.removals.as_slice();
        consume_search(budget, removals.len(), out)?;
        Ok(removals
            .binary_search_by(|entry| {
                entry
                    .container_id
                    .cmp(&container_id)
                    .then_with(|| entry.span.start().cmp(&span.start()))
                    .then_with(|| entry.span.end().cmp(&span.end()))
            })
            .is_ok())
    }
}

fn consume_search<'slot>(
    budget: Option<&ExecutionBudget>,
    length: usize,
    out: Out<'slot, '_, ConfigEditError>,
) -> OResult<'slot, ()> {
    let Some(budget) = budget else {
        return Ok(());
    };
    budget.consume(binary_search_bound(length), out)
}

fn binary_search_bound(length: usize) -> usize {
    (usize::BITS - length.leading_zeros()) as usize
}
