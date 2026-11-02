mod conversion;
mod failure;

use crate::{
    binding_impl::{BindingErrors, BindingMismatch, BindingShape},
    session::SessionBudget,
};
use core::{error::Error, fmt, slice::from_ref};
use opeo::{ErrSlot, OResult, Out, ResultOutExt};
use typol_config::{
    ConfigDiagnosticRedactor, ConfigFailureStage, ConfigParseErrors, ConfigValueErrors,
};
use typol_core::{
    BudgetError, ChargedString, ChargedVec, Diagnostic, DiagnosticCode, DiagnosticContext,
    SourceExcerptSet, SourceId, SourceSpan, ValueError,
};

use typol_report::{DiagnosticReport, ReportContext};
use typol_semantic::{CompiledSchema, SchemaAccessError, SpanLocation};

use conversion::ConversionDiagnostic;
pub use conversion::ConversionErrorKind;
pub(crate) use conversion::{ConversionDiagnosticInput, ConversionPath, ConversionPathSegment};
pub use failure::{BINDING_MISMATCH_CONTEXT, CodecStage, Failure};

#[cfg(all(test, feature = "std"))]
use typol_core::{ExecutionBudget, diagnostic_context};

static UNAVAILABLE_EXCERPTS: SourceExcerptSet = SourceExcerptSet::unavailable();
#[cfg(all(test, feature = "std"))]
static ERROR_BUFFER_CONTEXT: DiagnosticContext =
    diagnostic_context!("typol", "diagnostics", "empty-error-buffer");

fn shape_detail_len(shape: BindingShape) -> usize {
    match shape {
        BindingShape::Array {
            length: Some(length),
        } => 13 + decimal_digits(length),
        _ => 0,
    }
}

fn decimal_digits(mut value: u32) -> usize {
    let mut digits = 1;
    while value >= 10 {
        value /= 10;
        digits += 1;
    }
    digits
}

fn append_shape<'slot>(
    message: &mut ChargedString,
    shape: BindingShape,
    mut out: Out<'slot, '_, BudgetError>,
) -> OResult<'slot, ()> {
    message.try_push_str(shape.message(), out.reborrow())?;
    if let BindingShape::Array {
        length: Some(length),
    } = shape
    {
        message.try_push_str(" (length ", out.reborrow())?;
        append_decimal(message, length, out.reborrow())?;
        message.try_push_str(")", out.reborrow())?;
    }
    Ok(())
}

fn append_decimal<'slot>(
    message: &mut ChargedString,
    mut value: u32,
    mut out: Out<'slot, '_, BudgetError>,
) -> OResult<'slot, ()> {
    let mut digits = [0u8; 10];
    let mut length = 0;
    loop {
        let digit = u8::try_from(value % 10)
            .ok()
            .ok_or(BudgetError::CapacityExceeded)
            .store_err(out.reborrow())?;
        if let Some(slot) = digits.get_mut(length) {
            *slot = b'0' + digit;
        } else {
            return Err(out.fail(BudgetError::CapacityExceeded));
        }
        length += 1;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    for digit in digits.iter().take(length).rev() {
        message.try_push_char(char::from(*digit), out.reborrow())?;
    }
    Ok(())
}

enum ErrorStorage {
    Parsing(ConfigParseErrors),
    Validation(ConfigValueErrors),
    Binding {
        errors: BindingErrors,
        diagnostics: ChargedVec<Diagnostic>,
    },
    Conversion(ConversionDiagnostic),
    Emergency {
        stage: CodecStage,
        diagnostic: Diagnostic,
    },
    #[cfg(all(test, feature = "std"))]
    Collected {
        stage: CodecStage,
        diagnostics: ChargedVec<Diagnostic>,
    },
}

pub struct CodecErrors {
    storage: ErrorStorage,
}

impl CodecErrors {
    pub(crate) fn parsing(errors: ConfigParseErrors) -> Self {
        Self {
            storage: ErrorStorage::Parsing(errors),
        }
    }

    pub(crate) fn validation(errors: ConfigValueErrors) -> Self {
        Self {
            storage: ErrorStorage::Validation(errors),
        }
    }

    pub fn config_parse_errors(&self) -> Option<&ConfigParseErrors> {
        match &self.storage {
            ErrorStorage::Parsing(errors) => Some(errors),
            _ => None,
        }
    }

