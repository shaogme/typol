use crate::ast::AstAccessError;
use core::{
    fmt,
    slice::{Iter, from_ref},
};
use opeo::{ErrSlot, OResult, Out};
use typol_core::{
    BudgetError, ChargedString, ChargedVec, Diagnostic, DiagnosticCode, DiagnosticContext,
    DiagnosticMessage, ExecutionBudget, FilledTerminalSlot, SourceId, SourceSpan, Span,
    TerminalSlot, diagnostic_context,
};

pub(crate) static AST_STORAGE_RESOLVE_NODE_SPAN: DiagnosticContext =
    diagnostic_context!("typol-syntax", "ast-storage", "resolve-node-span");
pub(crate) static AST_STORAGE_DECODED_TEXT_BUDGET: DiagnosticContext = diagnostic_context!(
    "typol-syntax",
    "ast-storage",
    "decoded-text-budget-mismatch"
);
pub(crate) static AST_STORAGE_UPDATE_EXPR_SPAN: DiagnosticContext =
    diagnostic_context!("typol-syntax", "ast-storage", "update-expression-span");
static AST_STORAGE_NODE_ACCESS: DiagnosticContext =
    diagnostic_context!("typol-syntax", "ast-storage", "invalid-node-id");
static AST_STORAGE_LIST_ACCESS: DiagnosticContext =
    diagnostic_context!("typol-syntax", "ast-storage", "invalid-list-range");
static AST_STORAGE_TEXT_ACCESS: DiagnosticContext =
    diagnostic_context!("typol-syntax", "ast-storage", "invalid-text-range");
static AST_STORAGE_DECODED_TEXT_ACCESS: DiagnosticContext =
    diagnostic_context!("typol-syntax", "ast-storage", "invalid-decoded-text-id");
pub(crate) static AST_STORAGE_ARRAY_LENGTH_NUMBER: DiagnosticContext =
    diagnostic_context!("typol-syntax", "ast-storage", "resolve-array-length-number");

enum ErrorStorage {
    Inline(Diagnostic),
    Collected {
        diagnostics: ChargedVec<Diagnostic>,
        terminal: Option<FilledTerminalSlot<Diagnostic>>,
    },
}

#[derive(Clone)]
/// An ordered borrowing iterator over syntax diagnostics.
pub struct SyntaxDiagnosticIter<'diagnostics> {
    diagnostics: Iter<'diagnostics, Diagnostic>,
    terminal: Option<&'diagnostics Diagnostic>,
    remaining: usize,
}

impl<'diagnostics> Iterator for SyntaxDiagnosticIter<'diagnostics> {
    type Item = &'diagnostics Diagnostic;

    fn next(&mut self) -> Option<Self::Item> {
        let next = self.diagnostics.next().or_else(|| self.terminal.take());
        if next.is_some() {
            self.remaining -= 1;
        }
        next
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl SyntaxDiagnosticIter<'_> {
    /// Returns the first diagnostic.
    pub fn first(&self) -> Option<&Diagnostic> {
        self.clone().next()
    }

    /// Returns the diagnostic at the given index.
    pub fn get(&self, index: usize) -> Option<&Diagnostic> {
        self.clone().nth(index)
    }

    /// Returns whether the iterator has no remaining diagnostics.
    pub fn is_empty(&self) -> bool {
        self.remaining == 0
    }

    /// Creates an independent iterator starting at the current position.
    pub fn iter(&self) -> Self {
        self.clone()
    }
}

impl DoubleEndedIterator for SyntaxDiagnosticIter<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        let next = self
            .terminal
            .take()
            .or_else(|| self.diagnostics.next_back());
        if next.is_some() {
            self.remaining -= 1;
        }
        next
    }
}

impl ExactSizeIterator for SyntaxDiagnosticIter<'_> {}

/// An owned, non-empty collection of syntax errors; emergency returns need no heap allocation.
pub struct SyntaxErrors {
    storage: ErrorStorage,
}

