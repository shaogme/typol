mod assertions;
mod expression;
mod kinds;

use crate::{
    ConstraintList, EnvironmentAuthorization,
    model::{
        CompiledSchema, FieldId, FieldInfo, SchemaAccessError, StoredPayloadInfo, StoredTypeKind,
        TypeId, TypeInfo, VariantId, VariantInfo,
    },
};
use core::{cmp::Ordering, fmt, ptr, slice::Iter};
use opeo::{OResult, Out, ResultOutExt};
use typol_core::{
    BudgetError, ConfigValue, DiagnosticContext, ExecutionBudget, SourceSpan, diagnostic_context,
};

pub use crate::expressions::{ExpressionLiteral, ExpressionMethodInfo};
pub use assertions::{AssertionIter, AssertionList, AssertionView};
pub use expression::{
    ClosureView, ExpressionArgument, ExpressionBindingInfo, ExpressionKind, ExpressionNodeView,
    ExpressionView,
};
pub use kinds::{ConstraintData, RegexCounts, RegexView, TypeKind, VariantPayloadInfo};

static VIEW_INDEX_INVARIANT: DiagnosticContext =
    diagnostic_context!("typol-semantic", "schema-view", "field-index-entry-missing",);

/// A type-use view bound to its owning Schema; navigation does not expose internal handles.
/// Ordering includes the owner and exact use site for in-process indexing only; it is not stable across runs.
#[derive(Clone, Copy)]
pub struct TypeView<'a> {
    schema: &'a CompiledSchema,
    id: TypeId,
}

#[derive(Clone, Copy)]
pub struct FieldView<'a> {
    schema: &'a CompiledSchema,
    id: FieldId,
}

#[derive(Clone, Copy)]
pub struct VariantView<'a> {
    schema: &'a CompiledSchema,
    id: VariantId,
}

#[derive(Clone, Copy)]
pub struct FieldList<'a> {
    schema: &'a CompiledSchema,
    ids: &'a [FieldId],
    sorted: Option<&'a [usize]>,
}

#[derive(Clone, Copy)]
pub struct VariantList<'a> {
    schema: &'a CompiledSchema,
    ids: &'a [VariantId],
}

pub struct ViewIter<'a, I, V> {
    schema: &'a CompiledSchema,
    ids: Iter<'a, I>,
    make: fn(&'a CompiledSchema, I) -> V,
}

impl PartialEq for TypeView<'_> {
    fn eq(&self, other: &Self) -> bool {
        ptr::eq(self.schema, other.schema) && self.id == other.id
    }
}

impl Eq for TypeView<'_> {}

impl PartialOrd for TypeView<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TypeView<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        // Ordering is for in-process indexing only and preserves the identity of the owning Schema and exact use site.
        ptr::from_ref(self.schema)
            .cmp(&ptr::from_ref(other.schema))
            .then_with(|| self.id.cmp(&other.id))
    }
}

impl fmt::Debug for TypeView<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TypeView")
            .field("id", &self.id.index())
            .finish()
    }
}

