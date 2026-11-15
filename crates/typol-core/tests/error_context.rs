use std::{error::Error, fmt::Debug, io::Error as IoError, mem::size_of, string::String};
use typol_core::{
    BudgetError, CapacityExceeded, ConfigValueBuilder, Diagnostic, DiagnosticCode,
    DiagnosticContext, DiagnosticFailure, DiagnosticFieldStatus, DiagnosticKind,
    DiagnosticLabelInput, DiagnosticMessage, DiagnosticParameter, DiagnosticParameters,
    DiagnosticStage, ExecutionBudget, ExecutionLimits, InternalError, LocatedSpan, LocationError,
    LocationErrorKind, MAX_RELATED_LOCATIONS, NamedValue, RelatedLocationRole, ResourceLimit,
    SourceId, SourceLocation, SourceReadError, SourceReadErrorKind, SourceSpan, Span, SpanLocation,
    TerminalSlot, ValueError, ValueErrorKind, ValueSource, diagnostic_context,
};
use typol_i18n::{Language, Translator};
use typol_opeo_test_support::{TestOpeo, ensure, ensure_eq};
use u18n::RenderError;

static TEST_CONTEXT: DiagnosticContext =
    diagnostic_context!("typol-core-tests", "error-context", "stable-failure-point");
static CONVERSION_CONTEXT: DiagnosticContext =
    diagnostic_context!("typol", "conversion", "conversion-depth-limit");
static REGEX_COMPILE_CONTEXT: DiagnosticContext =
    diagnostic_context!("typol-regex", "regex-compile", "budget-error-conversion");

#[test]
fn budget_invariant_context_survives_recording_and_diagnostic_conversion()
-> Result<(), Box<dyn Error>> {
    let budget = TestOpeo::call(|out| ExecutionBudget::new(ExecutionLimits::new(10, 10_000), out))?;
    let error = budget.invariant_error(&TEST_CONTEXT);
    let recorded = budget
        .failure()
        .ok_or_else(|| IoError::other("budget did not record the internal error"))?;
    let mut diagnostic =
        Diagnostic::emergency(DiagnosticCode::BUDGET_INVARIANT_FAILURE, "internal failure");
    diagnostic.set_context(error.context());

    ensure_eq!(recorded.context(), Some(TEST_CONTEXT));
    ensure_eq!(diagnostic.code(), DiagnosticCode::BUDGET_INVARIANT_FAILURE);
    ensure_eq!(diagnostic.context(), Some(TEST_CONTEXT));
    ensure_eq!(diagnostic.located_primary(), None);
    Ok(())
}

#[test]
fn recorded_capacity_failure_preserves_its_category() -> Result<(), Box<dyn Error>> {
    let budget = TestOpeo::call(|out| ExecutionBudget::new(ExecutionLimits::new(10, 10_000), out))?;
    let error = BudgetError::CapacityExceeded(CapacityExceeded::HostSource);

    budget.record_failure(error);

    let recorded = budget
        .failure()
        .ok_or_else(|| IoError::other("budget did not record the capacity error"))?;
    ensure_eq!(recorded, error);
    ensure_eq!(
        recorded.diagnostic_code(),
        DiagnosticCode::HOST_SOURCE_CAPACITY_EXCEEDED
    );
    Ok(())
}

#[test]
fn regex_compilation_diagnostics_report_the_compilation_stage() {
    let capacity = Diagnostic::emergency(
        DiagnosticCode::REGEX_COMPILATION_CAPACITY_EXCEEDED,
        "capacity exceeded",
    );
    let mut invariant = Diagnostic::emergency(
        DiagnosticCode::REGEX_COMPILATION_INVARIANT_FAILURE,
        "internal failure",
    );
    invariant.set_context(Some(REGEX_COMPILE_CONTEXT));

    assert_eq!(capacity.kind().stage(), DiagnosticStage::SchemaCompilation);
    assert_eq!(invariant.kind().stage(), DiagnosticStage::SchemaCompilation);
}

