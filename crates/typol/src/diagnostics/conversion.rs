use super::{CodecErrors, SessionBudget};
use crate::diagnostics::CodecStage;
use opeo::{OResult, Out};
use typol_core::{
    BudgetError, ChargedString, Diagnostic, DiagnosticCode, DiagnosticContext, InstancePath,
    PathSegment, diagnostic_context,
};

const MAX_CONVERSION_PATH_SEGMENTS: usize = 16;

static REJECTED_CONTEXT: DiagnosticContext =
    diagnostic_context!("typol", "conversion", "conversion-rejected");
static SHAPE_MISMATCH_CONTEXT: DiagnosticContext =
    diagnostic_context!("typol", "conversion", "value-shape-mismatch");
static RANGE_CONTEXT: DiagnosticContext =
    diagnostic_context!("typol", "conversion", "value-out-of-range");
static INVALID_SCALAR_CONTEXT: DiagnosticContext =
    diagnostic_context!("typol", "conversion", "invalid-scalar-value");
static ARRAY_LENGTH_CONTEXT: DiagnosticContext =
    diagnostic_context!("typol", "conversion", "array-length-mismatch");
static FIELD_COUNT_CONTEXT: DiagnosticContext =
    diagnostic_context!("typol", "conversion", "structure-field-count-mismatch");
static MISSING_FIELD_CONTEXT: DiagnosticContext =
    diagnostic_context!("typol", "conversion", "required-field-missing");
static MISSING_VARIANT_CONTEXT: DiagnosticContext =
    diagnostic_context!("typol", "conversion", "enum-variant-missing");
static ENUM_PAYLOAD_CONTEXT: DiagnosticContext =
    diagnostic_context!("typol", "conversion", "enum-payload-mismatch");
static BINDING_INVARIANT_CONTEXT: DiagnosticContext =
    diagnostic_context!("typol", "conversion", "binding-invariant-failed");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConversionErrorKind {
    Rejected,
    ValueShapeMismatch,
    ValueOutOfRange,
    InvalidScalar,
    ArrayLengthMismatch,
    FieldCountMismatch,
    MissingField,
    MissingVariant,
    EnumPayloadMismatch,
    InternalBindingInvariant,
}

