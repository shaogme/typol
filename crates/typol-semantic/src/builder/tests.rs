mod defaults;

use crate::{
    AssertionInfo, BuildError, Builder, ClosureLocalId, ConstraintKind, ConstraintLayer,
    ConstraintRecord, ExpressionBinding, ExpressionNodeId, FieldId, FieldRecord, NumberValue,
    RefinementInput, SchemaAccessError, ShapeKind, SpanLocation, StoredConstraintData, TypeId,
    TypeRecord, TypeRecordOptions, TypedExpression, TypedExpressionKind, TypedExpressionNode,
    TypedLiteral,
};
use alloc::{format, string::String, vec, vec::Vec};
use core::fmt::{Debug, Formatter, Result as FmtResult};
use opeo::{OResult, Out};
use typol_core::{
    BudgetError, ChargedString, ChargedVec, ConfigValue, ConfigValueBuilder, ExecutionBudget,
    ExecutionLimits, SourceId, SourceLocation, SourceSpan, Span, SpanError, ValueError, ValueId,
    ValueSource,
};
use typol_model::{BinaryOperator, BuiltinType};
use typol_policy::{RegexLimits, ValueLimits};
use typol_test_support::TestSlot;

enum TestError {
    Build(BuildError),
    Budget(BudgetError),
    Value(ValueError),
    Span(SpanError),
    MissingLocation,
    Assertion(String),
}

impl Debug for TestError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        match self {
            Self::Build(error) => write!(formatter, "Build({error:?})"),
            Self::Budget(error) => write!(formatter, "Budget({error:?})"),
            Self::Value(error) => write!(formatter, "Value({error:?})"),
            Self::Span(error) => write!(formatter, "Span({error:?})"),
            Self::MissingLocation => formatter.write_str("MissingLocation"),
            Self::Assertion(message) => write!(formatter, "Assertion({message})"),
        }
    }
}

impl From<BuildError> for TestError {
    fn from(error: BuildError) -> Self {
        Self::Build(error)
    }
}

impl From<BudgetError> for TestError {
    fn from(error: BudgetError) -> Self {
        Self::Budget(error)
    }
}

impl From<ValueError> for TestError {
    fn from(error: ValueError) -> Self {
        Self::Value(error)
    }
}

impl From<SpanError> for TestError {
    fn from(error: SpanError) -> Self {
        Self::Span(error)
    }
}

type TestResult<T> = Result<T, TestError>;

struct TestSlots {
    build_errors: TestSlot<BuildError>,
    budget_errors: TestSlot<BudgetError>,
    value_errors: TestSlot<ValueError>,
    span_errors: TestSlot<SpanError>,
}

impl TestSlots {
    fn new() -> Self {
        Self {
            build_errors: TestSlot::default(),
            budget_errors: TestSlot::default(),
            value_errors: TestSlot::default(),
            span_errors: TestSlot::default(),
        }
    }

    fn build<T>(
        &mut self,
        operation: impl for<'slot> FnOnce(Out<'slot, 'slot, BuildError>) -> OResult<'slot, T>,
    ) -> Result<T, BuildError> {
        self.build_errors.access(operation)
    }

    fn budget<T>(
        &mut self,
        operation: impl for<'slot> FnOnce(Out<'slot, 'slot, BudgetError>) -> OResult<'slot, T>,
    ) -> Result<T, BudgetError> {
        self.budget_errors.access(operation)
    }

    fn span<T>(
        &mut self,
        operation: impl for<'slot> FnOnce(Out<'slot, 'slot, SpanError>) -> OResult<'slot, T>,
    ) -> Result<T, SpanError> {
        self.span_errors.access(operation)
    }

    fn value<T>(
        &mut self,
        operation: impl for<'slot> FnOnce(Out<'slot, 'slot, ValueError>) -> OResult<'slot, T>,
    ) -> Result<T, ValueError> {
        self.value_errors.access(operation)
    }
}

macro_rules! check_eq {
    ($actual:expr, $expected:expr $(,)?) => {{
        let actual = &$actual;
        let expected = &$expected;
        if actual != expected {
            return Err(TestError::Assertion(format!(
                "assertion failed: {}; actual={actual:?}, expected={expected:?}",
                stringify!($actual)
            )));
        }
    }};
}

macro_rules! check {
    ($condition:expr $(,)?) => {{
        if !$condition {
            return Err(TestError::Assertion(format!(
                "assertion failed: {}",
                stringify!($condition)
            )));
        }
    }};
}

