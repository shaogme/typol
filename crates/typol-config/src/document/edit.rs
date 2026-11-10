mod analysis;
mod error;

use self::analysis::{ContainerAnalysis, PreparedEdits, RemovalEntry};
use super::{
    encoding::{EntryEncoding, ScalarEncoder, ValueEncodingContext},
    path::{ConfigPath, ConfigPathSegment, ConfigReplacement, PathValueKind, SourcePathIndex},
    syntax::{SourceLayout, SourceSyntax},
};
use opeo::{ErrSlot, OResult, Out};
use typol_core::{
    BudgetError, ChargedString, ChargedVec, ConfigValue, ExecutionBudget, TextLayout, ValueView,
};

pub use error::{ConfigEditError, ConfigEditErrorKind};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConfigEditOptions {
    layout: TextLayout,
}

impl ConfigEditOptions {
    pub const fn new() -> Self {
        Self {
            layout: TextLayout::Compact,
        }
    }

    pub const fn layout(self) -> TextLayout {
        self.layout
    }

    pub const fn with_layout(mut self, layout: TextLayout) -> Self {
        self.layout = layout;
        self
    }
}

impl Default for ConfigEditOptions {
    fn default() -> Self {
        Self::new()
    }
}

const MAX_PATH_LOOKUPS_PER_EDIT: usize = 4;

pub enum ConfigEdit<'a> {
    Replace(ConfigReplacement<'a>),
    InsertField {
        path: &'a ConfigPath,
        value: &'a ConfigValue,
    },
    InsertMapEntry {
        container: &'a ConfigPath,
        key: &'a str,
        value: &'a ConfigValue,
    },
    InsertArrayItem {
        container: &'a ConfigPath,
        index: usize,
        value: &'a ConfigValue,
    },
    Remove {
        path: &'a ConfigPath,
    },
}

pub(super) struct SourcePatch {
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) encoded: ChargedString,
}

pub(super) struct EditContext<'a> {
    pub(super) source: &'a str,
    pub(super) index: &'a SourcePathIndex,
    pub(super) budget: Option<&'a ExecutionBudget>,
    pub(super) source_limit: usize,
    pub(super) options: ConfigEditOptions,
}

enum InsertionValue<'value, 'name> {
    Value(ValueView<'value>),
    Entry {
        name: &'name str,
        value: ValueView<'value>,
        map_key: bool,
    },
}

struct InsertionPlan<'analysis, 'source, 'value, 'name> {
    analysis: &'analysis ContainerAnalysis<'source>,
    index: Option<usize>,
    value: InsertionValue<'value, 'name>,
}

struct InsertionDetails {
    position: usize,
    prefix_comma: bool,
    prefix_space: bool,
    prefix_line_start: bool,
    suffix_comma: bool,
    suffix_space: bool,
    suffix_line_start: bool,
}

struct SpacingRequest<'source> {
    comma: bool,
    space: bool,
    line_start: bool,
    layout: SourceLayout<'source>,
    limit: usize,
}

pub(super) struct DocumentEditEngine<'a> {
    context: EditContext<'a>,
}

impl<'a> DocumentEditEngine<'a> {
    pub(super) const fn new(context: EditContext<'a>) -> Self {
        Self { context }
    }

