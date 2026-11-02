use opeo::{ErrSlot, OResult, Out, ResultOutExt};
use typol_core::{
    BudgetError, ChargedVec, ConfigValue, ConfigValueBuilder, DiagnosticContext, ExecutionBudget,
    NamedValue, ValueError, ValueErrorKind, ValueKind, ValuePayload, ValuePayloadView, ValueView,
    diagnostic_context,
};
use typol_policy::ValueLimits;

static COPY_INTERNAL: DiagnosticContext =
    diagnostic_context!("typol-semantic", "value-copy", "copy-invariant-failed",);
static COPY_VALUE_INTERNAL: DiagnosticContext = diagnostic_context!(
    "typol-semantic",
    "value-copy",
    "copied-value-invariant-failed",
);
static COPY_DUPLICATE_MAP_KEY: DiagnosticContext =
    diagnostic_context!("typol-semantic", "value-copy", "duplicate-map-key",);
static COPY_DUPLICATE_FIELD_NAME: DiagnosticContext =
    diagnostic_context!("typol-semantic", "value-copy", "duplicate-field-name",);

enum Task<'a> {
    Enter(ValueView<'a>, usize),
    Finish(ValueView<'a>, usize),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CopyError {
    Resource,
    Allocation,
    Capacity,
    Internal,
    InternalAt(DiagnosticContext),
    InvalidValue(ValueError),
}

impl CopyError {
    pub const fn internal(context: DiagnosticContext) -> Self {
        Self::InternalAt(context)
    }

    pub const fn context(self) -> Option<DiagnosticContext> {
        match self {
            Self::InternalAt(context) => Some(context),
            Self::InvalidValue(error) => match error.context() {
                Some(context) => Some(context),
                None => {
                    let context = match error.kind() {
                        ValueErrorKind::DuplicateMapKey => COPY_DUPLICATE_MAP_KEY,
                        ValueErrorKind::DuplicateFieldName => COPY_DUPLICATE_FIELD_NAME,
                        _ => COPY_VALUE_INTERNAL,
                    };
                    Some(match error.duplicate_indices() {
                        Some((_, second)) => context.with_object_id(second as u64),
                        None => context,
                    })
                }
            },
            Self::Resource | Self::Allocation | Self::Capacity | Self::Internal => None,
        }
    }

    pub const fn is_internal(self) -> bool {
        matches!(
            self,
            Self::Internal | Self::InternalAt(_) | Self::InvalidValue(_)
        )
    }

    pub const fn value_error(self) -> Option<ValueError> {
        match self {
            Self::InvalidValue(error) => Some(error),
            _ => None,
        }
    }
}

pub struct CopyOptions<'a> {
    enum_structure_depth: usize,
    budget: Option<&'a ExecutionBudget>,
    max_text_bytes: usize,
    value_errors: ErrSlot<ValueError>,
}

pub struct CopyRequest<'a> {
    root: &'a ConfigValue,
    depth: usize,
    nodes: &'a mut usize,
    limits: &'a ValueLimits,
    options: CopyOptions<'a>,
}

fn reserve<'slot, T>(
    buffer: &mut ChargedVec<T>,
    count: usize,
    mut out: Out<'slot, '_, CopyError>,
) -> OResult<'slot, ()> {
    buffer.try_reserve(count, out.reborrow())
}

fn count_node(nodes: &mut usize, limits: &ValueLimits) -> Result<(), CopyError> {
    if *nodes >= limits.max_value_nodes() {
        return Err(CopyError::Resource);
    }
    *nodes += 1;
    Ok(())
}

fn check_text<'slot>(
    text: &str,
    limits: &ValueLimits,
    options: &CopyOptions<'_>,
    mut out: Out<'slot, '_, CopyError>,
) -> OResult<'slot, ()> {
    if text.len() > limits.max_string_bytes() {
        return Err(out.fail(CopyError::Resource));
    }
    if let Some(budget) = options.budget {
        budget.construct_text(text.len(), options.max_text_bytes, out.reborrow())?;
    }
    Ok(())
}

