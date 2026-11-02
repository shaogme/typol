use super::{
    context::{ParserContext, ParserControl},
    literals::{ContextDiagnosticRequest, LiteralParser},
};
use crate::{
    ConfigLimits,
    diagnostics::{BudgetDiagnostic, DiagnosticFactory},
};
use opeo::{ErrSlot, OResult, Out};
use typol_core::{
    BudgetError, ByteSource, ChargedVec, Diagnostic, DiagnosticCode, DiagnosticContext,
    ExecutionBudget, LocatedSpan, SourceId, SourceSpan, diagnostic_context,
};
use typol_i18n::diagnostic_key;
use typol_semantic::SchemaAccessError;
use typol_syntax::{InputConfig, LexEvent, StreamLimits, Symbol, Token, TokenCursor, TokenKind};

static PARSER_SCHEMA_ACCESS: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "parser-schema-access",
    "compiled-schema-view-unavailable",
);
static PARSER_RECOVERY_HALTED: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "parser-recovery",
    "lexer-halted-before-recovery",
);
static PARSER_RECOVERY_INVALID_EVENT: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "parser-recovery",
    "unexpected-lexer-event-state",
);
static PARSER_RECOVERY_DEFERRED_FIELD: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "parser-recovery",
    "deferred-field-outside-assignment",
);

pub(super) struct ConfigToken<'token> {
    token: &'token Token,
}

impl<'token> ConfigToken<'token> {
    pub(super) fn kind(&self) -> TokenKind {
        self.token.kind()
    }

    pub(super) fn span(&self) -> LocatedSpan {
        self.token.located_span()
    }

    pub(super) fn text(&self) -> Option<&'token str> {
        self.token.text()
    }

    pub(super) fn decoded_string(&self) -> Option<&'token str> {
        self.token.decoded_string()
    }
}

pub(super) enum ParseFailure {
    Diagnostic(Diagnostic),
    Reported,
    Stopped,
    DeferredField,
}

impl From<Diagnostic> for ParseFailure {
    fn from(diagnostic: Diagnostic) -> Self {
        Self::Diagnostic(diagnostic)
    }
}

impl From<BudgetError> for ParseFailure {
    fn from(error: BudgetError) -> Self {
        Self::Diagnostic(BudgetDiagnostic::from_error(error))
    }
}

impl From<SchemaAccessError> for ParseFailure {
    fn from(error: SchemaAccessError) -> Self {
        Self::Diagnostic(DiagnosticFactory::internal(
            PARSER_SCHEMA_ACCESS.with_object_id(error.index() as u64),
            None,
            None,
            None,
        ))
    }
}

/// Records only consumed delimiters; it does not retain tokens or failed-item values.
pub(super) struct RecoveryCursor<'source> {
    cursor: TokenCursor<'source>,
    delimiters: ChargedVec<Symbol>,
    trusted: bool,
    stopped: bool,
    depth_limit: usize,
    consumed_end: usize,
    budget_errors: ErrSlot<BudgetError>,
    diagnostic_errors: ErrSlot<Diagnostic>,
}

pub(super) struct RecoveryCursorRequest<'source, 'limits, 'budget> {
    pub(super) source_id: SourceId,
    pub(super) source: &'source mut dyn ByteSource<Diagnostic>,
    pub(super) limits: &'limits ConfigLimits,
    pub(super) stream: StreamLimits,
    pub(super) budget: &'budget ExecutionBudget,
}

enum RecoveryEvent {
    Token,
    Diagnostic,
    Halted(SourceSpan),
}

