#[cfg(test)]
mod tests;

use crate::{
    AssertionInfo, CompiledRegexConstraints, CompiledSchema, ConstraintId, ConstraintRecord,
    EvalFailure, FieldId, FieldRecord, ProgramId, RefinementId, SchemaAccessError, ShapeId,
    ShapeKind, SpanLocation, TypeId, TypeRecord, TypeRecordOptions, VariantId, VariantRecord,
    constraints::{Refinement, RefinementSummary},
    indexes::SchemaIndexes,
    validation::{SchemaValidationRequest, Validation},
};
use core::cmp::Ordering;
use opeo::{OResult, Out, ResultOutExt};
use typol_core::{
    BudgetError, ChargedVec, DiagnosticContext, ExecutionBudget, RegexExecutionBudget, SourceId,
    SourceSpan, diagnostic_context,
};
use typol_policy::{RegexLimits, ValueLimits};

static BUILDER_INTERNAL: DiagnosticContext =
    diagnostic_context!("typol-semantic", "schema-build", "builder-invariant-failed",);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BuildError {
    Allocation,
    Resource,
    Capacity,
    Internal,
    InternalAt {
        context: DiagnosticContext,
        source_span: Option<SourceSpan>,
    },
    DefaultConstraintAt {
        source_span: SourceSpan,
    },
    /// The default-value expression failed during evaluation.
    DefaultEvaluationAt {
        failure: EvalFailure,
        source_span: SourceSpan,
    },
    SchemaAccess(SchemaAccessError),
    SchemaAccessAt {
        error: SchemaAccessError,
        source_span: SourceSpan,
    },
}

/// The prefix and inherited chain hold alias rules; the tail holds local field or payload rules.
pub struct RefinementInput {
    pub target: TypeId,
    pub rules: ChargedVec<ConstraintId>,
    pub next: Option<RefinementId>,
    pub tail: ChargedVec<ConstraintId>,
}

impl BuildError {
    pub const fn internal(context: DiagnosticContext) -> Self {
        Self::InternalAt {
            context,
            source_span: None,
        }
    }

    pub const fn context(self) -> Option<DiagnosticContext> {
        match self {
            Self::InternalAt { context, .. } => Some(context),
            Self::SchemaAccess(error) | Self::SchemaAccessAt { error, .. } => Some(error.context()),
            Self::DefaultEvaluationAt { failure, .. } => failure.context(),
            Self::Allocation
            | Self::Resource
            | Self::Capacity
            | Self::Internal
            | Self::DefaultConstraintAt { .. } => None,
        }
    }

    pub const fn is_internal(self) -> bool {
        matches!(
            self,
            Self::Internal
                | Self::InternalAt { .. }
                | Self::SchemaAccess(_)
                | Self::SchemaAccessAt { .. }
        ) || matches!(self, Self::DefaultEvaluationAt { failure, .. } if failure.is_internal())
    }

    pub const fn source_span(self) -> Option<SourceSpan> {
        match self {
            Self::InternalAt { source_span, .. } => source_span,
            Self::SchemaAccessAt { source_span, .. } => Some(source_span),
            Self::DefaultConstraintAt { source_span } => Some(source_span),
            Self::DefaultEvaluationAt { source_span, .. } => Some(source_span),
            _ => None,
        }
    }

    pub const fn schema_access(self) -> Option<SchemaAccessError> {
        match self {
            Self::SchemaAccess(error) | Self::SchemaAccessAt { error, .. } => Some(error),
            Self::DefaultEvaluationAt { failure, .. } => failure.schema_access(),
            _ => None,
        }
    }

    pub const fn with_source_span(self, source_span: SourceSpan) -> Self {
        match self {
            Self::InternalAt { context, .. } => Self::InternalAt {
                context,
                source_span: Some(source_span),
            },
            Self::SchemaAccess(error) | Self::SchemaAccessAt { error, .. } => {
                Self::SchemaAccessAt { error, source_span }
            }
            Self::DefaultConstraintAt { .. } | Self::DefaultEvaluationAt { .. } => self,
            Self::Allocation | Self::Resource | Self::Capacity | Self::Internal => self,
        }
    }
}

