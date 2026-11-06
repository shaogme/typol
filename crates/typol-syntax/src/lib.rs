#![no_std]

extern crate alloc;

mod ast;
mod cursor;
mod error;
mod formatter;
mod input;
mod lexer;
mod parser;
mod storage;
mod token;

pub use opeo;

pub use ast::{
    Annotation, AnnotationData, AnnotationIter, AnnotationKind, AnnotationList, ArrayDefault,
    ArrayLength, Assertion, AssertionIter, AssertionList, AstAccessError, BinaryOperator,
    BooleanLiteral, BraceEntry, BraceEntryIter, BraceEntryList, BraceKey, BracedDefault,
    BuiltinType, Declaration, DeclarationIter, DeclarationList, DefaultValue, DefaultValueIter,
    DefaultValueKind, DefaultValueList, EnumDecl, EnumDefault, EnumDefaultPayload,
    EnumDefaultPayloadKind, Expr, ExprKind, ExternModDecl, FieldDecl, FieldDeclIter, FieldDeclList,
    MethodArgument, MethodArgumentKind, ModuleDecl, Name, NameIter, NameList, NamedArgument,
    NamedBounds, NumberKind, NumberLiteral, ScalarLiteral, SchemaAst, SingleVariantPayload,
    SizeArguments, StringLiteral, StructDecl, StructureVariantPayload, TopLevelItem,
    TopLevelItemIter, TopLevelItemList, TypeAliasDecl, TypeExpr, TypeExprKind, TypePath,
    UnaryOperator, VariantDecl, VariantDeclIter, VariantDeclList, VariantPayload,
    VariantPayloadKind,
};
pub use cursor::TokenCursor;
pub use error::{SyntaxDiagnosticIter, SyntaxErrors};
pub use formatter::{
    FormattedSchema, SchemaDocument, SchemaFormatError, SchemaFormatErrorKind, SchemaFormatOptions,
    SchemaFormatter,
};
pub use input::{InputConfig, InputLimits, StreamLimits, StreamLimitsError, SyntaxLimits};
pub use parser::{SchemaParseOptions, SchemaParseOutput, SchemaParser, SourcePositionIndex};
pub use token::{Keyword, LexEvent, Symbol, Token, TokenKind};
