use crate::{
    context::CompileContext,
    diagnostics::{BuildFailure, DiagnosticCollector},
    symbols::{SymbolTable, TypeSymbolId},
    types::PathResolution,
    types::model::TypeSlot,
};
use opeo::{OResult, Out};
use typol_core::{ChargedVec, DiagnosticMessage, DiagnosticMessageKey, SourceSpan};
use typol_i18n::diagnostic_key;
use typol_syntax::TypeExpr;

#[derive(Clone, Copy)]
pub(super) struct AliasDependency {
    pub(super) target: TypeSymbolId,
    pub(super) reference_span: SourceSpan,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum AliasState {
    NotAlias,
    Unvisited,
    Visiting,
    Resolved,
    Invalid,
}

pub(super) struct AliasFrame {
    pub(super) symbol: TypeSymbolId,
    pub(super) next_dependency: usize,
}

pub(super) struct GraphSlots {
    pub(super) aliases: ChargedVec<TypeSlot>,
    pub(super) fields: ChargedVec<TypeSlot>,
    pub(super) variant_payloads: ChargedVec<TypeSlot>,
}

pub(super) struct TypeGraphSupport;

impl TypeGraphSupport {
    pub(super) fn empty_graph_slots<'slot>(
        symbols: &SymbolTable<'_>,
        context: &CompileContext,
        diagnostics: &mut DiagnosticCollector,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, GraphSlots> {
        let aliases =
            Self::empty_slots(symbols.types().len(), context, diagnostics, out.reborrow())?;
        let fields =
            Self::empty_slots(symbols.fields().len(), context, diagnostics, out.reborrow())?;
        let variant_payloads = Self::empty_slots(
            symbols.variants().len(),
            context,
            diagnostics,
            out.reborrow(),
        )?;
        Ok(GraphSlots {
            aliases,
            fields,
            variant_payloads,
        })
    }

    pub(super) fn empty_slots<'slot>(
        count: usize,
        context: &CompileContext,
        diagnostics: &mut DiagnosticCollector,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ChargedVec<TypeSlot>> {
        let mut slots = diagnostics.new_vec(context.execution(), out.reborrow())?;
        slots.try_reserve_exact(count, out.reborrow())?;
        for _ in 0..count {
            slots.try_push(TypeSlot::pending(), out.reborrow())?;
        }
        Ok(slots)
    }

    pub(super) fn push_expr<'slot, 'a>(
        expressions: &mut ChargedVec<TypeExpr<'a>>,
        expression: TypeExpr<'a>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ()> {
        expressions.try_reserve(1, out.reborrow())?;
        expressions.try_push(expression, out.reborrow())?;
        Ok(())
    }

    pub(super) fn report(
        problem: PathResolution,
        span: SourceSpan,
        diagnostics: &mut DiagnosticCollector,
    ) {
        let (key, message): (DiagnosticMessageKey, &str) = match problem {
            PathResolution::NotFound => (
                diagnostic_key!("diagnostic.schema.unknown_type_path"),
                "unknown type path",
            ),
            PathResolution::WrongKind => (
                diagnostic_key!("diagnostic.schema.type_path_wrong_kind"),
                "type path must end with a type name",
            ),
            PathResolution::Ambiguous | PathResolution::Invalid | PathResolution::Resolved(_) => {
                return;
            }
        };
        diagnostics
            .push(diagnostics.make_semantic(DiagnosticMessage::localized(message, key), span));
    }
}
