use opeo::{ErrSlot, OResult, Out};
use typol_core::{
    BudgetError, ChargedString, ChargedVec, ConfigValue, ConfigValueBuilder, Diagnostic,
    DiagnosticCode, DiagnosticContext, ExecutionBudget, LocatedSpan, NamedValue, ValueEntries,
    ValueError, ValueErrorKind, ValueId, ValueKind, ValuePayload, ValuePayloadView, ValueSource,
    ValueView, diagnostic_context,
};

/// A schema-shaped value with defaults applied, before constraints and assertions run.
#[derive(Debug)]
pub struct NormalizedConfigValue(ConfigValue);

impl NormalizedConfigValue {
    pub(crate) fn from_config_value(value: ConfigValue) -> Self {
        Self(value)
    }

    pub fn as_config_value(&self) -> &ConfigValue {
        &self.0
    }

    pub fn into_config_value(self) -> ConfigValue {
        self.0
    }
}

/// A configuration value that passed schema constraints and complete-tree assertions.
#[derive(Debug)]
pub struct ValidatedConfigValue(ConfigValue);

impl ValidatedConfigValue {
    pub(crate) fn from_config_value(value: ConfigValue) -> Self {
        Self(value)
    }

    pub fn as_config_value(&self) -> &ConfigValue {
        &self.0
    }

    pub fn into_config_value(self) -> ConfigValue {
        self.0
    }
}

static VALUE_BUDGET_MISMATCH: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "value-assembly",
    "value-budget-does-not-match-assembly",
);
static VALUE_DUPLICATE_ENTRY: DiagnosticContext =
    diagnostic_context!("typol-config", "value-assembly", "duplicate-entry");
static VALUE_INTERNAL_FAILURE: DiagnosticContext =
    diagnostic_context!("typol-config", "value-assembly", "invalid-value-invariant");
static VALUE_SCALAR_KIND_REQUIRED: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "value-assembly",
    "scalar-constructor-received-container",
);