#[test]
fn emergency_diagnostic_keeps_located_primary_without_workspace_growth()
-> Result<(), Box<dyn Error>> {
    let budget = TestOpeo::call(|out| ExecutionBudget::new(ExecutionLimits::new(10, 10_000), out))?;
    let initial = budget.workspace_bytes();
    let slot =
        TestOpeo::call::<BudgetError, _>(|out| TerminalSlot::<Diagnostic>::new(&budget, out))?;
    let reserved = budget.workspace_bytes();
    let source = SourceSpan::new(SourceId::new(3), Span::point(7));
    let start = SourceLocation::new(2, 4)
        .ok_or_else(|| IoError::other("line and column must be positive"))?;
    let end = SourceLocation::new(2, 6)
        .ok_or_else(|| IoError::other("line and column must be positive"))?;
    let primary = LocatedSpan::new(source, Some(SpanLocation { start, end }));
    let mut diagnostic =
        Diagnostic::emergency(DiagnosticCode::BUDGET_INVARIANT_FAILURE, "internal failure");
    diagnostic.set_context(Some(TEST_CONTEXT));
    diagnostic.set_located_primary(Some(primary));
    let slot = slot
        .store(diagnostic)
        .ok_or_else(|| IoError::other("terminal slot reserve capacity was lost"))?;

    let stored = slot
        .get()
        .ok_or_else(|| IoError::other("terminal slot contents are missing"))?;
    ensure_eq!(stored.located_primary(), Some(primary));
    ensure_eq!(
        stored.primary_location_status(),
        DiagnosticFieldStatus::Available
    );
    ensure_eq!(stored.context(), Some(TEST_CONTEXT));
    ensure!(!stored.kind().id().is_empty());
    ensure!(stored.message().is_ok_and(|message| !message.is_empty()));
    ensure_eq!(budget.workspace_bytes(), reserved);
    ensure_eq!(reserved - initial, size_of::<Diagnostic>());
    drop(slot);
    ensure_eq!(budget.workspace_bytes(), initial);
    Ok(())
}

#[cfg(target_pointer_width = "64")]
#[test]
fn primary_location_capacity_is_explicit() -> Result<(), Box<dyn Error>> {
    let source = SourceSpan::new(SourceId::new(2), Span::point(9));
    let start = SourceLocation::new(u32::MAX as usize + 1, 1)
        .ok_or_else(|| IoError::other("line and column must be positive"))?;
    let end = SourceLocation::new(u32::MAX as usize + 1, 2)
        .ok_or_else(|| IoError::other("line and column must be positive"))?;
    let mut diagnostic =
        Diagnostic::emergency(DiagnosticCode::BUDGET_INVARIANT_FAILURE, "internal failure");
    diagnostic.set_located_primary(Some(LocatedSpan::new(
        source,
        Some(SpanLocation { start, end }),
    )));

    ensure_eq!(diagnostic.primary(), Some(source));
    ensure_eq!(
        diagnostic.primary_location_status(),
        DiagnosticFieldStatus::Unavailable
    );
    ensure_eq!(
        diagnostic.located_primary(),
        Some(LocatedSpan::from(source))
    );
    Ok(())
}

#[test]
fn emergency_diagnostics_expose_stable_kinds_and_nonempty_causes() {
    let mut diagnostic = Diagnostic::emergency(
        DiagnosticCode::TYPE_BINDING_DEPTH_LIMIT_EXCEEDED,
        "limit exceeded",
    );
    diagnostic.set_context(Some(CONVERSION_CONTEXT));

    assert_eq!(
        diagnostic.kind().id(),
        "typol.binding.conversion_depth_exceeded"
    );
    assert_eq!(diagnostic.kind().stage(), DiagnosticStage::TypeBinding);
    assert!(
        diagnostic
            .message()
            .is_ok_and(|message| !message.is_empty())
    );
}

#[test]
fn type_binding_node_limit_has_stable_code_and_stage() {
    let code = DiagnosticCode::TYPE_BINDING_NODES_LIMIT_EXCEEDED;
    let diagnostic = Diagnostic::emergency(code, "limit exceeded");

    assert_eq!(ResourceLimit::TypeBindingNodes.diagnostic_code(), code);
    assert_eq!(
        ResourceLimit::from_diagnostic_code(code),
        Some(ResourceLimit::TypeBindingNodes)
    );
    assert!(code.is_resource_limit());
    assert_eq!(code.name(), "TYPE_BINDING_NODES_LIMIT_EXCEEDED");
    assert_eq!(diagnostic.kind().stage(), DiagnosticStage::TypeBinding);
}

#[test]
fn every_resource_limit_code_has_a_non_internal_stage() -> Result<(), Box<dyn Error>> {
    for value in DiagnosticCode::SOURCE_BYTES_LIMIT_EXCEEDED.get()
        ..=DiagnosticCode::TYPE_BINDING_NODES_LIMIT_EXCEEDED.get()
    {
        let code = DiagnosticCode::new(value)
            .ok_or_else(|| IoError::other("resource limit code should be nonzero"))?;
        let diagnostic = Diagnostic::emergency(code, "limit exceeded");
        ensure!(code.is_resource_limit());
        ensure!(diagnostic.kind().stage() != DiagnosticStage::Internal);
    }
    let execution = Diagnostic::emergency(
        DiagnosticCode::EXECUTION_STEPS_LIMIT_EXCEEDED,
        "limit exceeded",
    );
    let workspace = Diagnostic::emergency(
        DiagnosticCode::WORKSPACE_BYTES_LIMIT_EXCEEDED,
        "limit exceeded",
    );
    ensure_eq!(execution.kind().stage(), DiagnosticStage::ExecutionBudget);
    ensure_eq!(workspace.kind().stage(), DiagnosticStage::ExecutionBudget);
    Ok(())
}

