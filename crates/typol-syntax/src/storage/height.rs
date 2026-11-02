//! Child references read only the cached height of completed nodes; lists and inline payloads do not add structural depth.

use crate::storage::{
    Annotation, AnnotationData, ArrayDefault, ArrayLength, Assertion, BooleanLiteral, BraceEntry,
    BraceKey, BracedDefault, Declaration, DefaultValue, DefaultValueKind, Edge, EnumDecl,
    EnumDefault, EnumDefaultPayload, EnumDefaultPayloadKind, Expr, ExprKind, ExternModDecl,
    FieldDecl, Id, ListId, MethodArgument, MethodArgumentKind, ModuleDecl, Name, NamedArgument,
    NamedBounds, Node, NumberLiteral, ScalarLiteral, SizeArguments, Storage, StringLiteral,
    StructDecl, TypeAliasDecl, TypeExpr, TypeExprKind, TypePath, VariantDecl, VariantPayload,
    VariantPayloadKind,
};
use opeo::{OResult, Out};
use typol_core::Diagnostic;

pub(crate) trait ChildHeight<T> {
    fn child_height<'slot, 'borrow>(
        value: &T,
        storage: &Storage,
        out: Out<'slot, 'borrow, Diagnostic>,
    ) -> OResult<'slot, Option<usize>>;
}

pub(crate) struct Height;

impl Height {
    pub(crate) fn children<'slot, T>(
        value: &T,
        storage: &Storage,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Option<usize>>
    where
        Self: ChildHeight<T>,
    {
        storage.execution().consume(1, out.reborrow())?;
        Self::child_height(value, storage, out.reborrow())
    }
}

impl<T> ChildHeight<Id<T>> for Height
where
    Id<T>: Node<Value = T>,
{
    fn child_height<'slot>(
        value: &Id<T>,
        storage: &Storage,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Option<usize>> {
        Ok(Some(storage.height_for_diagnostic(*value, out.reborrow())?))
    }
}

impl<T> ChildHeight<ListId<T>> for Height
where
    ListId<T>: Edge<Value = T>,
    Self: ChildHeight<T>,
{
    fn child_height<'slot>(
        value: &ListId<T>,
        storage: &Storage,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Option<usize>> {
        let mut height = None;
        let items = storage.list_for_diagnostic(*value, out.reborrow())?;
        for item in items {
            height = height.max(Self::children(item, storage, out.reborrow())?);
        }
        Ok(height)
    }
}

impl<T> ChildHeight<Option<T>> for Height
where
    Self: ChildHeight<T>,
{
    fn child_height<'slot>(
        value: &Option<T>,
        storage: &Storage,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Option<usize>> {
        match value {
            Some(value) => Self::children(value, storage, out.reborrow()),
            None => Ok(None),
        }
    }
}

impl<T> ChildHeight<NamedArgument<T>> for Height
where
    Self: ChildHeight<T>,
{
    fn child_height<'slot>(
        value: &NamedArgument<T>,
        storage: &Storage,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Option<usize>> {
        let name_height = storage.height_for_diagnostic::<Name>(value.name, out.reborrow())?;
        let value_height = Self::children(&value.value, storage, out.reborrow())?;
        Ok(Some(name_height).max(value_height))
    }
}
macro_rules! children {
    (@impl $ty:ident {}) => {
        impl ChildHeight<$ty> for Height {
            fn child_height<'slot, 'borrow>(
                _value: &$ty,
                _storage: &Storage,
                _out: Out<'slot, 'borrow, Diagnostic>,
            ) -> OResult<'slot, Option<usize>> {
                Ok(None)
            }
        }
    };
    (@impl $ty:ident { $($field:ident),+ }) => {
        impl ChildHeight<$ty> for Height {
            fn child_height<'slot>(
                _value: &$ty,
                _storage: &Storage,
                mut out: Out<'slot, '_, Diagnostic>,
            ) -> OResult<'slot, Option<usize>> {
                let height: Option<usize> = None;
                $(let height = height.max(Height::children(&_value.$field, _storage, out.reborrow())?);)+
                Ok(height)
            }
        }
    };
    ($($ty:ident { $($field:ident),* }),* $(,)?) => {
        $(children!(@impl $ty { $($field),* });)*
    };
}
children! {
    Name {}, NumberLiteral {}, StringLiteral {}, BooleanLiteral {}, ArrayLength {},
    Annotation { data }, Assertion { expression, message }, ExternModDecl { name },
    ModuleDecl { name, declarations }, TypeAliasDecl { name, target, annotations },
    StructDecl { name, fields, assertions }, FieldDecl { name, ty, annotations, default },
    EnumDecl { name, variants }, VariantDecl { name, payload, assertions },
    VariantPayload { kind }, MethodArgument { kind }, Expr { kind }, TypePath { segments },
    TypeExpr { kind }, BraceEntry { key, value }, ArrayDefault { elements },
    BracedDefault { entries }, EnumDefaultPayload { kind }, EnumDefault { variant, payload },
    DefaultValue { kind }, NamedBounds { first, second },
}

