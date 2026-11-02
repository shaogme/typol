use crate::{LexEvent, TokenKind, input::InputConfig, lexer::Lexer, parser::SourcePositionIndex};
use opeo::{ErrSlot, OResult, Out, ResultOutExt};
use typol_core::{Diagnostic, SourceSpan, diagnostic_context};

/// Scans lexical events on demand and caches at most the current event and one lookahead event.
pub struct TokenCursor<'src> {
    lexer: Lexer<'src>,
    error_slot: ErrSlot<Diagnostic>,
    current: Option<LexEvent>,
    next: Option<LexEvent>,
    halted: Option<SourceSpan>,
}

impl<'src> TokenCursor<'src> {
    pub(crate) fn into_positions(self, source_len: usize) -> SourcePositionIndex {
        self.lexer.into_positions(source_len)
    }

    pub fn new<'slot, 'config>(
        config: InputConfig<'src, 'config>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Self> {
        let lexer = Lexer::new(config, out.reborrow())?;
        Ok(Self {
            lexer,
            error_slot: ErrSlot::new(),
            current: None,
            next: None,
            halted: None,
        })
    }

    pub fn peek<'slot>(
        &mut self,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, &LexEvent> {
        self.ensure_current(out.reborrow())?;
        let event = self
            .current
            .as_ref()
            .ok_or_else(|| {
                self.lexer.internal_error(diagnostic_context!(
                    "typol-syntax",
                    "token-cursor",
                    "current-event-missing-after-scan"
                ))
            })
            .store_err_into(out.reborrow())?;
        Ok(event)
    }

    pub fn peek_next<'slot>(
        &mut self,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, &LexEvent> {
        self.ensure_current(out.reborrow())?;
        if self.current.as_ref().is_some_and(is_terminal) {
            let event = self
                .current
                .as_ref()
                .ok_or_else(|| {
                    self.lexer.internal_error(diagnostic_context!(
                        "typol-syntax",
                        "token-cursor",
                        "terminal-event-disappeared"
                    ))
                })
                .store_err_into(out.reborrow())?;
            return Ok(event);
        }
        if self.next.is_none() {
            self.next = Some(self.scan());
        }
        let event = self
            .next
            .as_ref()
            .ok_or_else(|| {
                self.lexer.internal_error(diagnostic_context!(
                    "typol-syntax",
                    "token-cursor",
                    "next-event-cache-missing"
                ))
            })
            .store_err_into(out.reborrow())?;
        Ok(event)
    }

    /// Returns ownership of the current event; consuming a cached event does not scan further input.
    pub fn bump<'slot>(&mut self, mut out: Out<'slot, '_, Diagnostic>) -> OResult<'slot, LexEvent> {
        self.ensure_current(out.reborrow())?;
        let Some(event) = self.current.take() else {
            return Err(out
                .reborrow()
                .fail(self.lexer.internal_error(diagnostic_context!(
                    "typol-syntax",
                    "token-cursor",
                    "current-event-missing-on-bump"
                ))));
        };
        self.current = match &event {
            LexEvent::Eof(token) => Some(LexEvent::Halted(token.span())),
            LexEvent::Halted(span) => Some(LexEvent::Halted(*span)),
            _ => self.next.take(),
        };
        Ok(event)
    }

    fn ensure_current<'slot>(&mut self, mut out: Out<'slot, '_, Diagnostic>) -> OResult<'slot, ()> {
        if self.current.is_none() && self.next.is_some() {
            return Err(out
                .reborrow()
                .fail(self.lexer.internal_error(diagnostic_context!(
                    "typol-syntax",
                    "token-cursor",
                    "next-event-exists-without-current-event"
                ))));
        }
        if self.current.is_none() {
            self.current = Some(self.scan());
        }
        if self.current.is_some() {
            Ok(())
        } else {
            Err(out
                .reborrow()
                .fail(self.lexer.internal_error(diagnostic_context!(
                    "typol-syntax",
                    "token-cursor",
                    "scan-did-not-create-current-event"
                ))))
        }
    }

    fn scan(&mut self) -> LexEvent {
        if let Some(span) = self.halted {
            return LexEvent::Halted(span);
        }
        match self.error_slot.try_call(|out| self.lexer.next_token(out)) {
            Ok(token) if token.kind() == TokenKind::Eof => {
                self.halted = Some(token.span());
                LexEvent::Eof(token)
            }
            Ok(token) => LexEvent::Token(token),
            Err(caught) if self.lexer.fatal() => {
                let mut diagnostic = caught.take();
                let span = diagnostic.primary().unwrap_or_else(|| {
                    let current = self.lexer.current_span();
                    diagnostic.set_located_primary(Some(current));
                    current.source_span()
                });
                self.halted = Some(span);
                LexEvent::Fatal(diagnostic)
            }
            Err(caught) => LexEvent::Error(caught.take()),
        }
    }
}

fn is_terminal(event: &LexEvent) -> bool {
    matches!(
        event,
        LexEvent::Eof(_) | LexEvent::Fatal(_) | LexEvent::Halted(_)
    )
}
