#[cfg(test)]
mod tests;

use alloc::borrow::Cow;
use core::{cell::RefCell, cmp::Ordering, iter::Chain, ops::Deref, option::IntoIter, slice};
use opeo::{ErrSlot, OResult, Out, ResultOutExt};

use typol_core::{
    BudgetError, ChargedVec, Diagnostic, DiagnosticCode, DiagnosticContext, DiagnosticLabelInput,
    DiagnosticMessage, ExecutionBudget, RelatedLocationRole, SourceSpan, diagnostic_context,
};
use typol_i18n::{Translator, t};
use typol_semantic::SchemaAccessError;
use typol_syntax::AstAccessError;
use u18n::RenderError;

#[cfg(test)]
use alloc::vec::Vec;

macro_rules! schema_context_ref {
    ($stage:expr, $failure_point:literal) => {{
        use typol_core::{DiagnosticContext, DiagnosticContextDescriptor};

        static DESCRIPTOR: DiagnosticContextDescriptor = DiagnosticContextDescriptor::new(
            "typol-schema",
            $stage,
            concat!($failure_point, "@", line!()),
        );
        static CONTEXT: DiagnosticContext = DiagnosticContext::new(&DESCRIPTOR);
        &CONTEXT
    }};
}

macro_rules! schema_context {
    ($stage:expr, $failure_point:literal) => {{ *schema_context_ref!($stage, $failure_point) }};
}

macro_rules! schema_internal {
    ($translator:expr, $span:expr $(,)?) => {{
        $crate::diagnostics::EmergencyDiagnostics::internal(
            $span,
            schema_context!(module_path!(), "internal-invariant"),
            $translator,
        )
    }};
}

macro_rules! schema_invariant {
    ($stage:expr, $failure_point:literal) => {{
        use typol_core::BudgetError;

        BudgetError::invariant(schema_context_ref!($stage, $failure_point))
    }};
}

#[derive(Debug)]
pub(crate) struct BuildFailure {
    context: Option<DiagnosticContext>,
    budget: Option<BudgetError>,
}

impl BuildFailure {
    pub(crate) const fn empty() -> Self {
        Self {
            context: None,
            budget: None,
        }
    }

    pub(crate) const fn with_context(context: DiagnosticContext) -> Self {
        Self {
            context: Some(context),
            budget: None,
        }
    }

    pub(crate) const fn context(&self) -> Option<DiagnosticContext> {
        self.context
    }
}

impl From<AstAccessError> for BuildFailure {
    fn from(error: AstAccessError) -> Self {
        Self::with_context(EmergencyDiagnostics::ast_access_context(error))
    }
}

impl From<BudgetError> for BuildFailure {
    fn from(error: BudgetError) -> Self {
        Self {
            context: None,
            budget: Some(error),
        }
    }
}

static AST_NODE_ACCESS: DiagnosticContext =
    diagnostic_context!("typol-schema", "ast-access", "invalid-node-id");
static AST_LIST_ACCESS: DiagnosticContext =
    diagnostic_context!("typol-schema", "ast-access", "invalid-list-range");
static AST_TEXT_ACCESS: DiagnosticContext =
    diagnostic_context!("typol-schema", "ast-access", "invalid-text-range");
static AST_DECODED_TEXT_ACCESS: DiagnosticContext =
    diagnostic_context!("typol-schema", "ast-access", "invalid-decoded-text-id");
static SCHEMA_ACCESS_TYPE: DiagnosticContext =
    diagnostic_context!("typol-schema", "schema-access", "type-table-index");
static SCHEMA_ACCESS_FIELD: DiagnosticContext =
    diagnostic_context!("typol-schema", "schema-access", "field-table-index");
static SCHEMA_ACCESS_VARIANT: DiagnosticContext =
    diagnostic_context!("typol-schema", "schema-access", "variant-table-index");
static SCHEMA_ACCESS_PROGRAM: DiagnosticContext =
    diagnostic_context!("typol-schema", "schema-access", "program-table-index");
static SCHEMA_ACCESS_CONSTRAINT: DiagnosticContext =
    diagnostic_context!("typol-schema", "schema-access", "constraint-table-index");
static SCHEMA_ACCESS_REFINEMENT: DiagnosticContext =
    diagnostic_context!("typol-schema", "schema-access", "refinement-table-index");
