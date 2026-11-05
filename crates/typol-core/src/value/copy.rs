use crate::{
    BudgetError, ChargedVec, DiagnosticContext, ExecutionBudget, ValueError, diagnostic_context,
    value::{
        ConfigValueBuilder, NamedValue, ValueEntries, ValueId, ValueKind, ValuePayload,
        ValuePayloadView, ValueView,
    },
};
use opeo::{OResult, Out};

static VALUE_VIEW_ENTRY_NAME_MISSING: DiagnosticContext =
    diagnostic_context!("typol-core", "value-arena", "value-view-entry-name-missing",);
static VALUE_VIEW_CHILD_MISSING: DiagnosticContext =
    diagnostic_context!("typol-core", "value-arena", "value-view-child-missing",);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ValueCopyLimits {
    max_nesting_depth: usize,
}

impl ValueCopyLimits {
    pub const fn new(max_nesting_depth: usize) -> Self {
        Self { max_nesting_depth }
    }

    pub const fn max_nesting_depth(&self) -> usize {
        self.max_nesting_depth
    }
}

fn copy_invariant(context: &'static DiagnosticContext) -> ValueError {
    ValueError::from(BudgetError::invariant(context))
}

enum ValueCopyContainer<'a> {
    Optional,
    Array,
    Map,
    Structure,
    EnumerationSingle(&'a str),
    EnumerationStructure(&'a str),
}

impl ValueCopyContainer<'_> {
    const fn named(&self) -> bool {
        matches!(
            self,
            Self::Map | Self::Structure | Self::EnumerationStructure(_)
        )
    }
}

struct ValueCopySpec<'a> {
    value: ValueView<'a>,
    entries: ValueEntries<'a>,
    child_depth: usize,
    container: ValueCopyContainer<'a>,
    name: Option<&'a str>,
}

struct SingleValueCopySpec<'a> {
    value: ValueView<'a>,
    child: ValueView<'a>,
    child_depth: usize,
    container: ValueCopyContainer<'a>,
    name: Option<&'a str>,
}

struct CopiedValue<'a> {
    name: Option<&'a str>,
    id: ValueId,
}

enum ValueCopyFrame<'a> {
    Enter {
        value: ValueView<'a>,
        depth: usize,
        name: Option<&'a str>,
    },
    Finish {
        value: ValueView<'a>,
        start: usize,
        child_count: usize,
        container: ValueCopyContainer<'a>,
        name: Option<&'a str>,
    },
}

struct ValueCopyState<'a> {
    frames: ChargedVec<ValueCopyFrame<'a>>,
    values: ChargedVec<CopiedValue<'a>>,
}

pub(super) struct ValueViewCopier {
    budget: Option<ExecutionBudget>,
}

impl ValueViewCopier {
    pub(super) fn new(budget: Option<ExecutionBudget>) -> Self {
        Self { budget }
    }