impl<'a> TypeView<'a> {
    pub(crate) fn schema(self) -> &'a CompiledSchema {
        self.schema
    }

    pub(crate) fn id(self) -> TypeId {
        self.id
    }

    pub(crate) fn new(schema: &'a CompiledSchema, id: TypeId) -> Self {
        Self { schema, id }
    }

    fn info(self) -> Result<TypeInfo<'a>, SchemaAccessError> {
        self.schema
            .type_info(self.id)?
            .ok_or_else(|| SchemaAccessError::new("type", self.id.index()))
    }

    pub fn span<'slot>(
        self,
        mut out: Out<'slot, '_, SchemaAccessError>,
    ) -> OResult<'slot, SourceSpan> {
        Ok(self.info().store_err_into(out.reborrow())?.span())
    }

    pub fn constraints<'slot>(
        self,
        mut out: Out<'slot, '_, SchemaAccessError>,
    ) -> OResult<'slot, ConstraintList<'a>> {
        Ok(self.info().store_err_into(out.reborrow())?.constraints())
    }

    pub fn is_secret<'slot>(
        self,
        mut out: Out<'slot, '_, SchemaAccessError>,
    ) -> OResult<'slot, bool> {
        Ok(self.info().store_err_into(out.reborrow())?.is_secret())
    }

    pub fn name<'slot>(
        self,
        mut out: Out<'slot, '_, SchemaAccessError>,
    ) -> OResult<'slot, Option<&'a str>> {
        Ok(self.info().store_err_into(out.reborrow())?.name())
    }

    pub fn qualified_name<'slot>(
        self,
        mut out: Out<'slot, '_, SchemaAccessError>,
    ) -> OResult<'slot, Option<&'a str>> {
        Ok(self.info().store_err_into(out.reborrow())?.qualified_name())
    }

    /// Returns the declared short name of the underlying struct or enum; alias names do not participate in nominal matching.
    pub fn nominal_name<'slot>(
        self,
        mut out: Out<'slot, '_, SchemaAccessError>,
    ) -> OResult<'slot, Option<&'a str>> {
        Ok(self.info().store_err_into(out.reborrow())?.nominal_name())
    }

    pub fn declaration_span<'slot>(
        self,
        mut out: Out<'slot, '_, SchemaAccessError>,
    ) -> OResult<'slot, Option<SourceSpan>> {
        Ok(self
            .info()
            .store_err_into(out.reborrow())?
            .declaration_span())
    }

    pub fn same_type<'slot>(
        self,
        other: Self,
        mut out: Out<'slot, '_, SchemaAccessError>,
    ) -> OResult<'slot, bool> {
        if !ptr::eq(self.schema, other.schema) {
            return Ok(false);
        }
        self.schema
            .same_type(self.id, other.id)
            .store_err_into(out.reborrow())
    }

    pub fn kind<'slot>(
        self,
        mut out: Out<'slot, '_, SchemaAccessError>,
    ) -> OResult<'slot, TypeKind<'a>> {
        let info = self.info().store_err_into(out.reborrow())?;
        let kind = match info.kind() {
            StoredTypeKind::Builtin(b) => TypeKind::Builtin(b),
            StoredTypeKind::Option { inner } => TypeKind::Option {
                inner: self.schema.view(inner),
            },
            StoredTypeKind::Array { element, length } => TypeKind::Array {
                element: self.schema.view(element),
                length,
            },
            StoredTypeKind::Map { value } => TypeKind::Map {
                value: self.schema.view(value),
            },
            StoredTypeKind::Struct { fields } => TypeKind::Struct {
                fields: {
                    let sorted = match fields.first() {
                        Some(id) => Some(
                            self.schema
                                .indexes()
                                .field_slots(*id)
                                .store_err_into(out.reborrow())?,
                        ),
                        None => None,
                    };
                    FieldList {
                        schema: self.schema,
                        ids: fields,
                        sorted,
                    }
                },
            },
            StoredTypeKind::Enum { variants } => TypeKind::Enum {
                variants: VariantList {
                    schema: self.schema,
                    ids: variants,
                },
            },
        };
        Ok(kind)
    }

    pub fn fields<'slot>(
        self,
        mut out: Out<'slot, '_, SchemaAccessError>,
    ) -> OResult<'slot, FieldList<'a>> {
        let fields = self.info().store_err_into(out.reborrow())?.fields();
        let sorted = match fields.first() {
            Some(id) => Some(
                self.schema
                    .indexes()
                    .field_slots(*id)
                    .store_err_into(out.reborrow())?,
            ),
            None => None,
        };
        Ok(FieldList {
            schema: self.schema,
            ids: fields,
            sorted,
        })
    }

    pub fn variants<'slot>(
        self,
        mut out: Out<'slot, '_, SchemaAccessError>,
    ) -> OResult<'slot, VariantList<'a>> {
        Ok(VariantList {
            schema: self.schema,
            ids: self.info().store_err_into(out.reborrow())?.variants(),
        })
    }

    pub fn field_named<'slot>(
        self,
        name: &str,
        mut out: Out<'slot, '_, SchemaAccessError>,
    ) -> OResult<'slot, Option<FieldView<'a>>> {
        let fields = self.fields(out.reborrow())?;
        let found = fields.named(name, out.reborrow())?;
        Ok(found.map(|(_, field)| field))
    }

    pub fn variant_named<'slot>(
        self,
        name: &str,
        mut out: Out<'slot, '_, SchemaAccessError>,
    ) -> OResult<'slot, Option<VariantView<'a>>> {
        let variants = self.info().store_err_into(out.reborrow())?.variants();
        let Some(first) = variants.first() else {
            return Ok(None);
        };
        let slots = self
            .schema
            .indexes()
            .variant_slots(*first)
            .store_err_into(out.reborrow())?;
        let mut low = 0;
        let mut high = slots.len();
        while low < high {
            let middle = low + (high - low) / 2;
            let slot = *slots
                .get(middle)
                .ok_or_else(|| SchemaAccessError::new("variant sort slot", middle))
                .store_err_into(out.reborrow())?;
            let id = *variants
                .get(slot)
                .ok_or_else(|| SchemaAccessError::new("variant list slot", slot))
                .store_err_into(out.reborrow())?;
            let variant = VariantView {
                schema: self.schema,
                id,
            };
            match variant
                .info()
                .store_err_into(out.reborrow())?
                .name()
                .cmp(name)
            {
                Ordering::Less => low = middle + 1,
                Ordering::Greater => high = middle,
                Ordering::Equal => return Ok(Some(variant)),
            }
        }
        Ok(None)
    }

    pub fn variant_named_budgeted<'slot>(
        self,
        name: &str,
        budget: &ExecutionBudget,
        mut out: Out<'slot, '_, BudgetError>,
    ) -> OResult<'slot, Option<VariantView<'a>>> {
        let variants = self
            .info()
            .map(|info| info.variants())
            .map_err(|_| BudgetError::invariant(&VIEW_INDEX_INVARIANT))
            .store_err_into(out.reborrow())?;
        let Some(first) = variants.first() else {
            return Ok(None);
        };
        let slots = self
            .schema
            .indexes()
            .variant_slots(*first)
            .map_err(|_| BudgetError::invariant(&VIEW_INDEX_INVARIANT))
            .store_err_into(out.reborrow())?;
        let mut low = 0;
        let mut high = slots.len();
        while low < high {
            budget.consume(1, out.reborrow())?;
            let middle = low + (high - low) / 2;
            let slot = *slots
                .get(middle)
                .ok_or(BudgetError::invariant(&VIEW_INDEX_INVARIANT))
                .store_err_into(out.reborrow())?;
            let id = *variants
                .get(slot)
                .ok_or(BudgetError::invariant(&VIEW_INDEX_INVARIANT))
                .store_err_into(out.reborrow())?;
            let variant = VariantView {
                schema: self.schema,
                id,
            };
            let variant_name = variant
                .info()
                .map(|info| info.name())
                .map_err(|_| BudgetError::invariant(&VIEW_INDEX_INVARIANT))
                .store_err_into(out.reborrow())?;
            match budget.compare_text(variant_name, name, out.reborrow())? {
                Ordering::Less => low = middle + 1,
                Ordering::Greater => high = middle,
                Ordering::Equal => return Ok(Some(variant)),
            }
        }
        Ok(None)
    }

    pub fn assertions<'slot>(
        self,
        mut out: Out<'slot, '_, SchemaAccessError>,
    ) -> OResult<'slot, AssertionList<'a>> {
        Ok(AssertionList::new(
            self.schema,
            self.info().store_err_into(out.reborrow())?.assertions(),
        ))
    }
}

