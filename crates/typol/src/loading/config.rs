use super::{SourceNames, loader::LoadSupport};
use crate::{
    BoundSchema, TypolError, TypolOptions, TypolStage,
    binding_impl::TypedSchema,
    descriptor::{BudgetedTypol, Typol},
    error::{ErrorNames, ErrorSlot},
    loading::TrustedBoundSchema,
};
use opeo::{ErrSlot, OResult, Out};
use typol_config::{
    ConfigDocument, ConfigParseErrors, ConfigParseRequest, ConfigParser, Environment,
    ValidatedConfigValue,
};
use typol_core::{ExecutionBudget, Read, ReadRequest};
use typol_semantic::CompiledSchema;

#[cfg(feature = "std")]
use crate::loading::{FileReadRequest, PreparedConfig};
#[cfg(feature = "std")]
use std::{fmt, io::Read as IoRead, path::Path};
#[cfg(feature = "std")]
use typol_config::{ConfigSourceCommitError, ConfigSourceExpectation, ConfigSourceRevision};
#[cfg(feature = "std")]
use typol_core::{ExecutionLimits, SourceId};
#[cfg(feature = "std")]
use typol_std::{
    ConfigSourceWriteError, FileConfigInput, FileConfigSourceWriter, FileSchemaCompilation,
    HostError, HostOperation, ReadSource, SourceNames as HostSourceNames,
};

// The terminal slot reserves one error payload without consuming source-name edit limits.
#[cfg(feature = "std")]
const ERROR_STORAGE_LIMITS: ExecutionLimits = ExecutionLimits::new(1, 16_384);

pub(crate) struct ConfigSourceRequest<'source, 'budget, 'display, 'environment> {
    pub(crate) source: &'source mut dyn Read,
    pub(crate) budget: &'budget ExecutionBudget,
    pub(crate) display_name: &'display str,
    pub(crate) environment: &'environment mut dyn Environment,
}

pub(crate) trait ReadStrategy {
    type Value;
    type Error;

    fn convert<'slot, 'borrow>(
        &mut self,
        request: ConfigSourceRequest<'_, '_, '_, '_>,
        out: Out<'slot, 'borrow, Self::Error>,
    ) -> OResult<'slot, Self::Value>;

    fn finish_error(
        &mut self,
        slot: ErrorSlot,
        error: Self::Error,
        names: ErrorNames,
    ) -> TypolError;
}

struct ConfigExecution<'environment, Strategy> {
    environment: &'environment mut dyn Environment,
    budget: ExecutionBudget,
    slot: ErrorSlot,
    strategy: Strategy,
}

struct SourceExecution<'source, 'display, 'environment, Strategy> {
    source: &'source mut dyn Read,
    display_name: &'display str,
    execution: ConfigExecution<'environment, Strategy>,
}

/// Owns the compiled schema and source names; bindings can be reused across reads.
pub struct LoadedSchema {
    schema: CompiledSchema,
    names: SourceNames,
    options: TypolOptions,
}

struct ConfigValueParser<'loaded> {
    loaded: &'loaded LoadedSchema,
}

struct ConfigDocumentParser<'loaded> {
    loaded: &'loaded LoadedSchema,
}

impl ReadStrategy for ConfigValueParser<'_> {
    type Value = ValidatedConfigValue;
    type Error = ConfigParseErrors;

    fn convert<'slot, 'borrow>(
        &mut self,
        request: ConfigSourceRequest<'_, '_, '_, '_>,
        out: Out<'slot, 'borrow, Self::Error>,
    ) -> OResult<'slot, Self::Value> {
        ConfigParser::new(self.loaded.schema()).parse_with_budget(
            ConfigParseRequest::new(
                request.source,
                request.display_name,
                self.loaded.options.config(),
            ),
            request.environment,
            request.budget,
            out,
        )
    }

    fn finish_error(
        &mut self,
        slot: ErrorSlot,
        error: Self::Error,
        names: ErrorNames,
    ) -> TypolError {
        #[cfg(feature = "std")]
        if let Some(source_error) = error.source_read_error() {
            return LoadSupport::read_source_error(source_error);
        }
        slot.config(error, names)
    }
}

impl<'loaded> ReadStrategy for ConfigDocumentParser<'loaded> {
    type Value = ConfigDocument<'loaded>;
    type Error = ConfigParseErrors;

