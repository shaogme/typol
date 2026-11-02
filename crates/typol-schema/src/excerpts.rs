use crate::{SchemaCompileErrors, loader::LoadedFile};
use core::mem::size_of;
use opeo::{ErrSlot, OResult, Out};
use typol_core::{
    BudgetError, ChargedVec, Diagnostic, DiagnosticCode, ExcerptLimits, ExecutionBudget,
    LocatedSpan, SourceExcerptSet, SourceSpan, Span, SpanLocation,
};
use typol_syntax::{
    AstAccessError, Declaration, FieldDeclList, SchemaAst, TopLevelItem, VariantPayloadKind,
};

pub(crate) struct SchemaExcerpts;

/// Redacting every default keeps static failures independent of secret alias resolution.
struct DefaultRanges<'a, 'ast> {
    ranges: ChargedVec<Span>,
    pending: ChargedVec<Declaration<'ast>>,
    limits: ExcerptLimits,
    budget: &'a ExecutionBudget,
    errors: ErrSlot<BudgetError>,
}

impl SchemaExcerpts {
    pub(crate) fn errors(
        errors: SchemaCompileErrors,
        files: &[LoadedFile],
        limits: ExcerptLimits,
        budget: &ExecutionBudget,
    ) -> SchemaCompileErrors {
        let mut excerpts = SourceExcerptSet::new(limits, Some(budget));
        if !limits.enabled() {
            return errors.with_excerpts(excerpts);
        }
        for file in files {
            let Some(ranges) = Self::default_ranges(file, limits, budget) else {
                excerpts.mark_missing();
                continue;
            };
            let source_id = file.source_id();
            let mut capture = file.capture_cell().borrow_mut();
            for range in &ranges {
                let Some(secret) = capture.begin_secret(range.start()) else {
                    capture.invalidate();
                    break;
                };
                capture.end_secret(secret, range.end());
            }
            let mut allocation_errors = ErrSlot::<BudgetError>::new();
            let Some(mut diagnostic_spans) = allocation_errors
                .try_call(|out| ChargedVec::new(Some(budget), out))
                .ok()
            else {
                excerpts.mark_missing();
                continue;
            };
            let mut diagnostic_storage_failed = false;
            let max_records = Self::max_diagnostic_records(limits);
            for diagnostic in errors.diagnostics().iter() {
                for span in diagnostic
                    .primary()
                    .into_iter()
                    .chain(diagnostic.labels().iter().map(|label| label.span()))
                {
                    if span.source_id() != source_id {
                        continue;
                    }
                    let Some(location) = errors.span_location(span) else {
                        excerpts.mark_missing();
                        continue;
                    };
                    capture.pin(span, Some(location));
                    if diagnostic_spans.len() >= max_records {
                        excerpts.mark_missing();
                        continue;
                    }
                    if diagnostic_storage_failed
                        || !Self::push_diagnostic_span(
                            &mut diagnostic_spans,
                            Self::located_diagnostic(span, location),
                            &mut allocation_errors,
                        )
                    {
                        diagnostic_storage_failed = true;
                        excerpts.mark_missing();
                    }
                }
            }
            if !diagnostic_spans.is_empty() {
                capture.finish(&mut excerpts, diagnostic_spans.as_slice());
            }
        }
        errors.with_excerpts(excerpts)
    }

    pub(crate) fn records(
        files: &[LoadedFile],
        locations: &[(SourceSpan, SpanLocation)],
        limits: ExcerptLimits,
        budget: &ExecutionBudget,
    ) -> SourceExcerptSet {
        let mut excerpts = SourceExcerptSet::new(limits, Some(budget));
        if !limits.enabled() {
            return excerpts;
        }
        let max_records = Self::max_diagnostic_records(limits);
        for file in files {
            let Some(ranges) = Self::default_ranges(file, limits, budget) else {
                excerpts.mark_missing();
                continue;
            };
            let source_id = file.source_id();
            let mut capture = file.capture_cell().borrow_mut();
            for range in &ranges {
                let Some(secret) = capture.begin_secret(range.start()) else {
                    capture.invalidate();
                    break;
                };
                capture.end_secret(secret, range.end());
            }
            let mut allocation_errors = ErrSlot::<BudgetError>::new();
            let Some(mut diagnostic_spans) = allocation_errors
                .try_call(|out| ChargedVec::new(Some(budget), out))
                .ok()
            else {
                excerpts.mark_missing();
                continue;
            };
            let mut diagnostic_storage_failed = false;
            for (span, location) in locations
                .iter()
                .filter(|(span, _)| span.source_id() == source_id)
            {
                capture.pin(*span, Some(*location));
                if diagnostic_spans.len() >= max_records {
                    excerpts.mark_missing();
                    continue;
                }
                if diagnostic_storage_failed
                    || !Self::push_diagnostic_span(
                        &mut diagnostic_spans,
                        Self::located_diagnostic(*span, *location),
                        &mut allocation_errors,
                    )
                {
                    diagnostic_storage_failed = true;
                    excerpts.mark_missing();
                }
            }
            if !diagnostic_spans.is_empty() {
                capture.finish(&mut excerpts, diagnostic_spans.as_slice());
            }
        }
        excerpts
    }

    fn located_diagnostic(span: SourceSpan, location: SpanLocation) -> Diagnostic {
        let mut diagnostic = Diagnostic::emergency(DiagnosticCode::SCHEMA_SEMANTIC_ERROR);
        diagnostic.set_located_primary(Some(LocatedSpan::new(span, Some(location))));
        diagnostic
    }

