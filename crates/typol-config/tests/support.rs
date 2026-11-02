use typol_config::{
    ConfigParseErrors, ConfigParseRequest, ConfigParser, Environment, ValidatedConfigValue,
    opeo::{ErrSlot, OResult, Out},
};
use typol_core::ConfigValue;
use typol_schema::{
    SchemaCompileErrors, SchemaCompileOptions, SchemaCompiler, SchemaResolver, SchemaSource,
    SourceObserver,
};
use typol_semantic::CompiledSchema;
use typol_test_support::{TestFailure, TestResult};

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum TestOpeoOutcome<T, E> {
    Success(T),
    Failure(E),
}

const _: fn(TestOpeoOutcome<(), ()>) -> TestResult<()> = TestOpeoOutcome::test_error;

impl<T, E> TestOpeoOutcome<T, E> {
    pub fn is_ok(&self) -> bool {
        matches!(self, Self::Success(_))
    }

    pub fn is_err(&self) -> bool {
        matches!(self, Self::Failure(_))
    }

    pub fn test_error(self) -> TestResult<E> {
        if self.is_ok() {
            return Err(TestFailure::new(
                "OPEO call did not produce the expected error",
            ));
        }
        match self {
            Self::Failure(error) => Ok(error),
            Self::Success(_) => Err(TestFailure::new(
                "OPEO call did not produce the expected error",
            )),
        }
    }

    pub fn test_value(self) -> TestResult<T> {
        if self.is_err() {
            return Err(TestFailure::new("OPEO call failed"));
        }
        match self {
            Self::Success(value) => Ok(value),
            Self::Failure(_) => Err(TestFailure::new("OPEO call failed")),
        }
    }
}

fn capture<E, T>(
    errors: &mut ErrSlot<E>,
    operation: impl for<'slot> FnOnce(Out<'slot, 'slot, E>) -> OResult<'slot, T>,
) -> TestOpeoOutcome<T, E> {
    match errors.try_call(operation) {
        Ok(value) => TestOpeoOutcome::Success(value),
        Err(caught) => TestOpeoOutcome::Failure(caught.take()),
    }
}

pub(crate) struct Support;

impl Support {
    pub(crate) fn parse(
        schema: &CompiledSchema,
        request: ConfigParseRequest<'_>,
        environment: &mut dyn Environment,
    ) -> TestOpeoOutcome<ConfigValue, ConfigParseErrors> {
        match Self::parse_validated_config(schema, request, environment) {
            TestOpeoOutcome::Success(value) => TestOpeoOutcome::Success(value.into_config_value()),
            TestOpeoOutcome::Failure(error) => TestOpeoOutcome::Failure(error),
        }
    }

    pub(crate) fn parse_validated_config(
        schema: &CompiledSchema,
        request: ConfigParseRequest<'_>,
        environment: &mut dyn Environment,
    ) -> TestOpeoOutcome<ValidatedConfigValue, ConfigParseErrors> {
        let mut errors = ErrSlot::new();
        capture(&mut errors, |out| {
            ConfigParser::new(schema).parse(request, environment, out)
        })
    }

    pub(crate) fn compile_schema(
        options: SchemaCompileOptions,
        source: SchemaSource<'_>,
        resolver: Option<&mut dyn SchemaResolver>,
        observer: Option<&mut dyn SourceObserver>,
    ) -> TestOpeoOutcome<CompiledSchema, SchemaCompileErrors> {
        let mut errors = ErrSlot::new();
        capture(&mut errors, |mut out| {
            let compiler = SchemaCompiler::new(options, out.reborrow())?;
            match resolver {
                Some(resolver) => compiler.compile(source, resolver, observer, out.reborrow()),
                None => match observer {
                    Some(observer) => {
                        compiler.compile_without_imports_with_observer(source, observer, out)
                    }
                    None => compiler.compile_without_imports(source, out),
                },
            }
        })
    }
}
