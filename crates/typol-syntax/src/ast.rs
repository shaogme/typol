//! An owning Schema syntax tree with text and nodes stored in separate tables.
mod annotation;
mod declaration;
mod expression;
mod lists;
mod type_expr;
mod value;

use crate::storage::{
    Declaration as StoredDeclaration, Storage, TopLevelItem as StoredTopLevelItem,
};
pub use annotation::{
    Annotation, AnnotationData, AnnotationKind, Assertion, NamedArgument, NamedBounds,
    SizeArguments,
};
use core::{
    error::Error,
    fmt::{Debug, Display, Formatter, Result as FmtResult},
    mem,
};
pub use declaration::{
    Declaration, EnumDecl, ExternModDecl, FieldDecl, ModuleDecl, SingleVariantPayload, StructDecl,
    StructureVariantPayload, TopLevelItem, TypeAliasDecl, VariantDecl, VariantPayload,
    VariantPayloadKind,
};
pub use expression::{
    BinaryOperator, Expr, ExprKind, MethodArgument, MethodArgumentKind, UnaryOperator,
};
pub use lists::{
    AnnotationIter, AnnotationList, AssertionIter, AssertionList, BraceEntryIter, BraceEntryList,
    DeclarationIter, DeclarationList, DefaultValueIter, DefaultValueList, FieldDeclIter,
    FieldDeclList, NameIter, NameList, StringLiteralIter, StringLiteralList, TopLevelItemIter,
    TopLevelItemList, VariantDeclIter, VariantDeclList,
};
use opeo::{OResult, Out};
pub use type_expr::{ArrayLength, BuiltinType, TypeExpr, TypeExprKind, TypePath};
use typol_core::{ChargedVec, SourceSpan};
pub use value::{
    ArrayDefault, BooleanLiteral, BraceEntry, BraceKey, BracedDefault, DefaultValue,
    DefaultValueKind, EnumDefault, EnumDefaultPayload, EnumDefaultPayloadKind, Name, NumberKind,
    NumberLiteral, ScalarLiteral, StringLiteral,
};

/// A corruption state encountered while a read-only tree view resolves an internal reference.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AstAccessError {
    InvalidNodeId { id: usize },
    InvalidListRange { start: usize, end: usize },
    InvalidTextRange { start: usize, end: usize },
    InvalidDecodedTextId { id: usize },
}

impl AstAccessError {
    pub(crate) const fn invalid_list_range(start: usize, end: usize) -> Self {
        Self::InvalidListRange { start, end }
    }
}

impl Display for AstAccessError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        match self {
            Self::InvalidNodeId { id } => write!(formatter, "syntax tree node ID {id} is invalid"),
            Self::InvalidListRange { start, end } => {
                write!(
                    formatter,
                    "syntax tree list range [{start}, {end}) is invalid"
                )
            }
            Self::InvalidTextRange { start, end } => {
                write!(
                    formatter,
                    "syntax tree text range [{start}, {end}) is invalid"
                )
            }
            Self::InvalidDecodedTextId { id } => {
                write!(formatter, "syntax tree decoded-text ID {id} is invalid")
            }
        }
    }
}

impl Error for AstAccessError {}

/// Owns syntax nodes and a text pool; all child views borrow this tree, so the source text can be released after parsing.
///
/// Created by `SchemaParser::parse`. Node tables, internal IDs, and constructors are private; child access can only resolve
/// records owned by this tree. Lists provide `get` and allocation-free iteration; invalid references return an access error.
///
/// Node views cannot outlive their owning tree:
/// ```compile_fail
/// use typol_core::{ExecutionBudget, ExecutionLimits, SourceId};
/// use crate::SchemaParseOptions;
/// use typol_syntax::{AstAccessError, SchemaParser, SyntaxErrors};
/// use typol_syntax::opeo::ErrSlot;
/// let budget = ExecutionBudget::new(ExecutionLimits::new(1_000_000, 1_000_000)).unwrap();
/// let mut parse_errors = ErrSlot::<SyntaxErrors>::new();
/// let ast = parse_errors
///     .try_call(|out| SchemaParser::parse_str(SourceId::new(0), "entry struct Root {}", &SchemaParseOptions::default(), &budget, out))
///     .unwrap();
/// let mut access_errors = ErrSlot::<AstAccessError>::new();
/// let entry = access_errors
///     .try_call(|out| ast.items().first(out))
///     .unwrap()
///     .unwrap()
///     .entry_struct()
///     .unwrap();
/// drop(ast);
/// assert_eq!(access_errors.try_call(|out| entry.name(out)).unwrap(), "Root");
/// ```
///
/// Callers cannot use node views to assemble a new owned tree:
/// ```compile_fail
/// use typol_core::{ExecutionBudget, ExecutionLimits, SourceId};
/// use crate::SchemaParseOptions;
/// use typol_syntax::{SchemaAst, SchemaParser, SyntaxErrors};
/// use typol_syntax::opeo::ErrSlot;
/// let budget = ExecutionBudget::new(ExecutionLimits::new(1_000_000, 1_000_000)).unwrap();
/// let mut parse_errors = ErrSlot::<SyntaxErrors>::new();
/// let ast = parse_errors
///     .try_call(|out| SchemaParser::parse_str(SourceId::new(0), "", &SchemaParseOptions::default(), &budget, out))
///     .unwrap();
/// let forged = SchemaAst { span: ast.span(), ..ast };
/// ```
pub struct SchemaAst {
    storage: Storage,
    items: ChargedVec<StoredTopLevelItem>,
    span: SourceSpan,
}

impl SchemaAst {
    pub(crate) fn finish(
        storage: Storage,
        items: ChargedVec<StoredTopLevelItem>,
        span: SourceSpan,
    ) -> Self {
        Self {
            storage,
            items,
            span,
        }
    }

    /// The number of arena nodes actually inserted; text, edge tables, and file roots are not counted.
    pub fn node_count(&self) -> usize {
        self.storage.node_count()
    }

    pub(crate) fn memory_bytes(&self) -> Option<usize> {
        self.storage.memory_bytes()?.checked_add(
            self.items
                .capacity()
                .checked_mul(mem::size_of::<StoredTopLevelItem>())?,
        )
    }

    /// File roots and lists are transparent containers; leaf nodes have height 0, and each parent-child edge adds 1.
    pub fn structure_height<'slot>(
        &self,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, usize> {
        let mut height = 0;
        for item in &self.items {
            let item_height = match item {
                StoredTopLevelItem::ExternMod(id) => self.storage.height(*id, out.reborrow())?,
                StoredTopLevelItem::Declaration(declaration) => match declaration {
                    StoredDeclaration::Module(id) => self.storage.height(*id, out.reborrow())?,
                    StoredDeclaration::TypeAlias(id) => self.storage.height(*id, out.reborrow())?,
                    StoredDeclaration::Struct(id) => self.storage.height(*id, out.reborrow())?,
                    StoredDeclaration::Enum(id) => self.storage.height(*id, out.reborrow())?,
                },
                StoredTopLevelItem::EntryStruct(id) => self.storage.height(*id, out.reborrow())?,
            };
            height = height.max(item_height);
        }
        Ok(height)
    }

    pub fn items(&self) -> TopLevelItemList<'_> {
        TopLevelItemList::project(&self.storage, &self.items)
    }

    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

impl Debug for SchemaAst {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct("SchemaAst")
            .field("span", &self.span)
            .finish_non_exhaustive()
    }
}
