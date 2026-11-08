use crate::defaults::frames::NamedDefault;
use opeo::{OResult, Out, ResultOutExt};
use typol_core::{
    BudgetError, ChargedVec, ConfigValue, ConfigValueBuilder, DiagnosticContext,
    DiagnosticContextDescriptor, ExecutionBudget, NamedValue, ValueError, ValueId, ValueKind,
    ValuePayload, ValuePayloadView, ValueSource, ValueView,
};

static INVALID_VALUE_ENTRY: DiagnosticContextDescriptor = DiagnosticContextDescriptor::new(
    "typol-schema",
    "default-value-transfer",
    "invalid-value-entry",
);
static INVALID_VALUE_ENTRY_CONTEXT: DiagnosticContext =
    DiagnosticContext::new(&INVALID_VALUE_ENTRY);

enum CopyTask<'a> {
    Enter(ValueView<'a>),
    Finish(ValueView<'a>, usize),
}

pub(super) enum EnumPayload<'a> {
    Unit,
    Single(ChargedVec<ConfigValue>),
    Structure(ChargedVec<NamedDefault<'a>>),
}

pub(super) struct ValueTransfer;

impl ValueTransfer {
    pub(super) fn optional<'slot>(
        builder: &mut ConfigValueBuilder,
        value: Option<&ConfigValue>,
        budget: Option<&ExecutionBudget>,
        mut out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ValueId> {
        let child = match value {
            Some(value) => Some(Self::copy_into(
                builder,
                value.root(),
                budget,
                out.reborrow(),
            )?),
            None => None,
        };
        builder.add_optional(child, ValueSource::SchemaDefault, out)
    }

    pub(super) fn array<'slot>(
        builder: &mut ConfigValueBuilder,
        values: &[ConfigValue],
        budget: Option<&ExecutionBudget>,
        mut out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ValueId> {
        let mut children = ChargedVec::new(budget, out.reborrow())?;
        children.try_reserve_exact(values.len(), out.reborrow())?;
        for value in values {
            let child = Self::copy_into(builder, value.root(), budget, out.reborrow())?;
            children.try_push(child, out.reborrow())?;
        }
        builder.add_array(&children, ValueSource::SchemaDefault, out)
    }

    pub(super) fn named<'slot>(
        builder: &mut ConfigValueBuilder,
        values: &[NamedDefault<'_>],
        structure: bool,
        budget: Option<&ExecutionBudget>,
        mut out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ValueId> {
        let mut children = ChargedVec::new(budget, out.reborrow())?;
        children.try_reserve_exact(values.len(), out.reborrow())?;
        for value in values {
            let child = Self::copy_into(builder, value.value.root(), budget, out.reborrow())?;
            children.try_push(NamedValue::new(value.name, child), out.reborrow())?;
        }
        if structure {
            builder.add_structure(&children, ValueSource::SchemaDefault, out)
        } else {
            builder.add_map(&children, ValueSource::SchemaDefault, out)
        }
    }

    pub(super) fn enumeration<'slot>(
        builder: &mut ConfigValueBuilder,
        variant: &str,
        payload: EnumPayload<'_>,
        budget: Option<&ExecutionBudget>,
        mut out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ValueId> {
        match payload {
            EnumPayload::Unit => builder.add_enumeration(
                variant,
                ValuePayload::Unit,
                ValueSource::SchemaDefault,
                out,
            ),
            EnumPayload::Single(mut values) => {
                let Some(value) = values.pop() else {
                    return Err(out.fail(invalid_value_entry()));
                };
                if !values.is_empty() {
                    return Err(out.fail(invalid_value_entry()));
                }
                let child = Self::copy_into(builder, value.root(), budget, out.reborrow())?;
                builder.add_enumeration(
                    variant,
                    ValuePayload::Single(child),
                    ValueSource::SchemaDefault,
                    out,
                )
            }
            EnumPayload::Structure(values) => {
                let mut children = ChargedVec::new(budget, out.reborrow())?;
                children.try_reserve_exact(values.len(), out.reborrow())?;
                for value in &values {
                    let child =
                        Self::copy_into(builder, value.value.root(), budget, out.reborrow())?;
                    children.try_push(NamedValue::new(value.name, child), out.reborrow())?;
                }
                builder.add_enumeration(
                    variant,
                    ValuePayload::Structure(&children),
                    ValueSource::SchemaDefault,
                    out,
                )
            }
        }
    }

    pub(super) fn copy_into<'slot>(
        builder: &mut ConfigValueBuilder,
        root: ValueView<'_>,
        budget: Option<&ExecutionBudget>,
        mut out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ValueId> {
        let mut tasks = ChargedVec::new(budget, out.reborrow())?;
        let mut values = ChargedVec::new(budget, out.reborrow())?;
        tasks.try_push(CopyTask::Enter(root), out.reborrow())?;
        while let Some(task) = tasks.pop() {
            if let Some(budget) = budget {
                budget.consume(1, out.reborrow())?;
            }
            match task {
                CopyTask::Enter(value) => {
                    let start = values.len();
                    let children = child_count(value);
                    tasks.try_reserve(children.saturating_add(1), out.reborrow())?;
                    tasks.try_push(CopyTask::Finish(value, start), out.reborrow())?;
                    push_children(value, &mut tasks, out.reborrow())?;
                }
                CopyTask::Finish(value, start) => {
                    let child_count = values
                        .len()
                        .checked_sub(start)
                        .ok_or_else(invalid_value_entry)
                        .store_err_into(out.reborrow())?;
                    let mut children = ChargedVec::new(budget, out.reborrow())?;
                    children.try_reserve_exact(child_count, out.reborrow())?;
                    while values.len() > start {
                        let child = values
                            .pop()
                            .ok_or_else(invalid_value_entry)
                            .store_err_into(out.reborrow())?;
                        children.try_push(child, out.reborrow())?;
                    }
                    let mut children = children.into_iter();
                    let copied = copy_node(builder, value, &mut children, budget, out.reborrow())?;
                    values.try_push(copied, out.reborrow())?;
                }
            }
        }
        values
            .pop()
            .ok_or_else(invalid_value_entry)
            .store_err_into(out)
    }
}

fn child_count(value: ValueView<'_>) -> usize {
    match value.as_ref() {
        ValueKind::Optional(Some(_)) => 1,
        ValueKind::Array(entries) | ValueKind::Map(entries) | ValueKind::Structure(entries) => {
            entries.len()
        }
        ValueKind::Enumeration { payload, .. } => match payload {
            ValuePayloadView::Unit => 0,
            ValuePayloadView::Single(_) => 1,
            ValuePayloadView::Structure(fields) => fields.len(),
        },
        _ => 0,
    }
}

fn push_children<'slot, 'value>(
    value: ValueView<'value>,
    tasks: &mut ChargedVec<CopyTask<'value>>,
    mut out: Out<'slot, '_, ValueError>,
) -> OResult<'slot, ()> {
    match value.as_ref() {
        ValueKind::Optional(Some(child)) => tasks.try_push(CopyTask::Enter(child), out),
        ValueKind::Array(entries) | ValueKind::Map(entries) | ValueKind::Structure(entries) => {
            for entry in entries {
                tasks.try_push(CopyTask::Enter(entry.value()), out.reborrow())?;
            }
            Ok(())
        }
        ValueKind::Enumeration { payload, .. } => match payload {
            ValuePayloadView::Unit => Ok(()),
            ValuePayloadView::Single(child) => tasks.try_push(CopyTask::Enter(child), out),
            ValuePayloadView::Structure(fields) => {
                for field in fields {
                    tasks.try_push(CopyTask::Enter(field.value()), out.reborrow())?;
                }
                Ok(())
            }
        },
        _ => Ok(()),
    }
}

fn copy_node<'slot, 'value>(
    builder: &mut ConfigValueBuilder,
    value: ValueView<'value>,
    children: &mut impl Iterator<Item = ValueId>,
    budget: Option<&ExecutionBudget>,
    mut out: Out<'slot, '_, ValueError>,
) -> OResult<'slot, ValueId> {
    let source = value.source();
    match value.as_ref() {
        ValueKind::Signed(number) => builder.add_signed(number, source, out),
        ValueKind::Unsigned(number) => builder.add_unsigned(number, source, out),
        ValueKind::Float32(number) => builder.add_float32(number, source, out),
        ValueKind::Float64(number) => builder.add_float64(number, source, out),
        ValueKind::Bool(boolean) => builder.add_bool(boolean, source, out),
        ValueKind::String(text) => builder.add_string(text, source, out),
        ValueKind::Optional(child) => {
            let child = match child {
                Some(_) => Some(
                    children
                        .next()
                        .ok_or_else(invalid_value_entry)
                        .store_err_into(out.reborrow())?,
                ),
                None => None,
            };
            builder.add_optional(child, source, out)
        }
        ValueKind::Array(entries) => {
            let mut child_ids = ChargedVec::new(budget, out.reborrow())?;
            child_ids.try_reserve_exact(entries.len(), out.reborrow())?;
            for child in children.by_ref() {
                child_ids.try_push(child, out.reborrow())?;
            }
            builder.add_array(&child_ids, source, out)
        }
        ValueKind::Map(entries) | ValueKind::Structure(entries) => {
            let is_map = matches!(value.as_ref(), ValueKind::Map(_));
            let mut named = ChargedVec::new(budget, out.reborrow())?;
            named.try_reserve_exact(entries.len(), out.reborrow())?;
            for entry in entries {
                let name = entry
                    .name()
                    .ok_or_else(invalid_value_entry)
                    .store_err_into(out.reborrow())?;
                let child = children
                    .next()
                    .ok_or_else(invalid_value_entry)
                    .store_err_into(out.reborrow())?;
                named.try_push(NamedValue::new(name, child), out.reborrow())?;
            }
            if is_map {
                builder.add_map(&named, source, out)
            } else {
                builder.add_structure(&named, source, out)
            }
        }
        ValueKind::Enumeration { variant, payload } => {
            let payload = match payload {
                ValuePayloadView::Unit => ValuePayload::Unit,
                ValuePayloadView::Single(_) => ValuePayload::Single(
                    children
                        .next()
                        .ok_or_else(invalid_value_entry)
                        .store_err_into(out.reborrow())?,
                ),
                ValuePayloadView::Structure(fields) => {
                    let mut named = ChargedVec::new(budget, out.reborrow())?;
                    named.try_reserve_exact(fields.len(), out.reborrow())?;
                    for field in fields {
                        let name = field
                            .name()
                            .ok_or_else(invalid_value_entry)
                            .store_err_into(out.reborrow())?;
                        let child = children
                            .next()
                            .ok_or_else(invalid_value_entry)
                            .store_err_into(out.reborrow())?;
                        named.try_push(NamedValue::new(name, child), out.reborrow())?;
                    }
                    return builder.add_enumeration(
                        variant,
                        ValuePayload::Structure(&named),
                        source,
                        out,
                    );
                }
            };
            builder.add_enumeration(variant, payload, source, out)
        }
    }
}

fn invalid_value_entry() -> ValueError {
    ValueError::from(BudgetError::invariant(&INVALID_VALUE_ENTRY_CONTEXT))
}
