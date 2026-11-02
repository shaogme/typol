use crate::{
    BudgetError, ExecutionBudget, InstancePath, LocatedSpan, SourceReadError, SourceSpan, Span,
    ValueSource, diagnostic_context,
    storage::{WorkspaceBox, WorkspaceString},
    workspace::WorkspaceVec,
};
use alloc::string::String;
use core::fmt;
use opeo::{OResult, Out};

/// A diagnostic context description composed of static identifiers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiagnosticContextDescriptor {
    subsystem: &'static str,
    stage: &'static str,
    failure_point: &'static str,
}

impl DiagnosticContextDescriptor {
    pub const fn new(
        subsystem: &'static str,
        stage: &'static str,
        failure_point: &'static str,
    ) -> Self {
        Self {
            subsystem,
            stage,
            failure_point,
        }
    }

    pub const fn subsystem(self) -> &'static str {
        self.subsystem
    }

    pub const fn stage(self) -> &'static str {
        self.stage
    }

    pub const fn failure_point(self) -> &'static str {
        self.failure_point
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DiagnosticObject {
    None,
    Id(u64),
    Index(usize),
    Range { start: usize, end: usize },
}

/// A compact context pointing to a static description with an optional bounded numeric object identifier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiagnosticContext {
    descriptor: &'static DiagnosticContextDescriptor,
    object: DiagnosticObject,
}

impl DiagnosticContext {
    pub const fn new(descriptor: &'static DiagnosticContextDescriptor) -> Self {
        Self {
            descriptor,
            object: DiagnosticObject::None,
        }
    }

    pub const fn with_object_id(mut self, object_id: u64) -> Self {
        self.object = DiagnosticObject::Id(object_id);
        self
    }

    /// Retains an exact `usize` index for structured error queries.
    pub const fn with_object_index(mut self, object_index: usize) -> Self {
        self.object = DiagnosticObject::Index(object_index);
        self
    }

    /// Retains both `usize` endpoints of a range exactly.
    pub const fn with_object_range(mut self, start: usize, end: usize) -> Self {
        self.object = DiagnosticObject::Range { start, end };
        self
    }

    pub const fn subsystem(self) -> &'static str {
        self.descriptor.subsystem()
    }

    pub const fn stage(self) -> &'static str {
        self.descriptor.stage()
    }

    pub const fn failure_point(self) -> &'static str {
        self.descriptor.failure_point()
    }

    pub const fn object_id(self) -> Option<u64> {
        match self.object {
            DiagnosticObject::None => None,
            DiagnosticObject::Id(object_id) => Some(object_id),
            DiagnosticObject::Index(index) => Some(index as u64),
            DiagnosticObject::Range { start, .. } => Some(start as u64),
        }
    }

    /// Returns the exactly retained `usize` object index.
    pub const fn object_index(self) -> Option<usize> {
        match self.object {
            DiagnosticObject::Index(index) => Some(index),
            _ => None,
        }
    }

    /// Returns exactly retained range endpoints.
    pub const fn object_range(self) -> Option<(usize, usize)> {
        match self.object {
            DiagnosticObject::Range { start, end } => Some((start, end)),
            _ => None,
        }
    }
}

static PATH_BUDGET_MISMATCH: DiagnosticContext =
    diagnostic_context!("typol-core", "diagnostic", "path-budget-identity-mismatch",);

#[macro_export]
macro_rules! diagnostic_context {
    ($subsystem:literal, $stage:literal, $failure_point:literal $(,)?) => {{
        static DESCRIPTOR: $crate::DiagnosticContextDescriptor =
            $crate::DiagnosticContextDescriptor::new($subsystem, $stage, $failure_point);
        $crate::DiagnosticContext::new(&DESCRIPTOR)
    }};
}

/// A stable diagnostic code. Once assigned, its value and meaning do not change within the same major version.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct DiagnosticCode(u32);

impl DiagnosticCode {
    pub const RESOURCE_LIMIT_EXCEEDED: Self = Self(1);
    pub const ALLOCATION_FAILED: Self = Self(2);
    pub const LEXICAL_ERROR: Self = Self(3);
    pub const SCHEMA_SYNTAX_ERROR: Self = Self(4);
    pub const DIAGNOSTIC_LIMIT_REACHED: Self = Self(5);
    pub const REGEX_SYNTAX_ERROR: Self = Self(6);
    pub const SCHEMA_SEMANTIC_ERROR: Self = Self(7);
    pub const SCHEMA_RESOLVER_ERROR: Self = Self(8);
    pub const INTERNAL_ERROR: Self = Self(9);
    pub const CONFIG_SYNTAX_ERROR: Self = Self(10);
    pub const CONFIG_VALUE_ERROR: Self = Self(11);
    pub const CONFIG_VALIDATION_ERROR: Self = Self(12);
    pub const CONFIG_ENVIRONMENT_ERROR: Self = Self(13);
    pub const TYPE_BINDING_ERROR: Self = Self(14);
    pub const RUST_CONVERSION_ERROR: Self = Self(15);
    pub const CONFIG_ENCODING_ERROR: Self = Self(16);
    pub const SOURCE_INPUT_ERROR: Self = Self(17);
    pub const CAPACITY_EXCEEDED: Self = Self(18);
    pub const SCHEMA_CONTEXT_MISMATCH: Self = Self(19);
    pub const SCHEMA_SESSION_FINISHED: Self = Self(20);

