mod foundation;
mod kind;
mod parameters;
mod related;

pub use foundation::{
    DiagnosticCode, DiagnosticContext, DiagnosticContextDescriptor, DiagnosticObjectIdentifier,
};
pub use kind::{DiagnosticFieldStatus, DiagnosticKind, DiagnosticStage};
pub use parameters::{
    DiagnosticMessage, DiagnosticParameter, DiagnosticParameterValue, DiagnosticParameters,
    MAX_DIAGNOSTIC_PARAMETER_NAME_BYTES, MAX_DIAGNOSTIC_PARAMETERS,
};
pub use related::{DiagnosticLabel, DiagnosticLabelInput, RelatedLocation, RelatedLocationRole};

use alloc::borrow::Cow;

use crate::{
    BudgetError, ExecutionBudget, InstancePath, LocatedSpan, SourceId, SourceLocation,
    SourceReadError, SourceSpan, Span, SpanLocation, ValueSource, diagnostic_context,
    storage::{WorkspaceBox, WorkspaceString},
    workspace::WorkspaceVec,
};
use core::{error::Error, fmt};
use opeo::{ErrSlot, OResult, Out};
use u18n::RenderError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticFailure {
    Translation(RenderError),
    Budget(BudgetError),
    SourceRead(SourceReadError),
    EmergencyMessageUnavailable(DiagnosticCode),
}

impl fmt::Display for DiagnosticFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Translation(error) => fmt::Display::fmt(error, formatter),
            Self::Budget(error) => fmt::Display::fmt(error, formatter),
            Self::SourceRead(error) => fmt::Display::fmt(error, formatter),
            Self::EmergencyMessageUnavailable(code) => {
                write!(
                    formatter,
                    "emergency message unavailable for diagnostic {code:?}"
                )
            }
        }
    }
}

impl Error for DiagnosticFailure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Translation(error) => Some(error),
            Self::Budget(error) => Some(error),
            Self::SourceRead(error) => Some(error),
            Self::EmergencyMessageUnavailable(_) => None,
        }
    }
}

static PATH_BUDGET_MISMATCH: DiagnosticContext =
    diagnostic_context!("typol-core", "diagnostic", "path-budget-identity-mismatch",);

const fn empty_source_span() -> SourceSpan {
    SourceSpan::new(SourceId::new(0), Span::empty())
}

pub const MAX_RELATED_LOCATIONS: usize = 8;

#[macro_export]
macro_rules! diagnostic_context {
    ($subsystem:literal, $stage:literal, $failure_point:literal $(,)?) => {{
        static DESCRIPTOR: $crate::DiagnosticContextDescriptor =
            $crate::DiagnosticContextDescriptor::new($subsystem, $stage, $failure_point);
        $crate::DiagnosticContext::new(&DESCRIPTOR)
    }};
}

struct DiagnosticDetails {
    message: WorkspaceString,
    failure: Option<DiagnosticFailure>,
    parameters: Option<WorkspaceBox<DiagnosticParameters>>,
    kind_override: Option<DiagnosticKind>,
    labels: WorkspaceVec<RelatedLocation>,
    context: Option<DiagnosticContext>,
    value_source: Option<ValueSource>,
    path: Option<InstancePath>,
}

enum DiagnosticStorage {
    Emergency {
        message: Cow<'static, str>,
        failure: Option<DiagnosticFailure>,
        context: Option<DiagnosticContext>,
        value_source: Option<ValueSource>,
        kind_override: Option<DiagnosticKind>,
    },
    Complete(WorkspaceBox<DiagnosticDetails>),
}

struct DiagnosticEmergencyState {
    message: Cow<'static, str>,
    failure: Option<DiagnosticFailure>,
    context: Option<DiagnosticContext>,
    value_source: Option<ValueSource>,
    kind_override: Option<DiagnosticKind>,
}

#[derive(Clone, Copy)]
enum DiagnosticStatusField {
    Parameters,
    Primary,
    PrimaryLocation,
    Context,
    RelatedLocations,
    Path,
    ValueSource,
}

#[derive(Clone, Copy)]
struct DiagnosticPrimary {
    span: SourceSpan,
    location: [u32; 4],
}

impl DiagnosticPrimary {
    const EMPTY: Self = Self {
        span: empty_source_span(),
        location: [0; 4],
    };

