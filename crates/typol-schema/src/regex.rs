use opeo::{ErrSlot, OResult, Out};
use typol_core::{
    ChargedVec, Diagnostic, DiagnosticCode, DiagnosticContext, DiagnosticMessage, SourceSpan,
    diagnostic_context,
};
use typol_i18n::diagnostic_message;
use typol_policy::RegexLimits;
use typol_regex::{RegexCompileError, RegexCompileOptions, RegexCompiler};
use typol_syntax::{Annotation, AnnotationData};

use crate::{
    context::CompileContext,
    diagnostics::{BuildFailure, DiagnosticCollector, EmergencyDiagnostics},
};

pub use typol_semantic::{CompiledRegexConstraint, CompiledRegexConstraints};

static REGEX_PATTERN_FAILURE: DiagnosticContext =
    diagnostic_context!("typol-schema", "regex-pattern", "pattern-start-offset");

pub(crate) struct RegexConstraintCompiler<'a> {
    limits: &'a RegexLimits,
    context: &'a CompileContext,
    constraints: ChargedVec<CompiledRegexConstraint>,
    attempted_pattern_bytes: usize,
    pattern_bytes: usize,
    instruction_count: usize,
    range_count: usize,
    compile_errors: ErrSlot<RegexCompileError>,
}

impl<'a> RegexConstraintCompiler<'a> {
    pub(crate) fn constraint_count(&self) -> usize {
        self.constraints.len()
    }

