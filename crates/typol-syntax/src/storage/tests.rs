use super::{AstAccessError, BuiltinType, Declaration, Id, ListId, Name, Storage, TextRef};
use crate::{
    SyntaxLimits,
    ast::{ModuleDecl, Name as NameView},
    storage::{ExpressionBuilder, TypeBuilder},
};
use typol_core::{
    BudgetError, ChargedVec, Diagnostic, DiagnosticCode, ExecutionBudget, ExecutionLimits,
    SourceId, SourceSpan, Span, SpanError,
};
use typol_test_support::TestSlot;

#[derive(Debug)]
struct TestFailure;

type TestResult<T = ()> = Result<T, TestFailure>;

trait TestValue<T> {
    fn test_value(self) -> TestResult<T>;
}

impl<T, E> TestValue<T> for Result<T, E> {
    fn test_value(self) -> TestResult<T> {
        self.map_err(|_| TestFailure)
    }
}

impl<T> TestValue<T> for Option<T> {
    fn test_value(self) -> TestResult<T> {
        self.ok_or(TestFailure)
    }
}

trait TestError<T> {
    fn test_error(self) -> TestResult<T>;
}

impl<T, E> TestError<E> for Result<T, E> {
    fn test_error(self) -> TestResult<E> {
        self.err().ok_or(TestFailure)
    }
}

macro_rules! test_assert {
    ($condition:expr $(, $($message:tt)*)?) => {{
        if !$condition {
            return Err(TestFailure);
        }
    }};
}

macro_rules! test_assert_eq {
    ($left:expr, $right:expr $(, $($message:tt)*)?) => {{
        let left = &$left;
        let right = &$right;
        if left != right {
            return Err(TestFailure);
        }
    }};
}

#[test]
fn construction_rejects_foreign_and_out_of_bounds_spans_without_mutation() -> TestResult {
    let mut budget_errors = TestSlot::<BudgetError>::default();
    let mut span_errors = TestSlot::<SpanError>::default();
    let mut diagnostic_errors = TestSlot::<Diagnostic>::default();
    let mut access_errors = TestSlot::<AstAccessError>::default();
    let execution = budget_errors
        .access(|out| ExecutionBudget::new(ExecutionLimits::new(67_108_864, 67_108_864), out))
        .test_value()?;

    let source_id = SourceId::new(7);
    let valid = SourceSpan::new(
        source_id,
        span_errors
            .access(|out| Span::new(0, 3, out))
            .test_value()?,
    );
    let mut storage = diagnostic_errors
        .access(|out| Storage::new(source_id, 3, &SyntaxLimits::default(), &execution, out))
        .test_value()?;
    let name = diagnostic_errors
        .access(|out| storage.name_try_new("abc", valid, out))
        .test_value()?;
    let expression = diagnostic_errors
        .access(|out| ExpressionBuilder::expr_name(&mut storage, name, valid, out))
        .test_value()?;
    for invalid in [
        SourceSpan::new(
            SourceId::new(8),
            span_errors
                .access(|out| Span::new(0, 3, out))
                .test_value()?,
        ),
        SourceSpan::new(
            source_id,
            span_errors
                .access(|out| Span::new(0, 4, out))
                .test_value()?,
        ),
    ] {
        test_assert_eq!(
            diagnostic_errors
                .access(|out| storage.name_try_new("bad", invalid, out))
                .test_error()?
                .code(),
            DiagnosticCode::SCHEMA_SYNTAX_ERROR
        );
        test_assert_eq!(
            diagnostic_errors
                .access(|out| storage.expr_set_span(expression, invalid, out))
                .test_error()?
                .code(),
            DiagnosticCode::SCHEMA_SYNTAX_ERROR
        );
        test_assert_eq!(
            access_errors
                .access(|out| storage.name_text(name, out))
                .test_value()?,
            "abc"
        );
        test_assert_eq!(storage.name.len(), 1);
        test_assert!(matches!(
            diagnostic_errors.access(|out| storage.span(expression, out)),
            Ok(span) if span == valid
        ));
    }
    let empty_segments = diagnostic_errors
        .access(|out| ChargedVec::new(Some(&execution), out))
        .test_value()?;
    test_assert_eq!(
        diagnostic_errors
            .access(|out| TypeBuilder::type_path_new(&mut storage, empty_segments, valid, out))
            .test_error()?
            .code(),
        DiagnosticCode::SCHEMA_SYNTAX_ERROR
    );
    test_assert!(storage.type_path.is_empty());
    test_assert!(storage.name_edges.is_empty());

    Ok(())
}

