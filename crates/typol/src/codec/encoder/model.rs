use crate::{
    diagnostics::{CodecErrors, CodecStage, Failure},
    session::SessionBudget,
};
use opeo::{OResult, Out};
use typol_core::{
    ChargedVec, DiagnosticContext, ValueEntries, ValueEntryView, ValueView, WriteError,
    diagnostic_context,
};
use typol_i18n::Translator;
use typol_semantic::{FieldList, TypeView};

pub(super) static ENCODER_INVARIANT: DiagnosticContext =
    diagnostic_context!("typol", "encoding", "encoder-invariant");

pub(super) struct ValueInput<'schema, 'value> {
    pub(super) ty: TypeView<'schema>,
    pub(super) value: ValueView<'value>,
    pub(super) indent: usize,
}

pub(super) struct FieldsInput<'schema, 'value> {
    pub(super) fields: FieldList<'schema>,
    pub(super) values: ValueEntries<'value>,
    pub(super) indent: usize,
}

pub(super) enum Frame<'schema, 'value> {
    Value(ValueInput<'schema, 'value>),
    Text(&'static str),
    Fields {
        fields: FieldList<'schema>,
        values: ChargedVec<ValueEntryView<'value>>,
        slots: ChargedVec<usize>,
        next: usize,
        indent: usize,
    },
    Array {
        ty: TypeView<'schema>,
        values: ChargedVec<ValueEntryView<'value>>,
        next: usize,
        indent: usize,
    },
    Map {
        ty: TypeView<'schema>,
        values: ChargedVec<ValueEntryView<'value>>,
        slots: ChargedVec<usize>,
        next: usize,
        indent: usize,
    },
    EnumPayloadEnd {
        indent: usize,
    },
}

pub(super) struct EncoderModel;

impl EncoderModel {
    pub(super) fn write_failure(error: WriteError) -> Failure {
        match error {
            WriteError::AllocationFailed => Failure::Allocation,
            WriteError::CapacityExceeded(capacity) => Failure::Capacity(capacity),
            WriteError::HostFailure => Failure::Encoding,
            WriteError::Internal(internal) => Failure::Internal(internal),
        }
    }

    pub(super) fn push<'slot, 'schema, 'value>(
        session: &SessionBudget,
        frames: &mut ChargedVec<Frame<'schema, 'value>>,
        frame: Frame<'schema, 'value>,
        out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ()> {
        session.budget_codec(
            CodecStage::Encoding,
            |budget_out| frames.try_push(frame, budget_out),
            out,
        )
    }

    pub(super) fn encoding_error(failure: Failure, translator: Translator) -> CodecErrors {
        CodecErrors::emergency(CodecStage::Encoding, failure, translator)
    }
}
