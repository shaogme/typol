use crate::diagnostics::{BudgetDiagnostic, DiagnosticFactory};
use opeo::{ErrSlot, OResult, Out, ResultOutExt};
use typol_core::{
    BudgetError, Diagnostic, DiagnosticCode, DiagnosticContext, DiagnosticMessage, ExecutionBudget,
    InstancePath, LocatedSpan, PathSegment, ValueSource, diagnostic_context,
};
use typol_i18n::diagnostic_key;
use typol_semantic::{
    CompiledSchema, SchemaAccessError, TypeKind, TypeView, VariantPayloadInfo, VariantView,
};

static SECRET_SCHEMA_ACCESS: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "secret-boundary",
    "schema-view-access-failed",
);
static SECRET_FIELD_PAYLOAD_SHAPE: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "secret-boundary",
    "variant-payload-is-not-structure",
);
static SECRET_FIELD_TYPE_SHAPE: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "secret-boundary",
    "field-segment-on-non-structure",
);
static SECRET_INDEX_TYPE_SHAPE: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "secret-boundary",
    "index-segment-on-non-array",
);
static SECRET_MAP_TYPE_SHAPE: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "secret-boundary",
    "map-key-segment-on-non-map",
);
static SECRET_UNIT_PAYLOAD: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "secret-boundary",
    "unit-variant-has-payload-path",
);
static SECRET_UNRESOLVED_DYNAMIC_PATH: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "secret-redaction",
    "unresolved-dynamic-map-key",
);
static SECRET_PATH_PREFIX: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "secret-boundary",
    "path-prefix-out-of-bounds",
);
static SECRET_REDACTION_ALLOCATION: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "secret-redaction",
    "safe-diagnostic-allocation-failed",
);

enum BoundaryError {
    Budget(BudgetError),
    Internal(DiagnosticContext),
    UnresolvedPath,
}

impl From<BudgetError> for BoundaryError {
    fn from(error: BudgetError) -> Self {
        Self::Budget(error)
    }
}

impl From<SchemaAccessError> for BoundaryError {
    fn from(error: SchemaAccessError) -> Self {
        Self::Internal(SECRET_SCHEMA_ACCESS.with_object_id(error.index() as u64))
    }
}

impl BoundaryError {
    fn internal(context: DiagnosticContext, index: usize) -> Self {
        Self::Internal(context.with_object_id(index as u64))
    }

    fn diagnostic(
        self,
        primary: Option<LocatedSpan>,
        value_source: Option<ValueSource>,
    ) -> Diagnostic {
        match self {
            Self::Budget(error) => {
                located_error(BudgetDiagnostic::from_error(error), primary, value_source)
            }
            Self::Internal(context) => {
                DiagnosticFactory::internal(context, primary, None, value_source)
            }
            Self::UnresolvedPath => DiagnosticFactory::internal(
                SECRET_UNRESOLVED_DYNAMIC_PATH,
                primary,
                None,
                value_source,
            ),
        }
    }
}

fn schema_call<'slot, 'borrow, T>(
    errors: &mut ErrSlot<SchemaAccessError>,
    operation: impl for<'call> FnOnce(Out<'call, 'call, SchemaAccessError>) -> OResult<'call, T>,
    out: Out<'slot, 'borrow, BoundaryError>,
) -> OResult<'slot, T> {
    match errors.try_call(operation) {
        Ok(value) => Ok(value),
        Err(caught) => Err(out.fail(BoundaryError::from(caught.take()))),
    }
}

fn budget_call<'slot, 'borrow, T>(
    errors: &mut ErrSlot<BudgetError>,
    operation: impl for<'call> FnOnce(Out<'call, 'call, BudgetError>) -> OResult<'call, T>,
    out: Out<'slot, 'borrow, BoundaryError>,
) -> OResult<'slot, T> {
    match errors.try_call(operation) {
        Ok(value) => Ok(value),
        Err(caught) => Err(out.fail(BoundaryError::from(caught.take()))),
    }
}

