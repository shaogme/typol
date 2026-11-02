pub use crate::storage::BuiltinType;
use crate::{
    ast::NameList,
    storage::{
        ArrayLength as StoredArrayLength, AstAccessError, Storage, TypeExpr as StoredTypeExpr,
        TypeExprKind as StoredTypeExprKind, TypePath as StoredTypePath,
    },
};
use core::fmt::{Debug, Formatter, Result as FmtResult};
use opeo::{OResult, Out};
use typol_core::SourceSpan;

#[derive(Clone, Copy)]
pub struct ArrayLength<'a> {
    storage: &'a Storage,
    record: &'a StoredArrayLength,
}

#[derive(Clone, Copy)]
pub struct TypePath<'a> {
    storage: &'a Storage,
    record: &'a StoredTypePath,
}

#[derive(Clone, Copy)]
pub struct TypeExpr<'a> {
    storage: &'a Storage,
    record: &'a StoredTypeExpr,
}

#[derive(Clone, Copy, Debug)]
pub enum TypeExprKind<'a> {
    Builtin(BuiltinType),
    Option(TypeExpr<'a>),
    Array {
        element: TypeExpr<'a>,
        length: Option<ArrayLength<'a>>,
    },
    Map {
        value: TypeExpr<'a>,
    },
    Path(TypePath<'a>),
}

impl<'a> TypeExprKind<'a> {
    pub(super) fn project<'slot>(
        storage: &'a Storage,
        record: &'a StoredTypeExprKind,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, Self> {
        match record {
            StoredTypeExprKind::Builtin(value) => Ok(Self::Builtin(*value)),
            StoredTypeExprKind::Option(value) => {
                let record = storage.get(*value, out.reborrow())?;
                let value = TypeExpr::project(storage, record);
                Ok(Self::Option(value))
            }
            StoredTypeExprKind::Array { element, length } => {
                let element = storage.get(*element, out.reborrow())?;
                let element = TypeExpr::project(storage, element);
                let length = if let Some(value) = length {
                    let record = storage.get(*value, out.reborrow())?;
                    Some(ArrayLength::project(storage, record))
                } else {
                    None
                };
                Ok(Self::Array { element, length })
            }
            StoredTypeExprKind::Map { value } => {
                let record = storage.get(*value, out.reborrow())?;
                let value = TypeExpr::project(storage, record);
                Ok(Self::Map { value })
            }
            StoredTypeExprKind::Path(value) => {
                let record = storage.get(*value, out.reborrow())?;
                Ok(Self::Path(TypePath::project(storage, record)))
            }
        }
    }
}

impl Debug for ArrayLength<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct("ArrayLength")
            .field("span", &self.span())
            .finish_non_exhaustive()
    }
}

impl<'a> ArrayLength<'a> {
    pub fn raw<'slot>(self, mut out: Out<'slot, '_, AstAccessError>) -> OResult<'slot, &'a str> {
        let text = self.storage.text(self.record.raw, out.reborrow())?;
        Ok(text)
    }

    pub fn span(self) -> SourceSpan {
        self.record.span
    }
}

impl Debug for TypePath<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct("TypePath")
            .field("span", &self.span())
            .finish_non_exhaustive()
    }
}

impl<'a> TypePath<'a> {
    pub fn segments<'slot>(
        self,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, NameList<'a>> {
        let records = self.storage.list(self.record.segments, out.reborrow())?;
        Ok(NameList::project(self.storage, records))
    }

    pub fn span(self) -> SourceSpan {
        self.record.span
    }
}

impl Debug for TypeExpr<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct("TypeExpr")
            .field("span", &self.span())
            .finish_non_exhaustive()
    }
}

impl<'a> TypeExpr<'a> {
    pub fn kind<'slot>(
        self,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, TypeExprKind<'a>> {
        TypeExprKind::project(self.storage, &self.record.kind, out.reborrow())
    }

    pub fn span(self) -> SourceSpan {
        self.record.span
    }
}

impl<'a> ArrayLength<'a> {
    pub(crate) fn project(storage: &'a Storage, record: &'a StoredArrayLength) -> Self {
        Self { storage, record }
    }
}

impl<'a> TypePath<'a> {
    pub(crate) fn project(storage: &'a Storage, record: &'a StoredTypePath) -> Self {
        Self { storage, record }
    }
}

impl<'a> TypeExpr<'a> {
    pub(crate) fn project(storage: &'a Storage, record: &'a StoredTypeExpr) -> Self {
        Self { storage, record }
    }
}
