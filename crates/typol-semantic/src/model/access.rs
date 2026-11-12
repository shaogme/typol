use crate::model::CompiledSchema;
use core::{
    error::Error as CoreError,
    fmt,
    sync::atomic::{AtomicUsize, Ordering as AtomicOrdering},
};
use opeo::{OResult, Out};
use typol_core::{BudgetError, DiagnosticContext, SourceSpan, diagnostic_context};

static SCHEMA_ACCESS_FAILURE: DiagnosticContext = diagnostic_context!(
    "typol-semantic",
    "schema-access",
    "table-index-out-of-bounds",
);
static NEXT_SCHEMA_IDENTITY: AtomicUsize = AtomicUsize::new(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchemaIdentity(usize);

impl SchemaIdentity {
    pub(crate) fn next<'slot, E>(out: Out<'slot, '_, E>) -> OResult<'slot, Self>
    where
        E: From<BudgetError>,
    {
        let mut identity = NEXT_SCHEMA_IDENTITY.load(AtomicOrdering::Relaxed);
        loop {
            let Some(next_identity) = identity.checked_add(1) else {
                return Err(out.fail(E::from(BudgetError::CapacityExceeded)));
            };
            match NEXT_SCHEMA_IDENTITY.compare_exchange_weak(
                identity,
                next_identity,
                AtomicOrdering::Relaxed,
                AtomicOrdering::Relaxed,
            ) {
                Ok(_) => return Ok(Self(identity)),
                Err(current) => identity = current,
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct TypeId(usize);

impl TypeId {
    pub(crate) const fn from_index(index: usize) -> Self {
        Self(index)
    }

    #[cfg(feature = "compiler")]
    pub const fn new(index: usize) -> Self {
        Self(index)
    }

    pub const fn index(self) -> usize {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FieldId(usize);

impl FieldId {
    #[cfg(feature = "compiler")]
    pub const fn new(index: usize) -> Self {
        Self(index)
    }

    pub const fn index(self) -> usize {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VariantId(usize);

impl VariantId {
    #[cfg(feature = "compiler")]
    pub const fn new(index: usize) -> Self {
        Self(index)
    }

    pub const fn index(self) -> usize {
        self.0
    }
}

#[derive(Clone, Copy)]
pub struct EnvironmentAuthorization<'a> {
    schema: &'a CompiledSchema,
    slot: usize,
    name: &'a str,
    span: SourceSpan,
}

impl<'a> EnvironmentAuthorization<'a> {
    pub(crate) fn new(
        schema: &'a CompiledSchema,
        slot: usize,
        name: &'a str,
        span: SourceSpan,
    ) -> Self {
        Self {
            schema,
            slot,
            name,
            span,
        }
    }

    /// A normalized-name slot within this Schema; grants with the same name share a slot, which cannot be reused across Schemas.
    pub fn slot(self) -> usize {
        self.slot
    }

    /// The number of distinct grant names in this Schema.
    pub fn slot_count(self) -> usize {
        self.schema.indexes().environment_count()
    }

    pub fn name(&self) -> &str {
        self.name
    }

    pub fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchemaAccessError {
    table: &'static str,
    index: usize,
}

impl SchemaAccessError {
    pub(crate) fn new(table: &'static str, index: usize) -> Self {
        Self { table, index }
    }

    pub const fn table(self) -> &'static str {
        self.table
    }

    pub const fn index(self) -> usize {
        self.index
    }

    pub const fn context(self) -> DiagnosticContext {
        SCHEMA_ACCESS_FAILURE.with_object_id(self.index as u64)
    }
}

impl fmt::Display for SchemaAccessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid {} reference at index {}",
            self.table, self.index
        )
    }
}

impl CoreError for SchemaAccessError {}

#[cfg(test)]
mod tests {
    use super::SchemaAccessError;

    #[test]
    fn schema_access_context_retains_table_and_index() {
        let error = SchemaAccessError::new("field", 17);
        let context = error.context();

        assert_eq!(error.table(), "field");
        assert_eq!(error.index(), 17);
        assert_eq!(context.subsystem(), "typol-semantic");
        assert_eq!(context.stage(), "schema-access");
        assert_eq!(context.failure_point(), "table-index-out-of-bounds");
        assert_eq!(context.object_id(), Some(17));
    }
}
