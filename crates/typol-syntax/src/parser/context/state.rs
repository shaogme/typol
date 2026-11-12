use crate::{
    Symbol, TokenCursor, TokenKind,
    error::{DiagnosticSink, SyntaxDiagnostic},
    storage::Storage,
};
use opeo::{OResult, Out};
use typol_core::{
    ChargedBox, ChargedVec, Diagnostic, DiagnosticCode, ExecutionBudget, SourceId, SourceSpan,
    diagnostic_context,
};

pub(crate) struct DelimiterFrame {
    pub(crate) close: Symbol,
    pub(crate) open_span: SourceSpan,
}

/// Syntax frames and recovery loops are metered separately; completed expression height uses no syntax frame.
pub(crate) struct SyntaxBudget {
    frame_limit: usize,
    frames: usize,
}

pub(crate) struct ParserState<'src> {
    pub(crate) cursor: TokenCursor<'src>,
    pub(crate) ast: ChargedBox<Storage>,
    pub(crate) source_id: SourceId,
    pub(crate) source_limit: usize,
    pub(crate) actual_source_len: Option<usize>,
    pub(crate) diagnostics: DiagnosticSink,
    pub(crate) delimiters: ChargedVec<DelimiterFrame>,
    pub(crate) trusted: bool,
    pub(crate) prefix_frames: usize,
    pub(crate) budget: SyntaxBudget,
    pub(crate) last_end: usize,
    pub(crate) stopped: bool,
    pub(crate) execution: ExecutionBudget,
}

#[derive(Clone, Copy)]
pub(crate) struct TokenInfo {
    pub(crate) kind: TokenKind,
    pub(crate) span: SourceSpan,
}

impl SyntaxBudget {
    pub(crate) const fn new(frame_limit: usize) -> Self {
        Self {
            frame_limit,
            frames: 0,
        }
    }

    pub(crate) fn enter<'slot>(
        &mut self,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ()> {
        if self.frames >= self.frame_limit {
            return Err(out.reborrow().fail(SyntaxDiagnostic::emergency(
                DiagnosticCode::RESOURCE_LIMIT_EXCEEDED,
                span,
            )));
        }
        self.frames += 1;
        Ok(())
    }

    pub(crate) fn leave<'slot>(
        &mut self,
        count: usize,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ()> {
        let Some(frames) = self.frames.checked_sub(count) else {
            return Err(out.reborrow().fail(SyntaxDiagnostic::internal(
                diagnostic_context!("typol-syntax", "parser-budget", "frame-depth-underflow"),
                span,
            )));
        };
        self.frames = frames;
        Ok(())
    }
}
