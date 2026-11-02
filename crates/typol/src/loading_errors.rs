use crate::{
    diagnostics::CodecErrors,
    error::{TypolError, TypolStage},
};
use alloc::string::String;
use core::{error::Error, fmt, slice::from_ref};
use opeo::{ErrSlot, OResult, Out};
use typol_config::{ConfigFailureStage, ConfigParseErrors};
use typol_core::{
    BudgetError, Diagnostic, DiagnosticCode, DiagnosticContext as ErrorContext, ExecutionBudget,
    FilledTerminalSlot, SourceExcerptSet, SourceId, SourceSpan, TerminalSlot, diagnostic_context,
};
use typol_i18n::{Language, Translator};
use typol_schema::{DiagnosticList, SchemaCompileErrors};
use typol_semantic::SpanLocation;
use typol_std::{
    DiagnosticContext, DiagnosticRenderError, DiagnosticRenderer, DiagnosticReport,
    DiagnosticReportStage, DiagnosticReportTitle, FileSchemaCompileErrors,
    FileSchemaCompileFailure, HostError, HostErrorKind, RenderOptions, SourceNames,
};

static ERROR_PAYLOAD_STORAGE_CONTEXT: ErrorContext =
    diagnostic_context!("typol", "error-storage", "error-payload-capacity-exceeded");
static UNAVAILABLE_ERROR_EXCERPTS: SourceExcerptSet = SourceExcerptSet::unavailable();

enum Failure {
    Schema(SchemaCompileErrors),
    Host(HostError),
    Config(ConfigParseErrors),
    Codec(CodecErrors),
}

struct ErrorPayload {
    failure: Failure,
    names: Option<SourceNames>,
    names_failure: Option<HostErrorKind>,
}

enum ErrorStorage {
    File(FileSchemaCompileErrors),
    Retained {
        slot: FilledTerminalSlot<ErrorPayload>,
        stage: TypolStage,
    },
    InlineSchema(SchemaCompileErrors),
    InlineHost {
        error: HostError,
        stage: TypolStage,
    },
    PayloadStorageFailure {
        diagnostic: Diagnostic,
        stage: TypolStage,
    },
}

#[derive(Clone, Copy)]
pub struct TypolDiagnosticList<'a> {
    values: &'a [Diagnostic],
    schema: Option<DiagnosticList<'a>>,
}

pub struct TypolDiagnosticIter<'a> {
    values: TypolDiagnosticList<'a>,
    index: usize,
}

impl<'a> TypolDiagnosticList<'a> {
    fn from_slice(values: &'a [Diagnostic]) -> Self {
        Self {
            values,
            schema: None,
        }
    }

    fn from_schema(schema: DiagnosticList<'a>) -> Self {
        Self {
            values: &[],
            schema: Some(schema),
        }
    }

    pub fn len(self) -> usize {
        self.schema.map_or(self.values.len(), |schema| schema.len())
    }

    pub fn is_empty(self) -> bool {
        self.len() == 0
    }

    pub fn get(self, index: usize) -> Option<&'a Diagnostic> {
        self.schema
            .map_or_else(|| self.values.get(index), |schema| schema.get(index))
    }

    pub fn first(self) -> Option<&'a Diagnostic> {
        self.get(0)
    }

    pub fn last(self) -> Option<&'a Diagnostic> {
        self.len().checked_sub(1).and_then(|index| self.get(index))
    }

    pub fn iter(self) -> TypolDiagnosticIter<'a> {
        TypolDiagnosticIter {
            values: self,
            index: 0,
        }
    }
}

impl<'a> Iterator for TypolDiagnosticIter<'a> {
    type Item = &'a Diagnostic;

