use crate::{
    expressions::TypedExpression,
    model::{CompiledSchema, FieldId, SchemaAccessError, StoredConstraintData, TypeId},
    views::{ConstraintData, ExpressionView, FieldList, RegexView, TypeView},
};
use core::{fmt, iter::once, ptr};
use opeo::{OResult, Out, ResultOutExt};
use typol_core::{
    BudgetError, ChargedVec, DiagnosticContext, ExecutionBudget, SourceSpan, diagnostic_context,
};

#[cfg(feature = "compiler")]
use crate::builder::BuildError;
#[cfg(feature = "compiler")]
use core::cmp::Ordering;

static RULE_OWNER_SCHEMA_MISMATCH: DiagnosticContext = diagnostic_context!(
    "typol-semantic",
    "rule-ownership",
    "constraint-list-schema-mismatch",
);
static CONSTRAINT_ITERATOR_INVARIANT: DiagnosticContext = diagnostic_context!(
    "typol-semantic",
    "constraint-iteration",
    "constraint-index-missing",
);
#[cfg(feature = "compiler")]
static CONSTRAINT_BUILD_INTERNAL: DiagnosticContext =
    diagnostic_context!("typol-semantic", "constraint-build", "rule-kind-mismatch",);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConstraintKind {
    Min,
    Max,
    Range,
    Length,
    Regex,
    Host,
    Email,
    Cidr,
    Size,
    Unique,
    All,
    Sum,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConstraintLayer {
    Alias,
    Field,
    VariantPayload,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NumberValue {
    Signed(i128),
    Unsigned(u128),
    Float(f64),
}

pub struct ConstraintRecord {
    kind: ConstraintKind,
    layer: ConstraintLayer,
    span: SourceSpan,
    data: StoredConstraintData,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConstraintId(pub(crate) usize);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RefinementId(pub(crate) usize);

pub enum StoredRuleView<'a> {
    NumberBounds {
        min: Option<NumberValue>,
        max: Option<NumberValue>,
    },
    LengthBounds {
        min: u64,
        max: u64,
    },
    Regex {
        index: usize,
    },
    Host,
    Email,
    Cidr {
        v4_only: bool,
    },
    SizeBounds {
        min: Option<u64>,
        max: Option<u64>,
    },
    Unique {
        by: Option<&'a [FieldId]>,
    },
    All {
        expression: &'a TypedExpression,
        message: &'a str,
    },
    SumBounds {
        min: Option<NumberValue>,
        max: Option<NumberValue>,
    },
}

#[derive(Clone, Copy)]
pub struct Constraint<'a> {
    schema: &'a CompiledSchema,
    id: ConstraintId,
    data: &'a ConstraintRecord,
}

/// Identity binds a model to a rule record; distinct rules at the same location still have distinct identities.
#[derive(Clone, Copy)]
pub struct RuleIdentity<'a> {
    schema: &'a CompiledSchema,
    id: ConstraintId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuleOwner {
    Local,
    Inherited,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuleOwnerError {
    owner: RuleOwner,
    index: Option<usize>,
}

#[derive(Clone, Copy)]
pub struct Bounds<T> {
    min: Option<T>,
    max: Option<T>,
}

#[derive(Clone, Copy, Default)]
pub struct RefinementSummary {
    host: bool,
    number: Bounds<NumberValue>,
    length: Bounds<u64>,
    size: Bounds<u64>,
    sum: Bounds<NumberValue>,
}

pub struct Refinement {
    pub(crate) summary: RefinementSummary,
    pub(crate) target: TypeId,
    pub(crate) rules: ChargedVec<ConstraintId>,
    pub(crate) next: Option<RefinementId>,
    pub(crate) tail: ChargedVec<ConstraintId>,
    pub(crate) len: usize,
}

#[derive(Clone, Copy)]
pub struct ConstraintList<'a> {
    schema: &'a CompiledSchema,
    head: Option<RefinementId>,
}

#[derive(Clone)]
pub struct ConstraintIter<'a> {
    schema: &'a CompiledSchema,
    head: Option<RefinementId>,
    index: usize,
    tail: Option<RefinementId>,
    tail_index: usize,
}

