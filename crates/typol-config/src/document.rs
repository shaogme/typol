mod edit;
mod encoding;
mod formatter;
mod path;
mod syntax;

use core::{
    fmt::{self, Debug, Formatter},
    mem::{replace, swap},
};
use opeo::{ErrSlot, OResult, Out};
use typol_core::{
    BudgetError, ChargedString, ChargedVec, ConfigValue, Diagnostic, DiagnosticCode,
    DiagnosticContext, ExecutionBudget, SliceSource, SourceCommitError, SourceExpectation,
    SourceRevision, SourceWriter, TextLayout, Write, WriteError, diagnostic_context,
};
use typol_semantic::CompiledSchema;

use self::edit::{DocumentEditEngine, EditContext, SourcePatch};
use crate::{
    ConfigParseErrors, ConfigParseOptions, ConfigParseRequest, ConfigParser, Environment,
    ValidatedConfigValue, instance::LocationIndex,
};
use encoding::{ScalarEncoder, ValueEncodingContext};
use path::{PathLookup, SourcePathIndex};
use syntax::SourceSyntax;

pub use crate::document::edit::{
    ConfigEdit, ConfigEditError, ConfigEditErrorKind, ConfigEditOptions,
};
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
    snapshot: Option<ChargedString>,
    value: ValidatedConfigValue,
    index: SourcePathIndex,
}

/// Formats a validated configuration document while preserving its source tokens and comments.
pub struct ConfigFormatter;

pub struct ConfigFieldInsertion<'a> {
    path: &'a ConfigPath,
    value: &'a ConfigValue,
}