impl From<BudgetError> for BuildError {
    fn from(error: BudgetError) -> Self {
        match error {
            BudgetError::Resource
            | BudgetError::RegexSteps
            | BudgetError::RegexSearchSteps
            | BudgetError::RegexWorkspace => Self::Resource,
            BudgetError::Allocation => Self::Allocation,
            BudgetError::CapacityExceeded => Self::Capacity,
            BudgetError::InvariantViolation(context) => Self::internal(*context),
        }
    }
}

impl From<SchemaAccessError> for BuildError {
    fn from(error: SchemaAccessError) -> Self {
        Self::SchemaAccess(error)
    }
}

/// Borrows the source directory during freezing to validate UTF-8 spans without retaining source text or identity.
pub trait SourceLookup {
    fn source_count(&self) -> usize;

    fn is_valid_span(&self, span: SourceSpan) -> bool;
}

/// Evidence that the Schema compiler has validated default shapes and constraints.
pub struct SchemaDefaultValidation(());

impl SchemaDefaultValidation {
    /// Records full validation completed by the Schema default construction pass.
    ///
    /// # Safety
    /// The caller must ensure the Builder defaults exactly match values that passed full shape and constraint validation.
    pub unsafe fn from_completed_schema_validation() -> Self {
        Self(())
    }
}

/// Carries Schema compiler validated defaults into the freeze phase.
pub struct SchemaFreezeInput<'a, S: SourceLookup + ?Sized> {
    entry: TypeId,
    sources: &'a S,
    locations: ChargedVec<(SourceSpan, SpanLocation)>,
    default_validation: SchemaDefaultValidation,
}

impl<'a, S: SourceLookup + ?Sized> SchemaFreezeInput<'a, S> {
    pub fn new(
        entry: TypeId,
        sources: &'a S,
        locations: ChargedVec<(SourceSpan, SpanLocation)>,
        default_validation: SchemaDefaultValidation,
    ) -> Self {
        Self {
            entry,
            sources,
            locations,
            default_validation,
        }
    }
}

struct FreezeRequest<'a, S: SourceLookup + ?Sized> {
    entry: TypeId,
    sources: &'a S,
    locations: ChargedVec<(SourceSpan, SpanLocation)>,
    validate_defaults: bool,
}

impl SourceLookup for [&str] {
    fn source_count(&self) -> usize {
        self.len()
    }

    fn is_valid_span(&self, span: SourceSpan) -> bool {
        self.get(span.source_id().get() as usize)
            .is_some_and(|text| {
                span.span().end() <= text.len()
                    && text.is_char_boundary(span.span().start())
                    && text.is_char_boundary(span.span().end())
            })
    }
}

impl<const N: usize> SourceLookup for [&str; N] {
    fn source_count(&self) -> usize {
        N
    }

    fn is_valid_span(&self, span: SourceSpan) -> bool {
        self.as_slice().is_valid_span(span)
    }
}

/// Record insertion uses fallible allocation; freezing is allowed only after cross-table checks are complete.
pub struct Builder {
    schema: CompiledSchema,
    shape_index: ChargedVec<(ShapeKind, ShapeId)>,
    execution: ExecutionBudget,
    regex_execution: RegexExecutionBudget,
    value_limits: ValueLimits,
    regex_limits: RegexLimits,
}

impl Builder {
    pub fn execution(&self) -> &ExecutionBudget {
        &self.execution
    }

