use crate::{
    BudgetError, DiagnosticContext, ExecutionBudget, diagnostic_context, storage::WorkspaceString,
    workspace::WorkspaceVec,
};
use core::fmt;
use opeo::{OResult, Out};

static PATH_BUDGET_MISMATCH: DiagnosticContext = diagnostic_context!(
    "typol-core",
    "instance-path",
    "segment-budget-identity-mismatch",
);

/// A typed segment in a configuration instance path.
pub enum PathSegment {
    Field(WorkspaceString),
    Index(usize),
    MapKey(WorkspaceString),
    EnumPayload(WorkspaceString),
}

impl PathSegment {
    pub fn try_field<'slot>(
        name: &str,
        budget: Option<&ExecutionBudget>,
        out: Out<'slot, '_, BudgetError>,
    ) -> OResult<'slot, Self> {
        let name = WorkspaceString::copy(name, budget, out)?;
        Ok(Self::Field(name))
    }

    pub const fn index(index: usize) -> Self {
        Self::Index(index)
    }

    pub fn try_map_key<'slot>(
        key: &str,
        budget: Option<&ExecutionBudget>,
        out: Out<'slot, '_, BudgetError>,
    ) -> OResult<'slot, Self> {
        let key = WorkspaceString::copy(key, budget, out)?;
        Ok(Self::MapKey(key))
    }

    pub fn try_enum_payload<'slot>(
        variant: &str,
        budget: Option<&ExecutionBudget>,
        out: Out<'slot, '_, BudgetError>,
    ) -> OResult<'slot, Self> {
        let variant = WorkspaceString::copy(variant, budget, out)?;
        Ok(Self::EnumPayload(variant))
    }

    fn budget(&self) -> Option<&ExecutionBudget> {
        match self {
            Self::Field(text) | Self::MapKey(text) | Self::EnumPayload(text) => text.budget(),
            Self::Index(_) => None,
        }
    }

    pub fn as_field(&self) -> Option<&str> {
        match self {
            Self::Field(name) => Some(name),
            _ => None,
        }
    }

    pub const fn as_index(&self) -> Option<usize> {
        match self {
            Self::Index(index) => Some(*index),
            _ => None,
        }
    }

    pub fn as_map_key(&self) -> Option<&str> {
        match self {
            Self::MapKey(key) => Some(key),
            _ => None,
        }
    }

    pub fn as_enum_payload(&self) -> Option<&str> {
        match self {
            Self::EnumPayload(variant) => Some(variant),
            _ => None,
        }
    }
}

impl fmt::Debug for PathSegment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Field(name) => formatter
                .debug_struct("Field")
                .field("name_bytes", &name.len())
                .finish(),
            Self::Index(_) => formatter.write_str("Index(..)"),
            Self::MapKey(key) => formatter
                .debug_struct("MapKey")
                .field("key_bytes", &key.len())
                .finish(),
            Self::EnumPayload(variant) => formatter
                .debug_struct("EnumPayload")
                .field("variant_bytes", &variant.len())
                .finish(),
        }
    }
}

/// An ordered instance path; the root path has no segments.
pub struct InstancePath {
    segments: WorkspaceVec<PathSegment>,
}

impl InstancePath {
    /// Checks whether a name can use the Schema identifier shorthand in diagnostic paths.
    pub fn is_schema_identifier(name: &str) -> bool {
        let bytes = name.as_bytes();
        let Some(first) = bytes.first() else {
            return false;
        };
        if !matches!(*first, b'A'..=b'Z' | b'a'..=b'z' | b'_') {
            return false;
        }
        let mut index = 1;
        while let Some(byte) = bytes.get(index) {
            if !matches!(*byte, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_') {
                return false;
            }
            index += 1;
        }
        !matches!(
            name,
            "extern"
                | "mod"
                | "type"
                | "struct"
                | "entry"
                | "enum"
                | "option"
                | "map"
                | "i8"
                | "i16"
                | "i32"
                | "i64"
                | "i128"
                | "u8"
                | "u16"
                | "u32"
                | "u64"
                | "u128"
                | "f32"
                | "f64"
                | "string"
                | "bool"
                | "true"
                | "false"
                | "none"
        )
    }

