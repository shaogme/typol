mod text;

use opeo::{ErrSlot, OResult, Out, ResultOutExt};
use typol_core::{
    BudgetError, ChargedVec, ConfigValue, ExecutionBudget, ExecutionLimits, ValueEntries,
    ValueKind, ValuePayloadView, ValueView,
};
use typol_model::BuiltinType;
use typol_policy::ValueLimits;
use typol_semantic::{
    CompiledSchema, FieldList, SchemaAccessError, TypeKind, TypeView, VariantPayloadInfo,
};

use crate::{DisplayError, DisplayFailurePoint, display::text::TextBuffer};

macro_rules! schema_access {
    ($formatter:expr, $out:expr, $call:expr) => {{
        match $formatter
            .schema_errors
            .try_call(|schema_out| $call(schema_out))
        {
            Ok(value) => value,
            Err(caught) => {
                return Err($out.fail(DisplayError::from(caught.take())));
            }
        }
    }};
}

macro_rules! budget_access {
    ($formatter:expr, $out:expr, $call:expr) => {{
        match $formatter
            .budget_errors
            .try_call(|budget_out| $call(budget_out))
        {
            Ok(value) => value,
            Err(caught) => {
                return Err($out.fail(DisplayError::from(caught.take())));
            }
        }
    }};
}

struct Node<'a> {
    ty: TypeView<'a>,
    value: ValueView<'a>,
    depth: usize,
    secret: bool,
}

struct FieldRender<'a> {
    name: &'a str,
    ty: TypeView<'a>,
    value: ValueView<'a>,
    secret: bool,
}

struct MapRender<'a> {
    key: &'a str,
    value: ValueView<'a>,
}

enum Frame<'a> {
    Value(Node<'a>),
    Text(&'static str),
    Fields {
        entries: ChargedVec<FieldRender<'a>>,
        next: usize,
        depth: usize,
    },
    Array {
        ty: TypeView<'a>,
        values: ChargedVec<ValueView<'a>>,
        next: usize,
        depth: usize,
    },
    Map {
        ty: TypeView<'a>,
        entries: ChargedVec<MapRender<'a>>,
        next: usize,
        depth: usize,
    },
}

struct Formatter {
    output: TextBuffer,
    budget: ExecutionBudget,
    resources: ValueLimits,
    nodes: usize,
    schema_errors: ErrSlot<SchemaAccessError>,
    budget_errors: ErrSlot<BudgetError>,
}

struct FieldFrame<'a> {
    fields: FieldList<'a>,
    values: ValueEntries<'a>,
    depth: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct DisplayOptions {
    resources: ValueLimits,
    execution: ExecutionLimits,
    max_output_bytes: usize,
}

impl DisplayOptions {
    pub const fn new(
        resources: ValueLimits,
        execution: ExecutionLimits,
        max_output_bytes: usize,
    ) -> Self {
        Self {
            resources,
            execution,
            max_output_bytes,
        }
    }

    pub const fn resources(&self) -> &ValueLimits {
        &self.resources
    }

    pub const fn execution(&self) -> ExecutionLimits {
        self.execution
    }

    pub const fn max_output_bytes(&self) -> usize {
        self.max_output_bytes
    }
}

impl Default for DisplayOptions {
    fn default() -> Self {
        Self::new(
            ValueLimits::default(),
            ExecutionLimits::new(16_777_216, 67_108_864),
            1_048_576,
        )
    }
}

/// Checks the visible shape from the actual Schema entry point; secret boundaries are fully redacted without accessing their contents.
/// The returned value is terminal-safe display text, not a real serialization or reparsable configuration.
pub struct SafeValueFormatter;

impl SafeValueFormatter {
    pub fn format<'slot>(
        schema: &CompiledSchema,
        value: &ConfigValue,
        options: DisplayOptions,
        mut out: Out<'slot, '_, DisplayError>,
    ) -> OResult<'slot, String> {
        let mut budget_errors = ErrSlot::<BudgetError>::new();
        let budget = match budget_errors
            .try_call(|budget_out| ExecutionBudget::new(options.execution(), budget_out))
        {
            Ok(budget) => budget,
            Err(caught) => {
                return Err(out.fail(DisplayError::from(*caught.get())));
            }
        };
        let output = TextBuffer::new(
            &budget,
            options.max_output_bytes(),
            options.resources().max_string_bytes(),
            out.reborrow(),
        )?;
        let mut formatter = Formatter {
            output,
            budget,
            resources: *options.resources(),
            nodes: 0,
            schema_errors: ErrSlot::new(),
            budget_errors,
        };
        let mut frames = ChargedVec::new(Some(&formatter.budget), out.reborrow())?;
        push(
            &mut frames,
            Frame::Value(Node {
                ty: schema.entry(),
                value: value.root(),
                depth: 0,
                secret: false,
            }),
            out.reborrow(),
        )?;
        while let Some(frame) = frames.pop() {
            formatter.budget.consume(1, out.reborrow())?;
            formatter.frame(frame, &mut frames, out.reborrow())?;
        }
        drop(frames);
        formatter.output.finish(out.reborrow())
    }
}

