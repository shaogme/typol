use crate::{TypolError, TypolOptions, loading::LoadedSchema};
use opeo::{ErrSlot, OResult, Out};
use typol_schema::{SchemaCompiler, SchemaResolver, SchemaSource};

#[cfg(not(feature = "std"))]
use super::SourceNames;
#[cfg(feature = "std")]
use crate::{TypolStage, error::ErrorSlot};
#[cfg(feature = "std")]
use std::{fmt, path::Path};
#[cfg(feature = "std")]
use typol_core::{ExecutionBudget, SourceReadError};
#[cfg(feature = "std")]
use typol_std::{
    FileSchemaCompileOptions, FileSchemaCompiler, HostError, HostOperation,
    SourceNames as HostSourceNames,
};

pub(crate) struct LoadSupport;

impl LoadSupport {
    pub(crate) fn capture<'slot, 'borrow, E, T>(
        operation: impl for<'call> FnOnce(Out<'call, 'call, E>) -> OResult<'call, T>,
        map_error: impl FnOnce(E) -> TypolError,
        out: Out<'slot, 'borrow, TypolError>,
    ) -> OResult<'slot, T> {
        let mut slot = ErrSlot::new();
        match slot.try_call(operation) {
            Ok(value) => Ok(value),
            Err(caught) => Err(out.fail(map_error(caught.take()))),
        }
    }

    #[cfg(feature = "std")]
    pub(crate) fn read_source_error(error: SourceReadError) -> TypolError {
        TypolError::host(
            HostError::from_source_read_error(HostOperation::ConfigRead, error),
            TypolStage::ConfigRead,
        )
    }
}

pub struct SchemaLoader {
    options: TypolOptions,
}

#[cfg(feature = "std")]
struct CompileRequest<'root, 'resolver> {
    compiler: SchemaCompiler,
    root: SchemaSource<'root>,
    resolver: &'resolver mut dyn SchemaResolver,
    names: HostSourceNames,
}

#[cfg(feature = "std")]
impl<'root, 'resolver> CompileRequest<'root, 'resolver> {
    fn new(
        compiler: SchemaCompiler,
        root: SchemaSource<'root>,
        resolver: &'resolver mut dyn SchemaResolver,
        names: HostSourceNames,
    ) -> Self {
        Self {
            compiler,
            root,
            resolver,
            names,
        }
    }
}

impl SchemaLoader {
    pub fn new(options: TypolOptions) -> Self {
        Self { options }
    }

    #[cfg(feature = "std")]
    pub fn compile_file<'slot>(
        &self,
        path: &Path,
        out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, LoadedSchema> {
        let mut out = out;
        let file = LoadSupport::capture(
            |file_out| {
                FileSchemaCompiler::compile(
                    path,
                    FileSchemaCompileOptions::new(
                        *self.options.schema(),
                        self.options.files(),
                        self.options.names(),
                    ),
                    file_out,
                )
            },
            TypolError::file,
            out.reborrow(),
        )?;
        Ok(LoadedSchema::from_file(file, self.options))
    }

    #[cfg(feature = "std")]
    pub fn compile_text<'slot>(
        &self,
        name: &[u8],
        text: &str,
        out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, LoadedSchema> {
        self.compile_text_inner(SchemaSource::from_str(name, text), out)
    }

    #[cfg(feature = "std")]
    pub fn compile_with_resolver<'slot>(
        &self,
        root: SchemaSource<'_>,
        resolver: &mut dyn SchemaResolver,
        mut out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, LoadedSchema> {
        let compiler = LoadSupport::capture(
            |schema_out| SchemaCompiler::new(*self.options.schema(), schema_out),
            TypolError::schema_initialization,
            out.reborrow(),
        )?;
        let context = LoadSupport::capture(
            |budget_out| compiler.context(budget_out),
            TypolError::schema_budget,
            out.reborrow(),
        )?;
        let names = LoadSupport::capture(
            |budget_out| HostSourceNames::new(self.options.names(), &context, budget_out),
            TypolError::schema_budget,
            out.reborrow(),
        )?;
        self.compile_resolved(
            CompileRequest::new(compiler, root, resolver, names),
            out.reborrow(),
        )
    }