impl ConversionErrorKind {
    const fn message(self) -> &'static str {
        match self {
            Self::Rejected => "manual conversion rejected the value",
            Self::ValueShapeMismatch => "value shape mismatch",
            Self::ValueOutOfRange => "value is outside the target type range",
            Self::InvalidScalar => "scalar value is invalid",
            Self::ArrayLengthMismatch => "array length mismatch",
            Self::FieldCountMismatch => "struct field count mismatch",
            Self::MissingField => "required field is missing",
            Self::MissingVariant => "enum variant is unmatched",
            Self::EnumPayloadMismatch => "enum payload shape mismatch",
            Self::InternalBindingInvariant => "binding invariant failed",
        }
    }

    const fn code(self) -> DiagnosticCode {
        match self {
            Self::InternalBindingInvariant => DiagnosticCode::INTERNAL_ERROR,
            _ => DiagnosticCode::RUST_CONVERSION_ERROR,
        }
    }

    const fn context(self) -> DiagnosticContext {
        match self {
            Self::Rejected => REJECTED_CONTEXT,
            Self::ValueShapeMismatch => SHAPE_MISMATCH_CONTEXT,
            Self::ValueOutOfRange => RANGE_CONTEXT,
            Self::InvalidScalar => INVALID_SCALAR_CONTEXT,
            Self::ArrayLengthMismatch => ARRAY_LENGTH_CONTEXT,
            Self::FieldCountMismatch => FIELD_COUNT_CONTEXT,
            Self::MissingField => MISSING_FIELD_CONTEXT,
            Self::MissingVariant => MISSING_VARIANT_CONTEXT,
            Self::EnumPayloadMismatch => ENUM_PAYLOAD_CONTEXT,
            Self::InternalBindingInvariant => BINDING_INVARIANT_CONTEXT,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum ConversionPathSegment<'a> {
    Field(&'a str),
    Index(usize),
    Variant(&'a str),
}

#[derive(Clone, Copy)]
pub(crate) struct ConversionPath<'a> {
    segments: [Option<ConversionPathSegment<'a>>; MAX_CONVERSION_PATH_SEGMENTS],
    length: usize,
    truncated: bool,
}

impl<'a> ConversionPath<'a> {
    pub(crate) const fn root() -> Self {
        Self {
            segments: [None; MAX_CONVERSION_PATH_SEGMENTS],
            length: 0,
            truncated: false,
        }
    }

    pub(crate) fn pushed(mut self, segment: ConversionPathSegment<'a>) -> Self {
        if let Some(slot) = self.segments.get_mut(self.length) {
            *slot = Some(segment);
            self.length += 1;
        } else {
            self.truncated = true;
        }
        self
    }

    fn iter(&self) -> impl Iterator<Item = ConversionPathSegment<'a>> + '_ {
        self.segments
            .iter()
            .take(self.length)
            .filter_map(|segment| *segment)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ConversionDiagnosticInput<'a> {
    kind: ConversionErrorKind,
    expected: &'static str,
    actual: &'static str,
    path: ConversionPath<'a>,
}

impl<'a> ConversionDiagnosticInput<'a> {
    pub(crate) const fn new(
        kind: ConversionErrorKind,
        expected: &'static str,
        actual: &'static str,
        path: ConversionPath<'a>,
    ) -> Self {
        Self {
            kind,
            expected,
            actual,
            path,
        }
    }
}

pub(super) struct ConversionDiagnostic {
    kind: ConversionErrorKind,
    diagnostic: Diagnostic,
}

impl ConversionDiagnostic {
    pub(super) fn new<'slot>(
        input: ConversionDiagnosticInput<'_>,
        session: &SessionBudget,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, Self> {
        let message = session.budget_codec(
            CodecStage::Conversion,
            |budget_out| Self::message(input, session, budget_out),
            out.reborrow(),
        )?;
        let diagnostic = session.budget_codec(
            CodecStage::Conversion,
            |mut budget_out| {
                let mut diagnostic = Diagnostic::try_new(
                    input.kind.code(),
                    message.as_str(),
                    Some(session.execution()),
                    budget_out.reborrow(),
                )?;
                diagnostic.set_context(Some(input.kind.context()));
                let mut path = InstancePath::new(Some(session.execution()), budget_out.reborrow())?;
                for segment in input.path.iter() {
                    match segment {
                        ConversionPathSegment::Field(name) => {
                            path.push(
                                PathSegment::try_field(
                                    name,
                                    Some(session.execution()),
                                    budget_out.reborrow(),
                                )?,
                                budget_out.reborrow(),
                            )?;
                        }
                        ConversionPathSegment::Index(index) => {
                            path.push_index(index, budget_out.reborrow())?;
                        }
                        ConversionPathSegment::Variant(name) => {
                            let segment = PathSegment::try_enum_payload(
                                name,
                                Some(session.execution()),
                                budget_out.reborrow(),
                            )?;
                            path.push(segment, budget_out.reborrow())?;
                        }
                    }
                }
                diagnostic.try_set_path(
                    Some(path),
                    Some(session.execution()),
                    budget_out.reborrow(),
                )?;
                Ok(diagnostic)
            },
            out.reborrow(),
        )?;
        Ok(Self {
            kind: input.kind,
            diagnostic,
        })
    }

    fn message<'slot>(
        input: ConversionDiagnosticInput<'_>,
        session: &SessionBudget,
        mut out: Out<'slot, '_, BudgetError>,
    ) -> OResult<'slot, ChargedString> {
        let prefix = "Rust value conversion failed (";
        let expected_label = "); expected: ";
        let actual_label = "; actual: ";
        let truncated_label = "; diagnostic path was truncated";
        let suffix = if input.path.truncated {
            truncated_label
        } else {
            ""
        };
        let Some(capacity) = [
            prefix.len(),
            input.kind.message().len(),
            expected_label.len(),
            input.expected.len(),
            actual_label.len(),
            input.actual.len(),
            suffix.len(),
        ]
        .into_iter()
        .try_fold(0usize, usize::checked_add) else {
            return Err(out.fail(BudgetError::CapacityExceeded));
        };
        session
            .execution()
            .construct_text(capacity, session.max_text_bytes(), out.reborrow())?;
        session.execution().consume(capacity, out.reborrow())?;
        let mut message =
            ChargedString::with_capacity(capacity, Some(session.execution()), out.reborrow())?;
        message.try_push_str(prefix, out.reborrow())?;
        message.try_push_str(input.kind.message(), out.reborrow())?;
        message.try_push_str(expected_label, out.reborrow())?;
        message.try_push_str(input.expected, out.reborrow())?;
        message.try_push_str(actual_label, out.reborrow())?;
        message.try_push_str(input.actual, out.reborrow())?;
        message.try_push_str(suffix, out.reborrow())?;
        Ok(message)
    }

    pub(super) const fn kind(&self) -> ConversionErrorKind {
        self.kind
    }

    pub(super) const fn diagnostic(&self) -> &Diagnostic {
        &self.diagnostic
    }

    pub(super) fn diagnostic_mut(&mut self) -> &mut Diagnostic {
        &mut self.diagnostic
    }
}
