use core::{
    fmt::{self, Debug, Formatter},
    ops::{Deref, DerefMut},
};

use typol_regex::RegexCompileError;
use typol_semantic::CompiledSchema;
use typol_test_support::{TestOpeo, TestSlot};

use crate::{
    SchemaCompileContext, SchemaCompileErrors, SchemaCompileOptions, SchemaCompiler,
    SchemaResolver, SchemaSource, SourceObserver,
    opeo::{OResult, Out},
    test_support::TestOperation as SharedTestOperation,
};

pub(crate) struct TestOperation {
    shared: SharedTestOperation,
    regex_errors: Option<TestSlot<RegexCompileError>>,
}

impl TestOperation {
    pub(crate) const fn new() -> Self {
        Self {
            shared: SharedTestOperation::new(),
            regex_errors: None,
        }
    }

    pub(crate) fn regex<T>(
        &mut self,
        operation: impl for<'slot> FnOnce(Out<'slot, 'slot, RegexCompileError>) -> OResult<'slot, T>,
    ) -> Result<T, RegexCompileError> {
        self.regex_errors
            .get_or_insert_with(TestSlot::new)
            .access(operation)
    }
}

impl Deref for TestOperation {
    type Target = SharedTestOperation;

    fn deref(&self) -> &Self::Target {
        &self.shared
    }
}

impl DerefMut for TestOperation {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.shared
    }
}

pub(crate) struct CompileRun {
    pub(crate) result: Result<CompiledSchema, SchemaCompileErrors>,
    pub(crate) context: SchemaCompileContext,
}

pub(crate) struct CompileRunFailure<'errors>(&'errors Option<SchemaCompileErrors>);

impl Debug for CompileRunFailure<'_> {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

pub(crate) struct CompileRunErrors {
    error: Option<SchemaCompileErrors>,
}

impl CompileRunErrors {
    pub(crate) const fn new() -> Self {
        Self { error: None }
    }

    fn store(&mut self, error: SchemaCompileErrors) -> CompileRunFailure<'_> {
        if self.error.is_none() {
            self.error = Some(error);
        }
        CompileRunFailure(&self.error)
    }

    pub(crate) fn run_with_context<'errors, R: SchemaResolver + ?Sized>(
        options: SchemaCompileOptions,
        source: SchemaSource<'_>,
        resolver: &mut R,
        observer: Option<&mut dyn SourceObserver>,
        compile_errors: &'errors mut Self,
    ) -> Result<CompileRun, CompileRunFailure<'errors>> {
        if compile_errors.error.is_some() {
            return Err(CompileRunFailure(&compile_errors.error));
        }
        let compiler = match TestOpeo::call(|out| SchemaCompiler::new(options, out)) {
            Ok(compiler) => compiler,
            Err(error) => return Err(compile_errors.store(error)),
        };
        let context = match TestOpeo::call(|out| compiler.context(out)) {
            Ok(context) => context,
            Err(error) => return Err(compile_errors.store(error.into())),
        };
        let result = TestOpeo::call(|out| compiler.compile(source, resolver, observer, out));
        Ok(CompileRun { result, context })
    }
}