impl<'source> RecoveryCursor<'source> {
    pub(super) fn new<'slot>(
        request: RecoveryCursorRequest<'source, '_, '_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Self> {
        let RecoveryCursorRequest {
            source_id,
            source,
            limits,
            stream,
            budget,
        } = request;
        let input_limits = limits.input_limits();
        let mut budget_errors = ErrSlot::new();
        let delimiters =
            match budget_errors.try_call(|budget_out| ChargedVec::new(Some(budget), budget_out)) {
                Ok(delimiters) => delimiters,
                Err(caught) => {
                    return Err(out.fail(BudgetDiagnostic::from_error(caught.take())));
                }
            };
        Ok(Self {
            cursor: TokenCursor::new(
                InputConfig {
                    source_id,
                    source,
                    resources: &input_limits,
                    stream,
                    budget,
                },
                out.reborrow(),
            )?,
            delimiters,
            trusted: true,
            stopped: false,
            depth_limit: limits.max_nesting_depth().saturating_add(1),
            consumed_end: 0,
            budget_errors,
            diagnostic_errors: ErrSlot::new(),
        })
    }

    pub(super) fn peek<'slot, 'borrow>(
        &mut self,
        out: Out<'slot, 'borrow, ParseFailure>,
    ) -> OResult<'slot, ConfigToken<'_>> {
        let current = match self
            .diagnostic_errors
            .try_call(|cursor_out| self.cursor.peek(cursor_out))
        {
            Ok(current) => current,
            Err(caught) => {
                return Err(out.fail(ParseFailure::from(caught.take())));
            }
        };
        let event = match current {
            LexEvent::Token(_) | LexEvent::Eof(_) => RecoveryEvent::Token,
            LexEvent::Error(_) | LexEvent::Fatal(_) => RecoveryEvent::Diagnostic,
            LexEvent::Halted(span) => RecoveryEvent::Halted(*span),
        };
        match event {
            RecoveryEvent::Token => {
                let current = match self
                    .diagnostic_errors
                    .try_call(|cursor_out| self.cursor.peek(cursor_out))
                {
                    Ok(current) => current,
                    Err(caught) => {
                        return Err(out.fail(ParseFailure::from(caught.take())));
                    }
                };
                match current.token() {
                    Some(token) => Ok(ConfigToken { token }),
                    None => Err(out.fail(ParseFailure::from(Self::invalid_event_diagnostic()))),
                }
            }
            RecoveryEvent::Diagnostic => {
                self.stopped = true;
                match self
                    .diagnostic_errors
                    .try_call(|cursor_out| self.cursor.bump(cursor_out))
                {
                    Ok(LexEvent::Error(diagnostic) | LexEvent::Fatal(diagnostic)) => {
                        Err(out.fail(ParseFailure::from(diagnostic)))
                    }
                    Err(caught) => Err(out.fail(ParseFailure::from(caught.take()))),
                    Ok(LexEvent::Token(_) | LexEvent::Eof(_) | LexEvent::Halted(_)) => {
                        Err(out.fail(ParseFailure::from(Self::invalid_event_diagnostic())))
                    }
                }
            }
            RecoveryEvent::Halted(span) => {
                self.stopped = true;
                Err(out.fail(ParseFailure::from(Self::halted_diagnostic(span))))
            }
        }
    }

    fn halted_diagnostic(span: SourceSpan) -> Diagnostic {
        DiagnosticFactory::internal(PARSER_RECOVERY_HALTED, Some(span.into()), None, None)
    }

    fn invalid_event_diagnostic() -> Diagnostic {
        DiagnosticFactory::internal(PARSER_RECOVERY_INVALID_EVENT, None, None, None)
    }

    pub(super) fn bump<'slot>(
        &mut self,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, ()> {
        let (kind, span) = {
            let token = self.peek(out.reborrow())?;
            (token.kind(), token.span())
        };
        if let TokenKind::Symbol(symbol) = kind {
            if let Some(close) = closing_symbol(symbol) {
                if self.delimiters.len() >= self.depth_limit {
                    self.stopped = true;
                    return Err(
                        out.fail(ParseFailure::from(DiagnosticFactory::make_diagnostic(
                            DiagnosticCode::RESOURCE_LIMIT_EXCEEDED,
                            "configuration delimiter nesting depth exceeded the limit",
                            Some(span),
                            None,
                            None,
                        ))),
                    );
                }
                if let Err(caught) = self
                    .budget_errors
                    .try_call(|budget_out| self.delimiters.try_push(close, budget_out))
                {
                    self.stopped = true;
                    return Err(out.fail(ParseFailure::from(BudgetDiagnostic::from_error(
                        caught.take(),
                    ))));
                }
            } else if is_closing(symbol) && self.delimiters.pop() != Some(symbol) {
                self.trusted = false;
            }
        }
        match self
            .diagnostic_errors
            .try_call(|cursor_out| self.cursor.bump(cursor_out))
        {
            Ok(LexEvent::Token(_) | LexEvent::Eof(_)) => {
                self.consumed_end = span.span().end();
            }
            Err(caught) => {
                return Err(out.fail(ParseFailure::from(caught.take())));
            }
            Ok(_) => {
                return Err(out.fail(ParseFailure::from(Self::invalid_event_diagnostic())));
            }
        }
        Ok(())
    }

    pub(super) fn consumed_end(&self) -> usize {
        self.consumed_end
    }

    pub(super) fn depth(&self) -> usize {
        self.delimiters.len()
    }
}