    fn push_diagnostic_span(
        diagnostic_spans: &mut ChargedVec<Diagnostic>,
        diagnostic: Diagnostic,
        errors: &mut ErrSlot<BudgetError>,
    ) -> bool {
        errors
            .try_call(|out| diagnostic_spans.try_push(diagnostic, out))
            .is_ok()
    }

    fn max_diagnostic_records(limits: ExcerptLimits) -> usize {
        limits
            .max_records()
            .min(limits.max_bytes() / size_of::<Diagnostic>().max(1))
    }

    fn default_ranges(
        file: &LoadedFile,
        limits: ExcerptLimits,
        budget: &ExecutionBudget,
    ) -> Option<ChargedVec<Span>> {
        if !limits.enabled() {
            return None;
        }
        let ast = file.parsed_ast()?;
        let mut errors = ErrSlot::<BudgetError>::new();
        let ranges = errors
            .try_call(|out| ChargedVec::new(Some(budget), out))
            .ok()?;
        let pending = errors
            .try_call(|out| ChargedVec::new(Some(budget), out))
            .ok()?;
        let mut collector = DefaultRanges {
            ranges,
            pending,
            limits,
            budget,
            errors,
        };
        let mut ast_errors = ErrSlot::<AstAccessError>::new();
        match ast_errors.try_call(|out| collector.collect(ast, out)) {
            Ok(true) => Some(collector.ranges),
            Ok(false) | Err(_) => None,
        }
    }
}

impl<'ast> DefaultRanges<'_, 'ast> {
    fn collect<'slot>(
        &mut self,
        ast: &'ast SchemaAst,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, bool> {
        let items = ast.items();
        for index in 0..items.len() {
            if !self.budget.consume_optional(1) {
                return Ok(false);
            }
            let Some(item) = items.get(index, out.reborrow())? else {
                return Ok(false);
            };
            match item {
                TopLevelItem::EntryStruct(item) => {
                    let fields = item.fields(out.reborrow())?;
                    if !self.fields(fields, out.reborrow())? {
                        return Ok(false);
                    }
                }
                TopLevelItem::Declaration(item) => {
                    if !self.declaration(item, out.reborrow())? {
                        return Ok(false);
                    }
                }
                TopLevelItem::ExternMod(_) => {}
            }
        }
        while let Some(item) = self.pending.pop() {
            if !self.declaration(item, out.reborrow())? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn declaration<'slot>(
        &mut self,
        declaration: Declaration<'ast>,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, bool> {
        if !self.budget.consume_optional(1) {
            return Ok(false);
        }
        match declaration {
            Declaration::Module(module) => {
                let declarations = module.declarations(out.reborrow())?;
                for index in 0..declarations.len() {
                    if self.pending.len() >= self.limits.max_records() {
                        return Ok(false);
                    }
                    let Some(item) = declarations.get(index, out.reborrow())? else {
                        return Ok(false);
                    };
                    if !self.push_pending(item) {
                        return Ok(false);
                    }
                }
            }
            Declaration::Struct(item) => {
                let fields = item.fields(out.reborrow())?;
                if !self.fields(fields, out.reborrow())? {
                    return Ok(false);
                }
            }
            Declaration::Enum(item) => {
                let variants = item.variants(out.reborrow())?;
                for index in 0..variants.len() {
                    if !self.budget.consume_optional(1) {
                        return Ok(false);
                    }
                    let Some(variant) = variants.get(index, out.reborrow())? else {
                        return Ok(false);
                    };
                    let payload = variant.payload(out.reborrow())?;
                    if let VariantPayloadKind::Structure(payload) = payload.kind(out.reborrow())? {
                        let fields = payload.fields(out.reborrow())?;
                        if !self.fields(fields, out.reborrow())? {
                            return Ok(false);
                        }
                    }
                }
            }
            Declaration::TypeAlias(_) => {}
        }
        Ok(true)
    }

    fn fields<'slot>(
        &mut self,
        fields: FieldDeclList<'ast>,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, bool> {
        for index in 0..fields.len() {
            if !self.budget.consume_optional(1) {
                return Ok(false);
            }
            let Some(field) = fields.get(index, out.reborrow())? else {
                return Ok(false);
            };
            if let Some(default) = field.default(out.reborrow())? {
                if self.ranges.len() >= self.limits.max_records()
                    || !self.push_range(default.span().span())
                {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    fn push_range(&mut self, range: Span) -> bool {
        let Some(required_bytes) = self
            .ranges
            .len()
            .checked_add(1)
            .and_then(|count| count.checked_mul(size_of::<Span>()))
        else {
            return false;
        };
        if required_bytes > self.limits.max_bytes() {
            return false;
        }
        self.errors
            .try_call(|mut out| {
                self.ranges.try_reserve_exact(1, out.reborrow())?;
                self.ranges.try_push(range, out)
            })
            .is_ok()
    }

    fn push_pending(&mut self, declaration: Declaration<'ast>) -> bool {
        let Some(required_bytes) = self
            .pending
            .len()
            .checked_add(1)
            .and_then(|count| count.checked_mul(size_of::<Declaration<'ast>>()))
        else {
            return false;
        };
        if required_bytes > self.limits.max_bytes() {
            return false;
        }
        self.errors
            .try_call(|mut out| {
                self.pending.try_reserve_exact(1, out.reborrow())?;
                self.pending.try_push(declaration, out)
            })
            .is_ok()
    }
}