    fn from_span(span: SourceSpan) -> Self {
        Self {
            span,
            location: [0; 4],
        }
    }

    fn from_located(span: LocatedSpan) -> (Self, DiagnosticFieldStatus) {
        let mut primary = Self::from_span(span.source_span());
        let Some(location) = span.location() else {
            return (primary, DiagnosticFieldStatus::NotApplicable);
        };
        let values = [
            location.start.line(),
            location.start.column(),
            location.end.line(),
            location.end.column(),
        ];
        if values.iter().any(|value| *value > u32::MAX as usize) {
            return (primary, DiagnosticFieldStatus::Unavailable);
        }
        primary.location = values.map(|value| value as u32);
        (primary, DiagnosticFieldStatus::Available)
    }

    fn located_span(self, location_status: DiagnosticFieldStatus) -> LocatedSpan {
        if location_status != DiagnosticFieldStatus::Available {
            return LocatedSpan::from(self.span);
        }
        let [start_line, start_column, end_line, end_column] = self.location;
        let Some(start) = SourceLocation::new(start_line as usize, start_column as usize) else {
            return LocatedSpan::from(self.span);
        };
        let Some(end) = SourceLocation::new(end_line as usize, end_column as usize) else {
            return LocatedSpan::from(self.span);
        };
        LocatedSpan::new(self.span, Some(SpanLocation { start, end }))
    }
}

#[derive(Clone, Copy)]
struct DiagnosticStatuses(u16);

impl DiagnosticStatuses {
    const EMPTY: Self = Self(0);
    const TRUNCATED_BIT: u16 = 1 << 14;

    const fn get(self, field: DiagnosticStatusField) -> DiagnosticFieldStatus {
        let shift = match field {
            DiagnosticStatusField::Parameters => 0,
            DiagnosticStatusField::Primary => 2,
            DiagnosticStatusField::Context => 4,
            DiagnosticStatusField::RelatedLocations => 6,
            DiagnosticStatusField::Path => 8,
            DiagnosticStatusField::ValueSource => 10,
            DiagnosticStatusField::PrimaryLocation => 12,
        };
        match (self.0 >> shift) & 0b11 {
            0 => DiagnosticFieldStatus::NotApplicable,
            1 => DiagnosticFieldStatus::Available,
            2 => DiagnosticFieldStatus::Unavailable,
            _ => DiagnosticFieldStatus::Redacted,
        }
    }

    fn set(&mut self, field: DiagnosticStatusField, status: DiagnosticFieldStatus) {
        let shift = match field {
            DiagnosticStatusField::Parameters => 0,
            DiagnosticStatusField::Primary => 2,
            DiagnosticStatusField::Context => 4,
            DiagnosticStatusField::RelatedLocations => 6,
            DiagnosticStatusField::Path => 8,
            DiagnosticStatusField::ValueSource => 10,
            DiagnosticStatusField::PrimaryLocation => 12,
        };
        let value = match status {
            DiagnosticFieldStatus::NotApplicable => 0,
            DiagnosticFieldStatus::Available => 1,
            DiagnosticFieldStatus::Unavailable => 2,
            DiagnosticFieldStatus::Redacted => 3,
        };
        self.0 = (self.0 & !(0b11 << shift)) | (value << shift);
    }

    const fn related_locations_truncated(self) -> bool {
        self.0 & Self::TRUNCATED_BIT != 0
    }

    fn set_related_locations_truncated(&mut self, truncated: bool) {
        if truncated {
            self.0 |= Self::TRUNCATED_BIT;
        } else {
            self.0 &= !Self::TRUNCATED_BIT;
        }
    }
}

/// A structured diagnostic whose labels can be extended by downstream crates.
pub struct Diagnostic {
    code: DiagnosticCode,
    primary: DiagnosticPrimary,
    statuses: DiagnosticStatuses,
    storage: DiagnosticStorage,
}

impl Diagnostic {
    /// Creates an emergency diagnostic from static final text without allocating.
    pub const fn emergency(code: DiagnosticCode, message: &'static str) -> Self {
        Self {
            code,
            primary: DiagnosticPrimary::EMPTY,
            statuses: DiagnosticStatuses::EMPTY,
            storage: DiagnosticStorage::Emergency {
                message: Cow::Borrowed(message),
                failure: None,
                context: None,
                value_source: None,
                kind_override: None,
            },
        }
    }

