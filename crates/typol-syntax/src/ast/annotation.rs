pub use crate::token::AnnotationKind;
use crate::{
    ast::{BooleanLiteral, Expr, Name, NumberLiteral, StringLiteral, StringLiteralList},
    storage::{
        Annotation as StoredAnnotation, AnnotationData as StoredAnnotationData,
        Assertion as StoredAssertion, AstAccessError, Id, NamedArgument as StoredNamedArgument,
        NamedBounds as StoredNamedBounds, NumberLiteral as StoredNumberLiteral,
        SizeArguments as StoredSizeArguments, Storage,
    },
    token::VariantRestrictionMode,
};
use core::fmt::{Debug, Formatter, Result as FmtResult};
use opeo::{OResult, Out};
use typol_core::SourceSpan;

#[derive(Clone, Copy)]
pub struct NamedArgument<'a, T> {
    name: Name<'a>,
    value: T,
    span: SourceSpan,
}

#[derive(Clone, Copy)]
pub struct NamedBounds<'a> {
    storage: &'a Storage,
    record: &'a StoredNamedBounds,
}

pub struct NamedBoundsIter<'a, 'slot, 'borrow> {
    bounds: NamedBounds<'a>,
    index: usize,
    out: Out<'slot, 'borrow, AstAccessError>,
}

#[derive(Clone, Copy)]
pub struct Annotation<'a> {
    storage: &'a Storage,
    record: &'a StoredAnnotation,
}

#[derive(Clone, Copy)]
pub struct Assertion<'a> {
    storage: &'a Storage,
    record: &'a StoredAssertion,
}

#[derive(Clone, Copy, Debug)]
pub enum SizeArguments<'a> {
    Positional {
        min: NumberLiteral<'a>,
        max: NumberLiteral<'a>,
    },
    Named(NamedBounds<'a>),
}

#[derive(Clone, Copy, Debug)]
pub enum AnnotationData<'a> {
    Min(NumberLiteral<'a>),
    Max(NumberLiteral<'a>),
    Range {
        min: NumberLiteral<'a>,
        max: NumberLiteral<'a>,
    },
    Length {
        min: NumberLiteral<'a>,
        max: NumberLiteral<'a>,
    },
    Regex(StringLiteral<'a>),
    Host,
    Email,
    Cidr(NamedArgument<'a, BooleanLiteral<'a>>),
    Env(StringLiteral<'a>),
    Size(SizeArguments<'a>),
    Unique(Option<NamedArgument<'a, StringLiteral<'a>>>),
    All(Assertion<'a>),
    Sum(NamedBounds<'a>),
    Secret,
    Variants {
        mode: VariantRestrictionMode,
        names: StringLiteralList<'a>,
    },
}

impl<'a> SizeArguments<'a> {
    pub(super) fn project<'slot>(
        storage: &'a Storage,
        record: &'a StoredSizeArguments,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, Self> {
        match record {
            StoredSizeArguments::Positional { min, max } => {
                let min = storage.get(*min, out.reborrow())?;
                let max = storage.get(*max, out.reborrow())?;
                Ok(Self::Positional {
                    min: NumberLiteral::project(storage, min),
                    max: NumberLiteral::project(storage, max),
                })
            }
            StoredSizeArguments::Named(value) => {
                Ok(Self::Named(NamedBounds::project(storage, value)))
            }
        }
    }
}

impl<'a> AnnotationData<'a> {
    pub(super) fn project<'slot>(
        storage: &'a Storage,
        record: &'a StoredAnnotationData,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, Self> {
        match record {
            StoredAnnotationData::Min(value) => {
                let record = storage.get(*value, out.reborrow())?;
                Ok(Self::Min(NumberLiteral::project(storage, record)))
            }
            StoredAnnotationData::Max(value) => {
                let record = storage.get(*value, out.reborrow())?;
                Ok(Self::Max(NumberLiteral::project(storage, record)))
            }
            StoredAnnotationData::Range { min, max } => {
                let min = storage.get(*min, out.reborrow())?;
                let max = storage.get(*max, out.reborrow())?;
                Ok(Self::Range {
                    min: NumberLiteral::project(storage, min),
                    max: NumberLiteral::project(storage, max),
                })
            }
            StoredAnnotationData::Length { min, max } => {
                let min = storage.get(*min, out.reborrow())?;
                let max = storage.get(*max, out.reborrow())?;
                Ok(Self::Length {
                    min: NumberLiteral::project(storage, min),
                    max: NumberLiteral::project(storage, max),
                })
            }
            StoredAnnotationData::Regex(value) => {
                let record = storage.get(*value, out.reborrow())?;
                Ok(Self::Regex(StringLiteral::project(storage, record)))
            }
            StoredAnnotationData::Host => Ok(Self::Host),
            StoredAnnotationData::Email => Ok(Self::Email),
            StoredAnnotationData::Cidr(value) => {
                let name = storage.get(value.name, out.reborrow())?;
                let boolean = storage.get(value.value, out.reborrow())?;
                Ok(Self::Cidr(NamedArgument {
                    name: Name::project(storage, name),
                    value: BooleanLiteral::project(boolean),
                    span: value.span,
                }))
            }
            StoredAnnotationData::Env(value) => {
                let record = storage.get(*value, out.reborrow())?;
                Ok(Self::Env(StringLiteral::project(storage, record)))
            }
            StoredAnnotationData::Size(value) => {
                let arguments = SizeArguments::project(storage, value, out.reborrow())?;
                Ok(Self::Size(arguments))
            }
            StoredAnnotationData::Unique(value) => {
                let argument = if let Some(value) = value {
                    let name = storage.get(value.name, out.reborrow())?;
                    let string = storage.get(value.value, out.reborrow())?;
                    Some(NamedArgument {
                        name: Name::project(storage, name),
                        value: StringLiteral::project(storage, string),
                        span: value.span,
                    })
                } else {
                    None
                };
                Ok(Self::Unique(argument))
            }
            StoredAnnotationData::All(value) => {
                let record = storage.get(*value, out.reborrow())?;
                Ok(Self::All(Assertion::project(storage, record)))
            }
            StoredAnnotationData::Sum(value) => Ok(Self::Sum(NamedBounds::project(storage, value))),
            StoredAnnotationData::Secret => Ok(Self::Secret),
            StoredAnnotationData::Variants { mode, names } => {
                let records = storage.list(*names, out.reborrow())?;
                Ok(Self::Variants {
                    mode: *mode,
                    names: StringLiteralList::project(storage, records),
                })
            }
        }
    }
}

impl Debug for NamedBounds<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct("NamedBounds").finish_non_exhaustive()
    }
}

impl<'a> NamedBounds<'a> {
    pub(super) fn project(storage: &'a Storage, record: &'a StoredNamedBounds) -> Self {
        Self { storage, record }
    }

    pub fn iter<'slot, 'borrow>(
        self,
        out: Out<'slot, 'borrow, AstAccessError>,
    ) -> NamedBoundsIter<'a, 'slot, 'borrow> {
        NamedBoundsIter {
            bounds: self,
            index: 0,
            out,
        }
    }

    fn project_argument<'slot>(
        self,
        argument: &StoredNamedArgument<Id<StoredNumberLiteral>>,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, NamedArgument<'a, NumberLiteral<'a>>> {
        let name = self.storage.get(argument.name, out.reborrow())?;
        let number = self.storage.get(argument.value, out.reborrow())?;
        Ok(NamedArgument {
            name: Name::project(self.storage, name),
            value: NumberLiteral::project(self.storage, number),
            span: argument.span,
        })
    }
}

impl<'a, 'slot, 'borrow> Iterator for NamedBoundsIter<'a, 'slot, 'borrow> {
    type Item = OResult<'slot, NamedArgument<'a, NumberLiteral<'a>>>;

    fn next(&mut self) -> Option<Self::Item> {
        let argument = match self.index {
            0 => Some(&self.bounds.record.first),
            1 => self.bounds.record.second.as_ref(),
            _ => None,
        }?;
        self.index += 1;
        Some(self.bounds.project_argument(argument, self.out.reborrow()))
    }
}

impl Debug for Annotation<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct("Annotation")
            .field("span", &self.span())
            .finish_non_exhaustive()
    }
}

