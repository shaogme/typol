use typol_core::{
    BudgetError, Diagnostic, DiagnosticCode, DiagnosticContext, ValueError, ValueErrorKind,
    diagnostic_context,
};
use typol_semantic::SchemaAccessError;

#[cfg(all(test, feature = "std"))]
use opeo::{OResult, Out};
#[cfg(all(test, feature = "std"))]
use typol_core::ExecutionBudget;

pub static BINDING_MISMATCH_CONTEXT: DiagnosticContext =
    diagnostic_context!("typol", "binding", "schema-type-mismatch");
static RECURSIVE_TYPE_CONTEXT: DiagnosticContext =
    diagnostic_context!("typol", "binding", "recursive-rust-type");
static CONVERSION_DEPTH_CONTEXT: DiagnosticContext =
    diagnostic_context!("typol", "binding", "conversion-depth-limit");
static ARRAY_LENGTH_CONTEXT: DiagnosticContext =
    diagnostic_context!("typol", "binding", "array-length-unrepresentable");
static CONVERSION_CONTEXT: DiagnosticContext =
    diagnostic_context!("typol", "conversion", "value-conversion-rejected");
static ENCODING_CONTEXT: DiagnosticContext =
    diagnostic_context!("typol", "encoding", "value-not-representable");
static SECRET_OUTPUT_REJECTED_CONTEXT: DiagnosticContext =
    diagnostic_context!("typol", "encoding", "secret-output-rejected");

static SCHEMA_TYPE_CONTEXT: DiagnosticContext = diagnostic_context!(
    "typol-semantic",
    "schema-access",
    "type-index-out-of-bounds"
);
static SCHEMA_TYPE_OWNER_CONTEXT: DiagnosticContext = diagnostic_context!(
    "typol-semantic",
    "schema-access",
    "type-owner-index-out-of-bounds",
);
static SCHEMA_CHILD_TYPE_CONTEXT: DiagnosticContext = diagnostic_context!(
    "typol-semantic",
    "schema-access",
    "child-type-index-out-of-bounds",
);
static SCHEMA_FIELD_CONTEXT: DiagnosticContext = diagnostic_context!(
    "typol-semantic",
    "schema-access",
    "field-index-out-of-bounds"
);
static SCHEMA_FIELD_INDEX_CONTEXT: DiagnosticContext = diagnostic_context!(
    "typol-semantic",
    "schema-access",
    "field-index-table-out-of-bounds",
);
static SCHEMA_FIELD_SORT_SLOT_CONTEXT: DiagnosticContext = diagnostic_context!(
    "typol-semantic",
    "schema-access",
    "field-sort-slot-out-of-bounds",
);
static SCHEMA_FIELD_LIST_SLOT_CONTEXT: DiagnosticContext = diagnostic_context!(
    "typol-semantic",
    "schema-access",
    "field-list-slot-out-of-bounds",
);
static SCHEMA_VARIANT_CONTEXT: DiagnosticContext = diagnostic_context!(
    "typol-semantic",
    "schema-access",
    "variant-index-out-of-bounds",
);
static SCHEMA_SHAPE_CONTEXT: DiagnosticContext = diagnostic_context!(
    "typol-semantic",
    "schema-access",
    "shape-index-out-of-bounds"
);
static SCHEMA_PROGRAM_CONTEXT: DiagnosticContext = diagnostic_context!(
    "typol-semantic",
    "schema-access",
    "program-index-out-of-bounds"
);
static SCHEMA_CONSTRAINT_CONTEXT: DiagnosticContext = diagnostic_context!(
    "typol-semantic",
    "schema-access",
    "constraint-index-out-of-bounds",
);
static SCHEMA_REFINEMENT_CONTEXT: DiagnosticContext = diagnostic_context!(
    "typol-semantic",
    "schema-access",
    "refinement-index-out-of-bounds",
);
static SCHEMA_REGEX_CONTEXT: DiagnosticContext = diagnostic_context!(
    "typol-semantic",
    "schema-access",
    "regex-index-out-of-bounds"
);
static SCHEMA_EXPRESSION_CONTEXT: DiagnosticContext = diagnostic_context!(
    "typol-semantic",
    "schema-access",
    "expression-index-out-of-bounds",
);
static SCHEMA_OTHER_CONTEXT: DiagnosticContext = diagnostic_context!(
    "typol-semantic",
    "schema-access",
    "schema-table-index-out-of-bounds",
);
static VALUE_MAP_DUPLICATE_CONTEXT: DiagnosticContext =
    diagnostic_context!("typol-core", "value-construction", "duplicate-map-key");