    fn convert<'slot, 'borrow>(
        &mut self,
        request: ConfigSourceRequest<'_, '_, '_, '_>,
        out: Out<'slot, 'borrow, Self::Error>,
    ) -> OResult<'slot, Self::Value> {
        ConfigParser::new(self.loaded.schema()).parse_document_with_budget(
            ConfigParseRequest::new(
                request.source,
                request.display_name,
                self.loaded.options.config(),
            ),
            request.environment,
            request.budget,
            out,
        )
    }

    fn finish_error(
        &mut self,
        slot: ErrorSlot,
        error: Self::Error,
        names: ErrorNames,
    ) -> TypolError {
        #[cfg(feature = "std")]
        if let Some(source_error) = error.source_read_error() {
            return LoadSupport::read_source_error(source_error);
        }
        slot.config(error, names)
    }
}

impl LoadedSchema {
    #[cfg(feature = "std")]
    pub(crate) fn from_file(file: FileSchemaCompilation, options: TypolOptions) -> Self {
        let (schema, names) = file.into_parts();
        Self {
            schema,
            names: SourceNames::from_host(names),
            options,
        }
    }

    #[cfg(feature = "std")]
    pub(crate) fn from_memory(
        schema: CompiledSchema,
        names: HostSourceNames,
        options: TypolOptions,
    ) -> Self {
        Self {
            schema,
            names: SourceNames::from_host(names),
            options,
        }
    }

    #[cfg(not(feature = "std"))]
    pub(crate) fn from_memory(
        schema: CompiledSchema,
        names: SourceNames,
        options: TypolOptions,
    ) -> Self {
        Self {
            schema,
            names,
            options,
        }
    }

    pub fn schema(&self) -> &CompiledSchema {
        &self.schema
    }

    pub fn source_names(&self) -> &SourceNames {
        &self.names
    }