    pub const fn new(value: u32) -> Option<Self> {
        if value == 0 { None } else { Some(Self(value)) }
    }

    pub const fn get(self) -> u32 {
        self.0
    }

    pub const fn name(self) -> &'static str {
        match self.0 {
            1 => "RESOURCE_LIMIT_EXCEEDED",
            2 => "ALLOCATION_FAILED",
            3 => "LEXICAL_ERROR",
            4 => "SCHEMA_SYNTAX_ERROR",
            5 => "DIAGNOSTIC_LIMIT_REACHED",
            6 => "REGEX_SYNTAX_ERROR",
            7 => "SCHEMA_SEMANTIC_ERROR",
            8 => "SCHEMA_RESOLVER_ERROR",
            9 => "INTERNAL_ERROR",
            10 => "CONFIG_SYNTAX_ERROR",
            11 => "CONFIG_VALUE_ERROR",
            12 => "CONFIG_VALIDATION_ERROR",
            13 => "CONFIG_ENVIRONMENT_ERROR",
            14 => "TYPE_BINDING_ERROR",
            15 => "RUST_CONVERSION_ERROR",
            16 => "CONFIG_ENCODING_ERROR",
            17 => "SOURCE_INPUT_ERROR",
            18 => "CAPACITY_EXCEEDED",
            19 => "SCHEMA_CONTEXT_MISMATCH",
            20 => "SCHEMA_SESSION_FINISHED",
            _ => "UNKNOWN",
        }
    }
}

impl fmt::Debug for DiagnosticCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("DiagnosticCode")
            .field(&self.0)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiagnosticTranslationError;

/// Opaque index of a message declared in the generated diagnostic catalog.
///
/// Its numeric representation is build-local and must not be persisted or serialized.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiagnosticMessageKey(u16);

impl DiagnosticMessageKey {
    #[doc(hidden)]
    pub const fn __from_index(index: u16) -> Self {
        Self(index)
    }

    #[doc(hidden)]
    pub const fn __index(self) -> u16 {
        self.0
    }
}

#[derive(Clone, Copy)]
pub struct DiagnosticMessage<'a> {
    text: &'a str,
    key: Option<DiagnosticMessageKey>,
}

impl<'a> DiagnosticMessage<'a> {
    /// Creates a message that is displayed as provided.
    pub const fn custom(text: &'a str) -> Self {
        Self { text, key: None }
    }

    /// Creates a message with an English fallback and a generated translation key.
    pub const fn localized(text: &'a str, key: DiagnosticMessageKey) -> Self {
        Self {
            text,
            key: Some(key),
        }
    }

    /// Returns the generated translation key, if this message is localized.
    pub const fn key(self) -> Option<DiagnosticMessageKey> {
        self.key
    }

    /// Returns the English fallback or custom text.
    pub const fn text(self) -> &'a str {
        self.text
    }
}

impl<'a> From<&'a str> for DiagnosticMessage<'a> {
    fn from(text: &'a str) -> Self {
        Self::custom(text)
    }
}

impl<'a> From<&'a String> for DiagnosticMessage<'a> {
    fn from(text: &'a String) -> Self {
        Self::custom(text)
    }
}

/// Associates a source location with a message managed by the diagnostic’s own budget.
pub struct DiagnosticLabel {
    span: LocatedSpan,
    message: WorkspaceString,
    key: Option<DiagnosticMessageKey>,
}

impl DiagnosticLabel {
    pub const fn span(&self) -> SourceSpan {
        self.span.source_span()
    }

    pub const fn located_span(&self) -> LocatedSpan {
        self.span
    }

    pub fn message(&self) -> &str {
        self.message.as_str()
    }

    /// Returns the translation key attached to this label, if any.
    pub const fn key(&self) -> Option<DiagnosticMessageKey> {
        self.key
    }
}

impl fmt::Debug for DiagnosticLabel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DiagnosticLabel")
            .field("span", &self.span)
            .field("message_bytes", &self.message.len())
            .finish()
    }
}