    fn next(&mut self) -> Option<Self::Item> {
        let diagnostic = self.values.get(self.index)?;
        self.index += 1;
        Some(diagnostic)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.values.len().saturating_sub(self.index);
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for TypolDiagnosticIter<'_> {}

pub(crate) struct ErrorSlot {
    slot: TerminalSlot<ErrorPayload>,
}

pub(crate) struct ErrorNames {
    names: Option<SourceNames>,
    failure: Option<HostErrorKind>,
}

pub(crate) struct StdTypolError {
    storage: ErrorStorage,
}

impl ErrorNames {
    pub(crate) fn copied(names: SourceNames) -> Self {
        Self {
            names: Some(names),
            failure: None,
        }
    }

    pub(crate) fn failed(failure: HostErrorKind) -> Self {
        Self {
            names: None,
            failure: Some(failure),
        }
    }
}

impl ErrorSlot {
    pub(crate) fn in_budget<'slot>(
        budget: &ExecutionBudget,
        mut out: Out<'slot, '_, BudgetError>,
    ) -> OResult<'slot, Self> {
        let slot = TerminalSlot::new(budget, out.reborrow())?;
        Ok(Self { slot })
    }

    fn finish(self, failure: Failure, names: ErrorNames, stage: TypolStage) -> TypolError {
        self.finish_std(failure, names, stage).into()
    }

    fn finish_std(self, failure: Failure, names: ErrorNames, stage: TypolStage) -> StdTypolError {
        let slot = self.slot.store(ErrorPayload {
            failure,
            names: names.names,
            names_failure: names.failure,
        });
        StdTypolError::finish_stored_payload(slot, stage)
    }

    pub(crate) fn schema(self, errors: SchemaCompileErrors, names: SourceNames) -> TypolError {
        self.finish(
            Failure::Schema(errors),
            ErrorNames::copied(names),
            TypolStage::SchemaCompile,
        )
    }

    pub(crate) fn config(self, errors: ConfigParseErrors, names: ErrorNames) -> TypolError {
        let stage = match errors.stage() {
            ConfigFailureStage::Input => TypolStage::ConfigRead,
            ConfigFailureStage::Parsing => TypolStage::Decoding,
        };
        self.finish(Failure::Config(errors), names, stage)
    }

    pub(crate) fn codec(self, errors: CodecErrors, names: ErrorNames) -> TypolError {
        let stage = match errors.config_failure_stage() {
            Some(ConfigFailureStage::Input) => TypolStage::ConfigRead,
            Some(ConfigFailureStage::Parsing) | None => TypolStage::Decoding,
        };
        self.finish(Failure::Codec(errors), names, stage)
    }

    pub(crate) fn encoding(self, errors: CodecErrors, names: ErrorNames) -> TypolError {
        self.finish(Failure::Codec(errors), names, TypolStage::Encoding)
    }

    pub(crate) fn host(self, error: HostError, names: ErrorNames) -> TypolError {
        self.finish(Failure::Host(error), names, TypolStage::ConfigRead)
    }
}

fn failure_source(failure: &Failure) -> &(dyn Error + 'static) {
    match failure {
        Failure::Schema(errors) => errors,
        Failure::Host(error) => error,
        Failure::Config(errors) => errors,
        Failure::Codec(errors) => errors,
    }
}

impl StdTypolError {
    fn finish_stored_payload(
        slot: Option<FilledTerminalSlot<ErrorPayload>>,
        stage: TypolStage,
    ) -> Self {
        match slot {
            Some(slot) => Self {
                storage: ErrorStorage::Retained { slot, stage },
            },
            None => Self::payload_storage_failure(stage),
        }
    }

    #[cfg(test)]
    pub(crate) fn simulated_payload_storage_failure(stage: TypolStage) -> Self {
        Self::finish_stored_payload(None, stage)
    }

    pub(crate) fn file(errors: FileSchemaCompileErrors) -> Self {
        Self {
            storage: ErrorStorage::File(errors),
        }
    }

    pub(crate) fn schema_budget(error: BudgetError) -> Self {
        Self::schema_initialization(SchemaCompileErrors::budget_failure(error))
    }

    pub(crate) fn schema_initialization(errors: SchemaCompileErrors) -> Self {
        Self {
            storage: ErrorStorage::InlineSchema(errors),
        }
    }

