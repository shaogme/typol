mod model;

use crate::{
    conversion::{ConversionContext, ConversionValue},
    diagnostics::{CodecErrors, CodecStage, Failure},
    options::BindingLimits,
    session::{NodeBudget, SessionBudget},
};
use core::{
    any::TypeId,
    fmt,
    sync::atomic::{AtomicUsize, Ordering},
};
pub use model::{
    DescriptorName, DescriptorNode, DescriptorProvider, MapContainer, TypeDescriptor, TypeShape,
    VariantShape,
};
use opeo::{OResult, Out, ResultOutExt};
use typol_core::{ChargedVec, DiagnosticContext, ValueId, diagnostic_context};
use typol_model::BuiltinType;

static NAME_BUDGET_MISMATCH: DiagnosticContext =
    diagnostic_context!("typol", "binding", "descriptor-name-budget-mismatch");
static MEMBER_BUDGET_MISMATCH: DiagnosticContext =
    diagnostic_context!("typol", "binding", "descriptor-member-budget-mismatch");
static MEMBER_KIND_MISMATCH: DiagnosticContext =
    diagnostic_context!("typol", "binding", "descriptor-member-kind-mismatch");
static DESCRIPTOR_OWNER_MISMATCH: DiagnosticContext =
    diagnostic_context!("typol", "binding", "descriptor-owner-mismatch");
static DESCRIPTOR_INDEX_MISSING: DiagnosticContext =
    diagnostic_context!("typol", "binding", "descriptor-index-out-of-bounds");
static NEXT_OWNER: AtomicUsize = AtomicUsize::new(1);

/// Library-managed context for building a bound type description.
/// Names, descriptor nodes, workspace, and nested depth are budgeted; these limits cover only
/// library operations routed through this context, not handwritten Rust code in `Typol::describe`.
pub struct DescribeContext<'session> {
    owner: usize,
    limits: BindingLimits,
    session: &'session SessionBudget,
    nodes: NodeBudget,
    arena: ChargedVec<DescriptorNode>,
    depth: usize,
}

