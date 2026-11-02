use crate::{
    annotations::values::AnnotationValues,
    annotations::{CompiledConstraint, ConstraintData, TypedNumber},
    diagnostics::DiagnosticCollector,
};
use typol_core::{DiagnosticMessage, SourceSpan};
use typol_i18n::diagnostic_message;

pub(super) struct BoundValue<T> {
    pub(super) value: T,
    pub(super) span: SourceSpan,
}

pub(super) enum BoundKind {
    Min,
    Max,
}

#[derive(Default)]
struct NumericBounds {
    min: Option<TypedNumber>,
    max: Option<TypedNumber>,
    conflict: bool,
}

#[derive(Default)]
struct UnsignedBounds {
    min: Option<u64>,
    max: Option<u64>,
    conflict: bool,
}

#[derive(Default)]
pub(super) struct ConstraintSummary {
    numeric: NumericBounds,
    length: UnsignedBounds,
    size: UnsignedBounds,
    sum: NumericBounds,
    size_span: Option<SourceSpan>,
}

impl ConstraintSummary {
    pub(super) fn include(
        &mut self,
        constraint: &CompiledConstraint,
        diagnostics: &mut DiagnosticCollector,
    ) {
        match constraint.data() {
            ConstraintData::NumberBounds { min, max } => {
                self.numeric.include(
                    (*min, *max),
                    constraint.span(),
                    diagnostic_message!(
                        "diagnostic.schema.combined_numeric_constraints_unsatisfiable",
                        "combined numeric constraints have no satisfiable range"
                    ),
                    diagnostics,
                );
            }
            ConstraintData::LengthBounds { min, max } => self.length.include(
                (Some(*min), Some(*max)),
                constraint.span(),
                diagnostic_message!(
                    "diagnostic.schema.combined_length_constraints_unsatisfiable",
                    "combined @length constraints have no satisfiable range"
                ),
                diagnostics,
            ),
            ConstraintData::SizeBounds { min, max } => {
                self.size_span = Some(constraint.span());
                self.size.include(
                    (*min, *max),
                    constraint.span(),
                    diagnostic_message!(
                        "diagnostic.schema.combined_size_constraints_unsatisfiable",
                        "combined @size constraints have no satisfiable range"
                    ),
                    diagnostics,
                );
            }
            ConstraintData::SumBounds { min, max } => self.sum.include(
                (*min, *max),
                constraint.span(),
                diagnostic_message!(
                    "diagnostic.schema.combined_sum_constraints_unsatisfiable",
                    "combined @sum constraints have no satisfiable range"
                ),
                diagnostics,
            ),
            _ => {}
        }
    }

    pub(super) fn size_span(&self) -> Option<SourceSpan> {
        self.size_span
    }

    pub(super) fn size_bounds(&self) -> (Option<u64>, Option<u64>) {
        (self.size.min, self.size.max)
    }
}

impl NumericBounds {
    fn include(
        &mut self,
        bounds: (Option<TypedNumber>, Option<TypedNumber>),
        span: SourceSpan,
        message: DiagnosticMessage<'static>,
        diagnostics: &mut DiagnosticCollector,
    ) {
        let (min, max) = bounds;
        if let Some(min) = min {
            self.min = Some(match self.min {
                Some(current)
                    if AnnotationValues::compare_numbers(current, min)
                        .is_some_and(|order| order.is_gt()) =>
                {
                    current
                }
                _ => min,
            });
        }
        if let Some(max) = max {
            self.max = Some(match self.max {
                Some(current)
                    if AnnotationValues::compare_numbers(current, max)
                        .is_some_and(|order| order.is_lt()) =>
                {
                    current
                }
                _ => max,
            });
        }
        if !self.conflict
            && matches!((self.min, self.max), (Some(min), Some(max)) if AnnotationValues::compare_numbers(min, max).is_some_and(|order| order.is_gt()))
        {
            diagnostics.push(diagnostics.make_semantic(message, span));
            self.conflict = true;
        }
    }
}

impl UnsignedBounds {
    fn include(
        &mut self,
        bounds: (Option<u64>, Option<u64>),
        span: SourceSpan,
        message: DiagnosticMessage<'static>,
        diagnostics: &mut DiagnosticCollector,
    ) {
        let (min, max) = bounds;
        if let Some(min) = min {
            self.min = Some(self.min.map_or(min, |current| current.max(min)));
        }
        if let Some(max) = max {
            self.max = Some(self.max.map_or(max, |current| current.min(max)));
        }
        if !self.conflict && matches!((self.min, self.max), (Some(min), Some(max)) if min > max) {
            diagnostics.push(diagnostics.make_semantic(message, span));
            self.conflict = true;
        }
    }
}