impl ChildHeight<Declaration> for Height {
    fn child_height<'slot>(
        value: &Declaration,
        storage: &Storage,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Option<usize>> {
        let height = match value {
            Declaration::Module(id) => Height::children(id, storage, out.reborrow())?,
            Declaration::TypeAlias(id) => Height::children(id, storage, out.reborrow())?,
            Declaration::Struct(id) => Height::children(id, storage, out.reborrow())?,
            Declaration::Enum(id) => Height::children(id, storage, out.reborrow())?,
        };
        Ok(height)
    }
}

impl ChildHeight<AnnotationData> for Height {
    fn child_height<'slot>(
        value: &AnnotationData,
        storage: &Storage,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Option<usize>> {
        let height = match value {
            AnnotationData::Min(id) | AnnotationData::Max(id) => {
                Height::children(id, storage, out.reborrow())?
            }
            AnnotationData::Range { min, max } | AnnotationData::Length { min, max } => {
                Height::children(min, storage, out.reborrow())?.max(Height::children(
                    max,
                    storage,
                    out.reborrow(),
                )?)
            }
            AnnotationData::Regex(id) | AnnotationData::Env(id) => {
                Height::children(id, storage, out.reborrow())?
            }
            AnnotationData::Cidr(argument) => Height::children(argument, storage, out.reborrow())?,
            AnnotationData::Size(arguments) => {
                Height::children(arguments, storage, out.reborrow())?
            }
            AnnotationData::Unique(argument) => {
                Height::children(argument, storage, out.reborrow())?
            }
            AnnotationData::All(assertion) => Height::children(assertion, storage, out.reborrow())?,
            AnnotationData::Sum(bounds) => Height::children(bounds, storage, out.reborrow())?,
            AnnotationData::Host | AnnotationData::Email | AnnotationData::Secret => None,
        };
        Ok(height)
    }
}

impl ChildHeight<SizeArguments> for Height {
    fn child_height<'slot>(
        value: &SizeArguments,
        storage: &Storage,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Option<usize>> {
        let height = match value {
            SizeArguments::Positional { min, max } => Height::children(
                min,
                storage,
                out.reborrow(),
            )?
            .max(Height::children(max, storage, out.reborrow())?),
            SizeArguments::Named(bounds) => Height::children(bounds, storage, out.reborrow())?,
        };
        Ok(height)
    }
}

impl ChildHeight<VariantPayloadKind> for Height {
    fn child_height<'slot>(
        value: &VariantPayloadKind,
        storage: &Storage,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Option<usize>> {
        let height = match value {
            VariantPayloadKind::Unit => None,
            VariantPayloadKind::Single(payload) => {
                Height::children(&payload.ty, storage, out.reborrow())?.max(Height::children(
                    &payload.annotations,
                    storage,
                    out.reborrow(),
                )?)
            }
            VariantPayloadKind::Structure(payload) => {
                Height::children(&payload.fields, storage, out.reborrow())?
            }
        };
        Ok(height)
    }
}

impl ChildHeight<MethodArgumentKind> for Height {
    fn child_height<'slot>(
        value: &MethodArgumentKind,
        storage: &Storage,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Option<usize>> {
        let height = match value {
            MethodArgumentKind::Expression(expression) => {
                Height::children(expression, storage, out.reborrow())?
            }
            MethodArgumentKind::Closure { parameter, body } => Height::children(
                parameter,
                storage,
                out.reborrow(),
            )?
            .max(Height::children(body, storage, out.reborrow())?),
        };
        Ok(height)
    }
}