    pub const fn unmetered() -> Self {
        Self {
            segments: WorkspaceVec::unmetered(),
        }
    }

    pub fn new<'slot>(
        budget: Option<&ExecutionBudget>,
        out: Out<'slot, '_, BudgetError>,
    ) -> OResult<'slot, Self> {
        let segments = WorkspaceVec::new(budget, out)?;
        Ok(Self { segments })
    }

    pub fn budget(&self) -> Option<&ExecutionBudget> {
        self.segments.budget()
    }

    pub fn copy<'slot>(&self, mut out: Out<'slot, '_, BudgetError>) -> OResult<'slot, Self> {
        let mut copied = Self::new(self.budget(), out.reborrow())?;
        for segment in self.segments() {
            copied.push_segment_copy(segment, out.reborrow())?;
        }
        Ok(copied)
    }

    pub fn push_index<'slot>(
        &mut self,
        index: usize,
        out: Out<'slot, '_, BudgetError>,
    ) -> OResult<'slot, ()> {
        self.push(PathSegment::Index(index), out)
    }

    pub fn push_field<'slot>(
        &mut self,
        name: &str,
        out: Out<'slot, '_, BudgetError>,
    ) -> OResult<'slot, ()> {
        self.push_text(name, PathSegment::Field, out)
    }

    pub fn push_map_key<'slot>(
        &mut self,
        key: &str,
        out: Out<'slot, '_, BudgetError>,
    ) -> OResult<'slot, ()> {
        self.push_text(key, PathSegment::MapKey, out)
    }

    pub fn push_payload<'slot>(
        &mut self,
        variant: &str,
        out: Out<'slot, '_, BudgetError>,
    ) -> OResult<'slot, ()> {
        self.push_text(variant, PathSegment::EnumPayload, out)
    }

    fn push_text<'slot>(
        &mut self,
        value: &str,
        build: fn(WorkspaceString) -> PathSegment,
        mut out: Out<'slot, '_, BudgetError>,
    ) -> OResult<'slot, ()> {
        let text = WorkspaceString::copy(value, self.budget(), out.reborrow())?;
        self.push(build(text), out.reborrow())
    }

    fn push_segment_copy<'slot>(
        &mut self,
        segment: &PathSegment,
        mut out: Out<'slot, '_, BudgetError>,
    ) -> OResult<'slot, ()> {
        match segment {
            PathSegment::Field(name) => self.push_text(name, PathSegment::Field, out.reborrow()),
            PathSegment::MapKey(key) => self.push_text(key, PathSegment::MapKey, out.reborrow()),
            PathSegment::EnumPayload(variant) => {
                self.push_text(variant, PathSegment::EnumPayload, out.reborrow())
            }
            PathSegment::Index(index) => self.push_index(*index, out.reborrow()),
        }
    }

    pub fn segments(&self) -> &[PathSegment] {
        &self.segments
    }

    pub fn len(&self) -> usize {
        self.segments.len()
    }

    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    pub fn push<'slot>(
        &mut self,
        segment: PathSegment,
        mut out: Out<'slot, '_, BudgetError>,
    ) -> OResult<'slot, ()> {
        let compatible = match segment {
            PathSegment::Index(_) => true,
            _ => same_budget(self.budget(), segment.budget()),
        };
        if !compatible {
            return Err(out.fail(BudgetError::invariant(&PATH_BUDGET_MISMATCH)));
        }
        self.segments.try_push(segment, out.reborrow())
    }

    pub fn pop(&mut self) -> Option<PathSegment> {
        self.segments.pop()
    }

    pub fn truncate(&mut self, len: usize) {
        self.segments.truncate(len);
    }
}

fn same_budget(left: Option<&ExecutionBudget>, right: Option<&ExecutionBudget>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left.same_call(right),
        (None, None) => true,
        _ => false,
    }
}

impl Default for InstancePath {
    fn default() -> Self {
        Self::unmetered()
    }
}

impl fmt::Debug for InstancePath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InstancePath")
            .field("segment_count", &self.segments.len())
            .finish()
    }
}
