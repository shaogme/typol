use crate::{
    conversion::{ConversionContext, ConversionValue},
    descriptor::{
        BudgetedTypol, DescribeContext, DescriptorNode, DescriptorProvider, MapContainer,
        TypeDescriptor, TypeShape, Typol,
    },
    diagnostics::{
        CodecErrors, CodecStage, ConversionDiagnosticInput, ConversionErrorKind, Failure,
    },
    value_walk::{NAMED_ENTRY_NAME_MISSING, ValueNodeKind},
};
use alloc::{collections::TryReserveError, string::String, vec::Vec};
use core::{
    any::TypeId,
    cmp::Ordering,
    mem::{replace, size_of},
};
use opeo::{OResult, Out};
use typol_core::{ChargedVec, NamedValue, ValueId, ValueView};

#[cfg(feature = "std")]
use std::collections::HashMap as StdHashMap;

#[cfg(feature = "hashbrown")]
use hashbrown::HashMap as HashbrownMap;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrderedMap<T> {
    entries: Vec<(String, T)>,
}

#[cfg(any(feature = "std", feature = "hashbrown"))]
trait HashMapAdapter {
    type Map<T>;

    fn new<T>() -> Self::Map<T>;
    fn try_reserve<T>(map: &mut Self::Map<T>, additional: usize) -> bool;
    fn capacity<T>(map: &Self::Map<T>) -> usize;
    fn insert<T>(map: &mut Self::Map<T>, key: String, value: T) -> Option<T>;
}

#[cfg(feature = "std")]
struct StdHashMapAdapter;

#[cfg(feature = "hashbrown")]
struct HashbrownMapAdapter;

impl<T> Default for OrderedMap<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> OrderedMap<T> {
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, key: &str) -> Option<&T> {
        let index = self
            .entries
            .partition_point(|(entry, _)| entry.as_str() < key);
        self.entries
            .get(index)
            .filter(|(entry, _)| entry.as_str() == key)
            .map(|(_, value)| value)
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = (&str, &T)> + ExactSizeIterator {
        self.entries
            .iter()
            .map(|(key, value)| (key.as_str(), value))
    }

    pub fn try_insert(&mut self, key: String, value: T) -> Result<Option<T>, TryReserveError> {
        let index = self
            .entries
            .partition_point(|(entry, _)| entry.as_str() < key.as_str());
        if let Some((entry, current)) = self.entries.get_mut(index) {
            if entry.as_str() == key.as_str() {
                return Ok(Some(replace(current, value)));
            }
        }
        self.entries.try_reserve(1)?;
        self.entries.insert(index, (key, value));
        Ok(None)
    }
}

#[cfg(feature = "std")]
impl HashMapAdapter for StdHashMapAdapter {
    type Map<T> = StdHashMap<String, T>;

    fn new<T>() -> Self::Map<T> {
        StdHashMap::new()
    }

    fn try_reserve<T>(map: &mut Self::Map<T>, additional: usize) -> bool {
        map.try_reserve(additional).is_ok()
    }

    fn capacity<T>(map: &Self::Map<T>) -> usize {
        map.capacity()
    }

    fn insert<T>(map: &mut Self::Map<T>, key: String, value: T) -> Option<T> {
        map.insert(key, value)
    }
}

#[cfg(feature = "hashbrown")]
impl HashMapAdapter for HashbrownMapAdapter {
    type Map<T> = HashbrownMap<String, T>;

    fn new<T>() -> Self::Map<T> {
        HashbrownMap::new()
    }

    fn try_reserve<T>(map: &mut Self::Map<T>, additional: usize) -> bool {
        map.try_reserve(additional).is_ok()
    }

    fn capacity<T>(map: &Self::Map<T>) -> usize {
        map.capacity()
    }

    fn insert<T>(map: &mut Self::Map<T>, key: String, value: T) -> Option<T> {
        map.insert(key, value)
    }
}