fn value_error(error: ValueError) -> CopyError {
    match error.kind() {
        ValueErrorKind::ResourceLimitExceeded => CopyError::Resource,
        ValueErrorKind::AllocationFailed => CopyError::Allocation,
        ValueErrorKind::CapacityExceeded => CopyError::Capacity,
        ValueErrorKind::InternalInvariantFailure
        | ValueErrorKind::DuplicateMapKey
        | ValueErrorKind::DuplicateFieldName => CopyError::InvalidValue(error),
    }
}

impl From<BudgetError> for CopyError {
    fn from(error: BudgetError) -> Self {
        match error {
            BudgetError::Resource => Self::Resource,
            BudgetError::Allocation => Self::Allocation,
            BudgetError::CapacityExceeded => Self::Capacity,
            BudgetError::InvariantViolation(context) => Self::internal(*context),
        }
    }
}

impl<'a> CopyRequest<'a> {
    pub fn new(
        root: &'a ConfigValue,
        depth: usize,
        nodes: &'a mut usize,
        limits: &'a ValueLimits,
        options: CopyOptions<'a>,
    ) -> Self {
        Self {
            root,
            depth,
            nodes,
            limits,
            options,
        }
    }
}

impl<'a> CopyOptions<'a> {
    pub fn new(
        enum_structure_depth: usize,
        budget: Option<&'a ExecutionBudget>,
        max_text_bytes: usize,
    ) -> Self {
        Self {
            enum_structure_depth,
            budget,
            max_text_bytes,
            value_errors: ErrSlot::new(),
        }
    }

    fn with_value_error<T>(
        &mut self,
        operation: impl for<'slot> FnOnce(Out<'slot, 'slot, ValueError>) -> OResult<'slot, T>,
    ) -> Result<T, CopyError> {
        match self.value_errors.try_call(operation) {
            Ok(value) => Ok(value),
            Err(caught) => Err(value_error(caught.take())),
        }
    }
}

pub struct ValueCopy;