    pub(crate) fn host(error: HostError, stage: TypolStage) -> Self {
        Self {
            storage: ErrorStorage::InlineHost { error, stage },
        }
    }

    pub(crate) fn binding(errors: CodecErrors, names: ErrorNames, slot: ErrorSlot) -> Self {
        slot.finish_std(Failure::Codec(errors), names, TypolStage::Binding)
    }

    fn payload_storage_failure(stage: TypolStage) -> Self {
        let mut diagnostic = Diagnostic::emergency(DiagnosticCode::CAPACITY_EXCEEDED);
        diagnostic.set_context(Some(ERROR_PAYLOAD_STORAGE_CONTEXT));
        Self {
            storage: ErrorStorage::PayloadStorageFailure { diagnostic, stage },
        }
    }

    fn error_payload(&self) -> Option<&ErrorPayload> {
        match &self.storage {
            ErrorStorage::Retained { slot, .. } => slot.get(),
            _ => None,
        }
    }

    fn failure(&self) -> Option<&Failure> {
        self.error_payload().map(|payload| &payload.failure)
    }

    pub fn stage(&self) -> TypolStage {
        match &self.storage {
            ErrorStorage::Retained { stage, .. }
            | ErrorStorage::InlineHost { stage, .. }
            | ErrorStorage::PayloadStorageFailure { stage, .. } => *stage,
            ErrorStorage::InlineSchema(_) => TypolStage::SchemaCompile,
            ErrorStorage::File(errors) => match errors.failure() {
                FileSchemaCompileFailure::Root(_) => TypolStage::SchemaRead,
                FileSchemaCompileFailure::Compilation(_) => TypolStage::SchemaCompile,
            },
        }
    }

    pub fn host_error(&self) -> Option<&HostError> {
        match &self.storage {
            ErrorStorage::InlineHost { error, .. } => Some(error),
            ErrorStorage::File(errors) => match errors.failure() {
                FileSchemaCompileFailure::Root(error) => Some(error),
                _ => None,
            },
            _ => match self.failure() {
                Some(Failure::Host(error)) => Some(error),
                _ => None,
            },
        }
    }

    pub fn schema_compile_errors(&self) -> Option<&SchemaCompileErrors> {
        match &self.storage {
            ErrorStorage::InlineSchema(errors) => Some(errors),
            ErrorStorage::File(errors) => match errors.failure() {
                FileSchemaCompileFailure::Compilation(errors) => Some(errors),
                _ => None,
            },
            _ => match self.failure() {
                Some(Failure::Schema(errors)) => Some(errors),
                _ => None,
            },
        }
    }

    pub fn config_parse_errors(&self) -> Option<&ConfigParseErrors> {
        match self.failure() {
            Some(Failure::Config(errors)) => Some(errors),
            Some(Failure::Codec(errors)) => errors.config_parse_errors(),
            _ => None,
        }
    }

    pub fn codec_errors(&self) -> Option<&CodecErrors> {
        match self.failure() {
            Some(Failure::Codec(errors)) => Some(errors),
            _ => None,
        }
    }