impl<T: Typol> Typol for Option<T> {
    fn describe<'slot, 'borrow>(
        context: &mut DescribeContext<'_>,
        out: Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, TypeDescriptor> {
        describe_container::<Self>(
            context,
            TypeShape::Option(DescriptorProvider::of::<T>()),
            out,
        )
    }

    fn to_config_value<'slot>(
        &self,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        context.start::<Self>(out.reborrow())?;
        let ty = context.element_type(out.reborrow())?;
        match self {
            Some(value) => {
                let value = context.encode_child::<T>(ty, value, out.reborrow())?;
                context.optional(Some(value), out.reborrow())
            }
            None => context.none(out.reborrow()),
        }
    }

    fn from_config_value<'slot>(
        value: ConversionValue<'_>,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, Self> {
        context.start::<Self>(out.reborrow())?;
        match context.read_value_node(value.view(), out.reborrow())? {
            ValueNodeKind::Optional {
                inner,
                value: Some(value),
            } => context
                .decode_child::<T>(inner, ConversionValue::new(value), out.reborrow())
                .map(Some),
            ValueNodeKind::Optional { value: None, .. } => Ok(None),
            _ => context.reject_with_path(out.reborrow()),
        }
    }
}

impl<T: BudgetedTypol> BudgetedTypol for Option<T> {}

impl<T: Typol> Typol for Vec<T> {
    fn describe<'slot, 'borrow>(
        context: &mut DescribeContext<'_>,
        out: Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, TypeDescriptor> {
        describe_container::<Self>(
            context,
            TypeShape::Sequence(DescriptorProvider::of::<T>()),
            out,
        )
    }

    fn to_config_value<'slot>(
        &self,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        context.start::<Self>(out.reborrow())?;
        write_sequence(self, context, out.reborrow())
    }

    fn from_config_value<'slot>(
        value: ConversionValue<'_>,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, Self> {
        context.start::<Self>(out.reborrow())?;
        let values =
            read_sequence::<T>(value.view(), context, out.reborrow())?.into_unmetered_vec();
        let Some(bytes) = values.capacity().checked_mul(size_of::<T>()) else {
            return Err(out.fail(ConversionContext::resource_error()));
        };
        context.target_bytes(bytes, out.reborrow())?;
        Ok(values)
    }
}

impl<T: BudgetedTypol> BudgetedTypol for Vec<T> {}

impl<T: Typol, const N: usize> Typol for [T; N] {
    fn describe<'slot>(
        context: &mut DescribeContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, TypeDescriptor> {
        let length = DescribeContext::array_length(N, out.reborrow())?;
        describe_container::<Self>(
            context,
            TypeShape::Array {
                element: DescriptorProvider::of::<T>(),
                length,
            },
            out.reborrow(),
        )
    }

    fn to_config_value<'slot>(
        &self,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        context.start::<Self>(out.reborrow())?;
        write_sequence(self, context, out.reborrow())
    }

    fn from_config_value<'slot>(
        value: ConversionValue<'_>,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, Self> {
        context.start::<Self>(out.reborrow())?;
        let values =
            read_sequence::<T>(value.view(), context, out.reborrow())?.into_unmetered_vec();
        match values.try_into() {
            Ok(values) => Ok(values),
            Err(_) => context.fail_as(
                ConversionDiagnosticInput::new(
                    ConversionErrorKind::ArrayLengthMismatch,
                    "value with the Rust fixed-array length",
                    "array length mismatch",
                    context.path,
                ),
                out.reborrow(),
            ),
        }
    }
}

impl<T: BudgetedTypol, const N: usize> BudgetedTypol for [T; N] {}

impl<T: Typol> Typol for OrderedMap<T> {
    fn describe<'slot, 'borrow>(
        context: &mut DescribeContext<'_>,
        out: Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, TypeDescriptor> {
        describe_container::<Self>(
            context,
            TypeShape::Map {
                container: MapContainer::OrderedMap,
                value: DescriptorProvider::of::<T>(),
            },
            out,
        )
    }

    fn to_config_value<'slot>(
        &self,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        context.start::<Self>(out.reborrow())?;
        write_map(self.iter(), context, out.reborrow())
    }

    fn from_config_value<'slot>(
        value: ConversionValue<'_>,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, Self> {
        context.start::<Self>(out.reborrow())?;
        read_ordered_map(value, context, out.reborrow())
    }
}