    #[cfg(feature = "std")]
    pub fn set_source_name<'slot, 'borrow>(
        &mut self,
        source_id: SourceId,
        name: &str,
        out: Out<'slot, 'borrow, TypolError>,
    ) -> OResult<'slot, ()> {
        let options = self.options.name_edits();
        LoadSupport::capture(
            |host_out| self.names.set_name(source_id, name, options, host_out),
            |error| TypolError::host(error, TypolStage::SourceNames),
            out,
        )
    }

    pub const fn options(&self) -> &TypolOptions {
        &self.options
    }

    /// Reads and returns a configuration value validated against this Schema.
    pub fn read_value<'slot, R: Read>(
        &self,
        request: ReadRequest<'_, R>,
        environment: &mut dyn Environment,
        out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, ValidatedConfigValue> {
        self.run_read_request(
            request,
            environment,
            ConfigValueParser { loaded: self },
            out,
        )
    }

    /// Reads a configuration source and retains its text and source locations for editing.
    pub fn read_document<'slot, R: Read>(
        &self,
        request: ReadRequest<'_, R>,
        environment: &mut dyn Environment,
        out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, ConfigDocument<'_>> {
        self.run_read_request(
            request,
            environment,
            ConfigDocumentParser { loaded: self },
            out,
        )
    }

    #[cfg(feature = "std")]
    pub fn read_file<'slot>(
        &self,
        path: &Path,
        environment: &mut dyn Environment,
        out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, ConfigDocument<'_>> {
        self.read_file_named(path, "config", environment, out)
    }

    #[cfg(feature = "std")]
    pub fn read_file_named<'slot>(
        &self,
        path: &Path,
        display_name: &str,
        environment: &mut dyn Environment,
        out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, ConfigDocument<'_>> {
        self.run_file_request(
            FileReadRequest { path, display_name },
            environment,
            ConfigDocumentParser { loaded: self },
            out,
        )
    }

    #[cfg(feature = "std")]
    pub fn read_file_value<'slot>(
        &self,
        path: &Path,
        environment: &mut dyn Environment,
        out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, ValidatedConfigValue> {
        self.read_file_value_named(path, "config", environment, out)
    }

    #[cfg(feature = "std")]
    pub fn read_file_value_named<'slot>(
        &self,
        path: &Path,
        display_name: &str,
        environment: &mut dyn Environment,
        out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, ValidatedConfigValue> {
        self.run_file_request(
            FileReadRequest { path, display_name },
            environment,
            ConfigValueParser { loaded: self },
            out,
        )
    }

    #[cfg(feature = "std")]
    /// Writes a file using the document's retained source snapshot as the expectation.
    ///
    /// A local filesystem that cannot atomically compare that snapshot with the target returns
    /// `AtomicReplaceUnsupported`. Use `write_file_with_expectation` to choose an explicit
    /// overwrite policy when that behavior is intended.
    pub fn write_file(
        &self,
        document: &mut ConfigDocument<'_>,
        path: &Path,
    ) -> Result<ConfigSourceRevision, ConfigSourceCommitError<ConfigSourceWriteError>> {
        self.write_file_with_expectation(document, path, document.source_expectation())
    }

    #[cfg(feature = "std")]
    /// Writes a file using an explicit target expectation and the configured file/source limit.
    pub fn write_file_with_expectation(
        &self,
        document: &mut ConfigDocument<'_>,
        path: &Path,
        expectation: ConfigSourceExpectation<'_>,
    ) -> Result<ConfigSourceRevision, ConfigSourceCommitError<ConfigSourceWriteError>> {
        let max_bytes = self
            .options
            .config()
            .config()
            .max_source_bytes()
            .min(self.options.file_input_limits().max_file_bytes());
        if document.source().len() > max_bytes {
            return Err(ConfigSourceCommitError::ResourceLimitExceeded);
        }
        let mut writer =
            FileConfigSourceWriter::new(self.options.file_input_limits().max_path_bytes());
        document.write_source(path, &mut writer, expectation)
    }

    #[cfg(feature = "std")]
    pub fn read_io<'slot>(
        &self,
        reader: &mut dyn IoRead,
        display_name: &str,
        environment: &mut dyn Environment,
        out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, ValidatedConfigValue> {
        self.run_io_request(
            ReadRequest::new(reader, display_name),
            environment,
            ConfigValueParser { loaded: self },
            out,
        )
    }

    #[cfg(feature = "std")]
    pub(crate) fn run_file_request<'slot, Strategy: ReadStrategy>(
        &self,
        request: FileReadRequest<'_>,
        environment: &mut dyn Environment,
        strategy: Strategy,
        mut out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, Strategy::Value> {
        let slot = self.error_slot(out.reborrow())?;
        let budget = self.config_budget(out.reborrow())?;
        let mut prepared = self.prepare_config(request.path, slot, &budget, out.reborrow())?;
        self.run_source(
            SourceExecution {
                source: &mut prepared.source,
                display_name: request.display_name,
                execution: ConfigExecution {
                    environment,
                    budget,
                    slot: prepared.slot,
                    strategy,
                },
            },
            out,
        )
    }

    pub(crate) fn run_read_request<'slot, Strategy: ReadStrategy, R: Read>(
        &self,
        request: ReadRequest<'_, R>,
        environment: &mut dyn Environment,
        strategy: Strategy,
        mut out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, Strategy::Value> {
        let (mut source, display_name) = request.into_parts();
        let execution = ConfigExecution {
            environment,
            budget: self.config_budget(out.reborrow())?,
            slot: self.error_slot(out.reborrow())?,
            strategy,
        };
        self.run_source(
            SourceExecution {
                source: &mut source,
                display_name,
                execution,
            },
            out,
        )
    }

    #[cfg(feature = "std")]
    pub(crate) fn run_io_request<'slot, Strategy: ReadStrategy>(
        &self,
        request: ReadRequest<'_, &mut dyn IoRead>,
        environment: &mut dyn Environment,
        strategy: Strategy,
        mut out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, Strategy::Value> {
        let (reader, display_name) = request.into_parts();
        let budget = self.config_budget(out.reborrow())?;
        let slot = self.error_slot(out.reborrow())?;
        let mut source = LoadSupport::capture(
            |source_out| ReadSource::new(reader, &budget, source_out),
            LoadSupport::read_source_error,
            out.reborrow(),
        )?;
        self.run_source(
            SourceExecution {
                source: &mut source,
                display_name,
                execution: ConfigExecution {
                    environment,
                    budget,
                    slot,
                    strategy,
                },
            },
            out,
        )
    }

    fn run_source<'slot, Strategy: ReadStrategy>(
        &self,
        execution: SourceExecution<'_, '_, '_, Strategy>,
        out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, Strategy::Value> {
        let ConfigExecution {
            environment,
            budget,
            slot,
            mut strategy,
        } = execution.execution;
        let mut operation_errors = ErrSlot::new();
        let result = operation_errors.try_call(|operation_out| {
            strategy.convert(
                ConfigSourceRequest {
                    source: execution.source,
                    budget: &budget,
                    display_name: execution.display_name,
                    environment,
                },
                operation_out,
            )
        });
        match result {
            Ok(value) => Ok(value),
            Err(caught) => {
                Err(out.fail(strategy.finish_error(slot, caught.take(), self.error_names())))
            }
        }
    }

    pub(crate) fn config_budget<'slot, 'borrow>(
        &self,
        out: Out<'slot, 'borrow, TypolError>,
    ) -> OResult<'slot, ExecutionBudget> {
        LoadSupport::capture(
            |budget_out| ExecutionBudget::new(*self.options.config().execution(), budget_out),
            #[cfg(feature = "std")]
            |error| {
                TypolError::host(
                    HostError::budget(HostOperation::ConfigRead, error),
                    TypolStage::ConfigRead,
                )
            },
            #[cfg(not(feature = "std"))]
            |error| TypolError::budget(error, TypolStage::ConfigRead),
            out,
        )
    }

    #[cfg(feature = "std")]
    pub(crate) fn error_slot<'slot>(
        &self,
        mut out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, ErrorSlot> {
        let budget = LoadSupport::capture(
            |budget_out| ExecutionBudget::new(ERROR_STORAGE_LIMITS, budget_out),
            |error| TypolError::host(HostError::from_budget(error), TypolStage::SourceNames),
            out.reborrow(),
        )?;
        LoadSupport::capture(
            |budget_out| ErrorSlot::in_budget(&budget, budget_out),
            |error| TypolError::host(HostError::from_budget(error), TypolStage::SourceNames),
            out.reborrow(),
        )
    }

    #[cfg(not(feature = "std"))]
    pub(crate) fn error_slot<'slot>(
        &self,
        _: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, ErrorSlot> {
        Ok(ErrorSlot::new())
    }

    pub(crate) fn error_names(&self) -> ErrorNames {
        #[cfg(feature = "std")]
        {
            let mut host_errors = ErrSlot::new();
            match host_errors.try_call(|host_out| {
                self.names
                    .host_names()
                    .try_copy(self.options.name_edits(), host_out)
            }) {
                Ok(names) => ErrorNames::copied(names),
                Err(caught) => ErrorNames::failed(caught.take().kind()),
            }
        }

        #[cfg(not(feature = "std"))]
        {
            ErrorNames::empty()
        }
    }

    /// Binds a built-in or derive-generated type through the library-managed budget path.
    ///
    pub fn bind<'slot, T: BudgetedTypol>(
        &self,
        mut out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, BoundSchema<'_, T>> {
        let mut codec_errors = ErrSlot::new();
        let result = codec_errors
            .try_call(|codec_out| TypedSchema::bind(self.schema(), self.options.bind(), codec_out));
        match result {
            Ok(binding) => Ok(BoundSchema::new(self, binding)),
            Err(caught) => {
                let error_slot = self.error_slot(out.reborrow())?;
                Err(out.fail(TypolError::binding(
                    caught.take(),
                    self.error_names(),
                    error_slot,
                )))
            }
        }
    }

    /// Binds a handwritten `Typol` implementation through the explicit trusted extension path.
    ///
    /// Library budgets still cover context-routed work, but cannot constrain callback allocations,
    /// blocking, arbitrary computation, or the Rust value returned by handwritten code.
    pub fn bind_trusted<'slot, T: Typol>(
        &self,
        mut out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, TrustedBoundSchema<'_, T>> {
        let mut codec_errors = ErrSlot::new();
        let result = codec_errors.try_call(|codec_out| {
            TypedSchema::bind_trusted(self.schema(), self.options.bind(), codec_out)
        });
        match result {
            Ok(binding) => Ok(TrustedBoundSchema::new(self, binding)),
            Err(caught) => {
                let error_slot = self.error_slot(out.reborrow())?;
                Err(out.fail(TypolError::binding(
                    caught.take(),
                    self.error_names(),
                    error_slot,
                )))
            }
        }
    }

    #[cfg(feature = "std")]
    pub(crate) fn prepare_config<'slot>(
        &self,
        path: &Path,
        slot: ErrorSlot,
        budget: &ExecutionBudget,
        out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, PreparedConfig> {
        let mut host_errors = ErrSlot::new();
        let result = host_errors.try_call(|host_out| {
            FileConfigInput::open(
                path,
                self.options.file_input_limits(),
                self.options.config().config().max_source_bytes(),
                budget,
                host_out,
            )
        });
        match result {
            Ok(source) => Ok(PreparedConfig { source, slot }),
            Err(caught) => Err(out.fail(slot.host(caught.take(), self.error_names()))),
        }
    }
}

#[cfg(feature = "std")]
impl fmt::Debug for LoadedSchema {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LoadedSchema")
            .field("source_names", self.source_names())
            .finish_non_exhaustive()
    }
}