fn span(slots: &mut TestSlots) -> TestResult<SourceSpan> {
    let span = slots.span(|out| Span::new(0, 0, out))?;
    Ok(SourceSpan::new(SourceId::new(0), span))
}

fn locations(slots: &mut TestSlots) -> TestResult<Vec<(SourceSpan, SpanLocation)>> {
    let position = SourceLocation::new(1, 1).ok_or(TestError::MissingLocation)?;
    Ok(vec![(
        span(slots)?,
        SpanLocation {
            start: position,
            end: position,
        },
    )])
}

fn config_value(
    slots: &mut TestSlots,
    operation: impl for<'slot, 'borrow> FnOnce(
        &mut ConfigValueBuilder,
        Out<'slot, 'borrow, ValueError>,
    ) -> OResult<'slot, ValueId>,
) -> TestResult<ConfigValue> {
    let value = slots.value(|mut out| {
        let mut builder = ConfigValueBuilder::new(None, out.reborrow())?;
        let root = operation(&mut builder, out.reborrow())?;
        builder.finish(root, out)
    })?;
    Ok(value)
}

fn unsigned_value(slots: &mut TestSlots, value: u128) -> TestResult<ConfigValue> {
    config_value(slots, |builder, out| {
        builder.add_unsigned(value, ValueSource::RustValue, out)
    })
}

fn signed_value(slots: &mut TestSlots, value: i128) -> TestResult<ConfigValue> {
    config_value(slots, |builder, out| {
        builder.add_signed(value, ValueSource::RustValue, out)
    })
}

fn float32_value(slots: &mut TestSlots, value: f32) -> TestResult<ConfigValue> {
    config_value(slots, |builder, out| {
        builder.add_float32(value, ValueSource::RustValue, out)
    })
}

fn float64_value(slots: &mut TestSlots, value: f64) -> TestResult<ConfigValue> {
    config_value(slots, |builder, out| {
        builder.add_float64(value, ValueSource::RustValue, out)
    })
}

fn unsigned_array_value(slots: &mut TestSlots, value: u128) -> TestResult<ConfigValue> {
    config_value(slots, |builder, mut out| {
        let child = builder.add_unsigned(value, ValueSource::RustValue, out.reborrow())?;
        builder.add_array(&[child], ValueSource::RustValue, out)
    })
}

fn scalar(slots: &mut TestSlots, model: &mut Builder, builtin: BuiltinType) -> TestResult<TypeId> {
    let execution = slots.budget(|out| model.execution().try_clone(out))?;
    let id_span = span(slots)?;
    let id = slots.build(|out| model.reserve_type(id_span, out))?;
    let shape = slots.build(|out| model.intern_shape(ShapeKind::Builtin(builtin), out))?;
    let record_span = span(slots)?;
    let record = slots.budget(|out| {
        TypeRecord::new(
            TypeRecordOptions {
                name: None,
                qualified_name: None,
                shape,
                child: None,
                span: record_span,
            },
            &execution,
            out,
        )
    })?;
    slots.build(|out| model.set_type(id, record, out))?;
    Ok(id)
}

struct BaseIds {
    root: TypeId,
    boolean: TypeId,
}

fn base(slots: &mut TestSlots) -> TestResult<(Builder, BaseIds)> {
    let execution = slots
        .budget(|out| ExecutionBudget::new(ExecutionLimits::new(67_108_864, 67_108_864), out))?;
    let mut model = slots.build(|out| {
        Builder::new(
            &execution,
            ValueLimits::default(),
            RegexLimits::default(),
            out,
        )
    })?;
    let root_span = span(slots)?;
    let root = slots.build(|out| model.reserve_type(root_span, out))?;
    let shape = slots.build(|out| model.intern_shape(ShapeKind::Struct(root), out))?;
    let name = slots.budget(|out| ChargedString::copy("Root", Some(&execution), out))?;
    let record_span = span(slots)?;
    let record = slots.budget(|out| {
        TypeRecord::new(
            TypeRecordOptions {
                name: Some(name),
                qualified_name: None,
                shape,
                child: None,
                span: record_span,
            },
            &execution,
            out,
        )
    })?;
    slots.build(|out| model.set_type(root, record, out))?;
    let boolean = scalar(slots, &mut model, BuiltinType::Bool)?;
    Ok((model, BaseIds { root, boolean }))
}

struct AssertionRequest<'a> {
    model: &'a mut Builder,
    root: TypeId,
    boolean: TypeId,
    nodes: Vec<TypedExpressionNode>,
    index: usize,
}