    pub fn config_value_errors(&self) -> Option<&ConfigValueErrors> {
        match &self.storage {
            ErrorStorage::Validation(errors) => Some(errors),
            _ => None,
        }
    }

    pub fn binding_errors(&self) -> Option<&BindingErrors> {
        match &self.storage {
            ErrorStorage::Binding { errors, .. } => Some(errors),
            _ => None,
        }
    }

    pub fn conversion_error_kind(&self) -> Option<ConversionErrorKind> {
        match &self.storage {
            ErrorStorage::Conversion(error) => Some(error.kind()),
            _ => None,
        }
    }

    pub fn excerpts(&self) -> &SourceExcerptSet {
        match &self.storage {
            ErrorStorage::Parsing(errors) => errors.excerpts(),
            ErrorStorage::Validation(errors) => errors.excerpts(),
            ErrorStorage::Binding { .. }
            | ErrorStorage::Conversion(_)
            | ErrorStorage::Emergency { .. } => &UNAVAILABLE_EXCERPTS,
            #[cfg(all(test, feature = "std"))]
            ErrorStorage::Collected { .. } => &UNAVAILABLE_EXCERPTS,
        }
    }

    pub fn config_source_id(&self) -> Option<SourceId> {
        self.config_parse_errors()
            .and_then(|errors| errors.config_source_id())
            .or_else(|| {
                self.config_value_errors()
                    .and_then(|errors| errors.config_source_id())
            })
    }

    pub fn config_source_name(&self) -> Option<&str> {
        self.config_parse_errors()
            .and_then(|errors| errors.config_source_name())
            .or_else(|| {
                self.config_value_errors()
                    .and_then(|errors| errors.config_source_name())
            })
    }

    pub fn config_failure_stage(&self) -> Option<ConfigFailureStage> {
        self.config_parse_errors().map(ConfigParseErrors::stage)
    }

    pub fn span_location(&self, span: SourceSpan) -> Option<SpanLocation> {
        match &self.storage {
            ErrorStorage::Parsing(errors) => errors.span_location(span),
            ErrorStorage::Validation(errors) => errors.span_location(span),
            _ => None,
        }
    }

    pub fn diagnostic_context(&self, index: usize) -> Option<DiagnosticContext> {
        self.diagnostics().get(index)?.context()
    }

    pub(crate) fn emergency(stage: CodecStage, failure: Failure) -> Self {
        Self {
            storage: ErrorStorage::Emergency {
                stage,
                diagnostic: failure.emergency_diagnostic(),
            },
        }
    }