/// For one assignment, effective rules put aliases first and use-site rules last, retaining only the first occurrence of each identity.
pub struct EffectiveRules<'schema, 'context> {
    inherited: &'context [ConstraintList<'schema>],
    local: ConstraintList<'schema>,
}

impl<'a> Constraint<'a> {
    pub(crate) fn schema(&self) -> &'a CompiledSchema {
        self.schema
    }

    pub fn identity(self) -> RuleIdentity<'a> {
        RuleIdentity {
            schema: self.schema,
            id: self.id,
        }
    }

    pub fn kind(&self) -> ConstraintKind {
        self.data.kind
    }

    pub fn layer(&self) -> ConstraintLayer {
        self.data.layer
    }

    pub fn span(&self) -> SourceSpan {
        self.data.span
    }

    pub(crate) fn raw_data<'slot, E>(
        &self,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, StoredRuleView<'a>>
    where
        E: From<SchemaAccessError>,
    {
        let data = match &self.data.data {
            StoredConstraintData::NumberBounds { min, max } => StoredRuleView::NumberBounds {
                min: *min,
                max: *max,
            },
            StoredConstraintData::LengthBounds { min, max } => StoredRuleView::LengthBounds {
                min: *min,
                max: *max,
            },
            StoredConstraintData::Regex { regex } => StoredRuleView::Regex {
                index: regex.index(),
            },
            StoredConstraintData::Host => StoredRuleView::Host,
            StoredConstraintData::Email => StoredRuleView::Email,
            StoredConstraintData::Cidr { v4_only } => StoredRuleView::Cidr { v4_only: *v4_only },
            StoredConstraintData::SizeBounds { min, max } => StoredRuleView::SizeBounds {
                min: *min,
                max: *max,
            },
            StoredConstraintData::Unique { by } => StoredRuleView::Unique { by: by.as_deref() },
            StoredConstraintData::All { program } => {
                let program = self
                    .schema
                    .records()
                    .programs
                    .get(program.index())
                    .ok_or_else(|| SchemaAccessError::new("program", program.index()))
                    .store_err_into(out.reborrow())?;
                StoredRuleView::All {
                    expression: program.expression(),
                    message: program.message(),
                }
            }
            StoredConstraintData::SumBounds { min, max } => StoredRuleView::SumBounds {
                min: *min,
                max: *max,
            },
        };
        Ok(data)
    }
}

#[cfg(feature = "compiler")]
impl ConstraintRecord {
    pub fn span(&self) -> SourceSpan {
        self.span
    }

    pub fn new(
        kind: ConstraintKind,
        layer: ConstraintLayer,
        span: SourceSpan,
        data: StoredConstraintData,
    ) -> Self {
        Self {
            kind,
            layer,
            span,
            data,
        }
    }

    pub fn data(&self) -> &StoredConstraintData {
        &self.data
    }

    pub fn data_mut(&mut self) -> &mut StoredConstraintData {
        &mut self.data
    }

    pub fn kind(&self) -> ConstraintKind {
        self.kind
    }

    pub fn layer(&self) -> ConstraintLayer {
        self.layer
    }

    pub fn stored_data(&self) -> &StoredConstraintData {
        &self.data
    }
}

impl RuleIdentity<'_> {
    pub fn belongs_to(self, schema: &CompiledSchema) -> bool {
        ptr::eq(self.schema, schema)
    }
}

impl PartialEq for RuleIdentity<'_> {
    fn eq(&self, other: &Self) -> bool {
        ptr::eq(self.schema, other.schema) && self.id == other.id
    }
}

impl Eq for RuleIdentity<'_> {}

impl fmt::Debug for RuleIdentity<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuleIdentity")
            .finish_non_exhaustive()
    }
}

impl RuleOwnerError {
    const fn local() -> Self {
        Self {
            owner: RuleOwner::Local,
            index: None,
        }
    }

    const fn inherited(index: usize) -> Self {
        Self {
            owner: RuleOwner::Inherited,
            index: Some(index),
        }
    }