#[test]
fn schema_access_build_error_preserves_the_table_and_index() -> TestResult<()> {
    let access = SchemaAccessError::new("constraint", 23);
    let error = BuildError::from(access);
    let context = error.context().ok_or(TestError::Assertion(String::from(
        "schema access context is missing",
    )))?;

    check_eq!(error.schema_access(), Some(access));
    check_eq!(context.subsystem(), "typol-semantic");
    check_eq!(context.stage(), "schema-access");
    check_eq!(context.object_id(), Some(23));
    Ok(())
}

fn assertion(slots: &mut TestSlots, request: AssertionRequest<'_>) -> TestResult<()> {
    let AssertionRequest {
        model,
        root,
        boolean,
        nodes,
        index,
    } = request;
    let budget = slots
        .budget(|out| ExecutionBudget::new(ExecutionLimits::new(67_108_864, 67_108_864), out))?;
    let mut stored = slots.budget(|out| ChargedVec::new(Some(&budget), out))?;
    for node in nodes {
        slots.budget(|out| stored.try_push(node, out))?;
    }
    let expression_span = span(slots)?;
    let expression = TypedExpression::new(
        stored,
        ExpressionNodeId::new(index),
        boolean,
        expression_span,
    );
    let message = slots.budget(|out| ChargedString::with_capacity(0, Some(&budget), out))?;
    let assertion_span = span(slots)?;
    let id = slots.build(|out| {
        model.add_program(AssertionInfo::new(expression, message, assertion_span), out)
    })?;
    let assertions = budget_items(slots, &[id], &budget)?;
    slots
        .build(|out| model.type_mut(root, out))
        .map(|record| record.set_assertions(assertions))?;
    Ok(())
}

fn freeze_failure(slots: &mut TestSlots, model: Builder, root: TypeId) -> TestResult<BuildError> {
    let execution = slots.budget(|out| model.execution().try_clone(out))?;
    let source_locations = locations(slots)?;
    let locations = budget_items(slots, &source_locations, &execution)?;
    match slots
        .build_errors
        .access(|out| model.freeze(root, &[""], locations, out))
    {
        Ok(_) => Err(TestError::Assertion(String::from(
            "schema freezing should fail",
        ))),
        Err(error) => Ok(error),
    }
}

fn freeze_error(slots: &mut TestSlots, model: Builder, root: TypeId) -> TestResult<()> {
    let error = freeze_failure(slots, model, root)?;
    check!(error.is_internal() && error.context().is_some());
    Ok(())
}

fn field(
    slots: &mut TestSlots,
    model: &mut Builder,
    root: TypeId,
    ty: TypeId,
    value: ConfigValue,
) -> TestResult<()> {
    let execution = slots.budget(|out| model.execution().try_clone(out))?;
    let name = slots.budget(|out| ChargedString::copy("value", Some(&execution), out))?;
    let field_span = span(slots)?;
    let mut record = FieldRecord::new(name, ty, field_span);
    record.set_default(Some(value));
    let id = slots.build(|out| model.add_field(record, out))?;
    let fields = budget_items(slots, &[id], &execution)?;
    slots
        .build(|out| model.type_mut(root, out))?
        .set_fields(fields);
    Ok(())
}

#[test]
fn topology_rejects_missing_forward_and_cyclic_children_on_insertion() -> TestResult<()> {
    let mut slots = TestSlots::new();
    for (index, child) in [(0, 0), (1, 1), (0, usize::MAX)] {
        let (mut model, base_ids) = base(&mut slots)?;
        let root = base_ids.root;
        let boolean = base_ids.boolean;
        let node_span = span(&mut slots)?;
        let nodes = vec![TypedExpressionNode::new(
            TypedExpressionKind::Binary {
                operator: BinaryOperator::And,
                left: ExpressionNodeId::new(child),
                right: ExpressionNodeId::new(child),
            },
            boolean,
            node_span,
        )];
        check!(matches!(
            assertion(
                &mut slots,
                AssertionRequest {
                    model: &mut model,
                    root,
                    boolean,
                    nodes,
                    index,
                }
            ),
            Err(TestError::Build(error)) if error.is_internal() && error.context().is_some()
        ));
    }
    Ok(())
}