static SCHEMA_ACCESS_SHAPE: DiagnosticContext =
    diagnostic_context!("typol-schema", "schema-access", "shape-table-index");
static SCHEMA_ACCESS_EXPRESSION: DiagnosticContext =
    diagnostic_context!("typol-schema", "schema-access", "expression-table-index");
static SCHEMA_ACCESS_OTHER: DiagnosticContext =
    diagnostic_context!("typol-schema", "schema-access", "table-index");

pub(crate) type BuildResult<T> = Result<T, BuildFailure>;

#[derive(Clone, Copy)]
enum InsertOperation {
    Read,
    Shift,
}

enum InsertFailure {
    Budget(BudgetError),
    Internal {
        index: usize,
        operation: InsertOperation,
    },
}

pub(crate) struct ErrorDiagnostics {
    diagnostics: ChargedVec<Diagnostic>,
    terminal: Option<Diagnostic>,
    translator: Translator,
}

/// A borrowed list view over ordinary diagnostics and an optional terminal diagnostic.
#[derive(Clone, Copy)]
pub struct DiagnosticList<'a> {
    diagnostics: &'a [Diagnostic],
    terminal: Option<&'a Diagnostic>,
}

impl<'a> IntoIterator for DiagnosticList<'a> {
    type Item = &'a Diagnostic;
    type IntoIter = Chain<slice::Iter<'a, Diagnostic>, IntoIter<&'a Diagnostic>>;

    fn into_iter(self) -> Self::IntoIter {
        self.diagnostics.iter().chain(self.terminal)
    }
}

impl<'a> DiagnosticList<'a> {
    pub(crate) const fn new(
        diagnostics: &'a [Diagnostic],
        terminal: Option<&'a Diagnostic>,
    ) -> Self {
        Self {
            diagnostics,
            terminal,
        }
    }

    pub fn first(&self) -> Option<&'a Diagnostic> {
        self.diagnostics.first().or(self.terminal)
    }

    pub fn last(&self) -> Option<&'a Diagnostic> {
        self.terminal.or_else(|| self.diagnostics.last())
    }

    pub fn get(&self, index: usize) -> Option<&'a Diagnostic> {
        if index < self.diagnostics.len() {
            self.diagnostics.get(index)
        } else if index == self.diagnostics.len() {
            self.terminal
        } else {
            None
        }
    }

    pub fn len(&self) -> usize {
        self.diagnostics.len() + usize::from(self.terminal.is_some())
    }

    pub fn is_empty(&self) -> bool {
        self.diagnostics.is_empty() && self.terminal.is_none()
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &'a Diagnostic> + Clone {
        self.diagnostics.iter().chain(self.terminal)
    }

    pub(crate) const fn regular(&self) -> &'a [Diagnostic] {
        self.diagnostics
    }

    pub(crate) const fn terminal(&self) -> Option<&'a Diagnostic> {
        self.terminal
    }
}

pub(crate) struct DiagnosticCollector {
    diagnostics: ChargedVec<Diagnostic>,
    limit: usize,
    state: Option<Diagnostic>,
    fallback_span: SourceSpan,
    translator: Translator,
    operation_errors: RefCell<ErrSlot<InsertFailure>>,
}

pub(crate) struct DuplicateLabel<'message> {
    pub(crate) span: SourceSpan,
    pub(crate) role: RelatedLocationRole,
    pub(crate) message: DiagnosticMessage<'message>,
}

impl DiagnosticCollector {
    pub(crate) fn new<'slot, E>(
        limit: usize,
        fallback_span: SourceSpan,
        execution: &ExecutionBudget,
        translator: Translator,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, Self>
    where
        E: From<BudgetError>,
    {
        let mut diagnostics = ChargedVec::new(Some(execution), out.reborrow())?;
        // The two fixed tail slots are reserved for location-finalization failures in the first terminal diagnostic and in ordinary errors, respectively.
        diagnostics.try_reserve_exact(2, out.reborrow())?;
        Ok(Self {
            diagnostics,
            limit: limit.max(1),
            state: None,
            fallback_span,
            translator,
            operation_errors: RefCell::new(ErrSlot::new()),
        })
    }

    pub(crate) const fn translator(&self) -> Translator {
        self.translator
    }

