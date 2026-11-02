use crate::{
    expressions::{
        AssertionInfo, AssertionIter as StoredAssertionIter, AssertionList as StoredAssertionList,
    },
    model::{CompiledSchema, SchemaAccessError},
    views::ExpressionView,
};
use opeo::{OResult, Out, ResultOutExt};
use typol_core::SourceSpan;

pub struct AssertionIter<'a> {
    schema: &'a CompiledSchema,
    raw: StoredAssertionIter<'a>,
}

#[derive(Clone, Copy)]
pub struct AssertionView<'a> {
    schema: &'a CompiledSchema,
    raw: &'a AssertionInfo,
}

impl<'a> AssertionView<'a> {
    pub fn expression(self) -> ExpressionView<'a> {
        ExpressionView::new(self.schema, self.raw.expression())
    }

    pub fn message(self) -> &'a str {
        self.raw.message()
    }

    pub fn span(self) -> SourceSpan {
        self.raw.span()
    }
}

#[derive(Clone, Copy)]
pub struct AssertionList<'a> {
    schema: &'a CompiledSchema,
    raw: StoredAssertionList<'a>,
}

impl<'a> AssertionList<'a> {
    pub(super) fn new(schema: &'a CompiledSchema, raw: StoredAssertionList<'a>) -> Self {
        Self { schema, raw }
    }

    pub fn len(self) -> usize {
        self.raw.len()
    }

    pub fn is_empty(self) -> bool {
        self.raw.is_empty()
    }

    pub fn get<'slot>(
        self,
        i: usize,
        mut out: Out<'slot, '_, SchemaAccessError>,
    ) -> OResult<'slot, Option<AssertionView<'a>>> {
        match self.raw.get(i).store_err_into(out.reborrow())? {
            Some(raw) => Ok(Some(AssertionView {
                schema: self.schema,
                raw,
            })),
            None => Ok(None),
        }
    }

    pub fn iter<'iter, 'slot>(
        self,
        mut out: Out<'slot, 'iter, SchemaAccessError>,
    ) -> impl Iterator<Item = OResult<'slot, AssertionView<'a>>> + 'iter
    where
        'a: 'iter,
        'slot: 'iter,
    {
        self.raw.iter().map(move |raw| {
            raw.map(|raw| AssertionView {
                schema: self.schema,
                raw,
            })
            .store_err_into(out.reborrow())
        })
    }
}

impl<'a> IntoIterator for AssertionList<'a> {
    type Item = Result<AssertionView<'a>, SchemaAccessError>;
    type IntoIter = AssertionIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        AssertionIter {
            schema: self.schema,
            raw: self.raw.into_iter(),
        }
    }
}

impl<'a> Iterator for AssertionIter<'a> {
    type Item = Result<AssertionView<'a>, SchemaAccessError>;

    fn next(&mut self) -> Option<Self::Item> {
        self.raw.next().map(|raw| {
            raw.map(|raw| AssertionView {
                schema: self.schema,
                raw,
            })
        })
    }
}