    pub const fn owner(self) -> RuleOwner {
        self.owner
    }

    pub const fn index(self) -> Option<usize> {
        self.index
    }

    pub const fn context(self) -> DiagnosticContext {
        match self.index {
            Some(index) => RULE_OWNER_SCHEMA_MISMATCH.with_object_id(index as u64),
            None => RULE_OWNER_SCHEMA_MISMATCH,
        }
    }
}

impl<'schema, 'context> EffectiveRules<'schema, 'context> {
    pub fn new<'slot, 'borrow>(
        schema: &'schema CompiledSchema,
        inherited: &'context [ConstraintList<'schema>],
        local: ConstraintList<'schema>,
        out: Out<'slot, 'borrow, RuleOwnerError>,
    ) -> OResult<'slot, Self> {
        if !local.belongs_to(schema) {
            return Err(out.fail(RuleOwnerError::local()));
        }
        for (index, list) in inherited.iter().enumerate() {
            if !list.belongs_to(schema) {
                return Err(out.fail(RuleOwnerError::inherited(index)));
            }
        }
        Ok(Self { inherited, local })
    }

    pub fn collect_budgeted<'slot, E>(
        &self,
        budget: &ExecutionBudget,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, ChargedVec<Constraint<'schema>>>
    where
        E: From<BudgetError>,
    {
        let mut candidates: ChargedVec<Constraint<'schema>> =
            ChargedVec::new(Some(budget), out.reborrow())?;
        let mut sorted_ids: ChargedVec<ConstraintId> =
            ChargedVec::new(Some(budget), out.reborrow())?;
        for alias in [true, false] {
            for list in self.inherited.iter().copied().chain(once(self.local)) {
                budget.consume(1, out.reborrow())?;
                let mut iterator = list.raw_iter();
                while let Some(rule) = iterator.next_budgeted(budget, out.reborrow())? {
                    budget.consume(1, out.reborrow())?;
                    if (rule.layer() == ConstraintLayer::Alias) != alias {
                        continue;
                    }
                    candidates.try_push(rule, out.reborrow())?;
                    sorted_ids.try_push(rule.id, out.reborrow())?;
                }
            }
        }
        sorted_ids.sort_by(
            |left, right, mut sort_out| {
                budget.consume(1, sort_out.reborrow())?;
                Ok(left.0.cmp(&right.0))
            },
            out.reborrow(),
        )?;
        let mut seen = ChargedVec::new(Some(budget), out.reborrow())?;
        seen.try_reserve_exact(sorted_ids.len(), out.reborrow())?;
        for _ in &sorted_ids {
            budget.consume(1, out.reborrow())?;
            seen.try_push(false, out.reborrow())?;
        }
        let mut rules = ChargedVec::new(Some(budget), out.reborrow())?;
        for rule in candidates {
            let mut low = 0;
            let mut high = sorted_ids.len();
            while low < high {
                budget.consume(1, out.reborrow())?;
                let middle = low + (high - low) / 2;
                let id = sorted_ids
                    .get(middle)
                    .ok_or(BudgetError::invariant(&CONSTRAINT_ITERATOR_INVARIANT))
                    .store_err_into(out.reborrow())?;
                if id.0 < rule.id.0 {
                    low = middle + 1;
                } else {
                    high = middle;
                }
            }
            let candidate = sorted_ids
                .get(low)
                .ok_or(BudgetError::invariant(&CONSTRAINT_ITERATOR_INVARIANT))
                .store_err_into(out.reborrow())?;
            if candidate.0 != rule.id.0 {
                return Err(out.fail(BudgetError::invariant(&CONSTRAINT_ITERATOR_INVARIANT).into()));
            }
            let was_seen = seen
                .get_mut(low)
                .ok_or(BudgetError::invariant(&CONSTRAINT_ITERATOR_INVARIANT))
                .store_err_into(out.reborrow())?;
            if !*was_seen {
                *was_seen = true;
                rules.try_push(rule, out.reborrow())?;
            }
        }
        Ok(rules)
    }
}

impl<'a> ConstraintList<'a> {
    pub fn belongs_to(self, schema: &CompiledSchema) -> bool {
        ptr::eq(self.schema, schema)
    }