    pub(crate) fn new_vec<'slot, T>(
        &mut self,
        execution: &ExecutionBudget,
        out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ChargedVec<T>> {
        let result = {
            let mut operation_errors = self.operation_errors.borrow_mut();
            match operation_errors.try_call(|out| ChargedVec::new(Some(execution), out)) {
                Ok(values) => Ok(values),
                Err(caught) => Err(caught.take()),
            }
        };
        match result {
            Ok(values) => Ok(values),
            Err(failure) => {
                match failure {
                    InsertFailure::Budget(error) => {
                        self.abort(BudgetDiagnostics::make(
                            error,
                            self.fallback_span,
                            self.translator,
                        ));
                    }
                    failure @ InsertFailure::Internal { .. } => {
                        self.abort(
                            failure.internal_diagnostic(self.fallback_span, self.translator),
                        );
                    }
                }
                Err(out.fail(BuildFailure::empty()))
            }
        }
    }

    pub(crate) fn abort(&mut self, diagnostic: Diagnostic) {
        if self.state.is_none() {
            self.state = Some(diagnostic);
        }
    }

    pub(crate) fn require<'slot, T>(
        &mut self,
        value: Option<T>,
        span: SourceSpan,
        out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, T> {
        match value {
            Some(value) => Ok(value),
            None => {
                self.abort(schema_internal!(self.translator, span));
                Err(out.fail(BuildFailure::empty()))
            }
        }
    }

    pub(crate) fn report_failure(&mut self, failure: BuildFailure) {
        if let Some(error) = failure.budget {
            self.abort(BudgetDiagnostics::make(
                error,
                self.fallback_span,
                self.translator,
            ));
        } else if let Some(context) = failure.context() {
            self.abort(EmergencyDiagnostics::internal_without_span(
                context,
                self.translator,
            ));
        }
    }

