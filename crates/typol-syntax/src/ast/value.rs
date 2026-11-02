pub use crate::token::NumberKind;
use crate::{
    ast::{BraceEntryList, DefaultValueList},
    storage::{
        ArrayDefault as StoredArrayDefault, AstAccessError, BooleanLiteral as StoredBooleanLiteral,
        BraceEntry as StoredBraceEntry, BraceKey as StoredBraceKey,
        BracedDefault as StoredBracedDefault, DefaultValue as StoredDefaultValue,
        DefaultValueKind as StoredDefaultValueKind, EnumDefault as StoredEnumDefault,
        EnumDefaultPayload as StoredEnumDefaultPayload,
        EnumDefaultPayloadKind as StoredEnumDefaultPayloadKind, Name as StoredName,
        NumberLiteral as StoredNumberLiteral, ScalarLiteral as StoredScalarLiteral, Storage,
        StringLiteral as StoredStringLiteral,
    },
};
use core::fmt::{Debug, Formatter, Result as FmtResult};
use opeo::{OResult, Out};
use typol_core::SourceSpan;

#[derive(Clone, Copy)]
pub struct Name<'a> {
    storage: &'a Storage,
    record: &'a StoredName,
}

#[derive(Clone, Copy)]
pub struct NumberLiteral<'a> {
    storage: &'a Storage,
    record: &'a StoredNumberLiteral,
}

#[derive(Clone, Copy)]
pub struct StringLiteral<'a> {
    storage: &'a Storage,
    record: &'a StoredStringLiteral,
}

#[derive(Clone, Copy)]
pub struct BooleanLiteral<'a> {
    record: &'a StoredBooleanLiteral,
}

#[derive(Clone, Copy)]
pub struct BraceEntry<'a> {
    storage: &'a Storage,
    record: &'a StoredBraceEntry,
}

#[derive(Clone, Copy)]
pub struct ArrayDefault<'a> {
    storage: &'a Storage,
    record: &'a StoredArrayDefault,
}

#[derive(Clone, Copy)]
pub struct BracedDefault<'a> {
    storage: &'a Storage,
    record: &'a StoredBracedDefault,
}

#[derive(Clone, Copy)]
pub struct EnumDefaultPayload<'a> {
    storage: &'a Storage,
    record: &'a StoredEnumDefaultPayload,
}

#[derive(Clone, Copy)]
pub struct EnumDefault<'a> {
    storage: &'a Storage,
    record: &'a StoredEnumDefault,
}

#[derive(Clone, Copy)]
pub struct DefaultValue<'a> {
    storage: &'a Storage,
    record: &'a StoredDefaultValue,
}