    pub(crate) fn conversion<'slot>(
        input: ConversionDiagnosticInput<'_>,
        schema: &CompiledSchema,
        session: &SessionBudget,
        mut out: Out<'slot, '_, Self>,
    ) -> OResult<'slot, Self> {
        let mut error = ConversionDiagnostic::new(input, session, out.reborrow())?;
        let mut redaction_errors = ErrSlot::new();
        match redaction_errors.try_call(|redact_out| {
            ConfigDiagnosticRedactor::new(schema)
                .redact_conversion(error.diagnostic_mut(), redact_out)
        }) {
            Ok(()) => Ok(Self {
                storage: ErrorStorage::Conversion(error),
            }),
            Err(caught) => Ok(Self::safe_diagnostic(CodecStage::Conversion, caught.take())),
        }
    }

    pub(crate) fn safe_diagnostic(stage: CodecStage, diagnostic: Diagnostic) -> Self {
        Self {
            storage: ErrorStorage::Emergency { stage, diagnostic },
        }
    }

    pub(crate) fn binding_mismatches<'slot>(
        errors: BindingErrors,
        diagnostic_limit: usize,
        session: &SessionBudget,
        mut out: Out<'slot, '_, Self>,
    ) -> OResult<'slot, Self> {
        let mut diagnostics = session.budget_codec(
            CodecStage::Binding,
            |budget_out| ChargedVec::new(Some(session.execution()), budget_out),
            out.reborrow(),
        )?;
        for mismatch in errors.as_slice() {
            let diagnostic = Self::binding_mismatch_diagnostic(mismatch, session, out.reborrow())?;
            session.budget_codec(
                CodecStage::Binding,
                |budget_out| diagnostics.try_push(diagnostic, budget_out),
                out.reborrow(),
            )?;
        }
        if errors.is_truncated() && diagnostic_limit.max(1) > 1 {
            let diagnostic = Diagnostic::emergency(DiagnosticCode::DIAGNOSTIC_LIMIT_REACHED);
            session.budget_codec(
                CodecStage::Binding,
                |budget_out| diagnostics.try_push(diagnostic, budget_out),
                out.reborrow(),
            )?;
        }
        Ok(Self {
            storage: ErrorStorage::Binding {
                errors,
                diagnostics,
            },
        })
    }

    fn binding_mismatch_diagnostic<'slot>(
        mismatch: &BindingMismatch,
        session: &SessionBudget,
        mut out: Out<'slot, '_, Self>,
    ) -> OResult<'slot, Diagnostic> {
        let message = session.budget_codec(
            CodecStage::Binding,
            |mut budget_out| {
                let pieces = [
                    "Rust type binding failed: ",
                    mismatch.kind().message(),
                    "; schema path: ",
                    mismatch.schema_path(),
                    "; Rust path: ",
                    mismatch.rust_path(),
                    "; schema expects: ",
                ];
                let base_capacity = pieces
                    .iter()
                    .try_fold(0usize, |total, piece| total.checked_add(piece.len()))
                    .ok_or(BudgetError::CapacityExceeded)
                    .store_err(budget_out.reborrow())?;
                let expected = mismatch.expected();
                let actual = mismatch.actual();
                let actual_label = "; Rust actual: ";
                let capacity = base_capacity
                    .checked_add(actual_label.len())
                    .and_then(|total| total.checked_add(expected.message().len()))
                    .and_then(|total| total.checked_add(shape_detail_len(expected)))
                    .and_then(|total| total.checked_add(actual.message().len()))
                    .and_then(|total| total.checked_add(shape_detail_len(actual)))
                    .ok_or(BudgetError::CapacityExceeded)
                    .store_err(budget_out.reborrow())?;
                session.execution().construct_text(
                    capacity,
                    session.max_text_bytes(),
                    budget_out.reborrow(),
                )?;
                session
                    .execution()
                    .consume(capacity, budget_out.reborrow())?;
                let mut message = ChargedString::with_capacity(
                    capacity,
                    Some(session.execution()),
                    budget_out.reborrow(),
                )?;
                for piece in pieces {
                    message.try_push_str(piece, budget_out.reborrow())?;
                }
                append_shape(&mut message, expected, budget_out.reborrow())?;
                message.try_push_str(actual_label, budget_out.reborrow())?;
                append_shape(&mut message, actual, budget_out.reborrow())?;
                Ok(message)
            },
            out.reborrow(),
        )?;
        let diagnostic = session.budget_codec(
            CodecStage::Binding,
            |mut budget_out| {
                let mut diagnostic = Diagnostic::try_new(
                    DiagnosticCode::TYPE_BINDING_ERROR,
                    message.as_str(),
                    Some(session.execution()),
                    budget_out.reborrow(),
                )?;
                diagnostic.set_context(Some(BINDING_MISMATCH_CONTEXT));
                Ok(diagnostic)
            },
            out.reborrow(),
        )?;
        Ok(diagnostic)
    }

    pub const fn stage(&self) -> CodecStage {
        match &self.storage {
            ErrorStorage::Parsing(_) => CodecStage::Parsing,
            ErrorStorage::Validation(_) => CodecStage::Validation,
            ErrorStorage::Binding { .. } => CodecStage::Binding,
            ErrorStorage::Conversion(_) => CodecStage::Conversion,
            ErrorStorage::Emergency { stage, .. } => *stage,
            #[cfg(all(test, feature = "std"))]
            ErrorStorage::Collected { stage, .. } => *stage,
        }
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        match &self.storage {
            ErrorStorage::Parsing(errors) => errors.diagnostics(),
            ErrorStorage::Validation(errors) => errors.diagnostics(),
            ErrorStorage::Binding { diagnostics, .. } => diagnostics.as_slice(),
            ErrorStorage::Conversion(error) => from_ref(error.diagnostic()),
            ErrorStorage::Emergency { diagnostic, .. } => from_ref(diagnostic),
            #[cfg(all(test, feature = "std"))]
            ErrorStorage::Collected { diagnostics, .. } => diagnostics.as_slice(),
        }
    }
}

impl fmt::Debug for CodecErrors {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CodecErrors")
            .field("stage", &self.stage())
            .field("diagnostic_count", &self.diagnostics().len())
            .field(
                "binding_mismatch_count",
                &self.binding_errors().map_or(0, BindingErrors::len),
            )
            .finish()
    }
}

