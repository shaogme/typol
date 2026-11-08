use core::mem::size_of;
use std::{error::Error, io::Error as IoError};
use typol_core::{
    BudgetError, ChargedString, ChargedVec, ConfigValueBuilder, Diagnostic, DiagnosticCode,
    ExecutionBudget, ExecutionLimits, InstancePath, NamedValue, PathSegment, SourceId, SourceSpan,
    Span, ValueErrorKind, ValueKind, ValueSource,
};
use typol_opeo_test_support::TestOpeo;

macro_rules! ensure {
    ($condition:expr $(,)?) => {
        if !$condition {
            return Err(IoError::other(concat!("assertion failed: ", stringify!($condition))).into());
        }
    };
    ($condition:expr, $($argument:tt)+) => {
        if !$condition {
            return Err(IoError::other(format!($($argument)+)).into());
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

fn make_budget(steps: usize) -> Result<ExecutionBudget, BudgetError> {
    TestOpeo::call(|out| ExecutionBudget::new(ExecutionLimits::new(steps, 1_048_576), out))
}

#[test]
fn shared_steps_accept_exact_limit_and_stop_before_the_next_unit() -> Result<(), Box<dyn Error>> {
    let budget = make_budget(3)?;
    let shared = TestOpeo::call::<BudgetError, _>(|out| budget.try_clone(out))?;
    TestOpeo::call::<BudgetError, _>(|out| budget.consume(2, out))?;
    TestOpeo::call::<BudgetError, _>(|out| shared.consume(1, out))?;
    ensure_eq!(budget.steps(), 3);
    ensure_eq!(
        TestOpeo::call::<BudgetError, _>(|out| shared.consume(1, out)),
        Err(BudgetError::Resource)
    );
    ensure_eq!(budget.steps(), 3);
    ensure!(budget.exhausted());
    Ok(())
}

#[test]
fn charged_vec_single_and_bulk_push_charge_one_step_per_insert() -> Result<(), Box<dyn Error>> {
    let single_budget = make_budget(100)?;
    let mut single =
        TestOpeo::call::<BudgetError, _>(|out| ChargedVec::<u64>::new(Some(&single_budget), out))?;
    TestOpeo::call::<BudgetError, _>(|out| single.try_reserve_exact(4, out))?;
    for value in [1, 2, 3] {
        TestOpeo::call::<BudgetError, _>(|out| single.try_push(value, out))?;
    }
    ensure_eq!(single_budget.steps(), 3);

    let bulk_budget = make_budget(100)?;
    let mut bulk =
        TestOpeo::call::<BudgetError, _>(|out| ChargedVec::<u64>::new(Some(&bulk_budget), out))?;
    TestOpeo::call::<BudgetError, _>(|out| bulk.try_reserve_exact(4, out))?;
    TestOpeo::call::<BudgetError, _>(|out| bulk.extend([1, 2, 3], out))?;
    ensure_eq!(bulk_budget.steps(), 3);
    ensure_eq!(single.as_slice(), bulk.as_slice());
    Ok(())
}

#[test]
fn growth_cost_is_separate_and_live_capacity_is_released_with_the_owner()
-> Result<(), Box<dyn Error>> {
    let budget = make_budget(100)?;
    let initial = budget.workspace_bytes();
    let mut values =
        TestOpeo::call::<BudgetError, _>(|out| ChargedVec::<u64>::new(Some(&budget), out))?;
    TestOpeo::call::<BudgetError, _>(|out| values.try_reserve_exact(2, out))?;
    TestOpeo::call::<BudgetError, _>(|out| values.try_push(1, out))?;
    TestOpeo::call::<BudgetError, _>(|out| values.try_push(2, out))?;
    let old_bytes = values.capacity() * size_of::<u64>();
    TestOpeo::call::<BudgetError, _>(|out| values.try_reserve(1, out))?;
    let new_bytes = values.capacity() * size_of::<u64>();
    ensure_eq!(budget.workspace_bytes(), initial + new_bytes);
    ensure_eq!(
        budget.peak_workspace_bytes(),
        initial + old_bytes + new_bytes
    );
    ensure_eq!(budget.steps(), 4);
    values.clear();
    ensure_eq!(budget.workspace_bytes(), initial + new_bytes);
    drop(values);
    ensure_eq!(budget.workspace_bytes(), initial);
    Ok(())
}

#[test]
fn charged_string_keeps_text_and_capacity_in_one_owner() -> Result<(), Box<dyn Error>> {
    let budget = make_budget(100)?;
    let initial = budget.workspace_bytes();
    let mut text =
        TestOpeo::call::<BudgetError, _>(|out| ChargedString::copy("€", Some(&budget), out))?;
    ensure_eq!(text.as_str(), "€");
    TestOpeo::call::<BudgetError, _>(|out| text.try_push_char('🙂', out))?;
    ensure_eq!(text.as_str(), "€🙂");
    ensure_eq!(budget.steps(), 10);
    ensure!(budget.workspace_bytes() >= initial + text.capacity());
    drop(text);
    ensure_eq!(budget.workspace_bytes(), initial);
    Ok(())
}

#[test]
fn arena_views_preserve_order_source_and_named_entries() -> Result<(), Box<dyn Error>> {
    let budget = make_budget(1_000)?;
    let initial = budget.workspace_bytes();
    let mut builder = TestOpeo::call(|out| ConfigValueBuilder::new(Some(&budget), out))?;
    let label = TestOpeo::call(|out| builder.add_string("€€🙂", ValueSource::RustValue, out))?;
    TestOpeo::call(|out| builder.add_environment_reference(&[label], "LABEL", None, out))?;
    let number = TestOpeo::call(|out| builder.add_unsigned(42, ValueSource::SchemaDefault, out))?;
    let fields = [
        NamedValue::new("name", label),
        NamedValue::new("count", number),
    ];
    let structure =
        TestOpeo::call(|out| builder.add_structure(&fields, ValueSource::RustValue, out))?;
    let root =
        TestOpeo::call(|out| builder.add_array(&[structure], ValueSource::ConfigLiteral, out))?;
    let value = TestOpeo::call(|out| builder.finish(root, out))?;

    ensure_eq!(value.node_count(), 4);
    ensure_eq!(value.source(), ValueSource::ConfigLiteral);
    let ValueKind::Array(mut items) = value.as_ref() else {
        return Err(IoError::other("root value is not an array").into());
    };
    let item = items
        .next()
        .ok_or_else(|| IoError::other("array item is missing"))?;
    let ValueKind::Structure(mut fields) = item.value().as_ref() else {
        return Err(IoError::other("array item is not a structure").into());
    };
    let first = fields
        .next()
        .ok_or_else(|| IoError::other("first field is missing"))?;
    ensure_eq!(first.name(), Some("name"));
    ensure_eq!(first.value().source(), ValueSource::EnvironmentVariable);
    ensure!(matches!(first.value().as_ref(), ValueKind::String("€€🙂")));
    ensure!(budget.workspace_bytes() > initial);
    drop(value);
    ensure_eq!(budget.workspace_bytes(), initial);
    Ok(())
}

#[test]
fn value_entries_nth_methods_keep_mixed_double_ended_iterator_semantics()
-> Result<(), Box<dyn Error>> {
    let mut builder = TestOpeo::call(|out| ConfigValueBuilder::new(None, out))?;
    let mut values = Vec::new();
    for value in 0..6 {
        values.push(TestOpeo::call(|out| {
            builder.add_unsigned(value, ValueSource::ConfigLiteral, out)
        })?);
    }
    let root = TestOpeo::call(|out| builder.add_array(&values, ValueSource::ConfigLiteral, out))?;
    let value = TestOpeo::call(|out| builder.finish(root, out))?;
    let ValueKind::Array(mut entries) = value.as_ref() else {
        return Err(IoError::other("root value is not an array").into());
    };

    ensure_eq!(entries.len(), 6);
    let third = entries
        .nth(2)
        .ok_or_else(|| IoError::other("third item is missing"))?;
    ensure!(matches!(third.value().as_ref(), ValueKind::Unsigned(2)));
    ensure_eq!(entries.len(), 3);
    let from_back = entries
        .nth_back(1)
        .ok_or_else(|| IoError::other("second-to-last item is missing"))?;
    ensure!(matches!(from_back.value().as_ref(), ValueKind::Unsigned(4)));
    ensure_eq!(entries.len(), 1);
    let last = entries
        .next_back()
        .ok_or_else(|| IoError::other("remaining item is missing"))?;
    ensure!(matches!(last.value().as_ref(), ValueKind::Unsigned(3)));
    ensure!(entries.next().is_none());

    let ValueKind::Array(mut forward) = value.as_ref() else {
        return Err(IoError::other("root value is not an array").into());
    };
    ensure!(forward.nth(6).is_none());
    ensure_eq!(forward.len(), 0);
    ensure!(forward.next_back().is_none());

    let ValueKind::Array(mut backward) = value.as_ref() else {
        return Err(IoError::other("root value is not an array").into());
    };
    ensure!(backward.nth_back(6).is_none());
    ensure_eq!(backward.len(), 0);
    ensure!(backward.next().is_none());
    Ok(())
}

#[test]
fn arena_rejects_foreign_ids_and_duplicate_names_without_copying_them() -> Result<(), Box<dyn Error>>
{
    let mut first = TestOpeo::call(|out| ConfigValueBuilder::new(None, out))?;
    let child = TestOpeo::call(|out| first.add_unsigned(1, ValueSource::ConfigLiteral, out))?;
    let mut second = TestOpeo::call(|out| ConfigValueBuilder::new(None, out))?;
    let foreign = TestOpeo::call(|out| second.add_array(&[child], ValueSource::ConfigLiteral, out));
    ensure!(matches!(
        foreign,
        Err(error) if error.kind() == ValueErrorKind::InternalInvariantFailure
    ));

    let left = TestOpeo::call(|out| first.add_unsigned(2, ValueSource::ConfigLiteral, out))?;
    let right = TestOpeo::call(|out| first.add_unsigned(3, ValueSource::ConfigLiteral, out))?;
    let values = [
        NamedValue::new("private-key-name", left),
        NamedValue::new("private-key-name", right),
    ];
    let error = TestOpeo::call(|out| first.add_map(&values, ValueSource::ConfigLiteral, out))
        .err()
        .ok_or_else(|| IoError::other("expected duplicate key to be rejected"))?;
    ensure_eq!(error.kind(), ValueErrorKind::DuplicateMapKey);
    ensure_eq!(error.duplicate_indices(), Some((0, 1)));
    ensure!(!format!("{error:?}").contains("private-key-name"));
    Ok(())
}

#[test]
fn diagnostic_rejects_a_path_owned_by_another_execution() -> Result<(), Box<dyn Error>> {
    let first = make_budget(1_000)?;
    let second = make_budget(1_000)?;
    let mut diagnostic = TestOpeo::call(|out| {
        Diagnostic::try_new(
            DiagnosticCode::INTERNAL_ERROR,
            "diagnostic",
            Some(&first),
            out,
        )
    })?;
    let mut path = TestOpeo::call(|out| InstancePath::new(Some(&second), out))?;
    TestOpeo::call::<BudgetError, _>(|out| path.push_field("field", out))?;
    ensure!(matches!(
        TestOpeo::call(|out| diagnostic.try_set_path(Some(path), Some(&first), out)),
        Err(BudgetError::InvariantViolation(_))
    ));
    ensure!(diagnostic.path().is_none());
    Ok(())
}

#[test]
fn failed_emergency_promotion_preserves_the_original_diagnostic_code() -> Result<(), Box<dyn Error>>
{
    let base = make_budget(100)?;
    let initial = base.workspace_bytes();
    drop(base);
    let budget = TestOpeo::call(|out| {
        ExecutionBudget::new(
            ExecutionLimits::new(100, initial + size_of::<PathSegment>()),
            out,
        )
    })?;
    let mut path = TestOpeo::call(|out| InstancePath::new(Some(&budget), out))?;
    TestOpeo::call::<BudgetError, _>(|out| path.push_index(0, out))?;
    let mut diagnostic = Diagnostic::emergency(DiagnosticCode::INTERNAL_ERROR);
    ensure!(matches!(
        TestOpeo::call(|out| diagnostic.try_set_path(Some(path), Some(&budget), out)),
        Err(BudgetError::Resource)
    ));
    ensure_eq!(diagnostic.code(), DiagnosticCode::INTERNAL_ERROR);
    ensure!(diagnostic.path().is_none());
    Ok(())
}

#[test]
fn diagnostic_body_labels_and_path_share_one_budget() -> Result<(), Box<dyn Error>> {
    let budget = make_budget(1_000)?;
    let initial = budget.workspace_bytes();
    let mut diagnostic = TestOpeo::call(|out| {
        Diagnostic::try_new(
            DiagnosticCode::INTERNAL_ERROR,
            "message",
            Some(&budget),
            out,
        )
    })?;
    let span = SourceSpan::new(SourceId::new(0), Span::point(0));
    TestOpeo::call(|out| diagnostic.try_add_label(span, "rule explanation", Some(&budget), out))?;
    let mut path = TestOpeo::call(|out| InstancePath::new(Some(&budget), out))?;
    TestOpeo::call::<BudgetError, _>(|out| path.push_field("field", out))?;
    TestOpeo::call(|out| diagnostic.try_set_path(Some(path), Some(&budget), out))?;
    ensure_eq!(
        diagnostic.labels().first().map(|label| label.message()),
        Some("rule explanation")
    );
    ensure!(
        diagnostic
            .budget()
            .is_some_and(|owner| owner.same_call(&budget))
    );
    drop(diagnostic);
    ensure_eq!(budget.workspace_bytes(), initial);
    Ok(())
}