struct DiagnosticDetails {
    message: WorkspaceString,
    key: Option<DiagnosticMessageKey>,
    labels: WorkspaceVec<DiagnosticLabel>,
    context: Option<DiagnosticContext>,
    value_source: Option<ValueSource>,
    path: Option<InstancePath>,
}

enum DiagnosticStorage {
    Emergency {
        context: Option<DiagnosticContext>,
        value_source: Option<ValueSource>,
    },
    Complete(WorkspaceBox<DiagnosticDetails>),
}

/// A structured diagnostic whose labels can be extended by downstream crates.
pub struct Diagnostic {
    code: DiagnosticCode,
    primary: Option<LocatedSpan>,
    storage: DiagnosticStorage,
}

impl From<SourceReadError> for Diagnostic {
    fn from(error: SourceReadError) -> Self {
        let mut diagnostic = Self::emergency(error.diagnostic_code());
        diagnostic.set_context(error.context());
        if let (Some(source_id), Some(offset)) = (error.source_id(), error.offset()) {
            diagnostic.set_primary(Some(SourceSpan::new(source_id, Span::point(offset))));
        }
        diagnostic
    }
}

impl Diagnostic {
    /// Creates an emergency diagnostic without a message or allocation.
    pub const fn emergency(code: DiagnosticCode) -> Self {
        Self {
            code,
            primary: None,
            storage: DiagnosticStorage::Emergency {
                context: None,
                value_source: None,
            },
        }
    }

