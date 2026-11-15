use typol_core::{
    BudgetError, CapacityExceeded, Diagnostic, DiagnosticCode, DiagnosticContext, InternalError,
    ResourceLimit, SourceSpan, diagnostic_context,
};
use typol_i18n::{Translator, t};
use typol_semantic::SchemaAccessError;
use typol_syntax::AstAccessError;
use u18n::RenderError;

static AST_NODE_ACCESS: DiagnosticContext =
    diagnostic_context!("typol-schema", "ast-access", "invalid-node-id");
static AST_LIST_ACCESS: DiagnosticContext =
    diagnostic_context!("typol-schema", "ast-access", "invalid-list-range");
static AST_TEXT_ACCESS: DiagnosticContext =
    diagnostic_context!("typol-schema", "ast-access", "invalid-text-range");
static AST_DECODED_TEXT_ACCESS: DiagnosticContext =
    diagnostic_context!("typol-schema", "ast-access", "invalid-decoded-text-id");
static SCHEMA_ACCESS_TYPE: DiagnosticContext =
    diagnostic_context!("typol-schema", "schema-access", "type-table-index");
static SCHEMA_ACCESS_FIELD: DiagnosticContext =
    diagnostic_context!("typol-schema", "schema-access", "field-table-index");
static SCHEMA_ACCESS_VARIANT: DiagnosticContext =
    diagnostic_context!("typol-schema", "schema-access", "variant-table-index");
static SCHEMA_ACCESS_PROGRAM: DiagnosticContext =
    diagnostic_context!("typol-schema", "schema-access", "program-table-index");
static SCHEMA_ACCESS_CONSTRAINT: DiagnosticContext =
    diagnostic_context!("typol-schema", "schema-access", "constraint-table-index");
static SCHEMA_ACCESS_REFINEMENT: DiagnosticContext =
    diagnostic_context!("typol-schema", "schema-access", "refinement-table-index");
static SCHEMA_ACCESS_SHAPE: DiagnosticContext =
    diagnostic_context!("typol-schema", "schema-access", "shape-table-index");
static SCHEMA_ACCESS_EXPRESSION: DiagnosticContext =
    diagnostic_context!("typol-schema", "schema-access", "expression-table-index");
static SCHEMA_ACCESS_OTHER: DiagnosticContext =
    diagnostic_context!("typol-schema", "schema-access", "table-index");

pub(crate) struct BudgetDiagnostics;

impl BudgetDiagnostics {
    pub(crate) fn make(error: BudgetError, span: SourceSpan, translator: Translator) -> Diagnostic {
        Self::make_optional(error, Some(span), translator)
    }

    pub(crate) fn make_optional(
        error: BudgetError,
        span: Option<SourceSpan>,
        translator: Translator,
    ) -> Diagnostic {
        let message = match error {
            BudgetError::Allocation => t! {
                name: "diagnostic.emergency.allocation_failed",
                locale: translator,
            },
            BudgetError::CapacityExceeded(capacity) => capacity.emergency_message(translator),
            BudgetError::Internal(error) => error.emergency_message(translator),
            BudgetError::ResourceLimit(limit) => limit.emergency_message(translator),
        };
        let mut diagnostic =
            EmergencyDiagnostics::translated(error.diagnostic_code(), message, span);
        diagnostic.set_context(error.context());
        diagnostic
    }
}

pub(crate) struct EmergencyDiagnostics;

impl EmergencyDiagnostics {
    pub(crate) fn ast_access_context(error: AstAccessError) -> DiagnosticContext {
        match error {
            AstAccessError::InvalidNodeId { id } => AST_NODE_ACCESS
                .with_object_id(id as u64)
                .with_object_index(id),
            AstAccessError::InvalidListRange { start, end } => AST_LIST_ACCESS
                .with_object_id(start as u64)
                .with_object_range(start, end),
            AstAccessError::InvalidTextRange { start, end } => AST_TEXT_ACCESS
                .with_object_id(start as u64)
                .with_object_range(start, end),
            AstAccessError::InvalidDecodedTextId { id } => AST_DECODED_TEXT_ACCESS
                .with_object_id(id as u64)
                .with_object_index(id),
        }
    }

