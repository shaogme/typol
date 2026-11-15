use super::scalar::ScalarValidator;
use crate::{
    context::{AssignmentContext, SemanticStatus},
    diagnostics::{
        DiagnosticBuffer, DiagnosticFactory, ExecutionDiagnostic, RuleFailureLocation,
        RuleFailureMessage,
    },
    secret::{SecretPathContext, SecretPolicy, SecretTypeContext},
    session::ValidationResources,
    validation::Validator,
};
use opeo::{ErrSlot, OResult, Out};
use typol_core::{
    BudgetError, Diagnostic, DiagnosticContext, DiagnosticMessage, LocatedSpan, ValueKind,
    ValueSource, ValueView, diagnostic_context,
};
use typol_i18n::{Translator, t};
use typol_semantic::{
    AllItemRequest, AllItemValue, Constraint, ConstraintData, EvalError, SchemaAccessError,
    SchemaEvaluator as Evaluator, SchemaRules, SumBoundsRequest, TypeKind, TypeView,
};

static COLLECTION_SIZE_RULE_RESULT: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "collection-validation",
    "size-rule-result-missing",
);
static COLLECTION_VALUE_SHAPE: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "collection-validation",
    "collection-value-shape-mismatch",
);

#[derive(Clone, Copy)]
enum CollectionContents<'a> {
    Array(ValueView<'a>),
    Map(ValueView<'a>),
}

#[derive(Clone, Copy)]
struct CollectionView<'a> {
    contents: CollectionContents<'a>,
    item_type: TypeView<'a>,
}

impl<'a> CollectionView<'a> {
    fn array(self) -> Option<ValueView<'a>> {
        match self.contents {
            CollectionContents::Array(values) => Some(values),
            CollectionContents::Map(_) => None,
        }
    }

    fn item_source<'slot, 'borrow>(
        self,
        index: usize,
        span: Option<LocatedSpan>,
        translator: Translator,
        out: Out<'slot, 'borrow, Diagnostic>,
    ) -> OResult<'slot, ValueSource> {
        match match self.contents {
            CollectionContents::Array(value) | CollectionContents::Map(value) => {
                let entries = match value.as_ref() {
                    ValueKind::Array(entries) | ValueKind::Map(entries) => entries,
                    _ => return Err(out.fail(collection_invariant(span, translator))),
                };
                entries.get(index).map(|entry| entry.value().source())
            }
        } {
            Some(source) => Ok(source),
            None => Err(out.fail(collection_invariant(span, translator))),
        }
    }
}

#[derive(Clone, Copy)]
enum ItemLocation<'a> {
    Array { source: ValueSource },
    Map { key: &'a str, source: ValueSource },
}

struct ItemFailure<'a> {
    constraint: Constraint<'a>,
    result: Result<bool, EvalError>,
    message: DiagnosticMessage<'a>,
    location: Option<ItemLocation<'a>>,
}

struct ItemReportRequest<'work, 'schema, 'diagnostics> {
    metadata: &'work AssignmentContext<'schema>,
    collection: CollectionView<'schema>,
    index: usize,
    item: ItemFailure<'schema>,
    budget_errors: &'work mut ErrSlot<BudgetError>,
    schema_errors: &'work mut ErrSlot<SchemaAccessError>,
    diagnostics: &'work mut DiagnosticBuffer<'diagnostics>,
    reported: &'work mut bool,
}