    pub fn try_new<'slot, 'message>(
        code: DiagnosticCode,
        message: impl Into<DiagnosticMessage<'message>>,
        budget: Option<&ExecutionBudget>,
        mut out: Out<'slot, '_, BudgetError>,
    ) -> OResult<'slot, Self> {
        let message = message.into();
        let text = WorkspaceString::copy(message.text(), budget, out.reborrow())?;
        let labels = WorkspaceVec::new(budget, out.reborrow())?;
        let details = DiagnosticDetails {
            message: text,
            key: message.key(),
            labels,
            context: None,
            value_source: None,
            path: None,
        };
        let storage = WorkspaceBox::new(details, budget, out.reborrow())?;
        Ok(Self {
            code,
            primary: None,
            storage: DiagnosticStorage::Complete(storage),
        })
    }

    pub fn budget(&self) -> Option<&ExecutionBudget> {
        match &self.storage {
            DiagnosticStorage::Emergency { .. } => None,
            DiagnosticStorage::Complete(details) => details.message.budget(),
        }
    }

    fn promote<'slot>(
        &mut self,
        budget: Option<&ExecutionBudget>,
        mut out: Out<'slot, '_, BudgetError>,
    ) -> OResult<'slot, ()> {
        let (context, value_source) = match &self.storage {
            DiagnosticStorage::Emergency {
                context,
                value_source,
            } => (*context, *value_source),
            DiagnosticStorage::Complete(_) => return Ok(()),
        };
        let message = WorkspaceString::with_capacity(0, budget, out.reborrow())?;
        let labels = WorkspaceVec::new(budget, out.reborrow())?;
        let details = DiagnosticDetails {
            message,
            key: None,
            labels,
            context,
            value_source,
            path: None,
        };
        let storage = WorkspaceBox::new(details, budget, out.reborrow())?;
        self.storage = DiagnosticStorage::Complete(storage);
        Ok(())
    }

    pub fn try_add_label<'slot>(
        &mut self,
        span: impl Into<LocatedSpan>,
        text: &str,
        budget: Option<&ExecutionBudget>,
        out: Out<'slot, '_, BudgetError>,
    ) -> OResult<'slot, ()> {
        self.try_add_label_with_text(span, DiagnosticMessage::custom(text), budget, out)
    }

    pub fn try_add_label_with_text<'slot>(
        &mut self,
        span: impl Into<LocatedSpan>,
        message: DiagnosticMessage<'_>,
        budget: Option<&ExecutionBudget>,
        mut out: Out<'slot, '_, BudgetError>,
    ) -> OResult<'slot, ()> {
        self.promote(budget, out.reborrow())?;
        let DiagnosticStorage::Complete(details) = &mut self.storage else {
            return Err(out.fail(BudgetError::Allocation));
        };
        if !same_budget(details.message.budget(), budget) {
            return Err(out.fail(BudgetError::invariant(&PATH_BUDGET_MISMATCH)));
        }
        let text = WorkspaceString::copy(message.text(), budget, out.reborrow())?;
        details.labels.try_push(
            DiagnosticLabel {
                span: span.into(),
                message: text,
                key: message.key,
            },
            out.reborrow(),
        )
    }

    pub const fn code(&self) -> DiagnosticCode {
        self.code
    }

    pub fn message(&self) -> &str {
        match &self.storage {
            DiagnosticStorage::Emergency { .. } => "",
            DiagnosticStorage::Complete(details) => details.message.as_str(),
        }
    }

    /// Returns the generated key attached to the diagnostic's message, if any.
    pub fn message_key(&self) -> Option<DiagnosticMessageKey> {
        match &self.storage {
            DiagnosticStorage::Emergency { .. } => None,
            DiagnosticStorage::Complete(details) => details.key,
        }
    }

    /// Replaces the generated key attached to the message; emergency diagnostics keep none.
    pub fn set_message_key(&mut self, key: Option<DiagnosticMessageKey>) {
        match &mut self.storage {
            DiagnosticStorage::Emergency { .. } => {}
            DiagnosticStorage::Complete(details) => details.key = key,
        }
    }

    pub const fn primary(&self) -> Option<SourceSpan> {
        match self.primary {
            Some(primary) => Some(primary.source_span()),
            None => None,
        }
    }

    pub fn labels(&self) -> &[DiagnosticLabel] {
        match &self.storage {
            DiagnosticStorage::Emergency { .. } => &[],
            DiagnosticStorage::Complete(details) => &details.labels,
        }
    }

    pub fn path(&self) -> Option<&InstancePath> {
        match &self.storage {
            DiagnosticStorage::Emergency { .. } => None,
            DiagnosticStorage::Complete(details) => details.path.as_ref(),
        }
    }

    pub fn value_source(&self) -> Option<ValueSource> {
        match &self.storage {
            DiagnosticStorage::Emergency { value_source, .. } => *value_source,
            DiagnosticStorage::Complete(details) => details.value_source,
        }
    }

    pub fn set_primary(&mut self, primary: Option<SourceSpan>) {
        self.primary = primary.map(LocatedSpan::from);
    }

    pub fn set_located_primary(&mut self, primary: Option<LocatedSpan>) {
        self.primary = primary;
    }

    pub fn located_primary(&self) -> Option<LocatedSpan> {
        self.primary
    }

    pub fn context(&self) -> Option<DiagnosticContext> {
        match &self.storage {
            DiagnosticStorage::Emergency { context, .. } => *context,
            DiagnosticStorage::Complete(details) => details.context,
        }
    }

    pub fn set_context(&mut self, context: Option<DiagnosticContext>) {
        match &mut self.storage {
            DiagnosticStorage::Emergency {
                context: stored, ..
            } => *stored = context,
            DiagnosticStorage::Complete(details) => details.context = context,
        }
    }

    pub fn try_set_path<'slot>(
        &mut self,
        path: Option<InstancePath>,
        budget: Option<&ExecutionBudget>,
        mut out: Out<'slot, '_, BudgetError>,
    ) -> OResult<'slot, ()> {
        if path.is_none() {
            if let DiagnosticStorage::Complete(details) = &mut self.storage {
                details.path = None;
            }
            return Ok(());
        }
        if matches!(
            self.code,
            DiagnosticCode::ALLOCATION_FAILED | DiagnosticCode::DIAGNOSTIC_LIMIT_REACHED
        ) {
            return Ok(());
        }
        let Some(path) = path else {
            return Ok(());
        };
        if !same_budget(path.budget(), budget) {
            return Err(out.fail(BudgetError::invariant(&PATH_BUDGET_MISMATCH)));
        }
        self.promote(budget, out.reborrow())?;
        let DiagnosticStorage::Complete(details) = &mut self.storage else {
            return Err(out.fail(BudgetError::Allocation));
        };
        if !same_budget(details.message.budget(), budget) {
            return Err(out.fail(BudgetError::invariant(&PATH_BUDGET_MISMATCH)));
        }
        details.path = Some(path);
        Ok(())
    }

    pub fn set_value_source(&mut self, value_source: Option<ValueSource>) {
        match &mut self.storage {
            DiagnosticStorage::Emergency {
                value_source: stored,
                ..
            } => *stored = value_source,
            DiagnosticStorage::Complete(details) => details.value_source = value_source,
        }
    }
}

fn same_budget(left: Option<&ExecutionBudget>, right: Option<&ExecutionBudget>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left.same_call(right),
        (None, None) => true,
        _ => false,
    }
}

impl fmt::Debug for Diagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Diagnostic")
            .field("code", &self.code)
            .field("has_primary", &self.primary().is_some())
            .field("context", &self.context())
            .field("label_count", &self.labels().len())
            .field("has_path", &self.path().is_some())
            .field("value_source", &self.value_source())
            .finish()
    }
}

impl From<BudgetError> for Diagnostic {
    fn from(error: BudgetError) -> Self {
        let mut diagnostic = Self::emergency(error.diagnostic_code());
        diagnostic.set_context(error.context());
        diagnostic
    }
}