    pub fn diagnostics(&self) -> TypolDiagnosticList<'_> {
        if let ErrorStorage::PayloadStorageFailure { diagnostic, .. } = &self.storage {
            return TypolDiagnosticList::from_slice(from_ref(diagnostic));
        }
        if let Some(errors) = self.schema_compile_errors() {
            return TypolDiagnosticList::from_schema(errors.diagnostics());
        }
        if let Some(errors) = self.codec_errors() {
            return TypolDiagnosticList::from_slice(errors.diagnostics());
        }
        match self.failure() {
            Some(Failure::Config(errors)) => TypolDiagnosticList::from_slice(errors.diagnostics()),
            _ => TypolDiagnosticList::from_slice(&[]),
        }
    }

    pub fn diagnostic_context(&self, index: usize) -> Option<ErrorContext> {
        self.diagnostics().get(index)?.context()
    }

    pub fn span_location(&self, span: SourceSpan) -> Option<SpanLocation> {
        if let Some(errors) = self.schema_compile_errors() {
            return errors.span_location(span);
        }
        if let Some(errors) = self.codec_errors() {
            return errors.span_location(span);
        }
        match self.failure() {
            Some(Failure::Config(errors)) => errors.span_location(span),
            _ => None,
        }
    }

    pub fn excerpts(&self) -> &SourceExcerptSet {
        if let Some(errors) = self.schema_compile_errors() {
            return errors.excerpts();
        }
        if let Some(errors) = self.codec_errors() {
            return errors.excerpts();
        }
        match self.failure() {
            Some(Failure::Config(errors)) => errors.excerpts(),
            _ => &UNAVAILABLE_ERROR_EXCERPTS,
        }
    }

    pub fn source_name(&self, source_id: SourceId) -> Option<&str> {
        if let Some(errors) = self.config_parse_errors() {
            if errors.config_source_id() == Some(source_id) {
                return errors.config_source_name();
            }
        }
        if let Some(errors) = self.codec_errors() {
            if errors.config_source_id() == Some(source_id) {
                return errors.config_source_name();
            }
        }
        self.names().and_then(|names| names.name(source_id))
    }

    pub fn names_failure(&self) -> Option<HostErrorKind> {
        match &self.storage {
            ErrorStorage::Retained { .. } => self.error_payload().and_then(|payload| {
                payload
                    .names_failure
                    .or_else(|| payload.names.as_ref().and_then(SourceNames::failure))
            }),
            _ => self.names().and_then(SourceNames::failure),
        }
    }

    pub fn render<'slot>(
        &self,
        translator: Translator,
        options: RenderOptions,
        mut out: Out<'slot, '_, DiagnosticRenderError>,
    ) -> OResult<'slot, String> {
        if let Some(error) = self.host_error() {
            return DiagnosticRenderer::render_host(error, options, translator, out.reborrow());
        }

        let source_name = |id| self.source_name(id);
        let span_location = |span| self.span_location(span);
        let context = DiagnosticContext {
            source_name: &source_name,
            span_location: &span_location,
            excerpts: self.excerpts(),
        };

        if let ErrorStorage::PayloadStorageFailure { diagnostic, stage } = &self.storage {
            let stage = Some(report_stage(*stage));
            return DiagnosticRenderer::render_report(
                DiagnosticReport {
                    title: DiagnosticReportTitle::PayloadStorageFailed,
                    stage,
                    diagnostics: from_ref(diagnostic).iter(),
                    diagnostic_count: 1,
                    context,
                    options,
                    translator,
                },
                out.reborrow(),
            );
        }

        if let Some(errors) = self.schema_compile_errors() {
            return DiagnosticRenderer::render_report(
                DiagnosticReport {
                    title: DiagnosticReportTitle::SchemaCompilationFailed,
                    stage: None,
                    diagnostics: errors.diagnostics().iter(),
                    diagnostic_count: errors.diagnostics().len(),
                    context,
                    options,
                    translator,
                },
                out.reborrow(),
            );
        }

        let diagnostics = self.diagnostics();
        DiagnosticRenderer::render_report(
            DiagnosticReport {
                title: DiagnosticReportTitle::OperationFailed,
                stage: Some(report_stage(self.stage())),
                diagnostics: diagnostics.iter(),
                diagnostic_count: diagnostics.len(),
                context,
                options,
                translator,
            },
            out,
        )
    }

    fn names(&self) -> Option<&SourceNames> {
        match &self.storage {
            ErrorStorage::File(errors) => errors.names(),
            ErrorStorage::Retained { .. } => self
                .error_payload()
                .and_then(|payload| payload.names.as_ref()),
            _ => None,
        }
    }
}

