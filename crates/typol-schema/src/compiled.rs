mod errors;

mod records;

use opeo::{ErrSlot, OResult, Out};
use typol_core::{Read, SourceReadError};
use typol_semantic::{CompiledSchema, SchemaFreezeInput};

pub use errors::{
    LocationFailure, LocationStatus, SchemaCompileBudgetDomain, SchemaCompileContextFailure,
    SchemaCompileErrors, SchemaInternalCause,
};

use crate::{
    AnnotationCompileInput, AnnotationCompiler, DefaultInputs, DefaultPass, ExpressionCompileInput,
    ExpressionCompiler, SchemaCompileContext, SchemaCompileStage, TypeGraphCompiler,
    context::CompileContext,
    diagnostics::BuildFailure,
    diagnostics::DiagnosticCollector,
    excerpts::SchemaExcerpts,
    locations::SourceLocations,
    module_tree::ModuleTree,
    resolver::{SchemaResolver, SchemaSource, SourceObserver},
    session::{CompileSession, SessionParts, SourceCatalog},
    symbols::SymbolTable,
};

pub(crate) use records::{PreparedRecordCompilation, RecordBuilder};

use records::RecordCompilation;

pub(crate) struct SchemaCompilation;

struct PreparedSchemaCompilation {
    records: PreparedRecordCompilation,
    loaded: SourceCatalog,
    context: CompileContext,
    diagnostics: DiagnosticCollector,
}

