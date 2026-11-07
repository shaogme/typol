//! Typol is a schema-driven configuration library for Rust. It compiles `.tschem` schemas,
//! parses `.tconf` values against the compiled types, validates constraints and assertions, and
//! can convert validated values into Rust types with `#[derive(Typol)]`.
//!
//! The facade is `no_std` compatible by default and uses `alloc` for owned data. It does not
//! access files or the process environment unless the `std` feature is enabled and the matching
//! host adapter is used. The workspace requires Rust 1.85 or newer.
//!
//! # Quick start
//!
//! Compile an in-memory schema, bind it to a Rust type, then read configuration text. This
//! example uses APIs available with the default feature set:
//!
//! ```
//! use typol::{
//!     EmptyEnvironment, SchemaLoader, Typol, TypolError, TypolOptions,
//!     io::{ReadRequest, SliceSource},
//!     opeo::ErrSlot,
//! };
//!
//! #[derive(Debug, PartialEq, Typol)]
//! struct ServiceConfig {
//!     port: u16,
//! }
//!
//! fn load_config() -> Result<ServiceConfig, TypolError> {
//!     let mut errors = ErrSlot::<TypolError>::new();
//!     match errors.try_call(|mut out| {
//!         let loaded = SchemaLoader::new(TypolOptions::default()).compile_text(
//!             b"service",
//!             "entry struct ServiceConfig { port: u16 = 8080 }",
//!             out.reborrow(),
//!         )?;
//!         let binding = loaded.bind::<ServiceConfig>(out.reborrow())?;
//!         let mut environment = EmptyEnvironment;
//!         let source = SliceSource::new(b"{}");
//!         binding.read(ReadRequest::new(source, "config"), &mut environment, out)
//!     }) {
//!         Ok(config) => Ok(config),
//!         Err(caught) => Err(caught.take()),
//!     }
//! }
//!
//! assert_eq!(load_config().unwrap(), ServiceConfig { port: 8080 });
//! ```
//!
//! `compile_text` compiles schemas without imports. For imported schemas, implement
//! [`SchemaResolver`] and use [`SchemaLoader::compile_with_resolver`], or enable `std` and use
//! `SchemaLoader::compile_file`. A [`LoadedSchema`] can be reused for multiple reads. Use
//! [`LoadedSchema::read_value`] when the application needs Typol's validated value model instead
//! of a Rust type binding.
//!
//! To generate `.tconf` from Schema defaults without a Rust type, call
//! [`LoadedSchema::default_config_to_string`] or [`LoadedSchema::write_default_config`]. The entry
//! must provide defaults for every required field; optional fields without defaults become `none`.
//!
//! # Feature flags
//!
//! The crate has no default features. It remains `no_std` internally when `std` is enabled.
//!
//! | Feature | Adds | Requires `std` |
//! | --- | --- | --- |
//! | `std` | File and reader inputs, file-based schema loading, process environment access, and host diagnostic rendering | Yes |
//! | `serde` | Schema-aware Serde reads and writes for bound types | No |
//! | `hashbrown` | `hashbrown::HashMap<String, T>` bindings | No |
//! | `btree-map` | `alloc::collections::BTreeMap<String, T>` bindings; node allocation failure follows the global allocator and may panic or abort | No |
//! | `config-ir` | Config IR v1 JSON serialization; also enables `serde` | No |
//!
//! A `no_std` application needs an allocator for owned values. The library does not provide a
//! platform runtime or global allocator. Without `std`, supply schema sources through
//! [`SchemaLoader::compile_text`] or a [`SchemaResolver`], provide configuration through a
//! [`io::ReadRequest`] containing a [`Read`] source, and pass an [`Environment`] implementation to
//! each read. [`EmptyEnvironment`] is provided for reads that should not consult an external
//! source. BTree map target-storage charges estimate logical `(String, T)` entries and key text;
//! they do not bound actual tree-node or allocator overhead.
//!
//! # Validation and resource limits
//!
//! A successful read applies schema defaults and validates the resulting configuration before
//! [`BoundSchema::read`] returns a Rust value or [`LoadedSchema::read_value`] returns a
//! [`ValidatedConfigValue`]. Environment expressions are resolved only through the supplied
//! [`Environment`] provider and require matching authorization in the schema. The core library
//! does not read process variables automatically.
//!
//! Typol exposes configurable limits for parsing, schema compilation, binding, conversion,
//! diagnostics, and serialization through [`TypolOptions`] and the related option types. These
//! limits cover library-managed work; they are not operating-system CPU, total process-memory, or
//! real-time limits. Handwritten `Typol` implementations and caller-provided Serde implementations
//! remain user-code boundaries. Use `bind_trusted` for a handwritten `Typol` implementation and
//! review its behavior as application code.
//!
//! Values marked `@secret` receive safe diagnostic and display handling, but Rust values and
//! serialized output still contain their actual contents. Treat those values as sensitive in
//! application storage and logging.
//!
//! # Public API modules
//!
//! The crate root re-exports the common loading, binding, configuration input, environment, error,
//! and option APIs. The public modules provide more specialized types:
//!
//! - [`binding`] describes Rust mappings and binding and conversion policies.
//! - [`config`] exposes configuration parsing, validation, and limits.
//! - [`core`] exposes values, sources, diagnostics, and execution budgets.
//! - [`policy`] exposes shared resource and regular-expression limits.
//! - [`schema`] exposes schema compilation and source resolution.
//! - [`semantic`] exposes the compiled schema model and its evaluation interfaces.
//! - `host` is available with `std` and exposes file, process-environment, and diagnostic adapters.
//!
//! See the repository's Quick start and Advanced tutorial for end-to-end examples, and the DSL
//! specifications for the normative `.tschem` and `.tconf` language behavior.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

