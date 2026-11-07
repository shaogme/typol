use opeo::{OResult, Out};
use typol_core::{
    Diagnostic, DiagnosticContext, ExcerptLimits, ExecutionLimits, LocatedSpan, Read, SliceSource,
    SourceId, SourceReadError, SourceSpan, Span, diagnostic_context,
};
use typol_policy::{RegexOptions, ValueLimits};
use typol_syntax::{StreamLimits, SyntaxLimits};

use crate::SchemaCompileContext;

/// Fixed Schema declaration AST depth beyond nested types.
const TYPE_DECLARATION_DEPTH_OVERHEAD: usize = 5;

static SCHEMA_SOURCE_READ_FAILURE: DiagnosticContext =
    diagnostic_context!("typol-schema", "source-input", "source-read-failed");

/// A single-consumption Schema byte source with an opaque identity.
///
/// Identity is compared exactly within this load graph and is not stored in compiled output.
pub struct SchemaSource<'src, R: Read<SourceReadError> = &'src mut dyn Read<SourceReadError>> {
    identity: &'src [u8],
    input: R,
    known_len: Option<usize>,
    context: Option<&'src SchemaCompileContext>,
}

impl<'src, R: Read<SourceReadError> + ?Sized> SchemaSource<'src, &'src mut R> {
    pub fn from_source(identity: &'src [u8], input: &'src mut R) -> Self {
        Self {
            identity,
            input,
            known_len: None,
            context: None,
        }
    }

    pub fn from_source_in_context(
        identity: &'src [u8],
        input: &'src mut R,
        context: &'src SchemaCompileContext,
    ) -> Self {
        Self {
            identity,
            input,
            known_len: None,
            context: Some(context),
        }
    }
}

impl<'src> SchemaSource<'src, SliceSource<'src>> {
    pub fn from_str(identity: &'src [u8], text: &'src str) -> Self {
        Self {
            identity,
            input: SliceSource::new(text.as_bytes()),
            known_len: Some(text.len()),
            context: None,
        }
    }

    pub fn in_context(
        identity: &'src [u8],
        text: &'src str,
        context: &'src SchemaCompileContext,
    ) -> Self {
        Self {
            identity,
            input: SliceSource::new(text.as_bytes()),
            known_len: Some(text.len()),
            context: Some(context),
        }
    }
}

impl<'src> SchemaSource<'src> {
    pub(crate) fn source_read_diagnostic(
        error: SourceReadError,
        source_id: SourceId,
        offset: usize,
    ) -> Diagnostic {
        let source_id = error.source_id().unwrap_or(source_id);
        let offset = error.offset().unwrap_or(offset);
        let mut diagnostic = Diagnostic::emergency(error.diagnostic_code());
        diagnostic.set_located_primary(Some(LocatedSpan::new(
            SourceSpan::new(source_id, Span::point(offset)),
            None,
        )));
        diagnostic.set_context(Some(error.context().unwrap_or(SCHEMA_SOURCE_READ_FAILURE)));
        diagnostic
    }
}

impl<'src, R: Read<SourceReadError>> SchemaSource<'src, R> {
    pub(crate) const fn known_len(&self) -> Option<usize> {
        self.known_len
    }

    pub(crate) fn matches_context(&self, context: &SchemaCompileContext) -> bool {
        self.context
            .is_none_or(|source| source.same_session(context))
    }

    pub(crate) const fn identity(&self) -> &'src [u8] {
        self.identity
    }

    pub(crate) fn into_parts(self) -> (&'src [u8], R) {
        (self.identity, self.input)
    }
}

/// The finite error categories a host may encounter while resolving an imported source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResolverError {
    NotFound,
    ImportsNotSupported,
    ResourceLimitExceeded,
    AllocationFailed,
    CapacityExceeded,
    InternalInvariantFailure,
    HostFailure,
    ContextMismatch,
}

pub struct SchemaResolveRequest<'a> {
    pub from_identity: &'a [u8],
    pub module_name: &'a str,
    pub source_byte_limit: usize,
    pub context: &'a SchemaCompileContext,
}

/// Resolves the next Schema source through the host, using the current source identity and a single module name.
pub trait SchemaResolver {
    fn resolve<'src, 'slot>(
        &'src mut self,
        request: SchemaResolveRequest<'_>,
        out: Out<'slot, '_, ResolverError>,
    ) -> OResult<'slot, SchemaSource<'src>>;
}

/// Finite reasons for source-registration failure; no host paths or arbitrary error text are retained.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceObserverError {
    ResourceLimitExceeded,
    AllocationFailed,
    CapacityExceeded,
    InternalInvariantFailure,
    HostFailure,
}

pub struct SourceLoadedRequest<'a> {
    pub source_id: SourceId,
    pub identity: &'a [u8],
    pub context: &'a SchemaCompileContext,
}

/// Registers the identity of each actual source; failure immediately aborts compilation without resolving the current source or reading later imports.
///
/// Duplicate identities are not reported again, while sources that fail to resolve are still reported.
pub trait SourceObserver {
    fn source_loaded<'slot>(
        &mut self,
        request: SourceLoadedRequest<'_>,
        out: Out<'slot, '_, SourceObserverError>,
    ) -> OResult<'slot, ()>;
}