impl<'a> ConfigFieldInsertion<'a> {
    pub const fn new(path: &'a ConfigPath, value: &'a ConfigValue) -> Self {
        Self { path, value }
    }
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
                budget: document.workspace_budget(),
                source_limit: document.options.config().max_source_bytes(),
                constructed_text_limit: document.options.config().max_constructed_text_bytes(),
                options,
            },
            out,
        )
    }

    /// Checks whether a document already matches the canonical format without retaining it.
    pub fn is_formatted<'slot>(
        document: &ConfigDocument<'_>,
        options: ConfigFormatOptions,
        out: Out<'slot, '_, ConfigFormatError>,
    ) -> OResult<'slot, bool> {
        formatter::FormatterEngine::matches_source(
            formatter::FormatRequest {
                source: document.source(),
                budget: document.workspace_budget(),
                source_limit: document.options.config().max_source_bytes(),
                constructed_text_limit: document.options.config().max_constructed_text_bytes(),
                options,
            },
            out,
        )
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
        let source = ChargedString::from_utf8_bytes(source).map_err(|(_bytes, _error)| {
            out.reborrow().fail(ConfigParseErrors::emergency(
                Diagnostic::emergency(DiagnosticCode::INTERNAL_ERROR),
                schema.next_source_id(),
            ))
        })?;
        let index =
            SourcePathIndex::new(&locations, value.as_config_value(), budget, out.reborrow())?;
        Ok(ConfigDocument {
            schema,
            options,
            display_name,
            source,
            snapshot: None,
            value,
            index,
        })
    }

    pub fn source(&self) -> &str {
        self.source.as_str()
    }

    fn workspace_budget(&self) -> Option<&ExecutionBudget> {
        self.snapshot
            .as_ref()
            .and_then(ChargedString::budget)
            .or_else(|| self.source.budget())
    }

    fn consume_path_steps<'slot>(
        &self,
        path: &ConfigPath,
        out: Out<'slot, '_, ConfigEditError>,
    ) -> OResult<'slot, ()> {
        let Some(budget) = self.workspace_budget() else {
            return Ok(());
        };
        let index_size = self.index.len();
        let search_bound = (usize::BITS - index_size.leading_zeros()) as usize;
        let steps = path.segment_count().saturating_mul(search_bound.max(1));
        budget.consume(steps, out)
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

    pub const fn source_expectation(&self) -> SourceExpectation<'static> {
        SourceExpectation::DocumentSnapshot
    }

    pub fn write_to(&self, output: &mut dyn Write) -> Result<(), WriteError> {
        output.write_all(self.source.as_bytes())
    }

    pub fn set_value<'slot>(
        &mut self,
        replacement: ConfigReplacement<'_>,
        options: ConfigEditOptions,
        environment: &mut dyn Environment,
        out: Out<'slot, '_, ConfigEditError>,
    ) -> OResult<'slot, ()> {
        let replacements = [replacement];
        self.replace_values(&replacements, options, environment, out)
    }

    /// Applies non-overlapping edits as one parse-and-validation transaction.
    pub fn replace_values<'slot>(
        &mut self,
        replacements: &[ConfigReplacement<'_>],
        options: ConfigEditOptions,
        environment: &mut dyn Environment,
        mut out: Out<'slot, '_, ConfigEditError>,
    ) -> OResult<'slot, ()> {
        if let Some(budget) = self.workspace_budget() {
            budget.consume(replacements.len(), out.reborrow())?;
        }
        let source = self.source.as_str();
        let line_break = SourceSyntax::edit_line_break(
            source,
            self.workspace_budget(),
            options.layout(),
            !replacements.is_empty(),
        )
        .map_err(|kind| out.reborrow().fail(ConfigEditError::new(kind)))?;
        let mut patches = ChargedVec::new(self.workspace_budget(), out.reborrow())?;
        for replacement in replacements {
            self.consume_path_steps(replacement.path(), out.reborrow())?;
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
            let context = self
                .value_context(
                    start,
                    self.index.parent_id(replacement.path()),
                    options,
                    line_break,
                )
                .map_err(|kind| out.reborrow().fail(ConfigEditError::new(kind)))?;
            let mut encoded = ScalarEncoder::encode_value(
                replacement.value().root(),
                context,
                self.workspace_budget(),
                self.options.config().max_source_bytes(),
            )
            .map_err(|kind| out.reborrow().fail(ConfigEditError::new(kind)))?;
            let comments = SourceSyntax::comments_in(source, start, end, self.workspace_budget())
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
        options: ConfigEditOptions,
        environment: &mut dyn Environment,
        mut out: Out<'slot, '_, ConfigEditError>,
    ) -> OResult<'slot, ()> {
        let patches = DocumentEditEngine::new(EditContext {
            source: self.source.as_str(),
            index: &self.index,
            budget: self.workspace_budget(),
            source_limit: self.options.config().max_source_bytes(),
            options,
        })
        .resolve(edits, out.reborrow())?;
        self.commit_patches(patches, environment, out)
    }

    pub fn insert_field<'slot>(
        &mut self,
        insertion: ConfigFieldInsertion<'_>,
        options: ConfigEditOptions,
        environment: &mut dyn Environment,
        out: Out<'slot, '_, ConfigEditError>,
    ) -> OResult<'slot, ()> {
        self.apply_edits(
            &[ConfigEdit::InsertField {
                path: insertion.path,
                value: insertion.value,
            }],
            options,
            environment,
            out,
        )
    }

    pub fn remove<'slot>(
        &mut self,
        path: &ConfigPath,
        environment: &mut dyn Environment,
        out: Out<'slot, '_, ConfigEditError>,
    ) -> OResult<'slot, ()> {
        self.apply_edits(
            &[ConfigEdit::Remove { path }],
            ConfigEditOptions::default(),
            environment,
            out,
        )
    }

    fn value_context(
        &self,
        position: usize,
        fallback_parent: Option<usize>,
        options: ConfigEditOptions,
        line_break: &'static str,
    ) -> Result<ValueEncodingContext<'_>, ConfigEditErrorKind> {
        if options.layout() == TextLayout::Compact {
            return Ok(ValueEncodingContext {
                layout: TextLayout::Compact,
                line_break,
                indent: "",
                indent_unit: "",
                indent_levels: 0,
                depth: 0,
            });
        }
        let (indent, depth) = match SourceSyntax::indent_context(
            self.source.as_str(),
            position,
            self.workspace_budget(),
        ) {
            Ok(context) => context,
            Err(ConfigEditErrorKind::EditConflict) => (
                "",
                fallback_parent.map_or(0, |id| self.index.container_depth(id)),
            ),
            Err(kind) => return Err(kind),
        };
        Ok(ValueEncodingContext {
            layout: options.layout(),
            line_break,
            indent,
            indent_unit: "    ",
            indent_levels: 0,
            depth,
        })
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
        patches.sort_by(
            |left, right, mut sort_out| {
                if let Some(budget) = self.workspace_budget() {
                    budget.consume(1, sort_out.reborrow())?;
                }
                Ok(left.start.cmp(&right.start))
            },
            out.reborrow(),
        )?;
        let mut removed_bytes = 0usize;
        let mut added_bytes = 0usize;
        let mut previous_end = 0usize;
        let mut previous_start = None;
        for (index, patch) in patches.iter().enumerate() {
            if let Some(budget) = self.workspace_budget() {
                budget.consume(1, out.reborrow())?;
            }
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
        let Some(budget) = self.workspace_budget() else {
            return Err(out.fail(ConfigEditError::new(
                ConfigEditErrorKind::ResourceLimitExceeded,
            )));
        };
        budget.construct_text(
            candidate_len,
            self.options.config().max_constructed_text_bytes(),
            out.reborrow(),
        )?;
        let mut budget_errors = ErrSlot::<BudgetError>::new();
        let candidate = match budget_errors.try_call(|mut budget_out| {
            let mut candidate = ChargedVec::new(Some(budget), budget_out.reborrow())?;
            candidate.try_reserve_exact(candidate_len, budget_out.reborrow())?;
            let mut cursor = 0usize;
            for patch in &patches {
                budget.consume(1, budget_out.reborrow())?;
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
        let mut parse_errors = ErrSlot::new();
        let parsed = {
            let mut input = SliceSource::new(candidate.as_slice());
            let request =
                ConfigParseRequest::new(&mut input, self.display_name.as_str(), self.options);
            parse_errors.try_call(|parse_out| {
                ConfigParser::new(self.schema).parse_document_without_source_retention_with_budget(
                    request,
                    environment,
                    budget,
                    parse_out,
                )
            })
        };
        let parsed = match parsed {
            Ok(parsed) => parsed,
            Err(caught) => {
                let errors = caught.take();
                let kind = validation_error_kind(&errors);
                return Err(out.fail(ConfigEditError::validation(kind, errors)));
            }
        };
        let mut document_errors = ErrSlot::new();
        let updated = document_errors.try_call(|document_out| {
            ConfigDocument::from_parse(
                ConfigDocumentInput {
                    schema: self.schema,
                    options: self.options,
                    display_name: self.display_name.as_str(),
                    value: parsed.value,
                    locations: parsed.locations,
                    source: candidate,
                    budget,
                },
                document_out,
            )
        });
        let mut updated = match updated {
            Ok(document) => document,
            Err(caught) => {
                let errors = caught.take();
                let kind = validation_error_kind(&errors);
                return Err(out.fail(ConfigEditError::validation(kind, errors)));
            }
        };
        let mut budget_errors = ErrSlot::<BudgetError>::new();
        let empty_source = match budget_errors
            .try_call(|budget_out| ChargedString::with_capacity(0, None, budget_out))
        {
            Ok(source) => source,
            Err(caught) => {
                return Err(out.fail(ConfigEditError::new(edit_budget_error(caught.take()))));
            }
        };
        swap(&mut self.source, &mut updated.source);
        let previous_source = replace(&mut updated.source, empty_source);
        updated.snapshot = match self.snapshot.take() {
            Some(snapshot) => {
                drop(previous_source);
                Some(snapshot)
            }
            None => Some(previous_source),
        };
        swap(&mut self.source, &mut updated.source);
        *self = updated;
        Ok(())
    }

    /// Writes the retained source through a backend that provides atomic replacement semantics.
    pub fn write_source<W: SourceWriter>(
        &mut self,
        target: &W::Target,
        writer: &mut W,
        expectation: SourceExpectation<'_>,
    ) -> Result<SourceRevision, SourceCommitError<W::Error>> {
        let max_bytes = self.options.config().max_source_bytes();
        if self.source.len() > max_bytes {
            return Err(SourceCommitError::ResourceLimitExceeded);
        }
        let snapshot = self
            .snapshot
            .as_ref()
            .map_or(self.source.as_bytes(), |snapshot| snapshot.as_bytes());
        let expectation = match expectation {
            SourceExpectation::DocumentSnapshot => SourceExpectation::ExactSnapshot(snapshot),
            expectation => expectation,
        };
        let revision = writer
            .replace_atomically(target, expectation, self.source.as_bytes(), max_bytes)
            .map_err(SourceCommitError::Writer)?;
        self.snapshot = None;
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

fn validation_error_kind(errors: &ConfigParseErrors) -> ConfigEditErrorKind {
    let diagnostics = errors.diagnostics();
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code() == DiagnosticCode::ALLOCATION_FAILED)
    {
        ConfigEditErrorKind::AllocationFailed
    } else if diagnostics.iter().any(|diagnostic| {
        matches!(
            diagnostic.code(),
            DiagnosticCode::RESOURCE_LIMIT_EXCEEDED | DiagnosticCode::CAPACITY_EXCEEDED
        )
    }) {
        ConfigEditErrorKind::ResourceLimitExceeded
    } else {
        ConfigEditErrorKind::ValidationFailed
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
        BudgetError::Resource
        | BudgetError::RegexSteps
        | BudgetError::RegexSearchSteps
        | BudgetError::RegexWorkspace
        | BudgetError::CapacityExceeded => ConfigEditErrorKind::ResourceLimitExceeded,
        BudgetError::Allocation | BudgetError::InvariantViolation(_) => {
            ConfigEditErrorKind::AllocationFailed
        }
    }
}