    pub(crate) fn new(schema: &'a CompiledSchema, head: Option<RefinementId>) -> Self {
        Self { schema, head }
    }

    pub fn summary<'slot, E>(self, mut out: Out<'slot, '_, E>) -> OResult<'slot, RefinementSummary>
    where
        E: From<SchemaAccessError>,
    {
        let Some(id) = self.head else {
            return Ok(RefinementSummary::default());
        };
        self.schema
            .records()
            .refinements
            .get(id.0)
            .map(|segment| segment.summary)
            .ok_or_else(|| SchemaAccessError::new("refinement", id.0))
            .store_err_into(out.reborrow())
    }

    pub fn target_type<'slot, E>(
        self,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, Option<TypeView<'a>>>
    where
        E: From<SchemaAccessError>,
    {
        let Some(id) = self.head else {
            return Ok(None);
        };
        let target = self
            .schema
            .records()
            .refinements
            .get(id.0)
            .map(|segment| segment.target)
            .ok_or_else(|| SchemaAccessError::new("refinement", id.0))
            .store_err_into(out.reborrow())?;
        self.schema
            .type_info(target)
            .and_then(|info| info.ok_or_else(|| SchemaAccessError::new("type", target.index())))
            .store_err_into(out.reborrow())?;
        Ok(Some(self.schema.view(target)))
    }

    pub fn has_host<'slot, E>(self, mut out: Out<'slot, '_, E>) -> OResult<'slot, bool>
    where
        E: From<SchemaAccessError>,
    {
        let Some(id) = self.head else {
            return Ok(false);
        };
        self.schema
            .records()
            .refinements
            .get(id.0)
            .map(|segment| segment.summary.host)
            .ok_or_else(|| SchemaAccessError::new("refinement", id.0))
            .store_err_into(out.reborrow())
    }

    pub fn len<'slot, E>(self, mut out: Out<'slot, '_, E>) -> OResult<'slot, usize>
    where
        E: From<SchemaAccessError>,
    {
        let Some(id) = self.head else {
            return Ok(0);
        };
        self.schema
            .records()
            .refinements
            .get(id.0)
            .map(|segment| segment.len)
            .ok_or_else(|| SchemaAccessError::new("refinement", id.0))
            .store_err_into(out.reborrow())
    }

    pub fn is_empty<'slot, E>(self, mut out: Out<'slot, '_, E>) -> OResult<'slot, bool>
    where
        E: From<SchemaAccessError>,
    {
        Ok(self.len(out.reborrow())? == 0)
    }

    pub fn iter<'iter, 'slot>(
        self,
        mut out: Out<'slot, 'iter, SchemaAccessError>,
    ) -> impl Iterator<Item = OResult<'slot, Constraint<'a>>> + 'iter
    where
        'a: 'iter,
        'slot: 'iter,
    {
        self.raw_iter()
            .map(move |result| result.store_err_into(out.reborrow()))
    }

    fn raw_iter(self) -> ConstraintIter<'a> {
        ConstraintIter {
            schema: self.schema,
            head: self.head,
            index: 0,
            tail: self.head,
            tail_index: 0,
        }
    }
}

impl<'a> IntoIterator for ConstraintList<'a> {
    type Item = Result<Constraint<'a>, SchemaAccessError>;
    type IntoIter = ConstraintIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.raw_iter()
    }
}

