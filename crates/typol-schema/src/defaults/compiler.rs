mod regex_errors;

use crate::{
    SchemaLimits,
    annotations::{AnnotationCompilation, CompiledConstraint, ConstraintData, RegexId},
    context::CompileContext,
    defaults::{
        DefaultCompilation, DefaultDiagnostic, DefaultInputs, DefaultLiterals, DefaultNumber,
        build::DefaultBuilder,
        rules::{DefaultRuleChecker, SumRuleInput, UniqueRuleInput},
    },
    diagnostics::{BuildFailure, DiagnosticCollector, EmergencyDiagnostics},
    expressions::ExpressionCompilation,
    symbols::{FieldSymbol, FieldSymbolId, SymbolTable, VariantSymbol, VariantSymbolId},
    types::{ResolvedTypeGraph, ResolvedTypeKind, TypeUseId},
};
use core::{
    cell::RefCell,
    ops::{Deref, DerefMut},
};
use opeo::{ErrSlot, OResult, Out};
use typol_core::{
    BudgetError, ChargedVec, ConfigValue, ConfigValueBuilder, Diagnostic, DiagnosticMessage,
    ExecutionBudget, SourceSpan, ValueError, ValueId, ValueKind, ValueSource,
};
use typol_semantic::{
    CopyError, CopyOptions, CopyRequest, EvalError, RegexExecutionError, RegexExecutor,
    RegexMatchOptions, SchemaDefaultValidation, StoredRuleView as RuleData, ValueCopy, ValueRules,
};
use typol_syntax::{AstAccessError, BuiltinType, ScalarLiteral};

pub(super) struct BuiltValue {
    pub(super) value: ConfigValue,
    pub(super) span: SourceSpan,
    pub(super) member_spans: ChargedVec<SourceSpan>,
}

struct CachedFieldValue {
    field_index: usize,
    value: BuiltValue,
}

pub(super) enum CachedDefault {
    Unvisited,
    Visiting,
    Valid(usize),
    Invalid,
}

enum ScalarDefault<'a> {
    String(&'a str),
    Boolean(bool),
    Number(DefaultNumber),
}

#[derive(Clone, Copy)]
pub(super) enum CachedDefaultState {
    Unvisited,
    Visiting,
    Finished,
}

pub(super) struct DefaultData<'a, 'd> {
    pub(super) symbols: &'a SymbolTable<'a>,
    pub(super) types: &'a ResolvedTypeGraph,
    pub(super) annotations: &'a AnnotationCompilation,
    pub(super) expressions: &'a ExpressionCompilation,
    pub(super) limits: &'a SchemaLimits,
    pub(super) context: &'a CompileContext,
    pub(super) diagnostics: &'d mut DiagnosticCollector,
    pub(super) fields: ChargedVec<CachedDefault>,
    cached_values: ChargedVec<CachedFieldValue>,
    pub(super) value_nodes: usize,
    pub(super) regex_scratch: RegexExecutor,
    pub(super) fallback: SourceSpan,
}

pub(super) struct DefaultCompiler<'a, 'd> {
    data: DefaultData<'a, 'd>,
    evaluation_errors: RefCell<ErrSlot<EvalError>>,
    regex_errors: RefCell<ErrSlot<RegexExecutionError>>,
    value_errors: RefCell<ErrSlot<ValueError>>,
    copy_errors: RefCell<ErrSlot<CopyError>>,
}