impl PartialEq for FieldView<'_> {
    fn eq(&self, other: &Self) -> bool {
        ptr::eq(self.schema, other.schema) && self.id == other.id
    }
}

impl Eq for FieldView<'_> {}

impl fmt::Debug for FieldView<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FieldView")
            .field("id", &self.id.index())
            .finish()
    }
}

impl<'a> FieldView<'a> {
    /// Declared slot in its struct or anonymous variant; aliases retain the original order.
    pub fn slot<'slot>(self, mut out: Out<'slot, '_, SchemaAccessError>) -> OResult<'slot, usize> {
        Ok(self.info().store_err_into(out.reborrow())?.slot())
    }

    fn info(self) -> Result<FieldInfo<'a>, SchemaAccessError> {
        self.schema
            .field_info(self.id)?
            .ok_or_else(|| SchemaAccessError::new("field", self.id.index()))
    }

    pub fn span<'slot>(
        self,
        mut out: Out<'slot, '_, SchemaAccessError>,
    ) -> OResult<'slot, SourceSpan> {
        Ok(self.info().store_err_into(out.reborrow())?.span())
    }

    pub fn constraints<'slot>(
        self,
        mut out: Out<'slot, '_, SchemaAccessError>,
    ) -> OResult<'slot, ConstraintList<'a>> {
        Ok(self.info().store_err_into(out.reborrow())?.constraints())
    }

    pub fn is_secret<'slot>(
        self,
        mut out: Out<'slot, '_, SchemaAccessError>,
    ) -> OResult<'slot, bool> {
        Ok(self.info().store_err_into(out.reborrow())?.is_secret())
    }

    pub fn name<'slot>(
        self,
        mut out: Out<'slot, '_, SchemaAccessError>,
    ) -> OResult<'slot, &'a str> {
        Ok(self.info().store_err_into(out.reborrow())?.name())
    }

    pub fn ty<'slot>(
        self,
        mut out: Out<'slot, '_, SchemaAccessError>,
    ) -> OResult<'slot, TypeView<'a>> {
        let ty = self.info().store_err_into(out.reborrow())?.ty();
        self.schema
            .type_info(ty)
            .and_then(|info| info.ok_or_else(|| SchemaAccessError::new("type", ty.index())))
            .store_err_into(out.reborrow())?;
        Ok(self.schema.view(ty))
    }

    pub fn default_value<'slot>(
        self,
        mut out: Out<'slot, '_, SchemaAccessError>,
    ) -> OResult<'slot, Option<&'a ConfigValue>> {
        Ok(self.info().store_err_into(out.reborrow())?.default_value())
    }

    pub fn environment<'slot>(
        self,
        mut out: Out<'slot, '_, SchemaAccessError>,
    ) -> OResult<'slot, Option<EnvironmentAuthorization<'a>>> {
        Ok(self.info().store_err_into(out.reborrow())?.environment())
    }
}

