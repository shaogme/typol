mod binding;
mod containers;
mod pipeline;
mod scalar;
mod value_builder;

#[cfg(all(test, feature = "std"))]
mod tests;

pub use containers::OrderedMap;

use alloc::string::String;
use binding::BindingConversion;
use value_builder::ValueBuilder;

use crate::{
    binding_impl::{FieldBindings, TypedSchema},
    descriptor::Typol,
    diagnostics::{
        CodecErrors, CodecStage, ConversionDiagnosticInput, ConversionErrorKind, ConversionPath,
        ConversionPathSegment, Failure,
    },
    options::ConversionLimits,
    session::{NodeBudget, SessionBudget},
    value_walk::{ValueDepth, ValueNode, ValueNodeKind},
};
use core::{any::TypeId, fmt, mem};
use opeo::{OResult, Out};
use typol_core::{
    BudgetError, NamedValue, ValueError, ValueId, ValueKind, ValuePayload, ValueView,
};
use typol_semantic::{TypeKind, TypeView, VariantPayloadInfo};

pub(crate) use pipeline::ConvertedValue;
pub use pipeline::{
    Conversion, ConversionFields, ConversionPayload, ConversionValue, EnumInput, Sourced,
};

pub(super) use self::pipeline::{BindingCursor, BindingState, ConversionState, ResolvedBinding};

struct ShapeMismatch {
    kind: ConversionErrorKind,
    expected: &'static str,
    actual: &'static str,
}

/// Context for handwritten `Typol` implementations to inspect the current value and dispatch child conversions through the library.
/// `ConversionLimits` bounds conversion nodes and nesting depth reached through this context; execution budgets cover library-provided workspaces and operations; `target_bytes` and `target_text` charge target storage created by handwritten implementations. Use `target_text` for budgeted owned text and report other owned target storage with `target_bytes`.
///
/// These APIs can account only for work routed through them. They cannot intercept allocations, blocking, or arbitrary computation performed directly by an implementation. Public parsing paths validate the input before calling `Typol::from_config_value`, but do not validate the Rust value that the implementation returns.
pub struct ConversionContext<'call> {
    session: &'call SessionBudget,
    builder: Option<&'call mut ValueBuilder>,
    nodes: &'call mut NodeBudget,
    depth: ValueDepth,
    started: bool,
    binding: BindingState<'call>,
    state: Option<&'call mut ConversionState>,
    path: ConversionPath<'call>,
}

impl<'call> ConversionContext<'call> {
    pub(crate) fn new(session: &'call SessionBudget, nodes: &'call mut NodeBudget) -> Self {
        Self {
            session,
            builder: None,
            nodes,
            depth: ValueDepth::root(),
            started: false,
            binding: BindingState::Unbound,
            state: None,
            path: ConversionPath::root(),
        }
    }

    pub(super) fn encoding(
        session: &'call SessionBudget,
        builder: &'call mut ValueBuilder,
        nodes: &'call mut NodeBudget,
    ) -> Self {
        Self {
            session,
            builder: Some(builder),
            nodes,
            depth: ValueDepth::root(),
            started: false,
            binding: BindingState::Unbound,
            state: None,
            path: ConversionPath::root(),
        }
    }

    pub(crate) fn counters(limits: ConversionLimits) -> NodeBudget {
        NodeBudget::new(limits.max_nodes(), limits.max_depth())
    }

    pub(crate) fn node<'slot>(
        &mut self,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ()> {
        self.session.budget_codec(
            CodecStage::Conversion,
            |budget_out| {
                self.nodes
                    .node(self.depth.level(), self.session.execution(), budget_out)
            },
            out.reborrow(),
        )
    }