#[derive(Clone, Copy, Debug)]
pub enum ScalarLiteral<'a> {
    String(StringLiteral<'a>),
    Number(NumberLiteral<'a>),
    Boolean(BooleanLiteral<'a>),
}

#[derive(Clone, Copy, Debug)]
pub enum BraceKey<'a> {
    Identifier(Name<'a>),
    String(StringLiteral<'a>),
}

#[derive(Clone, Copy, Debug)]
pub enum EnumDefaultPayloadKind<'a> {
    Unit,
    Single(DefaultValue<'a>),
    Structure(BracedDefault<'a>),
}

#[derive(Clone, Copy, Debug)]
pub enum DefaultValueKind<'a> {
    Scalar(ScalarLiteral<'a>),
    None,
    Array(ArrayDefault<'a>),
    Braced(BracedDefault<'a>),
    Enum(EnumDefault<'a>),
}

impl<'a> ScalarLiteral<'a> {
    pub(super) fn project<'slot>(
        storage: &'a Storage,
        record: &'a StoredScalarLiteral,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, Self> {
        match record {
            StoredScalarLiteral::String(value) => {
                let record = storage.get(*value, out.reborrow())?;
                Ok(Self::String(StringLiteral::project(storage, record)))
            }
            StoredScalarLiteral::Number(value) => {
                let record = storage.get(*value, out.reborrow())?;
                Ok(Self::Number(NumberLiteral::project(storage, record)))
            }
            StoredScalarLiteral::Boolean(value) => {
                let record = storage.get(*value, out.reborrow())?;
                Ok(Self::Boolean(BooleanLiteral::project(record)))
            }
        }
    }

    pub fn span(self) -> SourceSpan {
        match self {
            Self::String(value) => value.span(),
            Self::Number(value) => value.span(),
            Self::Boolean(value) => value.span(),
        }
    }
}

impl<'a> BraceKey<'a> {
    pub(super) fn project<'slot>(
        storage: &'a Storage,
        record: &'a StoredBraceKey,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, Self> {
        match record {
            StoredBraceKey::Identifier(value) => {
                let record = storage.get(*value, out.reborrow())?;
                Ok(Self::Identifier(Name::project(storage, record)))
            }
            StoredBraceKey::String(value) => {
                let record = storage.get(*value, out.reborrow())?;
                Ok(Self::String(StringLiteral::project(storage, record)))
            }
        }
    }

    pub fn span(self) -> SourceSpan {
        match self {
            Self::Identifier(value) => value.span(),
            Self::String(value) => value.span(),
        }
    }
}

impl<'a> EnumDefaultPayloadKind<'a> {
    pub(super) fn project<'slot>(
        storage: &'a Storage,
        record: &'a StoredEnumDefaultPayloadKind,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, Self> {
        match record {
            StoredEnumDefaultPayloadKind::Unit => Ok(Self::Unit),
            StoredEnumDefaultPayloadKind::Single(value) => {
                let record = storage.get(*value, out.reborrow())?;
                Ok(Self::Single(DefaultValue::project(storage, record)))
            }
            StoredEnumDefaultPayloadKind::Structure(value) => {
                let record = storage.get(*value, out.reborrow())?;
                Ok(Self::Structure(BracedDefault::project(storage, record)))
            }
        }
    }
}

impl<'a> DefaultValueKind<'a> {
    pub(super) fn project<'slot>(
        storage: &'a Storage,
        record: &'a StoredDefaultValueKind,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, Self> {
        match record {
            StoredDefaultValueKind::Scalar(value) => {
                ScalarLiteral::project(storage, value, out.reborrow()).map(Self::Scalar)
            }
            StoredDefaultValueKind::None => Ok(Self::None),
            StoredDefaultValueKind::Array(value) => {
                let record = storage.get(*value, out.reborrow())?;
                Ok(Self::Array(ArrayDefault::project(storage, record)))
            }
            StoredDefaultValueKind::Braced(value) => {
                let record = storage.get(*value, out.reborrow())?;
                Ok(Self::Braced(BracedDefault::project(storage, record)))
            }
            StoredDefaultValueKind::Enum(value) => {
                let record = storage.get(*value, out.reborrow())?;
                Ok(Self::Enum(EnumDefault::project(storage, record)))
            }
        }
    }
}

impl Debug for NumberLiteral<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct("NumberLiteral")
            .field("span", &self.span())
            .finish_non_exhaustive()
    }
}

impl<'a> NumberLiteral<'a> {
    pub fn raw<'slot>(self, mut out: Out<'slot, '_, AstAccessError>) -> OResult<'slot, &'a str> {
        let text = self.storage.text(self.record.raw, out.reborrow())?;
        Ok(text)
    }

    pub fn kind(self) -> NumberKind {
        self.record.kind
    }

    pub fn is_negative(self) -> bool {
        self.record.negative
    }

    pub fn span(self) -> SourceSpan {
        self.record.span
    }
}

impl Debug for StringLiteral<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct("StringLiteral")
            .field("span", &self.span())
            .finish_non_exhaustive()
    }
}

impl<'a> StringLiteral<'a> {
    pub fn value<'slot>(self, mut out: Out<'slot, '_, AstAccessError>) -> OResult<'slot, &'a str> {
        let value = self.storage.text(self.record.value, out.reborrow())?;
        Ok(value)
    }

    pub fn span(self) -> SourceSpan {
        self.record.span
    }
}

impl Debug for BooleanLiteral<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct("BooleanLiteral")
            .field("span", &self.span())
            .finish_non_exhaustive()
    }
}

impl<'a> BooleanLiteral<'a> {
    pub fn value(self) -> bool {
        self.record.value
    }

    pub fn span(self) -> SourceSpan {
        self.record.span
    }
}

impl Debug for BraceEntry<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct("BraceEntry")
            .field("span", &self.span())
            .finish_non_exhaustive()
    }
}

impl<'a> BraceEntry<'a> {
    pub fn key<'slot>(
        self,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, BraceKey<'a>> {
        BraceKey::project(self.storage, &self.record.key, out.reborrow())
    }

    pub fn value<'slot>(
        self,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, DefaultValue<'a>> {
        let record = self.storage.get(self.record.value, out.reborrow())?;
        Ok(DefaultValue::project(self.storage, record))
    }

    pub fn span(self) -> SourceSpan {
        self.record.span
    }
}

impl Debug for ArrayDefault<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct("ArrayDefault")
            .field("span", &self.span())
            .finish_non_exhaustive()
    }
}

impl<'a> ArrayDefault<'a> {
    pub fn elements<'slot>(
        self,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, DefaultValueList<'a>> {
        let records = self.storage.list(self.record.elements, out.reborrow())?;
        Ok(DefaultValueList::project(self.storage, records))
    }

    pub fn span(self) -> SourceSpan {
        self.record.span
    }
}

