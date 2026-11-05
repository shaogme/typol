use core::{
    error::Error,
    fmt::{self, Debug, Display, Formatter},
};
use opeo::{ErrSlot, OResult, Out};
use typol_core::{BudgetError, ChargedString, ChargedVec, ConfigValue, ExecutionBudget, Span};

use super::{
    encoding::{ConfigEditErrorKind, EntryEncoding, ScalarEncoder},
    path::{ConfigPath, ConfigPathSegment, ConfigReplacement, PathValueKind, SourcePathIndex},
    syntax::{SourceLayout, SourceSyntax},
};
use crate::ConfigParseErrors;

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

pub struct ConfigEditError {
    kind: ConfigEditErrorKind,
    validation: Option<ConfigParseErrors>,
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
}

struct ContainerInfo {
    span: Span,
    kind: PathValueKind,
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

struct InsertionRequest<'a> {
    container: &'a ConfigPath,
    kind: PathValueKind,
    children: &'a [Span],
    index: usize,
    encoded: ChargedString,
}

pub(super) struct DocumentEditEngine<'a> {
    context: EditContext<'a>,
}

impl ConfigEditError {
    pub const fn kind(&self) -> ConfigEditErrorKind {
        self.kind
    }

    pub fn validation_errors(&self) -> Option<&ConfigParseErrors> {
        self.validation.as_ref()
    }

    pub(super) fn new(kind: ConfigEditErrorKind) -> Self {
        Self {
            kind,
            validation: None,
        }
    }

    pub(super) fn validation(errors: ConfigParseErrors) -> Self {
        Self {
            kind: ConfigEditErrorKind::ValidationFailed,
            validation: Some(errors),
        }
    }

    fn from_budget(error: BudgetError) -> Self {
        Self::new(edit_budget_error(error))
    }
}

impl From<BudgetError> for ConfigEditError {
    fn from(error: BudgetError) -> Self {
        Self::from_budget(error)
    }
}

impl Debug for ConfigEditError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConfigEditError")
            .field("kind", &self.kind)
            .field(
                "validation_diagnostic_count",
                &self
                    .validation
                    .as_ref()
                    .map(|errors| errors.diagnostics().len()),
            )
            .finish()
    }
}

impl Display for ConfigEditError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.kind {
            ConfigEditErrorKind::InvalidPath => "configuration edit path is invalid",
            ConfigEditErrorKind::ValueNotExplicit => {
                "configuration edit path has no explicit source value"
            }
            ConfigEditErrorKind::EncodingFailed => "configuration value encoding failed",
            ConfigEditErrorKind::EditConflict => "configuration edits overlap in source",
            ConfigEditErrorKind::InvalidContainer => {
                "configuration edit path does not name a supported container"
            }
            ConfigEditErrorKind::EntryAlreadyExists => "configuration edit entry already exists",
            ConfigEditErrorKind::ValidationFailed => "edited configuration failed validation",
            ConfigEditErrorKind::ResourceLimitExceeded => {
                "configuration edit resource limit exceeded"
            }
            ConfigEditErrorKind::AllocationFailed => "configuration edit allocation failed",
        })
    }
}