impl<T: BudgetedTypol> BudgetedTypol for OrderedMap<T> {}

#[cfg(feature = "std")]
impl<T: Typol> Typol for StdHashMap<String, T> {
    fn describe<'slot, 'borrow>(
        context: &mut DescribeContext<'_>,
        out: Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, TypeDescriptor> {
        describe_container::<Self>(
            context,
            TypeShape::Map {
                container: MapContainer::HashMap,
                value: DescriptorProvider::of::<T>(),
            },
            out,
        )
    }

    fn to_config_value<'slot>(
        &self,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        context.start::<Self>(out.reborrow())?;
        write_map(
            self.iter().map(|(key, value)| (key.as_str(), value)),
            context,
            out.reborrow(),
        )
    }

    fn from_config_value<'slot>(
        value: ConversionValue<'_>,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, Self> {
        context.start::<Self>(out.reborrow())?;
        read_hash_map::<T, StdHashMapAdapter>(value, context, out.reborrow())
    }
}

#[cfg(feature = "std")]
impl<T: BudgetedTypol> BudgetedTypol for StdHashMap<String, T> {}

#[cfg(feature = "hashbrown")]
impl<T: Typol> Typol for HashbrownMap<String, T> {
    fn describe<'slot, 'borrow>(
        context: &mut DescribeContext<'_>,
        out: Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, TypeDescriptor> {
        describe_container::<Self>(
            context,
            TypeShape::Map {
                container: MapContainer::HashMap,
                value: DescriptorProvider::of::<T>(),
            },
            out,
        )
    }

    fn to_config_value<'slot>(
        &self,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        context.start::<Self>(out.reborrow())?;
        write_map(
            self.iter().map(|(key, value)| (key.as_str(), value)),
            context,
            out.reborrow(),
        )
    }

    fn from_config_value<'slot>(
        value: ConversionValue<'_>,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, Self> {
        context.start::<Self>(out.reborrow())?;
        read_hash_map::<T, HashbrownMapAdapter>(value, context, out.reborrow())
    }
}

#[cfg(feature = "hashbrown")]
impl<T: BudgetedTypol> BudgetedTypol for HashbrownMap<String, T> {}

#[cfg(any(feature = "std", feature = "hashbrown"))]
fn read_hash_map<'slot, T: Typol, Adapter: HashMapAdapter>(
    value: ConversionValue<'_>,
    context: &mut ConversionContext<'_>,
    mut out: Out<'slot, '_, CodecErrors>,
) -> OResult<'slot, Adapter::Map<T>> {
    let ValueNodeKind::Map {
        value_type: ty,
        values: entries,
    } = context.read_value_node(value.view(), out.reborrow())?
    else {
        return context.reject_with_path(out.reborrow());
    };
    let Some(bytes) = entries.len().checked_mul(size_of::<(String, T)>()) else {
        return Err(out.fail(ConversionContext::resource_error()));
    };
    context.target_bytes(bytes, out.reborrow())?;
    let mut result = Adapter::new::<T>();
    if !Adapter::try_reserve(&mut result, entries.len()) {
        return Err(out.fail(ConversionContext::allocation_error()));
    }
    let Some(actual) = Adapter::capacity(&result).checked_mul(size_of::<(String, T)>()) else {
        return Err(out.fail(ConversionContext::resource_error()));
    };
    let Some(extra) = actual.checked_sub(bytes) else {
        return Err(out.fail(ConversionContext::resource_error()));
    };
    context.target_bytes(extra, out.reborrow())?;
    for entry in entries {
        context.child_with_path(
            None,
            |child, child_out| child.node(child_out),
            out.reborrow(),
        )?;
        let Some(entry) = entry.as_named() else {
            return Err(out.fail(CodecErrors::emergency(
                CodecStage::Conversion,
                Failure::internal(NAMED_ENTRY_NAME_MISSING),
            )));
        };
        let key = context.target_text(entry.name(), out.reborrow())?;
        context.session().budget_codec(
            CodecStage::Conversion,
            |budget_out| context.session().execution().consume(key.len(), budget_out),
            out.reborrow(),
        )?;
        let value =
            context.decode_child::<T>(ty, ConversionValue::new(entry.value()), out.reborrow())?;
        if Adapter::insert(&mut result, key, value).is_some() {
            return context.reject_with_path(out.reborrow());
        }
    }
    Ok(result)
}

