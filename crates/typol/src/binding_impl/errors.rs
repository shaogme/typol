use crate::{
    descriptor::MapContainer,
    diagnostics::{CodecErrors, CodecStage},
    session::SessionBudget,
};
use opeo::{OResult, Out, ResultOutExt};
use typol_core::{BudgetError, ChargedString, ChargedVec};
use typol_model::BuiltinType;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BindingMismatchKind {
    RootTypeName,
    TypeName,
    MissingField,
    ExtraField,
    FieldShape,
    MissingVariant,
    ExtraVariant,
    VariantPayload,
    MapContainer,
    TypeShape,
    ArrayLength,
    OptionNesting,
    DuplicateField,
    DuplicateVariant,
}

impl BindingMismatchKind {
    pub const fn message(self) -> &'static str {
        match self {
            Self::RootTypeName => "root type name mismatch",
            Self::TypeName => "type name mismatch",
            Self::MissingField => "schema field is missing from the Rust type",
            Self::ExtraField => "Rust field is not declared in the schema",
            Self::FieldShape => "field shape mismatch",
            Self::MissingVariant => "schema variant is missing from the Rust type",
            Self::ExtraVariant => "Rust variant is not declared in the schema",
            Self::VariantPayload => "enum payload shape mismatch",
            Self::MapContainer => "map container and schema shapes do not match",
            Self::TypeShape => "type shape mismatch",
            Self::ArrayLength => "fixed array length mismatch",
            Self::OptionNesting => "nested optional types are unsupported",
            Self::DuplicateField => "Rust type contains duplicate field names",
            Self::DuplicateVariant => "Rust type contains duplicate variant names",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BindingShape {
    Builtin(BuiltinType),
    Option,
    Sequence,
    Array { length: Option<u32> },
    Map { container: Option<MapContainer> },
    Structure,
    Enumeration,
    NamedType,
    UnitPayload,
    SinglePayload,
    StructurePayload,
    Missing,
    Extra,
    Duplicate,
}

impl BindingShape {
    pub const fn message(self) -> &'static str {
        match self {
            Self::Builtin(BuiltinType::I8) => "i8",
            Self::Builtin(BuiltinType::I16) => "i16",
            Self::Builtin(BuiltinType::I32) => "i32",
            Self::Builtin(BuiltinType::I64) => "i64",
            Self::Builtin(BuiltinType::I128) => "i128",
            Self::Builtin(BuiltinType::U8) => "u8",
            Self::Builtin(BuiltinType::U16) => "u16",
            Self::Builtin(BuiltinType::U32) => "u32",
            Self::Builtin(BuiltinType::U64) => "u64",
            Self::Builtin(BuiltinType::U128) => "u128",
            Self::Builtin(BuiltinType::F32) => "f32",
            Self::Builtin(BuiltinType::F64) => "f64",
            Self::Builtin(BuiltinType::String) => "string",
            Self::Builtin(BuiltinType::Bool) => "bool",
            Self::Option => "optional type",
            Self::Sequence => "sequence",
            Self::Array { .. } => "fixed array",
            Self::Map { container: None } => "map",
            Self::Map {
                container: Some(MapContainer::HashMap),
            } => "HashMap",
            Self::Map {
                container: Some(MapContainer::BTreeMap),
            } => "BTreeMap",
            Self::Structure => "struct",
            Self::Enumeration => "enum",
            Self::NamedType => "named field",
            Self::UnitPayload => "unit variant",
            Self::SinglePayload => "single-value payload",
            Self::StructurePayload => "struct payload",
            Self::Missing => "missing",
            Self::Extra => "extra",
            Self::Duplicate => "duplicate",
        }
    }
}

pub struct BindingMismatch {
    kind: BindingMismatchKind,
    schema_path: ChargedString,
    rust_path: ChargedString,
    expected: BindingShape,
    actual: BindingShape,
}

impl BindingMismatch {
    pub const fn kind(&self) -> BindingMismatchKind {
        self.kind
    }

    pub fn schema_path(&self) -> &str {
        self.schema_path.as_str()
    }

    pub fn rust_path(&self) -> &str {
        self.rust_path.as_str()
    }

    pub const fn expected(&self) -> BindingShape {
        self.expected
    }

    pub const fn actual(&self) -> BindingShape {
        self.actual
    }
}

pub struct BindingErrors {
    mismatches: ChargedVec<BindingMismatch>,
    truncated: bool,
}

impl BindingErrors {
    pub fn as_slice(&self) -> &[BindingMismatch] {
        self.mismatches.as_slice()
    }

    pub fn first(&self) -> Option<&BindingMismatch> {
        self.mismatches.first()
    }

    pub fn get(&self, index: usize) -> Option<&BindingMismatch> {
        self.mismatches.get(index)
    }

    pub fn len(&self) -> usize {
        self.mismatches.len()
    }

    pub fn is_empty(&self) -> bool {
        self.mismatches.is_empty()
    }

    /// Returns whether binding errors were truncated by the diagnostic limit.
    pub const fn is_truncated(&self) -> bool {
        self.truncated
    }

    pub(crate) fn new(mismatches: ChargedVec<BindingMismatch>, truncated: bool) -> Self {
        Self {
            mismatches,
            truncated,
        }
    }
}

pub(crate) struct BindingPath {
    schema: ChargedString,
    rust: ChargedString,
}

pub(crate) struct PathSegments<'a> {
    separator: &'static str,
    schema: &'a str,
    rust: &'a str,
}