impl ValueCopy {
    /// Each value and map key counts toward the caller’s cumulative node total; failed attempts do not refund the count.
    /// `enum_structure_depth` is the additional depth the caller charges for anonymous enum payload structure.
    pub fn copy_value<'slot>(
        request: CopyRequest<'_>,
        mut out: Out<'slot, '_, CopyError>,
    ) -> OResult<'slot, ConfigValue> {
        let CopyRequest {
            root,
            depth,
            nodes,
            limits,
            mut options,
        } = request;
        let budget = options.budget;
        let mut builder = options
            .with_value_error(|value_out| ConfigValueBuilder::new(budget, value_out))
            .store_err_into(out.reborrow())?;
        let mut tasks = ChargedVec::new(options.budget, out.reborrow())?;
        let mut values = ChargedVec::new(options.budget, out.reborrow())?;
        reserve(&mut tasks, 1, out.reborrow())?;
        tasks.try_push(Task::Enter(root.root(), depth), out.reborrow())?;
        while let Some(task) = tasks.pop() {
            if let Some(budget) = options.budget {
                budget.consume(1, out.reborrow())?;
            }
            match task {
                Task::Enter(value, depth) => {
                    if !limits.allows_depth(depth) {
                        return Err(out.fail(CopyError::Resource));
                    }
                    count_node(nodes, limits).store_err_into(out.reborrow())?;
                    let start = values.len();
                    let children = match value.as_ref() {
                        ValueKind::Optional(Some(_)) => 1,
                        ValueKind::Array(entries)
                        | ValueKind::Map(entries)
                        | ValueKind::Structure(entries) => entries.len(),
                        ValueKind::Enumeration { payload, .. } => match payload {
                            ValuePayloadView::Unit => 0,
                            ValuePayloadView::Single(_) => 1,
                            ValuePayloadView::Structure(fields) => fields.len(),
                        },
                        _ => 0,
                    };
                    let capacity = children
                        .checked_add(1)
                        .ok_or(CopyError::Resource)
                        .store_err_into(out.reborrow())?;
                    tasks.try_reserve(capacity, out.reborrow())?;
                    tasks.try_push(Task::Finish(value, start), out.reborrow())?;
                    let next_depth = depth
                        .checked_add(1)
                        .ok_or(CopyError::Resource)
                        .store_err_into(out.reborrow())?;
                    match value.as_ref() {
                        ValueKind::Optional(Some(child)) => {
                            tasks.try_push(Task::Enter(child, next_depth), out.reborrow())?;
                        }
                        ValueKind::Array(entries) => {
                            for entry in entries {
                                if let Some(budget) = options.budget {
                                    budget.consume(1, out.reborrow())?;
                                }
                                tasks.try_push(
                                    Task::Enter(entry.value(), next_depth),
                                    out.reborrow(),
                                )?;
                            }
                        }
                        ValueKind::Map(entries) | ValueKind::Structure(entries) => {
                            for entry in entries {
                                if let Some(budget) = options.budget {
                                    budget.consume(1, out.reborrow())?;
                                }
                                if matches!(value.as_ref(), ValueKind::Map(_)) {
                                    count_node(nodes, limits).store_err_into(out.reborrow())?;
                                }
                                tasks.try_push(
                                    Task::Enter(entry.value(), next_depth),
                                    out.reborrow(),
                                )?;
                            }
                        }
                        ValueKind::Enumeration { payload, .. } => match payload {
                            ValuePayloadView::Unit => {}
                            ValuePayloadView::Single(child) => {
                                tasks.try_push(Task::Enter(child, next_depth), out.reborrow())?;
                            }
                            ValuePayloadView::Structure(fields) => {
                                let next_depth = next_depth
                                    .checked_add(options.enum_structure_depth)
                                    .ok_or(CopyError::Resource)
                                    .store_err_into(out.reborrow())?;
                                for field in fields {
                                    if let Some(budget) = options.budget {
                                        budget.consume(1, out.reborrow())?;
                                    }
                                    tasks.try_push(
                                        Task::Enter(field.value(), next_depth),
                                        out.reborrow(),
                                    )?;
                                }
                            }
                        },
                        _ => {}
                    }
                }
                Task::Finish(value, start) => {
                    let child_count = values
                        .len()
                        .checked_sub(start)
                        .ok_or(CopyError::internal(COPY_INTERNAL))
                        .store_err_into(out.reborrow())?;
                    let mut children = ChargedVec::new(options.budget, out.reborrow())?;
                    children.try_reserve_exact(child_count, out.reborrow())?;
                    while values.len() > start {
                        if let Some(budget) = options.budget {
                            budget.consume(1, out.reborrow())?;
                        }
                        let child = values
                            .pop()
                            .ok_or(CopyError::internal(COPY_INTERNAL))
                            .store_err_into(out.reborrow())?;
                        children.try_push(child, out.reborrow())?;
                    }
                    let mut children = children.into_iter();
                    let source = value.source();
                    let copied = match value.as_ref() {
                        ValueKind::Signed(number) => options
                            .with_value_error(|value_out| {
                                builder.add_signed(number, source, value_out)
                            })
                            .store_err_into(out.reborrow())?,
                        ValueKind::Unsigned(number) => options
                            .with_value_error(|value_out| {
                                builder.add_unsigned(number, source, value_out)
                            })
                            .store_err_into(out.reborrow())?,
                        ValueKind::Float32(number) => options
                            .with_value_error(|value_out| {
                                builder.add_float32(number, source, value_out)
                            })
                            .store_err_into(out.reborrow())?,
                        ValueKind::Float64(number) => options
                            .with_value_error(|value_out| {
                                builder.add_float64(number, source, value_out)
                            })
                            .store_err_into(out.reborrow())?,
                        ValueKind::Bool(boolean) => options
                            .with_value_error(|value_out| {
                                builder.add_bool(boolean, source, value_out)
                            })
                            .store_err_into(out.reborrow())?,
                        ValueKind::String(text) => {
                            check_text(text, limits, &options, out.reborrow())?;
                            options
                                .with_value_error(|value_out| {
                                    builder.add_string(text, source, value_out)
                                })
                                .store_err_into(out.reborrow())?
                        }
                        ValueKind::Optional(child) => {
                            let child = match child {
                                Some(_) => Some(
                                    children
                                        .next()
                                        .ok_or(CopyError::internal(COPY_INTERNAL))
                                        .store_err_into(out.reborrow())?,
                                ),
                                None => None,
                            };
                            options
                                .with_value_error(|value_out| {
                                    builder.add_optional(child, source, value_out)
                                })
                                .store_err_into(out.reborrow())?
                        }
                        ValueKind::Array(entries) => {
                            let mut child_ids = ChargedVec::new(options.budget, out.reborrow())?;
                            child_ids.try_reserve_exact(entries.len(), out.reborrow())?;
                            child_ids.extend(children, out.reborrow())?;
                            options
                                .with_value_error(|value_out| {
                                    builder.add_array(&child_ids, source, value_out)
                                })
                                .store_err_into(out.reborrow())?
                        }
                        ValueKind::Map(entries) | ValueKind::Structure(entries) => {
                            let entry_count = entries.len();
                            let mut named = ChargedVec::new(options.budget, out.reborrow())?;
                            named.try_reserve_exact(entry_count, out.reborrow())?;
                            for (entry, child) in entries.zip(children) {
                                if let Some(budget) = options.budget {
                                    budget.consume(1, out.reborrow())?;
                                }
                                let name = entry
                                    .name()
                                    .ok_or(CopyError::internal(COPY_INTERNAL))
                                    .store_err_into(out.reborrow())?;
                                check_text(name, limits, &options, out.reborrow())?;
                                named.try_push(NamedValue::new(name, child), out.reborrow())?;
                            }
                            if matches!(value.as_ref(), ValueKind::Map(_)) {
                                options
                                    .with_value_error(|value_out| {
                                        builder.add_map(&named, source, value_out)
                                    })
                                    .store_err_into(out.reborrow())?
                            } else {
                                options
                                    .with_value_error(|value_out| {
                                        builder.add_structure(&named, source, value_out)
                                    })
                                    .store_err_into(out.reborrow())?
                            }
                        }
                        ValueKind::Enumeration { variant, payload } => {
                            check_text(variant, limits, &options, out.reborrow())?;
                            match payload {
                                ValuePayloadView::Unit => options
                                    .with_value_error(|value_out| {
                                        builder.add_enumeration(
                                            variant,
                                            ValuePayload::Unit,
                                            source,
                                            value_out,
                                        )
                                    })
                                    .store_err_into(out.reborrow())?,
                                ValuePayloadView::Single(_) => {
                                    let payload = ValuePayload::Single(
                                        children
                                            .next()
                                            .ok_or(CopyError::internal(COPY_INTERNAL))
                                            .store_err_into(out.reborrow())?,
                                    );
                                    options
                                        .with_value_error(|value_out| {
                                            builder.add_enumeration(
                                                variant, payload, source, value_out,
                                            )
                                        })
                                        .store_err_into(out.reborrow())?
                                }
                                ValuePayloadView::Structure(fields) => {
                                    let mut named =
                                        ChargedVec::new(options.budget, out.reborrow())?;
                                    named.try_reserve_exact(fields.len(), out.reborrow())?;
                                    for (field, child) in fields.zip(children) {
                                        if let Some(budget) = options.budget {
                                            budget.consume(1, out.reborrow())?;
                                        }
                                        let name = field
                                            .name()
                                            .ok_or(CopyError::internal(COPY_INTERNAL))
                                            .store_err_into(out.reborrow())?;
                                        check_text(name, limits, &options, out.reborrow())?;
                                        named.try_push(
                                            NamedValue::new(name, child),
                                            out.reborrow(),
                                        )?;
                                    }
                                    options
                                        .with_value_error(|value_out| {
                                            builder.add_enumeration(
                                                variant,
                                                ValuePayload::Structure(&named),
                                                source,
                                                value_out,
                                            )
                                        })
                                        .store_err_into(out.reborrow())?
                                }
                            }
                        }
                    };
                    values.try_push(copied, out.reborrow())?;
                }
            }
        }
        let root = values
            .pop()
            .ok_or(CopyError::internal(COPY_INTERNAL))
            .store_err_into(out.reborrow())?;
        options
            .with_value_error(|value_out| builder.finish(root, value_out))
            .store_err_into(out.reborrow())
    }
}