impl<'a, 'd> DefaultCompiler<'a, 'd> {
    pub(super) fn ast_access<'slot, 'borrow, T>(
        &self,
        out: Out<'slot, 'borrow, BuildFailure>,
        operation: impl for<'call> FnOnce(Out<'call, 'call, AstAccessError>) -> OResult<'call, T>,
    ) -> OResult<'slot, T> {
        self.data
            .context
            .ast_access(out, operation, BuildFailure::from)
    }

    pub(super) fn value_access<'slot, 'borrow, T>(
        &mut self,
        span: SourceSpan,
        out: Out<'slot, 'borrow, BuildFailure>,
        operation: impl for<'call> FnOnce(Out<'call, 'call, ValueError>) -> OResult<'call, T>,
    ) -> OResult<'slot, T> {
        let result = {
            let mut errors = self.value_errors.borrow_mut();
            match errors.try_call(operation) {
                Ok(value) => Ok(value),
                Err(caught) => Err(caught.take()),
            }
        };
        match result {
            Ok(value) => Ok(value),
            Err(error) => {
                self.data
                    .diagnostics
                    .push(DefaultBuilder::value_diagnostic(error, span));
                Err(out.fail(BuildFailure::empty()))
            }
        }
    }

    pub(super) fn build_value<'slot>(
        &mut self,
        span: SourceSpan,
        build: impl for<'call> FnOnce(
            &mut ConfigValueBuilder,
            &ExecutionBudget,
            Out<'call, 'call, ValueError>,
        ) -> OResult<'call, ValueId>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ConfigValue> {
        let execution = self.data.context.execution();
        let budget = self.budget_access(span, out.reborrow(), |_, budget_out| {
            execution.try_clone(budget_out)
        })?;
        let mut builder = self.value_access(span, out.reborrow(), |value_out| {
            ConfigValueBuilder::new(Some(&budget), value_out)
        })?;
        let root = self.value_access(span, out.reborrow(), |value_out| {
            build(&mut builder, &budget, value_out)
        })?;
        self.value_access(span, out, |value_out| builder.finish(root, value_out))
    }

    pub(super) fn budget_access<'slot, 'borrow, T>(
        &mut self,
        span: SourceSpan,
        out: Out<'slot, 'borrow, BuildFailure>,
        operation: impl for<'call> FnOnce(
            &'a ExecutionBudget,
            Out<'call, 'call, BudgetError>,
        ) -> OResult<'call, T>,
    ) -> OResult<'slot, T> {
        let context = self.data.context;
        context.budget_access(out, span, self.data.diagnostics, |budget_out| {
            operation(context.execution(), budget_out)
        })
    }

    pub(super) fn field_symbol(&self, id: FieldSymbolId) -> Option<&'a FieldSymbol<'a>> {
        self.data.symbols.field_symbol(id)
    }

    pub(super) fn variant_symbol(&self, id: VariantSymbolId) -> Option<&'a VariantSymbol<'a>> {
        self.data.symbols.variant_symbol(id)
    }

    pub(super) fn cached_default(&self, id: FieldSymbolId) -> Option<&CachedDefault> {
        self.data.fields.get(id.index())
    }

    pub(super) fn cached_default_state(&self, id: FieldSymbolId) -> Option<CachedDefaultState> {
        self.data.fields.get(id.index()).map(|value| match value {
            CachedDefault::Unvisited => CachedDefaultState::Unvisited,
            CachedDefault::Visiting => CachedDefaultState::Visiting,
            CachedDefault::Valid(_) | CachedDefault::Invalid => CachedDefaultState::Finished,
        })
    }

    pub(super) fn set_cached_default(
        &mut self,
        id: FieldSymbolId,
        value: CachedDefault,
    ) -> Option<()> {
        *self.data.fields.get_mut(id.index())? = value;
        Some(())
    }

    pub(super) fn cache_default<'slot>(
        &mut self,
        id: FieldSymbolId,
        value: BuiltValue,
        span: SourceSpan,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ()> {
        let field_index = id.index();
        if self.data.fields.get(field_index).is_none() {
            self.data.diagnostics.abort(schema_internal!(span));
            return Err(out.fail(BuildFailure::empty()));
        }
        let cached_index = self.data.cached_values.len();
        self.data
            .cached_values
            .try_push(CachedFieldValue { field_index, value }, out.reborrow())?;
        let Some(field) = self.data.fields.get_mut(field_index) else {
            self.data.diagnostics.abort(schema_internal!(span));
            return Err(out.fail(BuildFailure::empty()));
        };
        *field = CachedDefault::Valid(cached_index);
        Ok(())
    }

    pub(super) fn type_kind(&self, id: TypeUseId) -> Option<ResolvedTypeKind> {
        self.data.types.type_use(id).map(|type_use| type_use.kind())
    }

    pub(super) fn new_vec<'slot, T>(
        &mut self,
        out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ChargedVec<T>> {
        self.data
            .diagnostics
            .new_vec(self.data.context.execution(), out)
    }

    pub(super) fn new<'slot>(
        inputs: DefaultInputs<'a>,
        diagnostics: &'d mut DiagnosticCollector,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, Self> {
        let DefaultInputs {
            symbols,
            types,
            annotations,
            expressions,
            context,
            fallback,
        } = inputs;
        let limits = context.limits();
        let mut fields = diagnostics.new_vec(context.execution(), out.reborrow())?;
        let cached_values = diagnostics.new_vec(context.execution(), out.reborrow())?;
        fields.try_reserve_exact(symbols.fields().len(), out.reborrow())?;
        for _ in 0..symbols.fields().len() {
            fields.try_push(CachedDefault::Unvisited, out.reborrow())?;
        }
        Ok(Self {
            data: DefaultData {
                symbols,
                types,
                annotations,
                expressions,
                limits,
                context,
                diagnostics,
                fields,
                cached_values,
                value_nodes: 0,
                regex_scratch: RegexExecutor::new(),
                fallback,
            },
            evaluation_errors: RefCell::new(ErrSlot::new()),
            regex_errors: RefCell::new(ErrSlot::new()),
            value_errors: RefCell::new(ErrSlot::new()),
            copy_errors: RefCell::new(ErrSlot::new()),
        })
    }

    pub(super) fn finish<'slot>(
        self,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, DefaultCompilation> {
        let DefaultData {
            fields,
            cached_values,
            diagnostics,
            fallback,
            context,
            ..
        } = self.data;
        let mut defaults = diagnostics.new_vec(context.execution(), out.reborrow())?;
        defaults.try_reserve_exact(fields.len(), out.reborrow())?;
        for _ in fields {
            context.consume(1, fallback, diagnostics, out.reborrow())?;
            defaults.try_push(None, out.reborrow())?;
        }
        for cached in cached_values {
            context.consume(1, fallback, diagnostics, out.reborrow())?;
            let Some(field) = defaults.get_mut(cached.field_index) else {
                diagnostics.abort(schema_internal!(fallback));
                return Err(out.fail(BuildFailure::empty()));
            };
            *field = Some(cached.value.value);
        }
        let validation = if diagnostics.has_diagnostics() {
            None
        } else {
            // SAFETY: DefaultPass finishes only after every declared default was built and all alias, field, payload, and collection rules passed.
            Some(unsafe { SchemaDefaultValidation::from_completed_schema_validation() })
        };
        Ok(DefaultCompilation::new(defaults, validation))
    }

    pub(super) fn build_scalar<'slot>(
        &mut self,
        source: ScalarLiteral<'_>,
        target: ResolvedTypeKind,
        span: SourceSpan,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, Option<BuiltValue>> {
        let ResolvedTypeKind::Builtin(builtin) = target else {
            self.report_invalid(
                DefaultDiagnostic::scalar_type_mismatch(),
                source.span(),
                out.reborrow(),
            )?;
            return Ok(None);
        };
        let scalar = match source {
            ScalarLiteral::String(value) if builtin == BuiltinType::String => {
                let text = self.ast_access(out.reborrow(), |ast_out| value.value(ast_out))?;
                if !self.check_string(text, value.span(), out.reborrow())? {
                    return Ok(None);
                }
                ScalarDefault::String(text)
            }
            ScalarLiteral::Boolean(value) if builtin == BuiltinType::Bool => {
                ScalarDefault::Boolean(value.value())
            }
            ScalarLiteral::Number(value) => match DefaultLiterals::build_number(
                value,
                builtin,
                self.data.context,
                self.data.diagnostics,
                out.reborrow(),
            )? {
                Some(value) => ScalarDefault::Number(value),
                None => return Ok(None),
            },
            ScalarLiteral::String(_) => {
                self.report_invalid(
                    DefaultDiagnostic::string_type_mismatch(),
                    source.span(),
                    out.reborrow(),
                )?;
                return Ok(None);
            }
            ScalarLiteral::Boolean(_) => {
                self.report_invalid(
                    DefaultDiagnostic::boolean_type_mismatch(),
                    source.span(),
                    out.reborrow(),
                )?;
                return Ok(None);
            }
        };
        let value = self.build_value(
            span,
            |builder, _, mut value_out| {
                let source = ValueSource::SchemaDefault;
                let root = match scalar {
                    ScalarDefault::String(text) => {
                        builder.add_string(text, source, value_out.reborrow())?
                    }
                    ScalarDefault::Boolean(value) => {
                        builder.add_bool(value, source, value_out.reborrow())?
                    }
                    ScalarDefault::Number(DefaultNumber::Signed(value)) => {
                        builder.add_signed(value, source, value_out.reborrow())?
                    }
                    ScalarDefault::Number(DefaultNumber::Unsigned(value)) => {
                        builder.add_unsigned(value, source, value_out.reborrow())?
                    }
                    ScalarDefault::Number(DefaultNumber::Float32(value)) => {
                        builder.add_float32(value, source, value_out.reborrow())?
                    }
                    ScalarDefault::Number(DefaultNumber::Float64(value)) => {
                        builder.add_float64(value, source, value_out.reborrow())?
                    }
                };
                Ok(root)
            },
            out.reborrow(),
        )?;
        let member_spans = self.new_vec(out.reborrow())?;
        Ok(Some(self.built(value, span, member_spans)))
    }

    pub(super) fn clone_cached_default<'slot>(
        &mut self,
        id: FieldSymbolId,
        depth: usize,
        use_span: SourceSpan,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, Option<BuiltValue>> {
        self.check_depth(depth, use_span, out.reborrow())?;
        let fields = &self.data.fields;
        let cached_values = &self.data.cached_values;
        let value_nodes = &mut self.data.value_nodes;
        let limits = self.data.limits.values();
        let diagnostics = &mut self.data.diagnostics;
        let Some(CachedDefault::Valid(index)) = fields.get(id.index()) else {
            return Ok(None);
        };
        let Some(cached) = cached_values.get(*index) else {
            diagnostics.abort(schema_internal!(use_span));
            return Err(out.fail(BuildFailure::empty()));
        };
        let value = &cached.value;
        let span = value.span;
        let copy_errors = &self.copy_errors;
        let cloned = match copy_errors.borrow_mut().try_call(|copy_out| {
            ValueCopy::copy_value(
                CopyRequest::new(
                    &value.value,
                    depth,
                    value_nodes,
                    limits,
                    CopyOptions::new(0, Some(self.data.context.execution()), usize::MAX),
                ),
                copy_out,
            )
        }) {
            Ok(value) => value,
            Err(caught) => {
                diagnostics.push(copy_diagnostic(caught.take(), span));
                return Err(out.fail(BuildFailure::empty()));
            }
        };
        let context = self.data.context;
        let diagnostics = &mut self.data.diagnostics;
        let mut member_spans = diagnostics.new_vec(context.execution(), out.reborrow())?;
        member_spans.try_reserve_exact(value.member_spans.len(), out.reborrow())?;
        member_spans.extend_from_slice(&value.member_spans, out.reborrow())?;
        Ok(Some(BuiltValue {
            value: cloned,
            span: value.span,
            member_spans,
        }))
    }

    pub(super) fn validate_constraints<'slot>(
        &mut self,
        value: &BuiltValue,
        target: TypeUseId,
        extra: &[CompiledConstraint],
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, bool> {
        let span = value.span;
        let mut link = self
            .types
            .type_use(target)
            .and_then(|type_use| type_use.alias_chain());
        let mut depth = 0_usize;
        let mut valid = true;
        while let Some(link_id) = link {
            self.data
                .context
                .consume(1, span, self.data.diagnostics, out.reborrow())?;
            let Some(next_depth) = depth.checked_add(1) else {
                self.data
                    .diagnostics
                    .push(EmergencyDiagnostics::resource(span));
                return Err(out.fail(BuildFailure::empty()));
            };
            depth = next_depth;
            if depth > self.data.limits.max_nesting_depth() {
                self.data
                    .diagnostics
                    .push(EmergencyDiagnostics::resource(span));
                return Err(out.fail(BuildFailure::empty()));
            }
            let Some(alias) = self.data.types.alias_link(link_id) else {
                self.data.diagnostics.push(schema_internal!(span));
                return Err(out.fail(BuildFailure::empty()));
            };
            if let Some(constraints) = self.data.annotations.alias_constraints(alias.symbol()) {
                valid &=
                    self.validate_constraint_list(value, target, constraints, out.reborrow())?;
            }
            link = alias.next();
            if self.data.diagnostics.is_stopped() {
                return Err(out.fail(BuildFailure::empty()));
            }
        }
        if let Some(constraints) = self.annotations.type_use_constraints(target) {
            valid &= self.validate_constraint_list(value, target, constraints, out.reborrow())?;
        }
        valid &= self.validate_constraint_list(value, target, extra, out.reborrow())?;
        Ok(valid)
    }

    pub(super) fn validate_local_constraints<'slot>(
        &mut self,
        value: &BuiltValue,
        target: TypeUseId,
        constraints: &[CompiledConstraint],
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, bool> {
        self.validate_constraint_list(value, target, constraints, out.reborrow())
    }

    fn validate_constraint_list<'slot>(
        &mut self,
        value: &BuiltValue,
        target: TypeUseId,
        constraints: &[CompiledConstraint],
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, bool> {
        let mut valid = true;
        for constraint in constraints {
            self.data
                .context
                .consume(1, value.span, self.data.diagnostics, out.reborrow())?;
            if self.data.diagnostics.is_stopped() {
                return Err(out.fail(BuildFailure::empty()));
            }
            let matched = match constraint.data() {
                ConstraintData::Regex { regex } => {
                    self.check_regex(value, *regex, constraint.span(), out.reborrow())?
                }
                ConstraintData::NumberBounds { .. }
                | ConstraintData::LengthBounds { .. }
                | ConstraintData::Host
                | ConstraintData::Email
                | ConstraintData::Cidr { .. }
                | ConstraintData::SizeBounds { .. } => {
                    self.check_scalar(value, constraint, out.reborrow())?
                }
                ConstraintData::Unique { by } => self.check_unique(
                    UniqueRuleInput {
                        value,
                        target,
                        by: by.as_deref(),
                        constraint_span: constraint.span(),
                    },
                    out.reborrow(),
                )?,
                ConstraintData::All { .. } => {
                    self.check_all(value, target, constraint, out.reborrow())?
                }
                ConstraintData::SumBounds { min, max } => self.check_sum(
                    SumRuleInput {
                        value,
                        target,
                        bounds: (*min, *max),
                        constraint_span: constraint.span(),
                    },
                    out.reborrow(),
                )?,
            };
            if !matched {
                valid = false;
            }
        }
        Ok(valid)
    }

    fn check_scalar<'slot>(
        &mut self,
        value: &BuiltValue,
        constraint: &CompiledConstraint,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, bool> {
        let mut leaf = value.value.root();
        while let ValueKind::Optional(inner) = leaf.as_ref() {
            self.data
                .context
                .consume(1, value.span, self.data.diagnostics, out.reborrow())?;
            let Some(inner) = inner else {
                return Ok(true);
            };
            leaf = inner;
        }
        let data = match *constraint.data() {
            ConstraintData::NumberBounds { min, max } => RuleData::NumberBounds { min, max },
            ConstraintData::LengthBounds { min, max } => RuleData::LengthBounds { min, max },
            ConstraintData::SizeBounds { min, max } => RuleData::SizeBounds { min, max },
            ConstraintData::Host => RuleData::Host,
            ConstraintData::Email => RuleData::Email,
            ConstraintData::Cidr { v4_only } => RuleData::Cidr { v4_only },
            _ => {
                self.data.diagnostics.push(schema_internal!(value.span));
                return Err(out.fail(BuildFailure::empty()));
            }
        };
        let execution = self.data.context.execution();
        let result = {
            let mut errors = self.evaluation_errors.borrow_mut();
            match errors
                .try_call(|eval_out| ValueRules::scalar(leaf, data, Some(execution), eval_out))
            {
                Ok(value) => Ok(value),
                Err(caught) => Err(caught.take()),
            }
        };
        let Some(matched) = (match result {
            Ok(value) => value,
            Err(error) => {
                self.data.diagnostics.push(DefaultDiagnostic::execution(
                    error,
                    value.span,
                    self.diagnostics,
                ));
                return Err(out.fail(BuildFailure::empty()));
            }
        }) else {
            self.data.diagnostics.push(schema_internal!(value.span));
            return Err(out.fail(BuildFailure::empty()));
        };
        if matched {
            Ok(true)
        } else {
            self.constraint_failure(
                DefaultDiagnostic::local_constraint_failed(),
                value.span,
                constraint.span(),
                out.reborrow(),
            )
        }
    }

    fn check_regex<'slot>(
        &mut self,
        value: &BuiltValue,
        regex_id: RegexId,
        constraint_span: SourceSpan,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, bool> {
        let Some(text) = DefaultLiterals::optional_string(
            value.value.as_ref(),
            self.data.context,
            out.reborrow(),
        )?
        else {
            return self.constraint_failure(
                DefaultDiagnostic::regex_constraint_failed(),
                value.span,
                constraint_span,
                out.reborrow(),
            );
        };
        let Some(text) = text else {
            return Ok(true);
        };
        self.data
            .context
            .consume(1, value.span, self.data.diagnostics, out.reborrow())?;
        let Some(regex) = self
            .annotations
            .regexes()
            .constraints()
            .get(regex_id.index())
            .map(|entry| entry.regex())
        else {
            self.data.diagnostics.push(schema_internal!(value.span));
            return Err(out.fail(BuildFailure::empty()));
        };
        let scratch = &mut self.data.regex_scratch;
        let result = {
            let mut errors = self.regex_errors.borrow_mut();
            match errors.try_call(|regex_out| {
                scratch.is_match_budgeted(
                    regex,
                    RegexMatchOptions {
                        value: Some(text),
                        limits: self.data.context.regex_limits(),
                        budget: self.data.context.regex_execution(),
                    },
                    regex_out,
                )
            }) {
                Ok(value) => Ok(value),
                Err(caught) => Err(caught.take()),
            }
        };
        match result {
            Ok(true) => Ok(true),
            Ok(false) => self.constraint_failure(
                DefaultDiagnostic::regex_constraint_failed(),
                value.span,
                constraint_span,
                out.reborrow(),
            ),
            Err(error) => {
                let error = regex_errors::RegexExecutionDiagnostics::map(error, constraint_span);
                self.data.diagnostics.push(DefaultDiagnostic::execution(
                    error,
                    value.span,
                    self.diagnostics,
                ));
                Err(out.fail(BuildFailure::empty()))
            }
        }
    }

    fn check_unique<'slot>(
        &mut self,
        input: UniqueRuleInput<'_>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, bool> {
        let mut rules = DefaultRuleChecker::new(&mut self.data, &self.evaluation_errors);
        rules.check_unique(input, out.reborrow())
    }

    fn check_all<'slot>(
        &mut self,
        value: &BuiltValue,
        target: TypeUseId,
        constraint: &CompiledConstraint,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, bool> {
        let mut rules = DefaultRuleChecker::new(&mut self.data, &self.evaluation_errors);
        rules.check_all(value, target, constraint, out.reborrow())
    }

    fn check_sum<'slot>(
        &mut self,
        input: SumRuleInput<'_>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, bool> {
        let mut rules = DefaultRuleChecker::new(&mut self.data, &self.evaluation_errors);
        rules.check_sum(input, out.reborrow())
    }

    fn constraint_failure<'slot>(
        &mut self,
        message: DiagnosticMessage<'static>,
        value_span: SourceSpan,
        constraint_span: SourceSpan,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, bool> {
        let mut rules = DefaultRuleChecker::new(&mut self.data, &self.evaluation_errors);
        rules.constraint_failure(message, value_span, constraint_span, out.reborrow())
    }

    pub(super) fn check_depth<'slot>(
        &mut self,
        depth: usize,
        span: SourceSpan,
        out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ()> {
        if !self.data.limits.values().allows_depth(depth) {
            self.data
                .diagnostics
                .push(EmergencyDiagnostics::resource(span));
            Err(out.fail(BuildFailure::empty()))
        } else {
            Ok(())
        }
    }

    pub(super) fn add_value_node<'slot>(
        &mut self,
        span: SourceSpan,
        out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ()> {
        let Some(value_nodes) = self.data.value_nodes.checked_add(1) else {
            self.data
                .diagnostics
                .push(EmergencyDiagnostics::resource(span));
            return Err(out.fail(BuildFailure::empty()));
        };
        self.data.value_nodes = value_nodes;
        if self.data.value_nodes > self.data.limits.max_value_nodes() {
            self.data
                .diagnostics
                .push(EmergencyDiagnostics::resource(span));
            return Err(out.fail(BuildFailure::empty()));
        }
        Ok(())
    }

    pub(super) fn check_string<'slot>(
        &mut self,
        value: &str,
        span: SourceSpan,
        out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, bool> {
        if value.len() > self.data.limits.max_string_bytes() {
            self.data
                .diagnostics
                .push(EmergencyDiagnostics::resource(span));
            return Err(out.fail(BuildFailure::empty()));
        }
        Ok(true)
    }

    pub(super) fn built(
        &self,
        value: ConfigValue,
        span: SourceSpan,
        member_spans: ChargedVec<SourceSpan>,
    ) -> BuiltValue {
        BuiltValue {
            value,
            span,
            member_spans,
        }
    }

    pub(super) fn report_invalid<'slot>(
        &mut self,
        message: DiagnosticMessage<'static>,
        span: SourceSpan,
        out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ()> {
        self.data
            .diagnostics
            .push(self.diagnostics.make_semantic(message, span));
        if self.data.diagnostics.is_stopped() {
            Err(out.fail(BuildFailure::empty()))
        } else {
            Ok(())
        }
    }
}

impl<'a, 'd> Deref for DefaultCompiler<'a, 'd> {
    type Target = DefaultData<'a, 'd>;

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

impl DerefMut for DefaultCompiler<'_, '_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.data
    }
}

fn copy_diagnostic(error: CopyError, span: SourceSpan) -> Diagnostic {
    match error {
        CopyError::Resource => EmergencyDiagnostics::resource(span),
        CopyError::Allocation => EmergencyDiagnostics::allocation(span),
        CopyError::Capacity => EmergencyDiagnostics::capacity(span),
        CopyError::Internal => schema_internal!(span),
        CopyError::InternalAt(context) => EmergencyDiagnostics::internal(span, context),
        CopyError::InvalidValue(error) => DefaultBuilder::value_diagnostic(error, span),
    }
}
