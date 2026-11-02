use crate::{
    BuildError, Builder, ConstraintKind, ConstraintLayer, ConstraintRecord, EvalFailure,
    FieldRecord, NumberValue, RefinementInput, ShapeKind, SpanLocation, StoredConstraintData,
    TypeId, TypeRecord, TypeRecordOptions,
};
use alloc::{string::String, vec};
use core::fmt::{Debug, Formatter, Result as FmtResult};
use opeo::{OResult, Out};
use typol_core::{
    BudgetError, ChargedString, ChargedVec, ConfigValue, ConfigValueBuilder, ExecutionBudget,
    ExecutionLimits, SourceId, SourceLocation, SourceSpan, Span, SpanError, ValueError, ValueId,
    ValueSource,
};
use typol_model::BuiltinType;
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

fn span(slots: &mut TestSlots) -> TestResult<SourceSpan> {
    let span = slots.span(|out| Span::new(0, 0, out))?;
    Ok(SourceSpan::new(SourceId::new(0), span))
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

fn unsigned_pair_array_value(
    slots: &mut TestSlots,
    first: u128,
    second: u128,
) -> TestResult<ConfigValue> {
    config_value(slots, |builder, mut out| {
        let first = builder.add_unsigned(first, ValueSource::RustValue, out.reborrow())?;
        let second = builder.add_unsigned(second, ValueSource::RustValue, out.reborrow())?;
        builder.add_array(&[first, second], ValueSource::RustValue, out)
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

fn base(slots: &mut TestSlots) -> TestResult<(Builder, TypeId)> {
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
    Ok((model, root))
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

fn freeze_failure(slots: &mut TestSlots, model: Builder, root: TypeId) -> TestResult<BuildError> {
    let execution = slots.budget(|out| model.execution().try_clone(out))?;
    let source_location = SourceLocation::new(1, 1).ok_or(TestError::MissingLocation)?;
    let source_locations = vec![(
        span(slots)?,
        SpanLocation {
            start: source_location,
            end: source_location,
        },
    )];
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

#[test]
fn freeze_rejects_default_values_that_violate_inherited_rules() -> TestResult<()> {
    let mut slots = TestSlots::new();
    let (mut model, root) = base(&mut slots)?;
    let integer = scalar(&mut slots, &mut model, BuiltinType::U8)?;
    let execution = slots.budget(|out| model.execution().try_clone(out))?;
    let rule_span = span(&mut slots)?;
    let rule = slots.build(|out| {
        model.add_constraint(
            ConstraintRecord::new(
                ConstraintKind::Min,
                ConstraintLayer::Alias,
                rule_span,
                StoredConstraintData::NumberBounds {
                    min: Some(NumberValue::Unsigned(5)),
                    max: None,
                },
            ),
            out,
        )
    })?;
    let rules = budget_items(&mut slots, &[rule], &execution)?;
    let tail = slots.budget(|out| ChargedVec::new(Some(&execution), out))?;
    let refinement = slots.build(|out| {
        model.add_refinement(
            RefinementInput {
                target: integer,
                rules,
                next: None,
                tail,
            },
            out,
        )
    })?;
    slots
        .build(|out| model.type_mut(integer, out))?
        .set_refinement(refinement);
    let value = unsigned_value(&mut slots, 4)?;
    field(&mut slots, &mut model, root, integer, value)?;

    let error = freeze_failure(&mut slots, model, root)?;
    if error
        != (BuildError::DefaultConstraintAt {
            source_span: rule_span,
        })
        || error.is_internal()
        || error.source_span() != Some(rule_span)
    {
        return Err(TestError::Assertion(String::from(
            "default constraint failure did not preserve the rule source span",
        )));
    }
    Ok(())
}

#[test]
fn freeze_distinguishes_default_arithmetic_failures_from_constraint_failures() -> TestResult<()> {
    let mut slots = TestSlots::new();
    let (mut model, root) = base(&mut slots)?;
    let integer = scalar(&mut slots, &mut model, BuiltinType::U8)?;
    let execution = slots.budget(|out| model.execution().try_clone(out))?;
    let array_span = span(&mut slots)?;
    let array = slots.build(|out| model.reserve_type(array_span, out))?;
    let element = slots.build(|out| model.type_shape(integer, out))?;
    let shape = slots.build(|out| {
        model.intern_shape(
            ShapeKind::Array {
                element,
                length: Some(2),
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
    let rule_span = span(&mut slots)?;
    let rule = slots.build(|out| {
        model.add_constraint(
            ConstraintRecord::new(
                ConstraintKind::Sum,
                ConstraintLayer::Alias,
                rule_span,
                StoredConstraintData::SumBounds {
                    min: Some(NumberValue::Unsigned(0)),
                    max: None,
                },
            ),
            out,
        )
    })?;
    let rules = budget_items(&mut slots, &[rule], &execution)?;
    let tail = slots.budget(|out| ChargedVec::new(Some(&execution), out))?;
    let refinement = slots.build(|out| {
        model.add_refinement(
            RefinementInput {
                target: array,
                rules,
                next: None,
                tail,
            },
            out,
        )
    })?;
    slots
        .build(|out| model.type_mut(array, out))?
        .set_refinement(refinement);
    let value = unsigned_pair_array_value(&mut slots, u8::MAX.into(), 1)?;
    field(&mut slots, &mut model, root, array, value)?;

    let error = freeze_failure(&mut slots, model, root)?;
    if error
        != (BuildError::DefaultEvaluationAt {
            failure: EvalFailure::Arithmetic,
            source_span: rule_span,
        })
        || error.is_internal()
        || error.context().is_some()
        || error.source_span() != Some(rule_span)
    {
        return Err(TestError::Assertion(String::from(
            "default evaluation failure did not preserve the arithmetic error context",
        )));
    }
    let outer_span = slots.span(|out| Span::new(1, 2, out))?;
    let outer_span = SourceSpan::new(SourceId::new(0), outer_span);
    if error.with_source_span(outer_span).source_span() != Some(rule_span) {
        return Err(TestError::Assertion(String::from(
            "default error mapping replaced the rule source span",
        )));
    }
    Ok(())
}
