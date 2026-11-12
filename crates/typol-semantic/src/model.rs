mod access;
mod info;

pub use access::{
    EnvironmentAuthorization, FieldId, SchemaAccessError, SchemaIdentity, TypeId, VariantId,
};
pub use info::{ModelCounts, StoredPayloadInfo, StoredTypeKind};

pub(crate) use self::info::{FieldInfo, FieldInfoParts, VariantInfo, VariantInfoParts};
use crate::{
    constraints::{
        ConstraintList, ConstraintRecord, NumberValue, Refinement, RefinementId,
        VariantRestrictionMode,
    },
    engine::{EvalError, ExecutionField, ExecutionModel, ExecutionType, ExecutionVariant},
    expressions::{AssertionInfo, AssertionList, ProgramId},
    indexes::SchemaIndexes,
    regex::{CompiledRegexConstraints, RegexId},
    views::{RegexCounts, RegexView, TypeView},
};

use core::{cmp::Ordering, fmt};
use opeo::{OResult, Out, ResultOutExt};
use typol_core::{
    BudgetError, ChargedString, ChargedVec, ConfigValue, DiagnosticContext, ExecutionBudget,
    SourceExcerptSet, SourceId, SourceSpan, SpanLocation, diagnostic_context,
};
use typol_model::BuiltinType;

static MODEL_LOCATION_INVARIANT: DiagnosticContext = diagnostic_context!(
    "typol-semantic",
    "schema-model",
    "source-location-index-invalid",
);
static MODEL_VARIANT_INDEX_INVARIANT: DiagnosticContext = diagnostic_context!(
    "typol-semantic",
    "schema-variant-index",
    "variant-slot-missing",
);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ShapeId(pub(crate) usize);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ShapeKind {
    Builtin(BuiltinType),
    Option(ShapeId),
    Array {
        element: ShapeId,
        length: Option<u32>,
    },
    Map(ShapeId),
    Struct(TypeId),
    Enum(TypeId),
}

pub(crate) struct TypeData {
    pub(crate) name: Option<ChargedString>,
    pub(crate) qualified_name: Option<ChargedString>,
    pub(crate) shape: ShapeId,
    pub(crate) child: Option<TypeId>,
    pub(crate) span: SourceSpan,
    pub(crate) fields: ChargedVec<FieldId>,
    pub(crate) variants: ChargedVec<VariantId>,
    pub(crate) refinement: Option<RefinementId>,
    pub(crate) assertions: ChargedVec<ProgramId>,
    pub(crate) secret: bool,
}

pub struct TypeRecord {
    record: TypeData,
}

pub struct TypeRecordOptions {
    pub name: Option<ChargedString>,
    pub qualified_name: Option<ChargedString>,
    pub shape: ShapeId,
    pub child: Option<TypeId>,
    pub span: SourceSpan,
}

pub struct EnvironmentRecord {
    name: ChargedString,
    span: SourceSpan,
}

pub(crate) struct FieldData {
    pub(crate) name: ChargedString,
    pub(crate) ty: TypeId,
    pub(crate) span: SourceSpan,
    pub(crate) default: Option<ConfigValue>,
    pub(crate) refinement: Option<RefinementId>,
    pub(crate) secret: bool,
    pub(crate) environment: Option<EnvironmentRecord>,
    slot: usize,
    environment_slot: usize,
}

pub struct FieldRecord {
    record: FieldData,
}

pub enum StoredVariantPayload {
    Unit,
    Single(TypeId),
    Structure(ChargedVec<FieldId>),
}

pub(crate) struct VariantData {
    pub(crate) name: ChargedString,
    pub(crate) payload: StoredVariantPayload,
    pub(crate) structure_type: Option<TypeId>,
    pub(crate) span: SourceSpan,
    pub(crate) refinement: Option<RefinementId>,
    pub(crate) secret: bool,
    pub(crate) assertions: ChargedVec<ProgramId>,
}

pub struct VariantRecord {
    record: VariantData,
}

pub enum StoredConstraintData {
    NumberBounds {
        min: Option<NumberValue>,
        max: Option<NumberValue>,
    },
    LengthBounds {
        min: u64,
        max: u64,
    },
    Regex {
        regex: RegexId,
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
        by: Option<ChargedVec<FieldId>>,
    },
    All {
        program: ProgramId,
    },
    SumBounds {
        min: Option<NumberValue>,
        max: Option<NumberValue>,
    },
    VariantRestriction {
        mode: VariantRestrictionMode,
        variants: ChargedVec<VariantId>,
    },
}

