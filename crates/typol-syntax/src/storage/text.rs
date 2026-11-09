use crate::{ast::AstAccessError, error::SyntaxDiagnostic};
use core::{
    fmt::{Debug, Formatter, Result as FmtResult},
    mem,
};
use opeo::{OResult, Out};
use typol_core::{ChargedString, ChargedVec, Diagnostic, ExecutionBudget, SourceSpan};

#[derive(Clone, Copy)]
pub(crate) enum TextRef {
    Pooled { start: usize, end: usize },
    Decoded(usize),
}

impl Debug for TextRef {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        formatter.write_str("TextRef { .. }")
    }
}

pub(crate) struct TextStorage {
    text: ChargedString,
    decoded: ChargedVec<ChargedString>,
}

impl TextStorage {
    pub(crate) fn new<'slot>(
        execution: &ExecutionBudget,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Self> {
        let text = SyntaxDiagnostic::new_string(execution, span, out.reborrow())?;
        let decoded = SyntaxDiagnostic::new_vec(execution, span, out.reborrow())?;
        Ok(Self { text, decoded })
    }

    pub(crate) fn memory_bytes(&self) -> Option<usize> {
        let mut bytes = self
            .decoded
            .capacity()
            .checked_mul(mem::size_of::<ChargedString>())?
            .checked_add(self.text.capacity())?;
        for value in self.decoded.as_slice() {
            bytes = bytes.checked_add(value.capacity())?;
        }
        Some(bytes)
    }

    pub(crate) fn read<'slot, 'storage>(
        &'storage self,
        reference: TextRef,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, &'storage str> {
        match self.resolve(reference) {
            Ok(value) => Ok(value),
            Err(error) => Err(out.reborrow().fail(error)),
        }
    }

    pub(crate) fn read_for_diagnostic<'slot, 'storage>(
        &'storage self,
        reference: TextRef,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, &'storage str> {
        match self.resolve(reference) {
            Ok(value) => Ok(value),
            Err(error) => Err(out.reborrow().fail(SyntaxDiagnostic::ast_access(error))),
        }
    }

    pub(crate) fn append_pooled<'slot>(
        &mut self,
        value: &str,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, TextRef> {
        let start = self.text.len();
        SyntaxDiagnostic::push_str(&mut self.text, value, span, out.reborrow())?;
        Ok(TextRef::Pooled {
            start,
            end: self.text.len(),
        })
    }

    pub(crate) fn append_decoded<'slot>(
        &mut self,
        value: ChargedString,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, TextRef> {
        let index = self.decoded.len();
        SyntaxDiagnostic::try_push(&mut self.decoded, value, span, out.reborrow())?;
        Ok(TextRef::Decoded(index))
    }

    fn resolve(&self, reference: TextRef) -> Result<&str, AstAccessError> {
        match reference {
            TextRef::Pooled { start, end } => self
                .text
                .get(start..end)
                .ok_or(AstAccessError::InvalidTextRange { start, end }),
            TextRef::Decoded(index) => self
                .decoded
                .get(index)
                .map(ChargedString::as_str)
                .ok_or(AstAccessError::InvalidDecodedTextId { id: index }),
        }
    }
}