fn report_item<'slot>(
    request: ItemReportRequest<'_, '_, '_>,
    mut out: Out<'slot, '_, Diagnostic>,
) -> OResult<'slot, bool> {
    let ItemReportRequest {
        metadata,
        collection,
        index,
        item,
        budget_errors,
        schema_errors,
        diagnostics,
        reported,
    } = request;
    let ItemFailure {
        constraint,
        result,
        message,
        location,
    } = item;
    if matches!(result, Ok(true)) {
        return Ok(true);
    }
    let mut path = ScalarValidator::copy_path(metadata, out.reborrow())?;
    let hidden_path = SecretPolicy::hides(
        metadata.schema,
        metadata.path,
        SecretPathContext {
            primary: metadata.span,
            value_source: metadata.source,
            translator: metadata.translator,
        },
        out.reborrow(),
    )?;
    let hidden_type = SecretPolicy::secret_type(
        collection.item_type,
        SecretTypeContext {
            budget: metadata.path.budget(),
            schema_errors,
            budget_errors,
            translator: metadata.translator,
        },
        out.reborrow(),
    )?;
    let hidden = hidden_path || hidden_type;
    if !hidden {
        match collection.contents {
            CollectionContents::Array(_) => {
                if matches!(location, Some(ItemLocation::Map { .. })) {
                    return Err(out.fail(collection_invariant(metadata.span, metadata.translator)));
                }
                Validator::budget_call(
                    budget_errors,
                    metadata.translator,
                    |budget_out| path.push_index(index, budget_out),
                    out.reborrow(),
                )?;
            }
            CollectionContents::Map(entries) => {
                let key = match location {
                    Some(ItemLocation::Map { key, .. }) => key,
                    Some(ItemLocation::Array { .. }) => {
                        return Err(
                            out.fail(collection_invariant(metadata.span, metadata.translator))
                        );
                    }
                    None => {
                        let Some(entry) = (match entries.as_ref() {
                            ValueKind::Map(entries) => entries.get(index),
                            _ => None,
                        }) else {
                            return Err(
                                out.fail(collection_invariant(metadata.span, metadata.translator))
                            );
                        };
                        let Some(key) = entry.name() else {
                            return Err(
                                out.fail(collection_invariant(metadata.span, metadata.translator))
                            );
                        };
                        key
                    }
                };
                Validator::budget_call(
                    budget_errors,
                    metadata.translator,
                    |budget_out| path.push_map_key(key, budget_out),
                    out.reborrow(),
                )?;
            }
        }
    }
    let node = metadata
        .node
        .and_then(|parent| metadata.locations.item(parent, index));
    let span = node.map_or(metadata.span, |node| node.span());
    let source = match location {
        Some(ItemLocation::Array { source }) | Some(ItemLocation::Map { source, .. }) => source,
        None => {
            collection.item_source(index, metadata.span, metadata.translator, out.reborrow())?
        }
    };
    let error = ExecutionDiagnostic::failure(
        result,
        RuleFailureMessage {
            message,
            redacted: hidden,
            translator: metadata.translator,
        },
        RuleFailureLocation {
            span,
            constraint: constraint.span(),
            path,
            source: metadata.span.map(|_| source),
        },
        out.reborrow(),
    )?;
    let Some(error) = error else {
        return Ok(true);
    };
    *reported = true;
    Ok(diagnostics.push(error))
}

pub(super) struct CollectionValidationRequest<'work, 'schema, 'diagnostics, 'limits> {
    pub(super) metadata: &'work AssignmentContext<'schema>,
    pub(super) value: ValueView<'work>,
    pub(super) constraints: &'work [Constraint<'schema>],
    pub(super) resources: &'work mut ValidationResources<'limits>,
    pub(super) diagnostics: &'work mut DiagnosticBuffer<'diagnostics>,
}

pub(crate) struct CollectionValidator;