impl PartialEq for VariantView<'_> {
    fn eq(&self, other: &Self) -> bool {
        ptr::eq(self.schema, other.schema) && self.id == other.id
    }
}

impl Eq for VariantView<'_> {}

impl fmt::Debug for VariantView<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VariantView")
            .field("id", &self.id.index())
            .finish()
    }
}

impl<'a> VariantView<'a> {
    fn info(self) -> Result<VariantInfo<'a>, SchemaAccessError> {
        self.schema
            .variant_info(self.id)?
            .ok_or_else(|| SchemaAccessError::new("variant", self.id.index()))
    }

    pub fn span<'slot>(
        self,
        mut out: Out<'slot, '_, SchemaAccessError>,
    ) -> OResult<'slot, SourceSpan> {
        Ok(self.info().store_err_into(out.reborrow())?.span())
    }

    pub fn constraints<'slot>(
        self,
        mut out: Out<'slot, '_, SchemaAccessError>,
    ) -> OResult<'slot, ConstraintList<'a>> {
        Ok(self.info().store_err_into(out.reborrow())?.constraints())
    }

    pub fn is_secret<'slot>(
        self,
        mut out: Out<'slot, '_, SchemaAccessError>,
    ) -> OResult<'slot, bool> {
        Ok(self.info().store_err_into(out.reborrow())?.is_secret())
    }

    pub fn name<'slot>(
        self,
        mut out: Out<'slot, '_, SchemaAccessError>,
    ) -> OResult<'slot, &'a str> {
        Ok(self.info().store_err_into(out.reborrow())?.name())
    }

    pub fn structure_type<'slot>(
        self,
        mut out: Out<'slot, '_, SchemaAccessError>,
    ) -> OResult<'slot, Option<TypeView<'a>>> {
        let Some(id) = self.info().store_err_into(out.reborrow())?.structure_type() else {
            return Ok(None);
        };
        self.schema
            .type_info(id)
            .and_then(|info| info.ok_or_else(|| SchemaAccessError::new("type", id.index())))
            .store_err_into(out.reborrow())?;
        Ok(Some(self.schema.view(id)))
    }

    pub fn payload<'slot>(
        self,
        mut out: Out<'slot, '_, SchemaAccessError>,
    ) -> OResult<'slot, VariantPayloadInfo<'a>> {
        let info = self.info().store_err_into(out.reborrow())?;
        let payload = match info.payload() {
            StoredPayloadInfo::Unit => VariantPayloadInfo::Unit,
            StoredPayloadInfo::Single { ty } => VariantPayloadInfo::Single {
                ty: self.schema.view(ty),
            },
            StoredPayloadInfo::Structure { fields } => VariantPayloadInfo::Structure {
                fields: {
                    let sorted = match fields.first() {
                        Some(id) => Some(
                            self.schema
                                .indexes()
                                .field_slots(*id)
                                .store_err_into(out.reborrow())?,
                        ),
                        None => None,
                    };
                    FieldList {
                        schema: self.schema,
                        ids: fields,
                        sorted,
                    }
                },
            },
        };
        Ok(payload)
    }

    pub fn assertions<'slot>(
        self,
        mut out: Out<'slot, '_, SchemaAccessError>,
    ) -> OResult<'slot, AssertionList<'a>> {
        Ok(AssertionList::new(
            self.schema,
            self.info().store_err_into(out.reborrow())?.assertions(),
        ))
    }
}