    pub const fn translation_failure(code: DiagnosticCode, error: RenderError) -> Self {
        Self::failure(code, DiagnosticFailure::Translation(error))
    }

    pub fn translated_emergency(
        code: DiagnosticCode,
        message: Result<Cow<'static, str>, RenderError>,
    ) -> Self {
        match message {
            Ok(message) => Self {
                code,
                primary: DiagnosticPrimary::EMPTY,
                statuses: DiagnosticStatuses::EMPTY,
                storage: DiagnosticStorage::Emergency {
                    message,
                    failure: None,
                    context: None,
                    value_source: None,
                    kind_override: None,
                },
            },
            Err(error) => Self::translation_failure(code, error),
        }
    }

    pub const fn failure(code: DiagnosticCode, failure: DiagnosticFailure) -> Self {
        Self {
            code,
            primary: DiagnosticPrimary::EMPTY,
            statuses: DiagnosticStatuses::EMPTY,
            storage: DiagnosticStorage::Emergency {
                message: Cow::Borrowed(""),
                failure: Some(failure),
                context: None,
                value_source: None,
                kind_override: None,
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
        if let Some(error) = message.failure() {
            return Ok(Self::translation_failure(code, error));
        }
        let text = WorkspaceString::copy(message.text(), budget, out.reborrow())?;
        Self::try_new_with_text(code, text, message.parameters().copied(), budget, out)
    }

    pub fn try_new_with_text<'slot>(
        code: DiagnosticCode,
        text: WorkspaceString,
        parameters: Option<DiagnosticParameters>,
        budget: Option<&ExecutionBudget>,
        mut out: Out<'slot, '_, BudgetError>,
    ) -> OResult<'slot, Self> {
        if !same_budget(text.budget(), budget) {
            return Err(out.fail(BudgetError::invariant(&PATH_BUDGET_MISMATCH)));
        }
        let labels = WorkspaceVec::new(budget, out.reborrow())?;
        let parameters_status = parameters.map_or(DiagnosticFieldStatus::NotApplicable, |values| {
            values.status()
        });
        let parameters = match parameters.filter(|values| !values.is_empty()) {
            Some(values) => Some(WorkspaceBox::new(values, budget, out.reborrow())?),
            None => None,
        };
        let details = DiagnosticDetails {
            message: text,
            failure: None,
            parameters,
            kind_override: None,
            labels,
            context: None,
            value_source: None,
            path: None,
        };
        let storage = WorkspaceBox::new(details, budget, out.reborrow())?;
        Ok(Self {
            code,
            primary: DiagnosticPrimary::EMPTY,
            statuses: {
                let mut statuses = DiagnosticStatuses::EMPTY;
                statuses.set(DiagnosticStatusField::Parameters, parameters_status);
                statuses
            },
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
        let emergency_state = match &self.storage {
            DiagnosticStorage::Emergency {
                message,
                failure,
                context,
                value_source,
                kind_override,
            } => DiagnosticEmergencyState {
                message: message.clone(),
                failure: *failure,
                context: *context,
                value_source: *value_source,
                kind_override: *kind_override,
            },
            DiagnosticStorage::Complete(_) => return Ok(()),
        };
        let DiagnosticEmergencyState {
            message,
            failure,
            context,
            value_source,
            kind_override,
        } = emergency_state;
        let message = WorkspaceString::copy(message.as_ref(), budget, out.reborrow())?;
        let labels = WorkspaceVec::new(budget, out.reborrow())?;
        let details = DiagnosticDetails {
            message,
            failure,
            parameters: None,
            kind_override,
            labels,
            context,
            value_source,
            path: None,
        };
        let storage = WorkspaceBox::new(details, budget, out.reborrow())?;
        self.storage = DiagnosticStorage::Complete(storage);
        Ok(())
    }

    pub fn try_add_label_with_text<'slot>(
        &mut self,
        label: DiagnosticLabelInput<'_, impl Into<LocatedSpan>>,
        budget: Option<&ExecutionBudget>,
        mut out: Out<'slot, '_, BudgetError>,
    ) -> OResult<'slot, ()> {
        let DiagnosticLabelInput { span, text, role } = label;
        self.promote(budget, out.reborrow())?;
        let DiagnosticStorage::Complete(details) = &mut self.storage else {
            return Err(out.fail(BudgetError::Allocation));
        };
        if details.labels.len() >= MAX_RELATED_LOCATIONS {
            self.statuses.set_related_locations_truncated(true);
            self.statuses.set(
                DiagnosticStatusField::RelatedLocations,
                DiagnosticFieldStatus::Available,
            );
            return Ok(());
        }
        if !same_budget(details.message.budget(), budget) {
            return Err(out.fail(BudgetError::invariant(&PATH_BUDGET_MISMATCH)));
        }
        self.statuses.set(
            DiagnosticStatusField::RelatedLocations,
            DiagnosticFieldStatus::Unavailable,
        );
        let text = WorkspaceString::copy(text, budget, out.reborrow())?;
        details.labels.try_push(
            RelatedLocation::new(span.into(), text, role),
            out.reborrow(),
        )?;
        self.statuses.set(
            DiagnosticStatusField::RelatedLocations,
            DiagnosticFieldStatus::Available,
        );
        Ok(())
    }

