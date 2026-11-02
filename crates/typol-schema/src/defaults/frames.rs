use crate::{
    defaults::compiler::DefaultCompiler,
    diagnostics::BuildFailure,
    symbols::{FieldSymbolId, LookupResult, TypeSymbolId, VariantSymbolId},
    types::TypeUseId,
};
use core::cmp::Ordering;
use opeo::{OResult, Out};
use typol_core::{ChargedVec, ConfigValue, SourceSpan};
use typol_i18n::diagnostic_message;
use typol_syntax::{AstAccessError, BraceEntry, BraceEntryList, BraceKey};

#[derive(Clone, Copy, Eq, PartialEq)]
enum ParsedBraceKeyKind {
    String,
    Identifier,
}

#[derive(Clone, Copy)]
struct ParsedBraceKey<'a> {
    value: &'a str,
    span: SourceSpan,
    kind: ParsedBraceKeyKind,
}

#[derive(Clone, Copy)]
pub(super) enum RecordOwner {
    Struct(TypeSymbolId),
    Variant(VariantSymbolId),
}

#[derive(Clone, Copy)]
pub(super) struct MapKeyRef<'a> {
    pub(super) key: &'a str,
    pub(super) span: SourceSpan,
    pub(super) index: usize,
}

pub(super) struct NamedDefault<'a> {
    pub(super) name: &'a str,
    pub(super) value: ConfigValue,
}

pub(super) struct MapBuild<'a> {
    pub(super) entries: BraceEntryList<'a>,
    pub(super) target: TypeUseId,
    pub(super) span: SourceSpan,
    pub(super) depth: usize,
    pub(super) index: usize,
    pub(super) valid: bool,
    pub(super) values: ChargedVec<NamedDefault<'a>>,
    pub(super) spans: ChargedVec<SourceSpan>,
}

pub(super) struct RecordBuild<'a> {
    pub(super) fields: &'a [FieldSymbolId],
    pub(super) supplied: ChargedVec<Option<BraceEntry<'a>>>,
    pub(super) span: SourceSpan,
    pub(super) depth: usize,
    pub(super) index: usize,
    pub(super) valid: bool,
    pub(super) values: ChargedVec<NamedDefault<'a>>,
    pub(super) waiting: Option<(FieldSymbolId, bool)>,
}

pub(super) struct PrepareMapInput<'a> {
    pub(super) entries: BraceEntryList<'a>,
    pub(super) target: TypeUseId,
    pub(super) span: SourceSpan,
    pub(super) depth: usize,
}

pub(super) struct PrepareRecordInput<'a> {
    pub(super) entries: BraceEntryList<'a>,
    pub(super) owner: RecordOwner,
    pub(super) span: SourceSpan,
    pub(super) depth: usize,
}

pub(super) struct DefaultFrames<'b, 'a, 'd> {
    compiler: &'b mut DefaultCompiler<'a, 'd>,
}

impl<'b, 'a, 'd> DefaultFrames<'b, 'a, 'd> {
    pub(super) fn new(compiler: &'b mut DefaultCompiler<'a, 'd>) -> Self {
        Self { compiler }
    }

