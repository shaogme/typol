use std::{process::abort, string::String, vec::Vec};
use typol::{
    ConfigValue,
    core::{
        BudgetError, ChargedVec, ConfigValueBuilder, NamedValue, ValueError, ValueId, ValuePayload,
        ValueSource,
    },
    opeo::{OResult, Out},
};
use typol_test_support::TestOpeo;

pub struct Field {
    pub name: String,
    pub value: ConfigValue,
}

pub struct ValueSupport;

impl ValueSupport {
    pub fn field(name: String, value: ConfigValue) -> Field {
        Field { name, value }
    }

    pub fn scalar(
        add: impl for<'slot, 'borrow> FnOnce(
            &mut ConfigValueBuilder,
            Out<'slot, 'borrow, ValueError>,
        ) -> OResult<'slot, ValueId>,
    ) -> ConfigValue {
        Self::direct(|mut out| {
            let mut builder = ConfigValueBuilder::new(None, out.reborrow())?;
            let root = add(&mut builder, out.reborrow())?;
            builder.finish(root, out)
        })
    }

    pub fn structure<'slot>(
        fields: Vec<Field>,
        enumeration: Option<&str>,
        mut out: Out<'slot, '_, ValueError>,
    ) -> OResult<'slot, ConfigValue> {
        let mut builder = ConfigValueBuilder::new(None, out.reborrow())?;
        let mut names = Vec::new();
        let mut ids = ChargedVec::new(None, out.reborrow())?;
        names.try_reserve_exact(fields.len()).map_err(|_| {
            out.reborrow()
                .fail(ValueError::from(BudgetError::Allocation))
        })?;
        ids.try_reserve_exact(fields.len(), out.reborrow())?;
        for field in fields {
            let id = builder.append_value(field.value, out.reborrow())?;
            names.push(field.name);
            ids.try_push(id, out.reborrow())?;
        }
        let named = names
            .iter()
            .zip(&ids)
            .map(|(name, id)| NamedValue::new(name, *id))
            .collect::<Vec<_>>();
        let root = match enumeration {
            Some(name) => builder.add_enumeration(
                name,
                ValuePayload::Structure(&named),
                ValueSource::RustValue,
                out.reborrow(),
            )?,
            None => builder.add_structure(&named, ValueSource::RustValue, out.reborrow())?,
        };
        builder.finish(root, out)
    }

    fn direct<T>(
        operation: impl for<'slot> FnOnce(Out<'slot, 'slot, ValueError>) -> OResult<'slot, T>,
    ) -> T {
        match TestOpeo::call(operation) {
            Ok(value) => value,
            Err(_) => abort(),
        }
    }
}