enum CopyTask<'value> {
    Enter(ValueView<'value>),
    Finish(ValueView<'value>, usize),
}

pub(crate) struct OwnedNamedValue {
    name: ChargedString,
    value: ConfigValue,
}

pub(crate) type FieldValue = OwnedNamedValue;
pub(crate) type MapEntry = OwnedNamedValue;

impl OwnedNamedValue {
    pub(crate) fn try_new<'slot, E>(
        name: &str,
        value: ConfigValue,
        budget: Option<&ExecutionBudget>,
        out: Out<'slot, '_, E>,
    ) -> OResult<'slot, Self>
    where
        E: From<BudgetError>,
    {
        if !same_budget(value.budget(), budget) {
            return Err(out.fail(E::from(BudgetError::invariant(&VALUE_BUDGET_MISMATCH))));
        }
        let mut errors = ErrSlot::new();
        let name = match errors.try_call(|budget_out| ChargedString::copy(name, budget, budget_out))
        {
            Ok(name) => name,
            Err(caught) => return Err(out.fail(E::from(caught.take()))),
        };
        Ok(Self { name, value })
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    #[cfg(test)]
    pub(crate) fn value(&self) -> &ConfigValue {
        &self.value
    }
}

pub(crate) enum EnumPayload {
    Unit,
    Single(ConfigValue),
    Structure(ChargedVec<FieldValue>),
}

impl EnumPayload {
    pub(crate) fn unit() -> Self {
        Self::Unit
    }

    pub(crate) fn single(value: ConfigValue) -> Self {
        Self::Single(value)
    }

    pub(crate) fn structure(values: ChargedVec<FieldValue>) -> Self {
        Self::Structure(values)
    }
}

pub(crate) struct ValueFactory;

impl ValueFactory {
    pub(crate) fn scalar<'slot>(
        value: ValueView<'_>,
        source: ValueSource,
        budget: Option<&ExecutionBudget>,
        out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ConfigValue> {
        match value.as_ref() {
            ValueKind::Signed(value) => Self::signed(value, source, budget, out),
            ValueKind::Unsigned(value) => Self::unsigned(value, source, budget, out),
            ValueKind::Float32(value) => Self::float32(value, source, budget, out),
            ValueKind::Float64(value) => Self::float64(value, source, budget, out),
            ValueKind::Bool(value) => Self::boolean(value, source, budget, out),
            ValueKind::String(value) => Self::string(value, source, budget, out),
            _ => Err(out.fail(ValueError::from(BudgetError::invariant(
                &VALUE_SCALAR_KIND_REQUIRED,
            )))),
        }
    }

    pub(crate) const fn failure_code(error: ValueError) -> DiagnosticCode {
        Self::failure_code_for_kind(error.kind())
    }

    pub(crate) const fn failure_code_for_kind(kind: ValueErrorKind) -> DiagnosticCode {
        match kind {
            ValueErrorKind::ResourceLimitExceeded => DiagnosticCode::RESOURCE_LIMIT_EXCEEDED,
            ValueErrorKind::AllocationFailed => DiagnosticCode::ALLOCATION_FAILED,
            ValueErrorKind::CapacityExceeded => DiagnosticCode::CAPACITY_EXCEEDED,
            ValueErrorKind::DuplicateMapKey
            | ValueErrorKind::DuplicateFieldName
            | ValueErrorKind::InternalInvariantFailure => DiagnosticCode::INTERNAL_ERROR,
        }
    }

    pub(crate) fn failure_context(error: ValueError) -> Option<DiagnosticContext> {
        match error.kind() {
            ValueErrorKind::DuplicateMapKey | ValueErrorKind::DuplicateFieldName => Some(
                error
                    .duplicate_indices()
                    .map_or(VALUE_DUPLICATE_ENTRY, |(first, _)| {
                        VALUE_DUPLICATE_ENTRY.with_object_id(first as u64)
                    }),
            ),
            ValueErrorKind::InternalInvariantFailure => {
                Some(error.context().unwrap_or(VALUE_INTERNAL_FAILURE))
            }
            ValueErrorKind::AllocationFailed
            | ValueErrorKind::ResourceLimitExceeded
            | ValueErrorKind::CapacityExceeded => error.context(),
        }
    }

    pub(crate) fn failure(
        error: ValueError,
        primary: Option<LocatedSpan>,
        source: Option<ValueSource>,
    ) -> Diagnostic {
        let mut diagnostic = Diagnostic::emergency(Self::failure_code(error));
        diagnostic.set_context(Self::failure_context(error));
        diagnostic.set_located_primary(primary);
        diagnostic.set_value_source(source);
        diagnostic
    }

    pub(crate) fn signed<'slot>(
        value: i128,
        source: ValueSource,
        budget: Option<&ExecutionBudget>,
        out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ConfigValue> {
        Self::build(
            budget,
            |builder, out| builder.add_signed(value, source, out),
            out,
        )
    }

    pub(crate) fn unsigned<'slot>(
        value: u128,
        source: ValueSource,
        budget: Option<&ExecutionBudget>,
        out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ConfigValue> {
        Self::build(
            budget,
            |builder, out| builder.add_unsigned(value, source, out),
            out,
        )
    }

    pub(crate) fn float32<'slot>(
        value: f32,
        source: ValueSource,
        budget: Option<&ExecutionBudget>,
        out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ConfigValue> {
        Self::build(
            budget,
            |builder, out| builder.add_float32(value, source, out),
            out,
        )
    }

    pub(crate) fn float64<'slot>(
        value: f64,
        source: ValueSource,
        budget: Option<&ExecutionBudget>,
        out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ConfigValue> {
        Self::build(
            budget,
            |builder, out| builder.add_float64(value, source, out),
            out,
        )
    }

    pub(crate) fn boolean<'slot>(
        value: bool,
        source: ValueSource,
        budget: Option<&ExecutionBudget>,
        out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ConfigValue> {
        Self::build(
            budget,
            |builder, out| builder.add_bool(value, source, out),
            out,
        )
    }

    pub(crate) fn string<'slot>(
        value: &str,
        source: ValueSource,
        budget: Option<&ExecutionBudget>,
        out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ConfigValue> {
        Self::build(
            budget,
            |builder, out| builder.add_string(value, source, out),
            out,
        )
    }

    pub(crate) fn none<'slot>(
        source: ValueSource,
        budget: Option<&ExecutionBudget>,
        out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ConfigValue> {
        Self::optional(None, source, budget, out)
    }

    pub(crate) fn optional<'slot>(
        value: Option<&ConfigValue>,
        source: ValueSource,
        budget: Option<&ExecutionBudget>,
        mut out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ConfigValue> {
        let mut builder = ConfigValueBuilder::new(budget, out.reborrow())?;
        let child = value
            .map(|value| Self::copy_into(&mut builder, value.root(), budget, out.reborrow()))
            .transpose()?;
        let root = builder.add_optional(child, source, out.reborrow())?;
        builder.finish(root, out.reborrow())
    }

    pub(crate) fn optional_owned<'slot>(
        value: ConfigValue,
        source: ValueSource,
        budget: Option<&ExecutionBudget>,
        mut out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ConfigValue> {
        let (mut builder, child) = Self::continue_from(value, budget, out.reborrow())?;
        let root = builder.add_optional(Some(child), source, out.reborrow())?;
        builder.finish(root, out.reborrow())
    }

    pub(crate) fn array<'slot>(
        values: &[ConfigValue],
        source: ValueSource,
        budget: Option<&ExecutionBudget>,
        mut out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ConfigValue> {
        let mut builder = ConfigValueBuilder::new(budget, out.reborrow())?;
        let mut children = ChargedVec::new(budget, out.reborrow())?;
        children.try_reserve_exact(values.len(), out.reborrow())?;
        for value in values {
            children.try_push(
                Self::copy_into(&mut builder, value.root(), budget, out.reborrow())?,
                out.reborrow(),
            )?;
        }
        let root = builder.add_array(&children, source, out.reborrow())?;
        builder.finish(root, out.reborrow())
    }

    pub(crate) fn array_owned<'slot>(
        mut values: ChargedVec<ConfigValue>,
        source: ValueSource,
        budget: Option<&ExecutionBudget>,
        mut out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ConfigValue> {
        if values.len() == 1 {
            let value = values.pop().ok_or_else(|| {
                out.reborrow()
                    .fail(ValueError::from(BudgetError::CapacityExceeded))
            })?;
            let (mut builder, child) = Self::continue_from(value, budget, out.reborrow())?;
            let children = [child];
            let root = builder.add_array(&children, source, out.reborrow())?;
            return builder.finish(root, out.reborrow());
        }
        let mut builder = ConfigValueBuilder::new(budget, out.reborrow())?;
        let mut children = ChargedVec::new(budget, out.reborrow())?;
        children.try_reserve_exact(values.len(), out.reborrow())?;
        for value in values {
            children.try_push(builder.append_value(value, out.reborrow())?, out.reborrow())?;
        }
        let root = builder.add_array(&children, source, out.reborrow())?;
        builder.finish(root, out.reborrow())
    }

    pub(crate) fn map<'slot>(
        values: &[MapEntry],
        source: ValueSource,
        budget: Option<&ExecutionBudget>,
        mut out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ConfigValue> {
        Self::named(values, source, budget, true, out.reborrow())
    }

    pub(crate) fn structure<'slot>(
        values: &[FieldValue],
        source: ValueSource,
        budget: Option<&ExecutionBudget>,
        mut out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ConfigValue> {
        Self::named(values, source, budget, false, out.reborrow())
    }

    pub(crate) fn map_owned<'slot>(
        values: ChargedVec<MapEntry>,
        source: ValueSource,
        budget: Option<&ExecutionBudget>,
        out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ConfigValue> {
        Self::named_owned(values, source, budget, true, out)
    }

    pub(crate) fn structure_owned<'slot>(
        values: ChargedVec<FieldValue>,
        source: ValueSource,
        budget: Option<&ExecutionBudget>,
        out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ConfigValue> {
        Self::named_owned(values, source, budget, false, out)
    }

    pub(crate) fn structure_view<'slot>(
        values: ValueEntries<'_>,
        source: ValueSource,
        budget: Option<&ExecutionBudget>,
        mut out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ConfigValue> {
        let mut builder = ConfigValueBuilder::new(budget, out.reborrow())?;
        let mut named = ChargedVec::new(budget, out.reborrow())?;
        named.try_reserve_exact(values.len(), out.reborrow())?;
        for entry in values {
            let Some(name) = entry.name() else {
                return Err(out.fail(ValueError::from(BudgetError::CapacityExceeded)));
            };
            let value = Self::copy_into(&mut builder, entry.value(), budget, out.reborrow())?;
            named.try_push(NamedValue::new(name, value), out.reborrow())?;
        }
        let root = builder.add_structure(&named, source, out.reborrow())?;
        builder.finish(root, out.reborrow())
    }

    pub(crate) fn enumeration<'slot>(
        variant: &str,
        payload: EnumPayload,
        source: ValueSource,
        budget: Option<&ExecutionBudget>,
        mut out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ConfigValue> {
        match payload {
            EnumPayload::Single(value) => {
                let (mut builder, child) = Self::continue_from(value, budget, out.reborrow())?;
                let root = builder.add_enumeration(
                    variant,
                    ValuePayload::Single(child),
                    source,
                    out.reborrow(),
                )?;
                builder.finish(root, out.reborrow())
            }
            EnumPayload::Structure(mut fields) if fields.len() == 1 => {
                let field = fields.pop().ok_or_else(|| {
                    out.reborrow()
                        .fail(ValueError::from(BudgetError::CapacityExceeded))
                })?;
                let OwnedNamedValue { name, value } = field;
                let (mut builder, child) = Self::continue_from(value, budget, out.reborrow())?;
                let fields = [NamedValue::new(&name, child)];
                let root = builder.add_enumeration(
                    variant,
                    ValuePayload::Structure(&fields),
                    source,
                    out.reborrow(),
                )?;
                builder.finish(root, out.reborrow())
            }
            payload => Self::enumeration_copy(variant, payload, source, budget, out),
        }
    }

    fn enumeration_copy<'slot>(
        variant: &str,
        payload: EnumPayload,
        source: ValueSource,
        budget: Option<&ExecutionBudget>,
        mut out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ConfigValue> {
        let mut builder = ConfigValueBuilder::new(budget, out.reborrow())?;
        let root = match payload {
            EnumPayload::Unit => {
                builder.add_enumeration(variant, ValuePayload::Unit, source, out.reborrow())?
            }
            EnumPayload::Single(value) => {
                let child = Self::copy_into(&mut builder, value.root(), budget, out.reborrow())?;
                builder.add_enumeration(
                    variant,
                    ValuePayload::Single(child),
                    source,
                    out.reborrow(),
                )?
            }
            EnumPayload::Structure(fields) => {
                let mut names = ChargedVec::new(budget, out.reborrow())?;
                let mut named = ChargedVec::new(budget, out.reborrow())?;
                let mut child_ids = ChargedVec::new(budget, out.reborrow())?;
                named.try_reserve_exact(fields.len(), out.reborrow())?;
                names.try_reserve_exact(fields.len(), out.reborrow())?;
                child_ids.try_reserve_exact(fields.len(), out.reborrow())?;
                for field in fields {
                    let OwnedNamedValue { name, value } = field;
                    let value = builder.append_value(value, out.reborrow())?;
                    names.try_push(name, out.reborrow())?;
                    child_ids.try_push(value, out.reborrow())?;
                }
                for (name, value) in names.iter().zip(child_ids.iter().copied()) {
                    named.try_push(NamedValue::new(name, value), out.reborrow())?;
                }
                builder.add_enumeration(
                    variant,
                    ValuePayload::Structure(&named),
                    source,
                    out.reborrow(),
                )?
            }
        };
        builder.finish(root, out.reborrow())
    }

    fn named<'slot>(
        values: &[OwnedNamedValue],
        source: ValueSource,
        budget: Option<&ExecutionBudget>,
        map: bool,
        mut out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ConfigValue> {
        let mut builder = ConfigValueBuilder::new(budget, out.reborrow())?;
        let mut named = ChargedVec::new(budget, out.reborrow())?;
        named.try_reserve_exact(values.len(), out.reborrow())?;
        for entry in values {
            let value = Self::copy_into(&mut builder, entry.value.root(), budget, out.reborrow())?;
            named.try_push(NamedValue::new(entry.name(), value), out.reborrow())?;
        }
        let root = if map {
            builder.add_map(&named, source, out.reborrow())?
        } else {
            builder.add_structure(&named, source, out.reborrow())?
        };
        builder.finish(root, out.reborrow())
    }

    fn named_owned<'slot>(
        mut values: ChargedVec<OwnedNamedValue>,
        source: ValueSource,
        budget: Option<&ExecutionBudget>,
        map: bool,
        mut out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ConfigValue> {
        if values.len() == 1 {
            let field = values.pop().ok_or_else(|| {
                out.reborrow()
                    .fail(ValueError::from(BudgetError::CapacityExceeded))
            })?;
            let OwnedNamedValue { name, value } = field;
            let (mut builder, child) = Self::continue_from(value, budget, out.reborrow())?;
            let named = [NamedValue::new(&name, child)];
            let root = if map {
                builder.add_map(&named, source, out.reborrow())?
            } else {
                builder.add_structure(&named, source, out.reborrow())?
            };
            return builder.finish(root, out.reborrow());
        }
        let mut builder = ConfigValueBuilder::new(budget, out.reborrow())?;
        let mut names = ChargedVec::new(budget, out.reborrow())?;
        let mut named = ChargedVec::new(budget, out.reborrow())?;
        let mut child_ids = ChargedVec::new(budget, out.reborrow())?;
        named.try_reserve_exact(values.len(), out.reborrow())?;
        names.try_reserve_exact(values.len(), out.reborrow())?;
        child_ids.try_reserve_exact(values.len(), out.reborrow())?;
        for entry in values {
            let OwnedNamedValue { name, value } = entry;
            let value = builder.append_value(value, out.reborrow())?;
            names.try_push(name, out.reborrow())?;
            child_ids.try_push(value, out.reborrow())?;
        }
        for (name, value) in names.iter().zip(child_ids.iter().copied()) {
            named.try_push(NamedValue::new(name, value), out.reborrow())?;
        }
        let root = if map {
            builder.add_map(&named, source, out.reborrow())?
        } else {
            builder.add_structure(&named, source, out.reborrow())?
        };
        builder.finish(root, out.reborrow())
    }

    fn continue_from<'slot>(
        value: ConfigValue,
        budget: Option<&ExecutionBudget>,
        out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, (ConfigValueBuilder, ValueId)> {
        if !same_budget(value.budget(), budget) {
            return Err(out.fail(ValueError::from(BudgetError::invariant(
                &VALUE_BUDGET_MISMATCH,
            ))));
        }
        Ok(ConfigValueBuilder::from_value(value))
    }

    fn build<'slot>(
        budget: Option<&ExecutionBudget>,
        add: impl FnOnce(&mut ConfigValueBuilder, Out<'slot, '_, ValueError>) -> OResult<'slot, ValueId>,
        mut out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ConfigValue> {
        let mut builder = ConfigValueBuilder::new(budget, out.reborrow())?;
        let root = add(&mut builder, out.reborrow())?;
        builder.finish(root, out.reborrow())
    }

    fn copy_into<'slot, 'value>(
        builder: &mut ConfigValueBuilder,
        root: ValueView<'value>,
        budget: Option<&ExecutionBudget>,
        mut out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ValueId> {
        let mut tasks = ChargedVec::new(budget, out.reborrow())?;
        let mut values = ChargedVec::new(budget, out.reborrow())?;
        tasks.try_push(CopyTask::Enter(root), out.reborrow())?;
        while let Some(task) = tasks.pop() {
            match task {
                CopyTask::Enter(value) => {
                    let start = values.len();
                    let mut children = ChargedVec::new(budget, out.reborrow())?;
                    match value.as_ref() {
                        ValueKind::Optional(Some(child)) => {
                            children.try_push(child, out.reborrow())?;
                        }
                        ValueKind::Array(entries)
                        | ValueKind::Map(entries)
                        | ValueKind::Structure(entries) => {
                            children.try_reserve_exact(entries.len(), out.reborrow())?;
                            for entry in entries {
                                children.try_push(entry.value(), out.reborrow())?;
                            }
                        }
                        ValueKind::Enumeration { payload, .. } => match payload {
                            ValuePayloadView::Unit => {}
                            ValuePayloadView::Single(child) => {
                                children.try_push(child, out.reborrow())?;
                            }
                            ValuePayloadView::Structure(entries) => {
                                children.try_reserve_exact(entries.len(), out.reborrow())?;
                                for entry in entries {
                                    children.try_push(entry.value(), out.reborrow())?;
                                }
                            }
                        },
                        _ => {}
                    }
                    tasks.try_push(CopyTask::Finish(value, start), out.reborrow())?;
                    for child in children.into_iter().rev() {
                        tasks.try_push(CopyTask::Enter(child), out.reborrow())?;
                    }
                }
                CopyTask::Finish(value, start) => {
                    let child_count = values.len().saturating_sub(start);
                    let mut children = ChargedVec::new(budget, out.reborrow())?;
                    children.try_reserve_exact(child_count, out.reborrow())?;
                    children.extend(values.drain(start..), out.reborrow())?;
                    let source = value.source();
                    let is_map = matches!(value.as_ref(), ValueKind::Map(_));
                    let kind = value.as_ref();
                    let id = match kind {
                        ValueKind::Signed(number) => {
                            builder.add_signed(number, source, out.reborrow())?
                        }
                        ValueKind::Unsigned(number) => {
                            builder.add_unsigned(number, source, out.reborrow())?
                        }
                        ValueKind::Float32(number) => {
                            builder.add_float32(number, source, out.reborrow())?
                        }
                        ValueKind::Float64(number) => {
                            builder.add_float64(number, source, out.reborrow())?
                        }
                        ValueKind::Bool(boolean) => {
                            builder.add_bool(boolean, source, out.reborrow())?
                        }
                        ValueKind::String(text) => {
                            builder.add_string(text, source, out.reborrow())?
                        }
                        ValueKind::Optional(child) => {
                            let child = match child {
                                Some(_) => Some(children.first().copied().ok_or_else(|| {
                                    out.reborrow()
                                        .fail(ValueError::from(BudgetError::CapacityExceeded))
                                })?),
                                None => None,
                            };
                            builder.add_optional(child, source, out.reborrow())?
                        }
                        ValueKind::Array(_) => {
                            builder.add_array(&children, source, out.reborrow())?
                        }
                        ValueKind::Map(entries) | ValueKind::Structure(entries) => {
                            let mut named = ChargedVec::new(budget, out.reborrow())?;
                            named.try_reserve_exact(entries.len(), out.reborrow())?;
                            for (entry, child) in entries.zip(children.iter().copied()) {
                                let Some(name) = entry.name() else {
                                    return Err(
                                        out.fail(ValueError::from(BudgetError::CapacityExceeded))
                                    );
                                };
                                named.try_push(NamedValue::new(name, child), out.reborrow())?;
                            }
                            if is_map {
                                builder.add_map(&named, source, out.reborrow())?
                            } else {
                                builder.add_structure(&named, source, out.reborrow())?
                            }
                        }
                        ValueKind::Enumeration { variant, payload } => match payload {
                            ValuePayloadView::Unit => builder.add_enumeration(
                                variant,
                                ValuePayload::Unit,
                                source,
                                out.reborrow(),
                            )?,
                            ValuePayloadView::Single(_) => builder.add_enumeration(
                                variant,
                                ValuePayload::Single(children.first().copied().ok_or_else(
                                    || {
                                        out.reborrow()
                                            .fail(ValueError::from(BudgetError::CapacityExceeded))
                                    },
                                )?),
                                source,
                                out.reborrow(),
                            )?,
                            ValuePayloadView::Structure(entries) => {
                                let mut named = ChargedVec::new(budget, out.reborrow())?;
                                named.try_reserve_exact(entries.len(), out.reborrow())?;
                                for (entry, child) in entries.zip(children.iter().copied()) {
                                    let Some(name) = entry.name() else {
                                        return Err(out.fail(ValueError::from(
                                            BudgetError::CapacityExceeded,
                                        )));
                                    };
                                    named.try_push(NamedValue::new(name, child), out.reborrow())?;
                                }
                                builder.add_enumeration(
                                    variant,
                                    ValuePayload::Structure(&named),
                                    source,
                                    out.reborrow(),
                                )?
                            }
                        },
                    };
                    values.try_push(id, out.reborrow())?;
                }
            }
        }
        values
            .pop()
            .ok_or_else(|| out.fail(ValueError::from(BudgetError::CapacityExceeded)))
    }
}

fn same_budget(left: Option<&ExecutionBudget>, right: Option<&ExecutionBudget>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left.same_call(right),
        (None, None) => true,
        _ => false,
    }
}

#[cfg(test)]
mod test {
    use super::ValueFactory;
    use opeo::ErrSlot;
    use typol_core::{
        BudgetError, ConfigValueBuilder, DiagnosticCode, DiagnosticContext, LocatedSpan,
        NamedValue, SourceId, SourceSpan, Span, ValueError, ValueErrorKind, ValueSource,
    };
    use typol_test_support::{TestFailure, TestResult, test_eq};

    fn duplicate_value_error(map: bool) -> TestResult<ValueError> {
        let mut errors = ErrSlot::new();
        let result = errors.try_call(|mut out| {
            let mut builder = ConfigValueBuilder::new(None, out.reborrow())?;
            let value = builder.add_bool(false, ValueSource::RustValue, out.reborrow())?;
            let entries = [
                NamedValue::new("duplicate", value),
                NamedValue::new("duplicate", value),
            ];
            let root = if map {
                builder.add_map(&entries, ValueSource::RustValue, out.reborrow())?
            } else {
                builder.add_structure(&entries, ValueSource::RustValue, out.reborrow())?
            };
            builder.finish(root, out.reborrow())
        });
        match result {
            Ok(_) => Err(TestFailure::new("duplicate entry construction should fail")),
            Err(caught) => Ok(caught.take()),
        }
    }

    struct ValueFailureCase {
        error: ValueError,
        code: DiagnosticCode,
        context: Option<DiagnosticContext>,
    }

    #[test]
    fn value_failure_diagnostics_preserve_classification_context_and_origin() -> TestResult<()> {
        let primary: LocatedSpan = SourceSpan::new(SourceId::new(7), Span::from_zero_to(3)).into();
        let source = ValueSource::ExplicitFallback;
        let duplicate_map_error = duplicate_value_error(true)?;
        let duplicate_structure_error = duplicate_value_error(false)?;
        let duplicate_map_context = ValueFactory::failure_context(duplicate_map_error);
        let duplicate_structure_context = ValueFactory::failure_context(duplicate_structure_error);
        let errors = [
            ValueFailureCase {
                error: ValueError::from(BudgetError::Resource),
                code: DiagnosticCode::RESOURCE_LIMIT_EXCEEDED,
                context: None,
            },
            ValueFailureCase {
                error: ValueError::from(BudgetError::Allocation),
                code: DiagnosticCode::ALLOCATION_FAILED,
                context: None,
            },
            ValueFailureCase {
                error: ValueError::from(BudgetError::CapacityExceeded),
                code: DiagnosticCode::CAPACITY_EXCEEDED,
                context: None,
            },
            ValueFailureCase {
                error: duplicate_map_error,
                code: DiagnosticCode::INTERNAL_ERROR,
                context: duplicate_map_context,
            },
            ValueFailureCase {
                error: duplicate_structure_error,
                code: DiagnosticCode::INTERNAL_ERROR,
                context: duplicate_structure_context,
            },
        ];
        for case in errors {
            let diagnostic = ValueFactory::failure(case.error, Some(primary), Some(source));
            test_eq!(ValueFactory::failure_code(case.error), case.code)?;
            test_eq!(diagnostic.code(), case.code)?;
            test_eq!(diagnostic.context(), case.context)?;
            test_eq!(diagnostic.primary(), Some(primary.source_span()))?;
            test_eq!(diagnostic.value_source(), Some(source))?;
        }

        test_eq!(
            ValueFactory::failure_context(duplicate_map_error),
            duplicate_map_context
        )?;
        let duplicate_context = duplicate_map_context.ok_or_else(|| {
            TestFailure::new("duplicate-key error should include diagnostic context")
        })?;
        test_eq!(duplicate_context.subsystem(), "typol-config")?;
        test_eq!(duplicate_context.stage(), "value-assembly")?;
        test_eq!(duplicate_context.failure_point(), "duplicate-entry")?;
        test_eq!(duplicate_context.object_id(), Some(0))?;

        Ok(())
    }

    #[test]
    fn scalar_factory_reports_non_scalar_values_as_internal_invariants() -> TestResult<()> {
        let mut value_errors = ErrSlot::new();
        let value = value_errors
            .try_call(|out| ValueFactory::array(&[], ValueSource::RustValue, None, out))
            .map_err(|_| TestFailure::new("test array construction failed"))?;
        let mut scalar_errors = ErrSlot::new();
        let caught = scalar_errors
            .try_call(|out| {
                ValueFactory::scalar(value.root(), ValueSource::ConfigLiteral, None, out)
            })
            .err()
            .ok_or_else(|| TestFailure::new("non-scalar input should fail"))?;
        let error = caught.take();
        test_eq!(error.kind(), ValueErrorKind::InternalInvariantFailure)?;
        let context = error.context().ok_or_else(|| {
            TestFailure::new("non-scalar error should include diagnostic context")
        })?;
        test_eq!(context.subsystem(), "typol-config")?;
        test_eq!(context.stage(), "value-assembly")?;
        test_eq!(
            context.failure_point(),
            "scalar-constructor-received-container"
        )?;
        test_eq!(ValueFactory::failure_context(error), Some(context))?;
        let diagnostic = ValueFactory::failure(error, None, None);
        test_eq!(diagnostic.context(), Some(context))?;

        Ok(())
    }
}