impl fmt::Display for CodecErrors {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let source_name = |source_id| {
            (self.config_source_id() == Some(source_id))
                .then(|| self.config_source_name())
                .flatten()
        };
        let span_location = |span| self.span_location(span);
        let stage = match self.stage() {
            CodecStage::Binding => "type binding",
            CodecStage::Parsing => "parsing",
            CodecStage::Conversion => "conversion",
            CodecStage::Validation => "validation",
            CodecStage::Encoding => "encoding",
        };
        DiagnosticReport::new(
            "codec operation failed",
            Some(stage),
            self.diagnostics(),
            ReportContext::new(&source_name, &span_location, self.excerpts()),
        )
        .write_to(formatter)
    }
}

#[cfg(all(test, feature = "std"))]
pub(crate) struct ErrorBuffer {
    stage: CodecStage,
    budget: ExecutionBudget,
    diagnostics: ChargedVec<Diagnostic>,
    terminal: Option<Diagnostic>,
    limit: usize,
    max_text_bytes: usize,
    budget_errors: ErrSlot<BudgetError>,
}

#[cfg(all(test, feature = "std"))]
impl ErrorBuffer {
    pub(crate) fn new<'slot>(
        stage: CodecStage,
        limit: usize,
        session: &SessionBudget,
        mut out: Out<'slot, '_, BudgetError>,
    ) -> OResult<'slot, Self> {
        let budget = session.execution();
        Ok(Self {
            stage,
            budget: budget.try_clone(out.reborrow())?,
            diagnostics: ChargedVec::new(Some(budget), out.reborrow())?,
            terminal: None,
            limit: limit.max(1),
            max_text_bytes: session.max_text_bytes(),
            budget_errors: ErrSlot::new(),
        })
    }

    pub(crate) fn push(&mut self, failure: Failure) -> bool {
        if self.terminal.is_some() {
            return false;
        }
        if failure.terminal() {
            self.terminal = Some(failure.emergency_diagnostic());
            return false;
        }
        let budget = &self.budget;
        let max_text_bytes = self.max_text_bytes;
        let result = self.budget_errors.try_call(|mut out| {
            budget.consume(1, out.reborrow())?;
            if self.diagnostics.len() >= self.limit - 1 && !failure.terminal() {
                self.terminal = Some(Diagnostic::emergency(
                    DiagnosticCode::DIAGNOSTIC_LIMIT_REACHED,
                ));
                return Ok(());
            }
            self.diagnostics.try_reserve(2, out.reborrow())?;
            let diagnostic = failure.diagnostic(budget, max_text_bytes, out.reborrow())?;
            if failure.terminal() {
                self.terminal = Some(diagnostic);
            } else {
                self.diagnostics.try_push(diagnostic, out.reborrow())?;
            }
            Ok(())
        });
        if let Err(caught) = result {
            self.terminal = Some(Failure::from(caught.take()).emergency_diagnostic());
        }
        self.terminal.is_none()
    }

    pub(crate) fn finish(mut self) -> CodecErrors {
        if self
            .terminal
            .as_ref()
            .is_some_and(|error| error.code() == DiagnosticCode::ALLOCATION_FAILED)
        {
            return CodecErrors::emergency(self.stage, Failure::Allocation);
        }
        let count = self.diagnostics.len() + 1;
        let budget = &self.budget;
        if let Err(caught) = self
            .budget_errors
            .try_call(|out| budget.consume(count, out))
        {
            return CodecErrors::emergency(self.stage, caught.take().into());
        }
        if self.diagnostics.is_empty() {
            return CodecErrors {
                storage: ErrorStorage::Emergency {
                    stage: self.stage,
                    diagnostic: self.terminal.unwrap_or_else(|| {
                        Failure::internal(ERROR_BUFFER_CONTEXT).emergency_diagnostic()
                    }),
                },
            };
        }
        if let Some(terminal) = self.terminal.take() {
            // The terminal slot is reserved as regular slots grow, so no allocation is needed after termination.
            if let Err(caught) = self
                .budget_errors
                .try_call(|out| self.diagnostics.try_push(terminal, out))
            {
                return CodecErrors::emergency(self.stage, caught.take().into());
            }
        }
        CodecErrors {
            storage: ErrorStorage::Collected {
                stage: self.stage,
                diagnostics: self.diagnostics,
            },
        }
    }
}

