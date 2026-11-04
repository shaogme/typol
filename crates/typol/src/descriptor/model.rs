use super::{DescribeContext, Typol};
use crate::diagnostics::{CodecErrors, CodecStage, Failure};
use core::{any::TypeId, fmt};
use opeo::{OResult, Out};
use typol_core::{
    ChargedString, ChargedVec, DiagnosticContext, ExecutionBudget, diagnostic_context,
};
use typol_model::BuiltinType;

static PROVIDER_TYPE_MISMATCH: DiagnosticContext =
    diagnostic_context!("typol", "binding", "descriptor-provider-type-mismatch");

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct TypeDescriptor {
    owner: usize,
    slot: usize,
}

#[derive(Clone, Copy)]
pub struct DescriptorProvider {
    identity: TypeId,
    describe: for<'session, 'slot, 'borrow> fn(
        &mut DescribeContext<'session>,
        usize,
        Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, TypeDescriptor>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MapContainer {
    BTreeMap,
    HashMap,
    OrderedMap,
}

pub enum TypeShape {
    Transparent(DescriptorProvider),
    Scalar(BuiltinType),
    Option(DescriptorProvider),
    Sequence(DescriptorProvider),
    Array {
        element: DescriptorProvider,
        length: u32,
    },
    Map {
        container: MapContainer,
        value: DescriptorProvider,
    },
    Structure(ChargedVec<TypeDescriptor>),
    Enumeration(ChargedVec<TypeDescriptor>),
}

pub enum VariantShape {
    Unit,
    Single(DescriptorProvider),
    Structure(ChargedVec<TypeDescriptor>),
}

pub struct DescriptorName {
    text: ChargedString,
}

pub enum DescriptorNode {
    Type {
        identity: TypeId,
        name: Option<DescriptorName>,
        shape: TypeShape,
    },
    Field {
        name: DescriptorName,
        value: DescriptorProvider,
    },
    Variant {
        name: DescriptorName,
        shape: VariantShape,
    },
}

impl fmt::Debug for TypeDescriptor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TypeDescriptor")
            .finish_non_exhaustive()
    }
}

impl TypeDescriptor {
    pub(super) const fn new(owner: usize, slot: usize) -> Self {
        Self { owner, slot }
    }

    pub(super) const fn owner(self) -> usize {
        self.owner
    }

    pub(super) const fn slot(self) -> usize {
        self.slot
    }
}

impl DescriptorProvider {
    pub fn of<T: Typol>() -> Self {
        Self::new::<T>(|context, depth, out| context.describe::<T>(depth, out))
    }

    pub fn new<T: 'static>(
        describe: for<'session, 'slot, 'borrow> fn(
            &mut DescribeContext<'session>,
            usize,
            Out<'slot, 'borrow, CodecErrors>,
        ) -> OResult<'slot, TypeDescriptor>,
    ) -> Self {
        Self {
            identity: TypeId::of::<T>(),
            describe,
        }
    }

    pub const fn identity(self) -> TypeId {
        self.identity
    }

    pub fn resolve<'slot>(
        self,
        context: &mut DescribeContext<'_>,
        depth: usize,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, TypeDescriptor> {
        context.check_depth(depth, out.reborrow())?;
        let descriptor = (self.describe)(context, depth, out.reborrow())?;
        match context.get(descriptor, out.reborrow())? {
            DescriptorNode::Type { identity, .. } if *identity == self.identity => Ok(descriptor),
            _ => Err(out.reborrow().fail(CodecErrors::emergency(
                CodecStage::Binding,
                Failure::internal(PROVIDER_TYPE_MISMATCH.with_object_id(descriptor.slot as u64)),
            ))),
        }
    }
}

impl fmt::Debug for DescriptorProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DescriptorProvider")
            .finish_non_exhaustive()
    }
}

impl DescriptorName {
    pub(super) fn new(text: ChargedString) -> Self {
        Self { text }
    }

    pub fn text(&self) -> &str {
        self.text.as_str()
    }

    pub(super) fn budget(&self) -> Option<&ExecutionBudget> {
        self.text.budget()
    }
}

impl fmt::Debug for DescriptorName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DescriptorName")
            .field("bytes", &self.text.len())
            .finish()
    }
}
