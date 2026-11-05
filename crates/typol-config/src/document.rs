mod edit;
mod encoding;
mod formatter;
mod path;
mod syntax;

use core::{
    error::Error,
    fmt::{self, Debug, Display, Formatter},
    mem::swap,
    str,
};
use opeo::{ErrSlot, OResult, Out};
use typol_core::{
    BudgetError, ChargedString, ChargedVec, ConfigValue, Diagnostic, DiagnosticCode,
    DiagnosticContext, ExecutionBudget, SliceSource, Write, WriteError, diagnostic_context,
};
use typol_semantic::CompiledSchema;

use crate::{
    ConfigParseErrors, ConfigParseOptions, ConfigParseRequest, ConfigParser, Environment,
    ValidatedConfigValue, instance::LocationIndex,
};
use edit::{DocumentEditEngine, EditContext, SourcePatch};
use encoding::ScalarEncoder;
use path::{PathLookup, SourcePathIndex};
use syntax::SourceSyntax;

pub use edit::{ConfigEdit, ConfigEditError};
pub use encoding::ConfigEditErrorKind;
pub use formatter::{
    ConfigFormatError, ConfigFormatErrorKind, ConfigFormatOptions, FormattedConfig,
};
pub use path::{ConfigPath, ConfigPathSegment, ConfigReplacement};

static EDIT_SOURCE_RANGE_INVALID: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "config-document-edit",
    "source-range-invalid"
);

/// A validated configuration value together with its exact retained source text.
pub struct ConfigDocument<'schema> {
    schema: &'schema CompiledSchema,
    options: ConfigParseOptions,
    display_name: ChargedString,
    source: ChargedString,
    snapshot: ChargedVec<u8>,
    value: ValidatedConfigValue,
    index: SourcePathIndex,
}

/// Formats a validated configuration document while preserving its source tokens and comments.
pub struct ConfigFormatter;

/// The target state required before an atomic source replacement.
pub enum ConfigSourceExpectation<'snapshot> {
    /// Require exact byte equality with the supplied prior version.
    ExactSnapshot(&'snapshot [u8]),
    /// Use the snapshot retained by this document as the expected prior version.
    DocumentSnapshot,
    /// Require that the target does not exist.
    MustBeAbsent,
    /// Replace any existing target explicitly.
    ReplaceAny,
}

/// A backend revision returned after an atomic source replacement succeeds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConfigSourceRevision(u64);

/// A failure reported by a source writer or by the bounded document layer.
pub enum ConfigSourceCommitError<E> {
    ResourceLimitExceeded,
    AllocationFailed,
    Writer(E),
}

/// Performs bounded, transactional replacement of a platform-specific source target.
///
/// An implementation must keep the target unchanged for failures before the atomic commit and
/// return success only after the complete replacement is committed. If durability confirmation
/// can fail after visibility changes, the error must identify that the commit occurred and that
/// durability is uncertain.
pub trait ConfigSourceWriter {
    type Target: ?Sized;
    type Error;

    fn replace_atomically(
        &mut self,
        target: &Self::Target,
        expectation: ConfigSourceExpectation<'_>,
        replacement: &[u8],
        max_bytes: usize,
    ) -> Result<ConfigSourceRevision, Self::Error>;
}

pub(crate) struct ConfigDocumentInput<'schema, 'display, 'budget> {
    pub(crate) schema: &'schema CompiledSchema,
    pub(crate) options: ConfigParseOptions,
    pub(crate) display_name: &'display str,
    pub(crate) value: ValidatedConfigValue,
    pub(crate) locations: LocationIndex<'schema>,
    pub(crate) source: ChargedVec<u8>,
    pub(crate) budget: &'budget ExecutionBudget,
}

impl ConfigSourceRevision {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u64 {
        self.0
    }
}

impl ConfigFormatter {
    /// Produces an independently owned canonical representation of a parsed document.
    pub fn format<'slot>(
        document: &ConfigDocument<'_>,
        options: ConfigFormatOptions,
        out: Out<'slot, '_, ConfigFormatError>,
    ) -> OResult<'slot, FormattedConfig> {
        formatter::FormatterEngine::format_source(
            formatter::FormatRequest {
                source: document.source(),
                budget: document.snapshot.budget(),
                source_limit: document.options.config().max_source_bytes(),
                constructed_text_limit: document.options.config().max_constructed_text_bytes(),
                options,
            },
            out,
        )
    }
}