#[test]
fn freeze_rejects_escaped_closure_and_incompatible_operands() -> TestResult<()> {
    let mut slots = TestSlots::new();
    let (mut model, base_ids) = base(&mut slots)?;
    let root = base_ids.root;
    let boolean = base_ids.boolean;
    let closure_span = span(&mut slots)?;
    assertion(
        &mut slots,
        AssertionRequest {
            model: &mut model,
            root,
            boolean,
            nodes: vec![TypedExpressionNode::new(
                TypedExpressionKind::Binding(ExpressionBinding::Closure(ClosureLocalId::new(0))),
                boolean,
                closure_span,
            )],
            index: 0,
        },
    )?;
    freeze_error(&mut slots, model, root)?;
    let (mut model, base_ids) = base(&mut slots)?;
    let root = base_ids.root;
    let boolean = base_ids.boolean;
    let integer = scalar(&mut slots, &mut model, BuiltinType::U8)?;
    let first_span = span(&mut slots)?;
    let second_span = span(&mut slots)?;
    let operator_span = span(&mut slots)?;
    let nodes = vec![
        TypedExpressionNode::new(
            TypedExpressionKind::Literal(TypedLiteral::Integer(1)),
            integer,
            first_span,
        ),
        TypedExpressionNode::new(
            TypedExpressionKind::Literal(TypedLiteral::Boolean(true)),
            boolean,
            second_span,
        ),
        TypedExpressionNode::new(
            TypedExpressionKind::Binary {
                operator: BinaryOperator::And,
                left: ExpressionNodeId::new(0),
                right: ExpressionNodeId::new(1),
            },
            boolean,
            operator_span,
        ),
    ];
    assertion(
        &mut slots,
        AssertionRequest {
            model: &mut model,
            root,
            boolean,
            nodes,
            index: 2,
        },
    )?;
    freeze_error(&mut slots, model, root)?;
    Ok(())
}

#[test]
fn freeze_rejects_fields_from_other_nominal_owners() -> TestResult<()> {
    let mut slots = TestSlots::new();
    let (mut model, base_ids) = base(&mut slots)?;
    let root = base_ids.root;
    let boolean = base_ids.boolean;
    let execution = slots.budget(|out| model.execution().try_clone(out))?;
    let other_span = span(&mut slots)?;
    let other = slots.build(|out| model.reserve_type(other_span, out))?;
    let shape = slots.build(|out| model.intern_shape(ShapeKind::Struct(other), out))?;
    let field_name = slots.budget(|out| ChargedString::copy("flag", Some(&execution), out))?;
    let field_span = span(&mut slots)?;
    let id = slots
        .build(|out| model.add_field(FieldRecord::new(field_name, boolean, field_span), out))?;
    let record_span = span(&mut slots)?;
    let mut record = slots.budget(|out| {
        TypeRecord::new(
            TypeRecordOptions {
                name: None,
                qualified_name: None,
                shape,
                child: None,
                span: record_span,
            },
            &execution,
            out,
        )
    })?;
    record.set_fields(budget_items(&mut slots, &[id], &execution)?);
    slots.build(|out| model.set_type(other, record, out))?;
    let self_span = span(&mut slots)?;
    let field_node_span = span(&mut slots)?;
    let nodes = vec![
        TypedExpressionNode::new(
            TypedExpressionKind::Binding(ExpressionBinding::SelfValue),
            root,
            self_span,
        ),
        TypedExpressionNode::new(
            TypedExpressionKind::Field {
                receiver: ExpressionNodeId::new(0),
                field: id,
            },
            boolean,
            field_node_span,
        ),
    ];
    assertion(
        &mut slots,
        AssertionRequest {
            model: &mut model,
            root,
            boolean,
            nodes,
            index: 1,
        },
    )?;
    freeze_error(&mut slots, model, root)?;
    Ok(())
}