    pub(crate) fn make_semantic<'message>(
        &self,
        message: impl Into<DiagnosticMessage<'message>>,
        span: SourceSpan,
    ) -> Diagnostic {
        self.make(DiagnosticCode::SCHEMA_SEMANTIC_ERROR, message, Some(span))
    }

    pub(crate) fn make<'message>(
        &self,
        code: DiagnosticCode,
        message: impl Into<DiagnosticMessage<'message>>,
        span: Option<SourceSpan>,
    ) -> Diagnostic {
        if let Some(terminal) = &self.state {
            let diagnostic = match terminal.message() {
                Ok(message) => {
                    let mut errors = ErrSlot::<BudgetError>::new();
                    match errors.try_call(|out| {
                        Diagnostic::try_new(
                            terminal.code(),
                            DiagnosticMessage::custom(message),
                            self.diagnostics.budget(),
                            out,
                        )
                    }) {
                        Ok(diagnostic) => diagnostic,
                        Err(caught) => {
                            BudgetDiagnostics::make_optional(caught.take(), span, self.translator)
                        }
                    }
                }
                Err(error) => Diagnostic::failure(terminal.code(), error),
            };
            let mut diagnostic = diagnostic;
            diagnostic.set_primary(span);
            diagnostic.set_context(terminal.context());
            return diagnostic;
        }
        let budget = self.diagnostics.budget();
        let message = message.into();
        let mut errors = ErrSlot::<BudgetError>::new();
        match errors.try_call(|mut out| {
            let mut diagnostic = Diagnostic::try_new(code, message, budget, out.reborrow())?;
            diagnostic.set_primary(span);
            Ok(diagnostic)
        }) {
            Ok(diagnostic) => diagnostic,
            Err(caught) => BudgetDiagnostics::make_optional(caught.take(), span, self.translator),
        }
    }

    pub(crate) fn make_duplicate<'message>(
        &self,
        message: impl Into<DiagnosticMessage<'message>>,
        span: SourceSpan,
        earlier: DuplicateLabel<'message>,
    ) -> Diagnostic {
        let mut diagnostic = self.make_semantic(message, span);
        if diagnostic.code() != DiagnosticCode::SCHEMA_SEMANTIC_ERROR {
            return diagnostic;
        }
        let DuplicateLabel {
            span: earlier_span,
            role: earlier_role,
            message: earlier_message,
        } = earlier;
        if let Some(error) = earlier_message.failure() {
            let mut failure =
                Diagnostic::translation_failure(DiagnosticCode::SCHEMA_SEMANTIC_ERROR, error);
            failure.set_primary(Some(span));
            return failure;
        }
        let mut label_errors = ErrSlot::<BudgetError>::new();
        if let Err(caught) = label_errors.try_call(|out| {
            diagnostic.try_add_label_with_text(
                DiagnosticLabelInput {
                    span: earlier_span,
                    text: earlier_message.text(),
                    role: earlier_role,
                },
                self.diagnostics.budget(),
                out,
            )
        }) {
            return BudgetDiagnostics::make(caught.take(), span, self.translator);
        }
        diagnostic
    }

    pub(crate) fn push(&mut self, diagnostic: Diagnostic) {
        if self.is_stopped() {
            return;
        }
        let fatal = matches!(
            diagnostic.code(),
            DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
                | DiagnosticCode::ALLOCATION_FAILED
                | DiagnosticCode::INTERNAL_ERROR
                | DiagnosticCode::DIAGNOSTIC_LIMIT_REACHED
        ) || diagnostic.message().is_err();
        if fatal {
            self.abort(diagnostic);
            return;
        }
        if self.diagnostics.len() >= self.limit - 1 {
            self.abort(EmergencyDiagnostics::limit_reached(
                diagnostic.primary(),
                self.translator,
            ));
            return;
        }
        let span = diagnostic.primary();
        let mut budget_errors = ErrSlot::<BudgetError>::new();
        let execution = match budget_errors.try_call(|mut out| {
            let budget = self
                .diagnostics
                .budget()
                .ok_or_else(|| {
                    BudgetError::invariant(schema_context_ref!(
                        "diagnostic-collection",
                        "diagnostic-budget-missing"
                    ))
                })
                .store_err_into(out.reborrow())?;
            budget.try_clone(out)
        }) {
            Ok(execution) => execution,
            Err(caught) => {
                self.abort(BudgetDiagnostics::make_optional(
                    caught.take(),
                    span,
                    self.translator,
                ));
                return;
            }
        };
        let failure = {
            let diagnostics = &mut self.diagnostics;
            let mut operation_errors = self.operation_errors.borrow_mut();
            match operation_errors.try_call(|mut out| {
                let mut low = 0;
                let mut high = diagnostics.len();
                while low < high {
                    execution.consume(1, out.reborrow())?;
                    let middle = low + (high - low) / 2;
                    let Some(current) = diagnostics.get(middle) else {
                        return Err(out.fail(InsertFailure::Internal {
                            index: middle,
                            operation: InsertOperation::Read,
                        }));
                    };
                    if compare_diagnostics(current, &diagnostic) != Ordering::Greater {
                        low = middle + 1;
                    } else {
                        high = middle;
                    }
                }
                execution.consume(diagnostics.len() - low, out.reborrow())?;
                diagnostics.try_reserve(3, out.reborrow())?;
                diagnostics.try_push(diagnostic, out.reborrow())?;
                let Some(tail) = diagnostics.get_mut(low..) else {
                    return Err(out.fail(InsertFailure::Internal {
                        index: low,
                        operation: InsertOperation::Shift,
                    }));
                };
                tail.rotate_right(1);
                Ok(())
            }) {
                Ok(()) => None,
                Err(caught) => Some(caught.take()),
            }
        };
        match failure {
            None => {}
            Some(InsertFailure::Budget(error)) => {
                self.abort(BudgetDiagnostics::make_optional(
                    error,
                    span,
                    self.translator,
                ));
            }
            Some(error @ InsertFailure::Internal { .. }) => {
                self.abort(
                    error.internal_diagnostic(span.unwrap_or(self.fallback_span), self.translator),
                );
            }
        }
    }

    pub(crate) fn push_semantic<'message>(
        &mut self,
        message: impl Into<DiagnosticMessage<'message>>,
        span: SourceSpan,
    ) -> bool {
        let diagnostic = self.make_semantic(message, span);
        self.push(diagnostic);
        self.is_stopped()
    }

    pub(crate) fn remaining(&self) -> usize {
        self.limit.saturating_sub(self.diagnostics.len())
    }

    pub(crate) const fn is_stopped(&self) -> bool {
        self.state.is_some()
    }

    pub(crate) fn has_diagnostics(&self) -> bool {
        !self.diagnostics.is_empty() || self.is_stopped()
    }

    pub(crate) fn into_diagnostics(self) -> ErrorDiagnostics {
        let terminal = self.state;
        ErrorDiagnostics::new(self.diagnostics, terminal, self.translator)
    }
}