pub(crate) struct SchemaRecords {
    pub(crate) indexes: SchemaIndexes,
    pub(crate) entry_type: TypeId,
    pub(crate) next_source_id: Option<SourceId>,
    pub(crate) types: ChargedVec<TypeRecord>,
    pub(crate) fields: ChargedVec<FieldRecord>,
    pub(crate) variants: ChargedVec<VariantRecord>,
    pub(crate) regexes: CompiledRegexConstraints,
    pub(crate) locations: ChargedVec<(SourceSpan, SpanLocation)>,
    pub(crate) excerpts: SourceExcerptSet,
    pub(crate) shapes: ChargedVec<ShapeKind>,
    pub(crate) constraints: ChargedVec<ConstraintRecord>,
    pub(crate) refinements: ChargedVec<Refinement>,
    pub(crate) programs: ChargedVec<AssertionInfo>,
}

pub struct CompiledSchema {
    records: SchemaRecords,
    identity: SchemaIdentity,
}

pub(crate) struct TypeInfo<'a> {
    schema: &'a CompiledSchema,
    data: &'a TypeRecord,
    shape: &'a ShapeKind,
    owner: Option<&'a TypeRecord>,
    kind: StoredTypeKind<'a>,
    assertions: &'a [ProgramId],
}

impl CompiledSchema {
    pub const fn identity(&self) -> SchemaIdentity {
        self.identity
    }

    pub fn excerpts(&self) -> &SourceExcerptSet {
        &self.records.excerpts
    }

    #[cfg(feature = "compiler")]
    pub fn with_excerpts(mut self, excerpts: SourceExcerptSet) -> Self {
        self.records.excerpts = excerpts;
        self
    }

    pub fn source_locations(&self) -> &[(SourceSpan, SpanLocation)] {
        &self.records.locations
    }

    pub(crate) fn records(&self) -> &SchemaRecords {
        &self.records
    }

    #[cfg(feature = "compiler")]
    pub(crate) fn records_mut(&mut self) -> &mut SchemaRecords {
        &mut self.records
    }

    #[cfg(feature = "compiler")]
    pub(crate) fn empty<'slot, E>(
        execution: &ExecutionBudget,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, Self>
    where
        E: From<BudgetError>,
    {
        let identity = SchemaIdentity::next(out.reborrow())?;
        Ok(Self {
            identity,
            records: SchemaRecords {
                indexes: SchemaIndexes::new(execution, out.reborrow())?,
                entry_type: TypeId::new(0),
                next_source_id: None,
                types: ChargedVec::new(Some(execution), out.reborrow())?,
                fields: ChargedVec::new(Some(execution), out.reborrow())?,
                variants: ChargedVec::new(Some(execution), out.reborrow())?,
                shapes: ChargedVec::new(Some(execution), out.reborrow())?,
                constraints: ChargedVec::new(Some(execution), out.reborrow())?,
                refinements: ChargedVec::new(Some(execution), out.reborrow())?,
                programs: ChargedVec::new(Some(execution), out.reborrow())?,
                regexes: CompiledRegexConstraints::empty(),
                locations: ChargedVec::new(Some(execution), out.reborrow())?,
                excerpts: SourceExcerptSet::unavailable(),
            },
        })
    }

    pub(crate) fn indexes(&self) -> &SchemaIndexes {
        &self.records.indexes
    }

    #[cfg(feature = "compiler")]
    pub(crate) fn set_indexes(&mut self, indexes: SchemaIndexes) {
        self.records.indexes = indexes;
    }

    pub fn model_counts(&self) -> ModelCounts {
        ModelCounts {
            shapes: self.records.shapes.len(),
            type_uses: self.records.types.len(),
            constraints: self.records.constraints.len(),
            refinements: self.records.refinements.len(),
            programs: self.records.programs.len(),
            expression_nodes: self
                .records
                .programs
                .iter()
                .map(|p| p.expression().node_count())
                .sum(),
        }
    }

    pub(crate) fn same_type(&self, left: TypeId, right: TypeId) -> Result<bool, SchemaAccessError> {
        let left_shape = self
            .type_info(left)?
            .ok_or_else(|| SchemaAccessError::new("type", left.index()))?
            .data
            .record
            .shape;
        let right_shape = self
            .type_info(right)?
            .ok_or_else(|| SchemaAccessError::new("type", right.index()))?
            .data
            .record
            .shape;
        Ok(left_shape == right_shape)
    }

