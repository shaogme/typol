//! Configuration diagnostics are returned through the unified parsing entry point.

#![no_std]

extern crate alloc;

mod api;
mod context;
mod diagnostics;
mod document;
mod environment;
mod instance;
mod options;
mod parser;
mod scalar;
mod secret;
mod session;
mod validation;
mod value;
mod value_normalization;
mod value_validation;

pub use api::{ConfigDiagnosticRedactor, ConfigParser, ConfigValueProcessor};
pub use diagnostics::{ConfigFailureStage, ConfigParseErrors, ConfigValueErrors};
pub use document::{
    ConfigDocument, ConfigEdit, ConfigEditError, ConfigEditErrorKind, ConfigFormatError,
    ConfigFormatErrorKind, ConfigFormatOptions, ConfigFormatter, ConfigPath, ConfigPathSegment,
    ConfigReplacement, ConfigSourceCommitError, ConfigSourceExpectation, ConfigSourceRevision,
    ConfigSourceWriter, FormattedConfig,
};
pub use environment::{EmptyEnvironment, Environment, EnvironmentError, EnvironmentReadContext};
pub use opeo;
pub use options::{
    ConfigLimits, ConfigParseOptions, ConfigParseRequest, ConfigValidationOptions,
    ConfigValidationRequest,
};
pub use typol_syntax::{StreamLimits, StreamLimitsError};
pub use value::{NormalizedConfigValue, ValidatedConfigValue};