impl<E> Debug for ConfigSourceCommitError<E> {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::ResourceLimitExceeded => formatter.write_str("ResourceLimitExceeded"),
            Self::AllocationFailed => formatter.write_str("AllocationFailed"),
            Self::Writer(_) => formatter.write_str("Writer(..)"),
        }
    }
}

impl<E: Display> Display for ConfigSourceCommitError<E> {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::ResourceLimitExceeded => {
                formatter.write_str("configuration source write limit exceeded")
            }
            Self::AllocationFailed => {
                formatter.write_str("configuration source write allocation failed")
            }
            Self::Writer(error) => Display::fmt(error, formatter),
        }
    }
}

impl<E: Error + 'static> Error for ConfigSourceCommitError<E> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Writer(error) => Some(error),
            Self::ResourceLimitExceeded | Self::AllocationFailed => None,
        }
    }
}

impl<'schema> ConfigDocument<'schema> {
    pub(crate) fn from_parse<'slot>(
        request: ConfigDocumentInput<'schema, '_, '_>,
        mut out: Out<'slot, '_, ConfigParseErrors>,
    ) -> OResult<'slot, ConfigDocument<'schema>> {
        let ConfigDocumentInput {
            schema,
            options,
            display_name,
            value,
            locations,
            source,
            budget,
        } = request;
        let display_name = ChargedString::copy(display_name, Some(budget), out.reborrow())?;
        let snapshot = source;
        let source_text = str::from_utf8(snapshot.as_slice()).map_err(|_| {
            out.reborrow().fail(ConfigParseErrors::emergency(
                Diagnostic::emergency(DiagnosticCode::INTERNAL_ERROR),
                schema.next_source_id(),
            ))
        })?;
        let source = ChargedString::copy(source_text, Some(budget), out.reborrow())?;
        let index =
            SourcePathIndex::new(&locations, value.as_config_value(), budget, out.reborrow())?;
        Ok(ConfigDocument {
            schema,
            options,
            display_name,
            source,
            snapshot,
            value,
            index,
        })
    }

    pub fn source(&self) -> &str {
        self.source.as_str()
    }

    pub fn as_config_value(&self) -> &ConfigValue {
        self.value.as_config_value()
    }

    pub fn validated_value(&self) -> &ValidatedConfigValue {
        &self.value
    }

    /// Returns whether a path has a value written explicitly in the retained source.
    pub fn is_explicit(&self, path: &ConfigPath) -> bool {
        matches!(self.index.lookup(path), PathLookup::Explicit(_))
    }

    pub const fn source_expectation(&self) -> ConfigSourceExpectation<'static> {
        ConfigSourceExpectation::DocumentSnapshot
    }

    pub fn write_to(&self, output: &mut dyn Write) -> Result<(), WriteError> {
        output.write_all(self.source.as_bytes())
    }

    pub fn set_value<'slot>(
        &mut self,
        path: &ConfigPath,
        replacement: &ConfigValue,
        environment: &mut dyn Environment,
        out: Out<'slot, '_, ConfigEditError>,
    ) -> OResult<'slot, ()> {
        let replacements = [ConfigReplacement::new(path, replacement)];
        self.replace_values(&replacements, environment, out)
    }

    /// Applies non-overlapping edits as one parse-and-validation transaction.
    pub fn replace_values<'slot>(
        &mut self,
        replacements: &[ConfigReplacement<'_>],
        environment: &mut dyn Environment,
        mut out: Out<'slot, '_, ConfigEditError>,
    ) -> OResult<'slot, ()> {
        let source = self.source.as_str();
        let mut patches = ChargedVec::new(self.snapshot.budget(), out.reborrow())?;
        for replacement in replacements {
            let span = match self.index.lookup(replacement.path()) {
                PathLookup::Missing => {
                    return Err(out.fail(ConfigEditError::new(ConfigEditErrorKind::InvalidPath)));
                }
                PathLookup::Explicit(span) => span,
            };
            let start = span.start();
            let end = span.end();
            if start > end
                || end > source.len()
                || !source.is_char_boundary(start)
                || !source.is_char_boundary(end)
            {
                return Err(out.fail(ConfigEditError::new(ConfigEditErrorKind::InvalidPath)));
            }
            let mut encoded = ScalarEncoder::encode_value(
                replacement.value().root(),
                self.snapshot.budget(),
                self.options.config().max_source_bytes(),
            )
            .map_err(|kind| out.reborrow().fail(ConfigEditError::new(kind)))?;
            let comments = SourceSyntax::comments_in(source, start, end, self.snapshot.budget())
                .map_err(|kind| out.reborrow().fail(ConfigEditError::new(kind)))?;
            append_charged(
                &mut encoded,
                comments.as_str(),
                self.options.config().max_source_bytes(),
            )
            .map_err(|kind| out.reborrow().fail(ConfigEditError::new(kind)))?;
            patches.try_push(
                SourcePatch {
                    start,
                    end,
                    encoded,
                },
                out.reborrow(),
            )?;
        }
        self.commit_patches(patches, environment, out)
    }

    /// Applies path-based replacements, insertions, and removals in one validation transaction.
    pub fn apply_edits<'slot>(
        &mut self,
        edits: &[ConfigEdit<'_>],
        environment: &mut dyn Environment,
        mut out: Out<'slot, '_, ConfigEditError>,
    ) -> OResult<'slot, ()> {
        let patches = DocumentEditEngine::new(EditContext {
            source: self.source.as_str(),
            index: &self.index,
            budget: self.snapshot.budget(),
            source_limit: self.options.config().max_source_bytes(),
        })
        .resolve(edits, out.reborrow())?;
        self.commit_patches(patches, environment, out)
    }

    pub fn insert_field<'slot>(
        &mut self,
        path: &ConfigPath,
        value: &ConfigValue,
        environment: &mut dyn Environment,
        out: Out<'slot, '_, ConfigEditError>,
    ) -> OResult<'slot, ()> {
        self.apply_edits(&[ConfigEdit::InsertField { path, value }], environment, out)
    }

    pub fn remove<'slot>(
        &mut self,
        path: &ConfigPath,
        environment: &mut dyn Environment,
        out: Out<'slot, '_, ConfigEditError>,
    ) -> OResult<'slot, ()> {
        self.apply_edits(&[ConfigEdit::Remove { path }], environment, out)
    }

    fn commit_patches<'slot>(
        &mut self,
        mut patches: ChargedVec<SourcePatch>,
        environment: &mut dyn Environment,
        mut out: Out<'slot, '_, ConfigEditError>,
    ) -> OResult<'slot, ()> {
        if patches.is_empty() {
            return Ok(());
        }
        let source = self.source.as_str();
        patches.sort_unstable_by_key(|patch| patch.start);
        let mut removed_bytes = 0usize;
        let mut added_bytes = 0usize;
        let mut previous_end = 0usize;
        let mut previous_start = None;
        for (index, patch) in patches.iter().enumerate() {
            if index > 0 && (patch.start < previous_end || previous_start == Some(patch.start)) {
                return Err(out.fail(ConfigEditError::new(ConfigEditErrorKind::EditConflict)));
            }
            previous_start = Some(patch.start);
            previous_end = patch.end;
            removed_bytes = removed_bytes
                .checked_add(patch.end - patch.start)
                .ok_or_else(|| {
                    out.reborrow().fail(ConfigEditError::new(
                        ConfigEditErrorKind::ResourceLimitExceeded,
                    ))
                })?;
            added_bytes = added_bytes
                .checked_add(patch.encoded.len())
                .ok_or_else(|| {
                    out.reborrow().fail(ConfigEditError::new(
                        ConfigEditErrorKind::ResourceLimitExceeded,
                    ))
                })?;
        }
        let Some(candidate_len) = source
            .len()
            .checked_sub(removed_bytes)
            .and_then(|length| length.checked_add(added_bytes))
        else {
            return Err(out.fail(ConfigEditError::new(
                ConfigEditErrorKind::ResourceLimitExceeded,
            )));
        };
        if candidate_len > self.options.config().max_source_bytes() {
            return Err(out.fail(ConfigEditError::new(
                ConfigEditErrorKind::ResourceLimitExceeded,
            )));
        }
        let mut budget_errors = ErrSlot::<BudgetError>::new();
        let candidate = match budget_errors.try_call(|mut budget_out| {
            let mut candidate = ChargedVec::new(self.snapshot.budget(), budget_out.reborrow())?;
            candidate.try_reserve_exact(candidate_len, budget_out.reborrow())?;
            let mut cursor = 0usize;
            for patch in &patches {
                let Some(original) = source.as_bytes().get(cursor..patch.start) else {
                    return Err(budget_out.fail(BudgetError::invariant(&EDIT_SOURCE_RANGE_INVALID)));
                };
                candidate.extend_from_slice(original, budget_out.reborrow())?;
                candidate.extend_from_slice(patch.encoded.as_bytes(), budget_out.reborrow())?;
                cursor = patch.end;
            }
            let Some(original) = source.as_bytes().get(cursor..) else {
                return Err(budget_out.fail(BudgetError::invariant(&EDIT_SOURCE_RANGE_INVALID)));
            };
            candidate.extend_from_slice(original, budget_out.reborrow())?;
            Ok(candidate)
        }) {
            Ok(candidate) => candidate,
            Err(caught) => {
                return Err(out.fail(ConfigEditError::new(edit_budget_error(caught.take()))));
            }
        };
        let mut input = SliceSource::new(candidate.as_slice());
        let request = ConfigParseRequest::new(&mut input, self.display_name.as_str(), self.options);
        let mut parse_errors = ErrSlot::new();
        let parsed = parse_errors.try_call(|parse_out| {
            ConfigParser::new(self.schema).parse_document(request, environment, parse_out)
        });
        let mut updated = match parsed {
            Ok(document) => document,
            Err(caught) => return Err(out.fail(ConfigEditError::validation(caught.take()))),
        };
        swap(&mut updated.snapshot, &mut self.snapshot);
        *self = updated;
        Ok(())
    }

    /// Writes the retained source through a backend that provides atomic replacement semantics.
    pub fn write_source<W: ConfigSourceWriter>(
        &mut self,
        target: &W::Target,
        writer: &mut W,
        expectation: ConfigSourceExpectation<'_>,
    ) -> Result<ConfigSourceRevision, ConfigSourceCommitError<W::Error>> {
        let max_bytes = self.options.config().max_source_bytes();
        if self.source.len() > max_bytes {
            return Err(ConfigSourceCommitError::ResourceLimitExceeded);
        }
        let mut errors = ErrSlot::<BudgetError>::new();
        let snapshot = match errors.try_call(|mut budget_out| {
            let mut snapshot = ChargedVec::new(self.snapshot.budget(), budget_out.reborrow())?;
            snapshot.extend_from_slice(self.source.as_bytes(), budget_out.reborrow())?;
            Ok(snapshot)
        }) {
            Ok(snapshot) => snapshot,
            Err(caught) => {
                return Err(match caught.take() {
                    BudgetError::Resource => ConfigSourceCommitError::ResourceLimitExceeded,
                    BudgetError::Allocation | BudgetError::CapacityExceeded => {
                        ConfigSourceCommitError::AllocationFailed
                    }
                    BudgetError::InvariantViolation(_) => ConfigSourceCommitError::AllocationFailed,
                });
            }
        };
        let expectation = match expectation {
            ConfigSourceExpectation::DocumentSnapshot => {
                ConfigSourceExpectation::ExactSnapshot(self.snapshot.as_slice())
            }
            expectation => expectation,
        };
        let revision = writer
            .replace_atomically(target, expectation, self.source.as_bytes(), max_bytes)
            .map_err(ConfigSourceCommitError::Writer)?;
        self.snapshot = snapshot;
        Ok(revision)
    }
}

impl Debug for ConfigDocument<'_> {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConfigDocument")
            .field("source_bytes", &self.source.len())
            .field("source_nodes", &self.index.len())
            .field("value_nodes", &self.value.as_config_value().node_count())
            .finish()
    }
}

fn append_charged(
    output: &mut ChargedString,
    text: &str,
    limit: usize,
) -> Result<(), ConfigEditErrorKind> {
    if output
        .len()
        .checked_add(text.len())
        .filter(|length| *length <= limit)
        .is_none()
    {
        return Err(ConfigEditErrorKind::ResourceLimitExceeded);
    }
    let mut errors = ErrSlot::<BudgetError>::new();
    errors
        .try_call(|out| output.try_push_str(text, out))
        .map_err(|caught| edit_budget_error(caught.take()))?;
    Ok(())
}

fn edit_budget_error(error: BudgetError) -> ConfigEditErrorKind {
    match error {
        BudgetError::Resource | BudgetError::CapacityExceeded => {
            ConfigEditErrorKind::ResourceLimitExceeded
        }
        BudgetError::Allocation | BudgetError::InvariantViolation(_) => {
            ConfigEditErrorKind::AllocationFailed
        }
    }
}
