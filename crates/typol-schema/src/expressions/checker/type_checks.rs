use crate::{
    context::CompileContext,
    diagnostics::{BuildFailure, DiagnosticCollector},
    expressions::{ExpressionType, type_analysis::TypeAnalyzer},
    symbols::SymbolTable,
    types::ResolvedTypeGraph,
};
use opeo::{OResult, Out};
use typol_core::SourceSpan;
use typol_syntax::{AstAccessError, BuiltinType};

pub(super) struct TypeChecks<'types, 'source, 'diagnostics> {
    types: &'types ResolvedTypeGraph,
    symbols: &'types SymbolTable<'source>,
    diagnostics: &'diagnostics mut DiagnosticCollector,
    fallback_span: SourceSpan,
}

impl<'types, 'source, 'diagnostics> TypeChecks<'types, 'source, 'diagnostics> {
    pub(super) fn ast_access<'slot, 'borrow, T>(
        context: &CompileContext,
        out: Out<'slot, 'borrow, BuildFailure>,
        operation: impl for<'call> FnOnce(Out<'call, 'call, AstAccessError>) -> OResult<'call, T>,
    ) -> OResult<'slot, T> {
        context.ast_access(out, operation, BuildFailure::from)
    }

    pub(super) fn new(
        types: &'types ResolvedTypeGraph,
        symbols: &'types SymbolTable<'source>,
        diagnostics: &'diagnostics mut DiagnosticCollector,
        fallback_span: SourceSpan,
    ) -> Self {
        Self {
            types,
            symbols,
            diagnostics,
            fallback_span,
        }
    }

    pub(super) fn builtin_type<'slot>(
        &mut self,
        ty: ExpressionType,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, Option<BuiltinType>> {
        TypeAnalyzer::new(
            self.types,
            self.symbols,
            self.diagnostics,
            self.fallback_span,
        )
        .builtin_type(ty, out.reborrow())
    }

    pub(super) fn is_builtin<'slot>(
        &mut self,
        ty: ExpressionType,
        expected: BuiltinType,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, bool> {
        TypeAnalyzer::new(
            self.types,
            self.symbols,
            self.diagnostics,
            self.fallback_span,
        )
        .is_builtin(ty, expected, out.reborrow())
    }

    pub(super) fn is_option<'slot>(
        &mut self,
        ty: ExpressionType,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, bool> {
        TypeAnalyzer::new(
            self.types,
            self.symbols,
            self.diagnostics,
            self.fallback_span,
        )
        .is_option(ty, out.reborrow())
    }

    pub(super) fn same_type<'slot>(
        &mut self,
        left: ExpressionType,
        right: ExpressionType,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, bool> {
        TypeAnalyzer::new(
            self.types,
            self.symbols,
            self.diagnostics,
            self.fallback_span,
        )
        .same_type(left, right, out.reborrow())
    }

    pub(super) fn is_equality_comparable<'slot>(
        &mut self,
        ty: ExpressionType,
        inside_enum_payload: bool,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, bool> {
        TypeAnalyzer::new(
            self.types,
            self.symbols,
            self.diagnostics,
            self.fallback_span,
        )
        .is_equality_comparable(ty, inside_enum_payload, out.reborrow())
    }
}
