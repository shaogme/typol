use crate::{
    diagnostics::{CodecErrors, CodecStage, Failure},
    options::{BindOptions, WriteOptions},
};
use alloc::string::String;
use core::{cell::RefCell, cmp::Ordering, fmt};
use opeo::{ErrSlot, OResult, Out, ResultOutExt};
use typol_config::{
    ConfigParseErrors, ConfigParseOptions, ConfigValidationOptions, ConfigValueErrors,
};
use typol_core::{
    BudgetError, CapacityExceeded, ChargedString, ChargedVec, ExecutionBudget, ExecutionLimits,
    ResourceLimit, ValueError, ValueScalar,
};
use typol_i18n::Translator;
use typol_semantic::SchemaAccessError;

#[cfg(feature = "serde")]
use typol_semantic::CopyError;

pub(crate) struct TargetCharge {
    storage: ChargedVec<u8>,
}

pub(crate) struct SessionBudget {
    execution: ExecutionBudget,
    max_text_bytes: usize,
    max_string_bytes: usize,
    max_output_bytes: usize,
    output_bytes: usize,
    translator: Translator,
    budget_errors: RefCell<Option<ErrSlot<BudgetError>>>,
    value_errors: RefCell<Option<ErrSlot<ValueError>>>,
    schema_errors: RefCell<Option<ErrSlot<SchemaAccessError>>>,
    #[cfg(feature = "serde")]
    copy_errors: RefCell<Option<ErrSlot<CopyError>>>,
    parse_errors: RefCell<Option<ErrSlot<ConfigParseErrors>>>,
    validation_errors: RefCell<Option<ErrSlot<ConfigValueErrors>>>,
}