static INSERT_READ_FAILURE: DiagnosticContext = diagnostic_context!(
    "typol-schema",
    "diagnostic-collection",
    "diagnostic-index-read-failed"
);
static INSERT_SHIFT_FAILURE: DiagnosticContext = diagnostic_context!(
    "typol-schema",
    "diagnostic-collection",
    "diagnostic-range-shift-failed"
);

impl InsertFailure {
    fn internal_diagnostic(&self, span: SourceSpan, translator: Translator) -> Diagnostic {
        let Self::Internal { index, operation } = self else {
            return schema_internal!(translator, span);
        };
        let context = match operation {
            InsertOperation::Read => INSERT_READ_FAILURE,
            InsertOperation::Shift => INSERT_SHIFT_FAILURE,
        }
        .with_object_id(*index as u64);
        EmergencyDiagnostics::internal(span, context, translator)
    }
}

impl From<BudgetError> for InsertFailure {
    fn from(error: BudgetError) -> Self {
        Self::Budget(error)
    }
}

pub(crate) struct BudgetDiagnostics;

impl BudgetDiagnostics {
    pub(crate) fn make(error: BudgetError, span: SourceSpan, translator: Translator) -> Diagnostic {
        Self::make_optional(error, Some(span), translator)
    }

    pub(crate) fn make_optional(
        error: BudgetError,
        span: Option<SourceSpan>,
        translator: Translator,
    ) -> Diagnostic {
        let message = match error {
            BudgetError::Allocation => t! {
                name: "diagnostic.emergency.allocation_failed",
                locale: translator,
            },
            BudgetError::CapacityExceeded => t! {
                name: "diagnostic.emergency.capacity_exceeded",
                locale: translator,
            },
            BudgetError::InvariantViolation(_) => t! {
                name: "diagnostic.emergency.internal_error",
                locale: translator,
            },
            BudgetError::Resource
            | BudgetError::RegexSteps
            | BudgetError::RegexSearchSteps
            | BudgetError::RegexWorkspace => t! {
                name: "diagnostic.emergency.resource_limit_exceeded",
                locale: translator,
            },
        };
        let mut diagnostic =
            EmergencyDiagnostics::translated(error.diagnostic_code(), message, span);
        diagnostic.set_context(error.context());
        diagnostic
    }
}

pub(crate) struct EmergencyDiagnostics;

impl EmergencyDiagnostics {
    pub(crate) fn ast_access_context(error: AstAccessError) -> DiagnosticContext {
        match error {
            AstAccessError::InvalidNodeId { id } => AST_NODE_ACCESS
                .with_object_id(id as u64)
                .with_object_index(id),
            AstAccessError::InvalidListRange { start, end } => AST_LIST_ACCESS
                .with_object_id(start as u64)
                .with_object_range(start, end),
            AstAccessError::InvalidTextRange { start, end } => AST_TEXT_ACCESS
                .with_object_id(start as u64)
                .with_object_range(start, end),
            AstAccessError::InvalidDecodedTextId { id } => AST_DECODED_TEXT_ACCESS
                .with_object_id(id as u64)
                .with_object_index(id),
        }
    }

    pub(crate) fn schema_access_context(error: SchemaAccessError) -> DiagnosticContext {
        let context = match error.table() {
            "type" | "type owner" | "child type" => SCHEMA_ACCESS_TYPE,
            "field" | "field index" | "field sort slot" | "field list slot" => SCHEMA_ACCESS_FIELD,
            "variant" => SCHEMA_ACCESS_VARIANT,
            "program" => SCHEMA_ACCESS_PROGRAM,
            "constraint" => SCHEMA_ACCESS_CONSTRAINT,
            "refinement" => SCHEMA_ACCESS_REFINEMENT,
            "shape" => SCHEMA_ACCESS_SHAPE,
            "expression node" => SCHEMA_ACCESS_EXPRESSION,
            _ => SCHEMA_ACCESS_OTHER,
        };
        context.with_object_id(error.index() as u64)
    }

    pub(crate) fn translated(
        code: DiagnosticCode,
        message: Result<Cow<'static, str>, RenderError>,
        span: Option<SourceSpan>,
    ) -> Diagnostic {
        let mut diagnostic = match message {
            Ok(Cow::Borrowed(message)) => Diagnostic::emergency(code, message),
            Ok(Cow::Owned(_)) => Diagnostic::translation_failure(code, RenderError::InvalidCatalog),
            Err(error) => Diagnostic::translation_failure(code, error),
        };
        diagnostic.set_primary(span);
        diagnostic
    }

