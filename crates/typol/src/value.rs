use opeo::{OResult, Out};
use typol_core::{
    BudgetError, ChargedString, ChargedVec, ConfigValue, ConfigValueBuilder, DiagnosticContext,
    ExecutionBudget, NamedValue, ValueError, ValueId, ValuePayload, ValueSource,
    diagnostic_context,
};

static VALUE_BUDGET_MISMATCH: DiagnosticContext = diagnostic_context!(
    "typol",
    "value-construction",
    "value-budget-does-not-match-container",
);

pub(crate) struct OwnedNamedValue {
    name: ChargedString,
    value: ConfigValue,
}

enum EnumPayloadKind {
    Unit,
    Single(ConfigValue),
    Structure(ChargedVec<OwnedNamedValue>),
}

pub struct EnumPayload {
    kind: EnumPayloadKind,
}

pub(crate) type FieldValue = OwnedNamedValue;
pub(crate) type MapEntry = OwnedNamedValue;

pub(crate) struct ValueFactory;

impl OwnedNamedValue {
    pub(crate) fn new<'slot>(
        name: &str,
        value: ConfigValue,
        budget: Option<&ExecutionBudget>,
        mut out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, Self> {
        if !same_budget(value.budget(), budget) {
            return Err(out.fail(ValueError::from(BudgetError::invariant(
                &VALUE_BUDGET_MISMATCH,
            ))));
        }
        let name = ChargedString::copy(name, budget, out.reborrow())?;
        Ok(Self { name, value })
    }

    pub(crate) fn from_name<'slot>(
        name: ChargedString,
        value: ConfigValue,
        budget: Option<&ExecutionBudget>,
        out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, Self> {
        if !same_budget(name.budget(), budget) || !same_budget(value.budget(), budget) {
            return Err(out.fail(ValueError::from(BudgetError::invariant(
                &VALUE_BUDGET_MISMATCH,
            ))));
        }
        Ok(Self { name, value })
    }

    pub(crate) fn name(&self) -> &str {
        self.name.as_str()
    }
}

impl EnumPayload {
    pub fn unit() -> Self {
        Self {
            kind: EnumPayloadKind::Unit,
        }
    }

    pub(crate) fn single(value: ConfigValue) -> Self {
        Self {
            kind: EnumPayloadKind::Single(value),
        }
    }

    pub(crate) fn structure(values: ChargedVec<OwnedNamedValue>) -> Self {
        Self {
            kind: EnumPayloadKind::Structure(values),
        }
    }
}

impl ValueFactory {
    pub(crate) fn signed<'slot>(
        value: i128,
        budget: Option<&ExecutionBudget>,
        out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ConfigValue> {
        Self::build(
            budget,
            |builder, out| builder.add_signed(value, ValueSource::RustValue, out),
            out,
        )
    }

    pub(crate) fn unsigned<'slot>(
        value: u128,
        budget: Option<&ExecutionBudget>,
        out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ConfigValue> {
        Self::build(
            budget,
            |builder, out| builder.add_unsigned(value, ValueSource::RustValue, out),
            out,
        )
    }

    pub(crate) fn float32<'slot>(
        value: f32,
        budget: Option<&ExecutionBudget>,
        out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ConfigValue> {
        Self::build(
            budget,
            |builder, out| builder.add_float32(value, ValueSource::RustValue, out),
            out,
        )
    }

    pub(crate) fn float64<'slot>(
        value: f64,
        budget: Option<&ExecutionBudget>,
        out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ConfigValue> {
        Self::build(
            budget,
            |builder, out| builder.add_float64(value, ValueSource::RustValue, out),
            out,
        )
    }

    pub(crate) fn boolean<'slot>(
        value: bool,
        budget: Option<&ExecutionBudget>,
        out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ConfigValue> {
        Self::build(
            budget,
            |builder, out| builder.add_bool(value, ValueSource::RustValue, out),
            out,
        )
    }

    pub(crate) fn string<'slot>(
        value: &str,
        budget: Option<&ExecutionBudget>,
        out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ConfigValue> {
        Self::build(
            budget,
            |builder, out| builder.add_string(value, ValueSource::RustValue, out),
            out,
        )
    }

    pub(crate) fn none<'slot>(
        budget: Option<&ExecutionBudget>,
        out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ConfigValue> {
        Self::build(
            budget,
            |builder, out| builder.add_optional(None, ValueSource::RustValue, out),
            out,
        )
    }

    pub(crate) fn optional<'slot>(
        value: Option<ConfigValue>,
        budget: Option<&ExecutionBudget>,
        mut out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ConfigValue> {
        let mut builder = ConfigValueBuilder::new(budget, out.reborrow())?;
        let child = value
            .map(|value| builder.append_value(value, out.reborrow()))
            .transpose()?;
        let root = builder.add_optional(child, ValueSource::RustValue, out.reborrow())?;
        builder.finish(root, out.reborrow())
    }

