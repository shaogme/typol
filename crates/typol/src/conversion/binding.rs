use crate::{
    conversion::{
        BindingState, ConversionContext, ConversionFields, ConversionPathSegment,
        ConversionPayload, ConversionValue, EnumInput,
    },
    descriptor::Typol,
    diagnostics::{
        CodecErrors, CodecStage, ConversionDiagnosticInput, ConversionErrorKind, Failure,
    },
    value_walk::{NAMED_ENTRY_NAME_MISSING, ValueNodeKind},
};
use opeo::{OResult, Out};
use typol_core::{
    ChargedVec, NamedValue, ValueEntries, ValueId, ValuePayload, ValuePayloadView, ValueView,
};
use typol_model::BuiltinType;
use typol_semantic::{TypeKind, TypeView, VariantPayloadInfo};

pub(super) struct BindingConversion;

impl BindingConversion {
    pub(super) fn structure<'slot>(
        context: &mut ConversionContext<'_>,
        mut field_value: impl for<'context, 'out_borrow> FnMut(
            usize,
            &mut ConversionContext<'context>,
            Out<'slot, 'out_borrow, CodecErrors>,
        ) -> OResult<'slot, ValueId>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        let ty = context.current_type(out.reborrow())?;
        let kind = context.session().schema_codec(
            CodecStage::Conversion,
            |schema_out| ty.kind(schema_out),
            out.reborrow(),
        )?;
        if !matches!(kind, TypeKind::Struct { .. }) {
            return context.fail_as(
                ConversionDiagnosticInput::new(
                    ConversionErrorKind::ValueShapeMismatch,
                    "struct",
                    Self::type_shape(kind),
                    context.current_path(),
                ),
                out.reborrow(),
            );
        }
        let fields = Self::build_fields(context, &mut field_value, out.reborrow())?;
        context.add_structure(fields.as_slice(), out.reborrow())
    }

    fn build_fields<'call, 'slot>(
        context: &mut ConversionContext<'call>,
        field_value: &mut impl for<'context, 'out_borrow> FnMut(
            usize,
            &mut ConversionContext<'context>,
            Out<'slot, 'out_borrow, CodecErrors>,
        ) -> OResult<'slot, ValueId>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ChargedVec<NamedValue<'call>>> {
        let count = context.fields(out.reborrow())?.as_slice().len();
        let mut fields = context.session().budget_codec(
            CodecStage::Conversion,
            |budget_out| context.session().workspace(budget_out),
            out.reborrow(),
        )?;
        context.session().budget_codec(
            CodecStage::Conversion,
            |budget_out| fields.try_reserve_exact(count, budget_out),
            out.reborrow(),
        )?;
        for slot in 0..count {
            let bindings = context.fields(out.reborrow())?;
            let Some(binding) = bindings.as_slice().get(slot) else {
                return context.fail_as(
                    ConversionDiagnosticInput::new(
                        ConversionErrorKind::InternalBindingInvariant,
                        "a matching struct field binding exists",
                        "field binding slot is missing",
                        context.current_path(),
                    ),
                    out.reborrow(),
                );
            };
            let field = binding.field;
            let rust_slot = binding.rust_slot;
            let ty: TypeView<'call> = context.session().schema_codec(
                CodecStage::Conversion,
                |schema_out| field.ty(schema_out),
                out.reborrow(),
            )?;
            let name: &'call str = context.session().schema_codec(
                CodecStage::Conversion,
                |schema_out| field.name(schema_out),
                out.reborrow(),
            )?;
            let value = context.child_with_path(
                Some(ConversionPathSegment::Field(name)),
                |child, mut child_out| {
                    child.set_type(ty, child_out.reborrow())?;
                    field_value(rust_slot, child, child_out)
                },
                out.reborrow(),
            )?;
            let field_value = NamedValue::new(name, value);
            context.session().budget_codec(
                CodecStage::Conversion,
                |budget_out| fields.try_push(field_value, budget_out),
                out.reborrow(),
            )?;
        }
        Ok(fields)
    }

    pub(super) fn read_field<'call, 'slot, T: Typol>(
        context: &mut ConversionContext<'call>,
        fields: ConversionFields<'_>,
        rust_slot: usize,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, T> {
        let bindings = context.fields(out.reborrow())?;
        let field = bindings.by_rust_slot(rust_slot, context.session(), out.reborrow())?;
        let fields = fields.entries();
        if fields.len() != bindings.as_slice().len() {
            return context.fail_as(
                ConversionDiagnosticInput::new(
                    ConversionErrorKind::FieldCountMismatch,
                    "struct containing all schema fields",
                    "struct with a mismatched field count",
                    context.current_path(),
                ),
                out,
            );
        }
        let field_slot = context.session().schema_codec(
            CodecStage::Conversion,
            |schema_out| field.slot(schema_out),
            out.reborrow(),
        )?;
        let field_name: &'call str = context.session().schema_codec(
            CodecStage::Conversion,
            |schema_out| field.name(schema_out),
            out.reborrow(),
        )?;
        let field_ty = context.session().schema_codec(
            CodecStage::Conversion,
            |schema_out| field.ty(schema_out),
            out.reborrow(),
        )?;
        if let Some(value) = fields.get(field_slot) {
            let Some(value) = value.as_named() else {
                return Err(out.fail(Self::named_entry_name_error()));
            };
            let ordering = context.session().budget_codec(
                CodecStage::Conversion,
                |budget_out| {
                    context
                        .session()
                        .compare_text(value.name(), field_name, budget_out)
                },
                out.reborrow(),
            )?;
            if ordering.is_eq() {
                return context.typed_child_with_path::<T, _>(
                    field_ty,
                    Some(ConversionPathSegment::Field(field_name)),
                    |child, child_out| {
                        child.decode_here(ConversionValue::new(value.value()), child_out)
                    },
                    out.reborrow(),
                );
            }
        }
        let mut selected = None;
        for value in fields {
            let Some(value) = value.as_named() else {
                return Err(out.fail(Self::named_entry_name_error()));
            };
            let ordering = context.session().budget_codec(
                CodecStage::Conversion,
                |budget_out| {
                    context
                        .session()
                        .compare_text(value.name(), field_name, budget_out)
                },
                out.reborrow(),
            )?;
            if ordering.is_eq() {
                selected = Some(value.value());
                break;
            }
        }
        let Some(value) = selected else {
            return context.fail_as(
                ConversionDiagnosticInput::new(
                    ConversionErrorKind::MissingField,
                    "Schema field",
                    "field does not exist in ",
                    context
                        .current_path()
                        .pushed(ConversionPathSegment::Field(field_name)),
                ),
                out,
            );
        };
        context.typed_child_with_path::<T, _>(
            field_ty,
            Some(ConversionPathSegment::Field(field_name)),
            |child, child_out| child.decode_here(ConversionValue::new(value), child_out),
            out,
        )
    }

    pub(super) fn read_structure<'slot, 'value>(
        context: &mut ConversionContext<'_>,
        value: ValueView<'value>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ConversionFields<'value>> {
        let field_count = context.fields(out.reborrow())?.as_slice().len();
        let ValueNodeKind::Structure { values, fields } =
            context.read_value_node(value, out.reborrow())?
        else {
            return context.reject_with_path(out.reborrow());
        };
        if fields.len() == field_count {
            Self::require_named_entries(context, values, out.reborrow())?;
            Ok(ConversionFields::new(values))
        } else {
            context.fail_as(
                ConversionDiagnosticInput::new(
                    ConversionErrorKind::FieldCountMismatch,
                    "struct containing all schema fields",
                    "struct with a mismatched field count",
                    context.current_path(),
                ),
                out,
            )
        }
    }

    pub(super) fn variant<'call, 'child, 'slot, R>(
        context: &'child mut ConversionContext<'call>,
        rust_slot: usize,
        convert: impl for<'out_borrow> FnOnce(
            &mut ConversionContext<'call>,
            VariantPayloadInfo<'call>,
            Out<'slot, 'out_borrow, CodecErrors>,
        ) -> OResult<'slot, R>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, R> {
        let cursor = context.binding_cursor(out.reborrow())?;
        let binding = cursor.plan.get(
            cursor.identity,
            cursor.ty,
            context.session(),
            out.reborrow(),
        )?;
        for (slot, variant) in binding.variants.iter().enumerate() {
            context.session().budget_codec(
                CodecStage::Conversion,
                |budget_out| context.session().visit(budget_out),
                out.reborrow(),
            )?;
            if variant.rust_slot == rust_slot {
                let payload = context.session().schema_codec(
                    CodecStage::Conversion,
                    |schema_out| variant.variant.payload(schema_out),
                    out.reborrow(),
                )?;
                let variant_name: &'call str = context.session().schema_codec(
                    CodecStage::Conversion,
                    |schema_out| variant.variant.name(schema_out),
                    out.reborrow(),
                )?;
                let previous_binding =
                    context.replace_binding(BindingState::Variant { cursor, slot });
                let previous_path = context.replace_path(
                    context
                        .current_path()
                        .pushed(ConversionPathSegment::Variant(variant_name)),
                );
                let result = convert(context, payload, out.reborrow());
                context.replace_binding(previous_binding);
                context.replace_path(previous_path);
                return result;
            }
        }
        context.fail_as(
            ConversionDiagnosticInput::new(
                ConversionErrorKind::MissingVariant,
                "bound Rust enum variant",
                "variant slot is unmatched",
                context.current_path(),
            ),
            out,
        )
    }

    pub(super) fn unit_variant<'slot>(
        context: &mut ConversionContext<'_>,
        rust_slot: usize,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        Self::variant(
            context,
            rust_slot,
            |context, expected, mut variant_out| {
                if !matches!(expected, VariantPayloadInfo::Unit) {
                    return context.fail_as(
                        ConversionDiagnosticInput::new(
                            ConversionErrorKind::EnumPayloadMismatch,
                            Self::payload_shape(expected),
                            "Rust unit variant",
                            context.current_path(),
                        ),
                        variant_out.reborrow(),
                    );
                }
                context.add_current_enumeration(ValuePayload::Unit, variant_out.reborrow())
            },
            out.reborrow(),
        )
    }

    pub(super) fn single_variant<'slot, T: Typol>(
        context: &mut ConversionContext<'_>,
        rust_slot: usize,
        value: &T,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        Self::variant(
            context,
            rust_slot,
            |context, payload, mut variant_out| {
                let VariantPayloadInfo::Single { ty } = payload else {
                    return context.fail_as(
                        ConversionDiagnosticInput::new(
                            ConversionErrorKind::EnumPayloadMismatch,
                            Self::payload_shape(payload),
                            "Rust single-value variant",
                            context.current_path(),
                        ),
                        variant_out.reborrow(),
                    );
                };
                let value = context.encode_child::<T>(ty, value, variant_out.reborrow())?;
                context.add_current_enumeration(ValuePayload::Single(value), variant_out.reborrow())
            },
            out.reborrow(),
        )
    }

    pub(super) fn structure_variant<'slot>(
        context: &mut ConversionContext<'_>,
        rust_slot: usize,
        mut field_value: impl for<'context, 'out_borrow> FnMut(
            usize,
            &mut ConversionContext<'context>,
            Out<'slot, 'out_borrow, CodecErrors>,
        ) -> OResult<'slot, ValueId>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        Self::variant(
            context,
            rust_slot,
            |context, payload, mut variant_out| {
                if !matches!(payload, VariantPayloadInfo::Structure { .. }) {
                    return context.fail_as(
                        ConversionDiagnosticInput::new(
                            ConversionErrorKind::EnumPayloadMismatch,
                            Self::payload_shape(payload),
                            "Rust struct variant",
                            context.current_path(),
                        ),
                        variant_out.reborrow(),
                    );
                }
                let fields = Self::build_fields(context, &mut field_value, variant_out.reborrow())?;
                context.add_current_enumeration(
                    ValuePayload::Structure(fields.as_slice()),
                    variant_out.reborrow(),
                )
            },
            out.reborrow(),
        )
    }

    pub(super) fn read_enumeration<'slot, 'value>(
        context: &mut ConversionContext<'_>,
        value: ValueView<'value>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, EnumInput<'value>> {
        let ValueNodeKind::Enumeration(value) = context.read_value_node(value, out.reborrow())?
        else {
            return context.reject_with_path(out.reborrow());
        };
        let cursor = context.binding_cursor(out.reborrow())?;
        let binding = cursor.plan.get(
            cursor.identity,
            cursor.ty,
            context.session(),
            out.reborrow(),
        )?;
        for variant in &binding.variants {
            let variant_name = context.session().schema_codec(
                CodecStage::Conversion,
                |schema_out| variant.variant.name(schema_out),
                out.reborrow(),
            )?;
            let ordering = context.session().budget_codec(
                CodecStage::Conversion,
                |budget_out| {
                    context
                        .session()
                        .compare_text(value.variant(), variant_name, budget_out)
                },
                out.reborrow(),
            )?;
            if ordering.is_eq() {
                let expected = context.session().schema_codec(
                    CodecStage::Conversion,
                    |schema_out| variant.variant.payload(schema_out),
                    out.reborrow(),
                )?;
                if value.resolve_payload(expected).is_none() {
                    return context.fail_as(
                        ConversionDiagnosticInput::new(
                            ConversionErrorKind::EnumPayloadMismatch,
                            Self::payload_shape(expected),
                            Self::value_payload_shape(value.payload()),
                            context
                                .current_path()
                                .pushed(ConversionPathSegment::Variant(variant_name)),
                        ),
                        out,
                    );
                }
                if let ValuePayloadView::Structure(values) = value.payload() {
                    Self::require_named_entries(context, values, out.reborrow())?;
                }
                return Ok(EnumInput {
                    rust_slot: variant.rust_slot,
                    payload: ConversionPayload::from_view(value.payload()),
                });
            }
        }
        context.fail_as(
            ConversionDiagnosticInput::new(
                ConversionErrorKind::MissingVariant,
                "bound enum variant",
                "unmatched enum variant",
                context.current_path(),
            ),
            out,
        )
    }

    pub(super) fn read_single<'slot, T: Typol>(
        context: &mut ConversionContext<'_>,
        rust_slot: usize,
        value: ConversionValue<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, T> {
        Self::variant(
            context,
            rust_slot,
            |context, payload, mut variant_out| {
                let VariantPayloadInfo::Single { ty } = payload else {
                    return context.fail_as(
                        ConversionDiagnosticInput::new(
                            ConversionErrorKind::EnumPayloadMismatch,
                            Self::payload_shape(payload),
                            "Rust single-value variant",
                            context.current_path(),
                        ),
                        variant_out.reborrow(),
                    );
                };
                context.decode_child::<T>(ty, value, variant_out.reborrow())
            },
            out.reborrow(),
        )
    }

    pub(super) fn type_shape(kind: TypeKind<'_>) -> &'static str {
        match kind {
            TypeKind::Builtin(BuiltinType::I8) => "i8",
            TypeKind::Builtin(BuiltinType::I16) => "i16",
            TypeKind::Builtin(BuiltinType::I32) => "i32",
            TypeKind::Builtin(BuiltinType::I64) => "i64",
            TypeKind::Builtin(BuiltinType::I128) => "i128",
            TypeKind::Builtin(BuiltinType::U8) => "u8",
            TypeKind::Builtin(BuiltinType::U16) => "u16",
            TypeKind::Builtin(BuiltinType::U32) => "u32",
            TypeKind::Builtin(BuiltinType::U64) => "u64",
            TypeKind::Builtin(BuiltinType::U128) => "u128",
            TypeKind::Builtin(BuiltinType::F32) => "f32",
            TypeKind::Builtin(BuiltinType::F64) => "f64",
            TypeKind::Builtin(BuiltinType::String) => "string",
            TypeKind::Builtin(BuiltinType::Bool) => "bool",
            TypeKind::Option { .. } => "optional value",
            TypeKind::Array { .. } => "array",
            TypeKind::Map { .. } => "map",
            TypeKind::Struct { .. } => "struct",
            TypeKind::Enum { .. } => "enum",
        }
    }

    fn payload_shape(payload: VariantPayloadInfo<'_>) -> &'static str {
        match payload {
            VariantPayloadInfo::Unit => "unit payload",
            VariantPayloadInfo::Single { .. } => "single-value payload",
            VariantPayloadInfo::Structure { .. } => "struct payload",
        }
    }

    fn value_payload_shape(payload: ValuePayloadView<'_>) -> &'static str {
        match payload {
            ValuePayloadView::Unit => "unit payload",
            ValuePayloadView::Single(_) => "single-value payload",
            ValuePayloadView::Structure(_) => "struct payload",
        }
    }

    fn require_named_entries<'slot>(
        context: &ConversionContext<'_>,
        values: ValueEntries<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ()> {
        for value in values {
            context.session().budget_codec(
                CodecStage::Conversion,
                |budget_out| context.session().visit(budget_out),
                out.reborrow(),
            )?;
            if value.as_named().is_none() {
                return Err(out.fail(Self::named_entry_name_error()));
            }
        }
        Ok(())
    }

    fn named_entry_name_error() -> CodecErrors {
        CodecErrors::emergency(
            CodecStage::Conversion,
            Failure::internal(NAMED_ENTRY_NAME_MISSING),
        )
    }
}