impl<'a> FieldList<'a> {
    pub(crate) fn schema(self) -> &'a CompiledSchema {
        self.schema
    }

    pub(crate) fn ids(self) -> &'a [FieldId] {
        self.ids
    }

    pub(crate) fn unindexed(
        schema: &'a CompiledSchema,
        ids: &'a [FieldId],
    ) -> Result<Self, SchemaAccessError> {
        for id in ids {
            schema
                .field_info(*id)?
                .ok_or_else(|| SchemaAccessError::new("field", id.index()))?;
        }
        Ok(Self {
            schema,
            ids,
            sorted: None,
        })
    }

    /// Declared fields are looked up through the sorted index, returning both the list slot and an owner-bound view.
    /// The `unique` projection preserves its specified order and does not use the declaration-wide name index.
    pub fn named<'slot>(
        self,
        name: &str,
        mut out: Out<'slot, '_, SchemaAccessError>,
    ) -> OResult<'slot, Option<(usize, FieldView<'a>)>> {
        let Some(slots) = self.sorted else {
            for (slot, field) in self.iter().enumerate() {
                if field.name(out.reborrow())? == name {
                    return Ok(Some((slot, field)));
                }
            }
            return Ok(None);
        };
        let mut low = 0;
        let mut high = slots.len();
        while low < high {
            let middle = low + (high - low) / 2;
            let slot = *slots
                .get(middle)
                .ok_or_else(|| SchemaAccessError::new("field sort slot", middle))
                .store_err_into(out.reborrow())?;
            let field = self
                .get(slot)
                .ok_or_else(|| SchemaAccessError::new("field list slot", slot))
                .store_err_into(out.reborrow())?;
            match field.name(out.reborrow())?.cmp(name) {
                Ordering::Less => low = middle + 1,
                Ordering::Greater => high = middle,
                Ordering::Equal => return Ok(Some((slot, field))),
            }
        }
        Ok(None)
    }

    pub fn named_budgeted<'slot>(
        self,
        name: &str,
        budget: &ExecutionBudget,
        mut out: Out<'slot, '_, BudgetError>,
    ) -> OResult<'slot, Option<(usize, FieldView<'a>)>> {
        let Some(slots) = self.sorted else {
            for (slot, field) in self.iter().enumerate() {
                budget.consume(1, out.reborrow())?;
                let field_name = field
                    .info()
                    .map(|info| info.name())
                    .map_err(|_| BudgetError::invariant(&VIEW_INDEX_INVARIANT));
                let field_name = field_name.store_err_into(out.reborrow())?;
                if budget
                    .compare_text(field_name, name, out.reborrow())?
                    .is_eq()
                {
                    return Ok(Some((slot, field)));
                }
            }
            return Ok(None);
        };
        let mut low = 0;
        let mut high = slots.len();
        while low < high {
            budget.consume(1, out.reborrow())?;
            let middle = low + (high - low) / 2;
            let slot = *slots
                .get(middle)
                .ok_or(BudgetError::invariant(&VIEW_INDEX_INVARIANT))
                .store_err_into(out.reborrow())?;
            let field = self
                .get(slot)
                .ok_or(BudgetError::invariant(&VIEW_INDEX_INVARIANT))
                .store_err_into(out.reborrow())?;
            let field_name = field
                .info()
                .map(|info| info.name())
                .map_err(|_| BudgetError::invariant(&VIEW_INDEX_INVARIANT));
            let field_name = field_name.store_err_into(out.reborrow())?;
            match budget.compare_text(field_name, name, out.reborrow())? {
                Ordering::Less => low = middle + 1,
                Ordering::Greater => high = middle,
                Ordering::Equal => return Ok(Some((slot, field))),
            }
        }
        Ok(None)
    }

    pub fn len(self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(self) -> bool {
        self.ids.is_empty()
    }

    pub fn get(self, index: usize) -> Option<FieldView<'a>> {
        self.ids.get(index).map(|id| FieldView {
            schema: self.schema,
            id: *id,
        })
    }

    pub fn iter(
        self,
    ) -> impl DoubleEndedIterator<Item = FieldView<'a>> + ExactSizeIterator + Clone {
        self.ids.iter().map(move |id| FieldView {
            schema: self.schema,
            id: *id,
        })
    }
}