    pub(crate) fn entry_type(&self) -> TypeId {
        self.records.entry_type
    }

    /// Returns the next source handle available after the loaded Schema sources.
    pub const fn next_source_id(&self) -> Option<SourceId> {
        self.records.next_source_id
    }

    pub(crate) fn type_info(&self, id: TypeId) -> Result<Option<TypeInfo<'_>>, SchemaAccessError> {
        let Some(data) = self.records.types.get(id.index()) else {
            return Ok(None);
        };
        let shape = self
            .records
            .shapes
            .get(data.record.shape.0)
            .ok_or_else(|| SchemaAccessError::new("shape", data.record.shape.0))?;
        let owner = match shape {
            ShapeKind::Struct(owner) | ShapeKind::Enum(owner) => Some(
                self.records
                    .types
                    .get(owner.index())
                    .ok_or_else(|| SchemaAccessError::new("type owner", owner.index()))?,
            ),
            _ => None,
        };
        let child = match shape {
            ShapeKind::Option(_) | ShapeKind::Array { .. } | ShapeKind::Map(_) => Some(
                data.record
                    .child
                    .ok_or_else(|| SchemaAccessError::new("child type", id.index()))?,
            ),
            _ => None,
        };
        let kind = match shape {
            ShapeKind::Builtin(builtin) => StoredTypeKind::Builtin(*builtin),
            ShapeKind::Option(_) => StoredTypeKind::Option {
                inner: child.ok_or_else(|| SchemaAccessError::new("child type", id.index()))?,
            },
            ShapeKind::Array { length, .. } => StoredTypeKind::Array {
                element: child.ok_or_else(|| SchemaAccessError::new("child type", id.index()))?,
                length: *length,
            },
            ShapeKind::Map(_) => StoredTypeKind::Map {
                value: child.ok_or_else(|| SchemaAccessError::new("child type", id.index()))?,
            },
            ShapeKind::Struct(_) => StoredTypeKind::Struct {
                fields: &owner
                    .ok_or_else(|| SchemaAccessError::new("type owner", id.index()))?
                    .record
                    .fields,
            },
            ShapeKind::Enum(_) => StoredTypeKind::Enum {
                variants: &owner
                    .ok_or_else(|| SchemaAccessError::new("type owner", id.index()))?
                    .record
                    .variants,
            },
        };
        let assertions = match shape {
            ShapeKind::Struct(_) | ShapeKind::Enum(_) => {
                &owner
                    .ok_or_else(|| SchemaAccessError::new("type owner", id.index()))?
                    .record
                    .assertions
            }
            _ => &data.record.assertions,
        };
        Ok(Some(TypeInfo {
            schema: self,
            data,
            shape,
            owner,
            kind,
            assertions,
        }))
    }

    pub(crate) fn field_info(
        &self,
        id: FieldId,
    ) -> Result<Option<FieldInfo<'_>>, SchemaAccessError> {
        Ok(self.records.fields.get(id.index()).map(|data| {
            let environment = data.record.environment.as_ref().map(|environment| {
                EnvironmentAuthorization::new(
                    self,
                    data.record.environment_slot,
                    &environment.name,
                    environment.span,
                )
            });
            FieldInfo::new(FieldInfoParts {
                slot: data.record.slot,
                name: &data.record.name,
                ty: data.record.ty,
                span: data.record.span,
                default: data.record.default.as_ref(),
                constraints: ConstraintList::new(self, data.record.refinement),
                secret: data.record.secret,
                environment,
            })
        }))
    }

    pub(crate) fn variant_info(
        &self,
        id: VariantId,
    ) -> Result<Option<VariantInfo<'_>>, SchemaAccessError> {
        Ok(self.records.variants.get(id.index()).map(|data| {
            let payload = match &data.record.payload {
                StoredVariantPayload::Unit => StoredPayloadInfo::Unit,
                StoredVariantPayload::Single(ty) => StoredPayloadInfo::Single { ty: *ty },
                StoredVariantPayload::Structure(fields) => StoredPayloadInfo::Structure {
                    fields: fields.as_slice(),
                },
            };
            VariantInfo::new(VariantInfoParts {
                structure_type: data.record.structure_type,
                name: &data.record.name,
                payload,
                span: data.record.span,
                constraints: ConstraintList::new(self, data.record.refinement),
                secret: data.record.secret,
                assertions: AssertionList::new(&self.records.programs, &data.record.assertions),
            })
        }))
    }

    pub fn span_location_budgeted<'slot, E>(
        &self,
        span: SourceSpan,
        budget: &ExecutionBudget,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, Option<SpanLocation>>
    where
        E: From<BudgetError>,
    {
        let mut low = 0;
        let mut high = self.records.locations.len();
        while low < high {
            budget.consume(1, out.reborrow())?;
            let middle = low + (high - low) / 2;
            let (saved, location) = self
                .records
                .locations
                .get(middle)
                .copied()
                .ok_or(BudgetError::invariant(&MODEL_LOCATION_INVARIANT))
                .store_err_into(out.reborrow())?;
            match compare_spans(saved, span) {
                Ordering::Less => low = middle + 1,
                Ordering::Greater => high = middle,
                Ordering::Equal => return Ok(Some(location)),
            }
        }
        Ok(None)
    }

    pub fn span_location(&self, span: SourceSpan) -> Option<SpanLocation> {
        find_location(&self.records.locations, span)
    }
}