#[test]
fn rejected_node_growth_does_not_mutate_tables_text_or_counter() -> TestResult {
    let mut budget_errors = TestSlot::<BudgetError>::default();
    let mut span_errors = TestSlot::<SpanError>::default();
    let mut diagnostic_errors = TestSlot::<Diagnostic>::default();
    let mut access_errors = TestSlot::<AstAccessError>::default();
    let execution = budget_errors
        .access(|out| ExecutionBudget::new(ExecutionLimits::new(67_108_864, 67_108_864), out))
        .test_value()?;

    let source = SourceId::new(7);
    let span = SourceSpan::new(
        source,
        span_errors
            .access(|out| Span::new(0, 3, out))
            .test_value()?,
    );
    let mut limits = SyntaxLimits::default();
    limits.set_max_schema_ast_nodes(1);
    let mut storage = diagnostic_errors
        .access(|out| Storage::new(source, 3, &limits, &execution, out))
        .test_value()?;
    let name = diagnostic_errors
        .access(|out| storage.name_try_new("abc", span, out))
        .test_value()?;
    test_assert_eq!(
        diagnostic_errors
            .access(|out| storage.name_try_new("new", span, out))
            .test_error()?
            .code(),
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
    );
    test_assert_eq!(storage.node_count(), 1);
    test_assert_eq!(
        access_errors
            .access(|out| storage.name_text(name, out))
            .test_value()?,
        "abc"
    );
    test_assert_eq!(storage.name.len(), 1);
    limits.set_max_schema_ast_nodes(10);
    limits.set_max_nesting_depth(usize::MAX);
    let mut storage = diagnostic_errors
        .access(|out| Storage::new(source, 3, &limits, &execution, out))
        .test_value()?;
    let child = diagnostic_errors
        .access(|out| TypeBuilder::type_expr_builtin(&mut storage, BuiltinType::U8, span, out))
        .test_value()?;
    storage.type_expr.get_mut(0).test_value()?.height = usize::MAX;
    test_assert_eq!(
        diagnostic_errors
            .access(|out| TypeBuilder::type_expr_option(&mut storage, child, span, out))
            .test_error()?
            .code(),
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
    );
    test_assert_eq!(storage.type_expr.len(), 1);
    test_assert_eq!(storage.node_count(), 1);

    Ok(())
}

#[test]
fn invalid_node_ids_fail_reads_and_writes_without_mutating_live_nodes() -> TestResult {
    let mut budget_errors = TestSlot::<BudgetError>::default();
    let mut span_errors = TestSlot::<SpanError>::default();
    let mut diagnostic_errors = TestSlot::<Diagnostic>::default();
    let mut access_errors = TestSlot::<AstAccessError>::default();
    let execution = budget_errors
        .access(|out| ExecutionBudget::new(ExecutionLimits::new(67_108_864, 67_108_864), out))
        .test_value()?;
    let source_id = SourceId::new(7);
    let span = SourceSpan::new(
        source_id,
        span_errors
            .access(|out| Span::new(0, 3, out))
            .test_value()?,
    );
    let replacement = SourceSpan::new(
        source_id,
        span_errors
            .access(|out| Span::new(0, 2, out))
            .test_value()?,
    );
    let mut storage = diagnostic_errors
        .access(|out| Storage::new(source_id, 3, &SyntaxLimits::default(), &execution, out))
        .test_value()?;
    let name = diagnostic_errors
        .access(|out| storage.name_try_new("abc", span, out))
        .test_value()?;
    let live_expression = diagnostic_errors
        .access(|out| ExpressionBuilder::expr_name(&mut storage, name, span, out))
        .test_value()?;
    let invalid_expression = diagnostic_errors
        .access(|out| ExpressionBuilder::expr_name(&mut storage, name, span, out))
        .test_value()?;
    let _removed = storage.expr.pop().test_value()?;

    test_assert_eq!(
        access_errors
            .access(|out| storage.get(invalid_expression, out))
            .test_error()?,
        AstAccessError::InvalidNodeId { id: 1 }
    );
    let error = diagnostic_errors
        .access(|out| storage.expr_set_span(invalid_expression, replacement, out))
        .test_error()?;
    test_assert_eq!(error.code(), DiagnosticCode::INTERNAL_ERROR);
    let context = error.context().test_value()?;
    test_assert_eq!(context.subsystem(), "typol-syntax");
    test_assert_eq!(context.stage(), "ast-storage");
    test_assert_eq!(context.failure_point(), "update-expression-span");
    test_assert_eq!(context.object_id(), Some(1));
    test_assert_eq!(error.primary(), Some(replacement));
    test_assert_eq!(storage.expr.len(), 1);
    test_assert!(matches!(
        access_errors.access(|out| storage.get(live_expression, out)),
        Ok(expression) if expression.span == span
    ));

    Ok(())
}

