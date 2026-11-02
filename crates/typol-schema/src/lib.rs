#![no_std]

extern crate alloc;

#[macro_use]
mod diagnostics;
mod annotations;
mod compiled;
mod compiler;
mod context;
mod defaults;
mod excerpts;
mod execution;
mod expressions;
mod loader;
mod locations;
mod module_tree;
mod regex;
mod resolver;
mod session;
mod symbols;
mod types;

#[cfg(test)]
mod pass_tests;

#[cfg(test)]
mod test_support;

pub(crate) use annotations::{AnnotationCompileInput, AnnotationCompiler};
pub(crate) use defaults::{DefaultInputs, DefaultPass};
pub(crate) use expressions::{ExpressionCompileInput, ExpressionCompiler};
pub(crate) use types::TypeGraphCompiler;

pub use opeo;

pub use compiled::{
    LocationFailure, LocationStatus, SchemaCompileContextFailure, SchemaCompileErrors,
    SchemaInternalCause,
};
pub use compiler::{SchemaCompileContext, SchemaCompileStage, SchemaCompileUsage, SchemaCompiler};
pub use diagnostics::DiagnosticList;
pub use resolver::{
    ResolverError, SchemaCompileOptions, SchemaLimits, SchemaResolveRequest, SchemaResolver,
    SchemaSource, SourceLoadedRequest, SourceObserver, SourceObserverError,
};
