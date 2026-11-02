use crate::{
    ast::{
        Annotation, Assertion, BraceEntry, Declaration, DefaultValue, FieldDecl, Name,
        TopLevelItem, VariantDecl,
    },
    storage::{
        Annotation as StoredAnnotation, Assertion as StoredAssertion, AstAccessError,
        BraceEntry as StoredBraceEntry, Declaration as StoredDeclaration,
        DefaultValue as StoredDefaultValue, FieldDecl as StoredFieldDecl, Id, Name as StoredName,
        Storage, TopLevelItem as StoredTopLevelItem, VariantDecl as StoredVariantDecl,
    },
};
use core::fmt::{Debug, Formatter, Result as FmtResult};
use opeo::{OResult, Out};

macro_rules! ast_list {
    ($list:ident, $iter:ident, $view:ident, $record:ty, |$storage:ident, $value:ident, $out:ident| $project:block) => {
        /// Borrows a read-only child list from its owning tree in source order.
        #[derive(Clone, Copy)]
        pub struct $list<'a> {
            storage: &'a Storage,
            records: &'a [$record],
        }

        impl Debug for $list<'_> {
            fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
                formatter
                    .debug_struct(stringify!($list))
                    .field("len", &self.records.len())
                    .finish()
            }
        }

        impl<'a> $list<'a> {
            pub(crate) fn project(storage: &'a Storage, records: &'a [$record]) -> Self {
                Self { storage, records }
            }

            pub fn len(self) -> usize {
                self.records.len()
            }

            pub fn is_empty(self) -> bool {
                self.records.is_empty()
            }

            pub fn get<'slot, 'borrow>(
                self,
                index: usize,
                mut $out: Out<'slot, 'borrow, AstAccessError>,
            ) -> OResult<'slot, Option<$view<'a>>> {
                let Some(value) = self.records.get(index) else {
                    return Ok(None);
                };
                let $storage = self.storage;
                let $value = value;
                $project
            }

            pub fn first<'slot, 'borrow>(
                self,
                out: Out<'slot, 'borrow, AstAccessError>,
            ) -> OResult<'slot, Option<$view<'a>>> {
                self.get(0, out)
            }

            pub fn last<'slot, 'borrow>(
                self,
                out: Out<'slot, 'borrow, AstAccessError>,
            ) -> OResult<'slot, Option<$view<'a>>> {
                let Some(index) = self.len().checked_sub(1) else {
                    return Ok(None);
                };
                self.get(index, out)
            }

            pub fn iter<'slot, 'borrow>(
                self,
                out: Out<'slot, 'borrow, AstAccessError>,
            ) -> $iter<'a, 'slot, 'borrow> {
                $iter {
                    list: self,
                    front: 0,
                    back: self.len(),
                    out,
                }
            }

            pub fn into_iter<'slot, 'borrow>(
                self,
                out: Out<'slot, 'borrow, AstAccessError>,
            ) -> $iter<'a, 'slot, 'borrow> {
                self.iter(out)
            }
        }

        /// An allocation-free, double-ended iterator bound to its tree.
        pub struct $iter<'a, 'slot, 'borrow> {
            list: $list<'a>,
            front: usize,
            back: usize,
            out: Out<'slot, 'borrow, AstAccessError>,
        }

        impl<'a, 'slot, 'borrow> Iterator for $iter<'a, 'slot, 'borrow> {
            type Item = OResult<'slot, $view<'a>>;

            fn next(&mut self) -> Option<Self::Item> {
                if self.front == self.back {
                    return None;
                }
                let result = self.list.get(self.front, self.out.reborrow());
                self.front += 1;
                match result {
                    Ok(Some(view)) => Some(Ok(view)),
                    Ok(None) => None,
                    Err(failure) => Some(Err(failure)),
                }
            }

            fn size_hint(&self) -> (usize, Option<usize>) {
                let len = self.back - self.front;
                (len, Some(len))
            }
        }

        impl<'a, 'slot, 'borrow> DoubleEndedIterator for $iter<'a, 'slot, 'borrow> {
            fn next_back(&mut self) -> Option<Self::Item> {
                if self.front == self.back {
                    return None;
                }
                self.back -= 1;
                let result = self.list.get(self.back, self.out.reborrow());
                match result {
                    Ok(Some(view)) => Some(Ok(view)),
                    Ok(None) => None,
                    Err(failure) => Some(Err(failure)),
                }
            }
        }
    };
}

ast_list!(
    AnnotationList,
    AnnotationIter,
    Annotation,
    Id<StoredAnnotation>,
    |storage, value, out| {
        storage
            .get(*value, out.reborrow())
            .map(|record| Annotation::project(storage, record))
            .map(Some)
    }
);

ast_list!(
    AssertionList,
    AssertionIter,
    Assertion,
    Id<StoredAssertion>,
    |storage, value, out| {
        storage
            .get(*value, out.reborrow())
            .map(|record| Assertion::project(storage, record))
            .map(Some)
    }
);

ast_list!(
    BraceEntryList,
    BraceEntryIter,
    BraceEntry,
    Id<StoredBraceEntry>,
    |storage, value, out| {
        storage
            .get(*value, out.reborrow())
            .map(|record| BraceEntry::project(storage, record))
            .map(Some)
    }
);

ast_list!(
    DeclarationList,
    DeclarationIter,
    Declaration,
    StoredDeclaration,
    |storage, value, out| { Declaration::project(storage, value, out.reborrow()).map(Some) }
);

ast_list!(
    DefaultValueList,
    DefaultValueIter,
    DefaultValue,
    Id<StoredDefaultValue>,
    |storage, value, out| {
        storage
            .get(*value, out.reborrow())
            .map(|record| DefaultValue::project(storage, record))
            .map(Some)
    }
);

ast_list!(
    FieldDeclList,
    FieldDeclIter,
    FieldDecl,
    Id<StoredFieldDecl>,
    |storage, value, out| {
        storage
            .get(*value, out.reborrow())
            .map(|record| FieldDecl::project(storage, record))
            .map(Some)
    }
);

ast_list!(
    NameList,
    NameIter,
    Name,
    Id<StoredName>,
    |storage, value, out| {
        storage
            .get(*value, out.reborrow())
            .map(|record| Name::project(storage, record))
            .map(Some)
    }
);

ast_list!(
    TopLevelItemList,
    TopLevelItemIter,
    TopLevelItem,
    StoredTopLevelItem,
    |storage, value, out| { TopLevelItem::project(storage, value, out.reborrow()).map(Some) }
);

ast_list!(
    VariantDeclList,
    VariantDeclIter,
    VariantDecl,
    Id<StoredVariantDecl>,
    |storage, value, out| {
        storage
            .get(*value, out.reborrow())
            .map(|record| VariantDecl::project(storage, record))
            .map(Some)
    }
);