fn closing_symbol(symbol: Symbol) -> Option<Symbol> {
    match symbol {
        Symbol::LeftBrace => Some(Symbol::RightBrace),
        Symbol::LeftBracket => Some(Symbol::RightBracket),
        Symbol::LeftParen => Some(Symbol::RightParen),
        _ => None,
    }
}

fn is_closing(symbol: Symbol) -> bool {
    matches!(
        symbol,
        Symbol::RightBrace | Symbol::RightBracket | Symbol::RightParen
    )
}

pub(crate) struct Recovery;

impl Recovery {
    pub(super) fn record_failure(
        context: &mut ParserContext<'_, '_, '_>,
        failure: ParseFailure,
    ) -> bool {
        match failure {
            ParseFailure::Diagnostic(diagnostic) => {
                if let Some(span) = diagnostic.located_primary() {
                    context.session.pin(span.source_span(), span.location());
                    if span.source_id() == context.source_id
                        && diagnostic.code() != DiagnosticCode::CONFIG_VALIDATION_ERROR
                    {
                        context.session.begin_secret(span.span().start());
                    }
                } else {
                    context.session.invalidate_capture();
                }
                context.diagnostics.push(diagnostic) && !context.cursor.stopped
            }
            ParseFailure::DeferredField => {
                let primary = context
                    .current_node
                    .and_then(|node| context.locations.get(node))
                    .and_then(|node| node.span());
                context.diagnostics.push(DiagnosticFactory::internal(
                    PARSER_RECOVERY_DEFERRED_FIELD,
                    primary,
                    None,
                    None,
                ));
                false
            }
            ParseFailure::Stopped => false,
            ParseFailure::Reported => !context.diagnostics.stopped() && !context.cursor.stopped,
        }
    }

    /// The current container must remain on the stack; mismatched closing delimiters and EOF cannot be synchronization points.
    pub(super) fn recover_item<'slot>(
        context: &mut ParserContext<'_, '_, '_>,
        depth: usize,
        close: Symbol,
        failure: ParseFailure,
        mut out: Out<'slot, '_, ParseFailure>,
    ) -> OResult<'slot, ()> {
        if !Recovery::record_failure(context, failure) {
            return Err(out.fail(ParseFailure::Reported));
        }
        loop {
            if !context.cursor.trusted || context.cursor.depth() < depth {
                return Err(out.fail(ParseFailure::Reported));
            }
            let (kind, span) = {
                let token = context.cursor.peek(out.reborrow())?;
                (token.kind(), token.span())
            };
            if context.cursor.depth() == depth {
                if kind == TokenKind::Symbol(close) {
                    break;
                }
                if kind == TokenKind::Symbol(Symbol::Comma) {
                    context.cursor.bump(out.reborrow())?;
                    break;
                }
            }
            if kind == TokenKind::Eof
                || matches!(kind, TokenKind::Symbol(symbol) if is_closing(symbol)
                && context.cursor.delimiters.last() != Some(&symbol))
            {
                context.cursor.trusted = false;
                let diagnostic = ParserControl::diagnostic_call(
                    &context.diagnostic_errors,
                    |diagnostic_out| {
                        LiteralParser::context_diagnostic(
                            ContextDiagnosticRequest {
                                context,
                                code: DiagnosticCode::CONFIG_SYNTAX_ERROR,
                                message: "container is unclosed or its closing delimiter does not match",
                                key: Some(diagnostic_key!(
                                    "diagnostic.syntax.mismatched_closing_delimiter"
                                )),
                                span,
                                value_source: None,
                            },
                            diagnostic_out,
                        )
                    },
                    out.reborrow(),
                )?;
                let already_reported =
                    context
                        .diagnostics
                        .iter()
                        .next_back()
                        .is_some_and(|previous| {
                            previous.code() == DiagnosticCode::CONFIG_SYNTAX_ERROR
                                && previous.primary() == Some(span.source_span())
                        });
                if !already_reported {
                    context.diagnostics.push(diagnostic);
                }
                return Err(out.fail(ParseFailure::Reported));
            }
            context.cursor.bump(out.reborrow())?;
        }
        Ok(())
    }
}
