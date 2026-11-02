use opeo::{ErrSlot, OResult, Out};
use typol_core::{BudgetError, ExecutionBudget};
use typol_semantic::CompiledSchema;

use crate::{
    ResolverError, SchemaCompileErrors, SchemaCompileOptions, SchemaLimits, SchemaResolveRequest,
    SchemaResolver, SchemaSource, SourceObserver, compiled::SchemaCompilation,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchemaCompileStage {
    Initialization,
    RootRead,
    SourceLoading,
    Compilation,
    Finished,
    Records,
    Locations,
    Freezing,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchemaCompileUsage {
    pub steps: usize,
    pub peak_workspace_bytes: usize,
    pub workspace_bytes: usize,
    pub last_stage: SchemaCompileStage,
    pub failure: Option<BudgetError>,
}

struct NoImports;

impl SchemaResolver for NoImports {
    fn resolve<'src, 'slot>(
        &'src mut self,
        _request: SchemaResolveRequest<'_>,
        out: Out<'slot, '_, ResolverError>,
    ) -> OResult<'slot, SchemaSource<'src>> {
        Err(out.fail(ResolverError::ImportsNotSupported))
    }
}

/// Cloning shares the existing session only; it does not allocate or replenish its budget.
pub struct SchemaCompileContext {
    resources: SchemaLimits,
    execution: ExecutionBudget,
}

impl SchemaCompileContext {
    pub(crate) fn new<'slot, E>(
        resources: SchemaLimits,
        execution: &ExecutionBudget,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, Self>
    where
        E: From<BudgetError>,
    {
        Ok(Self {
            resources,
            execution: execution.try_clone(out.reborrow())?,
        })
    }

    pub fn try_clone<'slot>(&self, mut out: Out<'slot, '_, BudgetError>) -> OResult<'slot, Self> {
        let context = Self::new(self.resources, &self.execution, out.reborrow())?;
        Ok(context)
    }

    pub const fn resources(&self) -> &SchemaLimits {
        &self.resources
    }

    pub const fn execution(&self) -> &ExecutionBudget {
        &self.execution
    }

    pub fn same_session(&self, other: &Self) -> bool {
        self.execution.same_call(&other.execution)
    }

    pub fn set_stage(&self, stage: SchemaCompileStage) {
        self.execution.set_stage(stage as usize);
    }

    pub fn usage(&self) -> SchemaCompileUsage {
        SchemaCompileUsage {
            steps: self.execution.steps(),
            peak_workspace_bytes: self.execution.peak_workspace_bytes(),
            workspace_bytes: self.execution.workspace_bytes(),
            last_stage: match self.execution.stage() {
                1 => SchemaCompileStage::RootRead,
                2 => SchemaCompileStage::SourceLoading,
                3 => SchemaCompileStage::Compilation,
                4 => SchemaCompileStage::Finished,
                5 => SchemaCompileStage::Records,
                6 => SchemaCompileStage::Locations,
                7 => SchemaCompileStage::Freezing,
                _ => SchemaCompileStage::Initialization,
            },
            failure: self.execution.failure(),
        }
    }
}

/// A one-shot compilation session; root reads and all compilation phases share one execution budget.
pub struct SchemaCompiler {
    context: SchemaCompileContext,
}

impl SchemaCompiler {
    pub fn new<'slot>(
        options: SchemaCompileOptions,
        mut out: Out<'slot, '_, SchemaCompileErrors>,
    ) -> OResult<'slot, Self> {
        let mut budget_errors = ErrSlot::new();
        let execution = match budget_errors
            .try_call(|budget_out| ExecutionBudget::new(options.execution(), budget_out))
        {
            Ok(execution) => execution,
            Err(caught) => {
                return Err(out.fail(SchemaCompileErrors::from(caught.take())));
            }
        };
        let context = SchemaCompileContext::new(*options.limits(), &execution, out.reborrow())?;
        Ok(Self { context })
    }

    pub fn context<'slot>(
        &self,
        mut out: Out<'slot, '_, BudgetError>,
    ) -> OResult<'slot, SchemaCompileContext> {
        let context = self.context.try_clone(out.reborrow())?;
        Ok(context)
    }

    /// Compiles an in-memory source without importing other Schemas.
    pub fn compile_without_imports<'slot>(
        self,
        root: SchemaSource<'_>,
        mut out: Out<'slot, '_, SchemaCompileErrors>,
    ) -> OResult<'slot, CompiledSchema> {
        let mut resolver = NoImports;
        let schema = self.compile(root, &mut resolver, None, out.reborrow())?;
        Ok(schema)
    }

    /// Compiles an in-memory source without importing other Schemas and registers the names of sources read.
    pub fn compile_without_imports_with_observer<'slot>(
        self,
        root: SchemaSource<'_>,
        observer: &mut dyn SourceObserver,
        mut out: Out<'slot, '_, SchemaCompileErrors>,
    ) -> OResult<'slot, CompiledSchema> {
        let mut resolver = NoImports;
        let schema = self.compile(root, &mut resolver, Some(observer), out.reborrow())?;
        Ok(schema)
    }

    pub fn compile<'slot, R: SchemaResolver + ?Sized>(
        self,
        root: SchemaSource<'_>,
        resolver: &mut R,
        observer: Option<&mut dyn SourceObserver>,
        mut out: Out<'slot, '_, SchemaCompileErrors>,
    ) -> OResult<'slot, CompiledSchema> {
        let schema = self.compile_inner(root, resolver, observer, out.reborrow())?;
        Ok(schema)
    }

    fn compile_inner<'slot, R: SchemaResolver + ?Sized>(
        self,
        root: SchemaSource<'_>,
        resolver: &mut R,
        observer: Option<&mut dyn SourceObserver>,
        mut out: Out<'slot, '_, SchemaCompileErrors>,
    ) -> OResult<'slot, CompiledSchema> {
        let schema =
            SchemaCompilation::compile(root, resolver, &self.context, observer, out.reborrow())?;
        self.context.set_stage(SchemaCompileStage::Finished);
        Ok(schema)
    }
}

impl Drop for SchemaCompiler {
    fn drop(&mut self) {
        self.context.execution.finish();
    }
}
