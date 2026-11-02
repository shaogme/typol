use crate::diagnostics::CodecErrors;
use core::{error::Error, fmt};
use typol_config::ConfigParseErrors;
use typol_core::BudgetError;
use typol_schema::SchemaCompileErrors;

#[cfg(feature = "std")]
pub(crate) use crate::loading_errors::{ErrorNames, ErrorSlot};
#[cfg(feature = "std")]
use crate::loading_errors::{StdTypolError, TypolDiagnosticList};
#[cfg(feature = "std")]
use alloc::string::String;
#[cfg(feature = "std")]
use opeo::{OResult, Out};
#[cfg(not(feature = "std"))]
use typol_config::ConfigFailureStage;
#[cfg(feature = "std")]
use typol_core::{DiagnosticContext, SourceId, SourceSpan};
#[cfg(feature = "std")]
use typol_i18n::Translator;
#[cfg(feature = "std")]
use typol_semantic::SpanLocation;
#[cfg(feature = "std")]
use typol_std::{
    DiagnosticRenderError, FileSchemaCompileErrors, HostError, HostErrorKind, RenderOptions,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TypolStage {
    SchemaRead,
    SchemaCompile,
    SourceNames,
    Binding,
    ConfigRead,
    Decoding,
    Encoding,
}

#[cfg(not(feature = "std"))]
#[derive(Debug)]
enum Failure {
    Budget(BudgetError),
    Schema(SchemaCompileErrors),
    Config(ConfigParseErrors),
    Codec(CodecErrors),
}

pub struct TypolError {
    #[cfg(feature = "std")]
    inner: StdTypolError,
    #[cfg(not(feature = "std"))]
    stage: TypolStage,
    #[cfg(not(feature = "std"))]
    failure: Failure,
}

#[cfg(not(feature = "std"))]
pub(crate) struct ErrorNames;

#[cfg(not(feature = "std"))]
pub(crate) struct ErrorSlot;

#[cfg(not(feature = "std"))]
impl ErrorNames {
    pub(crate) const fn empty() -> Self {
        Self
    }
}

#[cfg(not(feature = "std"))]
impl ErrorSlot {
    pub(crate) const fn new() -> Self {
        Self
    }

    pub(crate) fn config(self, errors: ConfigParseErrors, _: ErrorNames) -> TypolError {
        TypolError::config(errors)
    }

    pub(crate) fn codec(self, errors: CodecErrors, _: ErrorNames) -> TypolError {
        TypolError::codec(errors, TypolStage::Decoding)
    }

    pub(crate) fn encoding(self, errors: CodecErrors, _: ErrorNames) -> TypolError {
        TypolError::codec(errors, TypolStage::Encoding)
    }
}

#[cfg(feature = "std")]
impl From<StdTypolError> for TypolError {
    fn from(inner: StdTypolError) -> Self {
        Self { inner }
    }
}

#[cfg(feature = "std")]
impl TypolError {
    pub(crate) fn file(errors: FileSchemaCompileErrors) -> Self {
        StdTypolError::file(errors).into()
    }

    pub(crate) fn schema_budget(error: BudgetError) -> Self {
        StdTypolError::schema_budget(error).into()
    }

    pub(crate) fn schema_initialization(errors: SchemaCompileErrors) -> Self {
        StdTypolError::schema_initialization(errors).into()
    }

    pub(crate) fn host(error: HostError, stage: TypolStage) -> Self {
        StdTypolError::host(error, stage).into()
    }

    pub(crate) fn binding(errors: CodecErrors, names: ErrorNames, slot: ErrorSlot) -> Self {
        StdTypolError::binding(errors, names, slot).into()
    }

    pub fn stage(&self) -> TypolStage {
        self.inner.stage()
    }

    pub fn budget_error(&self) -> Option<BudgetError> {
        self.inner.host_error().and_then(HostError::budget_error)
    }

    pub fn schema_errors(&self) -> Option<&SchemaCompileErrors> {
        self.inner.schema_compile_errors()
    }

    pub fn config_errors(&self) -> Option<&ConfigParseErrors> {
        self.inner.config_parse_errors()
    }

    pub fn codec_errors(&self) -> Option<&CodecErrors> {
        self.inner.codec_errors()
    }

    pub fn host_error(&self) -> Option<&HostError> {
        self.inner.host_error()
    }

    pub fn diagnostics(&self) -> TypolDiagnosticList<'_> {
        self.inner.diagnostics()
    }

    pub fn diagnostic_context(&self, index: usize) -> Option<DiagnosticContext> {
        self.inner.diagnostic_context(index)
    }

    pub fn span_location(&self, span: SourceSpan) -> Option<SpanLocation> {
        self.inner.span_location(span)
    }

    pub fn source_name(&self, source_id: SourceId) -> Option<&str> {
        self.inner.source_name(source_id)
    }

    pub fn names_failure(&self) -> Option<HostErrorKind> {
        self.inner.names_failure()
    }

    pub fn render<'slot>(
        &self,
        translator: Translator,
        options: RenderOptions,
        out: Out<'slot, '_, DiagnosticRenderError>,
    ) -> OResult<'slot, String> {
        self.inner.render(translator, options, out)
    }
}