impl ChildHeight<ExprKind> for Height {
    fn child_height<'slot>(
        value: &ExprKind,
        storage: &Storage,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Option<usize>> {
        let height = match value {
            ExprKind::Literal(literal) => Height::children(literal, storage, out.reborrow())?,
            ExprKind::None => None,
            ExprKind::Name(name) => Height::children(name, storage, out.reborrow())?,
            ExprKind::FieldAccess { receiver, field } => Height::children(
                receiver,
                storage,
                out.reborrow(),
            )?
            .max(Height::children(field, storage, out.reborrow())?),
            ExprKind::MethodCall {
                receiver,
                method,
                argument,
            } => Height::children(receiver, storage, out.reborrow())?
                .max(Height::children(method, storage, out.reborrow())?)
                .max(Height::children(argument, storage, out.reborrow())?),
            ExprKind::Unary { operand, .. } => Height::children(operand, storage, out.reborrow())?,
            ExprKind::Binary { left, right, .. } => Height::children(
                left,
                storage,
                out.reborrow(),
            )?
            .max(Height::children(right, storage, out.reborrow())?),
        };
        Ok(height)
    }
}

impl ChildHeight<TypeExprKind> for Height {
    fn child_height<'slot>(
        value: &TypeExprKind,
        storage: &Storage,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Option<usize>> {
        let height = match value {
            TypeExprKind::Builtin(_) => None,
            TypeExprKind::Option(inner) => Height::children(inner, storage, out.reborrow())?,
            TypeExprKind::Array { element, length } => Height::children(
                element,
                storage,
                out.reborrow(),
            )?
            .max(Height::children(length, storage, out.reborrow())?),
            TypeExprKind::Map { value } => Height::children(value, storage, out.reborrow())?,
            TypeExprKind::Path(path) => Height::children(path, storage, out.reborrow())?,
        };
        Ok(height)
    }
}

impl ChildHeight<ScalarLiteral> for Height {
    fn child_height<'slot>(
        value: &ScalarLiteral,
        storage: &Storage,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Option<usize>> {
        let height = match value {
            ScalarLiteral::String(id) => Height::children(id, storage, out.reborrow())?,
            ScalarLiteral::Number(id) => Height::children(id, storage, out.reborrow())?,
            ScalarLiteral::Boolean(id) => Height::children(id, storage, out.reborrow())?,
        };
        Ok(height)
    }
}

impl ChildHeight<BraceKey> for Height {
    fn child_height<'slot>(
        value: &BraceKey,
        storage: &Storage,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Option<usize>> {
        let height = match value {
            BraceKey::Identifier(id) => Height::children(id, storage, out.reborrow())?,
            BraceKey::String(id) => Height::children(id, storage, out.reborrow())?,
        };
        Ok(height)
    }
}

impl ChildHeight<EnumDefaultPayloadKind> for Height {
    fn child_height<'slot>(
        value: &EnumDefaultPayloadKind,
        storage: &Storage,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Option<usize>> {
        let height = match value {
            EnumDefaultPayloadKind::Unit => None,
            EnumDefaultPayloadKind::Single(id) => Height::children(id, storage, out.reborrow())?,
            EnumDefaultPayloadKind::Structure(id) => Height::children(id, storage, out.reborrow())?,
        };
        Ok(height)
    }
}

impl ChildHeight<DefaultValueKind> for Height {
    fn child_height<'slot>(
        value: &DefaultValueKind,
        storage: &Storage,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Option<usize>> {
        let height = match value {
            DefaultValueKind::Scalar(literal) => {
                Height::children(literal, storage, out.reborrow())?
            }
            DefaultValueKind::None => None,
            DefaultValueKind::Array(id) => Height::children(id, storage, out.reborrow())?,
            DefaultValueKind::Braced(id) => Height::children(id, storage, out.reborrow())?,
            DefaultValueKind::Enum(id) => Height::children(id, storage, out.reborrow())?,
        };
        Ok(height)
    }
}