impl SyntaxErrors {
    pub(crate) fn inline(diagnostic: Diagnostic) -> Self {
        Self {
            storage: ErrorStorage::Inline(diagnostic),
        }
    }

    /// Borrows and iterates diagnostics in source order.
    pub fn diagnostics(&self) -> SyntaxDiagnosticIter<'_> {
        let (diagnostics, terminal) = match &self.storage {
            ErrorStorage::Inline(diagnostic) => (from_ref(diagnostic), None),
            ErrorStorage::Collected {
                diagnostics,
                terminal,
            } => (
                diagnostics.as_slice(),
                terminal.as_ref().and_then(FilledTerminalSlot::get),
            ),
        };
        SyntaxDiagnosticIter {
            diagnostics: diagnostics.iter(),
            terminal,
            remaining: diagnostics.len() + usize::from(terminal.is_some()),
        }
    }

    /// Visits existing diagnostics in source order without allocating a temporary collection.
    pub fn for_each(&self, mut consume: impl FnMut(&Diagnostic)) {
        match &self.storage {
            ErrorStorage::Inline(diagnostic) => consume(diagnostic),
            ErrorStorage::Collected {
                diagnostics,
                terminal,
            } => {
                for diagnostic in diagnostics {
                    consume(diagnostic);
                }
                if let Some(diagnostic) = terminal.as_ref().and_then(FilledTerminalSlot::get) {
                    consume(diagnostic);
                }
            }
        }
    }
}

impl fmt::Debug for SyntaxErrors {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SyntaxErrors")
            .field("diagnostic_count", &self.diagnostics().len())
            .finish()
    }
}

pub(crate) struct DiagnosticSink {
    diagnostics: ChargedVec<Diagnostic>,
    terminal: Option<TerminalSlot<Diagnostic>>,
    filled_terminal: Option<FilledTerminalSlot<Diagnostic>>,
    limit: usize,
    stopped: bool,
}

impl DiagnosticSink {
    pub(crate) fn new<'slot>(
        limit: usize,
        budget: &ExecutionBudget,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Self> {
        let limit = limit.max(1);
        let mut diagnostics = SyntaxDiagnostic::budgeted(span, out.reborrow(), |out| {
            ChargedVec::new(Some(budget), out)
        })?;
        let terminal =
            SyntaxDiagnostic::budgeted(span, out.reborrow(), |out| TerminalSlot::new(budget, out))?;
        SyntaxDiagnostic::budgeted(span, out.reborrow(), |out| {
            diagnostics.try_reserve_exact(1, out)
        })?;
        Ok(Self {
            diagnostics,
            terminal: Some(terminal),
            filled_terminal: None,
            limit,
            stopped: false,
        })
    }

    pub(crate) fn push<'slot>(
        &mut self,
        diagnostic: Diagnostic,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ()> {
        if self.stopped {
            return Ok(());
        }
        let fatal = matches!(
            diagnostic.code(),
            DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
                | DiagnosticCode::ALLOCATION_FAILED
                | DiagnosticCode::INTERNAL_ERROR
                | DiagnosticCode::SOURCE_INPUT_ERROR
        );
        if fatal || diagnostic.code() == DiagnosticCode::DIAGNOSTIC_LIMIT_REACHED {
            self.store_terminal(diagnostic, out.reborrow())?;
            self.stopped = true;
            return Ok(());
        } else if self.diagnostics.len() >= self.limit - 1 {
            let terminal = SyntaxDiagnostic::emergency(
                DiagnosticCode::DIAGNOSTIC_LIMIT_REACHED,
                diagnostic.primary(),
            );
            self.store_terminal(terminal, out.reborrow())?;
            self.stopped = true;
            return Ok(());
        } else {
            let span = diagnostic.primary();
            SyntaxDiagnostic::budgeted(span, out.reborrow(), |out| {
                self.diagnostics.try_push(diagnostic, out)
            })?;
        }
        Ok(())
    }

