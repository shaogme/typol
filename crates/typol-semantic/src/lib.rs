#![no_std]
extern crate alloc;

mod constraints;
mod engine;
mod execution;
mod expressions;
mod indexes;
mod model;
mod regex;
mod views;

mod text;

#[cfg(feature = "compiler")]
mod builder;
#[cfg(feature = "compiler")]
mod validation;

use expressions::{StoredBindingInfo, StoredExpressionArgument, StoredExpressionKind};

pub use opeo;

pub use {
    constraints::{
        Bounds, Constraint, ConstraintId, ConstraintKind, ConstraintLayer, ConstraintList,
        ConstraintRecord, EffectiveRules, NumberValue, RefinementId, RefinementSummary,
        RuleIdentity, RuleOwner, RuleOwnerError, StoredRuleView, VariantRestrictionMode,
    },
    engine::{
        AllItemRequest, AllItemValue, Bindings, ComparisonOptions, CopyError, CopyOptions,
        CopyRequest, EvalError, EvalFailure, EvalKind, EvalValue, Evaluator, ExecutionField,
        ExecutionModel, ExecutionType, ExecutionVariant, Numeric, RegexExecutionError,
        RegexExecutionFailure, RegexExecutor, RegexMatchOptions, SumBoundsRequest, ValueComparison,
        ValueCopy, ValueRules,
    },
    execution::{SchemaBindings, SchemaComparison, SchemaEvaluator, SchemaRules, SchemaValue},
    expressions::{
        AssertionInfo, ClosureLocalId, ExpressionBinding, ExpressionMethod, ExpressionNodeId,
        ProgramId, TypedExpression, TypedExpressionKind, TypedExpressionNode, TypedLiteral,
        TypedMethodArgument,
    },
    model::{
        CompiledSchema, EnvironmentAuthorization, EnvironmentRecord, FieldId, FieldRecord,
        ModelCounts, SchemaAccessError, SchemaIdentity, ShapeId, ShapeKind, StoredConstraintData,
        StoredPayloadInfo, StoredTypeKind, StoredVariantPayload, TypeId, TypeRecord,
        TypeRecordOptions, VariantId, VariantRecord,
    },
    regex::{CompiledRegexConstraint, CompiledRegexConstraints, RegexId},
    text::{TextChecks, TextValidation},
    views::*,
};

pub use typol_core::{RegexExecutionBudget, SpanLocation};

#[cfg(feature = "compiler")]
pub use builder::{
    BuildError, Builder, RefinementInput, SchemaDefaultValidation, SchemaFreezeInput, SourceLookup,
};