fn schema_diagnostic_call<'slot, 'borrow, T>(
    errors: &mut ErrSlot<SchemaAccessError>,
    operation: impl for<'call> FnOnce(Out<'call, 'call, SchemaAccessError>) -> OResult<'call, T>,
    out: Out<'slot, 'borrow, Diagnostic>,
) -> OResult<'slot, T> {
    match errors.try_call(operation) {
        Ok(value) => Ok(value),
        Err(caught) => Err(out.fail(DiagnosticFactory::schema_access(caught.take()))),
    }
}

fn budget_diagnostic_call<'slot, 'borrow, T>(
    errors: &mut ErrSlot<BudgetError>,
    operation: impl for<'call> FnOnce(Out<'call, 'call, BudgetError>) -> OResult<'call, T>,
    out: Out<'slot, 'borrow, Diagnostic>,
) -> OResult<'slot, T> {
    match errors.try_call(operation) {
        Ok(value) => Ok(value),
        Err(caught) => Err(out.fail(BudgetDiagnostic::from_error(caught.take()))),
    }
}

/// Finds the outermost secret boundary through the compiled type view without reading values or retaining source text.
fn boundary<'slot>(
    schema: &CompiledSchema,
    path: &InstancePath,
    mut out: Out<'slot, '_, BoundaryError>,
) -> OResult<'slot, Option<usize>> {
    let mut ty = schema.entry();
    let mut structure_variant: Option<VariantView<'_>> = None;
    let mut schema_errors = ErrSlot::new();
    let mut budget_errors = ErrSlot::new();
    for index in 0..=path.len() {
        if let Some(budget) = path.budget() {
            budget_call(
                &mut budget_errors,
                |budget_out| budget.consume(1, budget_out),
                out.reborrow(),
            )?;
        }
        loop {
            if let Some(budget) = path.budget() {
                budget_call(
                    &mut budget_errors,
                    |budget_out| budget.consume(1, budget_out),
                    out.reborrow(),
                )?;
            }
            let info = ty;
            if (schema_call(
                &mut schema_errors,
                |access_out| info.is_secret(access_out),
                out.reborrow(),
            ))? {
                return Ok(Some(index));
            }
            if let TypeKind::Option { inner } = schema_call(
                &mut schema_errors,
                |access_out| info.kind(access_out),
                out.reborrow(),
            )? {
                ty = inner;
            } else {
                break;
            }
        }
        let Some(segment) = path.segments().get(index) else {
            return Ok(None);
        };
        let info = ty;
        match segment {
            PathSegment::Field(name) => {
                let id = if let Some(variant) = structure_variant.take() {
                    let VariantPayloadInfo::Structure { fields: ids } = schema_call(
                        &mut schema_errors,
                        |access_out| variant.payload(access_out),
                        out.reborrow(),
                    )?
                    else {
                        return Err(
                            out.fail(BoundaryError::internal(SECRET_FIELD_PAYLOAD_SHAPE, index))
                        );
                    };
                    let mut selected = None;
                    for field in ids.iter() {
                        let equal = if let Some(budget) = path.budget() {
                            budget_call(
                                &mut budget_errors,
                                |budget_out| budget.consume(1, budget_out),
                                out.reborrow(),
                            )?;
                            let field_name = schema_call(
                                &mut schema_errors,
                                |access_out| field.name(access_out),
                                out.reborrow(),
                            )?;
                            budget_call(
                                &mut budget_errors,
                                |budget_out| budget.compare_text(field_name, name, budget_out),
                                out.reborrow(),
                            )?
                            .is_eq()
                        } else {
                            schema_call(
                                &mut schema_errors,
                                |access_out| field.name(access_out),
                                out.reborrow(),
                            )? == name.as_str()
                        };
                        if equal {
                            selected = Some(field);
                            break;
                        }
                    }
                    selected
                        .ok_or(BoundaryError::UnresolvedPath)
                        .store_err_into(out.reborrow())?
                } else {
                    if !matches!(
                        schema_call(
                            &mut schema_errors,
                            |access_out| ty.kind(access_out),
                            out.reborrow(),
                        )?,
                        TypeKind::Struct { .. }
                    ) {
                        return Err(
                            out.fail(BoundaryError::internal(SECRET_FIELD_TYPE_SHAPE, index))
                        );
                    }
                    if let Some(budget) = path.budget() {
                        let fields = schema_call(
                            &mut schema_errors,
                            |access_out| ty.fields(access_out),
                            out.reborrow(),
                        )?;
                        let named = budget_call(
                            &mut budget_errors,
                            |budget_out| fields.named_budgeted(name, budget, budget_out),
                            out.reborrow(),
                        )?;
                        let Some((_, field)) = named else {
                            return Err(out.fail(BoundaryError::UnresolvedPath));
                        };
                        field
                    } else {
                        let field = schema_call(
                            &mut schema_errors,
                            |access_out| ty.field_named(name, access_out),
                            out.reborrow(),
                        )?;
                        let Some(field) = field else {
                            return Err(out.fail(BoundaryError::UnresolvedPath));
                        };
                        field
                    }
                };
                let field = id;
                if (schema_call(
                    &mut schema_errors,
                    |access_out| field.is_secret(access_out),
                    out.reborrow(),
                ))? {
                    return Ok(Some(index + 1));
                }
                ty = schema_call(
                    &mut schema_errors,
                    |access_out| field.ty(access_out),
                    out.reborrow(),
                )?;
            }
            PathSegment::Index(_) => {
                let TypeKind::Array { element, .. } = schema_call(
                    &mut schema_errors,
                    |access_out| info.kind(access_out),
                    out.reborrow(),
                )?
                else {
                    return Err(out.fail(BoundaryError::internal(SECRET_INDEX_TYPE_SHAPE, index)));
                };
                ty = element;
                if (boundary_secret_type(
                    ty,
                    path.budget(),
                    &mut schema_errors,
                    &mut budget_errors,
                    out.reborrow(),
                ))? {
                    return Ok(Some(index));
                }
            }
            PathSegment::MapKey(_) => {
                let TypeKind::Map { value } = schema_call(
                    &mut schema_errors,
                    |access_out| info.kind(access_out),
                    out.reborrow(),
                )?
                else {
                    return Err(out.fail(BoundaryError::internal(SECRET_MAP_TYPE_SHAPE, index)));
                };
                ty = value;
                if (boundary_secret_type(
                    ty,
                    path.budget(),
                    &mut schema_errors,
                    &mut budget_errors,
                    out.reborrow(),
                ))? {
                    return Ok(Some(index));
                }
            }
            PathSegment::EnumPayload(name) => {
                let variant_id = if let Some(budget) = path.budget() {
                    let variant = budget_call(
                        &mut budget_errors,
                        |budget_out| ty.variant_named_budgeted(name, budget, budget_out),
                        out.reborrow(),
                    )?;
                    let Some(variant) = variant else {
                        return Err(out.fail(BoundaryError::UnresolvedPath));
                    };
                    variant
                } else {
                    let variant = schema_call(
                        &mut schema_errors,
                        |access_out| ty.variant_named(name, access_out),
                        out.reborrow(),
                    )?;
                    let Some(variant) = variant else {
                        return Err(out.fail(BoundaryError::UnresolvedPath));
                    };
                    variant
                };
                let variant = variant_id;
                if (schema_call(
                    &mut schema_errors,
                    |access_out| variant.is_secret(access_out),
                    out.reborrow(),
                ))? {
                    return Ok(Some(index));
                }
                match (schema_call(
                    &mut schema_errors,
                    |access_out| variant.payload(access_out),
                    out.reborrow(),
                ))? {
                    VariantPayloadInfo::Single { ty: payload } => {
                        ty = payload;
                        if (boundary_secret_type(
                            ty,
                            path.budget(),
                            &mut schema_errors,
                            &mut budget_errors,
                            out.reborrow(),
                        ))? {
                            return Ok(Some(index));
                        }
                    }
                    VariantPayloadInfo::Structure { .. } => structure_variant = Some(variant_id),
                    VariantPayloadInfo::Unit => {
                        return Err(out.fail(BoundaryError::internal(SECRET_UNIT_PAYLOAD, index)));
                    }
                }
            }
        }
    }
    Ok(None)
}