impl Formatter {
    fn visit<'slot>(
        &mut self,
        depth: usize,
        mut out: Out<'slot, '_, DisplayError>,
    ) -> OResult<'slot, ()> {
        self.budget.consume(1, out.reborrow())?;
        self.nodes = self
            .nodes
            .checked_add(1)
            .ok_or(DisplayError::ResourceLimitExceeded)
            .store_err_into(out.reborrow())?;
        if depth > self.resources.max_nesting_depth()
            || self.nodes > self.resources.max_value_nodes()
        {
            return Err(out.fail(DisplayError::ResourceLimitExceeded));
        }
        Ok(())
    }

    fn secret<'slot>(
        &mut self,
        mut ty: TypeView<'_>,
        mut out: Out<'slot, '_, DisplayError>,
    ) -> OResult<'slot, bool> {
        loop {
            self.budget.consume(1, out.reborrow())?;
            if schema_access!(self, out, |schema_out| ty.is_secret(schema_out)) {
                return Ok(true);
            }
            match schema_access!(self, out, |schema_out| ty.kind(schema_out)) {
                TypeKind::Option { inner } => ty = inner,
                _ => return Ok(false),
            }
        }
    }

    fn fields<'a, 'slot>(
        &mut self,
        frame: FieldFrame<'a>,
        frames: &mut ChargedVec<Frame<'a>>,
        mut out: Out<'slot, '_, DisplayError>,
    ) -> OResult<'slot, ()> {
        let FieldFrame {
            fields,
            values,
            depth,
        } = frame;
        if fields.len() != values.len() {
            return Err(out.fail(DisplayError::ShapeMismatch));
        }
        let mut supplied = ChargedVec::new(Some(&self.budget), out.reborrow())?;
        supplied.try_reserve_exact(values.len(), out.reborrow())?;
        for value in values {
            supplied.try_push(value, out.reborrow())?;
        }
        let mut slots = ChargedVec::new(Some(&self.budget), out.reborrow())?;
        slots.try_reserve_exact(fields.len(), out.reborrow())?;
        slots.resize(fields.len(), usize::MAX, out.reborrow())?;
        for (index, value) in supplied.iter().enumerate() {
            self.budget.consume(1, out.reborrow())?;
            let name = value
                .name()
                .ok_or(DisplayError::ShapeMismatch)
                .store_err_into(out.reborrow())?;
            let (slot, _) = budget_access!(self, out, |budget_out| {
                fields.named_budgeted(name, &self.budget, budget_out)
            })
            .ok_or(DisplayError::ShapeMismatch)
            .store_err_into(out.reborrow())?;
            let slot_value = slots
                .get_mut(slot)
                .ok_or(DisplayError::ShapeMismatch)
                .store_err_into(out.reborrow())?;
            if *slot_value != usize::MAX {
                return Err(out.fail(DisplayError::ShapeMismatch));
            }
            *slot_value = index;
        }
        let mut entries = ChargedVec::new(Some(&self.budget), out.reborrow())?;
        entries.try_reserve_exact(fields.len(), out.reborrow())?;
        for (slot, field) in fields.iter().enumerate() {
            let value_index = slots
                .get(slot)
                .copied()
                .filter(|index| *index != usize::MAX)
                .ok_or(DisplayError::ShapeMismatch)
                .store_err_into(out.reborrow())?;
            let value = supplied
                .get(value_index)
                .ok_or(DisplayError::ShapeMismatch)
                .store_err_into(out.reborrow())?;
            entries.try_push(
                FieldRender {
                    name: schema_access!(self, out, |schema_out| field.name(schema_out)),
                    ty: schema_access!(self, out, |schema_out| field.ty(schema_out)),
                    value: value.value(),
                    secret: schema_access!(self, out, |schema_out| field.is_secret(schema_out)),
                },
                out.reborrow(),
            )?;
        }
        self.output.write("{", out.reborrow())?;
        push(
            frames,
            Frame::Fields {
                entries,
                next: 0,
                depth,
            },
            out.reborrow(),
        )
    }

    fn frame<'a, 'slot>(
        &mut self,
        frame: Frame<'a>,
        frames: &mut ChargedVec<Frame<'a>>,
        mut out: Out<'slot, '_, DisplayError>,
    ) -> OResult<'slot, ()> {
        match frame {
            Frame::Text(text) => self.output.write(text, out.reborrow()),
            Frame::Value(node) => self.value(node, frames, out.reborrow()),
            Frame::Fields {
                entries,
                next,
                depth,
            } => {
                let Some(entry) = entries.get(next) else {
                    return self.output.write("}", out.reborrow());
                };
                if next > 0 {
                    self.output.write(", ", out.reborrow())?;
                }
                self.output.escape(entry.name, out.reborrow())?;
                self.output.write(": ", out.reborrow())?;
                let child_depth = child_depth(depth, out.reborrow())?;
                let node = Node {
                    ty: entry.ty,
                    value: entry.value,
                    depth: child_depth,
                    secret: entry.secret,
                };
                push(
                    frames,
                    Frame::Fields {
                        entries,
                        next: next + 1,
                        depth,
                    },
                    out.reborrow(),
                )?;
                push(frames, Frame::Value(node), out.reborrow())
            }
            Frame::Array {
                ty,
                values,
                next,
                depth,
            } => {
                let Some(value) = values.get(next).copied() else {
                    return self.output.write("]", out.reborrow());
                };
                if next > 0 {
                    self.output.write(", ", out.reborrow())?;
                }
                let child_depth = child_depth(depth, out.reborrow())?;
                push(
                    frames,
                    Frame::Array {
                        ty,
                        values,
                        next: next + 1,
                        depth,
                    },
                    out.reborrow(),
                )?;
                push(
                    frames,
                    Frame::Value(Node {
                        ty,
                        value,
                        depth: child_depth,
                        secret: false,
                    }),
                    out.reborrow(),
                )
            }
            Frame::Map {
                ty,
                entries,
                next,
                depth,
            } => {
                let Some(entry) = entries.get(next) else {
                    return self.output.write("}", out.reborrow());
                };
                if next > 0 {
                    self.output.write(", ", out.reborrow())?;
                }
                let child_depth = child_depth(depth, out.reborrow())?;
                self.visit(child_depth, out.reborrow())?;
                self.output.string(entry.key, out.reborrow())?;
                self.output.write(": ", out.reborrow())?;
                let value = entry.value;
                push(
                    frames,
                    Frame::Map {
                        ty,
                        entries,
                        next: next + 1,
                        depth,
                    },
                    out.reborrow(),
                )?;
                push(
                    frames,
                    Frame::Value(Node {
                        ty,
                        value,
                        depth: child_depth,
                        secret: false,
                    }),
                    out.reborrow(),
                )
            }
        }
    }

    fn value<'a, 'slot>(
        &mut self,
        node: Node<'a>,
        frames: &mut ChargedVec<Frame<'a>>,
        mut out: Out<'slot, '_, DisplayError>,
    ) -> OResult<'slot, ()> {
        let Node {
            ty,
            value,
            depth,
            secret,
        } = node;
        self.visit(depth, out.reborrow())?;
        if secret || self.secret(ty, out.reborrow())? {
            return self.output.write("********", out.reborrow());
        }
        // When unwrapping an option reveals a secret as a collection’s direct element, its key, length, and presence are all hidden.
        match schema_access!(self, out, |schema_out| ty.kind(schema_out)) {
            TypeKind::Array { element, .. } if self.secret(element, out.reborrow())? => {
                return self.output.write("********", out.reborrow());
            }
            TypeKind::Map { value } if self.secret(value, out.reborrow())? => {
                return self.output.write("********", out.reborrow());
            }
            _ => {}
        }
        match (
            schema_access!(self, out, |schema_out| ty.kind(schema_out)),
            value.as_ref(),
        ) {
            (TypeKind::Builtin(builtin), value) => self.scalar(builtin, value, out.reborrow()),
            (TypeKind::Option { .. }, ValueKind::Optional(None)) => {
                self.output.write("none", out.reborrow())
            }
            (TypeKind::Option { inner }, ValueKind::Optional(Some(value))) => push(
                frames,
                Frame::Value(Node {
                    ty: inner,
                    value,
                    depth: child_depth(depth, out.reborrow())?,
                    secret: false,
                }),
                out.reborrow(),
            ),
            (TypeKind::Array { element, length }, ValueKind::Array(values)) => {
                if length.is_some_and(|length| length as usize != values.len()) {
                    return Err(out.fail(DisplayError::ShapeMismatch));
                }
                let mut children = ChargedVec::new(Some(&self.budget), out.reborrow())?;
                children.try_reserve_exact(values.len(), out.reborrow())?;
                for value in values {
                    self.budget.consume(1, out.reborrow())?;
                    children.try_push(value.value(), out.reborrow())?;
                }
                self.output.write("[", out.reborrow())?;
                push(
                    frames,
                    Frame::Array {
                        ty: element,
                        values: children,
                        next: 0,
                        depth,
                    },
                    out.reborrow(),
                )
            }
            (TypeKind::Map { value: ty }, ValueKind::Map(values)) => {
                let mut entries = ChargedVec::new(Some(&self.budget), out.reborrow())?;
                entries.try_reserve_exact(values.len(), out.reborrow())?;
                for entry in values {
                    self.budget.consume(1, out.reborrow())?;
                    let key = entry
                        .name()
                        .ok_or(DisplayError::ShapeMismatch)
                        .store_err_into(out.reborrow())?;
                    entries.try_push(
                        MapRender {
                            key,
                            value: entry.value(),
                        },
                        out.reborrow(),
                    )?;
                }
                entries.sort_by(
                    |left, right, compare_out| {
                        self.budget.compare_text(left.key, right.key, compare_out)
                    },
                    out.reborrow(),
                )?;
                for pair in entries.windows(2) {
                    self.budget.consume(1, out.reborrow())?;
                    let left = pair
                        .first()
                        .ok_or(DisplayError::internal(
                            DisplayFailurePoint::CollectionInvariant,
                        ))
                        .store_err_into(out.reborrow())?;
                    let right = pair
                        .get(1)
                        .ok_or(DisplayError::internal(
                            DisplayFailurePoint::CollectionInvariant,
                        ))
                        .store_err_into(out.reborrow())?;
                    if self
                        .budget
                        .compare_text(left.key, right.key, out.reborrow())?
                        .is_eq()
                    {
                        return Err(out.fail(DisplayError::ShapeMismatch));
                    }
                }
                self.output.write("{", out.reborrow())?;
                push(
                    frames,
                    Frame::Map {
                        ty,
                        entries,
                        next: 0,
                        depth,
                    },
                    out.reborrow(),
                )
            }
            (TypeKind::Struct { fields }, ValueKind::Structure(values)) => self.fields(
                FieldFrame {
                    fields,
                    values,
                    depth,
                },
                frames,
                out.reborrow(),
            ),
            (TypeKind::Enum { .. }, ValueKind::Enumeration { variant, payload }) => {
                let variant = budget_access!(self, out, |budget_out| {
                    ty.variant_named_budgeted(variant, &self.budget, budget_out)
                })
                .ok_or(DisplayError::ShapeMismatch)
                .store_err_into(out.reborrow())?;
                if schema_access!(self, out, |schema_out| variant.is_secret(schema_out)) {
                    return self.output.write("********", out.reborrow());
                }
                if let VariantPayloadInfo::Single { ty } =
                    schema_access!(self, out, |schema_out| variant.payload(schema_out))
                {
                    if self.secret(ty, out.reborrow())? {
                        return self.output.write("********", out.reborrow());
                    }
                }
                if matches!(
                    schema_access!(self, out, |schema_out| variant.payload(schema_out)),
                    VariantPayloadInfo::Structure { .. }
                ) {
                    let structure = schema_access!(self, out, |schema_out| {
                        variant.structure_type(schema_out)
                    })
                    .ok_or(DisplayError::internal(
                        DisplayFailurePoint::EnumStructureLookup,
                    ));
                    let structure = structure.store_err_into(out.reborrow())?;
                    if self.secret(structure, out.reborrow())? {
                        return self.output.write("********", out.reborrow());
                    }
                }
                self.output.write(".", out.reborrow())?;
                self.output.escape(
                    schema_access!(self, out, |schema_out| variant.name(schema_out)),
                    out.reborrow(),
                )?;
                match (
                    schema_access!(self, out, |schema_out| variant.payload(schema_out)),
                    payload,
                ) {
                    (VariantPayloadInfo::Unit, ValuePayloadView::Unit) => Ok(()),
                    (VariantPayloadInfo::Single { ty }, ValuePayloadView::Single(value)) => {
                        self.output.write("(", out.reborrow())?;
                        push(frames, Frame::Text(")"), out.reborrow())?;
                        let child_depth = child_depth(depth, out.reborrow())?;
                        push(
                            frames,
                            Frame::Value(Node {
                                ty,
                                value,
                                depth: child_depth,
                                secret: false,
                            }),
                            out.reborrow(),
                        )
                    }
                    (
                        VariantPayloadInfo::Structure { fields },
                        ValuePayloadView::Structure(values),
                    ) => {
                        let child_depth = child_depth(depth, out.reborrow())?;
                        self.visit(child_depth, out.reborrow())?;
                        self.output.write(" ", out.reborrow())?;
                        self.fields(
                            FieldFrame {
                                fields,
                                values,
                                depth: child_depth,
                            },
                            frames,
                            out.reborrow(),
                        )
                    }
                    _ => Err(out.fail(DisplayError::ShapeMismatch)),
                }
            }
            _ => Err(out.fail(DisplayError::ShapeMismatch)),
        }
    }

    fn scalar<'slot>(
        &mut self,
        builtin: BuiltinType,
        value: ValueKind<'_>,
        mut out: Out<'slot, '_, DisplayError>,
    ) -> OResult<'slot, ()> {
        match (builtin, value) {
            (BuiltinType::Bool, ValueKind::Bool(value)) => self
                .output
                .write(if value { "true" } else { "false" }, out.reborrow()),
            (BuiltinType::String, ValueKind::String(value)) => {
                self.output.string(value, out.reborrow())
            }
            (BuiltinType::F32, ValueKind::Float32(value)) if value.is_finite() => {
                self.output.float(value, out.reborrow())
            }
            (BuiltinType::F64, ValueKind::Float64(value)) if value.is_finite() => {
                self.output.float(value, out.reborrow())
            }
            (ty, ValueKind::Signed(value)) if signed_fits(ty, value) => {
                self.output.number(value, out.reborrow())
            }
            (ty, ValueKind::Unsigned(value)) if unsigned_fits(ty, value) => {
                self.output.number(value, out.reborrow())
            }
            _ => Err(out.fail(DisplayError::ShapeMismatch)),
        }
    }
}