    pub(crate) fn child_with_path<'child, 'slot, R>(
        &'child mut self,
        segment: Option<ConversionPathSegment<'child>>,
        convert: impl for<'out_borrow> FnOnce(
            &mut ConversionContext<'child>,
            Out<'slot, 'out_borrow, CodecErrors>,
        ) -> OResult<'slot, R>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, R> {
        let Some(depth) = self.depth.child() else {
            return Err(out.fail(CodecErrors::emergency(
                CodecStage::Conversion,
                Failure::Resource,
            )));
        };
        let depth_check = self.session.budget_codec(
            CodecStage::Conversion,
            |budget_out| {
                self.nodes
                    .check_depth(depth.level(), self.session.execution(), budget_out)
            },
            out.reborrow(),
        );
        depth_check?;
        // The child context borrows the same cumulative counters, and returning or failing does not change the parent depth.
        let path: ConversionPath<'child> = match segment {
            Some(segment) => self.path.pushed(segment),
            None => self.path,
        };
        let mut child = ConversionContext {
            session: self.session,
            builder: self.builder.as_deref_mut(),
            nodes: self.nodes,
            depth,
            started: false,
            binding: self.binding,
            state: self.state.as_deref_mut(),
            path,
        };
        convert(&mut child, out.reborrow())
    }

    pub fn budget_error(error: BudgetError) -> CodecErrors {
        CodecErrors::emergency(CodecStage::Conversion, error.into())
    }

    pub fn value_error(error: ValueError) -> CodecErrors {
        CodecErrors::emergency(CodecStage::Conversion, error.into())
    }

    pub fn conversion_error() -> CodecErrors {
        CodecErrors::emergency(CodecStage::Conversion, Failure::Conversion)
    }

    pub fn resource_error() -> CodecErrors {
        CodecErrors::emergency(CodecStage::Conversion, Failure::Resource)
    }

    pub fn allocation_error() -> CodecErrors {
        CodecErrors::emergency(CodecStage::Conversion, Failure::Allocation)
    }

    pub fn start<'slot, T: Typol>(
        &mut self,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ()> {
        // Each node context is single-entry, preventing manual implementations from re-entering it recursively.
        if self.started {
            return Err(out.fail(Self::resource_error()));
        }
        self.check::<T>(out.reborrow())?;
        if self.builder.is_none() {
            self.node(out.reborrow())?;
        }
        let identity = TypeId::of::<T>();
        match &mut self.binding {
            BindingState::Unbound => {
                return Err(out.fail(Conversion::internal_error()));
            }
            BindingState::Type(cursor) | BindingState::Variant { cursor, .. } => {
                cursor.identity = identity;
            }
        }
        self.started = true;
        Ok(())
    }

    pub fn encode_here<'slot, T: Typol>(
        &mut self,
        value: &T,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        if self.started {
            return Err(out.fail(Self::resource_error()));
        }
        let converted = T::to_config_value(value, self, out.reborrow())?;
        if !self.started {
            return Err(out.fail(Self::resource_error()));
        }
        Ok(converted)
    }

    pub fn decode_here<'slot, 'value, T: Typol>(
        &mut self,
        value: ConversionValue<'value>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, T> {
        if self.started {
            return Err(out.fail(Self::resource_error()));
        }
        let converted = T::from_config_value(value, self, out.reborrow())?;
        if !self.started {
            return Err(out.fail(Self::resource_error()));
        }
        Ok(converted)
    }

    pub fn reject<'slot, 'borrow, R>(out: Out<'slot, 'borrow, CodecErrors>) -> OResult<'slot, R> {
        Err(out.fail(CodecErrors::emergency(
            CodecStage::Conversion,
            Failure::Conversion,
        )))
    }

    pub fn reject_with_path<'slot, 'borrow, R>(
        &self,
        out: Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, R> {
        self.fail_as(
            ConversionDiagnosticInput::new(
                ConversionErrorKind::Rejected,
                "valid value defined by a manual Typol implementation",
                "manual conversion rejected the value",
                self.path,
            ),
            out,
        )
    }

    pub(crate) fn fail_as<'slot, 'borrow, R>(
        &self,
        input: ConversionDiagnosticInput<'call>,
        mut out: Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, R> {
        let schema = match self.binding {
            BindingState::Type(cursor) | BindingState::Variant { cursor, .. } => {
                cursor.plan.schema()
            }
            BindingState::Unbound => return Err(out.fail(Conversion::internal_error())),
        };
        let error = CodecErrors::conversion(input, schema, self.session, out.reborrow())?;
        Err(out.fail(error))
    }

    /// Charges target storage created by a handwritten implementation to the request budget.
    pub fn target_bytes<'slot>(
        &mut self,
        bytes: usize,
        out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ()> {
        ConversionState::target_bytes(self.state.as_deref_mut(), self.session, bytes, out)
    }

    /// Copies and returns owned text charged to target-storage and execution budgets.
    pub fn target_text<'slot>(
        &mut self,
        text: &str,
        out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, String> {
        ConversionState::target_text(self.state.as_deref_mut(), self.session, text, out)
    }

    pub fn text<'slot>(
        &mut self,
        text: &str,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        self.node(out.reborrow())?;
        let session = self.session;
        self.value_builder(out.reborrow())?
            .text(session, text, out.reborrow())
    }

    pub fn signed<'slot>(
        &mut self,
        value: i128,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        self.node(out.reborrow())?;
        let session = self.session;
        self.value_builder(out.reborrow())?
            .signed(session, value, out.reborrow())
    }

    pub fn unsigned<'slot>(
        &mut self,
        value: u128,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        self.node(out.reborrow())?;
        let session = self.session;
        self.value_builder(out.reborrow())?
            .unsigned(session, value, out.reborrow())
    }

    pub fn float32<'slot>(
        &mut self,
        value: f32,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        self.node(out.reborrow())?;
        let session = self.session;
        self.value_builder(out.reborrow())?
            .float32(session, value, out.reborrow())
    }

    pub fn float64<'slot>(
        &mut self,
        value: f64,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        self.node(out.reborrow())?;
        let session = self.session;
        self.value_builder(out.reborrow())?
            .float64(session, value, out.reborrow())
    }

    pub fn boolean<'slot>(
        &mut self,
        value: bool,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        self.node(out.reborrow())?;
        let session = self.session;
        self.value_builder(out.reborrow())?
            .boolean(session, value, out.reborrow())
    }

    pub fn none<'slot>(&mut self, mut out: Out<'slot, '_, CodecErrors>) -> OResult<'slot, ValueId> {
        self.node(out.reborrow())?;
        let session = self.session;
        self.value_builder(out.reborrow())?
            .none(session, out.reborrow())
    }

    pub fn optional<'slot>(
        &mut self,
        value: Option<ValueId>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        self.node(out.reborrow())?;
        let session = self.session;
        self.value_builder(out.reborrow())?
            .optional(session, value, out.reborrow())
    }

    pub fn array<'slot>(
        &mut self,
        values: &[ValueId],
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        self.node(out.reborrow())?;
        let session = self.session;
        self.value_builder(out.reborrow())?
            .array(session, values, out.reborrow())
    }

    pub fn map<'slot>(
        &mut self,
        values: &[NamedValue<'_>],
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        self.node(out.reborrow())?;
        let session = self.session;
        self.value_builder(out.reborrow())?
            .map(session, values, out.reborrow())
    }

    pub fn current_type<'slot>(
        &self,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, TypeView<'call>> {
        Ok(self.binding_cursor(out.reborrow())?.ty)
    }

    pub(crate) fn read_value_node<'slot, 'value>(
        &self,
        value: ValueView<'value>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueNodeKind<'call, 'value>> {
        let ty = self.current_type(out.reborrow())?;
        let kind = self.session.schema_codec(
            CodecStage::Conversion,
            |schema_out| ty.kind(schema_out),
            out.reborrow(),
        )?;
        match ValueNode::resolve(kind, value) {
            Some(node) => Ok(node),
            None => {
                let mismatch = match (kind, value.as_ref()) {
                    (
                        TypeKind::Array {
                            length: Some(_), ..
                        },
                        ValueKind::Array(_),
                    ) => ShapeMismatch {
                        kind: ConversionErrorKind::ArrayLengthMismatch,
                        expected: "array with the schema length",
                        actual: "array with a mismatched length",
                    },
                    (TypeKind::Struct { .. }, ValueKind::Structure(_)) => ShapeMismatch {
                        kind: ConversionErrorKind::FieldCountMismatch,
                        expected: "struct containing all schema fields",
                        actual: "struct with a mismatched field count",
                    },
                    (expected, actual) => ShapeMismatch {
                        kind: ConversionErrorKind::ValueShapeMismatch,
                        expected: BindingConversion::type_shape(expected),
                        actual: value_shape(actual),
                    },
                };
                self.fail_as(
                    ConversionDiagnosticInput::new(
                        mismatch.kind,
                        mismatch.expected,
                        mismatch.actual,
                        self.path,
                    ),
                    out,
                )
            }
        }
    }

    pub fn check<'slot, T: Typol>(
        &self,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ()> {
        let cursor = self.binding_cursor(out.reborrow())?;
        cursor
            .plan
            .get(TypeId::of::<T>(), cursor.ty, self.session, out.reborrow())?;
        Ok(())
    }

    pub(crate) fn bind<'slot, T: Typol>(
        &mut self,
        binding: &'call TypedSchema<'call, T>,
        state: &'call mut ConversionState,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ()> {
        self.binding = BindingState::Type(BindingCursor {
            plan: binding.plan(),
            ty: binding.schema().entry(),
            identity: TypeId::of::<T>(),
        });
        self.state = Some(state);
        self.check::<T>(out.reborrow())
    }

    pub(crate) fn set_type<'slot>(
        &mut self,
        ty: TypeView<'call>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ()> {
        let mut cursor = self.binding_cursor(out.reborrow())?;
        cursor.ty = ty;
        self.binding = BindingState::Type(cursor);
        Ok(())
    }

    pub(crate) fn binding_cursor<'slot>(
        &self,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, BindingCursor<'call>> {
        match self.binding {
            BindingState::Type(cursor) | BindingState::Variant { cursor, .. } => Ok(cursor),
            BindingState::Unbound => self.fail_as(
                ConversionDiagnosticInput::new(
                    ConversionErrorKind::InternalBindingInvariant,
                    "initialized binding context",
                    "binding context is not initialized",
                    self.path,
                ),
                out.reborrow(),
            ),
        }
    }

    pub(crate) fn replace_binding(&mut self, binding: BindingState<'call>) -> BindingState<'call> {
        mem::replace(&mut self.binding, binding)
    }

    pub(crate) fn current_path(&self) -> ConversionPath<'call> {
        self.path
    }

    pub(crate) fn replace_path(&mut self, path: ConversionPath<'call>) -> ConversionPath<'call> {
        mem::replace(&mut self.path, path)
    }

    fn typed_child<'child, 'slot, T: Typol, R>(
        &'child mut self,
        ty: TypeView<'child>,
        convert: impl for<'out_borrow> FnOnce(
            &mut ConversionContext<'child>,
            Out<'slot, 'out_borrow, CodecErrors>,
        ) -> OResult<'slot, R>,
        out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, R> {
        self.typed_child_with_path::<T, _>(ty, None, convert, out)
    }

    pub(crate) fn typed_child_with_path<'child, 'slot, T: Typol, R>(
        &'child mut self,
        ty: TypeView<'child>,
        segment: Option<ConversionPathSegment<'child>>,
        convert: impl for<'out_borrow> FnOnce(
            &mut ConversionContext<'child>,
            Out<'slot, 'out_borrow, CodecErrors>,
        ) -> OResult<'slot, R>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, R> {
        self.child_with_path(
            segment,
            |child, mut child_out| {
                let mut cursor = child.binding_cursor(child_out.reborrow())?;
                cursor.ty = ty;
                cursor.identity = TypeId::of::<T>();
                child.binding = BindingState::Type(cursor);
                child.check::<T>(child_out.reborrow())?;
                convert(child, child_out)
            },
            out.reborrow(),
        )
    }

    pub fn encode_indexed_child<'child, 'slot, T: Typol>(
        &'child mut self,
        index: usize,
        ty: TypeView<'child>,
        value: &T,
        out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        self.typed_child_with_path::<T, _>(
            ty,
            Some(ConversionPathSegment::Index(index)),
            |child, child_out| child.encode_here(value, child_out),
            out,
        )
    }

    pub fn decode_indexed_child<'child, 'slot, 'value, T: Typol>(
        &'child mut self,
        index: usize,
        ty: TypeView<'child>,
        value: ConversionValue<'value>,
        out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, T> {
        self.typed_child_with_path::<T, _>(
            ty,
            Some(ConversionPathSegment::Index(index)),
            |child, child_out| child.decode_here(value, child_out),
            out,
        )
    }

    pub fn encode_child<'child, 'slot, T: Typol>(
        &'child mut self,
        ty: TypeView<'child>,
        value: &T,
        out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        self.typed_child::<T, _>(
            ty,
            |child, child_out| child.encode_here(value, child_out),
            out,
        )
    }

    pub fn decode_child<'child, 'slot, 'value, T: Typol>(
        &'child mut self,
        ty: TypeView<'child>,
        value: ConversionValue<'value>,
        out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, T> {
        self.typed_child::<T, _>(
            ty,
            |child, child_out| child.decode_here(value, child_out),
            out,
        )
    }

    pub fn element_type<'slot>(
        &self,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, TypeView<'call>> {
        let ty = self.current_type(out.reborrow())?;
        let kind = self.session.schema_codec(
            CodecStage::Conversion,
            |schema_out| ty.kind(schema_out),
            out.reborrow(),
        )?;
        match kind {
            TypeKind::Option { inner } => Ok(inner),
            TypeKind::Array { element, .. } => Ok(element),
            TypeKind::Map { value } => Ok(value),
            actual => self.fail_as(
                ConversionDiagnosticInput::new(
                    ConversionErrorKind::ValueShapeMismatch,
                    "optional value, array, or map",
                    BindingConversion::type_shape(actual),
                    self.path,
                ),
                out,
            ),
        }
    }

    pub(crate) fn fields<'slot>(
        &self,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, &FieldBindings<'call>> {
        let resolved = self.resolve_binding(out.reborrow())?;
        match resolved.variant {
            Some(variant) => Ok(&variant.fields),
            None => Ok(&resolved.binding.fields),
        }
    }

    pub fn structure<'slot>(
        &mut self,
        field_value: impl for<'context, 'out_borrow> FnMut(
            usize,
            &mut ConversionContext<'context>,
            Out<'slot, 'out_borrow, CodecErrors>,
        ) -> OResult<'slot, ValueId>,
        out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        BindingConversion::structure(self, field_value, out)
    }

    pub fn read_field<'slot, T: Typol>(
        &mut self,
        fields: ConversionFields<'_>,
        rust_slot: usize,
        out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, T> {
        BindingConversion::read_field(self, fields, rust_slot, out)
    }

    pub fn read_structure<'slot, 'value>(
        &mut self,
        value: ConversionValue<'value>,
        out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ConversionFields<'value>> {
        BindingConversion::read_structure(self, value.view(), out)
    }

    pub fn variant<'child, 'slot, R>(
        &'child mut self,
        rust_slot: usize,
        convert: impl for<'out_borrow> FnOnce(
            &mut ConversionContext<'call>,
            VariantPayloadInfo<'call>,
            Out<'slot, 'out_borrow, CodecErrors>,
        ) -> OResult<'slot, R>,
        out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, R> {
        BindingConversion::variant(self, rust_slot, convert, out)
    }

    pub fn unit_variant<'slot>(
        &mut self,
        rust_slot: usize,
        out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        BindingConversion::unit_variant(self, rust_slot, out)
    }

    pub fn single_variant<'slot, T: Typol>(
        &mut self,
        rust_slot: usize,
        value: &T,
        out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        BindingConversion::single_variant(self, rust_slot, value, out)
    }

    pub fn structure_variant<'slot>(
        &mut self,
        rust_slot: usize,
        field_value: impl for<'context, 'out_borrow> FnMut(
            usize,
            &mut ConversionContext<'context>,
            Out<'slot, 'out_borrow, CodecErrors>,
        ) -> OResult<'slot, ValueId>,
        out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        BindingConversion::structure_variant(self, rust_slot, field_value, out)
    }

    pub fn read_enumeration<'slot, 'value>(
        &mut self,
        value: ConversionValue<'value>,
        out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, EnumInput<'value>> {
        BindingConversion::read_enumeration(self, value.view(), out)
    }

    pub fn read_single<'slot, T: Typol>(
        &mut self,
        rust_slot: usize,
        value: ConversionValue<'_>,
        out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, T> {
        BindingConversion::read_single(self, rust_slot, value, out)
    }

    pub(crate) fn session(&self) -> &SessionBudget {
        self.session
    }

    pub(crate) fn resolve_binding<'slot>(
        &self,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ResolvedBinding<'_, 'call>> {
        let cursor = self.binding_cursor(out.reborrow())?;
        let variant_slot = match self.binding {
            BindingState::Unbound => {
                return self.fail_as(
                    ConversionDiagnosticInput::new(
                        ConversionErrorKind::InternalBindingInvariant,
                        "initialized binding context",
                        "binding context is not initialized",
                        self.path,
                    ),
                    out,
                );
            }
            BindingState::Type(_) => None,
            BindingState::Variant { slot, .. } => Some(slot),
        };
        let binding = cursor
            .plan
            .get(cursor.identity, cursor.ty, self.session, out.reborrow())?;
        let variant = match variant_slot {
            Some(slot) => match binding.variants.get(slot) {
                Some(variant) => Some(variant),
                None => {
                    return self.fail_as(
                        ConversionDiagnosticInput::new(
                            ConversionErrorKind::InternalBindingInvariant,
                            "valid enum variant binding slot",
                            "enum variant binding slot is out of bounds",
                            self.path,
                        ),
                        out,
                    );
                }
            },
            None => None,
        };
        Ok(ResolvedBinding { binding, variant })
    }

    pub(crate) fn add_current_enumeration<'slot>(
        &mut self,
        payload: ValuePayload<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        self.node(out.reborrow())?;
        let resolved = self.resolve_binding(out.reborrow())?;
        let Some(variant) = resolved.variant else {
            return self.fail_as(
                ConversionDiagnosticInput::new(
                    ConversionErrorKind::InternalBindingInvariant,
                    "current enum variant binding",
                    "enum variant binding is missing",
                    self.path,
                ),
                out,
            );
        };
        let name = self.session.schema_codec(
            CodecStage::Conversion,
            |schema_out| variant.variant.name(schema_out),
            out.reborrow(),
        )?;
        let session = self.session;
        self.value_builder(out.reborrow())?
            .enumeration(session, name, payload, out.reborrow())
    }

    pub(crate) fn add_structure<'slot>(
        &mut self,
        fields: &[NamedValue<'_>],
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        self.node(out.reborrow())?;
        let session = self.session;
        self.value_builder(out.reborrow())?
            .structure(session, fields, out.reborrow())
    }

    fn value_builder<'context, 'slot>(
        &'context mut self,
        out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, &'context mut ValueBuilder> {
        if self.builder.is_none() {
            return self.fail_as(
                ConversionDiagnosticInput::new(
                    ConversionErrorKind::InternalBindingInvariant,
                    "shared value builder for the encoding phase",
                    "value builder is not initialized",
                    self.path,
                ),
                out,
            );
        }
        match self.builder.as_deref_mut() {
            Some(builder) => Ok(builder),
            None => Err(out.fail(Conversion::internal_error())),
        }
    }
}

impl fmt::Debug for ConversionContext<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConversionContext")
            .field("depth", &self.depth.level())
            .field("nodes", &self.nodes.nodes())
            .finish()
    }
}

fn value_shape(kind: ValueKind<'_>) -> &'static str {
    match kind {
        ValueKind::Signed(_) => "signed integer",
        ValueKind::Unsigned(_) => "unsigned integer",
        ValueKind::Float32(_) => "f32 value",
        ValueKind::Float64(_) => "f64 value",
        ValueKind::String(_) => "string",
        ValueKind::Bool(_) => "boolean value",
        ValueKind::Optional(_) => "optional value",
        ValueKind::Array(_) => "array",
        ValueKind::Map(_) => "map",
        ValueKind::Structure(_) => "struct",
        ValueKind::Enumeration { .. } => "enum",
    }
}