fn boundary_secret_type<'slot>(
    mut ty: TypeView<'_>,
    budget: Option<&ExecutionBudget>,
    schema_errors: &mut ErrSlot<SchemaAccessError>,
    budget_errors: &mut ErrSlot<BudgetError>,
    mut out: Out<'slot, '_, BoundaryError>,
) -> OResult<'slot, bool> {
    loop {
        if let Some(budget) = budget {
            budget_call(
                budget_errors,
                |budget_out| budget.consume(1, budget_out),
                out.reborrow(),
            )?;
        }
        let info = ty;
        if (schema_call(
            schema_errors,
            |access_out| info.is_secret(access_out),
            out.reborrow(),
        ))? {
            return Ok(true);
        }
        match (schema_call(
            schema_errors,
            |access_out| info.kind(access_out),
            out.reborrow(),
        ))? {
            TypeKind::Option { inner } => ty = inner,
            _ => break,
        }
    }
    Ok(false)
}

pub(crate) struct SecretPolicy;

impl SecretPolicy {
    pub(crate) fn hides<'slot, 'borrow>(
        schema: &CompiledSchema,
        path: &InstancePath,
        primary: Option<LocatedSpan>,
        value_source: Option<ValueSource>,
        out: Out<'slot, 'borrow, Diagnostic>,
    ) -> OResult<'slot, bool> {
        let mut errors = ErrSlot::new();
        match errors.try_call(|boundary_out| boundary(schema, path, boundary_out)) {
            Ok(value) => Ok(value.is_some()),
            Err(caught) => match caught.take() {
                BoundaryError::UnresolvedPath => Ok(false),
                error => Err(out.fail(error.diagnostic(primary, value_source))),
            },
        }
    }

    pub(crate) fn copy_path<'slot, 'borrow>(
        schema: &CompiledSchema,
        path: &InstancePath,
        primary: Option<LocatedSpan>,
        value_source: Option<ValueSource>,
        out: Out<'slot, 'borrow, Diagnostic>,
    ) -> OResult<'slot, InstancePath> {
        let mut boundary_errors = ErrSlot::new();
        let mut budget_errors = ErrSlot::new();
        let length =
            match boundary_errors.try_call(|boundary_out| boundary(schema, path, boundary_out)) {
                Ok(length) => length.unwrap_or(path.len()),
                Err(caught) => match caught.take() {
                    BoundaryError::UnresolvedPath => {
                        for segment in path.segments() {
                            if let Some(budget) = path.budget() {
                                if let Err(caught) = budget_errors
                                    .try_call(|budget_out| budget.consume(1, budget_out))
                                {
                                    return Err(out.fail(located_error(
                                        BudgetDiagnostic::from_error(caught.take()),
                                        primary,
                                        value_source,
                                    )));
                                }
                            }
                            if matches!(segment, PathSegment::MapKey(_)) {
                                return Err(out.fail(DiagnosticFactory::internal(
                                    SECRET_UNRESOLVED_DYNAMIC_PATH,
                                    primary,
                                    None,
                                    value_source,
                                )));
                            }
                        }
                        path.len()
                    }
                    error => {
                        return Err(out.fail(error.diagnostic(primary, value_source)));
                    }
                },
            };
        match budget_errors.try_call(|budget_out| Self::copy_path_prefix(path, length, budget_out))
        {
            Ok(path) => Ok(path),
            Err(caught) => Err(out.fail(located_error(
                BudgetDiagnostic::from_error(caught.take()),
                primary,
                value_source,
            ))),
        }
    }

    fn copy_path_prefix<'slot>(
        path: &InstancePath,
        length: usize,
        mut out: Out<'slot, '_, BudgetError>,
    ) -> OResult<'slot, InstancePath> {
        let segments = path
            .segments()
            .get(..length)
            .ok_or_else(|| BudgetError::invariant(&SECRET_PATH_PREFIX))
            .store_err_into(out.reborrow())?;
        let mut result = match path.budget() {
            Some(budget) => InstancePath::new(Some(budget), out.reborrow())?,
            None => InstancePath::new(None, out.reborrow())?,
        };
        for segment in segments {
            match segment {
                PathSegment::Field(name) => result.push_field(name, out.reborrow())?,
                PathSegment::Index(index) => result.push_index(*index, out.reborrow())?,
                PathSegment::MapKey(key) => result.push_map_key(key, out.reborrow())?,
                PathSegment::EnumPayload(variant) => {
                    result.push_payload(variant, out.reborrow())?
                }
            }
        }
        Ok(result)
    }

    pub(crate) fn secret_type<'slot>(
        mut ty: TypeView<'_>,
        budget: Option<&ExecutionBudget>,
        schema_errors: &mut ErrSlot<SchemaAccessError>,
        budget_errors: &mut ErrSlot<BudgetError>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, bool> {
        loop {
            if let Some(budget) = budget {
                budget_diagnostic_call(
                    budget_errors,
                    |budget_out| budget.consume(1, budget_out),
                    out.reborrow(),
                )?;
            }
            let info = ty;
            let is_secret = schema_diagnostic_call(
                schema_errors,
                |schema_out| info.is_secret(schema_out),
                out.reborrow(),
            )?;
            if is_secret {
                return Ok(true);
            }
            let kind = schema_diagnostic_call(
                schema_errors,
                |schema_out| info.kind(schema_out),
                out.reborrow(),
            )?;
            match kind {
                TypeKind::Option { inner } => ty = inner,
                _ => break,
            }
        }
        Ok(false)
    }

    /// Rebuilds safe diagnostics before returning; any allocation failure discards diagnostics containing the original path.
    pub(crate) fn redact<'slot, 'borrow>(
        schema: &CompiledSchema,
        diagnostic: &mut Diagnostic,
        out: Out<'slot, 'borrow, Diagnostic>,
    ) -> OResult<'slot, ()> {
        Self::redact_with_policy(schema, diagnostic, false, out)
    }

    pub(crate) fn redact_conversion<'slot, 'borrow>(
        schema: &CompiledSchema,
        diagnostic: &mut Diagnostic,
        out: Out<'slot, 'borrow, Diagnostic>,
    ) -> OResult<'slot, ()> {
        Self::redact_with_policy(schema, diagnostic, true, out)
    }

    fn redact_with_policy<'slot, 'borrow>(
        schema: &CompiledSchema,
        diagnostic: &mut Diagnostic,
        fail_closed_on_unresolved_path: bool,
        mut out: Out<'slot, 'borrow, Diagnostic>,
    ) -> OResult<'slot, ()> {
        if matches!(
            diagnostic.code(),
            DiagnosticCode::ALLOCATION_FAILED | DiagnosticCode::DIAGNOSTIC_LIMIT_REACHED
        ) {
            DiagnosticFactory::clear_path(diagnostic);
            return Ok(());
        }
        let Some(path) = diagnostic.path() else {
            return Ok(());
        };
        let mut boundary_errors = ErrSlot::new();
        let mut budget_errors = ErrSlot::new();
        let length = match boundary_errors
            .try_call(|boundary_out| boundary(schema, path, boundary_out))
        {
            Ok(Some(length)) => length,
            Ok(None) => return Ok(()),
            // Syntax errors may contain field or variant names that are not yet bound to a Schema; dynamic keys cannot use this fallback.
            Err(caught) => match caught.take() {
                BoundaryError::UnresolvedPath => {
                    let mut dynamic = false;
                    for segment in path.segments() {
                        if let Some(budget) = path.budget() {
                            if let Err(caught) =
                                budget_errors.try_call(|budget_out| budget.consume(1, budget_out))
                            {
                                return Err(out.fail(located_error(
                                    BudgetDiagnostic::from_error(caught.take()),
                                    diagnostic.located_primary(),
                                    diagnostic.value_source(),
                                )));
                            }
                        }
                        if matches!(segment, PathSegment::MapKey(_)) {
                            dynamic = true;
                            break;
                        }
                    }
                    if !dynamic {
                        if fail_closed_on_unresolved_path {
                            DiagnosticFactory::clear_path(diagnostic);
                        }
                        return Ok(());
                    }
                    return Err(out.fail(DiagnosticFactory::internal(
                        diagnostic
                            .context()
                            .unwrap_or(SECRET_UNRESOLVED_DYNAMIC_PATH),
                        diagnostic.located_primary(),
                        None,
                        diagnostic.value_source(),
                    )));
                }
                error => {
                    return Err(out.fail(
                        error.diagnostic(diagnostic.located_primary(), diagnostic.value_source()),
                    ));
                }
            },
        };
        let span = diagnostic.located_primary();
        let source = diagnostic.value_source();
        let context = diagnostic.context();
        let allocation = || {
            let mut diagnostic = DiagnosticFactory::allocation_diagnostic(span, None, source);
            diagnostic.set_context(Some(SECRET_REDACTION_ALLOCATION));
            diagnostic
        };
        let safe_path = match budget_errors
            .try_call(|budget_out| Self::copy_path_prefix(path, length, budget_out))
        {
            Ok(path) => path,
            Err(caught) => {
                let mut error = BudgetDiagnostic::from_error(caught.take());
                if error.code() == DiagnosticCode::ALLOCATION_FAILED {
                    error.set_context(Some(SECRET_REDACTION_ALLOCATION));
                    error.set_located_primary(span);
                }
                return Err(out.fail(error));
            }
        };
        let mut safe = DiagnosticFactory::make_diagnostic(
            diagnostic.code(),
            "value is hidden",
            span,
            Some(safe_path),
            source,
        );
        safe.set_context(context);
        safe.set_message_key(Some(diagnostic_key!("diagnostic.validation.value_hidden")));
        if safe.code() == DiagnosticCode::ALLOCATION_FAILED {
            return Err(out.fail(allocation()));
        }
        for label in diagnostic.labels() {
            if let Some(budget) = diagnostic.budget() {
                if let Err(caught) =
                    budget_errors.try_call(|budget_out| budget.consume(1, budget_out))
                {
                    return Err(out.fail(BudgetDiagnostic::from_error(caught.take())));
                }
            }
            DiagnosticFactory::add_label_message(
                &mut safe,
                label.located_span(),
                match label.key() {
                    Some(key) => DiagnosticMessage::localized(label.message(), key),
                    None => DiagnosticMessage::custom(label.message()),
                },
                out.reborrow(),
            )?;
        }
        *diagnostic = safe;
        Ok(())
    }
}

fn located_error(
    mut diagnostic: Diagnostic,
    primary: Option<LocatedSpan>,
    value_source: Option<ValueSource>,
) -> Diagnostic {
    if diagnostic.located_primary().is_none() {
        diagnostic.set_located_primary(primary);
    }
    if diagnostic.value_source().is_none() {
        diagnostic.set_value_source(value_source);
    }
    diagnostic
}