impl SchemaCompilation {
    pub(crate) fn compile<'slot, R: SchemaResolver + ?Sized, S: Read<SourceReadError>>(
        root: SchemaSource<'_, S>,
        resolver: &mut R,
        context: &SchemaCompileContext,
        observer: Option<&mut dyn SourceObserver>,
        mut out: Out<'slot, '_, SchemaCompileErrors>,
    ) -> OResult<'slot, CompiledSchema> {
        let parts =
            CompileSession::load(root, resolver, context, observer, out.reborrow())?.into_parts();
        let prepared = Self::compile_loaded(parts, out.reborrow())?;
        Self::freeze(prepared, out.reborrow())
    }

    fn compile_loaded<'slot>(
        parts: SessionParts,
        mut out: Out<'slot, '_, SchemaCompileErrors>,
    ) -> OResult<'slot, PreparedSchemaCompilation> {
        let SessionParts {
            sources: loaded,
            mut diagnostics,
            budget,
        } = parts;
        let limits = budget.limits();
        let context = CompileContext::new::<SchemaCompileErrors>(
            *limits,
            budget.regex_limits(),
            budget.execution(),
            budget.regex_execution(),
            out.reborrow(),
        )?;
        let mut stage_errors = ErrSlot::<BuildFailure>::new();
        if diagnostics.is_stopped()
            || loaded
                .files()
                .first()
                .and_then(|file| file.parsed_ast())
                .is_none()
        {
            return Err(out.fail(SourceLocations::errors_for_sources(
                diagnostics.into_diagnostics(),
                loaded.files(),
                context.execution(),
                *context.limits().excerpts(),
            )));
        }

        budget
            .execution()
            .set_stage(SchemaCompileStage::Compilation as usize);
        let tree = match stage_errors
            .try_call(|out| ModuleTree::build(&loaded, &context, &mut diagnostics, out))
        {
            Ok(tree) if !diagnostics.is_stopped() => tree,
            Ok(_) => {
                return Err(out.fail(SourceLocations::errors_for_sources(
                    diagnostics.into_diagnostics(),
                    loaded.files(),
                    context.execution(),
                    *context.limits().excerpts(),
                )));
            }
            Err(caught) => {
                diagnostics.report_failure(caught.take());
                return Err(out.fail(SourceLocations::errors_for_sources(
                    diagnostics.into_diagnostics(),
                    loaded.files(),
                    context.execution(),
                    *context.limits().excerpts(),
                )));
            }
        };
        let symbols = match stage_errors
            .try_call(|out| SymbolTable::build(&tree, &context, &mut diagnostics, out))
        {
            Ok(symbols) if !diagnostics.is_stopped() => symbols,
            Ok(_) => {
                return Err(out.fail(SourceLocations::errors_for_sources(
                    diagnostics.into_diagnostics(),
                    loaded.files(),
                    context.execution(),
                    *context.limits().excerpts(),
                )));
            }
            Err(caught) => {
                diagnostics.report_failure(caught.take());
                return Err(out.fail(SourceLocations::errors_for_sources(
                    diagnostics.into_diagnostics(),
                    loaded.files(),
                    context.execution(),
                    *context.limits().excerpts(),
                )));
            }
        };
        let mut types = match stage_errors.try_call(|out| {
            TypeGraphCompiler::build(&tree, &symbols, &context, &mut diagnostics, out)
        }) {
            Ok(types) if !diagnostics.is_stopped() => types,
            Ok(_) => {
                return Err(out.fail(SourceLocations::errors_for_sources(
                    diagnostics.into_diagnostics(),
                    loaded.files(),
                    context.execution(),
                    *context.limits().excerpts(),
                )));
            }
            Err(caught) => {
                diagnostics.report_failure(caught.take());
                return Err(out.fail(SourceLocations::errors_for_sources(
                    diagnostics.into_diagnostics(),
                    loaded.files(),
                    context.execution(),
                    *context.limits().excerpts(),
                )));
            }
        };
        budget
            .execution()
            .set_stage(SchemaCompileStage::Compilation as usize);
        let mut annotations = match stage_errors.try_call(|out| {
            AnnotationCompiler::compile(
                AnnotationCompileInput {
                    tree: &tree,
                    symbols: &symbols,
                    types: &types,
                    context: &context,
                    diagnostics: &mut diagnostics,
                },
                out,
            )
        }) {
            Ok(annotations) if !diagnostics.is_stopped() => annotations,
            Ok(_) => {
                return Err(out.fail(SourceLocations::errors_for_sources(
                    diagnostics.into_diagnostics(),
                    loaded.files(),
                    context.execution(),
                    *context.limits().excerpts(),
                )));
            }
            Err(caught) => {
                diagnostics.report_failure(caught.take());
                return Err(out.fail(SourceLocations::errors_for_sources(
                    diagnostics.into_diagnostics(),
                    loaded.files(),
                    context.execution(),
                    *context.limits().excerpts(),
                )));
            }
        };
        let expressions = match stage_errors.try_call(|out| {
            ExpressionCompiler::compile(
                ExpressionCompileInput {
                    tree: &tree,
                    symbols: &symbols,
                    types: &types,
                    annotations: &annotations,
                    diagnostics: &mut diagnostics,
                },
                out,
            )
        }) {
            Ok(expressions) if !diagnostics.is_stopped() => expressions,
            Ok(_) => {
                types.release_shape_ids();
                return Err(out.fail(SourceLocations::errors_for_sources(
                    diagnostics.into_diagnostics(),
                    loaded.files(),
                    context.execution(),
                    *context.limits().excerpts(),
                )));
            }
            Err(caught) => {
                types.release_shape_ids();
                diagnostics.report_failure(caught.take());
                return Err(out.fail(SourceLocations::errors_for_sources(
                    diagnostics.into_diagnostics(),
                    loaded.files(),
                    context.execution(),
                    *context.limits().excerpts(),
                )));
            }
        };
        types.release_shape_ids();
        if let Err(caught) = stage_errors.try_call(|out| {
            annotations.bind_programs(&expressions, &context, &mut diagnostics, out)
        }) {
            diagnostics.report_failure(caught.take());
            return Err(out.fail(SourceLocations::errors_for_sources(
                diagnostics.into_diagnostics(),
                loaded.files(),
                context.execution(),
                *context.limits().excerpts(),
            )));
        }
        let defaults = match stage_errors.try_call(|stage_out| {
            DefaultPass::compile(
                DefaultInputs {
                    symbols: &symbols,
                    types: &types,
                    annotations: &annotations,
                    expressions: &expressions,
                    context: &context,
                    fallback: tree.root_span(),
                },
                &mut diagnostics,
                stage_out,
            )
        }) {
            Ok(defaults) if !diagnostics.is_stopped() => defaults,
            Ok(_) => {
                return Err(out.fail(SourceLocations::errors_for_sources(
                    diagnostics.into_diagnostics(),
                    loaded.files(),
                    context.execution(),
                    *context.limits().excerpts(),
                )));
            }
            Err(caught) => {
                diagnostics.report_failure(caught.take());
                return Err(out.fail(SourceLocations::errors_for_sources(
                    diagnostics.into_diagnostics(),
                    loaded.files(),
                    context.execution(),
                    *context.limits().excerpts(),
                )));
            }
        };
        if diagnostics.has_diagnostics() {
            return Err(out.fail(SourceLocations::errors_for_sources(
                diagnostics.into_diagnostics(),
                loaded.files(),
                context.execution(),
                *context.limits().excerpts(),
            )));
        }
        let records = match stage_errors.try_call(|out| {
            RecordBuilder::compile(
                RecordCompilation {
                    loaded: &loaded,
                    tree: &tree,
                    symbols: &symbols,
                    types: &types,
                    annotations,
                    expressions,
                    defaults,
                    context: &context,
                },
                &mut diagnostics,
                out,
            )
        }) {
            Ok(records) if !diagnostics.has_diagnostics() => records,
            Ok(_) => {
                return Err(out.fail(SourceLocations::errors_for_sources(
                    diagnostics.into_diagnostics(),
                    loaded.files(),
                    context.execution(),
                    *context.limits().excerpts(),
                )));
            }
            Err(caught) => {
                diagnostics.report_failure(caught.take());
                return Err(out.fail(SourceLocations::errors_for_sources(
                    diagnostics.into_diagnostics(),
                    loaded.files(),
                    context.execution(),
                    *context.limits().excerpts(),
                )));
            }
        };
        drop(tree);
        drop(symbols);
        drop(types);
        Ok(PreparedSchemaCompilation {
            records,
            loaded,
            context,
            diagnostics,
        })
    }

    fn freeze<'slot>(
        prepared: PreparedSchemaCompilation,
        out: Out<'slot, '_, SchemaCompileErrors>,
    ) -> OResult<'slot, CompiledSchema> {
        let PreparedSchemaCompilation {
            records,
            loaded,
            context,
            mut diagnostics,
        } = prepared;
        let PreparedRecordCompilation {
            model,
            entry,
            locations,
            fallback,
            default_validation,
        } = records;
        let execution = context.execution();
        execution.set_stage(SchemaCompileStage::Freezing as usize);
        let mut stage_errors = ErrSlot::<BuildFailure>::new();
        let schema = match stage_errors.try_call(|builder_out| {
            context.builder_access(builder_out, fallback, &mut diagnostics, |freeze_out| {
                model.freeze_with_validated_defaults(
                    SchemaFreezeInput::new(entry, &loaded, locations, default_validation),
                    freeze_out,
                )
            })
        }) {
            Ok(schema) => schema,
            Err(caught) => {
                diagnostics.report_failure(caught.take());
                return Err(out.fail(SourceLocations::errors_for_sources(
                    diagnostics.into_diagnostics(),
                    loaded.files(),
                    execution,
                    *context.limits().excerpts(),
                )));
            }
        };
        let excerpts = SchemaExcerpts::records(
            loaded.files(),
            schema.source_locations(),
            *context.limits().excerpts(),
            execution,
        );
        Ok(schema.with_excerpts(excerpts))
    }
}