    pub(super) fn resolve<'slot>(
        &self,
        edits: &[ConfigEdit<'_>],
        mut out: Out<'slot, '_, ConfigEditError>,
    ) -> OResult<'slot, ChargedVec<SourcePatch>> {
        if let Some(budget) = self.context.budget {
            budget.consume(edits.len(), out.reborrow())?;
        }
        let mut container_ids = ChargedVec::new(self.context.budget, out.reborrow())?;
        let mut removals = ChargedVec::new(self.context.budget, out.reborrow())?;
        let mut appends = ChargedVec::new(self.context.budget, out.reborrow())?;
        let mut encodes_values = false;
        for edit in edits {
            self.consume_path_work(edit, out.reborrow())?;
            match edit {
                ConfigEdit::Replace(_) => encodes_values = true,
                ConfigEdit::InsertField { path, .. } => {
                    encodes_values = true;
                    let Some(id) = self.context.index.parent_id(path) else {
                        return Err(
                            out.fail(ConfigEditError::new(ConfigEditErrorKind::InvalidPath))
                        );
                    };
                    container_ids.try_push(id, out.reborrow())?;
                    appends.try_push(id, out.reborrow())?;
                }
                ConfigEdit::InsertMapEntry { container, .. } => {
                    encodes_values = true;
                    let Some(id) = self.context.index.node_id(container) else {
                        return Err(
                            out.fail(ConfigEditError::new(ConfigEditErrorKind::InvalidPath))
                        );
                    };
                    container_ids.try_push(id, out.reborrow())?;
                    appends.try_push(id, out.reborrow())?;
                }
                ConfigEdit::InsertArrayItem {
                    container, index, ..
                } => {
                    encodes_values = true;
                    let Some(id) = self.context.index.node_id(container) else {
                        return Err(
                            out.fail(ConfigEditError::new(ConfigEditErrorKind::InvalidPath))
                        );
                    };
                    container_ids.try_push(id, out.reborrow())?;
                    let Some(children) = self.context.index.direct_children_by_id(id) else {
                        return Err(
                            out.fail(ConfigEditError::new(ConfigEditErrorKind::InvalidPath))
                        );
                    };
                    let child_count = children.count();
                    if let Some(budget) = self.context.budget {
                        budget.consume(child_count, out.reborrow())?;
                    }
                    if *index == child_count {
                        appends.try_push(id, out.reborrow())?;
                    }
                }
                ConfigEdit::Remove { path } => {
                    let Some(target) = self.context.index.info(path) else {
                        return Err(
                            out.fail(ConfigEditError::new(ConfigEditErrorKind::InvalidPath))
                        );
                    };
                    let Some(span) = target.span else {
                        return Err(
                            out.fail(ConfigEditError::new(ConfigEditErrorKind::ValueNotExplicit))
                        );
                    };
                    let Some(container_id) = self.context.index.parent_id(path) else {
                        return Err(
                            out.fail(ConfigEditError::new(ConfigEditErrorKind::InvalidPath))
                        );
                    };
                    container_ids.try_push(container_id, out.reborrow())?;
                    removals.try_push(RemovalEntry::new(container_id, span), out.reborrow())?;
                }
            }
        }

        let line_break = SourceSyntax::edit_line_break(
            self.context.source,
            self.context.budget,
            self.context.options.layout,
            encodes_values,
        )
        .map_err(|kind| out.reborrow().fail(ConfigEditError::new(kind)))?;

        container_ids.sort_by(
            |left, right, mut sort_out| {
                if let Some(budget) = self.context.budget {
                    budget.consume(1, sort_out.reborrow())?;
                }
                Ok(left.cmp(right))
            },
            out.reborrow(),
        )?;
        container_ids.dedup();
        appends.sort_by(
            |left, right, mut sort_out| {
                if let Some(budget) = self.context.budget {
                    budget.consume(1, sort_out.reborrow())?;
                }
                Ok(left.cmp(right))
            },
            out.reborrow(),
        )?;
        appends.dedup();
        removals.sort_by(
            |left, right, mut sort_out| {
                if let Some(budget) = self.context.budget {
                    budget.consume(1, sort_out.reborrow())?;
                }
                Ok(left
                    .container_id()
                    .cmp(&right.container_id())
                    .then_with(|| left.span().start().cmp(&right.span().start()))
                    .then_with(|| left.span().end().cmp(&right.span().end())))
            },
            out.reborrow(),
        )?;

