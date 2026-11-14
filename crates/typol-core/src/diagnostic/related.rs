use crate::{LocatedSpan, SourceSpan, storage::WorkspaceString};
use core::fmt;

/// Semantic meaning of a source location related to a diagnostic.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelatedLocationRole {
    PreviousDeclaration,
    FirstDeclaration,
    TypeOrFieldDefinition,
    ConstraintDefinition,
    AssociatedInput,
}

pub struct DiagnosticLabelInput<'text, S = LocatedSpan> {
    pub span: S,
    pub text: &'text str,
    pub role: RelatedLocationRole,
}

impl RelatedLocationRole {
    pub const fn id(self) -> &'static str {
        match self {
            Self::PreviousDeclaration => "previous_declaration",
            Self::FirstDeclaration => "first_declaration",
            Self::TypeOrFieldDefinition => "type_or_field_definition",
            Self::ConstraintDefinition => "constraint_definition",
            Self::AssociatedInput => "associated_input",
        }
    }

    pub const fn is_declaration(self) -> bool {
        matches!(self, Self::PreviousDeclaration | Self::FirstDeclaration)
    }

    pub const fn is_definition(self) -> bool {
        matches!(
            self,
            Self::TypeOrFieldDefinition | Self::ConstraintDefinition
        )
    }
}

/// Associates a source location with its semantic role and safe display description.
pub struct RelatedLocation {
    span: LocatedSpan,
    message: WorkspaceString,
    role: RelatedLocationRole,
}

impl RelatedLocation {
    pub(crate) fn new(
        span: LocatedSpan,
        message: WorkspaceString,
        role: RelatedLocationRole,
    ) -> Self {
        Self {
            span,
            message,
            role,
        }
    }

    pub const fn span(&self) -> SourceSpan {
        self.span.source_span()
    }

    pub const fn located_span(&self) -> LocatedSpan {
        self.span
    }

    pub fn message(&self) -> &str {
        self.message.as_str()
    }

    pub const fn role(&self) -> RelatedLocationRole {
        self.role
    }
}

impl fmt::Debug for RelatedLocation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RelatedLocation")
            .field("span", &self.span)
            .field("role", &self.role)
            .field("message_bytes", &self.message.len())
            .finish()
    }
}

pub type DiagnosticLabel = RelatedLocation;
