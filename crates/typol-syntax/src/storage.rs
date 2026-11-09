//! Private node, child, and text tables; growth failures always return a diagnostic.
mod constructors;
mod expression;
mod height;
mod ids;
mod records;
mod text;
mod type_expr;

#[cfg(test)]
mod tests;

use crate::SyntaxLimits;
pub(crate) use crate::{
    AnnotationKind, NumberKind,
    ast::AstAccessError,
    error::{
        AST_STORAGE_ARRAY_LENGTH_NUMBER, AST_STORAGE_DECODED_TEXT_BUDGET,
        AST_STORAGE_RESOLVE_NODE_SPAN, AST_STORAGE_UPDATE_EXPR_SPAN, SyntaxDiagnostic,
    },
};
pub(crate) use constructors::StorageConstructor;
use core::mem;
pub(crate) use expression::{BinaryExpressionParts, ExpressionBuilder, MethodCallParts};
use height::{ChildHeight, Height};
pub(crate) use ids::{Edge, Id, ListId, Node, Stored};
use opeo::{OResult, Out};
pub(crate) use records::{
    Annotation, AnnotationData, ArrayDefault, ArrayLength, Assertion, BooleanLiteral, BraceEntry,
    BraceKey, BracedDefault, Declaration, DefaultValue, DefaultValueKind, EnumDecl, EnumDefault,
    EnumDefaultPayload, EnumDefaultPayloadKind, Expr, ExprKind, ExternModDecl, FieldDecl,
    FieldParams, MethodArgument, MethodArgumentKind, ModuleDecl, Name, NamedArgument, NamedBounds,
    NumberLiteral, NumberLiteralParams, ScalarLiteral, SingleVariantPayload, SizeArguments,
    StringLiteral, StructDecl, StructDeclParams, StructureVariantPayload, TopLevelItem,
    TypeAliasDecl, TypeAliasDeclParams, TypeExpr, TypeExprKind, TypePath, VariantDecl,
    VariantDeclParams, VariantPayload, VariantPayloadKind,
};
pub(crate) use text::{TextRef, TextStorage};
pub(crate) use type_expr::{ArrayTypeExprParts, TypeBuilder};
use typol_core::{
    ChargedString, ChargedVec, Diagnostic, DiagnosticCode, ExecutionBudget, SourceId, SourceSpan,
    Span,
};
pub use typol_model::{BinaryOperator, BuiltinType, UnaryOperator};

pub(crate) struct Storage {
    annotation_edges: ChargedVec<Id<Annotation>>,
    assertion_edges: ChargedVec<Id<Assertion>>,
    brace_entry_edges: ChargedVec<Id<BraceEntry>>,
    declaration_edges: ChargedVec<Declaration>,
    default_value_edges: ChargedVec<Id<DefaultValue>>,
    field_decl_edges: ChargedVec<Id<FieldDecl>>,
    name_edges: ChargedVec<Id<Name>>,
    variant_decl_edges: ChargedVec<Id<VariantDecl>>,
    source_id: SourceId,
    source_limit: usize,
    actual_source_len: Option<usize>,
    final_source_len: Option<usize>,
    node_count: usize,
    node_limit: usize,
    height_limit: usize,
    execution: ExecutionBudget,
    text: TextStorage,
    annotation: ChargedVec<Stored<Annotation>>,
    array_default: ChargedVec<Stored<ArrayDefault>>,
    array_length: ChargedVec<Stored<ArrayLength>>,
    assertion: ChargedVec<Stored<Assertion>>,
    boolean_literal: ChargedVec<Stored<BooleanLiteral>>,
    brace_entry: ChargedVec<Stored<BraceEntry>>,
    braced_default: ChargedVec<Stored<BracedDefault>>,
    default_value: ChargedVec<Stored<DefaultValue>>,
    enum_decl: ChargedVec<Stored<EnumDecl>>,
    enum_default: ChargedVec<Stored<EnumDefault>>,
    enum_default_payload: ChargedVec<Stored<EnumDefaultPayload>>,
    expr: ChargedVec<Stored<Expr>>,
    extern_mod_decl: ChargedVec<Stored<ExternModDecl>>,
    field_decl: ChargedVec<Stored<FieldDecl>>,
    method_argument: ChargedVec<Stored<MethodArgument>>,
    module_decl: ChargedVec<Stored<ModuleDecl>>,
    name: ChargedVec<Stored<Name>>,
    number_literal: ChargedVec<Stored<NumberLiteral>>,
    string_literal: ChargedVec<Stored<StringLiteral>>,
    struct_decl: ChargedVec<Stored<StructDecl>>,
    type_alias_decl: ChargedVec<Stored<TypeAliasDecl>>,
    type_expr: ChargedVec<Stored<TypeExpr>>,
    type_path: ChargedVec<Stored<TypePath>>,
    variant_decl: ChargedVec<Stored<VariantDecl>>,
    variant_payload: ChargedVec<Stored<VariantPayload>>,
}