fn read_ordered_map<'slot, T: Typol>(
    value: ConversionValue<'_>,
    context: &mut ConversionContext<'_>,
    mut out: Out<'slot, '_, CodecErrors>,
) -> OResult<'slot, OrderedMap<T>> {
    let ValueNodeKind::Map {
        value_type: ty,
        values: entries,
    } = context.read_value_node(value.view(), out.reborrow())?
    else {
        return context.reject_with_path(out.reborrow());
    };
    let mut sorted: ChargedVec<(&str, ValueView<'_>)> = context.session().budget_codec(
        CodecStage::Conversion,
        |budget_out| context.session().workspace(budget_out),
        out.reborrow(),
    )?;
    context.session().budget_codec(
        CodecStage::Conversion,
        |budget_out| sorted.try_reserve_exact(entries.len(), budget_out),
        out.reborrow(),
    )?;
    for entry in entries {
        context.child_with_path(
            None,
            |child, child_out| child.node(child_out),
            out.reborrow(),
        )?;
        let Some(entry) = entry.as_named() else {
            return Err(out.fail(CodecErrors::emergency(
                CodecStage::Conversion,
                Failure::internal(NAMED_ENTRY_NAME_MISSING),
            )));
        };
        context.session().budget_codec(
            CodecStage::Conversion,
            |budget_out| sorted.try_push((entry.name(), entry.value()), budget_out),
            out.reborrow(),
        )?;
    }
    sorted.sort_by(
        |left, right, sort_out| {
            context.session().budget_codec(
                CodecStage::Conversion,
                |budget_out| context.session().compare_text(left.0, right.0, budget_out),
                sort_out,
            )
        },
        out.reborrow(),
    )?;
    let mut previous = None;
    for (key, _) in sorted.iter() {
        let duplicate = if let Some(previous_key) = previous {
            context.session().budget_codec(
                CodecStage::Conversion,
                |mut budget_out| {
                    context.session().visit(budget_out.reborrow())?;
                    context
                        .session()
                        .compare_text(previous_key, key, budget_out)
                },
                out.reborrow(),
            )? == Ordering::Equal
        } else {
            false
        };
        if duplicate {
            return context.reject_with_path(out.reborrow());
        }
        previous = Some(*key);
    }
    let Some(bytes) = sorted.len().checked_mul(size_of::<(String, T)>()) else {
        return Err(out.fail(ConversionContext::resource_error()));
    };
    context.target_bytes(bytes, out.reborrow())?;
    let mut values = Vec::new();
    if values.try_reserve_exact(sorted.len()).is_err() {
        return Err(out.fail(ConversionContext::allocation_error()));
    }
    let Some(actual) = values.capacity().checked_mul(size_of::<(String, T)>()) else {
        return Err(out.fail(ConversionContext::resource_error()));
    };
    let Some(extra) = actual.checked_sub(bytes) else {
        return Err(out.fail(ConversionContext::resource_error()));
    };
    context.target_bytes(extra, out.reborrow())?;
    for (name, source_value) in sorted {
        let key = context.target_text(name, out.reborrow())?;
        context.session().budget_codec(
            CodecStage::Conversion,
            |budget_out| context.session().execution().consume(key.len(), budget_out),
            out.reborrow(),
        )?;
        let value =
            context.decode_child::<T>(ty, ConversionValue::new(source_value), out.reborrow())?;
        values.push((key, value));
    }
    Ok(OrderedMap { entries: values })
}

fn describe_container<'slot, 'borrow, T: 'static>(
    context: &mut DescribeContext<'_>,
    shape: TypeShape,
    out: Out<'slot, 'borrow, CodecErrors>,
) -> OResult<'slot, TypeDescriptor> {
    context.insert(
        DescriptorNode::Type {
            identity: TypeId::of::<T>(),
            name: None,
            shape,
        },
        context.depth(),
        out,
    )
}