#[test]
fn invalid_list_ranges_return_access_errors() -> TestResult {
    let mut budget_errors = TestSlot::<BudgetError>::default();
    let mut span_errors = TestSlot::<SpanError>::default();
    let mut diagnostic_errors = TestSlot::<Diagnostic>::default();
    let mut access_errors = TestSlot::<AstAccessError>::default();
    let execution = budget_errors
        .access(|out| ExecutionBudget::new(ExecutionLimits::new(67_108_864, 67_108_864), out))
        .test_value()?;
    let source_id = SourceId::new(7);
    let span = SourceSpan::new(
        source_id,
        span_errors
            .access(|out| Span::new(0, 1, out))
            .test_value()?,
    );
    let mut storage = diagnostic_errors
        .access(|out| Storage::new(source_id, 1, &SyntaxLimits::default(), &execution, out))
        .test_value()?;
    let out_of_bounds = ListId::<Id<Name>>::new(0, 1);
    let reversed = ListId::<Id<Name>>::new(1, 0);
    let name = diagnostic_errors
        .access(|out| storage.name_try_new("m", span, out))
        .test_value()?;
    let declarations = diagnostic_errors
        .access(|out| ChargedVec::new(Some(&execution), out))
        .test_value()?;
    let module = diagnostic_errors
        .access(|out| storage.module_decl_new(name, declarations, span, out))
        .test_value()?;
    storage
        .module_decl
        .get_mut(module.index())
        .test_value()?
        .value
        .declarations = ListId::<Declaration>::new(0, 1);
    let module_record = access_errors
        .access(|out| storage.get(module, out))
        .test_value()?;
    let module_view = ModuleDecl::project(&storage, module_record);

    test_assert_eq!(
        access_errors
            .access(|out| storage.list(out_of_bounds, out))
            .test_error()?,
        AstAccessError::InvalidListRange { start: 0, end: 1 }
    );
    test_assert_eq!(
        access_errors
            .access(|out| storage.list(reversed, out))
            .test_error()?,
        AstAccessError::InvalidListRange { start: 1, end: 0 }
    );
    test_assert_eq!(
        access_errors
            .access(|out| module_view.declarations(out))
            .test_error()?,
        AstAccessError::InvalidListRange { start: 0, end: 1 }
    );

    Ok(())
}

#[test]
fn invalid_text_ranges_and_decoded_indices_return_access_errors() -> TestResult {
    let mut budget_errors = TestSlot::<BudgetError>::default();
    let mut span_errors = TestSlot::<SpanError>::default();
    let mut diagnostic_errors = TestSlot::<Diagnostic>::default();
    let mut access_errors = TestSlot::<AstAccessError>::default();
    let execution = budget_errors
        .access(|out| ExecutionBudget::new(ExecutionLimits::new(67_108_864, 67_108_864), out))
        .test_value()?;
    let source_id = SourceId::new(7);
    let span = SourceSpan::new(
        source_id,
        span_errors
            .access(|out| Span::new(0, 3, out))
            .test_value()?,
    );
    let mut storage = diagnostic_errors
        .access(|out| Storage::new(source_id, 3, &SyntaxLimits::default(), &execution, out))
        .test_value()?;
    let name = diagnostic_errors
        .access(|out| storage.name_try_new("abc", span, out))
        .test_value()?;

    test_assert_eq!(
        access_errors
            .access(|out| storage.text(TextRef::Pooled { start: 0, end: 3 }, out))
            .test_value()?,
        "abc"
    );
    test_assert_eq!(
        access_errors
            .access(|out| storage.text(TextRef::Pooled { start: 0, end: 4 }, out))
            .test_error()?,
        AstAccessError::InvalidTextRange { start: 0, end: 4 }
    );
    test_assert_eq!(
        access_errors
            .access(|out| storage.text(TextRef::Pooled { start: 2, end: 1 }, out))
            .test_error()?,
        AstAccessError::InvalidTextRange { start: 2, end: 1 }
    );
    test_assert_eq!(
        access_errors
            .access(|out| storage.text(TextRef::Decoded(0), out))
            .test_error()?,
        AstAccessError::InvalidDecodedTextId { id: 0 }
    );
    storage.name.get_mut(name.index()).test_value()?.value.text =
        TextRef::Pooled { start: 0, end: 4 };
    let name_record = access_errors
        .access(|out| storage.get(name, out))
        .test_value()?;
    let name_view = NameView::project(&storage, name_record);
    test_assert_eq!(
        access_errors
            .access(|out| name_view.text(out))
            .test_error()?,
        AstAccessError::InvalidTextRange { start: 0, end: 4 }
    );

    Ok(())
}