impl<'a> ConstraintIter<'a> {
    pub fn next_budgeted<'slot, E>(
        &mut self,
        budget: &ExecutionBudget,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, Option<Constraint<'a>>>
    where
        E: From<BudgetError>,
    {
        while let Some(head) = self.head {
            budget.consume(1, out.reborrow())?;
            let segment = self
                .schema
                .records()
                .refinements
                .get(head.0)
                .ok_or(BudgetError::invariant(&CONSTRAINT_ITERATOR_INVARIANT))
                .store_err_into(out.reborrow())?;
            if let Some(id) = segment.rules.get(self.index) {
                self.index += 1;
                let data = self
                    .schema
                    .records()
                    .constraints
                    .get(id.0)
                    .ok_or(BudgetError::invariant(&CONSTRAINT_ITERATOR_INVARIANT))
                    .store_err_into(out.reborrow())?;
                return Ok(Some(Constraint {
                    schema: self.schema,
                    id: *id,
                    data,
                }));
            }
            self.head = segment.next;
            self.index = 0;
        }
        let Some(tail) = self.tail else {
            return Ok(None);
        };
        budget.consume(1, out.reborrow())?;
        let segment = self
            .schema
            .records()
            .refinements
            .get(tail.0)
            .ok_or(BudgetError::invariant(&CONSTRAINT_ITERATOR_INVARIANT))
            .store_err_into(out.reborrow())?;
        let Some(id) = segment.tail.get(self.tail_index) else {
            return Ok(None);
        };
        self.tail_index += 1;
        let data = self
            .schema
            .records()
            .constraints
            .get(id.0)
            .ok_or(BudgetError::invariant(&CONSTRAINT_ITERATOR_INVARIANT))
            .store_err_into(out.reborrow())?;
        Ok(Some(Constraint {
            schema: self.schema,
            id: *id,
            data,
        }))
    }
}

impl<'a> Iterator for ConstraintIter<'a> {
    type Item = Result<Constraint<'a>, SchemaAccessError>;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(head) = self.head {
            let segment = match self.schema.records().refinements.get(head.0) {
                Some(segment) => segment,
                None => {
                    return Some(Err(SchemaAccessError::new("refinement", head.0)));
                }
            };
            if let Some(id) = segment.rules.get(self.index) {
                self.index += 1;
                return Some(
                    self.schema
                        .records()
                        .constraints
                        .get(id.0)
                        .map(|data| Constraint {
                            schema: self.schema,
                            id: *id,
                            data,
                        })
                        .ok_or_else(|| SchemaAccessError::new("constraint", id.0)),
                );
            }
            self.head = segment.next;
            self.index = 0;
        }
        let tail = self.tail?;
        let segment = match self.schema.records().refinements.get(tail.0) {
            Some(segment) => segment,
            None => return Some(Err(SchemaAccessError::new("refinement", tail.0))),
        };
        let id = segment.tail.get(self.tail_index)?;
        self.tail_index += 1;
        Some(
            self.schema
                .records()
                .constraints
                .get(id.0)
                .map(|data| Constraint {
                    schema: self.schema,
                    id: *id,
                    data,
                })
                .ok_or_else(|| SchemaAccessError::new("constraint", id.0)),
        )
    }
}

impl<T: Copy> Bounds<T> {
    pub fn min(&self) -> Option<T> {
        self.min
    }

    pub fn max(&self) -> Option<T> {
        self.max
    }
}

impl<T> Default for Bounds<T> {
    fn default() -> Self {
        Self {
            min: None,
            max: None,
        }
    }
}

impl RefinementSummary {
    pub fn host(&self) -> bool {
        self.host
    }

    pub fn number(&self) -> Bounds<NumberValue> {
        self.number
    }

    pub fn length(&self) -> Bounds<u64> {
        self.length
    }

    pub fn size(&self) -> Bounds<u64> {
        self.size
    }

    pub fn sum(&self) -> Bounds<NumberValue> {
        self.sum
    }

    #[cfg(feature = "compiler")]
    pub(crate) fn include<'slot>(
        &mut self,
        data: &StoredConstraintData,
        mut out: Out<'slot, '_, BuildError>,
    ) -> OResult<'slot, ()> {
        match data {
            StoredConstraintData::Host => self.host = true,
            StoredConstraintData::NumberBounds { min, max } => self
                .number
                .include(*min, *max, compare_number)
                .store_err_into(out.reborrow())?,
            StoredConstraintData::LengthBounds { min, max } => self
                .length
                .include(Some(*min), Some(*max), |a, b| Some(a.cmp(&b)))
                .store_err_into(out.reborrow())?,
            StoredConstraintData::SizeBounds { min, max } => self
                .size
                .include(*min, *max, |a, b| Some(a.cmp(&b)))
                .store_err_into(out.reborrow())?,
            StoredConstraintData::SumBounds { min, max } => self
                .sum
                .include(*min, *max, compare_number)
                .store_err_into(out.reborrow())?,
            _ => {}
        }
        Ok(())
    }
}

