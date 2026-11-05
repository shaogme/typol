use alloc::format;
use core::{fmt::Debug, ops::Deref};
use typol_core::{
    AllocationError, BudgetError, Diagnostic, LocationError as CoreLocationError, Read,
    SourceReadError, SpanError,
};
use typol_semantic::CompiledSchema;
use typol_test_support::{
    TestError, TestExtract, TestFailure, TestOpeo, TestResult, TestResultExt, TestSlot,
    TestValueExt,
};

use crate::{
    SchemaCompileErrors, SchemaCompileOptions, SchemaCompiler, SchemaResolver, SchemaSource,
    SourceObserver, diagnostics::BuildFailure,
};
use opeo::{OResult, Out};

pub(crate) struct TestOperation {
    budget_errors: Option<TestSlot<BudgetError>>,
    diagnostic_errors: Option<TestSlot<Diagnostic>>,
    span_errors: Option<TestSlot<SpanError>>,
    build_errors: Option<TestSlot<BuildFailure>>,
    core_location_errors: Option<TestSlot<CoreLocationError>>,
    allocation_errors: Option<TestSlot<AllocationError>>,
}

pub(crate) struct SchemaCompileResult<T>(Result<T, SchemaCompileErrors>);

impl<T> SchemaCompileResult<T> {
    fn new(result: Result<T, SchemaCompileErrors>) -> Self {
        Self(result)
    }
}

impl<T> Deref for SchemaCompileResult<T> {
    type Target = Result<T, SchemaCompileErrors>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> TestExtract<T> for SchemaCompileResult<T> {
    fn test_value(self) -> TestResult<T> {
        match self.0 {
            Ok(value) => Ok(value),
            Err(error) => Err(TestFailure::new(format!("operation failed: {error:?}"))),
        }
    }
}

impl<T> TestValueExt<T> for SchemaCompileResult<T> {
    type Error = SchemaCompileErrors;

    fn into_test_value(self) -> TestResult<T> {
        match self.0 {
            Ok(value) => Ok(value),
            Err(error) => Err(TestFailure::new(format!("unexpected error: {error:?}"))),
        }
    }
}

impl<T: Debug> TestError<SchemaCompileErrors> for SchemaCompileResult<T> {
    fn test_error(self) -> TestResult<SchemaCompileErrors> {
        match self.0 {
            Err(error) => Ok(error),
            Ok(value) => Err(TestFailure::new(format!(
                "test expected the operation to fail, but it succeeded with value {value:?}"
            ))),
        }
    }
}

impl<T: Debug> TestResultExt<T> for SchemaCompileResult<T> {
    type Error = SchemaCompileErrors;

    fn into_test_error(self) -> TestResult<SchemaCompileErrors> {
        match self.0 {
            Err(error) => Ok(error),
            Ok(_) => Err(TestFailure::new(
                "expected the operation to fail, but it succeeded",
            )),
        }
    }
}

impl TestOperation {
    pub(crate) const fn new() -> Self {
        Self {
            budget_errors: None,
            diagnostic_errors: None,
            span_errors: None,
            build_errors: None,
            core_location_errors: None,
            allocation_errors: None,
        }
    }

    pub(crate) fn budget<T>(
        &mut self,
        operation: impl for<'slot> FnOnce(Out<'slot, 'slot, BudgetError>) -> OResult<'slot, T>,
    ) -> Result<T, BudgetError> {
        self.budget_errors
            .get_or_insert_with(TestSlot::new)
            .access(operation)
    }

    pub(crate) fn diagnostic<T>(
        &mut self,
        operation: impl for<'slot> FnOnce(Out<'slot, 'slot, Diagnostic>) -> OResult<'slot, T>,
    ) -> Result<T, Diagnostic> {
        self.diagnostic_errors
            .get_or_insert_with(TestSlot::new)
            .access(operation)
    }

    pub(crate) fn span<T>(
        &mut self,
        operation: impl for<'slot> FnOnce(Out<'slot, 'slot, SpanError>) -> OResult<'slot, T>,
    ) -> Result<T, SpanError> {
        self.span_errors
            .get_or_insert_with(TestSlot::new)
            .access(operation)
    }

    pub(crate) fn build<T>(
        &mut self,
        operation: impl for<'slot> FnOnce(Out<'slot, 'slot, BuildFailure>) -> OResult<'slot, T>,
    ) -> Result<T, BuildFailure> {
        self.build_errors
            .get_or_insert_with(TestSlot::new)
            .access(operation)
    }

    pub(crate) fn core_location<T>(
        &mut self,
        operation: impl for<'slot> FnOnce(Out<'slot, 'slot, CoreLocationError>) -> OResult<'slot, T>,
    ) -> Result<T, CoreLocationError> {
        self.core_location_errors
            .get_or_insert_with(TestSlot::new)
            .access(operation)
    }

    pub(crate) fn allocation<T>(
        &mut self,
        operation: impl for<'slot> FnOnce(Out<'slot, 'slot, AllocationError>) -> OResult<'slot, T>,
    ) -> Result<T, AllocationError> {
        self.allocation_errors
            .get_or_insert_with(TestSlot::new)
            .access(operation)
    }
}

pub(crate) struct SchemaCompileTestSupport;

impl SchemaCompileTestSupport {
    pub(crate) fn run<R: SchemaResolver + ?Sized, S: Read<SourceReadError>>(
        options: SchemaCompileOptions,
        source: SchemaSource<'_, S>,
        resolver: &mut R,
        observer: Option<&mut dyn SourceObserver>,
    ) -> SchemaCompileResult<CompiledSchema> {
        match TestOpeo::call(|mut out| {
            let compiler = SchemaCompiler::new(options, out.reborrow())?;
            let schema = compiler.compile(source, resolver, observer, out.reborrow())?;
            Ok(schema)
        }) {
            Ok(schema) => SchemaCompileResult::new(Ok(schema)),
            Err(error) => SchemaCompileResult::new(Err(error)),
        }
    }
}