impl SessionBudget {
    pub(crate) fn codec_from_budget<'slot, 'borrow, T>(
        stage: CodecStage,
        translator: Translator,
        operation: impl for<'call> FnOnce(Out<'call, 'call, BudgetError>) -> OResult<'call, T>,
        out: Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, T> {
        let mut errors = ErrSlot::<BudgetError>::new();
        match errors.try_call(operation) {
            Ok(value) => Ok(value),
            Err(caught) => Err(out.fail(CodecErrors::emergency(
                stage,
                caught.take().into(),
                translator,
            ))),
        }
    }

    pub(crate) fn new<'slot>(
        execution: ExecutionLimits,
        max_text_bytes: usize,
        max_output_bytes: usize,
        translator: Translator,
        mut out: Out<'slot, '_, BudgetError>,
    ) -> OResult<'slot, Self> {
        let execution = ExecutionBudget::new(execution, out.reborrow())?;
        Ok(Self {
            execution,
            max_text_bytes,
            max_string_bytes: usize::MAX,
            max_output_bytes,
            output_bytes: 0,
            translator,
            budget_errors: RefCell::new(None),
            value_errors: RefCell::new(None),
            schema_errors: RefCell::new(None),
            #[cfg(feature = "serde")]
            copy_errors: RefCell::new(None),
            parse_errors: RefCell::new(None),
            validation_errors: RefCell::new(None),
        })
    }

    pub(crate) fn binding<'slot>(
        options: BindOptions,
        mut out: Out<'slot, '_, BudgetError>,
    ) -> OResult<'slot, Self> {
        let session = Self::new(
            options.execution(),
            options.execution().max_steps(),
            0,
            options.translator(),
            out.reborrow(),
        )?;
        Ok(session)
    }

    pub(crate) fn codec<'slot>(
        options: WriteOptions,
        mut out: Out<'slot, '_, BudgetError>,
    ) -> OResult<'slot, Self> {
        let mut session = Self::new(
            *options.validation().execution(),
            options.validation().config().max_constructed_text_bytes(),
            options.max_output_bytes(),
            options.validation().translator(),
            out.reborrow(),
        )?;
        session.max_string_bytes = options.validation().config().max_string_bytes();
        Ok(session)
    }

    pub(crate) fn default_config<'slot>(
        validation: ConfigValidationOptions,
        max_output_bytes: usize,
        mut out: Out<'slot, '_, BudgetError>,
    ) -> OResult<'slot, Self> {
        let mut session = Self::new(
            *validation.execution(),
            validation.config().max_constructed_text_bytes(),
            max_output_bytes,
            validation.translator(),
            out.reborrow(),
        )?;
        session.max_string_bytes = validation.config().max_string_bytes();
        Ok(session)
    }

    pub(crate) fn parsing<'slot>(
        options: ConfigParseOptions,
        mut out: Out<'slot, '_, BudgetError>,
    ) -> OResult<'slot, Self> {
        let mut session = Self::new(
            *options.execution(),
            options.config().max_constructed_text_bytes(),
            0,
            options.translator(),
            out.reborrow(),
        )?;
        session.max_string_bytes = options.config().max_string_bytes();
        Ok(session)
    }

    pub(crate) fn parsing_with_budget<'slot>(
        options: ConfigParseOptions,
        budget: &ExecutionBudget,
        mut out: Out<'slot, '_, BudgetError>,
    ) -> OResult<'slot, Self> {
        let execution = budget.try_clone(out.reborrow())?;
        Ok(Self {
            execution,
            max_text_bytes: options.config().max_constructed_text_bytes(),
            max_string_bytes: options.config().max_string_bytes(),
            max_output_bytes: 0,
            output_bytes: 0,
            translator: options.translator(),
            budget_errors: RefCell::new(None),
            value_errors: RefCell::new(None),
            schema_errors: RefCell::new(None),
            #[cfg(feature = "serde")]
            copy_errors: RefCell::new(None),
            parse_errors: RefCell::new(None),
            validation_errors: RefCell::new(None),
        })
    }

    pub(crate) fn execution(&self) -> &ExecutionBudget {
        &self.execution
    }

    pub(crate) const fn translator(&self) -> Translator {
        self.translator
    }

    pub(crate) fn budget_codec<'slot, 'borrow, T>(
        &self,
        stage: CodecStage,
        operation: impl for<'call> FnOnce(Out<'call, 'call, BudgetError>) -> OResult<'call, T>,
        out: Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, T> {
        match self
            .budget_errors
            .borrow_mut()
            .get_or_insert_with(ErrSlot::new)
            .try_call(operation)
        {
            Ok(value) => Ok(value),
            Err(caught) => Err(out.fail(CodecErrors::emergency(
                stage,
                caught.take().into(),
                self.translator,
            ))),
        }
    }

    #[cfg(feature = "serde")]
    pub(crate) fn budget_as<'slot, 'borrow, T, E>(
        &self,
        operation: impl for<'call> FnOnce(Out<'call, 'call, BudgetError>) -> OResult<'call, T>,
        out: Out<'slot, 'borrow, E>,
    ) -> OResult<'slot, T>
    where
        E: From<BudgetError>,
    {
        match self
            .budget_errors
            .borrow_mut()
            .get_or_insert_with(ErrSlot::new)
            .try_call(operation)
        {
            Ok(value) => Ok(value),
            Err(caught) => Err(out.fail(caught.take().into())),
        }
    }

    pub(crate) fn value_codec<'slot, 'borrow, T>(
        &self,
        stage: CodecStage,
        operation: impl for<'call> FnOnce(Out<'call, 'call, ValueError>) -> OResult<'call, T>,
        out: Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, T> {
        match self
            .value_errors
            .borrow_mut()
            .get_or_insert_with(ErrSlot::new)
            .try_call(operation)
        {
            Ok(value) => Ok(value),
            Err(caught) => Err(out.fail(CodecErrors::emergency(
                stage,
                Failure::value_error(caught.take()),
                self.translator,
            ))),
        }
    }

    #[cfg(feature = "serde")]
    pub(crate) fn value_as<'slot, 'borrow, T, E>(
        &self,
        operation: impl for<'call> FnOnce(Out<'call, 'call, ValueError>) -> OResult<'call, T>,
        out: Out<'slot, 'borrow, E>,
    ) -> OResult<'slot, T>
    where
        E: From<ValueError>,
    {
        match self
            .value_errors
            .borrow_mut()
            .get_or_insert_with(ErrSlot::new)
            .try_call(operation)
        {
            Ok(value) => Ok(value),
            Err(caught) => Err(out.fail(caught.take().into())),
        }
    }

    pub(crate) fn schema_codec<'slot, 'borrow, T>(
        &self,
        stage: CodecStage,
        operation: impl for<'call> FnOnce(Out<'call, 'call, SchemaAccessError>) -> OResult<'call, T>,
        out: Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, T> {
        match self
            .schema_errors
            .borrow_mut()
            .get_or_insert_with(ErrSlot::new)
            .try_call(operation)
        {
            Ok(value) => Ok(value),
            Err(caught) => Err(out.fail(CodecErrors::emergency(
                stage,
                Failure::schema_access(caught.take()),
                self.translator,
            ))),
        }
    }

    #[cfg(feature = "serde")]
    pub(crate) fn schema_as<'slot, 'borrow, T, E>(
        &self,
        operation: impl for<'call> FnOnce(Out<'call, 'call, SchemaAccessError>) -> OResult<'call, T>,
        out: Out<'slot, 'borrow, E>,
    ) -> OResult<'slot, T>
    where
        E: From<SchemaAccessError>,
    {
        match self
            .schema_errors
            .borrow_mut()
            .get_or_insert_with(ErrSlot::new)
            .try_call(operation)
        {
            Ok(value) => Ok(value),
            Err(caught) => Err(out.fail(caught.take().into())),
        }
    }

    #[cfg(feature = "serde")]
    pub(crate) fn copy_as<'slot, 'borrow, T, E>(
        &self,
        operation: impl for<'call> FnOnce(Out<'call, 'call, CopyError>) -> OResult<'call, T>,
        out: Out<'slot, 'borrow, E>,
    ) -> OResult<'slot, T>
    where
        E: From<CopyError>,
    {
        match self
            .copy_errors
            .borrow_mut()
            .get_or_insert_with(ErrSlot::new)
            .try_call(operation)
        {
            Ok(value) => Ok(value),
            Err(caught) => Err(out.fail(caught.take().into())),
        }
    }

    pub(crate) fn parse_codec<'slot, 'borrow, T>(
        &self,
        operation: impl for<'call> FnOnce(Out<'call, 'call, ConfigParseErrors>) -> OResult<'call, T>,
        out: Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, T> {
        match self
            .parse_errors
            .borrow_mut()
            .get_or_insert_with(ErrSlot::new)
            .try_call(operation)
        {
            Ok(value) => Ok(value),
            Err(caught) => Err(out.fail(CodecErrors::parsing(caught.take(), self.translator))),
        }
    }

    pub(crate) fn validation_codec<'slot, 'borrow, T>(
        &self,
        operation: impl for<'call> FnOnce(Out<'call, 'call, ConfigValueErrors>) -> OResult<'call, T>,
        out: Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, T> {
        match self
            .validation_errors
            .borrow_mut()
            .get_or_insert_with(ErrSlot::new)
            .try_call(operation)
        {
            Ok(value) => Ok(value),
            Err(caught) => Err(out.fail(CodecErrors::validation(caught.take(), self.translator))),
        }
    }

    pub(crate) fn output_codec<'slot, 'borrow>(
        &mut self,
        bytes: usize,
        out: Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, ()> {
        let Self {
            execution,
            max_text_bytes,
            max_output_bytes,
            output_bytes,
            budget_errors,
            ..
        } = self;
        let Some(next) = output_bytes.checked_add(bytes) else {
            return Err(out.fail(CodecErrors::emergency(
                CodecStage::Encoding,
                Failure::Capacity(CapacityExceeded::FormatOutput),
                self.translator,
            )));
        };
        if next > *max_output_bytes {
            return Err(out.fail(CodecErrors::emergency(
                CodecStage::Encoding,
                Failure::ResourceLimit(ResourceLimit::ConfigFormatOutputBytes),
                self.translator,
            )));
        }
        let mut budget_slot = budget_errors.borrow_mut();
        let result = budget_slot
            .get_or_insert_with(ErrSlot::new)
            .try_call(|mut budget_out| {
                execution.construct_text(bytes, *max_text_bytes, budget_out.reborrow())?;
                execution.consume(bytes, budget_out.reborrow())?;
                *output_bytes = next;
                Ok(())
            });
        match result {
            Ok(()) => Ok(()),
            Err(caught) => Err(out.fail(CodecErrors::emergency(
                CodecStage::Encoding,
                caught.take().into(),
                self.translator,
            ))),
        }
    }

    pub(crate) const fn max_text_bytes(&self) -> usize {
        self.max_text_bytes
    }

    pub(crate) fn charge_environment_reference_text<'slot>(
        &self,
        name: &str,
        fallback: Option<ValueScalar<'_>>,
        out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ()> {
        let fallback_bytes = match fallback {
            Some(ValueScalar::String(value)) => value.len(),
            _ => 0,
        };
        if name.len() > self.max_string_bytes || fallback_bytes > self.max_string_bytes {
            return Err(out.fail(CodecErrors::emergency(
                CodecStage::Conversion,
                Failure::ResourceLimit(ResourceLimit::StringBytes),
                self.translator,
            )));
        }
        let Some(bytes) = name.len().checked_add(fallback_bytes) else {
            return Err(out.fail(CodecErrors::emergency(
                CodecStage::Conversion,
                Failure::Capacity(CapacityExceeded::RustConversion),
                self.translator,
            )));
        };
        self.budget_codec(
            CodecStage::Conversion,
            |budget_out| {
                self.execution
                    .construct_text(bytes, self.max_text_bytes, budget_out)
            },
            out,
        )
    }

    pub(crate) fn visit<'slot>(&self, mut out: Out<'slot, '_, BudgetError>) -> OResult<'slot, ()> {
        self.execution.consume(1, out.reborrow())
    }

    pub(crate) fn copy_text<'slot>(
        &self,
        value: &str,
        mut out: Out<'slot, '_, BudgetError>,
    ) -> OResult<'slot, ChargedString> {
        self.execution
            .construct_text(value.len(), self.max_text_bytes, out.reborrow())?;
        // Reads and writes are charged separately; ChargedString::copy accounts for the write.
        self.execution.consume(value.len(), out.reborrow())?;
        ChargedString::copy(value, Some(&self.execution), out.reborrow())
    }

    pub(crate) fn copy_target_text<'slot>(
        &self,
        target_charge: &mut TargetCharge,
        text: &str,
        mut out: Out<'slot, '_, BudgetError>,
    ) -> OResult<'slot, String> {
        self.execution
            .construct_text(text.len(), self.max_text_bytes, out.reborrow())?;
        self.execution.consume(text.len(), out.reborrow())?;

        let mut target = String::new();
        target
            .try_reserve_exact(text.len())
            .map_err(|_| BudgetError::Allocation)
            .store_err_into(out.reborrow())?;
        target_charge.add(target.capacity(), out.reborrow())?;

        self.execution.consume(text.len(), out.reborrow())?;
        target.push_str(text);
        Ok(target)
    }

    pub(crate) fn compare_text<'slot>(
        &self,
        left: &str,
        right: &str,
        mut out: Out<'slot, '_, BudgetError>,
    ) -> OResult<'slot, Ordering> {
        self.visit(out.reborrow())?;
        self.execution.compare_text(left, right, out.reborrow())
    }

    pub(crate) fn check_output<'slot>(
        &self,
        bytes: usize,
        mut out: Out<'slot, '_, BudgetError>,
    ) -> OResult<'slot, ()> {
        let next = self
            .output_bytes
            .checked_add(bytes)
            .ok_or(BudgetError::CapacityExceeded(
                CapacityExceeded::FormatOutput,
            ))
            .store_err(out.reborrow())?;
        if next > self.max_output_bytes {
            return Err(out.fail(BudgetError::ResourceLimit(
                ResourceLimit::ConfigFormatOutputBytes,
            )));
        }
        Ok(())
    }

    pub(crate) fn workspace<'slot, T>(
        &self,
        mut out: Out<'slot, '_, BudgetError>,
    ) -> OResult<'slot, ChargedVec<T>> {
        ChargedVec::new(Some(&self.execution), out.reborrow())
    }
}