    pub fn new<'slot>(
        execution: &ExecutionBudget,
        value_limits: ValueLimits,
        regex_limits: RegexLimits,
        regex_execution: &RegexExecutionBudget,
        mut out: Out<'slot, '_, BuildError>,
    ) -> OResult<'slot, Self> {
        let execution = execution.try_clone(out.reborrow())?;
        let regex_execution = regex_execution.try_clone(out.reborrow())?;
        Ok(Self {
            schema: CompiledSchema::empty(&execution, out.reborrow())?,
            shape_index: ChargedVec::new(Some(&execution), out.reborrow())?,
            execution,
            regex_execution,
            value_limits,
            regex_limits,
        })
    }

    pub fn reserve_type<'slot>(
        &mut self,
        span: SourceSpan,
        mut out: Out<'slot, '_, BuildError>,
    ) -> OResult<'slot, TypeId> {
        let execution = self.execution().try_clone(out.reborrow())?;
        self.schema
            .records_mut()
            .types
            .try_reserve(1, out.reborrow())?;
        let id = TypeId::new(self.schema.records_mut().types.len());
        let record = TypeRecord::new(
            TypeRecordOptions {
                name: None,
                qualified_name: None,
                shape: ShapeId(usize::MAX),
                child: None,
                span,
            },
            &execution,
            out.reborrow(),
        )?;
        self.schema
            .records_mut()
            .types
            .try_push(record, out.reborrow())?;
        Ok(id)
    }

    pub fn set_type<'slot>(
        &mut self,
        id: TypeId,
        record: TypeRecord,
        mut out: Out<'slot, '_, BuildError>,
    ) -> OResult<'slot, ()> {
        self.execution().consume(1, out.reborrow())?;
        *self
            .schema
            .records_mut()
            .types
            .get_mut(id.index())
            .ok_or(BuildError::internal(BUILDER_INTERNAL))
            .store_err_into(out.reborrow())? = record;
        Ok(())
    }

    pub fn type_mut<'slot>(
        &mut self,
        id: TypeId,
        mut out: Out<'slot, '_, BuildError>,
    ) -> OResult<'slot, &mut TypeRecord> {
        self.execution().consume(1, out.reborrow())?;
        match self.schema.records_mut().types.get_mut(id.index()) {
            Some(record) => Ok(record),
            None => Err(out.fail(BuildError::internal(BUILDER_INTERNAL))),
        }
    }

    pub fn type_shape<'slot>(
        &self,
        id: TypeId,
        mut out: Out<'slot, '_, BuildError>,
    ) -> OResult<'slot, ShapeId> {
        self.execution().consume(1, out.reborrow())?;
        let shape = self
            .schema
            .records()
            .types
            .get(id.index())
            .ok_or(BuildError::internal(BUILDER_INTERNAL));
        let shape = shape.store_err_into(out.reborrow())?.record().shape;
        self.schema
            .records()
            .shapes
            .get(shape.0)
            .ok_or(BuildError::internal(BUILDER_INTERNAL))
            .store_err_into(out.reborrow())?;
        Ok(shape)
    }

    pub fn intern_shape<'slot>(
        &mut self,
        kind: ShapeKind,
        mut out: Out<'slot, '_, BuildError>,
    ) -> OResult<'slot, ShapeId> {
        self.execution().consume(1, out.reborrow())?;
        let child = match kind {
            ShapeKind::Option(id) | ShapeKind::Map(id) => Some(id),
            ShapeKind::Array { element, .. } => Some(element),
            _ => None,
        };
        if let Some(child) = child {
            let child_kind = self
                .schema
                .records()
                .shapes
                .get(child.0)
                .ok_or(BuildError::internal(BUILDER_INTERNAL))
                .store_err_into(out.reborrow())?;
            if matches!(kind, ShapeKind::Option(_)) && matches!(child_kind, ShapeKind::Option(_)) {
                return Err(out.fail(BuildError::internal(BUILDER_INTERNAL)));
            }
        }
        let mut low = 0;
        let mut high = self.shape_index.len();
        while low < high {
            self.execution().consume(1, out.reborrow())?;
            let middle = low + (high - low) / 2;
            let entry = self
                .shape_index
                .get(middle)
                .ok_or(BuildError::internal(BUILDER_INTERNAL))
                .store_err_into(out.reborrow())?;
            match entry.0.cmp(&kind) {
                Ordering::Less => low = middle + 1,
                Ordering::Greater => high = middle,
                Ordering::Equal => return Ok(entry.1),
            }
        }
        let position = low;
        self.execution()
            .consume(self.shape_index.len() - position, out.reborrow())?;
        self.schema
            .records_mut()
            .shapes
            .try_reserve(1, out.reborrow())?;
        self.shape_index.try_reserve(1, out.reborrow())?;
        let id = ShapeId(self.schema.records_mut().shapes.len());
        self.schema
            .records_mut()
            .shapes
            .try_push(kind, out.reborrow())?;
        self.shape_index.try_push((kind, id), out.reborrow())?;
        self.shape_index
            .get_mut(position..)
            .ok_or(BuildError::internal(BUILDER_INTERNAL))
            .store_err_into(out.reborrow())?
            .rotate_right(1);
        Ok(id)
    }

    pub fn add_field<'slot>(
        &mut self,
        record: FieldRecord,
        mut out: Out<'slot, '_, BuildError>,
    ) -> OResult<'slot, FieldId> {
        self.execution().consume(1, out.reborrow())?;
        self.schema
            .records_mut()
            .fields
            .try_reserve(1, out.reborrow())?;
        let id = FieldId::new(self.schema.records_mut().fields.len());
        self.schema
            .records_mut()
            .fields
            .try_push(record, out.reborrow())?;
        Ok(id)
    }

    pub fn add_variant<'slot>(
        &mut self,
        record: VariantRecord,
        mut out: Out<'slot, '_, BuildError>,
    ) -> OResult<'slot, VariantId> {
        self.execution().consume(1, out.reborrow())?;
        self.schema
            .records_mut()
            .variants
            .try_reserve(1, out.reborrow())?;
        let id = VariantId::new(self.schema.records_mut().variants.len());
        self.schema
            .records_mut()
            .variants
            .try_push(record, out.reborrow())?;
        Ok(id)
    }

    pub fn add_program<'slot>(
        &mut self,
        program: AssertionInfo,
        mut out: Out<'slot, '_, BuildError>,
    ) -> OResult<'slot, ProgramId> {
        self.execution().consume(1, out.reborrow())?;
        Validation::program_topology(program.expression(), self.execution(), out.reborrow())?;
        self.schema
            .records_mut()
            .programs
            .try_reserve(1, out.reborrow())?;
        let id = ProgramId::new(self.schema.records_mut().programs.len());
        self.schema
            .records_mut()
            .programs
            .try_push(program, out.reborrow())?;
        Ok(id)
    }

    pub fn add_constraint<'slot>(
        &mut self,
        record: ConstraintRecord,
        mut out: Out<'slot, '_, BuildError>,
    ) -> OResult<'slot, ConstraintId> {
        self.execution().consume(1, out.reborrow())?;
        self.schema
            .records_mut()
            .constraints
            .try_reserve(1, out.reborrow())?;
        let id = ConstraintId(self.schema.records_mut().constraints.len());
        self.schema
            .records_mut()
            .constraints
            .try_push(record, out.reborrow())?;
        Ok(id)
    }

    pub fn add_refinement<'slot>(
        &mut self,
        input: RefinementInput,
        mut out: Out<'slot, '_, BuildError>,
    ) -> OResult<'slot, Option<RefinementId>> {
        let RefinementInput {
            target,
            rules,
            next,
            tail,
        } = input;
        self.execution().consume(1, out.reborrow())?;
        let rule_count = rules
            .len()
            .checked_add(tail.len())
            .ok_or(BuildError::Resource)
            .store_err_into(out.reborrow())?;
        self.execution().consume(rule_count, out.reborrow())?;
        if rules
            .iter()
            .chain(&tail)
            .any(|id| id.0 >= self.schema.records_mut().constraints.len())
        {
            return Err(out.fail(BuildError::internal(BUILDER_INTERNAL)));
        }
        let shape = self.type_shape(target, out.reborrow())?;
        let inherited = match next {
            Some(id) => {
                let segment = self
                    .schema
                    .records()
                    .refinements
                    .get(id.0)
                    .ok_or(BuildError::internal(BUILDER_INTERNAL))
                    .store_err_into(out.reborrow())?;
                if self.type_shape(segment.target, out.reborrow())? != shape
                    || !segment.tail.is_empty()
                {
                    return Err(out.fail(BuildError::internal(BUILDER_INTERNAL)));
                }
                segment.len
            }
            None => 0,
        };
        if rules.is_empty() && tail.is_empty() {
            return Ok(next);
        }
        let mut summary = match next {
            Some(id) => {
                self.schema
                    .records()
                    .refinements
                    .get(id.0)
                    .ok_or(BuildError::internal(BUILDER_INTERNAL))
                    .store_err_into(out.reborrow())?
                    .summary
            }
            None => RefinementSummary::default(),
        };
        for id in rules.iter().chain(&tail) {
            self.execution().consume(1, out.reborrow())?;
            let constraint = self
                .schema
                .records()
                .constraints
                .get(id.0)
                .ok_or(BuildError::internal(BUILDER_INTERNAL))
                .store_err_into(out.reborrow())?;
            summary.include(constraint.data(), out.reborrow())?;
        }
        let len = rules
            .len()
            .checked_add(inherited)
            .and_then(|n| n.checked_add(tail.len()))
            .ok_or(BuildError::Resource)
            .store_err_into(out.reborrow())?;
        self.schema
            .records_mut()
            .refinements
            .try_reserve(1, out.reborrow())?;
        let id = RefinementId(self.schema.records_mut().refinements.len());
        self.schema.records_mut().refinements.try_push(
            Refinement {
                summary,
                target,
                rules,
                next,
                tail,
                len,
            },
            out.reborrow(),
        )?;
        Ok(Some(id))
    }

    pub fn set_regexes(&mut self, regexes: CompiledRegexConstraints) {
        self.schema.records_mut().regexes = regexes;
    }

    pub fn spans<'slot, 'borrow>(
        &self,
        out: Out<'slot, 'borrow, BuildError>,
    ) -> OResult<'slot, ChargedVec<SourceSpan>> {
        Validation::collect_spans(&self.schema, self.execution(), out)
    }

    pub fn freeze<'slot>(
        self,
        entry: TypeId,
        sources: &(impl SourceLookup + ?Sized),
        locations: ChargedVec<(SourceSpan, SpanLocation)>,
        out: Out<'slot, '_, BuildError>,
    ) -> OResult<'slot, CompiledSchema> {
        self.freeze_inner(
            FreezeRequest {
                entry,
                sources,
                locations,
                validate_defaults: true,
            },
            out,
        )
    }

    pub fn freeze_with_validated_defaults<'slot, S: SourceLookup + ?Sized>(
        self,
        input: SchemaFreezeInput<'_, S>,
        out: Out<'slot, '_, BuildError>,
    ) -> OResult<'slot, CompiledSchema> {
        let SchemaFreezeInput {
            entry,
            sources,
            locations,
            default_validation,
        } = input;
        let _validation_evidence = default_validation;
        self.freeze_inner(
            FreezeRequest {
                entry,
                sources,
                locations,
                validate_defaults: false,
            },
            out,
        )
    }

    fn freeze_inner<'slot, S: SourceLookup + ?Sized>(
        mut self,
        request: FreezeRequest<'_, S>,
        mut out: Out<'slot, '_, BuildError>,
    ) -> OResult<'slot, CompiledSchema> {
        let FreezeRequest {
            entry,
            sources,
            locations,
            validate_defaults,
        } = request;
        let execution = &self.execution;
        execution.consume(1, out.reborrow())?;
        self.schema.records_mut().entry_type = entry;
        self.schema.records_mut().next_source_id = u32::try_from(sources.source_count())
            .ok()
            .map(SourceId::new);
        self.schema.records_mut().locations = locations;
        // Defaults query fields and variants during freezing, so build indexes before validation.
        let records = self.schema.records_mut();
        let indexes = SchemaIndexes::build(
            &mut records.fields,
            &records.variants,
            &records.types,
            execution,
            out.reborrow(),
        )?;
        self.schema.set_indexes(indexes);
        Validation::validate(
            SchemaValidationRequest {
                schema: &self.schema,
                sources,
                execution,
                regex_execution: &self.regex_execution,
                value_limits: &self.value_limits,
                regex_limits: &self.regex_limits,
                validate_defaults,
            },
            out.reborrow(),
        )?;
        Ok(self.schema)
    }
}