#[test]
fn defaults_are_checked_for_width_float_category_and_nested_values() -> TestResult<()> {
    let mut slots = TestSlots::new();
    let values = [
        unsigned_value(&mut slots, 256)?,
        signed_value(&mut slots, 1)?,
        float32_value(&mut slots, 1.0)?,
    ];
    for value in values {
        let (mut model, base_ids) = base(&mut slots)?;
        let root = base_ids.root;
        let integer = scalar(&mut slots, &mut model, BuiltinType::U8)?;
        field(&mut slots, &mut model, root, integer, value)?;
        let error = freeze_failure(&mut slots, model, root)?;
        check!(error.is_internal() && error.context().is_some());
        check_eq!(error.source_span(), Some(span(&mut slots)?));
    }
    let (mut model, base_ids) = base(&mut slots)?;
    let root = base_ids.root;
    let float = scalar(&mut slots, &mut model, BuiltinType::F32)?;
    let value = float64_value(&mut slots, 1.0)?;
    field(&mut slots, &mut model, root, float, value)?;
    freeze_error(&mut slots, model, root)?;
    let (mut model, base_ids) = base(&mut slots)?;
    let root = base_ids.root;
    let execution = slots.budget(|out| model.execution().try_clone(out))?;
    let integer = scalar(&mut slots, &mut model, BuiltinType::U8)?;
    let array_span = span(&mut slots)?;
    let array = slots.build(|out| model.reserve_type(array_span, out))?;
    let element = slots.build(|out| model.type_shape(integer, out))?;
    let shape = slots.build(|out| {
        model.intern_shape(
            ShapeKind::Array {
                element,
                length: Some(1),
            },
            out,
        )
    })?;
    let record_span = span(&mut slots)?;
    let record = slots.budget(|out| {
        TypeRecord::new(
            TypeRecordOptions {
                name: None,
                qualified_name: None,
                shape,
                child: Some(integer),
                span: record_span,
            },
            &execution,
            out,
        )
    })?;
    slots.build(|out| model.set_type(array, record, out))?;
    let value = unsigned_array_value(&mut slots, 256)?;
    field(&mut slots, &mut model, root, array, value)?;
    freeze_error(&mut slots, model, root)?;
    Ok(())
}

#[test]
fn checked_shapes_rules_and_freeze_reject_invalid_references_and_cycles() -> TestResult<()> {
    let mut slots = TestSlots::new();
    let (mut model, base_ids) = base(&mut slots)?;
    let root = base_ids.root;
    let boolean = base_ids.boolean;
    let execution = slots.budget(|out| model.execution().try_clone(out))?;
    let inner_shape = slots.build(|out| model.type_shape(boolean, out))?;
    let option = slots.build(|out| model.intern_shape(ShapeKind::Option(inner_shape), out))?;
    check!(matches!(
        slots
            .build_errors
            .access(|out| model.intern_shape(ShapeKind::Option(option), out)),
        Err(error) if error.is_internal() && error.context().is_some()
    ));
    let rule_span = span(&mut slots)?;
    let rule = slots.build(|out| {
        model.add_constraint(
            ConstraintRecord::new(
                ConstraintKind::Min,
                ConstraintLayer::Alias,
                rule_span,
                StoredConstraintData::NumberBounds {
                    min: Some(NumberValue::Unsigned(1)),
                    max: None,
                },
            ),
            out,
        )
    })?;
    let rules = budget_items(&mut slots, &[rule], &execution)?;
    let tail = slots.budget(|out| ChargedVec::new(Some(&execution), out))?;
    let head = slots.build(|out| {
        model.add_refinement(
            RefinementInput {
                target: boolean,
                rules,
                next: None,
                tail,
            },
            out,
        )
    })?;
    slots
        .build(|out| model.type_mut(boolean, out))?
        .set_refinement(head);
    freeze_error(&mut slots, model, root)?;
    let (mut model, base_ids) = base(&mut slots)?;
    let root = base_ids.root;
    let execution = slots.budget(|out| model.execution().try_clone(out))?;
    let fields = budget_items(&mut slots, &[FieldId::new(100)], &execution)?;
    slots
        .build(|out| model.type_mut(root, out))?
        .set_fields(fields);
    freeze_error(&mut slots, model, root)?;
    let (mut model, base_ids) = base(&mut slots)?;
    let root = base_ids.root;
    let execution = slots.budget(|out| model.execution().try_clone(out))?;
    let name = slots.budget(|out| ChargedString::copy("cycle", Some(&execution), out))?;
    let field_span = span(&mut slots)?;
    let id = slots.build(|out| model.add_field(FieldRecord::new(name, root, field_span), out))?;
    let fields = budget_items(&mut slots, &[id], &execution)?;
    slots
        .build(|out| model.type_mut(root, out))?
        .set_fields(fields);
    freeze_error(&mut slots, model, root)?;
    let (model, base_ids) = base(&mut slots)?;
    let root = base_ids.root;
    let execution = slots.budget(|out| model.execution().try_clone(out))?;
    let source_locations = locations(&mut slots)?;
    let source_locations = budget_items(&mut slots, &source_locations, &execution)?;
    check!(matches!(
        slots
            .build_errors
            .access(|out| model.freeze(root, &[] as &[&str; 0], source_locations, out)),
        Err(error) if error.is_internal() && error.context().is_some()
    ));
    let (model, base_ids) = base(&mut slots)?;
    let root = base_ids.root;
    let execution = slots.budget(|out| model.execution().try_clone(out))?;
    let empty_locations = slots.budget(|out| ChargedVec::new(Some(&execution), out))?;
    check!(matches!(
        slots
            .build_errors
            .access(|out| model.freeze(root, &[""], empty_locations, out)),
        Err(error) if error.is_internal() && error.context().is_some()
    ));
    Ok(())
}