impl fmt::Debug for SessionBudget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SessionBudget")
            .field("steps", &self.execution.steps())
            .field("workspace_bytes", &self.execution.workspace_bytes())
            .field("output_bytes", &self.output_bytes)
            .finish()
    }
}

impl TargetCharge {
    pub(crate) fn new<'slot>(
        session: &SessionBudget,
        out: Out<'slot, '_, BudgetError>,
    ) -> OResult<'slot, Self> {
        Ok(Self {
            storage: session.workspace(out)?,
        })
    }

    pub(crate) fn add<'slot>(
        &mut self,
        bytes: usize,
        out: Out<'slot, '_, BudgetError>,
    ) -> OResult<'slot, ()> {
        self.storage.charge_storage(bytes, out)
    }
}

pub(crate) struct NodeBudget {
    max_nodes: usize,
    max_depth: usize,
    nodes: usize,
}

impl NodeBudget {
    pub(crate) const fn new(max_nodes: usize, max_depth: usize) -> Self {
        Self {
            max_nodes,
            max_depth,
            nodes: 0,
        }
    }

    pub(crate) fn check_depth<'slot>(
        &self,
        depth: usize,
        budget: &ExecutionBudget,
        mut out: Out<'slot, '_, BudgetError>,
    ) -> OResult<'slot, ()> {
        if depth > self.max_depth {
            return Err(out.fail(BudgetError::ResourceLimit(ResourceLimit::TypeBindingDepth)));
        }
        budget.consume(1, out.reborrow())
    }

    pub(crate) fn node<'slot>(
        &mut self,
        depth: usize,
        budget: &ExecutionBudget,
        mut out: Out<'slot, '_, BudgetError>,
    ) -> OResult<'slot, ()> {
        self.check_depth(depth, budget, out.reborrow())?;
        let next = self
            .nodes
            .checked_add(1)
            .ok_or(BudgetError::CapacityExceeded(
                CapacityExceeded::RustConversion,
            ))
            .store_err(out.reborrow())?;
        if next > self.max_nodes {
            return Err(out.fail(BudgetError::ResourceLimit(ResourceLimit::TypeBindingNodes)));
        }
        self.nodes = next;
        Ok(())
    }

    pub(crate) const fn nodes(&self) -> usize {
        self.nodes
    }
}
