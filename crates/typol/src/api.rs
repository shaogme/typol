pub mod binding {
    pub use crate::{
        TypolError, TypolStage,
        binding_impl::{
            BindingErrors, BindingMismatch, BindingMismatchKind, BindingShape, TrustedTypedSchema,
            TypedSchema,
        },
        conversion::{
            ConversionContext, ConversionFields, ConversionPayload, ConversionValue, EnumInput,
            OrderedMap, Sourced,
        },
        descriptor::{
            BudgetedTypol, DescribeContext, DescriptorName, DescriptorNode, DescriptorProvider,
            MapContainer, TypeDescriptor, TypeShape, Typol, VariantShape,
        },
        diagnostics::{CodecErrors, CodecStage, ConversionErrorKind},
        loading::{BoundRead, BoundSchema, ConfigInput, LoadedSchema, TrustedBoundSchema},
        options::{
            BindOptions, BindingLimitError, BindingLimits, CodecOptions, ConversionLimitError,
            ConversionLimits, MAX_CONVERSION_DEPTH,
        },
    };
}

pub mod config {
    #[cfg(feature = "config-ir")]
    pub use typol_config::ConfigValueErrors;

    pub use typol_config::{
        ConfigFailureStage, ConfigLimits, ConfigParseErrors, ConfigParseRequest, ConfigParser,
        ConfigValidationOptions, ConfigValidationRequest, ConfigValueProcessor,
        EnvironmentReadContext, NormalizedConfigValue, StreamLimits, ValidatedConfigValue,
    };
}

pub mod core {
    pub use typol_core::{
        AllocationError, BudgetError, ByteSource, ChargedString, ChargedVec, ConfigValue,
        ConfigValueBuilder, Diagnostic, DiagnosticCode, DiagnosticContext as ErrorContext,
        ExecutionBudget, ExecutionLimits, LocatedSpan, NamedValue, NamedValueEntryView,
        SliceSource, SourceId, SourceReadError, SourceSpan, ValueEntries, ValueEntryView,
        ValueError, ValueId, ValueKind, ValuePayload, ValuePayloadView, ValueSource, ValueView,
    };

    pub use crate::loading::SourceNames;
}

pub mod model {
    pub use typol_model::{BinaryOperator, BuiltinType, UnaryOperator};
}

pub mod policy {
    pub use typol_policy::{RegexLimits, ResourceLimits, ValueLimits};
}

#[cfg(feature = "std")]
pub mod host {
    pub use crate::loading::{ConfigInput, ReaderLoadRequest};
    pub use typol_i18n::{Language, Translator};
    pub use typol_std::{
        DiagnosticContext, DiagnosticRenderError, DiagnosticRenderer, DiagnosticReport,
        DiagnosticReportStage, DiagnosticReportTitle, DisplayError, DisplayFailurePoint,
        DisplayOptions, FileConfigInput, FileConfigInputLimitParams, FileConfigInputLimits,
        FileSchemaCompilation, FileSchemaCompileErrors, FileSchemaCompileFailure,
        FileSchemaCompileOptions, FileSchemaCompiler, FileSchemaLimitParams, FileSchemaLimits,
        FileSchemaResolver, FileSchemaSource, HostError, HostErrorKind, HostFailureStep,
        HostOperation, ProcessEnvironment, ProcessEnvironmentLimitParams, ProcessEnvironmentLimits,
        ReadSource, RenderOptions, SafeValueFormatter, SourceNameEditOptions, SourceNameLimits,
    };
}

pub mod schema {
    pub use typol_schema::{
        ResolverError, SchemaCompileContext, SchemaCompileContextFailure, SchemaCompileErrors,
        SchemaCompileOptions, SchemaCompileStage, SchemaCompileUsage, SchemaCompiler, SchemaLimits,
        SchemaResolveRequest, SchemaResolver, SchemaSource, SourceLoadedRequest, SourceObserver,
        SourceObserverError,
    };
}

pub mod semantic {
    pub use typol_semantic::{
        AllItemRequest, AssertionInfo, AssertionView, Bindings, Bounds, ClosureLocalId,
        ComparisonOptions, CompiledRegexConstraint, CompiledRegexConstraints, CompiledSchema,
        Constraint, ConstraintId, ConstraintKind, ConstraintLayer, ConstraintList,
        ConstraintRecord, CopyError, CopyOptions, CopyRequest, EffectiveRules,
        EnvironmentAuthorization, EnvironmentRecord, EvalError, EvalFailure, EvalKind, EvalValue,
        Evaluator, ExecutionField, ExecutionModel, ExecutionType, ExecutionVariant,
        ExpressionBinding, ExpressionLiteral, ExpressionMethod, ExpressionMethodInfo,
        ExpressionNodeId, FieldId, FieldRecord, FieldView, ModelCounts, NumberValue, Numeric,
        ProgramId, RefinementId, RefinementSummary, RegexExecutionError, RegexExecutionFailure,
        RegexExecutor, RegexId, RegexMatchOptions, RuleIdentity, RuleOwner, RuleOwnerError,
        SchemaAccessError, SchemaBindings, SchemaComparison, SchemaEvaluator, SchemaRules,
        SchemaValue, ShapeId, ShapeKind, SpanLocation, StoredConstraintData, StoredPayloadInfo,
        StoredRuleView, StoredTypeKind, StoredVariantPayload, SumBoundsRequest, TextChecks,
        TextValidation, TypeId, TypeKind, TypeRecord, TypeRecordOptions, TypeView, TypedExpression,
        TypedExpressionKind, TypedExpressionNode, TypedLiteral, TypedMethodArgument,
        ValueComparison, ValueCopy, ValueRules, VariantId, VariantPayloadInfo, VariantRecord,
        VariantView,
    };
}