    pub(crate) fn stopped(&self) -> bool {
        self.stopped
    }

    fn store_terminal<'slot>(
        &mut self,
        diagnostic: Diagnostic,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ()> {
        let Some(slot) = self.terminal.take() else {
            return Err(out.reborrow().fail(SyntaxDiagnostic::internal(
                diagnostic_context!("typol-syntax", "diagnostics", "terminal-slot-missing"),
                diagnostic.primary(),
            )));
        };
        let Some(terminal) = slot.store(diagnostic) else {
            return Err(out.reborrow().fail(SyntaxDiagnostic::internal(
                diagnostic_context!("typol-syntax", "diagnostics", "terminal-slot-full"),
                None,
            )));
        };
        self.filled_terminal = Some(terminal);
        Ok(())
    }

    pub(crate) fn finish(self) -> Option<SyntaxErrors> {
        if self.diagnostics.is_empty() && self.filled_terminal.is_none() {
            None
        } else {
            Some(SyntaxErrors {
                storage: ErrorStorage::Collected {
                    diagnostics: self.diagnostics,
                    terminal: self.filled_terminal,
                },
            })
        }
    }
}

pub(crate) struct SyntaxDiagnostic;

impl SyntaxDiagnostic {
    pub(crate) fn emergency(
        code: DiagnosticCode,
        span: impl Into<Option<SourceSpan>>,
    ) -> Diagnostic {
        let mut diagnostic = Diagnostic::emergency(code);
        diagnostic.set_primary(span.into());
        diagnostic
    }

    pub(crate) fn internal(
        context: DiagnosticContext,
        span: impl Into<Option<SourceSpan>>,
    ) -> Diagnostic {
        let mut diagnostic = Self::emergency(DiagnosticCode::INTERNAL_ERROR, span);
        diagnostic.set_context(Some(context));
        diagnostic
    }

    pub(crate) fn ast_access(error: AstAccessError) -> Diagnostic {
        let context = match error {
            AstAccessError::InvalidNodeId { id } => AST_STORAGE_NODE_ACCESS.with_object_index(id),
            AstAccessError::InvalidListRange { start, end } => {
                AST_STORAGE_LIST_ACCESS.with_object_range(start, end)
            }
            AstAccessError::InvalidTextRange { start, end } => {
                AST_STORAGE_TEXT_ACCESS.with_object_range(start, end)
            }
            AstAccessError::InvalidDecodedTextId { id } => {
                AST_STORAGE_DECODED_TEXT_ACCESS.with_object_index(id)
            }
        };
        Self::internal(context, None)
    }

    pub(crate) fn internal_object(
        context: DiagnosticContext,
        span: impl Into<Option<SourceSpan>>,
        object_id: usize,
    ) -> Diagnostic {
        let mut diagnostic = Self::internal(context, span);
        if let Ok(object_id) = u64::try_from(object_id) {
            let context = diagnostic
                .context()
                .map(|context| context.with_object_id(object_id));
            diagnostic.set_context(context);
        }
        diagnostic
    }

    pub(crate) fn internal_object_at_end(
        context: DiagnosticContext,
        source_id: SourceId,
        source_len: usize,
        object_id: usize,
    ) -> Diagnostic {
        Self::internal_object(
            context,
            SourceSpan::new(source_id, Span::point(source_len)),
            object_id,
        )
    }