#[test]
fn emergency_diagnostic_keeps_final_text_without_allocating() {
    let diagnostic =
        Diagnostic::emergency(DiagnosticCode::CONFIG_VALUE_ERROR, "localized final text");

    assert_eq!(
        diagnostic.kind().id(),
        DiagnosticCode::CONFIG_VALUE_ERROR.name()
    );
    assert_eq!(diagnostic.message(), Ok("localized final text"));
}

#[test]
fn translation_failures_remain_errors_without_a_text_fallback() {
    let diagnostic = Diagnostic::translation_failure(
        DiagnosticCode::BUDGET_INVARIANT_FAILURE,
        RenderError::WriteFailed,
    );

    assert_eq!(
        diagnostic.message(),
        Err(DiagnosticFailure::Translation(RenderError::WriteFailed))
    );
}

#[test]
fn lexical_diagnostic_kinds_report_the_lexing_stage() {
    let kind = DiagnosticKind::new("syntax.invalid_character", DiagnosticStage::Lexing);

    assert_eq!(kind.stage(), DiagnosticStage::Lexing);
}

#[test]
fn diagnostic_identity_is_independent_from_its_final_message() -> Result<(), Box<dyn Error>> {
    let mut diagnostic = TestOpeo::call(|out| {
        Diagnostic::try_new(
            DiagnosticCode::CONFIG_VALUE_ERROR,
            DiagnosticMessage::custom("预期得到数组值"),
            None,
            out,
        )
    })?;

    diagnostic.set_kind(DiagnosticKind::new(
        "config.expected_array",
        DiagnosticStage::ConfigurationDecoding,
    ));

    ensure_eq!(diagnostic.kind().id(), "config.expected_array");
    ensure_eq!(diagnostic.message(), Ok("预期得到数组值"));
    Ok(())
}

#[test]
fn diagnostic_parameter_debug_redacts_values() {
    let parameter = DiagnosticParameter::unsigned("actual", 987_654_321);
    let debug = format!("{parameter:?}");

    assert!(debug.contains("actual"));
    assert!(debug.contains("redacted"));
    assert!(!debug.contains("987654321"));
}

#[test]
fn malformed_parameter_sets_report_unavailable_status() {
    let parameters = DiagnosticParameters::new([
        Some(DiagnosticParameter::unsigned("expected", 2)),
        None,
        Some(DiagnosticParameter::unsigned("actual", 3)),
        None,
    ]);

    assert_eq!(parameters.status(), DiagnosticFieldStatus::Unavailable);
    assert!(parameters.is_empty());
}

#[test]
fn related_location_capacity_exposes_truncation() -> Result<(), Box<dyn Error>> {
    let span = SourceSpan::new(SourceId::new(2), Span::point(3));
    let mut diagnostic = TestOpeo::call(|out| {
        Diagnostic::try_new(
            DiagnosticCode::CONFIG_VALUE_ERROR,
            DiagnosticMessage::custom("expected an array value"),
            None,
            out,
        )
    })?;
    for _ in 0..=MAX_RELATED_LOCATIONS {
        TestOpeo::call(|out| {
            diagnostic.try_add_label_with_text(
                DiagnosticLabelInput {
                    span,
                    text: "related input",
                    role: RelatedLocationRole::AssociatedInput,
                },
                None,
                out,
            )
        })?;
    }

    ensure_eq!(diagnostic.related_locations().len(), MAX_RELATED_LOCATIONS);
    ensure_eq!(
        diagnostic.related_locations_status(),
        DiagnosticFieldStatus::Available
    );
    ensure!(diagnostic.related_locations_truncated());
    Ok(())
}

#[test]
fn related_locations_keep_a_semantic_role() -> Result<(), Box<dyn Error>> {
    let span = SourceSpan::new(SourceId::new(1), Span::point(2));
    let mut diagnostic = TestOpeo::call(|out| {
        Diagnostic::try_new(
            DiagnosticCode::SCHEMA_SEMANTIC_ERROR,
            DiagnosticMessage::custom("the earlier field is here"),
            None,
            out,
        )
    })?;
    TestOpeo::call(|out| {
        diagnostic.try_add_label_with_text(
            DiagnosticLabelInput {
                span,
                text: "the earlier field is here",
                role: RelatedLocationRole::PreviousDeclaration,
            },
            None,
            out,
        )
    })?;

    let related = diagnostic
        .related_locations()
        .first()
        .ok_or_else(|| IoError::other("related location was not retained"))?;
    ensure_eq!(related.role(), RelatedLocationRole::PreviousDeclaration);
    ensure_eq!(related.span(), span);
    Ok(())
}