impl<'a> Annotation<'a> {
    pub fn kind<'slot>(
        self,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, AnnotationKind> {
        let data = self.data(out.reborrow())?;
        let kind = match data {
            AnnotationData::Min(_) => AnnotationKind::Min,
            AnnotationData::Max(_) => AnnotationKind::Max,
            AnnotationData::Range { .. } => AnnotationKind::Range,
            AnnotationData::Length { .. } => AnnotationKind::Length,
            AnnotationData::Regex(_) => AnnotationKind::Regex,
            AnnotationData::Host => AnnotationKind::Host,
            AnnotationData::Email => AnnotationKind::Email,
            AnnotationData::Cidr(_) => AnnotationKind::Cidr,
            AnnotationData::Env(_) => AnnotationKind::Env,
            AnnotationData::Size(_) => AnnotationKind::Size,
            AnnotationData::Unique(_) => AnnotationKind::Unique,
            AnnotationData::All(_) => AnnotationKind::All,
            AnnotationData::Sum(_) => AnnotationKind::Sum,
            AnnotationData::Secret => AnnotationKind::Secret,
            AnnotationData::Variants { .. } => AnnotationKind::Variants,
        };
        Ok(kind)
    }

    pub fn data<'slot>(
        self,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, AnnotationData<'a>> {
        AnnotationData::project(self.storage, &self.record.data, out.reborrow())
    }

    pub fn span(self) -> SourceSpan {
        self.record.span
    }
}

impl Debug for Assertion<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct("Assertion")
            .field("span", &self.span())
            .finish_non_exhaustive()
    }
}

impl<'a> Assertion<'a> {
    pub fn expression<'slot>(
        self,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, Expr<'a>> {
        let record = self.storage.get(self.record.expression, out.reborrow())?;
        Ok(Expr::project(self.storage, record))
    }

    pub fn message<'slot>(
        self,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, StringLiteral<'a>> {
        let record = self.storage.get(self.record.message, out.reborrow())?;
        Ok(StringLiteral::project(self.storage, record))
    }

    pub fn span(self) -> SourceSpan {
        self.record.span
    }
}

impl<T> Debug for NamedArgument<'_, T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct("NamedArgument")
            .field("span", &self.span)
            .finish_non_exhaustive()
    }
}

impl<'a, T: Copy> NamedArgument<'a, T> {
    pub fn name(self) -> Name<'a> {
        self.name
    }

    pub fn value(self) -> T {
        self.value
    }

    pub fn span(self) -> SourceSpan {
        self.span
    }
}

impl<'a> Annotation<'a> {
    pub(crate) fn project(storage: &'a Storage, record: &'a StoredAnnotation) -> Self {
        Self { storage, record }
    }
}

impl<'a> Assertion<'a> {
    pub(crate) fn project(storage: &'a Storage, record: &'a StoredAssertion) -> Self {
        Self { storage, record }
    }
}