    pub const fn code(&self) -> DiagnosticCode {
        self.code
    }

    pub fn parameters(&self) -> &[DiagnosticParameter] {
        match &self.storage {
            DiagnosticStorage::Emergency { .. } => &[],
            DiagnosticStorage::Complete(details) => details
                .parameters
                .as_deref()
                .map_or(&[], DiagnosticParameters::as_slice),
        }
    }

    pub const fn parameters_status(&self) -> DiagnosticFieldStatus {
        self.statuses.get(DiagnosticStatusField::Parameters)
    }

    pub fn set_parameters(&mut self, parameters: DiagnosticParameters) {
        match &mut self.storage {
            DiagnosticStorage::Emergency { .. } => {
                let status = if parameters.is_empty() {
                    parameters.status()
                } else {
                    DiagnosticFieldStatus::Unavailable
                };
                self.statuses.set(DiagnosticStatusField::Parameters, status);
            }
            DiagnosticStorage::Complete(details) => {
                self.statuses
                    .set(DiagnosticStatusField::Parameters, parameters.status());
                if parameters.is_empty() {
                    details.parameters = None;
                    return;
                }
                if let Some(stored) = details.parameters.as_mut() {
                    **stored = parameters;
                    return;
                }
                let budget = details.message.budget();
                let mut errors = ErrSlot::<BudgetError>::new();
                match errors.try_call(|out| WorkspaceBox::new(parameters, budget, out)) {
                    Ok(stored) => details.parameters = Some(stored),
                    Err(caught) => {
                        let _ = caught.take();
                        self.statuses.set(
                            DiagnosticStatusField::Parameters,
                            DiagnosticFieldStatus::Unavailable,
                        );
                    }
                }
            }
        }
    }

    pub fn kind(&self) -> DiagnosticKind {
        match &self.storage {
            DiagnosticStorage::Emergency {
                context,
                kind_override,
                ..
            } => kind_override.unwrap_or_else(|| DiagnosticKind::emergency(self.code, *context)),
            DiagnosticStorage::Complete(details) => details
                .kind_override
                .unwrap_or_else(|| DiagnosticKind::custom(self.code)),
        }
    }

    pub const fn is_emergency(&self) -> bool {
        matches!(&self.storage, DiagnosticStorage::Emergency { .. })
    }

    pub fn message(&self) -> Result<&str, DiagnosticFailure> {
        match &self.storage {
            DiagnosticStorage::Emergency {
                message, failure, ..
            } => failure.map_or(Ok(message.as_ref()), Err),
            DiagnosticStorage::Complete(details) => {
                details.failure.map_or(Ok(details.message.as_str()), Err)
            }
        }
    }

    pub fn set_kind(&mut self, kind: DiagnosticKind) {
        match &mut self.storage {
            DiagnosticStorage::Emergency { kind_override, .. } => *kind_override = Some(kind),
            DiagnosticStorage::Complete(details) => details.kind_override = Some(kind),
        }
    }

    pub const fn primary(&self) -> Option<SourceSpan> {
        if matches!(self.primary_status(), DiagnosticFieldStatus::Available) {
            Some(self.primary.span)
        } else {
            None
        }
    }

    pub const fn primary_status(&self) -> DiagnosticFieldStatus {
        self.statuses.get(DiagnosticStatusField::Primary)
    }

