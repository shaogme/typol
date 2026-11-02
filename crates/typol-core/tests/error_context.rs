use std::{error::Error, fmt::Debug, io::Error as IoError, mem::size_of, string::String};
use typol_core::{
    BudgetError, ConfigValueBuilder, Diagnostic, DiagnosticCode, DiagnosticContext,
    ExecutionBudget, ExecutionLimits, LocatedSpan, LocationError, LocationErrorKind, NamedValue,
    SourceId, SourceLocation, SourceReadError, SourceReadErrorKind, SourceSpan, Span, SpanLocation,
    TerminalSlot, ValueError, ValueErrorKind, ValueSource, diagnostic_context,
};
use typol_opeo_test_support::TestOpeo;

static TEST_CONTEXT: DiagnosticContext =
    diagnostic_context!("typol-core-tests", "error-context", "stable-failure-point");

macro_rules! ensure {
    ($condition:expr $(,)?) => {
        if !$condition {
            return Err(
                IoError::other(concat!("assertion failed: ", stringify!($condition))).into(),
            );
        }
    };
}

macro_rules! ensure_eq {
    ($left:expr, $right:expr $(,)?) => {{
        let left = &$left;
        let right = &$right;
        if left != right {
            return Err(IoError::other(format!(
                "assertion failed: {} and {} differ (actual {:?}, expected {:?})",
                stringify!($left),
                stringify!($right),
                left,
                right,
            ))
            .into());
        }
    }};
}

#[test]
fn budget_invariant_context_survives_recording_and_diagnostic_conversion()
-> Result<(), Box<dyn Error>> {
    let budget = TestOpeo::call(|out| ExecutionBudget::new(ExecutionLimits::new(10, 10_000), out))?;
    let error = budget.invariant_error(&TEST_CONTEXT);
    let recorded = budget
        .failure()
        .ok_or_else(|| IoError::other("budget did not record the internal error"))?;
    let diagnostic = Diagnostic::from(error);

    ensure_eq!(recorded.context(), Some(TEST_CONTEXT));
    ensure_eq!(diagnostic.code(), DiagnosticCode::INTERNAL_ERROR);
    ensure_eq!(diagnostic.context(), Some(TEST_CONTEXT));
    ensure_eq!(diagnostic.located_primary(), None);
    Ok(())
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
    let mut diagnostic = Diagnostic::emergency(DiagnosticCode::INTERNAL_ERROR);
    diagnostic.set_context(Some(TEST_CONTEXT));
    diagnostic.set_located_primary(Some(primary));
    let slot = slot
        .store(diagnostic)
        .ok_or_else(|| IoError::other("terminal slot reserve capacity was lost"))?;

    let stored = slot
        .get()
        .ok_or_else(|| IoError::other("terminal slot contents are missing"))?;
    ensure_eq!(stored.located_primary(), Some(primary));
    ensure_eq!(stored.context(), Some(TEST_CONTEXT));
    ensure_eq!(budget.workspace_bytes(), reserved);
    ensure_eq!(reserved - initial, size_of::<Diagnostic>());
    drop(slot);
    ensure_eq!(budget.workspace_bytes(), initial);
    Ok(())
}

#[test]
fn bounded_errors_expose_safe_numeric_context() -> Result<(), Box<dyn Error>> {
    let span_error = TestOpeo::call(|out| Span::new(9, 4, out))
        .err()
        .ok_or_else(|| IoError::other("expected a reversed range to be rejected"))?;
    ensure_eq!(span_error.attempted_range(), (9, 4));

    let read_error = SourceReadError::new(SourceReadErrorKind::InternalInvariantFailure)
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

fn alloc_debug(value: &impl Debug) -> String {
    format!("{value:?}")
}
