use crate::storage::{
    Annotation, AnnotationData, ArrayDefault, Assertion, BooleanLiteral, BraceEntry, BraceKey,
    BracedDefault, Declaration, DefaultValue, DefaultValueKind, EnumDecl, EnumDefault,
    EnumDefaultPayload, EnumDefaultPayloadKind, Expr, ExternModDecl, FieldDecl, FieldParams, Id,
    MethodArgument, MethodArgumentKind, ModuleDecl, Name, NumberLiteral, NumberLiteralParams,
    SingleVariantPayload, Storage, StringLiteral, StructDecl, StructDeclParams,
    StructureVariantPayload, TypeAliasDecl, TypeAliasDeclParams, TypeExpr, VariantDecl,
    VariantDeclParams, VariantPayload, VariantPayloadKind,
};
use opeo::{OResult, Out};
use typol_core::{ChargedString, ChargedVec, Diagnostic, SourceSpan};

pub(crate) struct StorageConstructor<'storage> {
    storage: &'storage mut Storage,
}

impl<'storage> StorageConstructor<'storage> {
    pub(crate) fn new(storage: &'storage mut Storage) -> Self {
        Self { storage }
    }

    pub(crate) fn annotation_new<'slot>(
        &mut self,
        data: AnnotationData,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<Annotation>> {
        self.storage
            .insert(Annotation { data, span }, out.reborrow())
    }

    pub(crate) fn array_default_new<'slot>(
        &mut self,
        elements: ChargedVec<Id<DefaultValue>>,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<ArrayDefault>> {
        let elements = self.storage.store_list(elements, span, out.reborrow())?;
        self.storage
            .insert(ArrayDefault { elements, span }, out.reborrow())
    }

    pub(crate) fn assertion_new<'slot>(
        &mut self,
        expression: Id<Expr>,
        message: Id<StringLiteral>,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<Assertion>> {
        self.storage.insert(
            Assertion {
                expression,
                message,
                span,
            },
            out.reborrow(),
        )
    }

    pub(crate) fn boolean_literal_new<'slot>(
        &mut self,
        value: bool,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<BooleanLiteral>> {
        self.storage
            .insert(BooleanLiteral { value, span }, out.reborrow())
    }

    pub(crate) fn brace_entry_new<'slot>(
        &mut self,
        key: BraceKey,
        value: Id<DefaultValue>,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<BraceEntry>> {
        self.storage
            .insert(BraceEntry { key, value, span }, out.reborrow())
    }

    pub(crate) fn braced_default_new<'slot>(
        &mut self,
        entries: ChargedVec<Id<BraceEntry>>,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<BracedDefault>> {
        let entries = self.storage.store_list(entries, span, out.reborrow())?;
        self.storage
            .insert(BracedDefault { entries, span }, out.reborrow())
    }

    pub(crate) fn default_value_new<'slot>(
        &mut self,
        kind: DefaultValueKind,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<DefaultValue>> {
        self.storage
            .insert(DefaultValue { kind, span }, out.reborrow())
    }

    pub(crate) fn enum_default_new<'slot>(
        &mut self,
        variant: Id<Name>,
        payload: Id<EnumDefaultPayload>,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<EnumDefault>> {
        self.storage.insert(
            EnumDefault {
                variant,
                payload,
                span,
            },
            out.reborrow(),
        )
    }

    pub(crate) fn enum_default_payload_unit<'slot>(
        &mut self,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<EnumDefaultPayload>> {
        self.storage.insert(
            EnumDefaultPayload {
                kind: EnumDefaultPayloadKind::Unit,
                span,
            },
            out.reborrow(),
        )
    }

    pub(crate) fn enum_default_payload_single<'slot>(
        &mut self,
        value: Id<DefaultValue>,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<EnumDefaultPayload>> {
        self.storage.insert(
            EnumDefaultPayload {
                kind: EnumDefaultPayloadKind::Single(value),
                span,
            },
            out.reborrow(),
        )
    }

    pub(crate) fn enum_default_payload_structure<'slot>(
        &mut self,
        fields: ChargedVec<Id<BraceEntry>>,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<EnumDefaultPayload>> {
        let fields = self.braced_default_new(fields, span, out.reborrow())?;
        self.storage.insert(
            EnumDefaultPayload {
                kind: EnumDefaultPayloadKind::Structure(fields),
                span,
            },
            out.reborrow(),
        )
    }

    pub(crate) fn method_argument_expression<'slot>(
        &mut self,
        expression: Id<Expr>,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<MethodArgument>> {
        self.storage.insert(
            MethodArgument {
                kind: MethodArgumentKind::Expression(expression),
                span,
            },
            out.reborrow(),
        )
    }

    pub(crate) fn method_argument_closure<'slot>(
        &mut self,
        parameter: Id<Name>,
        body: Id<Expr>,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<MethodArgument>> {
        self.storage.insert(
            MethodArgument {
                kind: MethodArgumentKind::Closure { parameter, body },
                span,
            },
            out.reborrow(),
        )
    }