    pub(crate) fn budgeted<'slot, T>(
        span: impl Into<Option<SourceSpan>>,
        mut out: Out<'slot, '_, Diagnostic>,
        operation: impl for<'budget> FnOnce(Out<'budget, 'budget, BudgetError>) -> OResult<'budget, T>,
    ) -> OResult<'slot, T> {
        Self::budgeted_with(out.reborrow(), operation, |error| Self::budget(error, span))
    }

    pub(crate) fn budgeted_with<'slot, T>(
        mut out: Out<'slot, '_, Diagnostic>,
        operation: impl for<'budget> FnOnce(Out<'budget, 'budget, BudgetError>) -> OResult<'budget, T>,
        convert: impl FnOnce(BudgetError) -> Diagnostic,
    ) -> OResult<'slot, T> {
        let mut errors = ErrSlot::<BudgetError>::new();
        match errors.try_call(operation) {
            Ok(value) => Ok(value),
            Err(error) => Err(out.reborrow().fail(convert(error.take()))),
        }
    }

    pub(crate) fn new_vec<'slot, T>(
        budget: &ExecutionBudget,
        span: SourceSpan,
        out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ChargedVec<T>> {
        Self::budgeted(span, out, |out| ChargedVec::new(Some(budget), out))
    }

    pub(crate) fn try_push<'slot, T>(
        values: &mut ChargedVec<T>,
        value: T,
        span: SourceSpan,
        out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ()> {
        Self::budgeted(span, out, |out| values.try_push(value, out))
    }

    pub(crate) fn try_reserve<'slot, T>(
        values: &mut ChargedVec<T>,
        additional: usize,
        span: SourceSpan,
        out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ()> {
        Self::budgeted(span, out, |out| values.try_reserve(additional, out))
    }

    pub(crate) fn try_reserve_exact<'slot, T>(
        values: &mut ChargedVec<T>,
        additional: usize,
        span: SourceSpan,
        out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ()> {
        Self::budgeted(span, out, |out| values.try_reserve_exact(additional, out))
    }

    pub(crate) fn new_string<'slot>(
        budget: &ExecutionBudget,
        span: SourceSpan,
        out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ChargedString> {
        Self::budgeted(span, out, |out| {
            ChargedString::with_capacity(0, Some(budget), out)
        })
    }

    pub(crate) fn push_str<'slot>(
        text: &mut ChargedString,
        value: &str,
        span: SourceSpan,
        out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ()> {
        Self::budgeted(span, out, |out| text.try_push_str(value, out))
    }

    pub(crate) fn try_add_label<'slot>(
        diagnostic: &mut Diagnostic,
        span: SourceSpan,
        text: &str,
        budget: &ExecutionBudget,
        out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ()> {
        Self::budgeted(span, out, |out| {
            diagnostic.try_add_label(span, text, Some(budget), out)
        })
    }

    pub(crate) fn make<'slot, 'message>(
        code: DiagnosticCode,
        message: impl Into<DiagnosticMessage<'message>>,
        span: SourceSpan,
        budget: &ExecutionBudget,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Diagnostic> {
        let mut diagnostic = Self::budgeted(span, out.reborrow(), |out| {
            Diagnostic::try_new(code, message, Some(budget), out)
        })?;
        diagnostic.set_primary(Some(span));
        Ok(diagnostic)
    }

    pub(crate) fn budget(error: BudgetError, span: impl Into<Option<SourceSpan>>) -> Diagnostic {
        let mut diagnostic = Self::emergency(error.diagnostic_code(), span);
        diagnostic.set_context(error.context());
        diagnostic
    }
}

#[cfg(test)]
mod tests {
    use typol_core::{
        BudgetError, DiagnosticContext, SourceId, SourceSpan, Span, diagnostic_context,
    };

    use super::{DiagnosticCode, SyntaxDiagnostic};

    static PARSER_BUDGET_CONTEXT: DiagnosticContext =
        diagnostic_context!("typol-syntax", "parser-budget", "test-invariant");

    #[test]
    fn parser_budget_diagnostic_keeps_failure_context_and_source_span() {
        let span = SourceSpan::new(SourceId::new(8), Span::point(3));
        let diagnostic =
            SyntaxDiagnostic::budget(BudgetError::invariant(&PARSER_BUDGET_CONTEXT), span);

        assert_eq!(diagnostic.code(), DiagnosticCode::INTERNAL_ERROR);
        assert_eq!(diagnostic.primary(), Some(span));
        assert_eq!(diagnostic.context(), Some(PARSER_BUDGET_CONTEXT));
    }
}
