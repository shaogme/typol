use crate::{
    error::SyntaxDiagnostic,
    storage::{Annotation, ArrayLength, Id, Name, Storage, TypeExpr, TypeExprKind, TypePath},
};
use opeo::{OResult, Out};
use typol_core::{ChargedVec, Diagnostic, DiagnosticCode, SourceSpan};
use typol_model::BuiltinType;

pub(crate) struct TypeBuilder;

pub(crate) struct ArrayTypeExprParts {
    pub(crate) element: Id<TypeExpr>,
    pub(crate) length: Option<Id<ArrayLength>>,
}

impl TypeBuilder {
    pub(crate) fn type_expr_builtin<'slot>(
        storage: &mut Storage,
        kind: BuiltinType,
        annotations: ChargedVec<Id<Annotation>>,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<TypeExpr>> {
        let annotations = storage.store_list(annotations, span, out.reborrow())?;
        let value = TypeExpr {
            kind: TypeExprKind::Builtin(kind),
            annotations,
            span,
        };
        storage.insert(value, out.reborrow())
    }

    pub(crate) fn type_expr_option<'slot>(
        storage: &mut Storage,
        inner: Id<TypeExpr>,
        annotations: ChargedVec<Id<Annotation>>,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<TypeExpr>> {
        let annotations = storage.store_list(annotations, span, out.reborrow())?;
        let value = TypeExpr {
            kind: TypeExprKind::Option(inner),
            annotations,
            span,
        };
        storage.insert(value, out.reborrow())
    }

    pub(crate) fn type_expr_array<'slot>(
        storage: &mut Storage,
        parts: ArrayTypeExprParts,
        annotations: ChargedVec<Id<Annotation>>,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<TypeExpr>> {
        let annotations = storage.store_list(annotations, span, out.reborrow())?;
        let value = TypeExpr {
            kind: TypeExprKind::Array {
                element: parts.element,
                length: parts.length,
            },
            annotations,
            span,
        };
        storage.insert(value, out.reborrow())
    }

    pub(crate) fn type_expr_map<'slot>(
        storage: &mut Storage,
        value: Id<TypeExpr>,
        annotations: ChargedVec<Id<Annotation>>,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<TypeExpr>> {
        let annotations = storage.store_list(annotations, span, out.reborrow())?;
        let value = TypeExpr {
            kind: TypeExprKind::Map { value },
            annotations,
            span,
        };
        storage.insert(value, out.reborrow())
    }

    pub(crate) fn type_expr_path<'slot>(
        storage: &mut Storage,
        segments: ChargedVec<Id<Name>>,
        annotations: ChargedVec<Id<Annotation>>,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<TypeExpr>> {
        let annotations = storage.store_list(annotations, span, out.reborrow())?;
        let path = Self::type_path_new(storage, segments, span, out.reborrow())?;
        let value = TypeExpr {
            kind: TypeExprKind::Path(path),
            annotations,
            span,
        };
        storage.insert(value, out.reborrow())
    }

    pub(crate) fn type_path_new<'slot>(
        storage: &mut Storage,
        segments: ChargedVec<Id<Name>>,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<TypePath>> {
        if segments.is_empty() {
            return Err(out.reborrow().fail(SyntaxDiagnostic::emergency(
                DiagnosticCode::SCHEMA_SYNTAX_ERROR,
                span,
            )));
        }
        let segments = storage.store_list(segments, span, out.reborrow())?;

        let value = TypePath { segments, span };
        storage.insert(value, out.reborrow())
    }
}