static VALUE_FIELD_DUPLICATE_CONTEXT: DiagnosticContext =
    diagnostic_context!("typol-core", "value-construction", "duplicate-field-name",);
static VALUE_INTERNAL_CONTEXT: DiagnosticContext =
    diagnostic_context!("typol-core", "value-construction", "value-invariant-failed");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CodecStage {
    Binding,
    Parsing,
    Conversion,
    Validation,
    Encoding,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Failure {
    Resource,
    ConversionDepth {
        requested_depth: usize,
        maximum_depth: usize,
    },
    Allocation,
    Capacity,
    Internal(DiagnosticContext),
    #[cfg(all(test, feature = "std"))]
    BindingMismatch,
    RecursiveType,
    ArrayLength,
    Conversion,
    ConversionAt(DiagnosticContext),
    Encoding,
    SecretOutput,
}

impl Failure {
    pub(crate) const fn internal(context: DiagnosticContext) -> Self {
        Self::Internal(context)
    }

    pub(crate) fn schema_access(error: SchemaAccessError) -> Self {
        let context = match error.table() {
            "type" => SCHEMA_TYPE_CONTEXT,
            "type owner" => SCHEMA_TYPE_OWNER_CONTEXT,
            "child type" => SCHEMA_CHILD_TYPE_CONTEXT,
            "field" => SCHEMA_FIELD_CONTEXT,
            "field index" => SCHEMA_FIELD_INDEX_CONTEXT,
            "field sort slot" => SCHEMA_FIELD_SORT_SLOT_CONTEXT,
            "field list slot" => SCHEMA_FIELD_LIST_SLOT_CONTEXT,
            "variant" => SCHEMA_VARIANT_CONTEXT,
            "shape" => SCHEMA_SHAPE_CONTEXT,
            "program" => SCHEMA_PROGRAM_CONTEXT,
            "constraint" => SCHEMA_CONSTRAINT_CONTEXT,
            "refinement" => SCHEMA_REFINEMENT_CONTEXT,
            "regex" => SCHEMA_REGEX_CONTEXT,
            "expression node" => SCHEMA_EXPRESSION_CONTEXT,
            _ => SCHEMA_OTHER_CONTEXT,
        };
        Self::internal(context.with_object_id(error.index() as u64))
    }

    pub(crate) fn value_error(error: ValueError) -> Self {
        match error.kind() {
            ValueErrorKind::AllocationFailed => Self::Allocation,
            ValueErrorKind::ResourceLimitExceeded => Self::Resource,
            ValueErrorKind::CapacityExceeded => Self::Capacity,
            ValueErrorKind::InternalInvariantFailure => {
                Self::internal(error.context().unwrap_or(VALUE_INTERNAL_CONTEXT))
            }
            ValueErrorKind::EnvironmentReferenceRequired => {
                Self::ConversionAt(error.context().unwrap_or(VALUE_INTERNAL_CONTEXT))
            }
            ValueErrorKind::DuplicateMapKey => Self::ConversionAt(
                error
                    .duplicate_indices()
                    .map_or(VALUE_MAP_DUPLICATE_CONTEXT, |(_, second)| {
                        VALUE_MAP_DUPLICATE_CONTEXT.with_object_id(second as u64)
                    }),
            ),
            ValueErrorKind::DuplicateFieldName => Self::ConversionAt(
                error
                    .duplicate_indices()
                    .map_or(VALUE_FIELD_DUPLICATE_CONTEXT, |(_, second)| {
                        VALUE_FIELD_DUPLICATE_CONTEXT.with_object_id(second as u64)
                    }),
            ),
        }
    }

    pub(crate) fn code(self) -> DiagnosticCode {
        match self {
            Self::Resource => DiagnosticCode::RESOURCE_LIMIT_EXCEEDED,
            Self::ConversionDepth { .. } => DiagnosticCode::RESOURCE_LIMIT_EXCEEDED,
            Self::Allocation => DiagnosticCode::ALLOCATION_FAILED,
            Self::Capacity => DiagnosticCode::CAPACITY_EXCEEDED,
            Self::Internal(_) => DiagnosticCode::INTERNAL_ERROR,
            #[cfg(all(test, feature = "std"))]
            Self::BindingMismatch => DiagnosticCode::TYPE_BINDING_ERROR,
            Self::RecursiveType | Self::ArrayLength => DiagnosticCode::TYPE_BINDING_ERROR,
            Self::Conversion | Self::ConversionAt(_) => DiagnosticCode::RUST_CONVERSION_ERROR,
            Self::Encoding | Self::SecretOutput => DiagnosticCode::CONFIG_ENCODING_ERROR,
        }
    }

    pub(crate) const fn context(self) -> Option<DiagnosticContext> {
        match self {
            Self::Internal(context) => Some(context),
            #[cfg(all(test, feature = "std"))]
            Self::BindingMismatch => Some(BINDING_MISMATCH_CONTEXT),
            Self::RecursiveType => Some(RECURSIVE_TYPE_CONTEXT),
            Self::ConversionDepth {
                requested_depth,
                maximum_depth,
            } => Some(CONVERSION_DEPTH_CONTEXT.with_object_range(requested_depth, maximum_depth)),
            Self::ArrayLength => Some(ARRAY_LENGTH_CONTEXT),
            Self::Conversion => Some(CONVERSION_CONTEXT),
            Self::ConversionAt(context) => Some(context),
            Self::Encoding => Some(ENCODING_CONTEXT),
            Self::SecretOutput => Some(SECRET_OUTPUT_REJECTED_CONTEXT),
            Self::Resource | Self::Allocation | Self::Capacity => None,
        }
    }

    #[cfg(all(test, feature = "std"))]
    fn message(self) -> &'static str {
        match self {
            Self::BindingMismatch => "Rust type description is incompatible with the schema",
            Self::RecursiveType => "recursive Rust types are unsupported",
            Self::ConversionDepth { .. } => "type depth exceeds the synchronous conversion limit",
            Self::ArrayLength => "fixed array length cannot be mapped to the schema",
            Self::Conversion | Self::ConversionAt(_) => "Rust value conversion failed",
            Self::Encoding => "value cannot be expressed in the configuration grammar",
            Self::SecretOutput => "generated output includes a secret value",
            _ => "",
        }
    }

    #[cfg(all(test, feature = "std"))]
    pub(super) fn terminal(self) -> bool {
        matches!(
            self,
            Self::Resource
                | Self::ConversionDepth { .. }
                | Self::Allocation
                | Self::Capacity
                | Self::Internal(_)
        )
    }

    pub(super) fn emergency_diagnostic(self) -> Diagnostic {
        let mut diagnostic = Diagnostic::emergency(self.code());
        diagnostic.set_context(self.context());
        diagnostic
    }

    #[cfg(all(test, feature = "std"))]
    pub(super) fn diagnostic<'slot>(
        self,
        budget: &ExecutionBudget,
        max_text_bytes: usize,
        mut out: Out<'slot, '_, BudgetError>,
    ) -> OResult<'slot, Diagnostic> {
        let mut diagnostic = if self.terminal() {
            Diagnostic::emergency(self.code())
        } else {
            budget.construct_text(self.message().len(), max_text_bytes, out.reborrow())?;
            budget.consume(self.message().len(), out.reborrow())?;
            Diagnostic::try_new(self.code(), self.message(), Some(budget), out.reborrow())?
        };
        diagnostic.set_context(self.context());
        Ok(diagnostic)
    }
}

impl From<BudgetError> for Failure {
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

impl From<ValueError> for Failure {
    fn from(error: ValueError) -> Self {
        Self::value_error(error)
    }
}