    pub(crate) fn array<'slot>(
        values: ChargedVec<ConfigValue>,
        budget: Option<&ExecutionBudget>,
        mut out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ConfigValue> {
        let mut builder = ConfigValueBuilder::new(budget, out.reborrow())?;
        let mut children = ChargedVec::new(budget, out.reborrow())?;
        children.try_reserve_exact(values.len(), out.reborrow())?;
        for value in values {
            children.try_push(builder.append_value(value, out.reborrow())?, out.reborrow())?;
        }
        let root = builder.add_array(&children, ValueSource::RustValue, out.reborrow())?;
        builder.finish(root, out.reborrow())
    }

    pub(crate) fn structure<'slot>(
        values: ChargedVec<OwnedNamedValue>,
        budget: Option<&ExecutionBudget>,
        out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ConfigValue> {
        Self::named(values, budget, false, out)
    }

    pub(crate) fn map<'slot>(
        values: ChargedVec<OwnedNamedValue>,
        budget: Option<&ExecutionBudget>,
        out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ConfigValue> {
        Self::named(values, budget, true, out)
    }

    pub(crate) fn enumeration<'slot>(
        variant: &str,
        payload: EnumPayload,
        budget: Option<&ExecutionBudget>,
        mut out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ConfigValue> {
        let mut builder = ConfigValueBuilder::new(budget, out.reborrow())?;
        let root = match payload.kind {
            EnumPayloadKind::Unit => builder.add_enumeration(
                variant,
                ValuePayload::Unit,
                ValueSource::RustValue,
                out.reborrow(),
            )?,
            EnumPayloadKind::Single(value) => {
                let child = builder.append_value(value, out.reborrow())?;
                builder.add_enumeration(
                    variant,
                    ValuePayload::Single(child),
                    ValueSource::RustValue,
                    out.reborrow(),
                )?
            }
            EnumPayloadKind::Structure(values) => {
                let (names, fields) =
                    Self::named_values(values, &mut builder, budget, out.reborrow())?;
                let fields = Self::named_refs(&names, &fields, budget, out.reborrow())?;
                builder.add_enumeration(
                    variant,
                    ValuePayload::Structure(fields.as_slice()),
                    ValueSource::RustValue,
                    out.reborrow(),
                )?
            }
        };
        builder.finish(root, out.reborrow())
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

    fn named<'slot>(
        values: ChargedVec<OwnedNamedValue>,
        budget: Option<&ExecutionBudget>,
        map: bool,
        mut out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ConfigValue> {
        let mut builder = ConfigValueBuilder::new(budget, out.reborrow())?;
        let (names, fields) = Self::named_values(values, &mut builder, budget, out.reborrow())?;
        let fields = Self::named_refs(&names, &fields, budget, out.reborrow())?;
        let root = if map {
            builder.add_map(&fields, ValueSource::RustValue, out.reborrow())?
        } else {
            builder.add_structure(&fields, ValueSource::RustValue, out.reborrow())?
        };
        builder.finish(root, out.reborrow())
    }

    fn named_values<'slot>(
        values: ChargedVec<OwnedNamedValue>,
        builder: &mut ConfigValueBuilder,
        budget: Option<&ExecutionBudget>,
        mut out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, (ChargedVec<ChargedString>, ChargedVec<ValueId>)> {
        let mut names = ChargedVec::new(budget, out.reborrow())?;
        let mut fields = ChargedVec::new(budget, out.reborrow())?;
        names.try_reserve_exact(values.len(), out.reborrow())?;
        fields.try_reserve_exact(values.len(), out.reborrow())?;
        for value in values {
            names.try_push(
                ChargedString::copy(value.name(), budget, out.reborrow())?,
                out.reborrow(),
            )?;
            fields.try_push(
                builder.append_value(value.value, out.reborrow())?,
                out.reborrow(),
            )?;
        }
        Ok((names, fields))
    }

    fn named_refs<'slot, 'name>(
        names: &'name [ChargedString],
        fields: &[ValueId],
        budget: Option<&ExecutionBudget>,
        mut out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ChargedVec<NamedValue<'name>>> {
        let mut named = ChargedVec::new(budget, out.reborrow())?;
        named.try_reserve_exact(names.len(), out.reborrow())?;
        for (name, value) in names.iter().zip(fields) {
            named.try_push(NamedValue::new(name.as_str(), *value), out.reborrow())?;
        }
        Ok(named)
    }
}

fn same_budget(left: Option<&ExecutionBudget>, right: Option<&ExecutionBudget>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left.same_call(right),
        (None, None) => true,
        _ => false,
    }
}