    pub(crate) fn new<'slot>(
        context: &'a CompileContext,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, Self> {
        let constraints = ChargedVec::new_regex(Some(context.regex_execution()), out.reborrow())?;
        Ok(RegexConstraintCompiler {
            limits: context.regex_limits(),
            context,
            constraints,
            attempted_pattern_bytes: 0,
            pattern_bytes: 0,
            instruction_count: 0,
            range_count: 0,
            compile_errors: ErrSlot::new(),
        })
    }

    pub(crate) fn compile_resolved_annotation<'slot>(
        &mut self,
        annotation: Annotation<'_>,
        valid_target: bool,
        diagnostics: &mut DiagnosticCollector,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ()> {
        if !valid_target {
            diagnostics.push(self.make_diagnostic(
                DiagnosticCode::SCHEMA_SEMANTIC_ERROR,
                diagnostic_message!(
                    "diagnostic.schema.regex_target_invalid",
                    "@regex can be used only on string or option<string>"
                ),
                annotation.span(),
                diagnostics,
            ));
            return Ok(());
        }
        self.compile_pattern_annotation(annotation, diagnostics, out.reborrow())
    }

    pub(crate) fn finish(self) -> CompiledRegexConstraints {
        CompiledRegexConstraints::new(
            self.constraints,
            self.pattern_bytes,
            self.instruction_count,
            self.range_count,
        )
    }

    fn make_diagnostic(
        &self,
        code: DiagnosticCode,
        message: DiagnosticMessage<'static>,
        span: SourceSpan,
        diagnostics: &DiagnosticCollector,
    ) -> Diagnostic {
        make_diagnostic(code, message, span, diagnostics)
    }

    fn compile_pattern_annotation<'slot>(
        &mut self,
        annotation: Annotation<'_>,
        diagnostics: &mut DiagnosticCollector,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ()> {
        let data = self.context.ast_access(
            out.reborrow(),
            |ast_out| annotation.data(ast_out),
            BuildFailure::from,
        )?;
        let AnnotationData::Regex(pattern) = data else {
            diagnostics.push(self.make_diagnostic(
                DiagnosticCode::SCHEMA_SEMANTIC_ERROR,
                diagnostic_message!(
                    "diagnostic.schema.regex_annotation_invalid",
                    "@regex annotation compile path is invalid"
                ),
                annotation.span(),
                diagnostics,
            ));
            return Ok(());
        };
        let pattern = self.context.ast_access(
            out.reborrow(),
            |ast_out| pattern.value(ast_out),
            BuildFailure::from,
        )?;

        if self.constraints.len() >= self.limits.max_regex_count() {
            diagnostics.push(EmergencyDiagnostics::resource(annotation.span()));
            return Ok(());
        }
        let Some(attempted_pattern_bytes) = self.attempted_pattern_bytes.checked_add(pattern.len())
        else {
            self.attempted_pattern_bytes = self.limits.max_regex_total_pattern_bytes();
            diagnostics.push(EmergencyDiagnostics::resource(annotation.span()));
            return Ok(());
        };
        if attempted_pattern_bytes > self.limits.max_regex_total_pattern_bytes() {
            self.attempted_pattern_bytes = self.limits.max_regex_total_pattern_bytes();
            diagnostics.push(EmergencyDiagnostics::resource(annotation.span()));
            return Ok(());
        }
        self.attempted_pattern_bytes = attempted_pattern_bytes;
        if pattern.len() > self.limits.max_regex_pattern_bytes()
            || self.instruction_count >= self.limits.max_regex_instructions_total()
        {
            diagnostics.push(EmergencyDiagnostics::resource(annotation.span()));
            return Ok(());
        }

        let remaining_instructions = self
            .limits
            .max_regex_instructions_total()
            .saturating_sub(self.instruction_count);
        let remaining_ranges = self
            .limits
            .max_regex_ranges_total()
            .saturating_sub(self.range_count);

        let regex = match self.compile_errors.try_call(|regex_out| {
            RegexCompiler::compile_with_options(
                pattern,
                RegexCompileOptions::new(
                    self.limits,
                    self.context.regex_execution(),
                    remaining_instructions,
                    remaining_ranges,
                ),
                regex_out,
            )
        }) {
            Ok(regex) => regex,
            Err(caught) => {
                let error = caught.take();
                diagnostics.push(compile_error_diagnostic(
                    error,
                    annotation.span(),
                    diagnostics,
                ));
                return Ok(());
            }
        };
        let Some(total_pattern_bytes) = self.pattern_bytes.checked_add(pattern.len()) else {
            diagnostics.push(EmergencyDiagnostics::resource(annotation.span()));
            return Ok(());
        };
        let Some(total_instructions) = self
            .instruction_count
            .checked_add(regex.instruction_count())
        else {
            diagnostics.push(EmergencyDiagnostics::resource(annotation.span()));
            return Ok(());
        };
        if total_instructions > self.limits.max_regex_instructions_total() {
            diagnostics.push(EmergencyDiagnostics::resource(annotation.span()));
            return Ok(());
        }
        let Some(total_ranges) = self.range_count.checked_add(regex.range_count()) else {
            diagnostics.push(EmergencyDiagnostics::resource(annotation.span()));
            return Ok(());
        };
        if total_ranges > self.limits.max_regex_ranges_total() {
            diagnostics.push(EmergencyDiagnostics::resource(annotation.span()));
            return Ok(());
        }

        self.constraints.try_reserve(1, out.reborrow())?;
        self.constraints.try_push(
            CompiledRegexConstraint::new(annotation.span(), regex),
            out.reborrow(),
        )?;
        self.pattern_bytes = total_pattern_bytes;
        self.instruction_count = total_instructions;
        self.range_count = total_ranges;
        Ok(())
    }
}

fn compile_error_diagnostic(
    error: RegexCompileError,
    span: SourceSpan,
    diagnostics: &DiagnosticCollector,
) -> Diagnostic {
    match error {
        RegexCompileError::Syntax(error) => {
            let mut diagnostic = make_diagnostic(
                DiagnosticCode::REGEX_SYNTAX_ERROR,
                diagnostic_message!(
                    "diagnostic.schema.regex_pattern_syntax_invalid",
                    "regex pattern syntax is invalid"
                ),
                span,
                diagnostics,
            );
            diagnostic.set_context(Some(
                REGEX_PATTERN_FAILURE.with_object_id(error.span().start() as u64),
            ));
            diagnostic
        }
        RegexCompileError::ResourceLimitExceeded(_) => EmergencyDiagnostics::resource(span),
        RegexCompileError::AllocationFailed => EmergencyDiagnostics::allocation(span),
        RegexCompileError::CapacityExceeded => EmergencyDiagnostics::capacity(span),
        RegexCompileError::InternalInvariantFailure(error) => {
            EmergencyDiagnostics::internal(span, error.diagnostic_context())
        }
    }
}

fn make_diagnostic(
    code: DiagnosticCode,
    message: DiagnosticMessage<'static>,
    span: SourceSpan,
    diagnostics: &DiagnosticCollector,
) -> Diagnostic {
    diagnostics.make(code, message, Some(span))
}
