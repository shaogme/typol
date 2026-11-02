use crate::{
    annotations::{AnnotationOwner, CompiledConstraint, ConstraintData},
    context::CompileContext,
    diagnostics::{BuildFailure, DiagnosticCollector},
    expressions::ExpressionCompilation,
    regex::CompiledRegexConstraints,
    symbols::{FieldSymbolId, TypeSymbolId, VariantSymbolId},
};
use opeo::{OResult, Out};
use typol_core::{ChargedVec, ExecutionBudget};
use typol_semantic::{EnvironmentRecord, ProgramId};

pub(crate) struct AnnotationSlot {
    rules: ChargedVec<CompiledConstraint>,
    secret: bool,
    environment: Option<EnvironmentRecord>,
}

pub(crate) struct AnnotationSlotParts {
    pub(crate) rules: ChargedVec<CompiledConstraint>,
    pub(crate) secret: bool,
    pub(crate) environment: Option<EnvironmentRecord>,
}

pub(crate) struct AnnotationParts {
    pub(crate) aliases: ChargedVec<AnnotationSlot>,
    pub(crate) fields: ChargedVec<AnnotationSlot>,
    pub(crate) variants: ChargedVec<AnnotationSlot>,
    pub(crate) regexes: CompiledRegexConstraints,
}

pub(crate) struct AnnotationCompilation {
    aliases: ChargedVec<AnnotationSlot>,
    fields: ChargedVec<AnnotationSlot>,
    variant_payloads: ChargedVec<AnnotationSlot>,
    regexes: CompiledRegexConstraints,
}

impl AnnotationCompilation {
    pub(crate) fn new(
        aliases: ChargedVec<AnnotationSlot>,
        fields: ChargedVec<AnnotationSlot>,
        variant_payloads: ChargedVec<AnnotationSlot>,
        regexes: CompiledRegexConstraints,
    ) -> Self {
        Self {
            aliases,
            fields,
            variant_payloads,
            regexes,
        }
    }

    pub(crate) fn into_parts(self) -> AnnotationParts {
        AnnotationParts {
            aliases: self.aliases,
            fields: self.fields,
            variants: self.variant_payloads,
            regexes: self.regexes,
        }
    }

    pub(crate) fn alias_constraints(&self, id: TypeSymbolId) -> Option<&[CompiledConstraint]> {
        self.aliases
            .get(id.index())
            .map(|slot| slot.rules.as_slice())
    }

    pub(crate) fn field_constraints(&self, id: FieldSymbolId) -> Option<&[CompiledConstraint]> {
        self.fields
            .get(id.index())
            .map(|slot| slot.rules.as_slice())
    }

    pub(crate) fn variant_payload_constraints(
        &self,
        id: VariantSymbolId,
    ) -> Option<&[CompiledConstraint]> {
        self.variant_payloads
            .get(id.index())
            .map(|slot| slot.rules.as_slice())
    }

    pub(crate) fn regexes(&self) -> &CompiledRegexConstraints {
        &self.regexes
    }

    pub(crate) fn set_regexes(&mut self, regexes: CompiledRegexConstraints) {
        self.regexes = regexes;
    }

    pub(crate) fn slot_mut(&mut self, owner: AnnotationOwner) -> Option<&mut AnnotationSlot> {
        match owner {
            AnnotationOwner::Alias(id) => self.aliases.get_mut(id.index()),
            AnnotationOwner::Field(id) => self.fields.get_mut(id.index()),
            AnnotationOwner::VariantPayload(id) => self.variant_payloads.get_mut(id.index()),
        }
    }

    pub(crate) fn bind_programs<'slot>(
        &mut self,
        expressions: &ExpressionCompilation,
        context: &CompileContext,
        diagnostics: &mut DiagnosticCollector,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, ()> {
        for slot in self
            .aliases
            .iter_mut()
            .chain(self.fields.iter_mut())
            .chain(self.variant_payloads.iter_mut())
        {
            if let Some(rule) = slot.rules.first() {
                context.consume(1, rule.span(), diagnostics, out.reborrow())?;
            }
            for rule in slot.rules.iter_mut() {
                context.consume(1, rule.span(), diagnostics, out.reborrow())?;
                let span = rule.span();
                if matches!(rule.data(), ConstraintData::All { .. }) {
                    if let Some(index) = expressions.all_index(span, out.reborrow())? {
                        *rule.data_mut() = ConstraintData::All {
                            program: ProgramId::new(index),
                        };
                    }
                }
            }
        }
        Ok(())
    }
}

impl AnnotationSlot {
    pub(crate) fn new<'slot>(
        execution: &ExecutionBudget,
        diagnostics: &mut DiagnosticCollector,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, Self> {
        let rules = diagnostics.new_vec(execution, out.reborrow())?;
        Ok(Self {
            rules,
            secret: false,
            environment: None,
        })
    }

    pub(crate) fn rules_mut(&mut self) -> &mut ChargedVec<CompiledConstraint> {
        &mut self.rules
    }

    pub(crate) fn set_secret(&mut self) {
        self.secret = true;
    }

    pub(crate) fn set_environment(&mut self, environment: EnvironmentRecord) {
        self.environment = Some(environment);
    }

    pub(crate) fn into_parts(self) -> AnnotationSlotParts {
        AnnotationSlotParts {
            rules: self.rules,
            secret: self.secret,
            environment: self.environment,
        }
    }
}