    fn ast_access<'slot, 'borrow, T>(
        &self,
        out: Out<'slot, 'borrow, BuildFailure>,
        operation: impl for<'call> FnOnce(Out<'call, 'call, AstAccessError>) -> OResult<'call, T>,
    ) -> OResult<'slot, T> {
        self.compiler.ast_access(out, operation)
    }

    pub(super) fn prepare_map<'slot>(
        &mut self,
        input: PrepareMapInput<'a>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, MapBuild<'a>> {
        let PrepareMapInput {
            entries,
            target,
            span,
            depth,
        } = input;
        let mut keys = self.compiler.new_vec(out.reborrow())?;
        keys.try_reserve_exact(entries.len(), out.reborrow())?;
        for index in 0..entries.len() {
            let entry = self.ast_access(out.reborrow(), |ast_out| entries.get(index, ast_out))?;
            let Some(entry) = entry else {
                self.compiler.diagnostics.abort(schema_internal!(span));
                return Err(out.fail(BuildFailure::empty()));
            };
            self.compiler
                .context
                .consume(1, span, self.compiler.diagnostics, out.reborrow())?;
            let parsed_key = self.ast_access(out.reborrow(), |mut ast_out| {
                let entry_key = entry.key(ast_out.reborrow())?;
                let kind = match entry_key {
                    BraceKey::String(_) => ParsedBraceKeyKind::String,
                    BraceKey::Identifier(_) => ParsedBraceKeyKind::Identifier,
                };
                let (key, key_span) = brace_key(entry_key, ast_out.reborrow())?;
                Ok(ParsedBraceKey {
                    value: key,
                    span: key_span,
                    kind,
                })
            })?;
            let key = parsed_key.value;
            let key_span = parsed_key.span;
            let key_is_string = parsed_key.kind == ParsedBraceKeyKind::String;
            let key_valid = self.compiler.check_string(key, key_span, out.reborrow())?;
            if !key_is_string {
                self.compiler.report_invalid(
                    diagnostic_message!(
                        "diagnostic.schema.default_map_key_requires_string",
                        "map default keys must be strings"
                    ),
                    key_span,
                    out.reborrow(),
                )?;
            }
            self.compiler.add_value_node(key_span, out.reborrow())?;
            if key_valid && key_is_string {
                keys.try_push(
                    MapKeyRef {
                        key,
                        span: key_span,
                        index,
                    },
                    out.reborrow(),
                )?;
            }
        }
        let mut sorted_keys = self.compiler.new_vec(out.reborrow())?;
        sorted_keys.try_reserve_exact(keys.len(), out.reborrow())?;
        sorted_keys.extend_from_slice(&keys, out.reborrow())?;
        let context = self.compiler.context;
        let diagnostics = &mut self.compiler.diagnostics;
        context.sort(
            &mut sorted_keys,
            |left, right, mut sort_out| {
                context.consume(1, span, diagnostics, sort_out.reborrow())?;
                let order = context.compare_text(left.key, right.key, sort_out.reborrow())?;
                Ok(order.then(left.index.cmp(&right.index)))
            },
            out.reborrow(),
        )?;
        let mut valid = keys.len() == entries.len();
        for pair in sorted_keys.windows(2) {
            let [left, right] = pair else {
                self.compiler.diagnostics.abort(schema_internal!(span));
                return Err(out.fail(BuildFailure::empty()));
            };
            if self
                .compiler
                .context
                .compare_text(left.key, right.key, out.reborrow())?
                .is_eq()
            {
                self.compiler.report_invalid(
                    diagnostic_message!(
                        "diagnostic.schema.default_map_duplicate_key",
                        "map default contains a duplicate key"
                    ),
                    right.span,
                    out.reborrow(),
                )?;
                valid = false;
            }
        }

        let values = self.value_buffer(entries.len(), span, out.reborrow())?;
        let spans = self.buffer(entries.len(), span, out.reborrow())?;
        Ok(MapBuild {
            entries,
            target,
            span,
            depth,
            index: 0,
            valid,
            values,
            spans,
        })
    }

    pub(super) fn prepare_record<'slot>(
        &mut self,
        input: PrepareRecordInput<'a>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, RecordBuild<'a>> {
        let PrepareRecordInput {
            entries,
            owner,
            span,
            depth,
        } = input;
        let field_ids = match owner {
            RecordOwner::Struct(id) => self
                .compiler
                .symbols
                .type_symbol(id)
                .map(|symbol| symbol.fields()),
            RecordOwner::Variant(id) => self
                .compiler
                .symbols
                .variant_symbol(id)
                .map(|variant| variant.fields()),
        };
        let Some(field_ids) = field_ids else {
            self.compiler.diagnostics.push(schema_internal!(span));
            return Err(out.fail(BuildFailure::empty()));
        };
        let mut supplied = self.compiler.new_vec(out.reborrow())?;
        supplied.try_reserve_exact(field_ids.len(), out.reborrow())?;
        for _ in field_ids {
            supplied.try_push(None, out.reborrow())?;
        }
        let mut valid = true;
        for entry_index in 0..entries.len() {
            let entry =
                self.ast_access(out.reborrow(), |ast_out| entries.get(entry_index, ast_out))?;
            let Some(entry) = entry else {
                self.compiler.diagnostics.abort(schema_internal!(span));
                return Err(out.fail(BuildFailure::empty()));
            };
            self.compiler
                .context
                .consume(1, span, self.compiler.diagnostics, out.reborrow())?;
            if self.compiler.diagnostics.is_stopped() {
                return Err(out.fail(BuildFailure::empty()));
            }
            let parsed_key = self.ast_access(out.reborrow(), |mut ast_out| {
                let key = entry.key(ast_out.reborrow())?;
                let kind = match key {
                    BraceKey::String(_) => ParsedBraceKeyKind::String,
                    BraceKey::Identifier(_) => ParsedBraceKeyKind::Identifier,
                };
                let (name, span) = brace_key(key, ast_out.reborrow())?;
                Ok(ParsedBraceKey {
                    value: name,
                    span,
                    kind,
                })
            })?;
            let name = parsed_key.value;
            let key_span = parsed_key.span;
            let is_identifier = parsed_key.kind == ParsedBraceKeyKind::Identifier;
            if !is_identifier {
                self.compiler.report_invalid(
                    diagnostic_message!(
                        "diagnostic.schema.default_struct_field_name_invalid",
                        "struct default field names must be identifiers"
                    ),
                    key_span,
                    out.reborrow(),
                )?;
                valid = false;
            }
            let lookup = match owner {
                RecordOwner::Struct(id) => {
                    self.compiler
                        .symbols
                        .lookup_field(id, name, out.reborrow())?
                }
                RecordOwner::Variant(id) => {
                    self.compiler
                        .symbols
                        .lookup_variant_field(id, name, out.reborrow())?
                }
            };
            let field_id = match lookup {
                LookupResult::Found(field_id) => field_id,
                LookupResult::Missing => {
                    self.compiler.report_invalid(
                        diagnostic_message!(
                            "diagnostic.schema.default_struct_field_unknown",
                            "struct default contains an unknown field"
                        ),
                        key_span,
                        out.reborrow(),
                    )?;
                    valid = false;
                    continue;
                }
                LookupResult::Ambiguous => {
                    self.compiler.report_invalid(
                        diagnostic_message!(
                            "diagnostic.schema.default_struct_field_ambiguous",
                            "struct default field name is ambiguous"
                        ),
                        key_span,
                        out.reborrow(),
                    )?;
                    valid = false;
                    continue;
                }
            };
            let Some(index) = self.field_index(field_ids, field_id, key_span, out.reborrow())?
            else {
                self.compiler.diagnostics.push(schema_internal!(key_span));
                return Err(out.fail(BuildFailure::empty()));
            };
            let slot = self.compiler.diagnostics.require(
                supplied.get_mut(index),
                key_span,
                out.reborrow(),
            )?;
            if slot.is_some() {
                self.compiler.report_invalid(
                    diagnostic_message!(
                        "diagnostic.schema.default_struct_duplicate_field",
                        "struct default contains a duplicate field"
                    ),
                    key_span,
                    out.reborrow(),
                )?;
                valid = false;
            } else {
                *slot = Some(entry);
            }
        }

        let values = self.value_buffer(field_ids.len(), span, out.reborrow())?;
        Ok(RecordBuild {
            fields: field_ids,
            supplied,
            span,
            depth,
            index: 0,
            valid,
            values,
            waiting: None,
        })
    }

    fn field_index<'slot>(
        &mut self,
        fields: &[FieldSymbolId],
        target: FieldSymbolId,
        span: SourceSpan,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, Option<usize>> {
        let mut low = 0;
        let mut high = fields.len();
        while low < high {
            self.compiler
                .context
                .consume(1, span, self.compiler.diagnostics, out.reborrow())?;
            let middle = low + (high - low) / 2;
            let field =
                self.compiler
                    .diagnostics
                    .require(fields.get(middle), span, out.reborrow())?;
            match field.index().cmp(&target.index()) {
                Ordering::Less => low = middle + 1,
                Ordering::Greater => high = middle,
                Ordering::Equal => return Ok(Some(middle)),
            }
        }
        Ok(None)
    }

    pub(super) fn spans<'slot>(
        &mut self,
        len: usize,
        span: SourceSpan,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ChargedVec<SourceSpan>> {
        self.buffer(len, span, out.reborrow())
    }

    pub(super) fn values<'slot>(
        &mut self,
        len: usize,
        span: SourceSpan,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ChargedVec<ConfigValue>> {
        self.value_buffer(len, span, out.reborrow())
    }

    fn buffer<'slot, T>(
        &mut self,
        len: usize,
        _span: SourceSpan,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ChargedVec<T>> {
        let mut buffer = self.compiler.new_vec(out.reborrow())?;
        buffer.try_reserve_exact(len, out.reborrow())?;
        Ok(buffer)
    }

    fn value_buffer<'slot, T>(
        &mut self,
        len: usize,
        span: SourceSpan,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ChargedVec<T>> {
        let mut values =
            self.compiler
                .budget_access(span, out.reborrow(), |execution, budget_out| {
                    ChargedVec::new(Some(execution), budget_out)
                })?;
        self.compiler
            .budget_access(span, out.reborrow(), |_, budget_out| {
                values.try_reserve_exact(len, budget_out)
            })?;
        Ok(values)
    }
}

fn brace_key<'slot, 'a>(
    entry: BraceKey<'a>,
    mut out: Out<'slot, '_, AstAccessError>,
) -> OResult<'slot, (&'a str, SourceSpan)> {
    match entry {
        BraceKey::Identifier(name) => Ok((name.text(out.reborrow())?, name.span())),
        BraceKey::String(value) => Ok((value.value(out.reborrow())?, value.span())),
    }
}
