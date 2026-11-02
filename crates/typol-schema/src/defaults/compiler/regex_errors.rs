use typol_core::SourceSpan;
use typol_regex::{RegexMatchError, RegexScratchError};
use typol_semantic::{EvalError, RegexExecutionError, RegexExecutionFailure};

pub(super) struct RegexExecutionDiagnostics;

impl RegexExecutionDiagnostics {
    pub(super) fn map(error: RegexExecutionError, fallback_span: SourceSpan) -> EvalError {
        let source_span = error.source_span().unwrap_or(fallback_span);
        if error.is_internal() {
            let context = error.context().unwrap_or_else(|| {
                schema_context!("default-regex-evaluation", "regex-context-missing")
            });
            return EvalError::internal(context).with_source_span(source_span);
        }
        match error {
            RegexExecutionError::Scratch(error) => Self::scratch(error, source_span),
            RegexExecutionError::Match(error) => Self::matched(error, source_span),
            RegexExecutionError::Budget(error) => {
                EvalError::from(error).with_source_span(source_span)
            }
            RegexExecutionError::Internal | RegexExecutionError::InternalAt { .. } => {
                EvalError::internal(schema_context!(
                    "default-regex-evaluation",
                    "regex-internal-error"
                ))
                .with_source_span(source_span)
            }
            RegexExecutionError::Located { error, .. } => Self::failure(error, source_span),
        }
    }

    fn failure(error: RegexExecutionFailure, source_span: SourceSpan) -> EvalError {
        match error {
            RegexExecutionFailure::Scratch(error) => Self::scratch(error, source_span),
            RegexExecutionFailure::Match(error) => Self::matched(error, source_span),
            RegexExecutionFailure::Budget(error) => {
                EvalError::from(error).with_source_span(source_span)
            }
            RegexExecutionFailure::Internal(Some(context)) => {
                EvalError::internal(context).with_source_span(source_span)
            }
            RegexExecutionFailure::Internal(None) => EvalError::internal(schema_context!(
                "default-regex-evaluation",
                "regex-context-missing"
            ))
            .with_source_span(source_span),
        }
    }

    fn scratch(error: RegexScratchError, source_span: SourceSpan) -> EvalError {
        match error {
            RegexScratchError::ResourceLimitExceeded(_) => EvalError::Resource,
            RegexScratchError::AllocationFailed => EvalError::Allocation,
            RegexScratchError::CapacityExceeded => EvalError::Capacity,
            RegexScratchError::InternalInvariantFailure(_) => EvalError::internal(schema_context!(
                "default-regex-evaluation",
                "scratch-internal-error"
            ))
            .with_source_span(source_span),
        }
    }

    fn matched(error: RegexMatchError, source_span: SourceSpan) -> EvalError {
        match error {
            RegexMatchError::ResourceLimitExceeded(_) => EvalError::Resource,
            RegexMatchError::AllocationFailed => EvalError::Allocation,
            RegexMatchError::CapacityExceeded => EvalError::Capacity,
            RegexMatchError::InternalInvariantFailure(_) => EvalError::internal(schema_context!(
                "default-regex-evaluation",
                "match-internal-error"
            ))
            .with_source_span(source_span),
        }
    }
}