impl Error for CodecErrors {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match &self.storage {
            ErrorStorage::Parsing(errors) => Some(errors),
            ErrorStorage::Validation(errors) => Some(errors),
            ErrorStorage::Binding { .. }
            | ErrorStorage::Conversion(_)
            | ErrorStorage::Emergency { .. } => None,
            #[cfg(all(test, feature = "std"))]
            ErrorStorage::Collected { .. } => None,
        }
    }
}

impl From<BudgetError> for CodecErrors {
    fn from(error: BudgetError) -> Self {
        Self::emergency(CodecStage::Binding, error.into())
    }
}

impl From<Failure> for CodecErrors {
    fn from(error: Failure) -> Self {
        Self::emergency(CodecStage::Encoding, error)
    }
}

impl From<SchemaAccessError> for CodecErrors {
    fn from(error: SchemaAccessError) -> Self {
        Self::emergency(CodecStage::Binding, Failure::schema_access(error))
    }
}

impl From<ValueError> for CodecErrors {
    fn from(error: ValueError) -> Self {
        Self::emergency(CodecStage::Conversion, Failure::value_error(error))
    }
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::{CodecErrors, CodecStage, Failure};
    use crate::test_support::{TestOpeo, TestValues};
    use typol_core::{BudgetError, DiagnosticCode, DiagnosticContext, diagnostic_context};
    use typol_test_support::{TestExpect, TestResult, test_verify_eq as assert_eq};

    static FAILURE_CONTEXT: DiagnosticContext =
        diagnostic_context!("typol-test", "encoding", "test-invariant");
    static BUDGET_CONTEXT: DiagnosticContext =
        diagnostic_context!("typol-core", "budget", "test-budget-invariant");

    #[test]
    fn emergency_internal_error_exposes_its_static_failure_point() -> TestResult {
        let errors =
            CodecErrors::emergency(CodecStage::Encoding, Failure::internal(FAILURE_CONTEXT));
        let diagnostic = errors
            .diagnostics()
            .first()
            .test_expect("should preserve emergency diagnostic")?;
        let context = errors
            .diagnostic_context(0)
            .test_expect("should preserve failure context")?;

        assert_eq!(diagnostic.code(), DiagnosticCode::INTERNAL_ERROR);
        assert_eq!(context.subsystem(), "typol-test");
        assert_eq!(context.stage(), "encoding");
        assert_eq!(context.failure_point(), "test-invariant");
        assert_eq!(diagnostic.primary(), None);
        assert_eq!(errors.diagnostic_context(1), None);
        Ok(())
    }

    #[test]
    fn budget_invariant_context_survives_codec_conversion() -> TestResult {
        let errors = CodecErrors::emergency(
            CodecStage::Binding,
            BudgetError::invariant(&BUDGET_CONTEXT).into(),
        );
        let context = errors
            .diagnostic_context(0)
            .test_expect("budget invariant context should be preserved through codec errors")?;

        assert_eq!(
            errors
                .diagnostics()
                .first()
                .map(|diagnostic| diagnostic.code()),
            Some(DiagnosticCode::INTERNAL_ERROR)
        );
        assert_eq!(context.subsystem(), "typol-core");
        assert_eq!(context.stage(), "budget");
        assert_eq!(context.failure_point(), "test-budget-invariant");
        Ok(())
    }

    #[test]
    fn duplicate_value_index_survives_conversion_failure_mapping() -> TestResult {
        let error = TestOpeo::error(|out| {
            TestValues::try_map(
                vec![
                    TestValues::map_entry("same".into(), TestValues::unsigned(1)),
                    TestValues::map_entry("same".into(), TestValues::unsigned(2)),
                ],
                out,
            )
        })?;
        let errors = CodecErrors::emergency(CodecStage::Conversion, error.into());
        let context = errors
            .diagnostic_context(0)
            .test_expect("duplicate-key conversion errors should preserve context")?;

        assert_eq!(
            errors
                .diagnostics()
                .first()
                .map(|diagnostic| diagnostic.code()),
            Some(DiagnosticCode::RUST_CONVERSION_ERROR)
        );
        assert_eq!(context.subsystem(), "typol-core");
        assert_eq!(context.stage(), "value-construction");
        assert_eq!(context.failure_point(), "duplicate-map-key");
        assert_eq!(context.object_id(), Some(1));
        Ok(())
    }
}
