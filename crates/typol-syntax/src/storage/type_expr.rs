use crate::{
    error::SyntaxDiagnostic,
    storage::{ArrayLength, Id, Name, Storage, TypeExpr, TypeExprKind, TypePath},
};
use opeo::{OResult, Out};
use typol_core::{ChargedVec, Diagnostic, DiagnosticCode, SourceSpan};
use typol_model::BuiltinType;

pub(crate) struct TypeBuilder;

impl TypeBuilder {
    pub(crate) fn type_expr_builtin<'slot>(
        storage: &mut Storage,
        kind: BuiltinType,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<TypeExpr>> {
        let value = TypeExpr {
            kind: TypeExprKind::Builtin(kind),
            span,
        };
        storage.insert(value, out.reborrow())
    }

    pub(crate) fn type_expr_option<'slot>(
        storage: &mut Storage,
        inner: Id<TypeExpr>,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<TypeExpr>> {
        let value = TypeExpr {
            kind: TypeExprKind::Option(inner),
            span,
        };
        storage.insert(value, out.reborrow())
    }

    pub(crate) fn type_expr_array<'slot>(
        storage: &mut Storage,
        element: Id<TypeExpr>,
        length: Option<Id<ArrayLength>>,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<TypeExpr>> {
        let value = TypeExpr {
            kind: TypeExprKind::Array { element, length },
            span,
        };
        storage.insert(value, out.reborrow())
    }

    pub(crate) fn type_expr_map<'slot>(
        storage: &mut Storage,
        value: Id<TypeExpr>,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<TypeExpr>> {
        let value = TypeExpr {
            kind: TypeExprKind::Map { value },
            span,
        };
        storage.insert(value, out.reborrow())
    }

    pub(crate) fn type_expr_path<'slot>(
        storage: &mut Storage,
        segments: ChargedVec<Id<Name>>,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<TypeExpr>> {
        let value = TypeExpr {
            kind: TypeExprKind::Path(Self::type_path_new(
                storage,
                segments,
                span,
                out.reborrow(),
            )?),
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