impl Debug for BracedDefault<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct("BracedDefault")
            .field("span", &self.span())
            .finish_non_exhaustive()
    }
}

impl<'a> BracedDefault<'a> {
    pub fn entries<'slot>(
        self,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, BraceEntryList<'a>> {
        let records = self.storage.list(self.record.entries, out.reborrow())?;
        Ok(BraceEntryList::project(self.storage, records))
    }

    pub fn span(self) -> SourceSpan {
        self.record.span
    }
}

impl Debug for EnumDefaultPayload<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct("EnumDefaultPayload")
            .field("span", &self.span())
            .finish_non_exhaustive()
    }
}

impl<'a> EnumDefaultPayload<'a> {
    pub fn kind<'slot>(
        self,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, EnumDefaultPayloadKind<'a>> {
        EnumDefaultPayloadKind::project(self.storage, &self.record.kind, out.reborrow())
    }

    pub fn span(self) -> SourceSpan {
        self.record.span
    }
}

impl Debug for EnumDefault<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct("EnumDefault")
            .field("span", &self.span())
            .finish_non_exhaustive()
    }
}

impl<'a> EnumDefault<'a> {
    pub fn variant<'slot>(
        self,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, &'a str> {
        let variant = self
            .storage
            .name_text(self.record.variant, out.reborrow())?;
        Ok(variant)
    }

    pub fn variant_node<'slot>(
        self,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, Name<'a>> {
        let record = self.storage.get(self.record.variant, out.reborrow())?;
        Ok(Name::project(self.storage, record))
    }

    pub fn payload<'slot>(
        self,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, EnumDefaultPayload<'a>> {
        let record = self.storage.get(self.record.payload, out.reborrow())?;
        Ok(EnumDefaultPayload::project(self.storage, record))
    }

    pub fn span(self) -> SourceSpan {
        self.record.span
    }
}

impl Debug for DefaultValue<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct("DefaultValue")
            .field("span", &self.span())
            .finish_non_exhaustive()
    }
}

impl<'a> DefaultValue<'a> {
    pub fn kind<'slot>(
        self,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, DefaultValueKind<'a>> {
        DefaultValueKind::project(self.storage, &self.record.kind, out.reborrow())
    }

    pub fn span(self) -> SourceSpan {
        self.record.span
    }
}

impl Debug for Name<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct("Name")
            .field("span", &self.span())
            .finish_non_exhaustive()
    }
}

impl<'a> Name<'a> {
    pub fn text<'slot>(self, mut out: Out<'slot, '_, AstAccessError>) -> OResult<'slot, &'a str> {
        let text = self.storage.text(self.record.text, out.reborrow())?;
        Ok(text)
    }

    pub fn span(self) -> SourceSpan {
        self.record.span
    }
}

impl<'a> NumberLiteral<'a> {
    pub(crate) fn project(storage: &'a Storage, record: &'a StoredNumberLiteral) -> Self {
        Self { storage, record }
    }
}

impl<'a> StringLiteral<'a> {
    pub(crate) fn project(storage: &'a Storage, record: &'a StoredStringLiteral) -> Self {
        Self { storage, record }
    }
}

impl<'a> BooleanLiteral<'a> {
    pub(crate) fn project(record: &'a StoredBooleanLiteral) -> Self {
        Self { record }
    }
}

impl<'a> BraceEntry<'a> {
    pub(crate) fn project(storage: &'a Storage, record: &'a StoredBraceEntry) -> Self {
        Self { storage, record }
    }
}

impl<'a> ArrayDefault<'a> {
    pub(crate) fn project(storage: &'a Storage, record: &'a StoredArrayDefault) -> Self {
        Self { storage, record }
    }
}

impl<'a> BracedDefault<'a> {
    pub(crate) fn project(storage: &'a Storage, record: &'a StoredBracedDefault) -> Self {
        Self { storage, record }
    }
}

impl<'a> EnumDefaultPayload<'a> {
    pub(crate) fn project(storage: &'a Storage, record: &'a StoredEnumDefaultPayload) -> Self {
        Self { storage, record }
    }
}

impl<'a> EnumDefault<'a> {
    pub(crate) fn project(storage: &'a Storage, record: &'a StoredEnumDefault) -> Self {
        Self { storage, record }
    }
}

impl<'a> DefaultValue<'a> {
    pub(crate) fn project(storage: &'a Storage, record: &'a StoredDefaultValue) -> Self {
        Self { storage, record }
    }
}

impl<'a> Name<'a> {
    pub(crate) fn project(storage: &'a Storage, record: &'a StoredName) -> Self {
        Self { storage, record }
    }
}