fn push<'a, 'slot>(
    frames: &mut ChargedVec<Frame<'a>>,
    frame: Frame<'a>,
    mut out: Out<'slot, '_, DisplayError>,
) -> OResult<'slot, ()> {
    if let Some(budget) = frames.budget() {
        budget.consume(1, out.reborrow())?;
    }
    frames.try_push(frame, out)
}

fn signed_fits(ty: BuiltinType, value: i128) -> bool {
    match ty {
        BuiltinType::I8 => i8::try_from(value).is_ok(),
        BuiltinType::I16 => i16::try_from(value).is_ok(),
        BuiltinType::I32 => i32::try_from(value).is_ok(),
        BuiltinType::I64 => i64::try_from(value).is_ok(),
        BuiltinType::I128 => true,
        _ => false,
    }
}

fn unsigned_fits(ty: BuiltinType, value: u128) -> bool {
    match ty {
        BuiltinType::U8 => u8::try_from(value).is_ok(),
        BuiltinType::U16 => u16::try_from(value).is_ok(),
        BuiltinType::U32 => u32::try_from(value).is_ok(),
        BuiltinType::U64 => u64::try_from(value).is_ok(),
        BuiltinType::U128 => true,
        _ => false,
    }
}

fn child_depth<'slot, 'borrow>(
    depth: usize,
    out: Out<'slot, 'borrow, DisplayError>,
) -> OResult<'slot, usize> {
    match depth.checked_add(1) {
        Some(depth) => Ok(depth),
        None => Err(out.fail(DisplayError::ResourceLimitExceeded)),
    }
}