fn write_sequence<'slot, T: Typol>(
    values: &[T],
    context: &mut ConversionContext<'_>,
    mut out: Out<'slot, '_, CodecErrors>,
) -> OResult<'slot, ValueId> {
    let ty = context.element_type(out.reborrow())?;
    let mut result: ChargedVec<ValueId> = context.session().budget_codec(
        CodecStage::Conversion,
        |budget_out| context.session().workspace(budget_out),
        out.reborrow(),
    )?;
    for (index, value) in values.iter().enumerate() {
        let converted = context.encode_indexed_child::<T>(index, ty, value, out.reborrow())?;
        context.session().budget_codec(
            CodecStage::Conversion,
            |budget_out| result.try_push(converted, budget_out),
            out.reborrow(),
        )?;
    }
    context.array(result.as_slice(), out)
}

fn read_sequence<'slot, T: Typol>(
    value: ValueView<'_>,
    context: &mut ConversionContext<'_>,
    mut out: Out<'slot, '_, CodecErrors>,
) -> OResult<'slot, ChargedVec<T>> {
    let ValueNodeKind::Array {
        element: ty,
        values,
        ..
    } = context.read_value_node(value, out.reborrow())?
    else {
        return context.reject_with_path(out.reborrow());
    };
    let mut result = context.session().budget_codec(
        CodecStage::Conversion,
        |budget_out| context.session().workspace(budget_out),
        out.reborrow(),
    )?;
    context.session().budget_codec(
        CodecStage::Conversion,
        |budget_out| result.try_reserve_exact(values.len(), budget_out),
        out.reborrow(),
    )?;
    for (index, value) in values.enumerate() {
        let converted = context.decode_indexed_child::<T>(
            index,
            ty,
            ConversionValue::new(value.value()),
            out.reborrow(),
        )?;
        context.session().budget_codec(
            CodecStage::Conversion,
            |budget_out| result.try_push(converted, budget_out),
            out.reborrow(),
        )?;
    }
    Ok(result)
}

fn write_map<'slot, 'value, T: Typol>(
    entries: impl ExactSizeIterator<Item = (&'value str, &'value T)>,
    context: &mut ConversionContext<'_>,
    mut out: Out<'slot, '_, CodecErrors>,
) -> OResult<'slot, ValueId> {
    let ty = context.element_type(out.reborrow())?;
    let mut sorted = context.session().budget_codec(
        CodecStage::Conversion,
        |budget_out| context.session().workspace(budget_out),
        out.reborrow(),
    )?;
    context.session().budget_codec(
        CodecStage::Conversion,
        |budget_out| sorted.try_reserve_exact(entries.len(), budget_out),
        out.reborrow(),
    )?;
    for entry in entries {
        context.session().budget_codec(
            CodecStage::Conversion,
            |budget_out| context.session().visit(budget_out),
            out.reborrow(),
        )?;
        context.session().budget_codec(
            CodecStage::Conversion,
            |budget_out| sorted.try_push(entry, budget_out),
            out.reborrow(),
        )?;
    }
    sorted.sort_by(
        |left, right, sort_out| {
            context.session().budget_codec(
                CodecStage::Conversion,
                |budget_out| context.session().compare_text(left.0, right.0, budget_out),
                sort_out,
            )
        },
        out.reborrow(),
    )?;
    let mut result: ChargedVec<NamedValue<'value>> = context.session().budget_codec(
        CodecStage::Conversion,
        |budget_out| context.session().workspace(budget_out),
        out.reborrow(),
    )?;
    context.session().budget_codec(
        CodecStage::Conversion,
        |budget_out| result.try_reserve_exact(sorted.len(), budget_out),
        out.reborrow(),
    )?;
    for (key, value) in sorted {
        context.child_with_path(
            None,
            |child, child_out| child.node(child_out),
            out.reborrow(),
        )?;
        let value = context.encode_child::<T>(ty, value, out.reborrow())?;
        let entry = NamedValue::new(key, value);
        context.session().budget_codec(
            CodecStage::Conversion,
            |budget_out| result.try_push(entry, budget_out),
            out.reborrow(),
        )?;
    }
    context.map(result.as_slice(), out.reborrow())
}