impl<'a> TypeInfo<'a> {
    pub fn is_secret(&self) -> bool {
        self.data.record.secret
    }

    pub fn kind(&self) -> StoredTypeKind<'a> {
        self.kind
    }

    pub fn name(&self) -> Option<&'a str> {
        self.data
            .record
            .name
            .as_deref()
            .or_else(|| match self.shape {
                ShapeKind::Struct(_) | ShapeKind::Enum(_) => {
                    self.owner.and_then(|owner| owner.record.name.as_deref())
                }
                _ => None,
            })
    }

    pub fn qualified_name(&self) -> Option<&'a str> {
        self.data
            .record
            .qualified_name
            .as_deref()
            .or_else(|| match self.shape {
                ShapeKind::Struct(_) | ShapeKind::Enum(_) => self
                    .owner
                    .and_then(|owner| owner.record.qualified_name.as_deref()),
                _ => None,
            })
    }

    pub(crate) fn nominal_name(&self) -> Option<&'a str> {
        match self.shape {
            ShapeKind::Struct(_) | ShapeKind::Enum(_) => {
                self.owner.and_then(|owner| owner.record.name.as_deref())
            }
            _ => None,
        }
    }

    pub fn declaration_span(&self) -> Option<SourceSpan> {
        match self.shape {
            ShapeKind::Struct(_) | ShapeKind::Enum(_) => self.owner.map(|owner| owner.record.span),
            _ => self
                .data
                .record
                .name
                .as_ref()
                .map(|_| self.data.record.span),
        }
    }

    pub fn span(&self) -> SourceSpan {
        self.data.record.span
    }

    pub fn fields(&self) -> &'a [FieldId] {
        match self.kind() {
            StoredTypeKind::Struct { fields } => fields,
            _ => &[],
        }
    }

    pub fn variants(&self) -> &'a [VariantId] {
        match self.kind() {
            StoredTypeKind::Enum { variants } => variants,
            _ => &[],
        }
    }

    pub fn constraints(&self) -> ConstraintList<'a> {
        ConstraintList::new(self.schema, self.data.record.refinement)
    }

    pub fn assertions(&self) -> AssertionList<'a> {
        AssertionList::new(&self.schema.records.programs, self.assertions)
    }
}

impl fmt::Debug for CompiledSchema {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CompiledSchema")
            .field("type_count", &self.records.types.len())
            .field("field_count", &self.records.fields.len())
            .field("variant_count", &self.records.variants.len())
            .field("regex_count", &self.records.regexes.constraints().len())
            .field("span_count", &self.records.locations.len())
            .finish()
    }
}

fn find_location(records: &[(SourceSpan, SpanLocation)], span: SourceSpan) -> Option<SpanLocation> {
    records
        .binary_search_by(|(candidate, _)| compare_spans(*candidate, span))
        .ok()
        .and_then(|index| records.get(index).map(|(_, location)| *location))
}

fn compare_spans(left: SourceSpan, right: SourceSpan) -> Ordering {
    left.source_id()
        .get()
        .cmp(&right.source_id().get())
        .then_with(|| left.span().start().cmp(&right.span().start()))
        .then_with(|| left.span().end().cmp(&right.span().end()))
}