#[cfg(not(feature = "std"))]
impl TypolError {
    pub(crate) fn budget(error: BudgetError, stage: TypolStage) -> Self {
        Self {
            stage,
            failure: Failure::Budget(error),
        }
    }

    pub(crate) fn schema(error: SchemaCompileErrors) -> Self {
        Self {
            stage: TypolStage::SchemaCompile,
            failure: Failure::Schema(error),
        }
    }

    pub(crate) fn config(error: ConfigParseErrors) -> Self {
        let stage = match error.stage() {
            ConfigFailureStage::Input => TypolStage::ConfigRead,
            ConfigFailureStage::Parsing => TypolStage::Decoding,
        };
        Self {
            stage,
            failure: Failure::Config(error),
        }
    }

    pub(crate) fn codec(error: CodecErrors, stage: TypolStage) -> Self {
        Self {
            stage,
            failure: Failure::Codec(error),
        }
    }

    pub(crate) fn binding(errors: CodecErrors, _: ErrorNames, _: ErrorSlot) -> Self {
        Self::codec(errors, TypolStage::Binding)
    }

    pub fn stage(&self) -> TypolStage {
        self.stage
    }

    pub fn budget_error(&self) -> Option<BudgetError> {
        match self.failure {
            Failure::Budget(error) => Some(error),
            _ => None,
        }
    }

    pub fn schema_errors(&self) -> Option<&SchemaCompileErrors> {
        match &self.failure {
            Failure::Schema(errors) => Some(errors),
            _ => None,
        }
    }

    pub fn config_errors(&self) -> Option<&ConfigParseErrors> {
        match &self.failure {
            Failure::Config(errors) => Some(errors),
            _ => None,
        }
    }

    pub fn codec_errors(&self) -> Option<&CodecErrors> {
        match &self.failure {
            Failure::Codec(errors) => Some(errors),
            _ => None,
        }
    }
}

#[cfg(feature = "std")]
impl fmt::Debug for TypolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.inner, formatter)
    }
}

#[cfg(not(feature = "std"))]
impl fmt::Debug for TypolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TypolError")
            .field("stage", &self.stage)
            .finish_non_exhaustive()
    }
}

impl fmt::Display for TypolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        #[cfg(feature = "std")]
        {
            fmt::Display::fmt(&self.inner, formatter)
        }

        #[cfg(not(feature = "std"))]
        {
            formatter.write_str(match self.stage {
                TypolStage::SchemaRead => "schema reading failed",
                TypolStage::SchemaCompile => "schema compilation failed",
                TypolStage::SourceNames => "source name registration failed",
                TypolStage::ConfigRead => "configuration reading failed",
                TypolStage::Binding => "type binding failed",
                TypolStage::Decoding => "configuration decoding failed",
                TypolStage::Encoding => "configuration encoding failed",
            })
        }
    }
}

impl Error for TypolError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        #[cfg(feature = "std")]
        {
            Error::source(&self.inner)
        }

        #[cfg(not(feature = "std"))]
        {
            match &self.failure {
                Failure::Budget(error) => Some(error),
                Failure::Schema(error) => Some(error),
                Failure::Config(error) => Some(error),
                Failure::Codec(error) => Some(error),
            }
        }
    }
}
