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

#[path = "allocation_harness/allocation_support.rs"]
mod allocation_support;

#[cfg(test)]
#[path = "allocation_harness/allocation_tests.rs"]
mod allocation_tests;

mod test_support;

pub(crate) use annotations::{AnnotationCompileInput, AnnotationCompiler};
pub(crate) use defaults::{DefaultInputs, DefaultPass};
pub(crate) use expressions::{ExpressionCompileInput, ExpressionCompiler};
pub(crate) use types::TypeGraphCompiler;

pub use opeo;

pub use compiled::{LocationFailure, LocationStatus, SchemaCompileErrors};
pub use compiler::{SchemaCompileContext, SchemaCompileStage, SchemaCompileUsage, SchemaCompiler};
pub use resolver::{
    ResolverError, SchemaCompileOptions, SchemaLimits, SchemaResolveRequest, SchemaResolver,
    SchemaSource, SourceLoadedRequest, SourceObserver, SourceObserverError,
};