#[cfg(feature = "std")]
extern crate std;

mod api;
mod binding_impl;
mod codec;
#[cfg(feature = "config-ir")]
mod config_ir;
mod conversion;
mod descriptor;
mod diagnostics;
mod error;
mod loading;
#[cfg(feature = "std")]
mod loading_errors;
mod options;
mod session;
#[cfg(any(feature = "serde", test))]
mod value;
mod value_walk;

#[cfg(feature = "serde")]
mod serde;

#[cfg(all(test, feature = "std"))]
mod test_support;

#[cfg(all(test, feature = "std"))]
mod tests;

#[cfg(all(test, feature = "std"))]
pub(crate) use self::{
    binding_impl::{BindingTestRoot, BindingTestSupport},
    tests::TestAssertions,
};

#[cfg(all(test, feature = "std", feature = "serde"))]
pub(crate) use self::tests::AllocationScope;

pub use conversion::{OrderedMap, Sourced};
pub use error::{TypolError, TypolStage};
pub use loading::{
    BoundRead, BoundSchema, LoadedSchema, SchemaLoader, SourceNames, TrustedBoundSchema,
};
pub use opeo;
pub use options::{ConversionLimitError, DefaultConfigOptions, TypolOptions, WriteOptions};
pub use typol_config::{
    ConfigDocument, ConfigEdit, ConfigEditError, ConfigEditErrorKind, ConfigFormatError,
    ConfigFormatErrorKind, ConfigFormatOptions, ConfigFormatter, ConfigParseErrors,
    ConfigParseOptions, ConfigParseRequest, ConfigParser, ConfigPath, ConfigPathSegment,
    ConfigReplacement, ConfigValidationOptions, ConfigValueProcessor, EmptyEnvironment,
    Environment, EnvironmentError, EnvironmentReadContext, FormattedConfig, NormalizedConfigValue,
    ValidatedConfigValue,
};
pub use typol_core::{
    ConfigValue, ExecutionBudget, ExecutionLimits, Read, SliceSource, SourceCommitError,
    SourceExpectation, SourceRevision, SourceWriter,
};

pub use api::{binding, config, core, model, policy, schema, semantic};
pub use typol_macros::Typol;
pub use typol_schema::{
    SchemaCompileBudgetDomain, SchemaCompileErrors, SchemaCompileOptions, SchemaCompiler,
    SchemaResolver, SchemaSource,
};
pub use typol_syntax::{
    FormattedSchema, SchemaDocument, SchemaFormatError, SchemaFormatErrorKind, SchemaFormatOptions,
    SchemaFormatter, SchemaParseOptions, SchemaParser, SyntaxErrors,
};

#[cfg(feature = "std")]
pub use api::host;

#[cfg(feature = "std")]
pub use typol_std::{ProcessEnvironment, ProcessEnvironmentLimits};

#[cfg(feature = "config-ir")]
pub use config_ir::{
    ConfigIr, ConfigIrError, ConfigIrErrorKind, ConfigIrOptions, ConfigIrValueMode,
    ConfigValueErrors,
};

pub mod io {
    pub use typol_core::{FmtWriteSink, Read, ReadRequest, SliceSource, Write, WriteError};

    #[cfg(feature = "std")]
    pub use typol_std::IoWriteSink;
}

#[cfg(feature = "std")]
pub mod std_support {
    pub use crate::host::*;
    pub use crate::{
        BoundSchema, LoadedSchema, SchemaLoader, TrustedBoundSchema, TypolError, TypolStage,
    };
}

#[doc(hidden)]
pub mod __private {
    pub use crate::{
        conversion::{ConversionContext, ConversionFields, ConversionPayload, ConversionValue},
        descriptor::{
            BudgetedTypol, DescribeContext, DescriptorNode, DescriptorProvider, TypeDescriptor,
            TypeShape, Typol, VariantShape,
        },
        diagnostics::CodecErrors,
    };
    pub use opeo::{OResult, Out};
    pub use typol_core::{ConfigValue, ValueId};
}