fn report_stage(stage: TypolStage) -> DiagnosticReportStage {
    match stage {
        TypolStage::SchemaRead => DiagnosticReportStage::SchemaRead,
        TypolStage::SchemaCompile => DiagnosticReportStage::SchemaCompilation,
        TypolStage::SourceNames => DiagnosticReportStage::SourceNameRegistration,
        TypolStage::Binding => DiagnosticReportStage::TypeBinding,
        TypolStage::ConfigRead => DiagnosticReportStage::ConfigurationReading,
        TypolStage::Decoding => DiagnosticReportStage::Decoding,
        TypolStage::Encoding => DiagnosticReportStage::Encoding,
    }
}

impl fmt::Debug for StdTypolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StdTypolError")
            .field("stage", &self.stage())
            .field("diagnostic_count", &self.diagnostics().len())
            .field("names_failure", &self.names_failure())
            .finish()
    }
}

impl fmt::Display for StdTypolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut errors = ErrSlot::<DiagnosticRenderError>::new();
        match errors.call(|out| {
            self.render(
                Translator::new(Language::English),
                RenderOptions::default(),
                out,
            )
        }) {
            Ok(rendered) => formatter.write_str(&rendered),
            Err(_) => formatter.write_str("Typol operation failed"),
        }
    }
}

impl Error for StdTypolError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match &self.storage {
            ErrorStorage::File(errors) => Some(errors),
            ErrorStorage::Retained { .. } => self.failure().map(failure_source),
            ErrorStorage::InlineSchema(errors) => Some(errors),
            ErrorStorage::InlineHost { error, .. } => Some(error),
            ErrorStorage::PayloadStorageFailure { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ErrorSlot, StdTypolError, TypolStage};
    use crate::test_support::TestOpeo;
    use opeo::ErrSlot;
    use std::error::Error;
    use typol_core::{
        BudgetError, DiagnosticCode, DiagnosticContext, SourceId, diagnostic_context,
    };
    use typol_i18n::{Language, Translator};
    use typol_schema::{SchemaCompileOptions, SchemaCompiler, SchemaSource};
    use typol_std::{RenderOptions, SourceNameEditOptions, SourceNameLimits, SourceNames};
    use typol_test_support::{
        TestExpect, TestFailure, TestResult, test_verify as assert, test_verify_eq as assert_eq,
    };

    static SCHEMA_CONTEXT: DiagnosticContext =
        diagnostic_context!("typol-schema", "schema-initialization", "test-invariant",);

    #[test]
    fn failed_payload_storage_keeps_stage_capacity_code_context_and_source_contract() -> TestResult
    {
        for stage in [
            TypolStage::SchemaCompile,
            TypolStage::ConfigRead,
            TypolStage::Decoding,
            TypolStage::Binding,
            TypolStage::Encoding,
        ] {
            let errors = StdTypolError::simulated_payload_storage_failure(stage);
            let diagnostic = errors
                .diagnostics()
                .first()
                .test_expect("payload storage failure should preserve emergency diagnostics")?;
            let context = errors
                .diagnostic_context(0)
                .test_expect("payload storage failure should preserve structured context")?;

            assert_eq!(errors.stage(), stage);
            assert_eq!(errors.diagnostics().len(), 1);
            assert_eq!(diagnostic.code(), DiagnosticCode::CAPACITY_EXCEEDED);
            assert_eq!(context.subsystem(), "typol");
            assert_eq!(context.stage(), "error-storage");
            assert_eq!(context.failure_point(), "error-payload-capacity-exceeded");
            assert!(errors.schema_compile_errors().is_none());
            assert!(errors.config_parse_errors().is_none());
            assert!(errors.codec_errors().is_none());
            assert!(errors.source().is_none());

            let displayed = errors.to_string();
            assert!(displayed.contains("failed to store error payload"));
            assert!(displayed.contains("CAPACITY_EXCEEDED"));

            let rendered = TestOpeo::value(|out| {
                errors.render(
                    Translator::new(Language::English),
                    RenderOptions::default(),
                    out,
                )
            })?;
            assert!(rendered.contains("failed to store error payload"));
            assert!(rendered.contains("CAPACITY_EXCEEDED"));
        }
        Ok(())
    }

