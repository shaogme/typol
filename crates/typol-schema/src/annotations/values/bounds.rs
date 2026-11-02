use crate::{
    annotations::{
        TypedNumber,
        values::{AnnotationError, AnnotationValues},
    },
    context::CompileContext,
};
use opeo::{OResult, Out};
use typol_core::DiagnosticMessage;
use typol_i18n::diagnostic_message;
use typol_syntax::{BuiltinType, NamedArgument, NamedBounds, NumberLiteral, SizeArguments};

use super::summary::{BoundKind, BoundValue};

struct NamedArguments<'a> {
    first: Option<NamedArgument<'a, NumberLiteral<'a>>>,
    second: Option<NamedArgument<'a, NumberLiteral<'a>>>,
}

pub(in crate::annotations) struct AnnotationBounds;

impl AnnotationBounds {
    pub(in crate::annotations) fn parse_collection_bounds<'slot>(
        bounds: SizeArguments<'_>,
        context: &CompileContext,
        mut out: Out<'slot, '_, AnnotationError>,
    ) -> OResult<'slot, (Option<u64>, Option<u64>)> {
        let (min, max) = match bounds {
            SizeArguments::Positional { min, max } => {
                let min = AnnotationValues::parse_nonnegative_u64(min, context, out.reborrow())?;
                let Some(min) = min else {
                    return Err(out.fail(AnnotationError::Invalid(
                        diagnostic_message!("diagnostic.schema.annotation_size_bounds_invalid", "collection bounds must be non-negative decimal integers within the u64 range"),
                    )));
                };
                let max = AnnotationValues::parse_nonnegative_u64(max, context, out.reborrow())?;
                let Some(max) = max else {
                    return Err(out.fail(AnnotationError::Invalid(
                        diagnostic_message!("diagnostic.schema.annotation_size_bounds_invalid", "collection bounds must be non-negative decimal integers within the u64 range"),
                    )));
                };
                (Some(min), Some(max))
            }
            SizeArguments::Named(bounds) => Self::parse_named_bounds(
                bounds,
                |number, mut parse_out| {
                    match AnnotationValues::parse_nonnegative_u64(
                    number,
                    context,
                    parse_out.reborrow(),
                )? {
                    Some(value) => Ok(value),
                    None => Err(parse_out.fail(AnnotationError::Invalid(
                        diagnostic_message!("diagnostic.schema.annotation_size_bounds_invalid", "collection bounds must be non-negative decimal integers within the u64 range"),
                    ))),
                }
                },
                diagnostic_message!(
                    "diagnostic.schema.annotation_size_bound_repeated",
                    "collection bound cannot be specified more than once"
                ),
                context,
                out.reborrow(),
            )?,
        };
        if matches!((min, max), (Some(min), Some(max)) if min > max) {
            return Err(out.fail(AnnotationError::Invalid(diagnostic_message!(
                "diagnostic.schema.annotation_size_bounds_reversed",
                "collection minimum cannot exceed the maximum"
            ))));
        }
        Ok((min, max))
    }

    pub(in crate::annotations) fn parse_sum_bounds<'slot>(
        bounds: NamedBounds<'_>,
        builtin: BuiltinType,
        context: &CompileContext,
        mut out: Out<'slot, '_, AnnotationError>,
    ) -> OResult<'slot, (Option<TypedNumber>, Option<TypedNumber>)> {
        Self::parse_named_bounds(
            bounds,
            |number, mut parse_out| match AnnotationValues::parse_typed_number(
                number,
                builtin,
                context,
                parse_out.reborrow(),
            )? {
                Some(value) => Ok(value),
                None => Err(parse_out.fail(AnnotationError::Invalid(diagnostic_message!(
                    "diagnostic.schema.annotation_sum_bounds_incompatible",
                    "@sum bounds are incompatible with the array element type"
                )))),
            },
            diagnostic_message!(
                "diagnostic.schema.annotation_sum_bound_repeated",
                "@sum bound cannot be specified more than once"
            ),
            context,
            out.reborrow(),
        )
    }

    fn parse_named_bounds<'slot, 'a, T>(
        bounds: NamedBounds<'a>,
        mut parse: impl for<'call> FnMut(
            NumberLiteral<'a>,
            Out<'slot, 'call, AnnotationError>,
        ) -> OResult<'slot, T>,
        duplicate_message: DiagnosticMessage<'static>,
        context: &CompileContext,
        mut out: Out<'slot, '_, AnnotationError>,
    ) -> OResult<'slot, (Option<T>, Option<T>)> {
        let mut min: Option<BoundValue<T>> = None;
        let mut max: Option<BoundValue<T>> = None;
        let arguments = Self::named_arguments(bounds, context, out.reborrow())?;
        for argument in [arguments.first, arguments.second].into_iter().flatten() {
            let bound = parse(argument.value(), out.reborrow())?;
            context.execution().consume(1, out.reborrow())?;
            let name = context.ast_access(
                out.reborrow(),
                |ast_out| argument.name().text(ast_out),
                AnnotationError::from,
            )?;
            let is_min = context.compare_text(name, "min", out.reborrow())?;
            let kind = if is_min.is_eq() {
                BoundKind::Min
            } else {
                let is_max = context.compare_text(name, "max", out.reborrow())?;
                if is_max.is_eq() {
                    BoundKind::Max
                } else {
                    return Err(out.fail(AnnotationError::Invalid(diagnostic_message!(
                        "diagnostic.syntax.boundary_argument",
                        "bound name must be min or max"
                    ))));
                }
            };
            match kind {
                BoundKind::Min if min.is_none() => {
                    min = Some(BoundValue {
                        value: bound,
                        span: argument.span(),
                    });
                }
                BoundKind::Max if max.is_none() => {
                    max = Some(BoundValue {
                        value: bound,
                        span: argument.span(),
                    });
                }
                BoundKind::Min | BoundKind::Max => {
                    let earlier_span = match kind {
                        BoundKind::Min => min.as_ref().map(|bound| bound.span),
                        BoundKind::Max => max.as_ref().map(|bound| bound.span),
                    };
                    let Some(earlier_span) = earlier_span else {
                        return Err(out.fail(AnnotationError::internal(schema_context!(
                            "annotation-values",
                            "duplicate-bound-source-missing"
                        ))));
                    };
                    return Err(out.fail(AnnotationError::DuplicateBound {
                        message: duplicate_message,
                        span: argument.span(),
                        earlier_span,
                    }));
                }
            }
        }
        Ok((min.map(|bound| bound.value), max.map(|bound| bound.value)))
    }

    fn named_arguments<'slot, 'a>(
        bounds: NamedBounds<'a>,
        context: &CompileContext,
        mut out: Out<'slot, '_, AnnotationError>,
    ) -> OResult<'slot, NamedArguments<'a>> {
        context.ast_access(
            out.reborrow(),
            |mut ast_out| {
                let first_result = {
                    let mut arguments = bounds.iter(ast_out.reborrow());
                    arguments.next()
                };
                let first = match first_result {
                    Some(argument) => Some(argument?),
                    None => None,
                };
                let (skipped, second_result) = {
                    let mut arguments = bounds.iter(ast_out.reborrow());
                    let skipped = arguments.next();
                    (skipped, arguments.next())
                };
                if let Some(skipped) = skipped {
                    skipped?;
                }
                let second = match second_result {
                    Some(argument) => Some(argument?),
                    None => None,
                };
                Ok(NamedArguments { first, second })
            },
            AnnotationError::from,
        )
    }
}