#[cfg(feature = "compiler")]
impl TypeRecord {
    pub(crate) fn record(&self) -> &TypeData {
        &self.record
    }

    pub fn new<'slot, E>(
        options: TypeRecordOptions,
        execution: &ExecutionBudget,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, Self>
    where
        E: From<BudgetError>,
    {
        Ok(Self {
            record: TypeData {
                name: options.name,
                qualified_name: options.qualified_name,
                shape: options.shape,
                child: options.child,
                span: options.span,
                fields: ChargedVec::new(Some(execution), out.reborrow())?,
                variants: ChargedVec::new(Some(execution), out.reborrow())?,
                refinement: None,
                assertions: ChargedVec::new(Some(execution), out.reborrow())?,
                secret: false,
            },
        })
    }

    pub fn set_secret(&mut self, value: bool) {
        self.record.secret = value;
    }

    pub fn set_fields(&mut self, value: ChargedVec<FieldId>) {
        self.record.fields = value;
    }

    pub fn set_variants(&mut self, value: ChargedVec<VariantId>) {
        self.record.variants = value;
    }

    pub fn set_refinement(&mut self, value: Option<RefinementId>) {
        self.record.refinement = value;
    }

    pub fn set_assertions(&mut self, value: ChargedVec<ProgramId>) {
        self.record.assertions = value;
    }

    pub fn shape(&self) -> ShapeId {
        self.record.shape
    }

    pub fn span(&self) -> SourceSpan {
        self.record.span
    }
}

#[cfg(feature = "compiler")]
impl FieldRecord {
    pub(crate) fn record(&self) -> &FieldData {
        &self.record
    }

    pub(crate) fn set_slot(&mut self, slot: usize) {
        self.record.slot = slot;
    }

    pub(crate) fn set_environment_slot(&mut self, slot: usize) {
        self.record.environment_slot = slot;
    }

    pub fn new(name: ChargedString, ty: TypeId, span: SourceSpan) -> Self {
        Self {
            record: FieldData {
                name,
                ty,
                span,
                default: None,
                refinement: None,
                secret: false,
                environment: None,
                slot: 0,
                environment_slot: 0,
            },
        }
    }

    pub fn set_default(&mut self, value: Option<ConfigValue>) {
        self.record.default = value;
    }

    pub fn set_refinement(&mut self, value: Option<RefinementId>) {
        self.record.refinement = value;
    }

    pub fn set_secret(&mut self, value: bool) {
        self.record.secret = value;
    }

    pub fn set_environment(&mut self, value: Option<EnvironmentRecord>) {
        self.record.environment = value;
    }
}

#[cfg(feature = "compiler")]
impl VariantRecord {
    pub(crate) fn record(&self) -> &VariantData {
        &self.record
    }

    pub fn set_structure_type(&mut self, ty: TypeId) {
        self.record.structure_type = Some(ty);
    }

    pub fn new<'slot, E>(
        name: ChargedString,
        payload: StoredVariantPayload,
        span: SourceSpan,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, Self>
    where
        E: From<BudgetError>,
    {
        let assertions = ChargedVec::new(name.budget(), out.reborrow())?;
        Ok(Self {
            record: VariantData {
                name,
                payload,
                structure_type: None,
                span,
                refinement: None,
                secret: false,
                assertions,
            },
        })
    }

    pub fn set_refinement(&mut self, value: Option<RefinementId>) {
        self.record.refinement = value;
    }

    pub fn set_secret(&mut self, value: bool) {
        self.record.secret = value;
    }

    pub fn set_assertions(&mut self, value: ChargedVec<ProgramId>) {
        self.record.assertions = value;
    }
}

#[cfg(feature = "compiler")]
impl EnvironmentRecord {
    pub fn new(name: ChargedString, span: SourceSpan) -> Self {
        Self { name, span }
    }

    pub(crate) fn name(&self) -> &str {
        self.name.as_str()
    }

    pub fn span(&self) -> SourceSpan {
        self.span
    }
}