#[test]
fn freeze_rejects_non_utf8_endpoints_and_out_of_source_ranges() -> TestResult<()> {
    let mut slots = TestSlots::new();
    for offset in [1, 3] {
        let (mut model, base_ids) = base(&mut slots)?;
        let root = base_ids.root;
        let execution = slots.budget(|out| model.execution().try_clone(out))?;
        let invalid_span = slots.span(|out| Span::new(offset, offset, out))?;
        let invalid = SourceSpan::new(SourceId::new(0), invalid_span);
        let old_shape = slots.build(|out| model.type_shape(root, out))?;
        let record = slots.budget(|out| {
            TypeRecord::new(
                TypeRecordOptions {
                    name: None,
                    qualified_name: None,
                    shape: old_shape,
                    child: None,
                    span: invalid,
                },
                &execution,
                out,
            )
        })?;
        slots.build(|out| model.set_type(root, record, out))?;
        let mut records = locations(&mut slots)?;
        records.push((
            invalid,
            records
                .first()
                .map(|record| record.1)
                .ok_or(TestError::MissingLocation)?,
        ));
        let locations = budget_items(&mut slots, &records, &execution)?;
        check!(matches!(
            slots
                .build_errors
                .access(|out| model.freeze(root, &["é"], locations, out)),
            Err(error) if error.is_internal() && error.context().is_some()
        ));
    }
    Ok(())
}

#[test]
fn freeze_rejects_duplicate_names_and_positive_signed_minimum_magnitude() -> TestResult<()> {
    let mut slots = TestSlots::new();
    let (mut model, base_ids) = base(&mut slots)?;
    let root = base_ids.root;
    let boolean = base_ids.boolean;
    let execution = slots.budget(|out| model.execution().try_clone(out))?;
    let first_name = slots.budget(|out| ChargedString::copy("same", Some(&execution), out))?;
    let first_span = span(&mut slots)?;
    let first = slots
        .build(|out| model.add_field(FieldRecord::new(first_name, boolean, first_span), out))?;
    let second_name = slots.budget(|out| ChargedString::copy("same", Some(&execution), out))?;
    let second_span = span(&mut slots)?;
    let second = slots
        .build(|out| model.add_field(FieldRecord::new(second_name, boolean, second_span), out))?;
    let fields = budget_items(&mut slots, &[first, second], &execution)?;
    slots
        .build(|out| model.type_mut(root, out))?
        .set_fields(fields);
    freeze_error(&mut slots, model, root)?;
    let (mut model, base_ids) = base(&mut slots)?;
    let root = base_ids.root;
    let boolean = base_ids.boolean;
    let signed = scalar(&mut slots, &mut model, BuiltinType::I128)?;
    let first_span = span(&mut slots)?;
    let second_span = span(&mut slots)?;
    let operator_span = span(&mut slots)?;
    let nodes = vec![
        TypedExpressionNode::new(
            TypedExpressionKind::Literal(TypedLiteral::Integer(1_u128 << 127)),
            signed,
            first_span,
        ),
        TypedExpressionNode::new(
            TypedExpressionKind::Literal(TypedLiteral::Integer(0)),
            signed,
            second_span,
        ),
        TypedExpressionNode::new(
            TypedExpressionKind::Binary {
                operator: BinaryOperator::Greater,
                left: ExpressionNodeId::new(0),
                right: ExpressionNodeId::new(1),
            },
            boolean,
            operator_span,
        ),
    ];
    assertion(
        &mut slots,
        AssertionRequest {
            model: &mut model,
            root,
            boolean,
            nodes,
            index: 2,
        },
    )?;
    freeze_error(&mut slots, model, root)?;
    Ok(())
}

fn budget_items<T: Copy>(
    slots: &mut TestSlots,
    items: &[T],
    execution: &ExecutionBudget,
) -> TestResult<ChargedVec<T>> {
    let mut result = slots.budget(|out| ChargedVec::new(Some(execution), out))?;
    slots.budget(|out| result.extend_from_slice(items, out))?;
    Ok(result)
}
