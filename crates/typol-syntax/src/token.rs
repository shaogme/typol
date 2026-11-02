use core::fmt;
use opeo::{OResult, Out};
use typol_core::{
    BudgetError, ChargedString, Diagnostic, ExecutionBudget, LocatedSpan, SourceSpan,
};

/// A reserved word. Names are case-sensitive and match the Schema DSL lexer table.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Keyword {
    Extern,
    Mod,
    Type,
    Struct,
    Entry,
    Enum,
    Option,
    Map,
    I8,
    I16,
    I32,
    I64,
    I128,
    U8,
    U16,
    U32,
    U64,
    U128,
    F32,
    F64,
    String,
    Bool,
    True,
    False,
    None,
}

impl Keyword {
    pub(crate) fn from_identifier<'slot, E>(
        identifier: &str,
        budget: &ExecutionBudget,
        mut out: Out<'slot, '_, E>,
    ) -> OResult<'slot, Option<Self>>
    where
        E: From<BudgetError>,
    {
        for (text, kind) in [
            ("extern", Self::Extern),
            ("mod", Self::Mod),
            ("type", Self::Type),
            ("struct", Self::Struct),
            ("entry", Self::Entry),
            ("enum", Self::Enum),
            ("option", Self::Option),
            ("map", Self::Map),
            ("i8", Self::I8),
            ("i16", Self::I16),
            ("i32", Self::I32),
            ("i64", Self::I64),
            ("i128", Self::I128),
            ("u8", Self::U8),
            ("u16", Self::U16),
            ("u32", Self::U32),
            ("u64", Self::U64),
            ("u128", Self::U128),
            ("f32", Self::F32),
            ("f64", Self::F64),
            ("string", Self::String),
            ("bool", Self::Bool),
            ("true", Self::True),
            ("false", Self::False),
            ("none", Self::None),
        ] {
            budget.consume_with(1, out.reborrow(), E::from)?;
            if identifier.len() == text.len()
                && budget
                    .compare_text(identifier, text, out.reborrow())?
                    .is_eq()
            {
                return Ok(Some(kind));
            }
        }
        Ok(None)
    }
}

/// Punctuation and operators used in the DSL.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Symbol {
    PathSeparator,
    LeftParen,
    RightParen,
    LeftBrace,
    RightBrace,
    LeftBracket,
    RightBracket,
    Comma,
    Semicolon,
    Colon,
    Dot,
    At,
    Equal,
    Bang,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    EqualEqual,
    AndAnd,
    OrOr,
    Plus,
    Minus,
    Star,
    Slash,
    Pipe,
}

/// The lexical category of a token; its text is available through [`Token::text`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TokenKind {
    Eof,
    Identifier,
    Integer,
    Float,
    String,
    Keyword(Keyword),
    Symbol(Symbol),
}

/// A token stores only a name, raw number text, or decoded string; punctuation and reserved words have no payload.
pub struct Token {
    kind: TokenKind,
    span: LocatedSpan,
    text: Option<ChargedString>,
}

impl Token {
    pub(crate) const fn new(
        kind: TokenKind,
        span: LocatedSpan,
        text: Option<ChargedString>,
    ) -> Self {
        Self { kind, span, text }
    }

    pub const fn kind(&self) -> TokenKind {
        self.kind
    }

    pub const fn span(&self) -> SourceSpan {
        self.span.source_span()
    }

    pub const fn located_span(&self) -> LocatedSpan {
        self.span
    }

    pub fn text(&self) -> Option<&str> {
        if matches!(
            self.kind,
            TokenKind::Identifier | TokenKind::Integer | TokenKind::Float
        ) {
            self.text.as_ref().map(ChargedString::as_str)
        } else {
            None
        }
    }

    pub fn decoded_string(&self) -> Option<&str> {
        if self.kind == TokenKind::String {
            self.text.as_ref().map(ChargedString::as_str)
        } else {
            None
        }
    }

    pub fn into_decoded_string(self) -> Option<ChargedString> {
        if self.kind == TokenKind::String {
            self.text
        } else {
            None
        }
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Token")
            .field("kind", &self.kind)
            .field("span", &self.span)
            .field("text_bytes", &self.text.as_ref().map(|text| text.len()))
            .finish()
    }
}

#[derive(Debug)]
pub enum LexEvent {
    Token(Token),
    Error(Diagnostic),
    Fatal(Diagnostic),
    Eof(Token),
    Halted(SourceSpan),
}

impl LexEvent {
    pub fn token(&self) -> Option<&Token> {
        match self {
            Self::Token(token) | Self::Eof(token) => Some(token),
            _ => None,
        }
    }

    pub fn diagnostic(&self) -> Option<&Diagnostic> {
        match self {
            Self::Error(error) | Self::Fatal(error) => Some(error),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum AnnotationKind {
    Min,
    Max,
    Range,
    Length,
    Regex,
    Host,
    Email,
    Cidr,
    Env,
    Size,
    Unique,
    All,
    Sum,
    Secret,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum NumberKind {
    Integer,
    Float,
}