/// Conversion contract between Rust values and configuration trees. Each implementation must call `start` once, and decoded children must be dispatched through the context.
/// Derived and built-in implementations route nested conversions through library-budgeted paths; the library cannot limit ordinary allocations, blocking, or arbitrary computation performed directly by handwritten implementations.
/// `from_config_value` receives input validated against the bound Schema, but the library does not validate the returned `Self` again.
/// Handwritten structs can reuse the built-in string conversion through `context.read_field::<String>(...)`; it calls `target_text` to charge target text storage.
/// When a handwritten implementation creates owned text itself, use `target_text` instead of copying the string outside the context.
/// Handwritten implementations must follow this contract themselves; the library cannot prove or enforce compliance. `target_text` and `target_bytes` are recommended accounting APIs, not interceptors for all target storage.
pub trait Typol: Sized + 'static {
    fn describe<'slot, 'borrow>(
        context: &mut DescribeContext<'_>,
        out: Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, TypeDescriptor>;

    fn to_config_value<'slot, 'borrow>(
        &self,
        context: &mut ConversionContext<'_>,
        out: Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, ValueId>;

    fn from_config_value<'slot, 'borrow>(
        value: ConversionValue<'_>,
        context: &mut ConversionContext<'_>,
        out: Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, Self>;
}

/// This hidden capability marker selects the library-managed binding API path. It is not a
/// sandbox or a security boundary; handwritten Rust code can still be implemented and trusted
/// explicitly through `bind_trusted`.
#[doc(hidden)]
pub trait BudgetedTypol: Typol {}

impl<'session> DescribeContext<'session> {
    pub(crate) fn new<'slot>(
        limits: BindingLimits,
        session: &'session SessionBudget,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, Self> {
        // The identity counter is used only to check handle ownership; it does not cache types or Schemas globally.
        let mut next_owner = NEXT_OWNER.load(Ordering::Relaxed);
        let owner = loop {
            let following = next_owner
                .checked_add(1)
                .ok_or_else(|| CodecErrors::emergency(CodecStage::Binding, Failure::Resource))
                .store_err_into(out.reborrow())?;
            match NEXT_OWNER.compare_exchange_weak(
                next_owner,
                following,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break next_owner,
                Err(actual) => next_owner = actual,
            }
        };
        let arena = session.budget_codec(
            CodecStage::Binding,
            |budget_out| session.workspace(budget_out),
            out.reborrow(),
        )?;
        Ok(Self {
            owner,
            limits,
            session,
            nodes: NodeBudget::new(limits.max_nodes(), limits.max_depth()),
            arena,
            depth: 0,
        })
    }

    pub(crate) fn session(&self) -> &SessionBudget {
        self.session
    }

    pub const fn depth(&self) -> usize {
        self.depth
    }

    pub(crate) fn describe<'slot, T: Typol>(
        &mut self,
        depth: usize,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, TypeDescriptor> {
        self.check_depth(depth, out.reborrow())?;
        let previous = self.depth;
        self.depth = depth;
        let result = T::describe(self, out.reborrow());
        self.depth = previous;
        result
    }

    pub(crate) fn check_depth<'slot>(
        &self,
        depth: usize,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ()> {
        self.session.budget_codec(
            CodecStage::Binding,
            |budget_out| {
                self.nodes
                    .check_depth(depth, self.session.execution(), budget_out)
            },
            out.reborrow(),
        )
    }

    pub fn name<'slot>(
        &self,
        text: &str,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, DescriptorName> {
        if text.len() > self.limits.max_name_bytes() {
            return Err(out.fail(CodecErrors::emergency(
                CodecStage::Binding,
                self.session.execution().resource_error().into(),
            )));
        }
        let copied = self.session.budget_codec(
            CodecStage::Binding,
            |budget_out| self.session.copy_text(text, budget_out),
            out.reborrow(),
        )?;
        Ok(DescriptorName::new(copied))
    }

    pub fn scalar<'slot, T: 'static>(
        &mut self,
        kind: BuiltinType,
        depth: usize,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, TypeDescriptor> {
        self.insert(
            DescriptorNode::Type {
                identity: TypeId::of::<T>(),
                name: None,
                shape: TypeShape::Scalar(kind),
            },
            depth,
            out.reborrow(),
        )
    }

    pub fn members<'slot>(
        &self,
        descriptors: &[TypeDescriptor],
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ChargedVec<TypeDescriptor>> {
        let mut members = self.session.budget_codec(
            CodecStage::Binding,
            |budget_out| self.session.workspace(budget_out),
            out.reborrow(),
        )?;
        self.session.budget_codec(
            CodecStage::Binding,
            |budget_out| members.try_reserve_exact(descriptors.len(), budget_out),
            out.reborrow(),
        )?;
        for descriptor in descriptors {
            self.get(*descriptor, out.reborrow())?;
            self.session.budget_codec(
                CodecStage::Binding,
                |budget_out| members.try_push(*descriptor, budget_out),
                out.reborrow(),
            )?;
        }
        Ok(members)
    }

    pub fn array_length<'slot>(
        length: usize,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, u32> {
        match u32::try_from(length) {
            Ok(length) => Ok(length),
            Err(_) => Err(out.reborrow().fail(CodecErrors::emergency(
                CodecStage::Binding,
                Failure::ArrayLength,
            ))),
        }
    }

    pub fn insert<'slot>(
        &mut self,
        node: DescriptorNode,
        depth: usize,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, TypeDescriptor> {
        self.check_node(&node, out.reborrow())?;
        self.session.budget_codec(
            CodecStage::Binding,
            |budget_out| self.nodes.node(depth, self.session.execution(), budget_out),
            out.reborrow(),
        )?;
        let descriptor = TypeDescriptor::new(self.owner, self.arena.len());
        self.session.budget_codec(
            CodecStage::Binding,
            |budget_out| self.arena.try_push(node, budget_out),
            out.reborrow(),
        )?;
        Ok(descriptor)
    }

    fn check_node<'slot>(
        &self,
        node: &DescriptorNode,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ()> {
        let name = match node {
            DescriptorNode::Type { name, .. } => name.as_ref(),
            DescriptorNode::Field { name, .. } | DescriptorNode::Variant { name, .. } => Some(name),
        };
        if name.is_some_and(|name| {
            !name
                .budget()
                .is_some_and(|budget| budget.same_call(self.session.execution()))
        }) {
            return Err(out.fail(CodecErrors::emergency(
                CodecStage::Binding,
                Failure::internal(NAME_BUDGET_MISMATCH),
            )));
        }
        let members = match node {
            DescriptorNode::Type {
                shape: TypeShape::Structure(members),
                ..
            }
            | DescriptorNode::Variant {
                shape: VariantShape::Structure(members),
                ..
            } => Some((members, false)),
            DescriptorNode::Type {
                shape: TypeShape::Enumeration(members),
                ..
            } => Some((members, true)),
            _ => None,
        };
        if let Some((members, variants)) = members {
            if !members
                .budget()
                .is_some_and(|budget| budget.same_call(self.session.execution()))
            {
                return Err(out.fail(CodecErrors::emergency(
                    CodecStage::Binding,
                    Failure::internal(MEMBER_BUDGET_MISMATCH),
                )));
            }
            for descriptor in members {
                match self.get(*descriptor, out.reborrow())? {
                    DescriptorNode::Variant { .. } if variants => {}
                    DescriptorNode::Field { .. } if !variants => {}
                    _ => {
                        return Err(out.fail(CodecErrors::emergency(
                            CodecStage::Binding,
                            Failure::internal(
                                MEMBER_KIND_MISMATCH.with_object_id(descriptor.slot() as u64),
                            ),
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    pub fn get<'slot>(
        &self,
        descriptor: TypeDescriptor,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, &DescriptorNode> {
        self.session.budget_codec(
            CodecStage::Binding,
            |budget_out| self.session.visit(budget_out),
            out.reborrow(),
        )?;
        if descriptor.owner() != self.owner {
            return Err(out.fail(CodecErrors::emergency(
                CodecStage::Binding,
                Failure::internal(
                    DESCRIPTOR_OWNER_MISMATCH.with_object_id(descriptor.slot() as u64),
                ),
            )));
        }
        match self.arena.get(descriptor.slot()) {
            Some(node) => Ok(node),
            None => Err(out.reborrow().fail(CodecErrors::emergency(
                CodecStage::Binding,
                Failure::internal(
                    DESCRIPTOR_INDEX_MISSING.with_object_id(descriptor.slot() as u64),
                ),
            ))),
        }
    }
}

impl fmt::Debug for DescribeContext<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DescribeContext")
            .field("nodes", &self.nodes.nodes())
            .finish()
    }
}