impl ExecutionModel for CompiledSchema {
    fn type_info<'slot>(
        &self,
        id: TypeId,
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, Option<ExecutionType<'_>>> {
        let info = CompiledSchema::type_info(self, id).store_err_into(out.reborrow())?;
        Ok(info.map(|info| ExecutionType::new(info.kind())))
    }

    fn field_info<'slot>(
        &self,
        id: FieldId,
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, Option<ExecutionField<'_>>> {
        let Some(info) = CompiledSchema::field_info(self, id).store_err_into(out.reborrow())?
        else {
            return Ok(None);
        };
        let host = info.constraints().has_host(out.reborrow())?;
        Ok(Some(ExecutionField::new(
            info.name(),
            info.ty(),
            host,
            info.slot(),
        )))
    }

    fn variant_info<'slot>(
        &self,
        id: VariantId,
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, Option<ExecutionVariant<'_>>> {
        let Some(info) = CompiledSchema::variant_info(self, id).store_err_into(out.reborrow())?
        else {
            return Ok(None);
        };
        let host = info.constraints().has_host(out.reborrow())?;
        Ok(Some(ExecutionVariant::new(
            info.name(),
            info.payload(),
            host,
        )))
    }

    fn same_type<'slot>(
        &self,
        left: TypeId,
        right: TypeId,
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, bool> {
        CompiledSchema::same_type(self, left, right).store_err_into(out.reborrow())
    }

    fn host<'slot>(
        &self,
        id: TypeId,
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, Option<bool>> {
        let Some(info) = CompiledSchema::type_info(self, id).store_err_into(out.reborrow())? else {
            return Ok(None);
        };
        let host = info.constraints().has_host(out.reborrow())?;
        Ok(Some(host))
    }

    fn entry_type<'slot, 'borrow>(
        &self,
        _out: Out<'slot, 'borrow, EvalError>,
    ) -> OResult<'slot, Option<TypeId>> {
        Ok(Some(CompiledSchema::entry_type(self)))
    }

    fn variant_named<'slot>(
        &self,
        owner: TypeId,
        name: &str,
        budget: &ExecutionBudget,
        mut out: Out<'slot, '_, EvalError>,
    ) -> OResult<'slot, Option<VariantId>> {
        let type_info = CompiledSchema::type_info(self, owner).store_err_into(out.reborrow())?;
        let Some(type_info) = type_info else {
            return Ok(None);
        };
        let variants = type_info.variants();
        let Some(first) = variants.first() else {
            return Ok(None);
        };
        let slots = self
            .indexes()
            .variant_slots(*first)
            .map_err(|_| BudgetError::invariant(&MODEL_VARIANT_INDEX_INVARIANT))
            .store_err_into(out.reborrow())?;
        let mut low = 0;
        let mut high = slots.len();
        while low < high {
            budget.consume(1, out.reborrow())?;
            let middle = low + (high - low) / 2;
            let slot = *slots
                .get(middle)
                .ok_or(BudgetError::invariant(&MODEL_VARIANT_INDEX_INVARIANT))
                .store_err_into(out.reborrow())?;
            let id = *variants
                .get(slot)
                .ok_or(BudgetError::invariant(&MODEL_VARIANT_INDEX_INVARIANT))
                .store_err_into(out.reborrow())?;
            let variant_info =
                CompiledSchema::variant_info(self, id).store_err_into(out.reborrow())?;
            let variant = variant_info
                .ok_or(BudgetError::invariant(&MODEL_VARIANT_INDEX_INVARIANT))
                .store_err_into(out.reborrow())?;
            match budget.compare_text(variant.name(), name, out.reborrow())? {
                Ordering::Less => low = middle + 1,
                Ordering::Greater => high = middle,
                Ordering::Equal => return Ok(Some(id)),
            }
        }
        Ok(None)
    }
}

impl CompiledSchema {
    pub fn entry(&self) -> TypeView<'_> {
        self.view(self.records().entry_type)
    }

    pub(crate) fn view(&self, id: TypeId) -> TypeView<'_> {
        TypeView::new(self, id)
    }
}

impl CompiledSchema {
    pub fn types(&self) -> impl ExactSizeIterator<Item = TypeView<'_>> {
        (0..self.records().types.len()).map(|index| self.view(TypeId::from_index(index)))
    }

    pub fn regex_programs(&self) -> impl ExactSizeIterator<Item = RegexView<'_>> {
        self.records()
            .regexes
            .constraints()
            .iter()
            .map(|raw| RegexView::new(self, raw))
    }

    pub fn regex_counts(&self) -> RegexCounts {
        RegexCounts {
            programs: self.records().regexes.constraints().len(),
            pattern_bytes: self.records().regexes.pattern_bytes(),
            instructions: self.records().regexes.instruction_count(),
            ranges: self.records().regexes.range_count(),
        }
    }
}