        let mut containers = ChargedVec::new(self.context.budget, out.reborrow())?;
        for id in container_ids.as_slice() {
            containers.try_push(
                ContainerAnalysis::new(
                    *id,
                    self.context.source,
                    self.context.index,
                    self.context.budget,
                    out.reborrow(),
                )?,
                out.reborrow(),
            )?;
        }
        let prepared = PreparedEdits::new(containers, removals, appends);
        if let Some(budget) = self.context.budget {
            budget.consume(edits.len(), out.reborrow())?;
        }
        let mut patches = ChargedVec::new(self.context.budget, out.reborrow())?;
        for edit in edits {
            patches.try_push(
                self.resolve_edit(edit, &prepared, line_break, out.reborrow())?,
                out.reborrow(),
            )?;
        }
        Ok(patches)
    }

    fn consume_path_work<'slot>(
        &self,
        edit: &ConfigEdit<'_>,
        mut out: Out<'slot, '_, ConfigEditError>,
    ) -> OResult<'slot, ()> {
        let path_length = match edit {
            ConfigEdit::Replace(replacement) => replacement.path().segment_count(),
            ConfigEdit::InsertField { path, .. } | ConfigEdit::Remove { path } => {
                path.segment_count()
            }
            ConfigEdit::InsertMapEntry { container, .. }
            | ConfigEdit::InsertArrayItem { container, .. } => container.segment_count(),
        };
        let index_size = self.context.index.len();
        let search_bound = binary_search_bound(index_size);
        let path_steps = path_length
            .saturating_add(1)
            .checked_mul(search_bound.max(1))
            .and_then(|steps| steps.checked_mul(MAX_PATH_LOOKUPS_PER_EDIT))
            .ok_or_else(|| {
                out.reborrow().fail(ConfigEditError::new(
                    ConfigEditErrorKind::ResourceLimitExceeded,
                ))
            })?;
        let auxiliary_steps = search_bound.checked_mul(3).ok_or_else(|| {
            out.reborrow().fail(ConfigEditError::new(
                ConfigEditErrorKind::ResourceLimitExceeded,
            ))
        })?;
        let steps = path_steps.checked_add(auxiliary_steps).ok_or_else(|| {
            out.reborrow().fail(ConfigEditError::new(
                ConfigEditErrorKind::ResourceLimitExceeded,
            ))
        })?;
        if let Some(budget) = self.context.budget {
            budget.consume(steps, out)?;
        }
        Ok(())
    }

    fn resolve_edit<'slot>(
        &self,
        edit: &ConfigEdit<'_>,
        prepared: &PreparedEdits<'_>,
        line_break: &'static str,
        mut out: Out<'slot, '_, ConfigEditError>,
    ) -> OResult<'slot, SourcePatch> {
        let budget = self.context.budget;
        let limit = self.context.source_limit;
        match edit {
            ConfigEdit::Replace(replacement) => {
                let Some(info) = self.context.index.info(replacement.path()) else {
                    return Err(out.fail(ConfigEditError::new(ConfigEditErrorKind::InvalidPath)));
                };
                let Some(span) = info.span else {
                    return Err(out.fail(ConfigEditError::new(ConfigEditErrorKind::InvalidPath)));
                };
                let start = span.start();
                let end = span.end();
                let context = self
                    .value_context(
                        start,
                        self.context.index.parent_id(replacement.path()),
                        line_break,
                    )
                    .map_err(|kind| out.reborrow().fail(ConfigEditError::new(kind)))?;
                let mut encoded =
                    ScalarEncoder::encode_value(replacement.value().root(), context, budget, limit)
                        .map_err(|kind| out.reborrow().fail(ConfigEditError::new(kind)))?;
                let comments = SourceSyntax::comments_in(self.context.source, start, end, budget)
                    .map_err(|kind| out.reborrow().fail(ConfigEditError::new(kind)))?;
                append_charged(&mut encoded, comments.as_str(), limit)
                    .map_err(|kind| out.reborrow().fail(ConfigEditError::new(kind)))?;
                Ok(SourcePatch {
                    start,
                    end,
                    encoded,
                })
            }
            ConfigEdit::InsertField { path, value } => {
                let Some(ConfigPathSegment::Field(name)) = path.last() else {
                    return Err(out.fail(ConfigEditError::new(ConfigEditErrorKind::InvalidPath)));
                };
                if self
                    .context
                    .index
                    .info(path)
                    .is_some_and(|node| node.span.is_some())
                {
                    return Err(out.fail(ConfigEditError::new(
                        ConfigEditErrorKind::EntryAlreadyExists,
                    )));
                }
                let Some(parent_id) = self.context.index.parent_id(path) else {
                    return Err(out.fail(ConfigEditError::new(ConfigEditErrorKind::InvalidPath)));
                };
                let Some(analysis) = prepared.container(parent_id, budget, out.reborrow())? else {
                    return Err(out.fail(ConfigEditError::new(ConfigEditErrorKind::InvalidPath)));
                };
                if analysis.kind() != PathValueKind::Structure {
                    return Err(
                        out.fail(ConfigEditError::new(ConfigEditErrorKind::InvalidContainer))
                    );
                }
                self.insert_patch(
                    InsertionPlan {
                        analysis,
                        index: None,
                        value: InsertionValue::Entry {
                            name,
                            value: value.root(),
                            map_key: false,
                        },
                    },
                    prepared,
                    line_break,
                    out,
                )
            }
            ConfigEdit::InsertMapEntry {
                container,
                key,
                value,
            } => {
                let Some(container_id) = self.context.index.node_id(container) else {
                    return Err(out.fail(ConfigEditError::new(ConfigEditErrorKind::InvalidPath)));
                };
                if self
                    .context
                    .index
                    .map_key_child_info(container_id, key)
                    .is_some_and(|node| node.span.is_some())
                {
                    return Err(out.fail(ConfigEditError::new(
                        ConfigEditErrorKind::EntryAlreadyExists,
                    )));
                }
                let Some(analysis) = prepared.container(container_id, budget, out.reborrow())?
                else {
                    return Err(out.fail(ConfigEditError::new(ConfigEditErrorKind::InvalidPath)));
                };
                if analysis.kind() != PathValueKind::Map {
                    return Err(
                        out.fail(ConfigEditError::new(ConfigEditErrorKind::InvalidContainer))
                    );
                }
                self.insert_patch(
                    InsertionPlan {
                        analysis,
                        index: None,
                        value: InsertionValue::Entry {
                            name: key,
                            value: value.root(),
                            map_key: true,
                        },
                    },
                    prepared,
                    line_break,
                    out,
                )
            }
            ConfigEdit::InsertArrayItem {
                container,
                index,
                value,
            } => {
                let Some(container_id) = self.context.index.node_id(container) else {
                    return Err(out.fail(ConfigEditError::new(ConfigEditErrorKind::InvalidPath)));
                };
                let Some(analysis) = prepared.container(container_id, budget, out.reborrow())?
                else {
                    return Err(out.fail(ConfigEditError::new(ConfigEditErrorKind::InvalidPath)));
                };
                if analysis.kind() != PathValueKind::Array {
                    return Err(
                        out.fail(ConfigEditError::new(ConfigEditErrorKind::InvalidContainer))
                    );
                }
                self.insert_patch(
                    InsertionPlan {
                        analysis,
                        index: Some(*index),
                        value: InsertionValue::Value(value.root()),
                    },
                    prepared,
                    line_break,
                    out,
                )
            }
            ConfigEdit::Remove { path } => self.remove_patch(path, prepared, out),
        }
    }

    fn insert_patch<'slot>(
        &self,
        plan: InsertionPlan<'_, '_, '_, '_>,
        prepared: &PreparedEdits<'_>,
        line_break: &'static str,
        mut out: Out<'slot, '_, ConfigEditError>,
    ) -> OResult<'slot, SourcePatch> {
        let InsertionPlan {
            analysis,
            index,
            value,
        } = plan;
        let child_count = analysis.child_count();
        let index = index.unwrap_or(child_count);
        if index > child_count {
            return Err(out.fail(ConfigEditError::new(ConfigEditErrorKind::InvalidPath)));
        }
        let last_is_removed = if index == child_count {
            match analysis.last_span() {
                Some(last) => {
                    prepared.removes(analysis.id(), last, self.context.budget, out.reborrow())?
                }
                None => false,
            }
        } else {
            false
        };
        let append_after_removed_last = last_is_removed;
        let Some(layout) =
            analysis.layout_for_insertion(index, self.context.budget, out.reborrow())?
        else {
            return Err(out.fail(ConfigEditError::new(ConfigEditErrorKind::InvalidPath)));
        };
        let mut insertion = ChargedString::with_capacity(0, self.context.budget, out.reborrow())?;
        let details = if index < child_count {
            let Some(span) = analysis.child_span(index) else {
                return Err(out.fail(ConfigEditError::new(ConfigEditErrorKind::InvalidPath)));
            };
            InsertionDetails {
                position: span.start(),
                prefix_comma: false,
                prefix_space: false,
                prefix_line_start: false,
                suffix_comma: true,
                suffix_space: !layout.multiline,
                suffix_line_start: layout.multiline,
            }
        } else if let Some(last) = analysis.last_span() {
            if let Some(comma) = analysis.trailing_comma() {
                InsertionDetails {
                    position: comma + 1,
                    prefix_comma: false,
                    prefix_space: !layout.multiline,
                    prefix_line_start: layout.multiline,
                    suffix_comma: true,
                    suffix_space: false,
                    suffix_line_start: false,
                }
            } else {
                InsertionDetails {
                    position: last.end(),
                    prefix_comma: !append_after_removed_last,
                    prefix_space: !layout.multiline,
                    prefix_line_start: layout.multiline,
                    suffix_comma: false,
                    suffix_space: false,
                    suffix_line_start: false,
                }
            }
        } else {
            InsertionDetails {
                position: analysis.open() + 1,
                prefix_comma: false,
                prefix_space: false,
                prefix_line_start: layout.multiline,
                suffix_comma: false,
                suffix_space: false,
                suffix_line_start: false,
            }
        };
        self.append_spacing(
            &mut insertion,
            SpacingRequest {
                comma: details.prefix_comma,
                space: details.prefix_space,
                line_start: details.prefix_line_start,
                layout,
                limit: self.context.source_limit,
            },
        )
        .map_err(|kind| out.reborrow().fail(ConfigEditError::new(kind)))?;
        let context =
            if self.context.options.layout == TextLayout::Pretty && details.prefix_line_start {
                ValueEncodingContext {
                    layout: self.context.options.layout,
                    line_break,
                    indent: layout.indent,
                    indent_unit: layout.indent_unit,
                    indent_levels: layout.indent_levels,
                    depth: 0,
                }
            } else {
                self.value_context(details.position, Some(analysis.id()), line_break)
                    .map_err(|kind| out.reborrow().fail(ConfigEditError::new(kind)))?
            };
        match value {
            InsertionValue::Value(value) => {
                ScalarEncoder::encode_value_into(
                    &mut insertion,
                    value,
                    context,
                    self.context.budget,
                    self.context.source_limit,
                )
                .map_err(|kind| out.reborrow().fail(ConfigEditError::new(kind)))?;
            }
            InsertionValue::Entry {
                name,
                value,
                map_key,
            } => {
                ScalarEncoder::encode_entry_into(
                    &mut insertion,
                    EntryEncoding {
                        name,
                        value,
                        map_key,
                        colon_spacing: layout.colon_spacing,
                        context,
                        budget: self.context.budget,
                        limit: self.context.source_limit,
                    },
                )
                .map_err(|kind| out.reborrow().fail(ConfigEditError::new(kind)))?;
            }
        }
        self.append_spacing(
            &mut insertion,
            SpacingRequest {
                comma: details.suffix_comma,
                space: details.suffix_space,
                line_start: details.suffix_line_start,
                layout,
                limit: self.context.source_limit,
            },
        )
        .map_err(|kind| out.reborrow().fail(ConfigEditError::new(kind)))?;
        Ok(SourcePatch {
            start: details.position,
            end: details.position,
            encoded: insertion,
        })
    }

    fn remove_patch<'slot>(
        &self,
        path: &ConfigPath,
        prepared: &PreparedEdits<'_>,
        mut out: Out<'slot, '_, ConfigEditError>,
    ) -> OResult<'slot, SourcePatch> {
        let Some(target) = self.context.index.info(path) else {
            return Err(out.fail(ConfigEditError::new(ConfigEditErrorKind::InvalidPath)));
        };
        let Some(span) = target.span else {
            return Err(out.fail(ConfigEditError::new(ConfigEditErrorKind::ValueNotExplicit)));
        };
        let Some(container_id) = self.context.index.parent_id(path) else {
            return Err(out.fail(ConfigEditError::new(ConfigEditErrorKind::InvalidPath)));
        };
        let Some(container) =
            prepared.container(container_id, self.context.budget, out.reborrow())?
        else {
            return Err(out.fail(ConfigEditError::new(ConfigEditErrorKind::InvalidPath)));
        };
        let Some(item) = container.item_bounds(span, self.context.budget, out.reborrow())? else {
            return Err(out.fail(ConfigEditError::new(ConfigEditErrorKind::EditConflict)));
        };
        let preserve_tail = container.last_span() == Some(span)
            && prepared.appends(container_id, self.context.budget, out.reborrow())?;
        let (start, end) = if let Some(comma) = item.next_comma {
            (item.start, comma + 1)
        } else if let Some(comma) = item.previous_comma {
            (comma + 1, if preserve_tail { span.end() } else { item.end })
        } else {
            (
                item.start,
                if preserve_tail { span.end() } else { item.end },
            )
        };
        let comments =
            SourceSyntax::comments_in(self.context.source, start, end, self.context.budget)
                .map_err(|kind| out.reborrow().fail(ConfigEditError::new(kind)))?;
        Ok(SourcePatch {
            start,
            end,
            encoded: comments,
        })
    }

    fn append_spacing(
        &self,
        output: &mut ChargedString,
        request: SpacingRequest<'_>,
    ) -> Result<(), ConfigEditErrorKind> {
        let spacing_bytes = if request.line_start {
            request
                .layout
                .indent_unit
                .len()
                .checked_mul(request.layout.indent_levels)
                .and_then(|length| length.checked_add(request.layout.indent.len()))
                .and_then(|length| length.checked_add(request.layout.line_break.len()))
                .ok_or(ConfigEditErrorKind::ResourceLimitExceeded)?
        } else if request.space {
            1
        } else {
            0
        };
        let total_bytes = spacing_bytes
            .checked_add(usize::from(request.comma))
            .ok_or(ConfigEditErrorKind::ResourceLimitExceeded)?;
        let mut errors = ErrSlot::<BudgetError>::new();
        errors
            .try_call(|out| {
                if let Some(budget) = self.context.budget {
                    budget.consume(total_bytes, out)?;
                }
                Ok(())
            })
            .map_err(|caught| ConfigEditError::budget_kind(caught.take()))?;
        if request.comma {
            append_charged(output, ",", request.limit)?;
        }
        if request.line_start {
            append_charged(output, request.layout.line_break, request.limit)?;
            append_charged(output, request.layout.indent, request.limit)?;
            for _ in 0..request.layout.indent_levels {
                append_charged(output, request.layout.indent_unit, request.limit)?;
            }
        } else if request.space {
            append_charged(output, " ", request.limit)?;
        }
        Ok(())
    }

    fn value_context(
        &self,
        position: usize,
        fallback_container: Option<usize>,
        line_break: &'static str,
    ) -> Result<ValueEncodingContext<'_>, ConfigEditErrorKind> {
        if self.context.options.layout == TextLayout::Compact {
            return Ok(ValueEncodingContext {
                layout: TextLayout::Compact,
                line_break,
                indent: "",
                indent_unit: "",
                indent_levels: 0,
                depth: 0,
            });
        }
        let (indent, depth) = match SourceSyntax::indent_context(
            self.context.source,
            position,
            self.context.budget,
        ) {
            Ok(context) => context,
            Err(ConfigEditErrorKind::EditConflict) => (
                "",
                fallback_container.map_or(0, |id| self.context.index.container_depth(id)),
            ),
            Err(kind) => return Err(kind),
        };
        Ok(ValueEncodingContext {
            layout: self.context.options.layout,
            line_break,
            indent,
            indent_unit: "    ",
            indent_levels: 0,
            depth,
        })
    }
}

fn append_charged(
    output: &mut ChargedString,
    text: &str,
    limit: usize,
) -> Result<(), ConfigEditErrorKind> {
    if output
        .len()
        .checked_add(text.len())
        .filter(|length| *length <= limit)
        .is_none()
    {
        return Err(ConfigEditErrorKind::ResourceLimitExceeded);
    }
    let mut errors = ErrSlot::<BudgetError>::new();
    errors
        .try_call(|out| output.try_push_str(text, out))
        .map_err(|caught| ConfigEditError::budget_kind(caught.take()))?;
    Ok(())
}

fn binary_search_bound(length: usize) -> usize {
    (usize::BITS - length.leading_zeros()) as usize
}