impl Error for ConfigEditError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.validation
            .as_ref()
            .map(|errors| errors as &(dyn Error + 'static))
    }
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
        let mut patches = ChargedVec::new(self.context.budget, out.reborrow())?;
        for edit in edits {
            patches.try_push(self.resolve_edit(edit, out.reborrow())?, out.reborrow())?;
        }
        Ok(patches)
    }

    fn resolve_edit<'slot>(
        &self,
        edit: &ConfigEdit<'_>,
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
                let mut encoded =
                    ScalarEncoder::encode_value(replacement.value().root(), budget, limit)
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
                let Some(parent) = path.parent_path() else {
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
                let colon_spacing =
                    self.entry_colon_spacing(&parent, PathValueKind::Structure, out.reborrow())?;
                let encoded = ScalarEncoder::encode_entry(EntryEncoding {
                    name,
                    value: value.root(),
                    map_key: false,
                    colon_spacing,
                    budget,
                    limit,
                })
                .map_err(|kind| out.reborrow().fail(ConfigEditError::new(kind)))?;
                self.insert_patch(&parent, PathValueKind::Structure, encoded, out)
            }
            ConfigEdit::InsertMapEntry {
                container,
                key,
                value,
            } => {
                let entry_path = container.pushed(ConfigPathSegment::MapKey((*key).into()));
                if self
                    .context
                    .index
                    .info(&entry_path)
                    .is_some_and(|node| node.span.is_some())
                {
                    return Err(out.fail(ConfigEditError::new(
                        ConfigEditErrorKind::EntryAlreadyExists,
                    )));
                }
                let colon_spacing =
                    self.entry_colon_spacing(container, PathValueKind::Map, out.reborrow())?;
                let encoded = ScalarEncoder::encode_entry(EntryEncoding {
                    name: key,
                    value: value.root(),
                    map_key: true,
                    colon_spacing,
                    budget,
                    limit,
                })
                .map_err(|kind| out.reborrow().fail(ConfigEditError::new(kind)))?;
                self.insert_patch(container, PathValueKind::Map, encoded, out)
            }
            ConfigEdit::InsertArrayItem {
                container,
                index,
                value,
            } => {
                let encoded = ScalarEncoder::encode_value(value.root(), budget, limit)
                    .map_err(|kind| out.reborrow().fail(ConfigEditError::new(kind)))?;
                self.insert_array_patch(container, *index, encoded, out)
            }
            ConfigEdit::Remove { path } => self.remove_patch(path, out),
        }
    }

    fn insert_patch<'slot>(
        &self,
        container: &ConfigPath,
        kind: PathValueKind,
        encoded: ChargedString,
        mut out: Out<'slot, '_, ConfigEditError>,
    ) -> OResult<'slot, SourcePatch> {
        let children = self.child_spans(container, kind, out.reborrow())?;
        self.insert_patch_from_children(
            InsertionRequest {
                container,
                kind,
                children: children.as_slice(),
                index: children.len(),
                encoded,
            },
            out,
        )
    }

    fn insert_array_patch<'slot>(
        &self,
        container: &ConfigPath,
        index: usize,
        encoded: ChargedString,
        mut out: Out<'slot, '_, ConfigEditError>,
    ) -> OResult<'slot, SourcePatch> {
        let children = self.child_spans(container, PathValueKind::Array, out.reborrow())?;
        if index > children.len() {
            return Err(out.fail(ConfigEditError::new(ConfigEditErrorKind::InvalidPath)));
        }
        self.insert_patch_from_children(
            InsertionRequest {
                container,
                kind: PathValueKind::Array,
                children: children.as_slice(),
                index,
                encoded,
            },
            out,
        )
    }

    fn insert_patch_from_children<'slot>(
        &self,
        request: InsertionRequest<'_>,
        mut out: Out<'slot, '_, ConfigEditError>,
    ) -> OResult<'slot, SourcePatch> {
        let InsertionRequest {
            container,
            kind,
            children,
            index,
            encoded,
        } = request;
        let info = self.container_info(container, kind, out.reborrow())?;
        let bounds = SourceSyntax::scan_container(self.context.source, info.span, kind)
            .map_err(|kind| out.reborrow().fail(ConfigEditError::new(kind)))?;
        let reference = children.first().copied();
        let layout = SourceSyntax::layout(self.context.source, bounds, reference)
            .map_err(|kind| out.reborrow().fail(ConfigEditError::new(kind)))?;
        let mut insertion = ChargedString::with_capacity(0, self.context.budget, out.reborrow())?;
        let details = if index < children.len() {
            let Some(span) = children.get(index) else {
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
        } else if let Some(last) = children.last() {
            let comma = SourceSyntax::trailing_comma(self.context.source, bounds, last.end())
                .map_err(|kind| out.reborrow().fail(ConfigEditError::new(kind)))?;
            if let Some(comma) = comma {
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
                    prefix_comma: true,
                    prefix_space: !layout.multiline,
                    prefix_line_start: layout.multiline,
                    suffix_comma: false,
                    suffix_space: false,
                    suffix_line_start: false,
                }
            }
        } else {
            InsertionDetails {
                position: bounds.open + 1,
                prefix_comma: false,
                prefix_space: false,
                prefix_line_start: layout.multiline,
                suffix_comma: false,
                suffix_space: false,
                suffix_line_start: false,
            }
        };
        Self::append_spacing(
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
        append_charged(&mut insertion, encoded.as_str(), self.context.source_limit)
            .map_err(|kind| out.reborrow().fail(ConfigEditError::new(kind)))?;
        Self::append_spacing(
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
        mut out: Out<'slot, '_, ConfigEditError>,
    ) -> OResult<'slot, SourcePatch> {
        let Some(target) = self.context.index.info(path) else {
            return Err(out.fail(ConfigEditError::new(ConfigEditErrorKind::InvalidPath)));
        };
        let Some(span) = target.span else {
            return Err(out.fail(ConfigEditError::new(ConfigEditErrorKind::ValueNotExplicit)));
        };
        let Some(container) = path.parent_path() else {
            return Err(out.fail(ConfigEditError::new(ConfigEditErrorKind::InvalidPath)));
        };
        let container_info = self.container_info_any(&container, out.reborrow())?;
        let bounds = SourceSyntax::scan_container(
            self.context.source,
            container_info.span,
            container_info.kind,
        )
        .map_err(|kind| out.reborrow().fail(ConfigEditError::new(kind)))?;
        let item = SourceSyntax::locate_item(self.context.source, bounds, span)
            .map_err(|kind| out.reborrow().fail(ConfigEditError::new(kind)))?;
        let (start, end) = if let Some(comma) = item.next_comma {
            (item.start, comma + 1)
        } else if let Some(comma) = item.previous_comma {
            (comma + 1, item.end)
        } else {
            (item.start, item.end)
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

    fn container_info<'slot>(
        &self,
        path: &ConfigPath,
        kind: PathValueKind,
        out: Out<'slot, '_, ConfigEditError>,
    ) -> OResult<'slot, ContainerInfo> {
        let Some(info) = self.context.index.info(path) else {
            return Err(out.fail(ConfigEditError::new(ConfigEditErrorKind::InvalidPath)));
        };
        if info.kind != kind {
            return Err(out.fail(ConfigEditError::new(ConfigEditErrorKind::InvalidContainer)));
        }
        let Some(span) = info.span else {
            return Err(out.fail(ConfigEditError::new(ConfigEditErrorKind::ValueNotExplicit)));
        };
        Ok(ContainerInfo { span, kind })
    }

    fn container_info_any<'slot>(
        &self,
        path: &ConfigPath,
        out: Out<'slot, '_, ConfigEditError>,
    ) -> OResult<'slot, ContainerInfo> {
        let Some(info) = self.context.index.info(path) else {
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
        Ok(ContainerInfo {
            span,
            kind: info.kind,
        })
    }

    fn child_spans<'slot>(
        &self,
        container: &ConfigPath,
        kind: PathValueKind,
        mut out: Out<'slot, '_, ConfigEditError>,
    ) -> OResult<'slot, ChargedVec<Span>> {
        self.container_info(container, kind, out.reborrow())?;
        let Some(children) = self.context.index.direct_children(container) else {
            return Err(out.fail(ConfigEditError::new(ConfigEditErrorKind::InvalidPath)));
        };
        let mut spans = ChargedVec::new(self.context.budget, out.reborrow())?;
        for child in children {
            if let Some(span) = child.span {
                spans.try_push(span, out.reborrow())?;
            }
        }
        spans.sort_unstable_by_key(|span| span.start());
        Ok(spans)
    }

    fn entry_colon_spacing<'slot>(
        &self,
        container: &ConfigPath,
        kind: PathValueKind,
        mut out: Out<'slot, '_, ConfigEditError>,
    ) -> OResult<'slot, &'a str> {
        let info = self.container_info(container, kind, out.reborrow())?;
        let bounds = SourceSyntax::scan_container(self.context.source, info.span, kind)
            .map_err(|kind| out.reborrow().fail(ConfigEditError::new(kind)))?;
        let reference = self
            .context
            .index
            .direct_children(container)
            .and_then(|mut children| children.find_map(|child| child.span));
        let layout = SourceSyntax::layout(self.context.source, bounds, reference)
            .map_err(|kind| out.reborrow().fail(ConfigEditError::new(kind)))?;
        Ok(layout.colon_spacing)
    }

    fn append_spacing(
        output: &mut ChargedString,
        request: SpacingRequest<'_>,
    ) -> Result<(), ConfigEditErrorKind> {
        if request.comma {
            append_charged(output, ",", request.limit)?;
        }
        if request.line_start {
            append_charged(output, request.layout.line_break, request.limit)?;
            append_charged(output, request.layout.indent, request.limit)?;
        } else if request.space {
            append_charged(output, " ", request.limit)?;
        }
        Ok(())
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
        .map_err(|caught| edit_budget_error(caught.take()))?;
    Ok(())
}

fn edit_budget_error(error: BudgetError) -> ConfigEditErrorKind {
    match error {
        BudgetError::Resource | BudgetError::CapacityExceeded => {
            ConfigEditErrorKind::ResourceLimitExceeded
        }
        BudgetError::Allocation | BudgetError::InvariantViolation(_) => {
            ConfigEditErrorKind::AllocationFailed
        }
    }
}