#[cfg(feature = "compiler")]
impl<T: Copy> Bounds<T> {
    fn include(
        &mut self,
        min: Option<T>,
        max: Option<T>,
        compare: impl Fn(T, T) -> Option<Ordering>,
    ) -> Result<(), BuildError> {
        if let Some(min) = min {
            if self
                .min
                .is_none_or(|prior| compare(min, prior).is_some_and(|order| order.is_gt()))
            {
                self.min = Some(min);
            }
        }
        if let Some(max) = max {
            if self
                .max
                .is_none_or(|prior| compare(max, prior).is_some_and(|order| order.is_lt()))
            {
                self.max = Some(max);
            }
        }
        if let (Some(min), Some(max)) = (self.min, self.max) {
            if !compare(min, max).is_some_and(|order| !order.is_gt()) {
                return Err(BuildError::internal(CONSTRAINT_BUILD_INTERNAL));
            }
        }
        Ok(())
    }
}

#[cfg(feature = "compiler")]
fn compare_number(a: NumberValue, b: NumberValue) -> Option<Ordering> {
    match (a, b) {
        (NumberValue::Signed(a), NumberValue::Signed(b)) => Some(a.cmp(&b)),
        (NumberValue::Unsigned(a), NumberValue::Unsigned(b)) => Some(a.cmp(&b)),
        (NumberValue::Float(a), NumberValue::Float(b)) => a.partial_cmp(&b),
        _ => None,
    }
}

impl<'a> Constraint<'a> {
    pub fn data<'slot>(
        &self,
        mut out: Out<'slot, '_, SchemaAccessError>,
    ) -> OResult<'slot, ConstraintData<'a>> {
        let raw_data = self.raw_data(out.reborrow())?;
        let data = match raw_data {
            StoredRuleView::NumberBounds { min, max } => ConstraintData::NumberBounds { min, max },
            StoredRuleView::LengthBounds { min, max } => ConstraintData::LengthBounds { min, max },
            StoredRuleView::Regex { index } => ConstraintData::Regex {
                program: RegexView::new(
                    self.schema(),
                    self.schema()
                        .records()
                        .regexes
                        .constraints()
                        .get(index)
                        .ok_or_else(|| SchemaAccessError::new("regex", index))
                        .store_err_into(out.reborrow())?,
                ),
            },
            StoredRuleView::Host => ConstraintData::Host,
            StoredRuleView::Email => ConstraintData::Email,
            StoredRuleView::Cidr { v4_only } => ConstraintData::Cidr { v4_only },
            StoredRuleView::SizeBounds { min, max } => ConstraintData::SizeBounds { min, max },
            StoredRuleView::Unique { by } => {
                let by = match by {
                    Some(ids) => Some(
                        FieldList::unindexed(self.schema(), ids).store_err_into(out.reborrow())?,
                    ),
                    None => None,
                };
                ConstraintData::Unique { by }
            }
            StoredRuleView::All {
                expression,
                message,
            } => ConstraintData::All {
                expression: ExpressionView::new(self.schema(), expression),
                message,
            },
            StoredRuleView::SumBounds { min, max } => ConstraintData::SumBounds { min, max },
        };
        Ok(data)
    }
}

#[cfg(test)]
mod rule_owner_tests {
    use super::{RuleOwner, RuleOwnerError};

    #[test]
    fn rule_owner_error_identifies_the_invalid_inherited_slot() {
        let error = RuleOwnerError::inherited(4);
        let context = error.context();

        assert_eq!(error.owner(), RuleOwner::Inherited);
        assert_eq!(error.index(), Some(4));
        assert_eq!(context.subsystem(), "typol-semantic");
        assert_eq!(context.stage(), "rule-ownership");
        assert_eq!(context.failure_point(), "constraint-list-schema-mismatch");
        assert_eq!(context.object_id(), Some(4));
    }
}
