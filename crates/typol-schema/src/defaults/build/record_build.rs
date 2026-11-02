use crate::{
    defaults::{
        build::tasks::{Task, TaskState},
        compiler::DefaultCompiler,
        frames::{NamedDefault, RecordBuild},
        value_transfer::ValueTransfer,
    },
    diagnostics::BuildFailure,
    symbols::SymbolState,
    types::ResolvedTypeKind,
};
use typol_i18n::diagnostic_message;

use opeo::{OResult, Out};
use typol_core::ChargedVec;
use typol_syntax::AstAccessError;

pub(super) struct RecordTaskBuilder<'b, 'a, 'd> {
    compiler: &'b mut DefaultCompiler<'a, 'd>,
}

impl<'b, 'a, 'd> RecordTaskBuilder<'b, 'a, 'd> {
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
        &mut self,
        mut frame: RecordBuild<'a>,
        tasks: &mut ChargedVec<Task<'a>>,
        state: &mut TaskState<'a>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, bool> {
        if let Some((id, supplied)) = frame.waiting.take() {
            let Some(field) = self.compiler.field_symbol(id) else {
                self.compiler
                    .diagnostics
                    .abort(schema_internal!(frame.span));
                return Err(out.fail(BuildFailure::empty()));
            };
            let target = match self.compiler.types.field_type(id) {
                Some(target) => target,
                None => {
                    self.compiler
                        .diagnostics
                        .abort(schema_internal!(field.declaration().span()));
                    return Err(out.fail(BuildFailure::empty()));
                }
            };
            let built = if supplied {
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
                state.take_result()
            } else {
                self.compiler.clone_cached_default(
                    id,
                    frame.depth + 1,
                    frame.span,
                    out.reborrow(),
                )?
            };
            if let Some(built) = built {
                let name =
                    self.ast_access(out.reborrow(), |ast_out| field.declaration().name(ast_out))?;
                let field_value = NamedDefault {
                    name,
                    value: built.value,
                };
                self.compiler.budget_access(
                    field.declaration().span(),
                    out.reborrow(),
                    |_execution, budget_out| frame.values.try_push(field_value, budget_out),
                )?;
            } else {
                frame.valid = false;
            }
        }
        while let Some(id) = frame.fields.get(frame.index).copied() {
            self.compiler.context.consume(
                1,
                frame.span,
                self.compiler.diagnostics,
                out.reborrow(),
            )?;
            let supplied = frame.supplied.get(frame.index).copied();
            let Some(supplied) = supplied else {
                self.compiler
                    .diagnostics
                    .abort(schema_internal!(frame.span));
                return Err(out.fail(BuildFailure::empty()));
            };
            frame.index += 1;
            let Some(field) = self.compiler.field_symbol(id) else {
                self.compiler
                    .diagnostics
                    .abort(schema_internal!(frame.span));
                return Err(out.fail(BuildFailure::empty()));
            };
            if field.state() != SymbolState::Valid {
                frame.valid = false;
                continue;
            }
            let Some(target) = self.compiler.types.field_type(id) else {
                frame.valid = false;
                continue;
            };
            if let Some(entry) = supplied {
                frame.waiting = Some((id, true));
                let depth = frame.depth + 1;
                let value = self.ast_access(out.reborrow(), |ast_out| entry.value(ast_out))?;
                Self::push_task(tasks, Task::Record(frame), out.reborrow())?;
                Self::push_task(tasks, Task::value(value, target, depth), out.reborrow())?;
                return Ok(false);
            } else if self
                .ast_access(out.reborrow(), |ast_out| {
                    field.declaration().default(ast_out)
                })?
                .is_some()
            {
                frame.waiting = Some((id, false));
                Self::push_task(tasks, Task::Record(frame), out.reborrow())?;
                Self::push_task(tasks, Task::Field(id), out.reborrow())?;
                return Ok(false);
            } else if matches!(
                self.compiler.types.type_use(target).map(|item| item.kind()),
                Some(ResolvedTypeKind::Option(_))
            ) {
                self.compiler
                    .add_value_node(field.declaration().span(), out.reborrow())?;
                let member_spans = self.compiler.new_vec(out.reborrow())?;
                let span = field.declaration().span();
                let config_value = self.compiler.build_value(
                    span,
                    |builder, budget, value_out| {
                        ValueTransfer::optional(builder, None, Some(budget), value_out)
                    },
                    out.reborrow(),
                )?;
                let value = self.compiler.built(config_value, span, member_spans);
                if !self.compiler.validate_constraints(
                    &value,
                    target,
                    self.compiler
                        .annotations
                        .field_constraints(id)
                        .unwrap_or(&[]),
                    out.reborrow(),
                )? {
                    frame.valid = false;
                    continue;
                }
                let name =
                    self.ast_access(out.reborrow(), |ast_out| field.declaration().name(ast_out))?;
                let field_value = NamedDefault {
                    name,
                    value: value.value,
                };
                self.compiler.budget_access(
                    field.declaration().span(),
                    out.reborrow(),
                    |_execution, budget_out| frame.values.try_push(field_value, budget_out),
                )?;
            } else {
                let name_node = self.ast_access(out.reborrow(), |ast_out| {
                    field.declaration().name_node(ast_out)
                })?;
                self.compiler.report_invalid(
                    diagnostic_message!(
                        "diagnostic.schema.default_struct_required_field_missing",
                        "struct default omits a required field"
                    ),
                    name_node.span(),
                    out.reborrow(),
                )?;
                frame.valid = false;
            }
            if self.compiler.diagnostics.is_stopped() {
                return Err(out.fail(BuildFailure::empty()));
            }
        }
        state.set_record_result(frame.valid.then_some(frame.values));
        Ok(true)
    }

    fn push_task<'slot>(
        tasks: &mut ChargedVec<Task<'a>>,
        task: Task<'a>,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ()> {
        tasks.try_reserve(1, out.reborrow())?;
        tasks.try_push(task, out.reborrow())?;
        Ok(())
    }
}
