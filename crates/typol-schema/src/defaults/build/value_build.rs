use crate::{
    defaults::{
        build::tasks::{ArrayBuild, PrepareEnumInput, Task, TaskState, ValueTask},
        compiler::{BuiltValue, DefaultCompiler},
        frames::{DefaultFrames, PrepareMapInput, PrepareRecordInput, RecordOwner},
        value_transfer::{EnumPayload, ValueTransfer},
    },
    diagnostics::BuildFailure,
    module_tree::TypeDeclarationKind,
    symbols::{LookupResult, SymbolState, VariantSymbolId},
    types::{ResolvedTypeKind, TypeUseId},
};
use typol_i18n::diagnostic_message;

use opeo::{OResult, Out};
use typol_core::{ChargedVec, SourceSpan, ValueSource};
use typol_syntax::{
    ArrayDefault, AstAccessError, DefaultValueKind, EnumDefaultPayloadKind, ScalarLiteral,
    VariantPayloadKind,
};

pub(super) struct ValueDefaultBuilder<'b, 'a, 'd> {
    compiler: &'b mut DefaultCompiler<'a, 'd>,
}

struct SingleArrayBuild {
    supported: bool,
    value: Option<BuiltValue>,
}

impl SingleArrayBuild {
    fn unsupported() -> Self {
        Self {
            supported: false,
            value: None,
        }
    }

    fn invalid() -> Self {
        Self {
            supported: true,
            value: None,
        }
    }

    fn built(value: BuiltValue) -> Self {
        Self {
            supported: true,
            value: Some(value),
        }
    }
}

struct SingleArrayInput<'a> {
    source: ArrayDefault<'a>,
    target: TypeUseId,
    depth: usize,
    span: SourceSpan,
}

struct ScalarDefaultTask<'a> {
    value: ScalarLiteral<'a>,
    span: SourceSpan,
    target: TypeUseId,
}

impl<'b, 'a, 'd> ValueDefaultBuilder<'b, 'a, 'd> {
    pub(super) fn new(compiler: &'b mut DefaultCompiler<'a, 'd>) -> Self {
        Self { compiler }
    }