    #[cfg(feature = "std")]
    fn compile_resolved<'slot>(
        &self,
        request: CompileRequest<'_, '_>,
        mut out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, LoadedSchema> {
        let CompileRequest {
            compiler,
            root,
            resolver,
            mut names,
        } = request;
        let context = LoadSupport::capture(
            |budget_out| compiler.context(budget_out),
            TypolError::schema_budget,
            out.reborrow(),
        )?;
        let error_slot = self.error_slot(context.execution(), out.reborrow())?;
        let mut schema_errors = ErrSlot::new();
        let result = schema_errors
            .try_call(|schema_out| compiler.compile(root, resolver, Some(&mut names), schema_out));
        match result {
            Ok(schema) => Ok(LoadedSchema::from_memory(schema, names, self.options)),
            Err(caught) => Err(out.fail(error_slot.schema(caught.take(), names))),
        }
    }

    #[cfg(feature = "std")]
    fn compile_text_inner<'slot>(
        &self,
        root: SchemaSource<'_>,
        mut out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, LoadedSchema> {
        let compiler = LoadSupport::capture(
            |schema_out| SchemaCompiler::new(*self.options.schema(), schema_out),
            TypolError::schema_initialization,
            out.reborrow(),
        )?;
        let context = LoadSupport::capture(
            |budget_out| compiler.context(budget_out),
            TypolError::schema_budget,
            out.reborrow(),
        )?;
        let names = LoadSupport::capture(
            |budget_out| HostSourceNames::new(self.options.names(), &context, budget_out),
            TypolError::schema_budget,
            out.reborrow(),
        )?;
        let error_slot = self.error_slot(context.execution(), out.reborrow())?;
        let mut names = names;
        let mut schema_errors = ErrSlot::new();
        let result = schema_errors.try_call(|schema_out| {
            compiler.compile_without_imports_with_observer(root, &mut names, schema_out)
        });
        match result {
            Ok(schema) => Ok(LoadedSchema::from_memory(schema, names, self.options)),
            Err(caught) => Err(out.fail(error_slot.schema(caught.take(), names))),
        }
    }

    #[cfg(feature = "std")]
    fn error_slot<'slot>(
        &self,
        budget: &ExecutionBudget,
        mut out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, ErrorSlot> {
        LoadSupport::capture(
            |budget_out| ErrorSlot::in_budget(budget, budget_out),
            |error| TypolError::host(HostError::from_budget(error), TypolStage::SourceNames),
            out.reborrow(),
        )
    }
}

#[cfg(not(feature = "std"))]
impl SchemaLoader {
    pub fn compile_text<'slot>(
        &self,
        name: &[u8],
        text: &str,
        mut out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, LoadedSchema> {
        let compiler = LoadSupport::capture(
            |schema_out| SchemaCompiler::new(*self.options.schema(), schema_out),
            TypolError::schema,
            out.reborrow(),
        )?;
        let mut names = SourceNames::default();
        let schema = LoadSupport::capture(
            |schema_out| {
                compiler.compile_without_imports_with_observer(
                    SchemaSource::from_str(name, text),
                    &mut names,
                    schema_out,
                )
            },
            TypolError::schema,
            out.reborrow(),
        )?;
        Ok(LoadedSchema::from_memory(schema, names, self.options))
    }

    pub fn compile_with_resolver<'slot>(
        &self,
        root: SchemaSource<'_>,
        resolver: &mut dyn SchemaResolver,
        mut out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, LoadedSchema> {
        let compiler = LoadSupport::capture(
            |schema_out| SchemaCompiler::new(*self.options.schema(), schema_out),
            TypolError::schema,
            out.reborrow(),
        )?;
        let mut names = SourceNames::default();
        let schema = LoadSupport::capture(
            |schema_out| compiler.compile(root, resolver, Some(&mut names), schema_out),
            TypolError::schema,
            out.reborrow(),
        )?;
        Ok(LoadedSchema::from_memory(schema, names, self.options))
    }
}

#[cfg(feature = "std")]
impl fmt::Debug for SchemaLoader {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SchemaLoader")
            .field("options", &self.options)
            .finish()
    }
}