pub(crate) struct MismatchDetails {
    pub(crate) kind: BindingMismatchKind,
    pub(crate) expected: BindingShape,
    pub(crate) actual: BindingShape,
}

impl<'a> PathSegments<'a> {
    pub(crate) const fn same(separator: &'static str, segment: &'a str) -> Self {
        Self {
            separator,
            schema: segment,
            rust: segment,
        }
    }

    pub(crate) const fn different(separator: &'static str, schema: &'a str, rust: &'a str) -> Self {
        Self {
            separator,
            schema,
            rust,
        }
    }
}

pub(crate) struct BindingErrorCollector {
    mismatches: ChargedVec<BindingMismatch>,
    mismatch_limit: usize,
    single_diagnostic: bool,
    truncated: bool,
}

impl BindingErrorCollector {
    pub(crate) fn new<'slot>(
        diagnostic_limit: usize,
        session: &SessionBudget,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, Self> {
        let diagnostic_limit = diagnostic_limit.max(1);
        let single_diagnostic = diagnostic_limit == 1;
        let mismatch_limit = if single_diagnostic {
            1
        } else {
            diagnostic_limit - 1
        };
        let mismatches = session.budget_codec(
            CodecStage::Binding,
            |budget_out| ChargedVec::new(Some(session.execution()), budget_out),
            out.reborrow(),
        )?;
        Ok(Self {
            mismatches,
            mismatch_limit,
            single_diagnostic,
            truncated: false,
        })
    }

    pub(crate) fn record<'slot>(
        &mut self,
        path: BindingPath,
        details: MismatchDetails,
        session: &SessionBudget,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ()> {
        if self.truncated {
            return Ok(());
        }
        if self.mismatches.len() >= self.mismatch_limit {
            self.truncated = true;
            return Ok(());
        }
        let mismatch = path.into_mismatch(details);
        session.budget_codec(
            CodecStage::Binding,
            |budget_out| self.mismatches.try_push(mismatch, budget_out),
            out.reborrow(),
        )?;
        self.truncated = self.single_diagnostic;
        Ok(())
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.mismatches.is_empty()
    }

    pub(crate) const fn is_truncated(&self) -> bool {
        self.truncated
    }

    pub(crate) fn into_errors(self) -> BindingErrors {
        BindingErrors::new(self.mismatches, self.truncated)
    }
}

impl BindingPath {
    pub(crate) fn root<'slot>(
        session: &SessionBudget,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, Self> {
        let schema = session.budget_codec(
            CodecStage::Binding,
            |budget_out| session.copy_text("$", budget_out),
            out.reborrow(),
        )?;
        let rust = session.budget_codec(
            CodecStage::Binding,
            |budget_out| session.copy_text("$", budget_out),
            out.reborrow(),
        )?;
        Ok(Self { schema, rust })
    }

    pub(crate) fn extend<'slot>(
        &self,
        session: &SessionBudget,
        segments: PathSegments<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, Self> {
        let schema = extend_path(
            &self.schema,
            segments.separator,
            segments.schema,
            session,
            out.reborrow(),
        )?;
        let rust = extend_path(
            &self.rust,
            segments.separator,
            segments.rust,
            session,
            out.reborrow(),
        )?;
        Ok(Self { schema, rust })
    }

    pub(crate) fn duplicate<'slot>(
        &self,
        session: &SessionBudget,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, Self> {
        let schema = session.budget_codec(
            CodecStage::Binding,
            |budget_out| session.copy_text(self.schema.as_str(), budget_out),
            out.reborrow(),
        )?;
        let rust = session.budget_codec(
            CodecStage::Binding,
            |budget_out| session.copy_text(self.rust.as_str(), budget_out),
            out.reborrow(),
        )?;
        Ok(Self { schema, rust })
    }

    pub(crate) fn into_mismatch(self, details: MismatchDetails) -> BindingMismatch {
        BindingMismatch {
            kind: details.kind,
            schema_path: self.schema,
            rust_path: self.rust,
            expected: details.expected,
            actual: details.actual,
        }
    }
}

fn extend_path<'slot>(
    base: &str,
    separator: &str,
    segment: &str,
    session: &SessionBudget,
    out: Out<'slot, '_, CodecErrors>,
) -> OResult<'slot, ChargedString> {
    session.budget_codec(
        CodecStage::Binding,
        |mut budget_out| {
            let separator = if segment.is_empty() { "" } else { separator };
            let capacity = base
                .len()
                .checked_add(separator.len())
                .and_then(|length| length.checked_add(segment.len()))
                .ok_or(BudgetError::CapacityExceeded)
                .store_err(budget_out.reborrow())?;
            session.execution().construct_text(
                capacity,
                session.max_text_bytes(),
                budget_out.reborrow(),
            )?;
            session
                .execution()
                .consume(capacity, budget_out.reborrow())?;
            let mut path = ChargedString::with_capacity(
                capacity,
                Some(session.execution()),
                budget_out.reborrow(),
            )?;
            path.try_push_str(base, budget_out.reborrow())?;
            path.try_push_str(separator, budget_out.reborrow())?;
            path.try_push_str(segment, budget_out.reborrow())?;
            Ok(path)
        },
        out,
    )
}