    #[test]
    fn inline_schema_failure_preserves_its_context_through_loading() -> TestResult {
        let errors = StdTypolError::schema_budget(BudgetError::invariant(&SCHEMA_CONTEXT));
        let diagnostic = errors
            .diagnostics()
            .first()
            .test_expect("loading errors should preserve schema emergency diagnostics")?;
        let context = errors
            .diagnostic_context(0)
            .test_expect("loading errors should preserve schema context")?;

        assert_eq!(errors.stage(), TypolStage::SchemaCompile);
        assert_eq!(diagnostic.code(), DiagnosticCode::INTERNAL_ERROR);
        assert_eq!(context.subsystem(), "typol-schema");
        assert_eq!(context.stage(), "schema-initialization");
        assert_eq!(context.failure_point(), "test-invariant");
        assert_eq!(errors.diagnostic_context(1), None);
        Ok(())
    }

    #[test]
    fn schema_render_and_display_use_registered_escaped_source_names() -> TestResult {
        let compiler =
            TestOpeo::value(|out| SchemaCompiler::new(SchemaCompileOptions::default(), out))?;
        let context = TestOpeo::value(|out| compiler.context(out))?;
        let error_slot = TestOpeo::value(|out| ErrorSlot::in_budget(context.execution(), out))?;
        let mut names =
            TestOpeo::value(|out| SourceNames::new(SourceNameLimits::default(), &context, out))?;
        let mut schema_errors = ErrSlot::new();
        let compilation = schema_errors.try_call(|schema_out| {
            compiler.compile_without_imports_with_observer(
                SchemaSource::from_str(b"root", "entry struct Root { value: Missing }"),
                &mut names,
                schema_out,
            )
        });
        let schema_errors = match compilation {
            Ok(_) => return Err(TestFailure::new("test schema should fail to compile")),
            Err(caught) => caught.take(),
        };
        TestOpeo::value(|out| {
            names.set_name(
                SourceId::new(0),
                "custom\u{1b}\n",
                SourceNameEditOptions::default(),
                out,
            )
        })?;
        let errors = error_slot.schema(schema_errors, names);
        let rendered = TestOpeo::value(|out| {
            errors.render(
                Translator::new(Language::English),
                RenderOptions::default(),
                out,
            )
        })?;
        let displayed = errors.to_string();

        assert!(rendered.contains("custom\\u{1b}\\n:"));
        assert!(displayed.contains("custom\\u{1b}\\n:"));
        assert!(!rendered.contains('\u{1b}'));
        assert!(!displayed.contains('\u{1b}'));
        Ok(())
    }

    #[test]
    fn schema_render_and_display_share_unknown_source_fallback() -> TestResult {
        let compiler =
            TestOpeo::value(|out| SchemaCompiler::new(SchemaCompileOptions::default(), out))?;
        let context = TestOpeo::value(|out| compiler.context(out))?;
        let error_slot = TestOpeo::value(|out| ErrorSlot::in_budget(context.execution(), out))?;
        let names =
            TestOpeo::value(|out| SourceNames::new(SourceNameLimits::new(0, 4096), &context, out))?;
        let mut schema_errors = ErrSlot::new();
        let compilation = schema_errors.try_call(|schema_out| {
            compiler.compile_without_imports(
                SchemaSource::from_str(b"root", "entry struct Root { value: Missing }"),
                schema_out,
            )
        });
        let schema_errors = match compilation {
            Ok(_) => return Err(TestFailure::new("test schema should fail to compile")),
            Err(caught) => caught.take(),
        };
        let errors = error_slot.schema(schema_errors, names);
        let rendered = TestOpeo::value(|out| {
            errors.render(
                Translator::new(Language::English),
                RenderOptions::default(),
                out,
            )
        })?;
        let displayed = errors.to_string();

        assert!(rendered.contains("source#0:"));
        assert!(displayed.contains("source#0:"));
        Ok(())
    }
}