impl CollectionValidator {
    pub(super) fn validate<'slot>(
        request: CollectionValidationRequest<'_, '_, '_, '_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, SemanticStatus> {
        let CollectionValidationRequest {
            metadata,
            value,
            constraints,
            resources,
            diagnostics,
        } = request;
        if !matches!(value.as_ref(), ValueKind::Array(_) | ValueKind::Map(_)) {
            return Ok(SemanticStatus::Success);
        }
        let contents = match value.as_ref() {
            ValueKind::Array(_) => CollectionContents::Array(value),
            ValueKind::Map(_) => CollectionContents::Map(value),
            _ => return Err(out.fail(collection_invariant(metadata.span, metadata.translator))),
        };
        let kind = Validator::schema_call(
            &mut resources.schema_access_errors,
            resources.translator,
            |access_out| metadata.expected.kind(access_out),
            out.reborrow(),
        )?;
        let item_type = match (kind, contents) {
            (TypeKind::Array { element, .. }, CollectionContents::Array(_)) => element,
            (TypeKind::Map { value }, CollectionContents::Map(_)) => value,
            _ => return Err(out.fail(collection_invariant(metadata.span, metadata.translator))),
        };
        let collection = CollectionView {
            contents,
            item_type,
        };
        let mut reported = false;
        for &constraint in constraints {
            Validator::budget_call(
                &mut resources.budget_errors,
                resources.translator,
                |budget_out| resources.budget.consume(1, budget_out),
                out.reborrow(),
            )?;
            let data = Validator::schema_call(
                &mut resources.schema_access_errors,
                resources.translator,
                |access_out| constraint.data(access_out),
                out.reborrow(),
            )?;
            let result = match data {
                ConstraintData::SizeBounds { .. } => match resources
                    .eval_errors
                    .try_call(|eval_out| {
                        SchemaRules::scalar(value, data, Some(&resources.budget), eval_out)
                    })
                    .map_err(|caught| caught.take())
                {
                    Ok(Some(result)) => Ok(result),
                    Ok(None) => Err(EvalError::internal(COLLECTION_SIZE_RULE_RESULT)),
                    Err(error) => Err(error),
                },
                ConstraintData::SumBounds { min, max } => {
                    let Some(values) = collection.array() else {
                        return Err(
                            out.fail(collection_invariant(metadata.span, metadata.translator))
                        );
                    };
                    let evaluator = Evaluator::new(
                        metadata.schema,
                        resources.value_limits,
                        resources.regex_limits,
                        Some(&resources.budget),
                    );
                    resources
                        .eval_errors
                        .try_call(|eval_out| {
                            SchemaRules::sum_bounds(
                                &evaluator,
                                SumBoundsRequest {
                                    element: item_type,
                                    values,
                                    min,
                                    max,
                                },
                                eval_out,
                            )
                        })
                        .map_err(|caught| caught.take())
                }
                ConstraintData::Unique { by } => {
                    let Some(values) = collection.array() else {
                        return Err(
                            out.fail(collection_invariant(metadata.span, metadata.translator))
                        );
                    };
                    let evaluator = Evaluator::new(
                        metadata.schema,
                        resources.value_limits,
                        resources.regex_limits,
                        Some(&resources.budget),
                    );
                    let duplicates = resources
                        .eval_errors
                        .try_call(|eval_out| {
                            SchemaRules::unique(&evaluator, values, item_type, by, eval_out)
                        })
                        .map_err(|caught| caught.take());
                    match duplicates {
                        Ok(duplicates) => {
                            for (index, duplicate) in duplicates.into_iter().enumerate() {
                                if duplicate
                                    && !(report_item(
                                        ItemReportRequest {
                                            metadata,
                                            collection,
                                            index,
                                            item: ItemFailure {
                                                constraint,
                                                result: Ok(false),
                                                message: DiagnosticMessage::from(
                                                    t! { name: "diagnostic.validation.duplicate_array_elements", locale: metadata.translator },
                                                ),
                                                location: None,
                                            },
                                            budget_errors: &mut resources.budget_errors,
                                            schema_errors: &mut resources.schema_access_errors,
                                            diagnostics,
                                            reported: &mut reported,
                                        },
                                        out.reborrow(),
                                    ))?
                                {
                                    return Ok(SemanticStatus::Stopped);
                                }
                            }
                        }
                        Err(error) => {
                            let path = ScalarValidator::copy_path(metadata, out.reborrow())?;
                            let error = ExecutionDiagnostic::failure(
                                Err(error),
                                RuleFailureMessage {
                                    message: DiagnosticMessage::custom(
                                        "array uniqueness validation failed",
                                    ),
                                    redacted: false,
                                    translator: metadata.translator,
                                },
                                RuleFailureLocation {
                                    span: metadata.span,
                                    constraint: constraint.span(),
                                    path,
                                    source: metadata.source,
                                },
                                out.reborrow(),
                            )?;
                            if let Some(error) = error {
                                reported = true;
                                if !diagnostics.push(error) {
                                    return Ok(SemanticStatus::Stopped);
                                }
                            }
                        }
                    }
                    continue;
                }
                ConstraintData::All { .. } => {
                    let ConstraintData::All {
                        expression,
                        message,
                    } = data
                    else {
                        continue;
                    };
                    let evaluator = Evaluator::new(
                        metadata.schema,
                        resources.value_limits,
                        resources.regex_limits,
                        Some(&resources.budget),
                    );
                    let entries = match value.as_ref() {
                        ValueKind::Array(entries) | ValueKind::Map(entries) => entries,
                        _ => {
                            return Err(
                                out.fail(collection_invariant(metadata.span, metadata.translator))
                            );
                        }
                    };
                    for (index, entry) in entries.enumerate() {
                        let item_value = entry.value();
                        let (item, location) = match (collection.contents, entry.name()) {
                            (CollectionContents::Array(_), None) => (
                                AllItemValue::ArrayItem(item_value),
                                ItemLocation::Array {
                                    source: item_value.source(),
                                },
                            ),
                            (CollectionContents::Map(_), Some(key)) => (
                                AllItemValue::MapEntry {
                                    key,
                                    value: item_value,
                                },
                                ItemLocation::Map {
                                    key,
                                    source: item_value.source(),
                                },
                            ),
                            _ => {
                                return Err(out.fail(collection_invariant(
                                    metadata.span,
                                    metadata.translator,
                                )));
                            }
                        };
                        let result = resources
                            .eval_errors
                            .try_call(|eval_out| {
                                SchemaRules::all_item(
                                    &evaluator,
                                    AllItemRequest {
                                        expression,
                                        element: item_type,
                                        item,
                                    },
                                    eval_out,
                                )
                            })
                            .map_err(|caught| caught.take());
                        if !report_item(
                            ItemReportRequest {
                                metadata,
                                collection,
                                index,
                                item: ItemFailure {
                                    constraint,
                                    result,
                                    message: DiagnosticMessage::custom(message),
                                    location: Some(location),
                                },
                                budget_errors: &mut resources.budget_errors,
                                schema_errors: &mut resources.schema_access_errors,
                                diagnostics,
                                reported: &mut reported,
                            },
                            out.reborrow(),
                        )? {
                            return Ok(SemanticStatus::Stopped);
                        }
                    }
                    continue;
                }
                _ => continue,
            };
            if !matches!(result, Ok(true)) {
                let path = ScalarValidator::copy_path(metadata, out.reborrow())?;
                if let Some(error) = ExecutionDiagnostic::failure(
                    result,
                    RuleFailureMessage {
                        message: DiagnosticMessage::from(
                            t! { name: "diagnostic.validation.collection_boundary_failed", locale: metadata.translator },
                        ),
                        redacted: false,
                        translator: metadata.translator,
                    },
                    RuleFailureLocation {
                        span: metadata.span,
                        constraint: constraint.span(),
                        path,
                        source: metadata.source,
                    },
                    out.reborrow(),
                )? {
                    reported = true;
                    if !diagnostics.push(error) {
                        return Ok(SemanticStatus::Stopped);
                    }
                }
            }
        }
        Ok(ExecutionDiagnostic::status(reported))
    }
}

fn collection_invariant(span: Option<LocatedSpan>, translator: Translator) -> Diagnostic {
    DiagnosticFactory::internal(COLLECTION_VALUE_SHAPE, span, None, None, translator)
}