    pub const fn primary_location_status(&self) -> DiagnosticFieldStatus {
        self.statuses.get(DiagnosticStatusField::PrimaryLocation)
    }

    pub fn related_locations(&self) -> &[RelatedLocation] {
        match &self.storage {
            DiagnosticStorage::Emergency { .. } => &[],
            DiagnosticStorage::Complete(details) => &details.labels,
        }
    }

    pub const fn related_locations_status(&self) -> DiagnosticFieldStatus {
        self.statuses.get(DiagnosticStatusField::RelatedLocations)
    }

    pub const fn related_locations_truncated(&self) -> bool {
        self.statuses.related_locations_truncated()
    }

    pub fn labels(&self) -> &[DiagnosticLabel] {
        self.related_locations()
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

    pub const fn value_source_status(&self) -> DiagnosticFieldStatus {
        self.statuses.get(DiagnosticStatusField::ValueSource)
    }

    pub const fn path_status(&self) -> DiagnosticFieldStatus {
        self.statuses.get(DiagnosticStatusField::Path)
    }

    pub fn set_primary(&mut self, primary: Option<SourceSpan>) {
        self.primary = primary.map_or(DiagnosticPrimary::EMPTY, DiagnosticPrimary::from_span);
        self.statuses.set(
            DiagnosticStatusField::Primary,
            status_for_presence(primary.is_some()),
        );
        self.statuses.set(
            DiagnosticStatusField::PrimaryLocation,
            DiagnosticFieldStatus::NotApplicable,
        );
    }

    pub fn set_located_primary(&mut self, primary: Option<LocatedSpan>) {
        let (stored, location_status) = primary.map_or(
            (
                DiagnosticPrimary::EMPTY,
                DiagnosticFieldStatus::NotApplicable,
            ),
            DiagnosticPrimary::from_located,
        );
        self.primary = stored;
        self.statuses.set(
            DiagnosticStatusField::Primary,
            status_for_presence(primary.is_some()),
        );
        self.statuses
            .set(DiagnosticStatusField::PrimaryLocation, location_status);
    }

    pub fn set_primary_status(&mut self, status: DiagnosticFieldStatus) {
        let has_primary = self.primary_status() == DiagnosticFieldStatus::Available;
        if status != DiagnosticFieldStatus::Available {
            self.primary = DiagnosticPrimary::EMPTY;
            self.statuses.set(
                DiagnosticStatusField::PrimaryLocation,
                DiagnosticFieldStatus::NotApplicable,
            );
        }
        let status = if status == DiagnosticFieldStatus::Available && !has_primary {
            DiagnosticFieldStatus::Unavailable
        } else {
            status
        };
        self.statuses.set(DiagnosticStatusField::Primary, status);
    }

    pub fn located_primary(&self) -> Option<LocatedSpan> {
        if matches!(self.primary_status(), DiagnosticFieldStatus::Available) {
            Some(self.primary.located_span(self.primary_location_status()))
        } else {
            None
        }
    }

    pub fn context(&self) -> Option<DiagnosticContext> {
        match &self.storage {
            DiagnosticStorage::Emergency { context, .. } => *context,
            DiagnosticStorage::Complete(details) => details.context,
        }
    }

    pub const fn context_status(&self) -> DiagnosticFieldStatus {
        self.statuses.get(DiagnosticStatusField::Context)
    }

    pub fn set_context(&mut self, context: Option<DiagnosticContext>) {
        self.statuses.set(
            DiagnosticStatusField::Context,
            status_for_presence(context.is_some()),
        );
        match &mut self.storage {
            DiagnosticStorage::Emergency {
                context: stored, ..
            } => *stored = context,
            DiagnosticStorage::Complete(details) => details.context = context,
        }
    }

    pub fn set_context_status(&mut self, status: DiagnosticFieldStatus) {
        if status != DiagnosticFieldStatus::Available {
            self.set_context(None);
        }
        let status = if status == DiagnosticFieldStatus::Available && self.context().is_none() {
            DiagnosticFieldStatus::Unavailable
        } else {
            status
        };
        self.statuses.set(DiagnosticStatusField::Context, status);
    }

    pub fn set_related_locations_status(&mut self, status: DiagnosticFieldStatus) {
        if status != DiagnosticFieldStatus::Available {
            if let DiagnosticStorage::Complete(details) = &mut self.storage {
                details.labels.clear();
            }
            self.statuses.set_related_locations_truncated(false);
        }
        let status =
            if status == DiagnosticFieldStatus::Available && self.related_locations().is_empty() {
                DiagnosticFieldStatus::Unavailable
            } else {
                status
            };
        self.statuses
            .set(DiagnosticStatusField::RelatedLocations, status);
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
            self.statuses.set(
                DiagnosticStatusField::Path,
                DiagnosticFieldStatus::NotApplicable,
            );
            return Ok(());
        }
        if matches!(
            self.code,
            DiagnosticCode::ALLOCATION_FAILED | DiagnosticCode::DIAGNOSTIC_LIMIT_REACHED
        ) {
            self.statuses.set(
                DiagnosticStatusField::Path,
                DiagnosticFieldStatus::Unavailable,
            );
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
        self.statuses.set(
            DiagnosticStatusField::Path,
            DiagnosticFieldStatus::Available,
        );
        Ok(())
    }

    pub fn set_path_status(&mut self, status: DiagnosticFieldStatus) {
        if status != DiagnosticFieldStatus::Available {
            if let DiagnosticStorage::Complete(details) = &mut self.storage {
                details.path = None;
            }
        }
        let status = if status == DiagnosticFieldStatus::Available && self.path().is_none() {
            DiagnosticFieldStatus::Unavailable
        } else {
            status
        };
        self.statuses.set(DiagnosticStatusField::Path, status);
    }

    pub fn set_value_source(&mut self, value_source: Option<ValueSource>) {
        self.statuses.set(
            DiagnosticStatusField::ValueSource,
            status_for_presence(value_source.is_some()),
        );
        match &mut self.storage {
            DiagnosticStorage::Emergency {
                value_source: stored,
                ..
            } => *stored = value_source,
            DiagnosticStorage::Complete(details) => details.value_source = value_source,
        }
    }

    pub fn set_value_source_status(&mut self, status: DiagnosticFieldStatus) {
        if status != DiagnosticFieldStatus::Available {
            self.set_value_source(None);
        }
        let status = if status == DiagnosticFieldStatus::Available && self.value_source().is_none()
        {
            DiagnosticFieldStatus::Unavailable
        } else {
            status
        };
        self.statuses
            .set(DiagnosticStatusField::ValueSource, status);
    }
}

fn same_budget(left: Option<&ExecutionBudget>, right: Option<&ExecutionBudget>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left.same_call(right),
        (None, None) => true,
        _ => false,
    }
}