    pub(crate) fn schema_access_context(error: SchemaAccessError) -> DiagnosticContext {
        let context = match error.table() {
            "type" | "type owner" | "child type" => SCHEMA_ACCESS_TYPE,
            "field" | "field index" | "field sort slot" | "field list slot" => SCHEMA_ACCESS_FIELD,
            "variant" => SCHEMA_ACCESS_VARIANT,
            "program" => SCHEMA_ACCESS_PROGRAM,
            "constraint" => SCHEMA_ACCESS_CONSTRAINT,
            "refinement" => SCHEMA_ACCESS_REFINEMENT,
            "shape" => SCHEMA_ACCESS_SHAPE,
            "expression node" => SCHEMA_ACCESS_EXPRESSION,
            _ => SCHEMA_ACCESS_OTHER,
        };
        context.with_object_id(error.index() as u64)
    }

    pub(crate) fn translated(
        code: DiagnosticCode,
        message: Result<&'static str, RenderError>,
        span: Option<SourceSpan>,
    ) -> Diagnostic {
        let mut diagnostic = match message {
            Ok(message) => Diagnostic::emergency(code, message),
            Err(error) => Diagnostic::translation_failure(code, error),
        };
        diagnostic.set_primary(span);
        diagnostic
    }

    pub(crate) fn resource(
        limit: ResourceLimit,
        span: SourceSpan,
        translator: Translator,
    ) -> Diagnostic {
        Self::translated(
            limit.diagnostic_code(),
            limit.emergency_message(translator),
            Some(span),
        )
    }

    pub(crate) fn allocation(span: SourceSpan, translator: Translator) -> Diagnostic {
        Self::translated(
            DiagnosticCode::ALLOCATION_FAILED,
            t! {
                name: "diagnostic.emergency.allocation_failed",
                locale: translator,
            },
            Some(span),
        )
    }

    pub(crate) fn internal(
        span: SourceSpan,
        context: DiagnosticContext,
        translator: Translator,
    ) -> Diagnostic {
        Self::internal_error(
            span,
            InternalError::SchemaCompilationInvariantFailure(context),
            translator,
        )
    }

    pub(crate) fn internal_error(
        span: SourceSpan,
        error: InternalError,
        translator: Translator,
    ) -> Diagnostic {
        Self::internal_at(Some(span), error, translator)
    }

    pub(crate) fn internal_error_without_span(
        error: InternalError,
        translator: Translator,
    ) -> Diagnostic {
        Self::internal_at(None, error, translator)
    }

    fn internal_at(
        span: Option<SourceSpan>,
        error: InternalError,
        translator: Translator,
    ) -> Diagnostic {
        let mut diagnostic = Self::translated(
            error.diagnostic_code(),
            error.emergency_message(translator),
            span,
        );
        diagnostic.set_context(Some(error.context()));
        diagnostic
    }

    pub(crate) fn capacity(span: SourceSpan, translator: Translator) -> Diagnostic {
        Self::capacity_error(span, CapacityExceeded::SchemaStorage, translator)
    }

    pub(crate) fn capacity_error(
        span: SourceSpan,
        capacity: CapacityExceeded,
        translator: Translator,
    ) -> Diagnostic {
        Self::translated(
            capacity.diagnostic_code(),
            capacity.emergency_message(translator),
            Some(span),
        )
    }

    pub(crate) fn limit_reached(span: Option<SourceSpan>, translator: Translator) -> Diagnostic {
        Self::translated(
            DiagnosticCode::DIAGNOSTIC_LIMIT_REACHED,
            t! {
                name: "diagnostic.emergency.diagnostic_limit_reached",
                locale: translator,
            },
            span,
        )
    }
}