    pub(crate) fn name_try_new<'slot>(
        &mut self,
        text: &str,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<Name>> {
        let value = Name {
            text: self.storage.pool(text, span, out.reborrow())?,
            span,
        };
        self.storage.insert(value, out.reborrow())
    }

    pub(crate) fn number_literal_try_new<'slot>(
        &mut self,
        params: NumberLiteralParams<'_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<NumberLiteral>> {
        let NumberLiteralParams {
            raw,
            kind,
            negative,
            span,
        } = params;
        let raw = self.storage.pool(raw, span, out.reborrow())?;
        self.storage.insert(
            NumberLiteral {
                raw,
                kind,
                negative,
                span,
            },
            out.reborrow(),
        )
    }

    pub(crate) fn string_literal_from_owned<'slot>(
        &mut self,
        value: ChargedString,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<StringLiteral>> {
        let value = StringLiteral {
            value: self.storage.move_text(value, span, out.reborrow())?,
            span,
        };
        self.storage.insert(value, out.reborrow())
    }

    pub(crate) fn variant_payload_unit<'slot>(
        &mut self,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<VariantPayload>> {
        self.storage.insert(
            VariantPayload {
                kind: VariantPayloadKind::Unit,
                span,
            },
            out.reborrow(),
        )
    }

    pub(crate) fn variant_payload_single<'slot>(
        &mut self,
        ty: Id<TypeExpr>,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<VariantPayload>> {
        self.storage.insert(
            VariantPayload {
                kind: VariantPayloadKind::Single(SingleVariantPayload { ty }),
                span,
            },
            out.reborrow(),
        )
    }

    pub(crate) fn variant_payload_structure<'slot>(
        &mut self,
        fields: ChargedVec<Id<FieldDecl>>,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<VariantPayload>> {
        let fields = self.storage.store_list(fields, span, out.reborrow())?;
        self.storage.insert(
            VariantPayload {
                kind: VariantPayloadKind::Structure(StructureVariantPayload { fields }),
                span,
            },
            out.reborrow(),
        )
    }

    pub(crate) fn enum_decl_new<'slot>(
        &mut self,
        name: Id<Name>,
        variants: ChargedVec<Id<VariantDecl>>,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<EnumDecl>> {
        let variants = self.storage.store_list(variants, span, out.reborrow())?;
        let value = EnumDecl {
            name,
            variants,
            span,
        };
        self.storage.insert(value, out.reborrow())
    }

    pub(crate) fn extern_mod_decl_new<'slot>(
        &mut self,
        name: Id<Name>,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<ExternModDecl>> {
        self.storage
            .insert(ExternModDecl { name, span }, out.reborrow())
    }

    pub(crate) fn field_decl_new<'slot>(
        &mut self,
        params: FieldParams,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<FieldDecl>> {
        let FieldParams {
            name,
            ty,
            default,
            span,
        } = params;
        self.storage.insert(
            FieldDecl {
                name,
                ty,
                default,
                span,
            },
            out.reborrow(),
        )
    }

    pub(crate) fn module_decl_new<'slot>(
        &mut self,
        name: Id<Name>,
        declarations: ChargedVec<Declaration>,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<ModuleDecl>> {
        let declarations = self
            .storage
            .store_list(declarations, span, out.reborrow())?;
        let value = ModuleDecl {
            name,
            declarations,
            span,
        };
        self.storage.insert(value, out.reborrow())
    }

    pub(crate) fn struct_decl_new<'slot>(
        &mut self,
        params: StructDeclParams,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<StructDecl>> {
        let StructDeclParams {
            name,
            fields,
            assertions,
            span,
        } = params;
        let fields = self.storage.store_list(fields, span, out.reborrow())?;
        let assertions = self.storage.store_list(assertions, span, out.reborrow())?;
        let value = StructDecl {
            name,
            fields,
            assertions,
            span,
        };
        self.storage.insert(value, out.reborrow())
    }

    pub(crate) fn type_alias_decl_new<'slot>(
        &mut self,
        params: TypeAliasDeclParams,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<TypeAliasDecl>> {
        let TypeAliasDeclParams { name, target, span } = params;
        let value = TypeAliasDecl { name, target, span };
        self.storage.insert(value, out.reborrow())
    }

    pub(crate) fn variant_decl_new<'slot>(
        &mut self,
        params: VariantDeclParams,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<VariantDecl>> {
        let VariantDeclParams {
            name,
            payload,
            assertions,
            span,
        } = params;
        let assertions = self.storage.store_list(assertions, span, out.reborrow())?;
        let value = VariantDecl {
            name,
            payload,
            assertions,
            span,
        };
        self.storage.insert(value, out.reborrow())
    }
}