const fn status_for_presence(present: bool) -> DiagnosticFieldStatus {
    if present {
        DiagnosticFieldStatus::Available
    } else {
        DiagnosticFieldStatus::NotApplicable
    }
}

impl fmt::Debug for Diagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Diagnostic")
            .field("code", &self.code)
            .field("kind", &self.kind().id())
            .field("parameter_count", &self.parameters().len())
            .field("parameter_status", &self.parameters_status())
            .field("has_primary", &self.primary().is_some())
            .field("primary_status", &self.primary_status())
            .field("primary_location_status", &self.primary_location_status())
            .field("context", &self.context())
            .field("label_count", &self.labels().len())
            .field("related_locations_status", &self.related_locations_status())
            .field(
                "related_locations_truncated",
                &self.related_locations_truncated(),
            )
            .field("has_path", &self.path().is_some())
            .field("path_status", &self.path_status())
            .field("value_source", &self.value_source())
            .field("value_source_status", &self.value_source_status())
            .finish()
    }
}

impl From<BudgetError> for Diagnostic {
    fn from(error: BudgetError) -> Self {
        let mut diagnostic =
            Self::failure(error.diagnostic_code(), DiagnosticFailure::Budget(error));
        diagnostic.set_context(error.context());
        diagnostic
    }
}

impl From<SourceReadError> for Diagnostic {
    fn from(error: SourceReadError) -> Self {
        let code = error.diagnostic_code();
        let context = error.context();
        let source_id = error.source_id();
        let offset = error.offset();
        let mut diagnostic = Self::failure(code, DiagnosticFailure::SourceRead(error));
        diagnostic.set_context(context);
        if let (Some(source_id), Some(offset)) = (source_id, offset) {
            diagnostic.set_primary(Some(SourceSpan::new(source_id, Span::point(offset))));
        }
        diagnostic
    }
}