impl<'a> IntoIterator for FieldList<'a> {
    type Item = FieldView<'a>;
    type IntoIter = ViewIter<'a, FieldId, FieldView<'a>>;

    fn into_iter(self) -> Self::IntoIter {
        ViewIter {
            schema: self.schema,
            ids: self.ids.iter(),
            make: |schema, id| FieldView { schema, id },
        }
    }
}

impl<'a> VariantList<'a> {
    pub fn len(self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(self) -> bool {
        self.ids.is_empty()
    }

    pub fn get(self, index: usize) -> Option<VariantView<'a>> {
        self.ids.get(index).map(|id| VariantView {
            schema: self.schema,
            id: *id,
        })
    }

    pub fn iter(
        self,
    ) -> impl DoubleEndedIterator<Item = VariantView<'a>> + ExactSizeIterator + Clone {
        self.ids.iter().map(move |id| VariantView {
            schema: self.schema,
            id: *id,
        })
    }
}

impl<'a> IntoIterator for VariantList<'a> {
    type Item = VariantView<'a>;
    type IntoIter = ViewIter<'a, VariantId, VariantView<'a>>;

    fn into_iter(self) -> Self::IntoIter {
        ViewIter {
            schema: self.schema,
            ids: self.ids.iter(),
            make: |schema, id| VariantView { schema, id },
        }
    }
}

impl<I: Copy, V> Iterator for ViewIter<'_, I, V> {
    type Item = V;

    fn next(&mut self) -> Option<V> {
        self.ids.next().map(|id| (self.make)(self.schema, *id))
    }
}