#[test]
fn bounded_errors_expose_safe_numeric_context() -> Result<(), Box<dyn Error>> {
    let span_error = TestOpeo::call(|out| Span::new(9, 4, out))
        .err()
        .ok_or_else(|| IoError::other("expected a reversed range to be rejected"))?;
    ensure_eq!(span_error.attempted_range(), (9, 4));

    let read_error = SourceReadError::new(SourceReadErrorKind::Internal(
        InternalError::SourceInputInvariantFailure(TEST_CONTEXT),
    ))
    .with_offset(12)
    .with_requested_bytes(5)
    .with_context(TEST_CONTEXT);
    ensure_eq!(read_error.offset(), Some(12));
    ensure_eq!(read_error.requested_bytes(), Some(5));
    ensure_eq!(read_error.context(), Some(TEST_CONTEXT));

    let location_error = LocationError::new(LocationErrorKind::OffsetOutOfBounds)
        .with_offset(12)
        .with_related_offset(8);
    ensure_eq!(location_error.offset(), Some(12));
    ensure_eq!(location_error.related_offset(), Some(8));

    let budget_error = BudgetError::invariant(&TEST_CONTEXT);
    let converted_read_error = SourceReadError::from(budget_error);
    let converted_value_error = ValueError::from(budget_error);
    ensure_eq!(converted_read_error.context(), Some(TEST_CONTEXT));
    ensure_eq!(converted_value_error.context(), Some(TEST_CONTEXT));
    Ok(())
}

#[test]
fn duplicate_value_error_reports_indices_without_copying_names() -> Result<(), Box<dyn Error>> {
    let mut builder = TestOpeo::call(|out| ConfigValueBuilder::new(None, out))?;
    let first = TestOpeo::call(|out| builder.add_unsigned(1, ValueSource::ConfigLiteral, out))?;
    let second = TestOpeo::call(|out| builder.add_unsigned(2, ValueSource::ConfigLiteral, out))?;
    let entries = [
        NamedValue::new("private-key-name", first),
        NamedValue::new("private-key-name", second),
    ];
    let result = TestOpeo::call(|out| builder.add_map(&entries, ValueSource::ConfigLiteral, out));
    let error = result
        .err()
        .ok_or_else(|| IoError::other("expected duplicate key to be rejected"))?;
    let debug = alloc_debug(&error);

    ensure_eq!(error.kind(), ValueErrorKind::DuplicateMapKey);
    ensure_eq!(error.duplicate_indices(), Some((0, 1)));
    ensure!(!debug.contains("private-key-name"));
    Ok(())
}

#[test]
fn every_resource_limit_has_emergency_text_in_each_locale() -> Result<(), Box<dyn Error>> {
    let limits = [
        ResourceLimit::SourceBytes,
        ResourceLimit::SchemaTotalBytes,
        ResourceLimit::SchemaSourceCount,
        ResourceLimit::SchemaImportDepth,
        ResourceLimit::ExecutionSteps,
        ResourceLimit::WorkspaceBytes,
        ResourceLimit::SchemaParseDepth,
        ResourceLimit::TokenBytes,
        ResourceLimit::StringBytes,
        ResourceLimit::SchemaAstNodes,
        ResourceLimit::ConfigParseDepth,
        ResourceLimit::ConfigValueDepth,
        ResourceLimit::ConfigValueNodes,
        ResourceLimit::ConfigTextBytes,
        ResourceLimit::EnvironmentEntries,
        ResourceLimit::EnvironmentBytes,
        ResourceLimit::ValidationTraversal,
        ResourceLimit::RegexCompilationBudget,
        ResourceLimit::RegexPatternBytes,
        ResourceLimit::RegexNestingDepth,
        ResourceLimit::RegexInstructionCount,
        ResourceLimit::RegexCharacterRanges,
        ResourceLimit::RegexExecutionSteps,
        ResourceLimit::RegexSearchSteps,
        ResourceLimit::RegexWorkspaceBytes,
        ResourceLimit::RegexScratchStates,
        ResourceLimit::TypeBindingDepth,
        ResourceLimit::SchemaFormatOutputBytes,
        ResourceLimit::ConfigFormatOutputBytes,
        ResourceLimit::FormattingWorkspaceBytes,
        ResourceLimit::FormattingNestingDepth,
        ResourceLimit::TypeBindingNodes,
    ];

    for language in [Language::English, Language::Chinese] {
        let translator = Translator::new(language);
        for limit in limits {
            let message = limit
                .emergency_message(translator)
                .map_err(|_| IoError::other("resource-limit message translation failed"))?;
            ensure!(!message.is_empty());
        }
    }
    Ok(())
}

fn alloc_debug(value: &impl Debug) -> String {
    format!("{value:?}")
}
