use crate::{
    instance::{LocationIndex, NodeId},
    validation::ValidatedFallback,
};
use opeo::{OResult, Out};
use typol_core::{ConfigValue, InstancePath, LocatedSpan, ValueSource};
use typol_semantic::{
    CompiledSchema, ConstraintList, FieldView, SchemaAccessError, TypeView, VariantView,
};

/// Borrows the current value location; validators use the handle to read constraints, secret markers, and expression IR.
pub(crate) struct AssignmentContext<'a> {
    pub(crate) schema: &'a CompiledSchema,
    /// A prevalidated fallback has no concrete instance node; its location is recorded only after adoption.
    pub(crate) node: Option<NodeId>,
    pub(crate) locations: &'a LocationIndex<'a>,
    pub(crate) inherited_constraints: &'a [ConstraintList<'a>],
    pub(crate) expected: TypeView<'a>,
    pub(crate) field: Option<FieldView<'a>>,
    pub(crate) variant: Option<VariantView<'a>>,
    pub(crate) span: Option<LocatedSpan>,
    pub(crate) path: &'a InstancePath,
    pub(crate) source: Option<ValueSource>,
    pub(crate) shape_validated: bool,
}

#[derive(Clone, Copy)]
pub(crate) enum SemanticStatus {
    Success,
    Reported,
    Stopped,
}

pub(crate) struct RuleContext;

impl RuleContext {
    /// The constraints at the use site already include inherited rules, so the type-level list must not be chained again.
    pub(crate) fn constraints<'slot, 'a>(
        expected: TypeView<'a>,
        field: Option<FieldView<'a>>,
        variant: Option<VariantView<'a>>,
        mut out: Out<'slot, '_, SchemaAccessError>,
    ) -> OResult<'slot, ConstraintList<'a>> {
        let constraints = if let Some(field) = field {
            field.constraints(out.reborrow())?
        } else if let Some(variant) = variant {
            variant.constraints(out.reborrow())?
        } else {
            expected.constraints(out.reborrow())?
        };
        Ok(constraints)
    }
}

/// Completion state is independent of origin: a prevalidated value still needs a concrete instance recorded, but its rules must not run again.
pub(crate) enum CompletionState {
    ChildrenComplete,
    NeedsTraversal,
    NeedsTraversalWithKnownShape,
    Validated(ValidatedFallback),
}

pub(crate) struct EnvironmentSpans {
    pub(crate) call: LocatedSpan,
    pub(crate) name: LocatedSpan,
}

pub(crate) enum ValueOrigin {
    Environment { call: LocatedSpan },
    ExplicitFallback { value: LocatedSpan },
}

impl ValueOrigin {
    pub(crate) fn span(&self) -> LocatedSpan {
        match self {
            Self::Environment { call } => *call,
            Self::ExplicitFallback { value } => *value,
        }
    }
}

pub(crate) struct FallbackValue {
    pub(crate) value: ConfigValue,
    pub(crate) proof: ValidatedFallback,
    pub(crate) span: LocatedSpan,
}

pub(crate) struct EnvironmentValue {
    pub(crate) value: ConfigValue,
    pub(crate) origin: ValueOrigin,
    pub(crate) completion: CompletionState,
}

#[derive(Clone, Copy)]
pub(crate) struct AssignmentTarget<'a> {
    pub(crate) expected: TypeView<'a>,
    pub(crate) field: Option<FieldView<'a>>,
    pub(crate) variant: Option<VariantView<'a>>,
    pub(crate) span: LocatedSpan,
}