    fn ast_access<'slot, 'borrow, T>(
        &self,
        out: Out<'slot, 'borrow, BuildFailure>,
        operation: impl for<'call> FnOnce(Out<'call, 'call, AstAccessError>) -> OResult<'call, T>,
    ) -> OResult<'slot, T> {
        self.compiler.ast_access(out, operation)
    }

    pub(super) fn build<'slot>(
        compiler: &'b mut DefaultCompiler<'a, 'd>,
        value: ValueTask<'a>,
        tasks: &mut ChargedVec<Task<'a>>,
        state: &mut TaskState<'a>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ()> {
        let mut builder = Self { compiler };
        let ValueTask {
            source,
            target,
            depth,
        } = value;
        state.set_result(None);
        builder
            .compiler
            .check_depth(depth, source.span(), out.reborrow())?;
        builder
            .compiler
            .add_value_node(source.span(), out.reborrow())?;
        let Some(kind) = builder.compiler.type_kind(target) else {
            builder
                .compiler
                .diagnostics
                .abort(schema_internal!(source.span()));
            return Err(out.fail(BuildFailure::empty()));
        };
        builder.push_task(tasks, Task::Validate(target), out.reborrow())?;
        if let ResolvedTypeKind::Option(inner) = kind {
            let value_kind = builder.ast_access(out.reborrow(), |ast_out| source.kind(ast_out))?;
            if matches!(value_kind, DefaultValueKind::None) {
                let value = builder.compiler.build_value(
                    source.span(),
                    |value_builder, budget, value_out| {
                        ValueTransfer::optional(value_builder, None, Some(budget), value_out)
                    },
                    out.reborrow(),
                )?;
                let member_spans = builder.compiler.new_vec(out.reborrow())?;
                state.set_result(Some(builder.compiler.built(
                    value,
                    source.span(),
                    member_spans,
                )));
            } else {
                builder.push_task(tasks, Task::Some(source.span()), out.reborrow())?;
                builder.push_task(tasks, Task::value(source, inner, depth + 1), out.reborrow())?;
            }
            return Ok(());
        }
        let value_kind = builder.ast_access(out.reborrow(), |ast_out| source.kind(ast_out))?;
        match value_kind {
            DefaultValueKind::None => {
                builder.compiler.report_invalid(
                    diagnostic_message!(
                        "diagnostic.schema.default_none_requires_option",
                        "none can be used only with an option type"
                    ),
                    source.span(),
                    out.reborrow(),
                )?;
            }
            DefaultValueKind::Scalar(scalar) => {
                state.set_result(builder.compiler.build_scalar(
                    scalar,
                    kind,
                    source.span(),
                    out.reborrow(),
                )?);
            }
            DefaultValueKind::Array(array) => {
                let ResolvedTypeKind::Array { element, length } = kind else {
                    builder.compiler.report_invalid(
                        diagnostic_message!(
                            "diagnostic.schema.default_array_type_mismatch",
                            "array default does not match the target type"
                        ),
                        array.span(),
                        out.reborrow(),
                    )?;
                    return Ok(());
                };
                let single_array = builder.single_array_chain(
                    SingleArrayInput {
                        source: array,
                        target,
                        depth,
                        span: source.span(),
                    },
                    out.reborrow(),
                )?;
                if single_array.supported {
                    state.set_result(single_array.value);
                    return Ok(());
                }
                let elements =
                    builder.ast_access(out.reborrow(), |ast_out| array.elements(ast_out))?;
                let mut valid = true;
                if let Some(expected) = length {
                    if usize::try_from(expected).ok() != Some(elements.len()) {
                        builder.compiler.report_invalid(
                            diagnostic_message!(
                                "diagnostic.schema.default_array_length_mismatch",
                                "fixed-size array default element count does not match"
                            ),
                            array.span(),
                            out.reborrow(),
                        )?;
                        valid = false;
                    }
                }
                let element_count = elements.len();
                let values = DefaultFrames::new(builder.compiler).values(
                    element_count,
                    array.span(),
                    out.reborrow(),
                )?;
                let spans = DefaultFrames::new(builder.compiler).spans(
                    element_count,
                    array.span(),
                    out.reborrow(),
                )?;
                builder.push_task(
                    tasks,
                    Task::Array(ArrayBuild {
                        source: array,
                        target: element,
                        depth,
                        index: 0,
                        valid,
                        values,
                        spans,
                    }),
                    out.reborrow(),
                )?;
            }
            DefaultValueKind::Braced(object) => match kind {
                ResolvedTypeKind::Map(value_type) => {
                    let entries =
                        builder.ast_access(out.reborrow(), |ast_out| object.entries(ast_out))?;
                    let frame = DefaultFrames::new(builder.compiler).prepare_map(
                        PrepareMapInput {
                            entries,
                            target: value_type,
                            span: source.span(),
                            depth,
                        },
                        out.reborrow(),
                    )?;
                    builder.push_task(tasks, Task::Map(frame), out.reborrow())?;
                }
                ResolvedTypeKind::User(owner) => {
                    let Some(symbol) = builder.compiler.symbols.type_symbol(owner) else {
                        builder
                            .compiler
                            .diagnostics
                            .push(schema_internal!(source.span()));
                        return Err(out.fail(BuildFailure::empty()));
                    };
                    let declaration = symbol.declaration();
                    match declaration {
                        TypeDeclarationKind::Struct(_) => {
                            let entries = builder
                                .ast_access(out.reborrow(), |ast_out| object.entries(ast_out))?;
                            let frame = DefaultFrames::new(builder.compiler).prepare_record(
                                PrepareRecordInput {
                                    entries,
                                    owner: RecordOwner::Struct(owner),
                                    span: source.span(),
                                    depth,
                                },
                                out.reborrow(),
                            )?;
                            builder.push_task(
                                tasks,
                                Task::Structure(source.span()),
                                out.reborrow(),
                            )?;
                            builder.push_task(tasks, Task::Record(frame), out.reborrow())?;
                        }
                        TypeDeclarationKind::Enum(_) => {
                            builder.compiler.report_invalid(
                                diagnostic_message!(
                                    "diagnostic.schema.default_enum_variant_syntax_required",
                                    "enum default must use .Variant syntax"
                                ),
                                source.span(),
                                out.reborrow(),
                            )?;
                        }
                        TypeDeclarationKind::Alias(_) => {
                            builder
                                .compiler
                                .diagnostics
                                .push(schema_internal!(source.span()));
                            return Err(out.fail(BuildFailure::empty()));
                        }
                    }
                }
                _ => {
                    builder.compiler.report_invalid(
                        diagnostic_message!(
                            "diagnostic.schema.default_braced_requires_struct_or_map",
                            "brace-enclosed defaults apply only to structs or maps"
                        ),
                        source.span(),
                        out.reborrow(),
                    )?;
                }
            },
            DefaultValueKind::Enum(value) => {
                state.set_result(builder.prepare_enum(
                    PrepareEnumInput {
                        source: value,
                        kind,
                        depth,
                        tasks,
                    },
                    out.reborrow(),
                )?);
            }
        }
        Ok(())
    }

    fn single_array_chain<'slot>(
        &mut self,
        input: SingleArrayInput<'a>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, SingleArrayBuild> {
        let SingleArrayInput {
            source: root,
            target: root_target,
            depth: root_depth,
            span: root_span,
        } = input;
        let mut current = root;
        let mut current_target = root_target;
        let mut array_count = 0_usize;
        let mut outer_member_span = None;
        let scalar = loop {
            self.compiler.context.consume(
                1,
                self.compiler.fallback,
                self.compiler.diagnostics,
                out.reborrow(),
            )?;
            if !self.unaliased(current_target) {
                return Ok(SingleArrayBuild::unsupported());
            }
            let Some(ResolvedTypeKind::Array { element, length }) =
                self.compiler.type_kind(current_target)
            else {
                return Ok(SingleArrayBuild::unsupported());
            };
            if length.is_some_and(|length| length != 1) {
                return Ok(SingleArrayBuild::unsupported());
            }
            let elements = self.ast_access(out.reborrow(), |ast_out| current.elements(ast_out))?;
            if elements.len() != 1 {
                return Ok(SingleArrayBuild::unsupported());
            }
            let Some(child) =
                self.ast_access(out.reborrow(), |ast_out| elements.get(0, ast_out))?
            else {
                return Ok(SingleArrayBuild::unsupported());
            };
            if outer_member_span.is_none() {
                outer_member_span = Some(child.span());
            }
            let Some(next_count) = array_count.checked_add(1) else {
                return Err(out.fail(BuildFailure::empty()));
            };
            array_count = next_count;
            match self.ast_access(out.reborrow(), |ast_out| child.kind(ast_out))? {
                DefaultValueKind::Array(array) => {
                    current = array;
                    current_target = element;
                }
                DefaultValueKind::Scalar(scalar)
                    if self.unaliased(element)
                        && matches!(
                            self.compiler.type_kind(element),
                            Some(ResolvedTypeKind::Builtin(_))
                        ) =>
                {
                    break ScalarDefaultTask {
                        value: scalar,
                        span: child.span(),
                        target: element,
                    };
                }
                _ => return Ok(SingleArrayBuild::unsupported()),
            }
        };

        let mut current = root;
        let mut current_depth = root_depth;
        for _ in 0..array_count {
            let elements = self.ast_access(out.reborrow(), |ast_out| current.elements(ast_out))?;
            let Some(child) =
                self.ast_access(out.reborrow(), |ast_out| elements.get(0, ast_out))?
            else {
                return Ok(SingleArrayBuild::unsupported());
            };
            self.compiler.context.consume(
                1,
                self.compiler.fallback,
                self.compiler.diagnostics,
                out.reborrow(),
            )?;
            let Some(next_depth) = current_depth.checked_add(1) else {
                return Err(out.fail(BuildFailure::empty()));
            };
            current_depth = next_depth;
            self.compiler
                .check_depth(current_depth, child.span(), out.reborrow())?;
            self.compiler.add_value_node(child.span(), out.reborrow())?;
            self.compiler.context.consume(
                3,
                self.compiler.fallback,
                self.compiler.diagnostics,
                out.reborrow(),
            )?;
            match self.ast_access(out.reborrow(), |ast_out| child.kind(ast_out))? {
                DefaultValueKind::Array(array) => current = array,
                DefaultValueKind::Scalar(_) => break,
                _ => return Ok(SingleArrayBuild::unsupported()),
            }
        }

        let Some(scalar_kind) = self.compiler.type_kind(scalar.target) else {
            return Err(out.fail(BuildFailure::empty()));
        };
        let Some(scalar) =
            self.compiler
                .build_scalar(scalar.value, scalar_kind, scalar.span, out.reborrow())?
        else {
            return Ok(SingleArrayBuild::invalid());
        };
        let value = self.compiler.build_value(
            root_span,
            |builder, budget, mut value_out| {
                let mut child = ValueTransfer::copy_into(
                    builder,
                    scalar.value.root(),
                    Some(budget),
                    value_out.reborrow(),
                )?;
                for _ in 0..array_count {
                    child = builder.add_array(
                        &[child],
                        ValueSource::SchemaDefault,
                        value_out.reborrow(),
                    )?;
                }
                Ok(child)
            },
            out.reborrow(),
        )?;
        let mut member_spans = self.compiler.new_vec(out.reborrow())?;
        let Some(outer_member_span) = outer_member_span else {
            return Err(out.fail(BuildFailure::empty()));
        };
        member_spans.try_push(outer_member_span, out.reborrow())?;
        Ok(SingleArrayBuild::built(self.compiler.built(
            value,
            root_span,
            member_spans,
        )))
    }

    fn unaliased(&self, target: TypeUseId) -> bool {
        self.compiler
            .types
            .type_use(target)
            .is_some_and(|type_use| type_use.alias_chain().is_none())
    }

    fn push_task<'slot>(
        &mut self,
        tasks: &mut ChargedVec<Task<'a>>,
        task: Task<'a>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ()> {
        tasks.try_reserve(1, out.reborrow())?;
        tasks.try_push(task, out.reborrow())?;
        Ok(())
    }

    pub(super) fn enum_value<'slot>(
        &mut self,
        id: VariantSymbolId,
        payload: EnumPayload<'a>,
        span: SourceSpan,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, BuiltValue> {
        let Some(variant) = self.compiler.variant_symbol(id) else {
            self.compiler.diagnostics.abort(schema_internal!(span));
            return Err(out.fail(BuildFailure::empty()));
        };
        let name = self.ast_access(out.reborrow(), |ast_out| {
            variant.declaration().name(ast_out)
        })?;
        let value = self.compiler.build_value(
            span,
            |builder, budget, value_out| {
                ValueTransfer::enumeration(builder, name, payload, Some(budget), value_out)
            },
            out.reborrow(),
        )?;
        let member_spans = self.compiler.new_vec(out.reborrow())?;
        Ok(self.compiler.built(value, span, member_spans))
    }

    fn prepare_enum<'slot>(
        &mut self,
        input: PrepareEnumInput<'_, 'a>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, Option<BuiltValue>> {
        let PrepareEnumInput {
            source,
            kind,
            depth,
            tasks,
        } = input;
        let span = source.span();
        let ResolvedTypeKind::User(owner) = kind else {
            self.compiler.report_invalid(
                diagnostic_message!(
                    "diagnostic.schema.default_enum_type_mismatch",
                    "enum default does not match the target type"
                ),
                source.span(),
                out.reborrow(),
            )?;
            return Ok(None);
        };
        let Some(symbol) = self.compiler.symbols.type_symbol(owner) else {
            self.compiler.diagnostics.push(schema_internal!(span));
            return Err(out.fail(BuildFailure::empty()));
        };
        if !matches!(symbol.declaration(), TypeDeclarationKind::Enum(_)) {
            self.compiler.report_invalid(
                diagnostic_message!(
                    "diagnostic.schema.default_enum_type_mismatch",
                    "enum default does not match the target type"
                ),
                source.span(),
                out.reborrow(),
            )?;
            return Ok(None);
        }
        let (variant_name, variant_node) = self.ast_access(out.reborrow(), |mut ast_out| {
            let name = source.variant(ast_out.reborrow())?;
            let node = source.variant_node(ast_out.reborrow())?;
            Ok((name, node))
        })?;
        let variant_span = variant_node.span();
        let lookup = self.compiler.symbols.lookup_variant::<BuildFailure>(
            owner,
            variant_name,
            out.reborrow(),
        )?;
        let variant_id = match lookup {
            LookupResult::Found(id) => id,
            LookupResult::Missing => {
                self.compiler.report_invalid(
                    diagnostic_message!(
                        "diagnostic.schema.default_enum_variant_unknown",
                        "enum default refers to an unknown variant"
                    ),
                    variant_span,
                    out.reborrow(),
                )?;
                return Ok(None);
            }
            LookupResult::Ambiguous => {
                self.compiler.report_invalid(
                    diagnostic_message!(
                        "diagnostic.schema.default_enum_variant_ambiguous",
                        "variant name in enum default is ambiguous"
                    ),
                    variant_span,
                    out.reborrow(),
                )?;
                return Ok(None);
            }
        };
        let Some(variant) = self.compiler.symbols.variant_symbol(variant_id) else {
            self.compiler
                .diagnostics
                .push(schema_internal!(variant_span));
            return Err(out.fail(BuildFailure::empty()));
        };
        if variant.state() != SymbolState::Valid {
            return Ok(None);
        }
        let (declared_payload, provided_payload) =
            self.ast_access(out.reborrow(), |mut ast_out| {
                let declared = variant.declaration().payload(ast_out.reborrow())?;
                let declared_kind = declared.kind(ast_out.reborrow())?;
                let provided = source.payload(ast_out.reborrow())?;
                let provided_kind = provided.kind(ast_out.reborrow())?;
                Ok((declared_kind, provided_kind))
            })?;
        match (declared_payload, provided_payload) {
            (VariantPayloadKind::Unit, EnumDefaultPayloadKind::Unit) => {
                return Ok(Some(self.enum_value(
                    variant_id,
                    EnumPayload::Unit,
                    span,
                    out.reborrow(),
                )?));
            }
            (VariantPayloadKind::Single(_), EnumDefaultPayloadKind::Single(value)) => {
                let Some(target) = self.compiler.types.variant_payload_type(variant_id) else {
                    return Ok(None);
                };
                self.push_task(
                    tasks,
                    Task::enum_single(variant_id, target, span),
                    out.reborrow(),
                )?;
                self.push_task(tasks, Task::value(value, target, depth + 1), out.reborrow())?;
            }
            (VariantPayloadKind::Structure(_), EnumDefaultPayloadKind::Structure(object)) => {
                let entries = self.ast_access(out.reborrow(), |ast_out| object.entries(ast_out))?;
                let frame = DefaultFrames::new(self.compiler).prepare_record(
                    PrepareRecordInput {
                        entries,
                        owner: RecordOwner::Variant(variant_id),
                        span: object.span(),
                        depth,
                    },
                    out.reborrow(),
                )?;
                self.push_task(tasks, Task::Enumeration(variant_id, span), out.reborrow())?;
                self.push_task(tasks, Task::Record(frame), out.reborrow())?;
            }
            _ => {
                let payload_span = self
                    .ast_access(out.reborrow(), |ast_out| source.payload(ast_out))?
                    .span();
                self.compiler.report_invalid(
                    diagnostic_message!(
                        "diagnostic.schema.default_enum_payload_mismatch",
                        "enum default payload form does not match the variant declaration"
                    ),
                    payload_span,
                    out.reborrow(),
                )?;
            }
        }
        Ok(None)
    }
}
