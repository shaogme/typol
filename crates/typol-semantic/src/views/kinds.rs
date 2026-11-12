use crate::{
    CompiledSchema, NumberValue, VariantId, VariantRestrictionMode,
    regex::CompiledRegexConstraint,
    views::{ExpressionView, FieldList, TypeView, VariantList},
};
use core::ptr;
use typol_core::SourceSpan;
use typol_model::BuiltinType;
use typol_regex::Regex;

#[derive(Clone, Copy)]
pub enum TypeKind<'a> {
    Builtin(BuiltinType),
    Option {
        inner: TypeView<'a>,
    },
    Array {
        element: TypeView<'a>,
        length: Option<u32>,
    },
    Map {
        value: TypeView<'a>,
    },
    Struct {
        fields: FieldList<'a>,
    },
    Enum {
        variants: VariantList<'a>,
    },
}

#[derive(Clone, Copy)]
pub enum VariantPayloadInfo<'a> {
    Unit,
    Single { ty: TypeView<'a> },
    Structure { fields: FieldList<'a> },
}

#[derive(Clone, Copy)]
pub struct RegexView<'a> {
    schema: &'a CompiledSchema,
    raw: &'a CompiledRegexConstraint,
}

impl<'a> RegexView<'a> {
    pub(crate) fn new(schema: &'a CompiledSchema, raw: &'a CompiledRegexConstraint) -> Self {
        Self { schema, raw }
    }

    pub fn regex(self) -> &'a Regex {
        self.raw.regex()
    }

    pub fn span(self) -> SourceSpan {
        self.raw.span()
    }
}

impl PartialEq for RegexView<'_> {
    fn eq(&self, other: &Self) -> bool {
        ptr::eq(self.schema, other.schema) && ptr::eq(self.raw, other.raw)
    }
}

impl Eq for RegexView<'_> {}

pub enum ConstraintData<'a> {
    NumberBounds {
        min: Option<NumberValue>,
        max: Option<NumberValue>,
    },
    LengthBounds {
        min: u64,
        max: u64,
    },
    Regex {
        program: RegexView<'a>,
    },
    Host,
    Email,
    Cidr {
        v4_only: bool,
    },
    SizeBounds {
        min: Option<u64>,
        max: Option<u64>,
    },
    Unique {
        by: Option<FieldList<'a>>,
    },
    All {
        expression: ExpressionView<'a>,
        message: &'a str,
    },
    SumBounds {
        min: Option<NumberValue>,
        max: Option<NumberValue>,
    },
    VariantRestriction {
        mode: VariantRestrictionMode,
        variants: &'a [VariantId],
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RegexCounts {
    pub programs: usize,
    pub pattern_bytes: usize,
    pub instructions: usize,
    pub ranges: usize,
}