/// Resource ceilings for Schema loading, syntax trees, and default-value validation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchemaLimits {
    excerpts: ExcerptLimits,
    syntax: SyntaxLimits,
    stream: StreamLimits,
    values: ValueLimits,
    max_schema_total_bytes: usize,
    max_schema_files: usize,
}

impl SchemaLimits {
    pub const fn excerpts(&self) -> &ExcerptLimits {
        &self.excerpts
    }

    pub const fn syntax(&self) -> &SyntaxLimits {
        &self.syntax
    }

    /// Returns the input window and token length limits for Schema lexing.
    pub const fn stream_limits(&self) -> StreamLimits {
        self.stream
    }

    pub const fn values(&self) -> &ValueLimits {
        &self.values
    }

    pub const fn max_source_bytes(&self) -> usize {
        self.syntax.max_source_bytes()
    }

    pub const fn max_diagnostics(&self) -> usize {
        self.syntax.max_diagnostics()
    }

    pub const fn max_schema_total_bytes(&self) -> usize {
        self.max_schema_total_bytes
    }

    pub const fn max_schema_files(&self) -> usize {
        self.max_schema_files
    }

    pub const fn max_nesting_depth(&self) -> usize {
        self.syntax.max_nesting_depth()
    }

    /// Returns the maximum zero-based depth for value trees, including Schema defaults.
    pub const fn max_value_nesting_depth(&self) -> usize {
        self.values.max_nesting_depth()
    }

    pub const fn max_string_bytes(&self) -> usize {
        self.syntax.max_string_bytes()
    }

    pub const fn max_schema_ast_nodes(&self) -> usize {
        self.syntax.max_schema_ast_nodes()
    }

    pub const fn max_value_nodes(&self) -> usize {
        self.values.max_value_nodes()
    }

    /// Sets the input window and token length limits for Schema lexing.
    pub fn set_stream_limits(&mut self, stream: StreamLimits) {
        self.stream = stream;
    }

    pub fn set_excerpts(&mut self, limits: ExcerptLimits) {
        self.excerpts = limits;
    }

    pub fn set_max_source_bytes(&mut self, value: usize) {
        self.syntax.set_max_source_bytes(value);
    }

    pub fn set_max_diagnostics(&mut self, value: usize) {
        self.syntax.set_max_diagnostics(value);
    }

    pub fn set_max_schema_total_bytes(&mut self, value: usize) {
        self.max_schema_total_bytes = value;
    }

    pub fn set_max_schema_files(&mut self, value: usize) {
        self.max_schema_files = value;
    }

    pub fn set_max_nesting_depth(&mut self, value: usize) {
        self.syntax.set_max_nesting_depth(value);
        self.values.set_max_nesting_depth(value);
    }

    /// Sets the maximum depth for value trees, including Schema defaults, independently.
    pub fn set_max_value_nesting_depth(&mut self, value: usize) {
        self.values.set_max_nesting_depth(value);
    }

    pub fn set_max_string_bytes(&mut self, value: usize) {
        self.syntax.set_max_string_bytes(value);
        self.values.set_max_string_bytes(value);
    }

    pub fn set_max_schema_ast_nodes(&mut self, value: usize) {
        self.syntax.set_max_schema_ast_nodes(value);
    }

    pub fn set_max_value_nodes(&mut self, value: usize) {
        self.values.set_max_value_nodes(value);
    }
}

impl Default for SchemaLimits {
    fn default() -> Self {
        Self {
            excerpts: ExcerptLimits::default(),
            syntax: SyntaxLimits::default(),
            stream: StreamLimits::default(),
            values: ValueLimits::default(),
            max_schema_total_bytes: 8_388_608,
            max_schema_files: 64,
        }
    }
}

/// Schema resource and execution settings for one compile session.
#[derive(Clone, Copy, Debug)]
pub struct SchemaCompileOptions {
    limits: SchemaLimits,
    regex: RegexOptions,
    execution: ExecutionLimits,
}

impl SchemaCompileOptions {
    pub fn new(limits: SchemaLimits) -> Self {
        Self {
            limits,
            regex: RegexOptions::default(),
            execution: ExecutionLimits::new(67_108_864, 67_108_864),
        }
    }

    pub const fn limits(&self) -> &SchemaLimits {
        &self.limits
    }

    pub const fn regex(&self) -> &RegexOptions {
        &self.regex
    }

    pub const fn with_regex(mut self, regex: RegexOptions) -> Self {
        self.regex = regex;
        self
    }

    pub const fn execution(&self) -> ExecutionLimits {
        self.execution
    }

    pub const fn with_execution(mut self, execution: ExecutionLimits) -> Self {
        self.execution = execution;
        self
    }

    /// Sets the maximum nesting depth for Schema syntax and value trees.
    pub fn with_max_nesting_depth(mut self, value: usize) -> Self {
        self.limits.set_max_nesting_depth(value);
        self
    }

    /// Sets the maximum zero-based depth for Schema type expressions.
    pub fn with_max_typed_depth(self, value: usize) -> Self {
        self.with_max_nesting_depth(value.saturating_add(TYPE_DECLARATION_DEPTH_OVERHEAD))
    }
}

impl Default for SchemaCompileOptions {
    fn default() -> Self {
        Self::new(SchemaLimits::default())
    }
}
