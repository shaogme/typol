use typol_core::{ChargedVec, SourceSpan};
use typol_regex::Regex;
/// A compiled regex and the source span of its `@regex` annotation.
pub struct CompiledRegexConstraint {
    span: SourceSpan,
    regex: Regex,
}

impl CompiledRegexConstraint {
    #[cfg(feature = "compiler")]
    pub fn new(span: SourceSpan, regex: Regex) -> Self {
        Self { span, regex }
    }

    pub const fn span(&self) -> SourceSpan {
        self.span
    }

    pub const fn regex(&self) -> &Regex {
        &self.regex
    }
}

/// An owned collection of all `@regex` constraints in one static Schema compilation.
pub struct CompiledRegexConstraints {
    constraints: Option<ChargedVec<CompiledRegexConstraint>>,
    pattern_bytes: usize,
    instruction_count: usize,
    range_count: usize,
}

impl CompiledRegexConstraints {
    #[cfg(feature = "compiler")]
    pub fn new(
        constraints: ChargedVec<CompiledRegexConstraint>,
        pattern_bytes: usize,
        instruction_count: usize,
        range_count: usize,
    ) -> Self {
        Self {
            constraints: Some(constraints),
            pattern_bytes,
            instruction_count,
            range_count,
        }
    }

    #[cfg(feature = "compiler")]
    pub fn empty() -> Self {
        Self {
            constraints: None,
            pattern_bytes: 0,
            instruction_count: 0,
            range_count: 0,
        }
    }

    pub fn constraints(&self) -> &[CompiledRegexConstraint] {
        match &self.constraints {
            None => &[],
            Some(values) => values,
        }
    }

    pub fn regex_for(&self, annotation_span: SourceSpan) -> Option<&Regex> {
        self.constraints()
            .iter()
            .find(|constraint| constraint.span == annotation_span)
            .map(CompiledRegexConstraint::regex)
    }

    pub fn pattern_bytes(&self) -> usize {
        self.pattern_bytes
    }

    pub fn instruction_count(&self) -> usize {
        self.instruction_count
    }

    pub fn range_count(&self) -> usize {
        self.range_count
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RegexId(usize);

impl RegexId {
    #[cfg(feature = "compiler")]
    pub const fn new(index: usize) -> Self {
        Self(index)
    }

    pub const fn index(self) -> usize {
        self.0
    }
}
