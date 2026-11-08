//! Defaults and omitted fields are built with continuation tasks, and field-cache dependencies use the same work stack.
mod record_build;
mod tasks;
mod value_build;

use crate::{
    defaults::{
        build::{
            record_build::RecordTaskBuilder,
            tasks::{ArrayBuild, EnumTask, Task, TaskState},
            value_build::ValueDefaultBuilder,
        },
        compiler::{CachedDefault, CachedDefaultState, DefaultCompiler},
        frames::{MapBuild, NamedDefault},
        value_transfer::{EnumPayload, ValueTransfer},
    },
    diagnostics::{BuildFailure, EmergencyDiagnostics},
    symbols::FieldSymbolId,
};
use typol_i18n::diagnostic_message;

use opeo::{OResult, Out};
use typol_core::{ChargedVec, Diagnostic, SourceSpan, ValueError, ValueErrorKind};
use typol_syntax::{AstAccessError, BraceKey};

pub(super) struct DefaultBuilder<'b, 'a, 'd> {
    compiler: &'b mut DefaultCompiler<'a, 'd>,
}

impl<'b, 'a, 'd> DefaultBuilder<'b, 'a, 'd> {
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

    pub(super) fn ensure_field_default<'slot>(
        &mut self,
        root: FieldSymbolId,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, bool> {
        let mut tasks = self.compiler.new_vec(out.reborrow())?;
        self.push_task(&mut tasks, Task::Field(root), out.reborrow())?;
        let mut state = TaskState::new();
        'work: while let Some(task) = tasks.pop() {
            self.compiler.context.consume(
                1,
                self.compiler.fallback,
                self.compiler.diagnostics,
                out.reborrow(),
            )?;
            if self.compiler.diagnostics.is_stopped() {
                return Err(out.fail(BuildFailure::empty()));
            }
            match task {
                Task::Field(id) => {
                    let fallback = self.compiler.fallback;
                    let Some(field) = self.compiler.field_symbol(id) else {
                        self.compiler.diagnostics.abort(schema_internal!(fallback));
                        return Err(out.fail(BuildFailure::empty()));
                    };
                    let Some(cache_state) = self.compiler.cached_default_state(id) else {
                        self.compiler
                            .diagnostics
                            .abort(schema_internal!(field.declaration().span()));
                        return Err(out.fail(BuildFailure::empty()));
                    };
                    match cache_state {
                        CachedDefaultState::Finished => continue,
                        CachedDefaultState::Visiting => {
                            if self.compiler.diagnostics.push_semantic(
                                diagnostic_message!(
                                    "diagnostic.schema.default_recursive_expansion",
                                    "default value has recursive expansion"
                                ),
                                field.declaration().span(),
                            ) {
                                return Err(out.fail(BuildFailure::empty()));
                            }
                            self.set_cached_default(
                                id,
                                CachedDefault::Invalid,
                                field.declaration().span(),
                                out.reborrow(),
                            )?;
                            continue;
                        }
                        CachedDefaultState::Unvisited => {}
                    }
                    let default = self.ast_access(out.reborrow(), |ast_out| {
                        field.declaration().default(ast_out)
                    })?;
                    let Some(source) = default else {
                        continue;
                    };
                    let Some(target) = self.compiler.types.field_type(id) else {
                        self.set_cached_default(
                            id,
                            CachedDefault::Invalid,
                            field.declaration().span(),
                            out.reborrow(),
                        )?;
                        continue;
                    };
                    self.set_cached_default(
                        id,
                        CachedDefault::Visiting,
                        field.declaration().span(),
                        out.reborrow(),
                    )?;
                    self.push_task(&mut tasks, Task::FinishField(id, target), out.reborrow())?;
                    self.push_task(&mut tasks, Task::value(source, target, 0), out.reborrow())?;
                }
                Task::FinishField(id, target) => {
                    if let Some(value) = state.result() {
                        if !self.compiler.validate_local_constraints(
                            value,
                            target,
                            self.compiler
                                .annotations
                                .field_constraints(id)
                                .unwrap_or(&[]),
                            out.reborrow(),
                        )? {
                            state.set_result(None);
                        }
                    }
                    let fallback = self.compiler.fallback;
                    if let Some(value) = state.take_result() {
                        self.compiler
                            .cache_default(id, value, fallback, out.reborrow())?;
                    } else {
                        self.set_cached_default(
                            id,
                            CachedDefault::Invalid,
                            fallback,
                            out.reborrow(),
                        )?;
                    }
                }
                Task::Value(value) => {
                    ValueDefaultBuilder::build(
                        self.compiler,
                        value,
                        &mut tasks,
                        &mut state,
                        out.reborrow(),
                    )?;
                }
                Task::Validate(target) => {
                    if let Some(value) = state.result() {
                        if !self.compiler.validate_constraints(
                            value,
                            target,
                            &[],
                            out.reborrow(),
                        )? {
                            state.set_result(None);
                        }
                    }
                }
                Task::Some(span) => {
                    self.finish_some(span, &mut state, out.reborrow())?;
                }
                Task::Array(frame) => {
                    self.advance_array(frame, &mut tasks, &mut state, out.reborrow())?;
                }
                Task::Map(frame) => {
                    self.advance_map(frame, &mut tasks, &mut state, out.reborrow())?;
                }
                Task::Record(frame) => {
                    if !RecordTaskBuilder::new(self.compiler).build(
                        frame,
                        &mut tasks,
                        &mut state,
                        out.reborrow(),
                    )? {
                        continue 'work;
                    }
                }
                Task::Structure(span) => {
                    self.finish_structure(span, &mut state, out.reborrow())?;
                }
                Task::EnumSingle(EnumTask { id, target, span }) => {
                    if let Some(value) = state.take_result() {
                        if self.compiler.validate_local_constraints(
                            &value,
                            target,
                            self.compiler
                                .annotations
                                .variant_payload_constraints(id)
                                .unwrap_or(&[]),
                            out.reborrow(),
                        )? {
                            let mut payload_values = self.compiler.new_vec(out.reborrow())?;
                            payload_values.try_push(value.value, out.reborrow())?;
                            let payload = EnumPayload::Single(payload_values);
                            let mut value_builder = ValueDefaultBuilder::new(self.compiler);
                            state.set_result(Some(value_builder.enum_value(
                                id,
                                payload,
                                span,
                                out.reborrow(),
                            )?));
                        }
                    }
                }
                Task::Enumeration(id, span) => {
                    let built = if let Some(fields) = state.take_record_result() {
                        let payload = EnumPayload::Structure(fields);
                        let mut value_builder = ValueDefaultBuilder::new(self.compiler);
                        Some(value_builder.enum_value(id, payload, span, out.reborrow())?)
                    } else {
                        None
                    };
                    state.set_result(built);
                }
            }
        }
        let Some(cached) = self.compiler.cached_default(root) else {
            let fallback = self.compiler.fallback;
            self.compiler.diagnostics.abort(schema_internal!(fallback));
            return Err(out.fail(BuildFailure::empty()));
        };
        Ok(matches!(cached, CachedDefault::Valid(_)))
    }

    fn finish_some<'slot>(
        &mut self,
        span: SourceSpan,
        state: &mut TaskState<'a>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ()> {
        if let Some(value) = state.take_result() {
            let built = self.compiler.build_value(
                span,
                |builder, budget, value_out| {
                    ValueTransfer::optional(builder, Some(&value.value), Some(budget), value_out)
                },
                out.reborrow(),
            )?;
            let member_spans = self.compiler.new_vec(out.reborrow())?;
            state.set_result(Some(self.compiler.built(built, span, member_spans)));
        }
        Ok(())
    }

    fn advance_array<'slot>(
        &mut self,
        mut frame: ArrayBuild<'a>,
        tasks: &mut ChargedVec<Task<'a>>,
        state: &mut TaskState<'a>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ()> {
        if frame.index > 0 {
            match state.take_result() {
                Some(value) => frame.values.try_push(value.value, out.reborrow())?,
                None => frame.valid = false,
            }
        }
        let elements = self.ast_access(out.reborrow(), |ast_out| frame.source.elements(ast_out))?;
        let child =
            self.ast_access(out.reborrow(), |ast_out| elements.get(frame.index, ast_out))?;
        if let Some(child) = child {
            frame.spans.try_push(child.span(), out.reborrow())?;
            frame.index += 1;
            let (target, depth) = (frame.target, frame.depth + 1);
            self.push_task(tasks, Task::Array(frame), out.reborrow())?;
            self.push_task(tasks, Task::value(child, target, depth), out.reborrow())?;
        } else if frame.valid {
            let span = frame.source.span();
            let built = self.compiler.build_value(
                span,
                |builder, budget, value_out| {
                    ValueTransfer::array(builder, &frame.values, Some(budget), value_out)
                },
                out.reborrow(),
            )?;
            state.set_result(Some(self.compiler.built(built, span, frame.spans)));
        } else {
            state.set_result(None);
        }
        Ok(())
    }

    fn advance_map<'slot>(
        &mut self,
        mut frame: MapBuild<'a>,
        tasks: &mut ChargedVec<Task<'a>>,
        state: &mut TaskState<'a>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ()> {
        if frame.index > 0 {
            let Some(previous) = frame.index.checked_sub(1) else {
                self.compiler.diagnostics.push(schema_internal!(frame.span));
                return Err(out.fail(BuildFailure::empty()));
            };
            let entry = self.ast_access(out.reborrow(), |ast_out| {
                frame.entries.get(previous, ast_out)
            })?;
            let Some(entry) = entry else {
                self.compiler
                    .diagnostics
                    .abort(schema_internal!(frame.span));
                return Err(out.fail(BuildFailure::empty()));
            };
            let (key, span) = self.ast_access(out.reborrow(), |mut ast_out| {
                let key = entry.key(ast_out.reborrow())?;
                brace_key(key, ast_out.reborrow())
            })?;
            match state.take_result() {
                Some(value) => {
                    let map_entry = NamedDefault {
                        name: key,
                        value: value.value,
                    };
                    self.compiler.budget_access(
                        span,
                        out.reborrow(),
                        |_execution, budget_out| frame.values.try_push(map_entry, budget_out),
                    )?;
                }
                None => frame.valid = false,
            }
        }
        let entry = self.ast_access(out.reborrow(), |ast_out| {
            frame.entries.get(frame.index, ast_out)
        })?;
        if let Some(entry) = entry {
            let child = self.ast_access(out.reborrow(), |ast_out| entry.value(ast_out))?;
            frame.spans.try_push(child.span(), out.reborrow())?;
            frame.index += 1;
            let (target, depth) = (frame.target, frame.depth + 1);
            self.push_task(tasks, Task::Map(frame), out.reborrow())?;
            self.push_task(tasks, Task::value(child, target, depth), out.reborrow())?;
        } else if frame.valid {
            let built = self.compiler.build_value(
                frame.span,
                |builder, budget, value_out| {
                    ValueTransfer::named(builder, &frame.values, false, Some(budget), value_out)
                },
                out.reborrow(),
            )?;
            state.set_result(Some(self.compiler.built(built, frame.span, frame.spans)));
        } else {
            state.set_result(None);
        }
        Ok(())
    }

    fn finish_structure<'slot>(
        &mut self,
        span: SourceSpan,
        state: &mut TaskState<'a>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ()> {
        let built = if let Some(fields) = state.take_record_result() {
            let value = self.compiler.build_value(
                span,
                |builder, budget, value_out| {
                    ValueTransfer::named(builder, &fields, true, Some(budget), value_out)
                },
                out.reborrow(),
            )?;
            let member_spans = self.compiler.new_vec(out.reborrow())?;
            Some(self.compiler.built(value, span, member_spans))
        } else {
            None
        };
        state.set_result(built);
        Ok(())
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

    fn set_cached_default<'slot>(
        &mut self,
        id: FieldSymbolId,
        value: CachedDefault,
        span: SourceSpan,
        out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ()> {
        if self.compiler.set_cached_default(id, value).is_none() {
            self.compiler.diagnostics.abort(schema_internal!(span));
            return Err(out.fail(BuildFailure::empty()));
        }
        Ok(())
    }

    pub(super) fn value_diagnostic(error: ValueError, span: SourceSpan) -> Diagnostic {
        match error.kind() {
            ValueErrorKind::ResourceLimitExceeded => EmergencyDiagnostics::resource(span),
            ValueErrorKind::AllocationFailed => EmergencyDiagnostics::allocation(span),
            ValueErrorKind::CapacityExceeded => EmergencyDiagnostics::capacity(span),
            ValueErrorKind::InternalInvariantFailure => match error.context() {
                Some(context) => EmergencyDiagnostics::internal(span, context),
                None => schema_internal!(span),
            },
            ValueErrorKind::EnvironmentReferenceRequired => schema_internal!(span),
            ValueErrorKind::DuplicateMapKey => {
                let context = error.duplicate_indices().map_or_else(
                    || schema_context!("value-construction", "duplicate-map-index-missing"),
                    |(_, second)| {
                        schema_context!("value-construction", "duplicate-map-key")
                            .with_object_id(second as u64)
                    },
                );
                EmergencyDiagnostics::internal(span, context)
            }
            ValueErrorKind::DuplicateFieldName => {
                let context = error.duplicate_indices().map_or_else(
                    || schema_context!("value-construction", "duplicate-field-index-missing"),
                    |(_, second)| {
                        schema_context!("value-construction", "duplicate-field-name")
                            .with_object_id(second as u64)
                    },
                );
                EmergencyDiagnostics::internal(span, context)
            }
        }
    }
}

fn brace_key<'slot, 'a>(
    entry: BraceKey<'a>,
    mut out: Out<'slot, '_, AstAccessError>,
) -> OResult<'slot, (&'a str, SourceSpan)> {
    match entry {
        BraceKey::Identifier(name) => Ok((name.text(out.reborrow())?, name.span())),
        BraceKey::String(value) => Ok((value.value(out.reborrow())?, value.span())),
    }
}