    pub(crate) fn resource(span: SourceSpan, translator: Translator) -> Diagnostic {
        Self::translated(
            DiagnosticCode::RESOURCE_LIMIT_EXCEEDED,
            t! {
                name: "diagnostic.emergency.resource_limit_exceeded",
                locale: translator,
            },
            Some(span),
        )
    }

    pub(crate) fn allocation(span: SourceSpan, translator: Translator) -> Diagnostic {
        Self::translated(
            DiagnosticCode::ALLOCATION_FAILED,
            t! {
                name: "diagnostic.emergency.allocation_failed",
                locale: translator,
            },
            Some(span),
        )
    }

    pub(crate) fn internal(
        span: SourceSpan,
        context: DiagnosticContext,
        translator: Translator,
    ) -> Diagnostic {
        Self::internal_at(Some(span), context, translator)
    }

    pub(crate) fn internal_without_span(
        context: DiagnosticContext,
        translator: Translator,
    ) -> Diagnostic {
        Self::internal_at(None, context, translator)
    }

    fn internal_at(
        span: Option<SourceSpan>,
        context: DiagnosticContext,
        translator: Translator,
    ) -> Diagnostic {
        let mut diagnostic = Self::translated(
            DiagnosticCode::INTERNAL_ERROR,
            t! {
                name: "diagnostic.emergency.internal_error",
                locale: translator,
            },
            span,
        );
        diagnostic.set_context(Some(context));
        diagnostic
    }

    pub(crate) fn capacity(span: SourceSpan, translator: Translator) -> Diagnostic {
        Self::translated(
            DiagnosticCode::CAPACITY_EXCEEDED,
            t! {
                name: "diagnostic.emergency.capacity_exceeded",
                locale: translator,
            },
            Some(span),
        )
    }

    pub(crate) fn limit_reached(span: Option<SourceSpan>, translator: Translator) -> Diagnostic {
        Self::translated(
            DiagnosticCode::DIAGNOSTIC_LIMIT_REACHED,
            t! {
                name: "diagnostic.emergency.diagnostic_limit_reached",
                locale: translator,
            },
            span,
        )
    }
}

fn compare_diagnostics(left: &Diagnostic, right: &Diagnostic) -> Ordering {
    match (left.primary(), right.primary()) {
        (Some(left), Some(right)) => left
            .source_id()
            .get()
            .cmp(&right.source_id().get())
            .then_with(|| left.span().start().cmp(&right.span().start()))
            .then_with(|| left.span().end().cmp(&right.span().end())),
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
    }
}

impl ErrorDiagnostics {
    fn new(
        diagnostics: ChargedVec<Diagnostic>,
        terminal: Option<Diagnostic>,
        translator: Translator,
    ) -> Self {
        Self {
            diagnostics,
            terminal,
            translator,
        }
    }

    pub(crate) fn iter(&self) -> impl DoubleEndedIterator<Item = &Diagnostic> {
        self.diagnostics.iter().chain(self.terminal.iter())
    }

    pub(crate) fn last(&self) -> Option<&Diagnostic> {
        self.terminal.as_ref().or_else(|| self.diagnostics.last())
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.diagnostics.is_empty() && self.terminal.is_none()
    }

    pub(crate) fn list(&self) -> DiagnosticList<'_> {
        DiagnosticList::new(&self.diagnostics, self.terminal.as_ref())
    }

    #[cfg(test)]
    pub(crate) fn into_vec_for_test(self) -> Vec<Diagnostic> {
        let mut diagnostics = self.diagnostics.into_unmetered_vec();
        if let Some(terminal) = self.terminal {
            diagnostics.push(terminal);
        }
        diagnostics
    }

    pub(crate) fn append_terminal(&mut self, diagnostic: Diagnostic) {
        if self.terminal.is_none() {
            self.terminal = Some(diagnostic);
        }
    }

    pub(crate) const fn translator(&self) -> Translator {
        self.translator
    }
}

impl Deref for ErrorDiagnostics {
    type Target = [Diagnostic];

    fn deref(&self) -> &Self::Target {
        &self.diagnostics
    }
}