    pub(super) fn copy_to<'slot, 'value>(
        &self,
        builder: &mut ConfigValueBuilder,
        value: ValueView<'value>,
        limits: ValueCopyLimits,
        mut out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ValueId> {
        let mut state = ValueCopyState {
            frames: ChargedVec::new(self.budget.as_ref(), out.reborrow())?,
            values: ChargedVec::new(self.budget.as_ref(), out.reborrow())?,
        };
        state.frames.try_push(
            ValueCopyFrame::Enter {
                value,
                depth: 0,
                name: None,
            },
            out.reborrow(),
        )?;
        while let Some(frame) = state.frames.pop() {
            match frame {
                ValueCopyFrame::Enter { value, depth, name } => {
                    Self::check_copy_depth(depth, limits, out.reborrow())?;
                    match value.as_ref() {
                        ValueKind::Signed(number) => {
                            let id = builder.add_signed(number, value.source(), out.reborrow())?;
                            state
                                .values
                                .try_push(CopiedValue { name, id }, out.reborrow())?;
                        }
                        ValueKind::Unsigned(number) => {
                            let id =
                                builder.add_unsigned(number, value.source(), out.reborrow())?;
                            state
                                .values
                                .try_push(CopiedValue { name, id }, out.reborrow())?;
                        }
                        ValueKind::Float32(number) => {
                            let id = builder.add_float32(number, value.source(), out.reborrow())?;
                            state
                                .values
                                .try_push(CopiedValue { name, id }, out.reborrow())?;
                        }
                        ValueKind::Float64(number) => {
                            let id = builder.add_float64(number, value.source(), out.reborrow())?;
                            state
                                .values
                                .try_push(CopiedValue { name, id }, out.reborrow())?;
                        }
                        ValueKind::String(text) => {
                            let id = builder.add_string(text, value.source(), out.reborrow())?;
                            state
                                .values
                                .try_push(CopiedValue { name, id }, out.reborrow())?;
                        }
                        ValueKind::Bool(boolean) => {
                            let id = builder.add_bool(boolean, value.source(), out.reborrow())?;
                            state
                                .values
                                .try_push(CopiedValue { name, id }, out.reborrow())?;
                        }
                        ValueKind::Optional(None) => {
                            let id = builder.add_optional(None, value.source(), out.reborrow())?;
                            state
                                .values
                                .try_push(CopiedValue { name, id }, out.reborrow())?;
                        }
                        ValueKind::Optional(Some(child)) => {
                            let child_depth = Self::next_copy_depth(depth, out.reborrow())?;
                            Self::check_copy_depth(child_depth, limits, out.reborrow())?;
                            self.schedule_single_copy(
                                &mut state,
                                SingleValueCopySpec {
                                    value,
                                    child,
                                    child_depth,
                                    container: ValueCopyContainer::Optional,
                                    name,
                                },
                                out.reborrow(),
                            )?;
                        }
                        ValueKind::Array(entries) => {
                            let child_depth = if entries.len() == 0 {
                                depth
                            } else {
                                let child_depth = Self::next_copy_depth(depth, out.reborrow())?;
                                Self::check_copy_depth(child_depth, limits, out.reborrow())?;
                                child_depth
                            };
                            self.schedule_copy_children(
                                &mut state,
                                ValueCopySpec {
                                    value,
                                    entries,
                                    child_depth,
                                    container: ValueCopyContainer::Array,
                                    name,
                                },
                                out.reborrow(),
                            )?;
                        }
                        ValueKind::Map(entries) => {
                            let child_depth = if entries.len() == 0 {
                                depth
                            } else {
                                let child_depth = Self::next_copy_depth(depth, out.reborrow())?;
                                Self::check_copy_depth(child_depth, limits, out.reborrow())?;
                                child_depth
                            };
                            self.schedule_copy_children(
                                &mut state,
                                ValueCopySpec {
                                    value,
                                    entries,
                                    child_depth,
                                    container: ValueCopyContainer::Map,
                                    name,
                                },
                                out.reborrow(),
                            )?;
                        }
                        ValueKind::Structure(entries) => {
                            let child_depth = if entries.len() == 0 {
                                depth
                            } else {
                                let child_depth = Self::next_copy_depth(depth, out.reborrow())?;
                                Self::check_copy_depth(child_depth, limits, out.reborrow())?;
                                child_depth
                            };
                            self.schedule_copy_children(
                                &mut state,
                                ValueCopySpec {
                                    value,
                                    entries,
                                    child_depth,
                                    container: ValueCopyContainer::Structure,
                                    name,
                                },
                                out.reborrow(),
                            )?;
                        }
                        ValueKind::Enumeration { variant, payload } => match payload {
                            ValuePayloadView::Unit => {
                                let id = builder.add_enumeration(
                                    variant,
                                    ValuePayload::Unit,
                                    value.source(),
                                    out.reborrow(),
                                )?;
                                state
                                    .values
                                    .try_push(CopiedValue { name, id }, out.reborrow())?;
                            }
                            ValuePayloadView::Single(child) => {
                                let child_depth = Self::next_copy_depth(depth, out.reborrow())?;
                                Self::check_copy_depth(child_depth, limits, out.reborrow())?;
                                self.schedule_single_copy(
                                    &mut state,
                                    SingleValueCopySpec {
                                        value,
                                        child,
                                        child_depth,
                                        container: ValueCopyContainer::EnumerationSingle(variant),
                                        name,
                                    },
                                    out.reborrow(),
                                )?;
                            }
                            ValuePayloadView::Structure(entries) => {
                                let payload_depth = Self::next_copy_depth(depth, out.reborrow())?;
                                Self::check_copy_depth(payload_depth, limits, out.reborrow())?;
                                let child_depth = if entries.len() == 0 {
                                    payload_depth
                                } else {
                                    let child_depth =
                                        Self::next_copy_depth(payload_depth, out.reborrow())?;
                                    Self::check_copy_depth(child_depth, limits, out.reborrow())?;
                                    child_depth
                                };
                                self.schedule_copy_children(
                                    &mut state,
                                    ValueCopySpec {
                                        value,
                                        entries,
                                        child_depth,
                                        container: ValueCopyContainer::EnumerationStructure(
                                            variant,
                                        ),
                                        name,
                                    },
                                    out.reborrow(),
                                )?;
                            }
                        },
                    }
                }
                ValueCopyFrame::Finish {
                    value,
                    start,
                    child_count,
                    container,
                    name,
                } => {
                    let end = start.checked_add(child_count).ok_or_else(|| {
                        out.reborrow()
                            .fail(ValueError::from(BudgetError::CapacityExceeded))
                    })?;
                    if end != state.values.len() {
                        return Err(out
                            .reborrow()
                            .fail(copy_invariant(&VALUE_VIEW_CHILD_MISSING)));
                    }
                    let children = state.values.as_slice().get(start..end).ok_or_else(|| {
                        out.reborrow()
                            .fail(copy_invariant(&VALUE_VIEW_CHILD_MISSING))
                    })?;
                    let id = match container {
                        ValueCopyContainer::Optional => {
                            let child = children.first().ok_or_else(|| {
                                out.reborrow()
                                    .fail(copy_invariant(&VALUE_VIEW_CHILD_MISSING))
                            })?;
                            builder.add_optional(Some(child.id), value.source(), out.reborrow())?
                        }
                        ValueCopyContainer::Array => {
                            let ids = self.copied_ids(children, out.reborrow())?;
                            builder.add_array(&ids, value.source(), out.reborrow())?
                        }
                        ValueCopyContainer::Map => {
                            let values = self.copied_named_values(children, out.reborrow())?;
                            builder.add_map(&values, value.source(), out.reborrow())?
                        }
                        ValueCopyContainer::Structure => {
                            let values = self.copied_named_values(children, out.reborrow())?;
                            builder.add_structure(&values, value.source(), out.reborrow())?
                        }
                        ValueCopyContainer::EnumerationSingle(variant) => {
                            let child = children.first().ok_or_else(|| {
                                out.reborrow()
                                    .fail(copy_invariant(&VALUE_VIEW_CHILD_MISSING))
                            })?;
                            builder.add_enumeration(
                                variant,
                                ValuePayload::Single(child.id),
                                value.source(),
                                out.reborrow(),
                            )?
                        }
                        ValueCopyContainer::EnumerationStructure(variant) => {
                            let fields = self.copied_named_values(children, out.reborrow())?;
                            builder.add_enumeration(
                                variant,
                                ValuePayload::Structure(&fields),
                                value.source(),
                                out.reborrow(),
                            )?
                        }
                    };
                    state.values.truncate(start);
                    state
                        .values
                        .try_push(CopiedValue { name, id }, out.reborrow())?;
                }
            }
        }
        if state.values.len() != 1 {
            return Err(out.fail(copy_invariant(&VALUE_VIEW_CHILD_MISSING)));
        }
        let copied = state
            .values
            .pop()
            .ok_or_else(|| out.fail(copy_invariant(&VALUE_VIEW_CHILD_MISSING)))?;
        Ok(copied.id)
    }

    fn schedule_single_copy<'slot, 'value>(
        &self,
        state: &mut ValueCopyState<'value>,
        spec: SingleValueCopySpec<'value>,
        mut out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ()> {
        let start = state.values.len();
        state.frames.try_push(
            ValueCopyFrame::Finish {
                value: spec.value,
                start,
                child_count: 1,
                container: spec.container,
                name: spec.name,
            },
            out.reborrow(),
        )?;
        state.frames.try_push(
            ValueCopyFrame::Enter {
                value: spec.child,
                depth: spec.child_depth,
                name: None,
            },
            out.reborrow(),
        )
    }

    fn schedule_copy_children<'slot, 'value>(
        &self,
        state: &mut ValueCopyState<'value>,
        spec: ValueCopySpec<'value>,
        mut out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ()> {
        let ValueCopySpec {
            value,
            entries,
            child_depth,
            container,
            name,
        } = spec;
        let named = container.named();
        let start = state.values.len();
        let child_count = entries.len();
        state.frames.try_push(
            ValueCopyFrame::Finish {
                value,
                start,
                child_count,
                container,
                name,
            },
            out.reborrow(),
        )?;
        for index in (0..child_count).rev() {
            let entry = entries.get(index).ok_or_else(|| {
                out.reborrow()
                    .fail(copy_invariant(&VALUE_VIEW_CHILD_MISSING))
            })?;
            let child_name = if named {
                Some(entry.name().ok_or_else(|| {
                    out.reborrow()
                        .fail(copy_invariant(&VALUE_VIEW_ENTRY_NAME_MISSING))
                })?)
            } else {
                None
            };
            state.frames.try_push(
                ValueCopyFrame::Enter {
                    value: entry.value(),
                    depth: child_depth,
                    name: child_name,
                },
                out.reborrow(),
            )?;
        }
        Ok(())
    }

    fn copied_ids<'slot, 'value>(
        &self,
        values: &[CopiedValue<'value>],
        mut out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ChargedVec<ValueId>> {
        let mut ids = ChargedVec::new(self.budget.as_ref(), out.reborrow())?;
        ids.try_reserve_exact(values.len(), out.reborrow())?;
        for value in values {
            ids.try_push(value.id, out.reborrow())?;
        }
        Ok(ids)
    }

    fn copied_named_values<'slot, 'value>(
        &self,
        values: &[CopiedValue<'value>],
        mut out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ChargedVec<NamedValue<'value>>> {
        let mut named = ChargedVec::new(self.budget.as_ref(), out.reborrow())?;
        named.try_reserve_exact(values.len(), out.reborrow())?;
        for value in values {
            let name = value.name.ok_or_else(|| {
                out.reborrow()
                    .fail(copy_invariant(&VALUE_VIEW_ENTRY_NAME_MISSING))
            })?;
            named.try_push(NamedValue::new(name, value.id), out.reborrow())?;
        }
        Ok(named)
    }

    fn next_copy_depth<'slot>(
        depth: usize,
        out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, usize> {
        depth
            .checked_add(1)
            .ok_or_else(|| out.fail(ValueError::from(BudgetError::CapacityExceeded)))
    }

    fn check_copy_depth<'slot>(
        depth: usize,
        limits: ValueCopyLimits,
        out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ()> {
        if depth > limits.max_nesting_depth() {
            return Err(out.fail(ValueError::from(BudgetError::Resource)));
        }
        Ok(())
    }
}