fn capacity_bytes<T>(values: &ChargedVec<T>) -> Option<usize> {
    values.capacity().checked_mul(mem::size_of::<T>())
}

impl Storage {
    pub(crate) fn memory_bytes(&self) -> Option<usize> {
        let mut bytes = 0usize;
        macro_rules! add_capacities { ($total:ident, $storage:ident; $($field:ident),+ $(,)?) => { $($total = $total.checked_add(capacity_bytes(&$storage.$field)?)?;)+ }; }
        add_capacities!(bytes, self;
            annotation_edges, assertion_edges, brace_entry_edges, declaration_edges,
            default_value_edges, field_decl_edges, name_edges, variant_decl_edges,
            annotation, array_default, array_length, assertion, boolean_literal, brace_entry,
            braced_default, default_value, enum_decl, enum_default, enum_default_payload,
            expr, extern_mod_decl, field_decl, method_argument, module_decl, name,
            number_literal, string_literal, struct_decl, type_alias_decl, type_expr, type_path,
            variant_decl, variant_payload,
        );
        bytes.checked_add(self.text.memory_bytes()?)
    }

    pub(crate) fn execution(&self) -> &ExecutionBudget {
        &self.execution
    }

    pub(crate) fn list<'slot, 'storage, T>(
        &'storage self,
        range: ListId<T>,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, &'storage [T]>
    where
        ListId<T>: Edge<Value = T>,
    {
        let start = range.start();
        let end = range.end();
        let Some(values) = ListId::<T>::table(self).get(start..end) else {
            return Err(out
                .reborrow()
                .fail(AstAccessError::invalid_list_range(start, end)));
        };
        Ok(values)
    }

    pub(crate) fn list_for_diagnostic<'slot, 'storage, T>(
        &'storage self,
        range: ListId<T>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, &'storage [T]>
    where
        ListId<T>: Edge<Value = T>,
    {
        let start = range.start();
        let end = range.end();
        let Some(values) = ListId::<T>::table(self).get(start..end) else {
            return Err(out.reborrow().fail(SyntaxDiagnostic::ast_access(
                AstAccessError::InvalidListRange { start, end },
            )));
        };
        Ok(values)
    }

    pub(crate) fn store_list<'slot, T>(
        &mut self,
        values: ChargedVec<T>,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ListId<T>>
    where
        ListId<T>: Edge<Value = T>,
        Height: ChildHeight<T>,
    {
        self.execution.consume(1, out.reborrow())?;
        self.check_span(span, out.reborrow())?;
        self.check_node_budget(span, out.reborrow())?;
        self.execution.consume(values.len(), out.reborrow())?;
        let mut children = None;
        for value in values.iter() {
            children = children.max(Height::children(value, self, out.reborrow())?);
        }
        if let Some(child) = children {
            if child >= self.height_limit {
                return Err(out.reborrow().fail(SyntaxDiagnostic::emergency(
                    DiagnosticCode::RESOURCE_LIMIT_EXCEEDED,
                    span,
                )));
            }
        }
        let table = ListId::<T>::table_mut(self);
        SyntaxDiagnostic::try_reserve(table, values.len(), span, out.reborrow())?;
        let start = table.len();
        for value in values {
            SyntaxDiagnostic::try_push(table, value, span, out.reborrow())?;
        }
        Ok(ListId::new(start, table.len()))
    }

    pub(crate) fn get<'slot, 'storage, T>(
        &'storage self,
        id: Id<T>,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, &'storage T>
    where
        Id<T>: Node<Value = T>,
    {
        let index = id.index();
        let Some(stored) = Id::<T>::table(self).get(index) else {
            return Err(out
                .reborrow()
                .fail(AstAccessError::InvalidNodeId { id: index }));
        };
        Ok(&stored.value)
    }

    pub(crate) fn span<'slot, T>(
        &self,
        id: Id<T>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, SourceSpan>
    where
        Id<T>: Node<Value = T>,
    {
        let index = id.index();
        let Some(stored) = Id::<T>::table(self).get(index) else {
            return Err(out
                .reborrow()
                .fail(SyntaxDiagnostic::internal_object_at_end(
                    AST_STORAGE_RESOLVE_NODE_SPAN,
                    self.source_id,
                    self.final_source_len.unwrap_or(self.source_limit),
                    index,
                )));
        };
        let span = Id::<T>::span(&stored.value);
        if self
            .final_source_len
            .is_some_and(|source_len| span.span().end() > source_len)
        {
            return Err(out.reborrow().fail(SyntaxDiagnostic::emergency(
                DiagnosticCode::SCHEMA_SYNTAX_ERROR,
                span,
            )));
        }
        Ok(span)
    }

    pub(crate) fn height<'slot, T>(
        &self,
        id: Id<T>,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, usize>
    where
        Id<T>: Node<Value = T>,
    {
        let index = id.index();
        let Some(stored) = Id::<T>::table(self).get(index) else {
            return Err(out
                .reborrow()
                .fail(AstAccessError::InvalidNodeId { id: index }));
        };
        Ok(stored.height)
    }

    pub(crate) fn height_for_diagnostic<'slot, T>(
        &self,
        id: Id<T>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, usize>
    where
        Id<T>: Node<Value = T>,
    {
        let index = id.index();
        let Some(stored) = Id::<T>::table(self).get(index) else {
            return Err(out.reborrow().fail(SyntaxDiagnostic::ast_access(
                AstAccessError::InvalidNodeId { id: index },
            )));
        };
        Ok(stored.height)
    }

    pub(crate) fn brace_key_span<'slot>(
        &self,
        key: &BraceKey,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, SourceSpan> {
        match key {
            BraceKey::Identifier(id) => self.span(*id, out.reborrow()),
            BraceKey::String(id) => self.span(*id, out.reborrow()),
        }
    }

    pub(crate) fn insert<'slot, T>(
        &mut self,
        value: T,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<T>>
    where
        Id<T>: Node<Value = T>,
    {
        let span = Id::<T>::span(&value);
        self.execution.consume(1, out.reborrow())?;
        self.check_span(span, out.reborrow())?;
        let child_height = Id::<T>::children(&value, self, out.reborrow())?;
        let height = match child_height {
            Some(child) => match child.checked_add(1) {
                Some(height) => height,
                None => {
                    return Err(out.reborrow().fail(SyntaxDiagnostic::emergency(
                        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED,
                        span,
                    )));
                }
            },
            None => 0,
        };
        self.check_node_budget(span, out.reborrow())?;
        if height > self.height_limit {
            return Err(out.reborrow().fail(SyntaxDiagnostic::emergency(
                DiagnosticCode::RESOURCE_LIMIT_EXCEEDED,
                span,
            )));
        }
        let table = Id::<T>::table_mut(self);
        let index = table.len();
        SyntaxDiagnostic::try_push(table, Stored { value, height }, span, out.reborrow())?;
        self.node_count += 1;
        Ok(Id::new(index))
    }

    pub(crate) fn new<'slot>(
        source_id: SourceId,
        source_limit: usize,
        limits: &SyntaxLimits,
        execution: &ExecutionBudget,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Self> {
        let span = SourceSpan::new(source_id, Span::empty());
        macro_rules! new_vec {
            ($out:ident) => {
                SyntaxDiagnostic::new_vec(execution, span, $out.reborrow())?
            };
        }
        Ok(Self {
            source_id,
            source_limit,
            actual_source_len: None,
            final_source_len: None,
            execution: execution.try_clone_with(out.reborrow(), |error| {
                SyntaxDiagnostic::budget(error, span)
            })?,
            node_count: 0,
            node_limit: limits.max_schema_ast_nodes(),
            height_limit: limits.max_nesting_depth(),
            text: TextStorage::new(execution, span, out.reborrow())?,
            annotation_edges: new_vec!(out),
            assertion_edges: new_vec!(out),
            brace_entry_edges: new_vec!(out),
            declaration_edges: new_vec!(out),
            default_value_edges: new_vec!(out),
            field_decl_edges: new_vec!(out),
            name_edges: new_vec!(out),
            variant_decl_edges: new_vec!(out),
            annotation: new_vec!(out),
            array_default: new_vec!(out),
            array_length: new_vec!(out),
            assertion: new_vec!(out),
            boolean_literal: new_vec!(out),
            brace_entry: new_vec!(out),
            braced_default: new_vec!(out),
            default_value: new_vec!(out),
            enum_decl: new_vec!(out),
            enum_default: new_vec!(out),
            enum_default_payload: new_vec!(out),
            expr: new_vec!(out),
            extern_mod_decl: new_vec!(out),
            field_decl: new_vec!(out),
            method_argument: new_vec!(out),
            module_decl: new_vec!(out),
            name: new_vec!(out),
            number_literal: new_vec!(out),
            string_literal: new_vec!(out),
            struct_decl: new_vec!(out),
            type_alias_decl: new_vec!(out),
            type_expr: new_vec!(out),
            type_path: new_vec!(out),
            variant_decl: new_vec!(out),
            variant_payload: new_vec!(out),
        })
    }

    pub(crate) fn node_count(&self) -> usize {
        self.node_count
    }

    pub(crate) fn confirm_eof(&mut self, source_len: usize) {
        self.actual_source_len = Some(source_len);
    }

    pub(crate) fn finish_source<'slot>(
        &mut self,
        source_len: usize,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ()> {
        if source_len > self.source_limit || self.actual_source_len != Some(source_len) {
            return Err(out.reborrow().fail(SyntaxDiagnostic::emergency(
                DiagnosticCode::INTERNAL_ERROR,
                span,
            )));
        }
        self.final_source_len = Some(source_len);
        Ok(())
    }

    fn check_node_budget<'slot>(
        &self,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ()> {
        if self.node_count >= self.node_limit {
            Err(out.reborrow().fail(SyntaxDiagnostic::emergency(
                DiagnosticCode::RESOURCE_LIMIT_EXCEEDED,
                span,
            )))
        } else {
            Ok(())
        }
    }

    fn check_span<'slot>(
        &self,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ()> {
        if span.source_id() != self.source_id || span.span().end() > self.source_limit {
            return Err(out.reborrow().fail(SyntaxDiagnostic::emergency(
                DiagnosticCode::SCHEMA_SYNTAX_ERROR,
                span,
            )));
        }
        Ok(())
    }

    pub(crate) fn text<'slot, 'storage>(
        &'storage self,
        reference: TextRef,
        out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, &'storage str> {
        self.text.read(reference, out)
    }

    fn pool<'slot>(
        &mut self,
        text: &str,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, TextRef> {
        self.execution.consume_with(1, out.reborrow(), |error| {
            SyntaxDiagnostic::budget(error, span)
        })?;
        self.check_span(span, out.reborrow())?;
        self.check_node_budget(span, out.reborrow())?;
        self.text.append_pooled(text, span, out)
    }

    fn move_text<'slot>(
        &mut self,
        text: ChargedString,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, TextRef> {
        self.execution.consume_with(1, out.reborrow(), |error| {
            SyntaxDiagnostic::budget(error, span)
        })?;
        self.check_span(span, out.reborrow())?;
        self.check_node_budget(span, out.reborrow())?;
        if !text
            .budget()
            .is_some_and(|budget| self.execution.same_call(budget))
        {
            return Err(out.reborrow().fail(SyntaxDiagnostic::internal(
                AST_STORAGE_DECODED_TEXT_BUDGET,
                span,
            )));
        }
        self.text.append_decoded(text, span, out)
    }

    pub(crate) fn name_text<'slot, 'storage>(
        &'storage self,
        name: Id<Name>,
        mut out: Out<'slot, '_, AstAccessError>,
    ) -> OResult<'slot, &'storage str> {
        let name = self.get(name, out.reborrow())?;
        self.text(name.text, out.reborrow())
    }

    pub(crate) fn name_text_for_diagnostic<'slot, 'storage>(
        &'storage self,
        name: Id<Name>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, &'storage str> {
        let index = name.index();
        let Some(name) = self.name.get(index) else {
            return Err(out.reborrow().fail(SyntaxDiagnostic::ast_access(
                AstAccessError::InvalidNodeId { id: index },
            )));
        };
        self.text
            .read_for_diagnostic(name.value.text, out.reborrow())
    }

    pub(crate) fn expr_set_span<'slot>(
        &mut self,
        id: Id<Expr>,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<Expr>> {
        self.execution.consume_with(1, out.reborrow(), |error| {
            SyntaxDiagnostic::budget(error, span)
        })?;
        self.check_span(span, out.reborrow())?;
        let index = id.index();
        let Some(expression) = self.expr.get_mut(index) else {
            return Err(out.reborrow().fail(SyntaxDiagnostic::internal_object(
                AST_STORAGE_UPDATE_EXPR_SPAN,
                span,
                index,
            )));
        };
        expression.value.span = span;
        Ok(id)
    }
}

impl Storage {
    pub(crate) fn annotation_new<'slot>(
        &mut self,
        data: AnnotationData,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<Annotation>> {
        let value = Annotation { data, span };
        self.insert(value, out.reborrow())
    }

    pub(crate) fn array_default_new<'slot>(
        &mut self,
        elements: ChargedVec<Id<DefaultValue>>,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<ArrayDefault>> {
        let elements = self.store_list(elements, span, out.reborrow())?;

        let value = ArrayDefault { elements, span };
        self.insert(value, out.reborrow())
    }

    pub(crate) fn array_length_number<'slot>(
        &mut self,
        number: Id<NumberLiteral>,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<ArrayLength>> {
        let value = ArrayLength {
            raw: match Id::<NumberLiteral>::table(self).get(number.index()) {
                Some(stored) => stored.value.raw,
                None => {
                    return Err(out.reborrow().fail(SyntaxDiagnostic::internal_object(
                        AST_STORAGE_ARRAY_LENGTH_NUMBER,
                        span,
                        number.index(),
                    )));
                }
            },
            span,
        };
        self.insert(value, out.reborrow())
    }

    pub(crate) fn assertion_new<'slot>(
        &mut self,
        expression: Id<Expr>,
        message: Id<StringLiteral>,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<Assertion>> {
        let value = Assertion {
            expression,
            message,
            span,
        };
        self.insert(value, out.reborrow())
    }

    pub(crate) fn boolean_literal_new<'slot>(
        &mut self,
        value: bool,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<BooleanLiteral>> {
        let value = BooleanLiteral { value, span };
        self.insert(value, out.reborrow())
    }

    pub(crate) fn brace_entry_new<'slot>(
        &mut self,
        key: BraceKey,
        value: Id<DefaultValue>,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<BraceEntry>> {
        let value = BraceEntry { key, value, span };
        self.insert(value, out.reborrow())
    }

    pub(crate) fn braced_default_new<'slot>(
        &mut self,
        entries: ChargedVec<Id<BraceEntry>>,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<BracedDefault>> {
        let entries = self.store_list(entries, span, out.reborrow())?;

        let value = BracedDefault { entries, span };
        self.insert(value, out.reborrow())
    }

    pub(crate) fn default_value_new<'slot>(
        &mut self,
        kind: DefaultValueKind,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<DefaultValue>> {
        let value = DefaultValue { kind, span };
        self.insert(value, out.reborrow())
    }

    pub(crate) fn enum_decl_new<'slot>(
        &mut self,
        name: Id<Name>,
        variants: ChargedVec<Id<VariantDecl>>,
        span: SourceSpan,
        out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<EnumDecl>> {
        StorageConstructor::new(self).enum_decl_new(name, variants, span, out)
    }

    pub(crate) fn enum_default_new<'slot>(
        &mut self,
        variant: Id<Name>,
        payload: Id<EnumDefaultPayload>,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<EnumDefault>> {
        let value = EnumDefault {
            variant,
            payload,
            span,
        };
        self.insert(value, out.reborrow())
    }

    pub(crate) fn enum_default_payload_unit<'slot>(
        &mut self,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<EnumDefaultPayload>> {
        let value = EnumDefaultPayload {
            kind: EnumDefaultPayloadKind::Unit,
            span,
        };
        self.insert(value, out.reborrow())
    }

    pub(crate) fn enum_default_payload_single<'slot>(
        &mut self,
        value: Id<DefaultValue>,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<EnumDefaultPayload>> {
        let value = EnumDefaultPayload {
            kind: EnumDefaultPayloadKind::Single(value),
            span,
        };
        self.insert(value, out.reborrow())
    }

    pub(crate) fn enum_default_payload_structure<'slot>(
        &mut self,
        fields: ChargedVec<Id<BraceEntry>>,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<EnumDefaultPayload>> {
        let value = EnumDefaultPayload {
            kind: EnumDefaultPayloadKind::Structure(self.braced_default_new(
                fields,
                span,
                out.reborrow(),
            )?),
            span,
        };
        self.insert(value, out.reborrow())
    }

    pub(crate) fn extern_mod_decl_new<'slot>(
        &mut self,
        name: Id<Name>,
        span: SourceSpan,
        out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<ExternModDecl>> {
        StorageConstructor::new(self).extern_mod_decl_new(name, span, out)
    }

    pub(crate) fn field_decl_new<'slot>(
        &mut self,
        params: FieldParams,
        out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<FieldDecl>> {
        StorageConstructor::new(self).field_decl_new(params, out)
    }

    pub(crate) fn method_argument_expression<'slot>(
        &mut self,
        expression: Id<Expr>,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<MethodArgument>> {
        let value = MethodArgument {
            kind: MethodArgumentKind::Expression(expression),
            span,
        };
        self.insert(value, out.reborrow())
    }

    pub(crate) fn method_argument_closure<'slot>(
        &mut self,
        parameter: Id<Name>,
        body: Id<Expr>,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<MethodArgument>> {
        let value = MethodArgument {
            kind: MethodArgumentKind::Closure { parameter, body },
            span,
        };
        self.insert(value, out.reborrow())
    }

    pub(crate) fn module_decl_new<'slot>(
        &mut self,
        name: Id<Name>,
        declarations: ChargedVec<Declaration>,
        span: SourceSpan,
        out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<ModuleDecl>> {
        StorageConstructor::new(self).module_decl_new(name, declarations, span, out)
    }

    pub(crate) fn name_try_new<'slot>(
        &mut self,
        text: &str,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<Name>> {
        let value = Name {
            text: self.pool(text, span, out.reborrow())?,
            span,
        };
        self.insert(value, out.reborrow())
    }

    pub(crate) fn number_literal_try_new<'slot>(
        &mut self,
        params: NumberLiteralParams<'_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<NumberLiteral>> {
        let NumberLiteralParams {
            raw,
            kind,
            negative,
            span,
        } = params;
        let value = NumberLiteral {
            raw: self.pool(raw, span, out.reborrow())?,
            kind,
            negative,
            span,
        };
        self.insert(value, out.reborrow())
    }

    pub(crate) fn string_literal_from_owned<'slot>(
        &mut self,
        value: ChargedString,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<StringLiteral>> {
        let value = StringLiteral {
            value: self.move_text(value, span, out.reborrow())?,
            span,
        };
        self.insert(value, out.reborrow())
    }

    pub(crate) fn struct_decl_new<'slot>(
        &mut self,
        params: StructDeclParams,
        out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<StructDecl>> {
        StorageConstructor::new(self).struct_decl_new(params, out)
    }

    pub(crate) fn type_alias_decl_new<'slot>(
        &mut self,
        params: TypeAliasDeclParams,
        out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<TypeAliasDecl>> {
        StorageConstructor::new(self).type_alias_decl_new(params, out)
    }

    pub(crate) fn variant_decl_new<'slot>(
        &mut self,
        params: VariantDeclParams,
        out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<VariantDecl>> {
        StorageConstructor::new(self).variant_decl_new(params, out)
    }

    pub(crate) fn variant_payload_unit<'slot>(
        &mut self,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<VariantPayload>> {
        let value = VariantPayload {
            kind: VariantPayloadKind::Unit,
            span,
        };
        self.insert(value, out.reborrow())
    }

    pub(crate) fn variant_payload_single<'slot>(
        &mut self,
        ty: Id<TypeExpr>,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<VariantPayload>> {
        let value = VariantPayload {
            kind: VariantPayloadKind::Single(SingleVariantPayload { ty }),
            span,
        };
        self.insert(value, out.reborrow())
    }

    pub(crate) fn variant_payload_structure<'slot>(
        &mut self,
        fields: ChargedVec<Id<FieldDecl>>,
        span: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<VariantPayload>> {
        let fields = self.store_list(fields, span, out.reborrow())?;

        let value = VariantPayload {
            kind: VariantPayloadKind::Structure(StructureVariantPayload { fields }),
            span,
        };
        self.insert(value, out.reborrow())
    }
}
