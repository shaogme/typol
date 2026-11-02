use crate::{
    defaults::{
        compiler::BuiltValue,
        frames::{MapBuild, NamedDefault, RecordBuild},
    },
    symbols::{FieldSymbolId, VariantSymbolId},
    types::{ResolvedTypeKind, TypeUseId},
};

use typol_core::{ChargedVec, ConfigValue, SourceSpan};
use typol_syntax::{ArrayDefault, DefaultValue, EnumDefault};

pub(super) struct ValueTask<'a> {
    pub(super) source: DefaultValue<'a>,
    pub(super) target: TypeUseId,
    pub(super) depth: usize,
}

pub(super) struct EnumTask {
    pub(super) id: VariantSymbolId,
    pub(super) target: TypeUseId,
    pub(super) span: SourceSpan,
}

pub(super) struct ArrayBuild<'a> {
    pub(super) source: ArrayDefault<'a>,
    pub(super) target: TypeUseId,
    pub(super) depth: usize,
    pub(super) index: usize,
    pub(super) valid: bool,
    pub(super) values: ChargedVec<ConfigValue>,
    pub(super) spans: ChargedVec<SourceSpan>,
}

pub(super) enum Task<'a> {
    Field(FieldSymbolId),
    FinishField(FieldSymbolId, TypeUseId),
    Value(ValueTask<'a>),
    Validate(TypeUseId),
    Some(SourceSpan),
    Array(ArrayBuild<'a>),
    Map(MapBuild<'a>),
    Record(RecordBuild<'a>),
    Structure(SourceSpan),
    EnumSingle(EnumTask),
    Enumeration(VariantSymbolId, SourceSpan),
}

pub(super) struct PrepareEnumInput<'task, 'source> {
    pub(super) source: EnumDefault<'source>,
    pub(super) kind: ResolvedTypeKind,
    pub(super) depth: usize,
    pub(super) tasks: &'task mut ChargedVec<Task<'source>>,
}

impl<'a> Task<'a> {
    pub(super) fn value(source: DefaultValue<'a>, target: TypeUseId, depth: usize) -> Self {
        Self::Value(ValueTask {
            source,
            target,
            depth,
        })
    }

    pub(super) fn enum_single(id: VariantSymbolId, target: TypeUseId, span: SourceSpan) -> Self {
        Self::EnumSingle(EnumTask { id, target, span })
    }
}

pub(super) struct TaskState<'a> {
    result: Option<BuiltValue>,
    record_result: Option<ChargedVec<NamedDefault<'a>>>,
}

impl<'a> TaskState<'a> {
    pub(super) fn new() -> Self {
        Self {
            result: None,
            record_result: None,
        }
    }

    pub(super) fn result(&self) -> Option<&BuiltValue> {
        self.result.as_ref()
    }

    pub(super) fn take_result(&mut self) -> Option<BuiltValue> {
        self.result.take()
    }

    pub(super) fn set_result(&mut self, result: Option<BuiltValue>) {
        self.result = result;
    }

    pub(super) fn take_record_result(&mut self) -> Option<ChargedVec<NamedDefault<'a>>> {
        self.record_result.take()
    }

    pub(super) fn set_record_result(&mut self, result: Option<ChargedVec<NamedDefault<'a>>>) {
        self.record_result = result;
    }
}
