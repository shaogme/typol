#[cfg(test)]
use crate::diagnostics::NAMED_ENTRY_NAME_MISSING;

use crate::{
    diagnostics::{BudgetDiagnostic, DiagnosticFactory},
    secret::SecretPolicy,
};
use core::cell::RefCell;
use opeo::{ErrSlot, OResult, Out, ResultOutExt};
use typol_core::{
    BudgetError, ChargedVec, ConfigValue, Diagnostic, DiagnosticCode, DiagnosticContext,
    ExecutionBudget, InstancePath, LocatedSpan, ValueKind, ValuePayloadView, ValueSource,
    ValueView, diagnostic_context,
};
use typol_semantic::{CompiledSchema, FieldView, SchemaAccessError, TypeView, VariantView};

static INSTANCE_LOCATION_INVARIANT: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "instance-location-index",
    "parent-child-or-value-shape-mismatch",
);
static INSTANCE_LOCATION_NODE_MISSING: DiagnosticContext = diagnostic_context!(
    "typol-config",
    "instance-location-walk",
    "node-reference-unavailable",
);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct NodeId(usize);

#[derive(Clone, Copy)]
pub(crate) enum Member<'schema> {
    Root,
    Field(FieldView<'schema>),
    Index(usize),
    MapItem(usize),
    Optional,
    Payload(VariantView<'schema>),
    VariantField {
        variant: VariantView<'schema>,
        field: FieldView<'schema>,
    },
}

pub(crate) struct LocationNode<'schema> {
    parent: Option<NodeId>,
    member: Member<'schema>,
    span: Option<LocatedSpan>,
    expected: TypeView<'schema>,
    variant: Option<VariantView<'schema>>,
    path_depth: usize,
    complete: bool,
    children: ChargedVec<NodeId>,
    slots: ChargedVec<Option<NodeId>>,
}

/// Stores only instance identity, parent links, and locations; key text is borrowed from the actual value tree when a path is needed.
pub(crate) struct LocationIndex<'schema> {
    nodes: ChargedVec<LocationNode<'schema>>,
    access_errors: RefCell<ErrSlot<SchemaAccessError>>,
    budget_errors: RefCell<ErrSlot<BudgetError>>,
}

struct LocationErrorSlots<'a> {
    access: &'a RefCell<ErrSlot<SchemaAccessError>>,
    budget: &'a RefCell<ErrSlot<BudgetError>>,
}

fn schema_access<'slot, 'borrow, T>(
    errors: &RefCell<ErrSlot<SchemaAccessError>>,
    operation: impl for<'call> FnOnce(Out<'call, 'call, SchemaAccessError>) -> OResult<'call, T>,
    out: Out<'slot, 'borrow, Diagnostic>,
) -> OResult<'slot, T> {
    match errors.borrow_mut().try_call(operation) {
        Ok(value) => Ok(value),
        Err(caught) => Err(out.fail(DiagnosticFactory::schema_access(caught.take()))),
    }
}

fn budget_error<'slot, 'borrow, T>(
    errors: &RefCell<ErrSlot<BudgetError>>,
    operation: impl for<'call> FnOnce(Out<'call, 'call, BudgetError>) -> OResult<'call, T>,
    out: Out<'slot, 'borrow, Diagnostic>,
) -> OResult<'slot, T> {
    match errors.borrow_mut().try_call(operation) {
        Ok(value) => Ok(value),
        Err(caught) => Err(out.fail(BudgetDiagnostic::from_error(caught.take()))),
    }
}

impl<'schema> LocationIndex<'schema> {
    pub(crate) fn new_budgeted<'slot>(
        budget: &ExecutionBudget,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Self> {
        Ok(Self {
            nodes: ChargedVec::new(Some(budget), out.reborrow())?,
            access_errors: RefCell::new(ErrSlot::new()),
            budget_errors: RefCell::new(ErrSlot::new()),
        })
    }

    pub(crate) fn budget(&self) -> Option<&ExecutionBudget> {
        self.nodes.budget()
    }

    pub(crate) fn begin<'slot>(
        &mut self,
        mut node: LocationNode<'schema>,
        limit: usize,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, NodeId> {
        if let Some(budget) = self.budget() {
            budget.consume(1, out.reborrow())?;
            node.children = ChargedVec::new(Some(budget), out.reborrow())?;
            node.slots = ChargedVec::new(Some(budget), out.reborrow())?;
        }
        if self.nodes.len() >= limit {
            return Err(out.fail(DiagnosticFactory::make_diagnostic(
                DiagnosticCode::RESOURCE_LIMIT_EXCEEDED,
                "located node count exceeded the limit",
                node.span,
                None,
                None,
            )));
        }
        self.nodes.try_reserve(1, out.reborrow())?;
        let id = NodeId(self.nodes.len());
        if let Some(parent) = node.parent {
            let Some(parent) = self.nodes.get_mut(parent.0) else {
                return Err(out.fail(internal(node.span)));
            };
            let slot = schema_access(
                &self.access_errors,
                |access_out| node.member.slot(access_out),
                out.reborrow(),
            )?;
            let required = slot
                .checked_add(1)
                .ok_or_else(|| internal(node.span))
                .store_err_into(out.reborrow())?;
            if parent.slots.len() < required {
                parent
                    .slots
                    .try_reserve(required - parent.slots.len(), out.reborrow())?;
                parent.slots.resize(required, None, out.reborrow())?;
            }
            let Some(parent_slot) = parent.slots.get_mut(slot) else {
                return Err(out.fail(internal(node.span)));
            };
            if parent_slot.is_some() {
                return Err(out.fail(internal(node.span)));
            }
            *parent_slot = Some(id);
            parent.children.try_push(id, out.reborrow())?;
        }
        self.nodes.try_push(node, out.reborrow())?;
        Ok(id)
    }

    #[cfg(test)]
    pub(crate) fn nodes(&self) -> impl Iterator<Item = (NodeId, &LocationNode<'schema>)> {
        self.nodes
            .iter()
            .enumerate()
            .map(|(index, node)| (NodeId(index), node))
    }

    pub(crate) fn set_variant<'slot, 'borrow>(
        &mut self,
        id: NodeId,
        variant: VariantView<'schema>,
        span: impl Into<Option<LocatedSpan>>,
        out: Out<'slot, 'borrow, Diagnostic>,
    ) -> OResult<'slot, ()> {
        let span = span.into();
        let Some(node) = self.nodes.get_mut(id.0) else {
            return Err(out.fail(internal(span)));
        };
        node.variant = Some(variant);
        Ok(())
    }

    pub(crate) fn get(&self, id: NodeId) -> Option<&LocationNode<'schema>> {
        self.nodes.get(id.0)
    }

    pub(crate) fn item(&self, parent: NodeId, index: usize) -> Option<&LocationNode<'schema>> {
        let id = self.get(parent)?.slots.get(index)?.as_ref()?;
        let node = self.get(*id)?;
        node.complete.then_some(node)
    }

    pub(crate) fn complete<'slot, 'borrow>(
        &mut self,
        id: NodeId,
        span: impl Into<Option<LocatedSpan>>,
        out: Out<'slot, 'borrow, Diagnostic>,
    ) -> OResult<'slot, ()> {
        let span = span.into();
        let Some(node) = self.nodes.get_mut(id.0) else {
            return Err(out.fail(internal(span)));
        };
        node.span = span;
        node.complete = true;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn path<'slot>(
        &self,
        id: NodeId,
        root: &ConfigValue,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, InstancePath> {
        let Some(node) = self.get(id) else {
            return Err(out.fail(DiagnosticFactory::internal(
                INSTANCE_LOCATION_NODE_MISSING.with_object_id(id.0 as u64),
                None,
                None,
                None,
            )));
        };
        let span = node.span;
        let mut chain = ChargedVec::unmetered();
        let mut current = Some(id);
        while let Some(id) = current {
            let Some(node) = self.get(id) else {
                return Err(out.fail(internal(span)));
            };
            if !node.complete {
                return Err(out.fail(internal(span)));
            }
            chain.try_push(node.member, out.reborrow())?;
            current = node.parent;
        }

        let mut value = root.root();
        let mut path = InstancePath::unmetered();
        for member in chain.iter().rev() {
            match *member {
                Member::Root => {}
                Member::Field(field) => {
                    let ValueKind::Structure(fields) = value.as_ref() else {
                        return Err(out.fail(internal(span)));
                    };
                    let slot = schema_access(
                        &self.access_errors,
                        |access_out| field.slot(access_out),
                        out.reborrow(),
                    )?;
                    let Some(field) = fields.get(slot) else {
                        return Err(out.fail(internal(span)));
                    };
                    let Some(field) = field.as_named() else {
                        return Err(out.fail(DiagnosticFactory::internal(
                            NAMED_ENTRY_NAME_MISSING,
                            span,
                            None,
                            None,
                        )));
                    };
                    budget_error(
                        &self.budget_errors,
                        |budget_out| path.push_field(field.name(), budget_out),
                        out.reborrow(),
                    )?;
                    value = field.value();
                }
                Member::Index(index) => {
                    let ValueKind::Array(values) = value.as_ref() else {
                        return Err(out.fail(internal(span)));
                    };
                    let Some(item) = values.get(index) else {
                        return Err(out.fail(internal(span)));
                    };
                    value = item.value();
                    budget_error(
                        &self.budget_errors,
                        |budget_out| path.push_index(index, budget_out),
                        out.reborrow(),
                    )?;
                }
                Member::MapItem(index) => {
                    let ValueKind::Map(entries) = value.as_ref() else {
                        return Err(out.fail(internal(span)));
                    };
                    let Some(entry) = entries.get(index) else {
                        return Err(out.fail(internal(span)));
                    };
                    let Some(entry) = entry.as_named() else {
                        return Err(out.fail(DiagnosticFactory::internal(
                            NAMED_ENTRY_NAME_MISSING,
                            span,
                            None,
                            None,
                        )));
                    };
                    budget_error(
                        &self.budget_errors,
                        |budget_out| path.push_map_key(entry.name(), budget_out),
                        out.reborrow(),
                    )?;
                    value = entry.value();
                }
                Member::Optional => {
                    let ValueKind::Optional(Some(inner)) = value.as_ref() else {
                        return Err(out.fail(internal(span)));
                    };
                    value = inner;
                }
                Member::Payload(variant) => {
                    let ValueKind::Enumeration {
                        variant: name,
                        payload: ValuePayloadView::Single(inner),
                    } = value.as_ref()
                    else {
                        return Err(out.fail(internal(span)));
                    };
                    let variant_name = schema_access(
                        &self.access_errors,
                        |access_out| variant.name(access_out),
                        out.reborrow(),
                    )?;
                    if name != variant_name {
                        return Err(out.fail(internal(span)));
                    }
                    value = inner;
                    budget_error(
                        &self.budget_errors,
                        |budget_out| path.push_payload(variant_name, budget_out),
                        out.reborrow(),
                    )?;
                }
                Member::VariantField { variant, field } => {
                    let ValueKind::Enumeration {
                        variant: name,
                        payload: ValuePayloadView::Structure(fields),
                    } = value.as_ref()
                    else {
                        return Err(out.fail(internal(span)));
                    };
                    let variant_name = schema_access(
                        &self.access_errors,
                        |access_out| variant.name(access_out),
                        out.reborrow(),
                    )?;
                    if name != variant_name {
                        return Err(out.fail(internal(span)));
                    }
                    budget_error(
                        &self.budget_errors,
                        |budget_out| path.push_payload(variant_name, budget_out),
                        out.reborrow(),
                    )?;
                    let slot = schema_access(
                        &self.access_errors,
                        |access_out| field.slot(access_out),
                        out.reborrow(),
                    )?;
                    let Some(field) = fields.get(slot) else {
                        return Err(out.fail(internal(span)));
                    };
                    let Some(field) = field.as_named() else {
                        return Err(out.fail(DiagnosticFactory::internal(
                            NAMED_ENTRY_NAME_MISSING,
                            span,
                            None,
                            None,
                        )));
                    };
                    budget_error(
                        &self.budget_errors,
                        |budget_out| path.push_field(field.name(), budget_out),
                        out.reborrow(),
                    )?;
                    value = field.value();
                }
            }
        }
        Ok(path)
    }
}

impl<'schema> LocationNode<'schema> {
    pub(crate) fn span(&self) -> Option<LocatedSpan> {
        self.span
    }

    pub(crate) fn expected(&self) -> TypeView<'schema> {
        self.expected
    }

    pub(crate) fn variant(&self) -> Option<VariantView<'schema>> {
        self.variant
    }

    pub(crate) fn path_depth(&self) -> usize {
        self.path_depth
    }

    #[cfg(test)]
    pub(crate) fn complete(&self) -> bool {
        self.complete
    }

    fn descend<'a, 'slot>(
        &self,
        value: ValueView<'a>,
        budget: Option<&ExecutionBudget>,
        slots: &LocationErrorSlots<'_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ValueView<'a>> {
        let invalid = || -> Diagnostic { internal(self.span) };
        match (self.member, value.as_ref()) {
            (Member::Field(id), ValueKind::Structure(fields)) => {
                let field = id;
                let slot = schema_access(
                    slots.access,
                    |access_out| field.slot(access_out),
                    out.reborrow(),
                )?;
                let Some(field) = fields.get(slot) else {
                    return Err(out.fail(invalid()));
                };
                Ok(field.value())
            }
            (Member::Index(index), ValueKind::Array(values)) => match values.get(index) {
                Some(value) => Ok(value.value()),
                None => Err(out.fail(invalid())),
            },
            (Member::MapItem(index), ValueKind::Map(entries)) => match entries.get(index) {
                Some(entry) => Ok(entry.value()),
                None => Err(out.fail(invalid())),
            },
            (Member::Optional, ValueKind::Optional(Some(value))) => Ok(value),
            (
                Member::Payload(id),
                ValueKind::Enumeration {
                    variant,
                    payload: ValuePayloadView::Single(value),
                },
            ) => {
                if if let Some(budget) = budget {
                    let name = schema_access(
                        slots.access,
                        |access_out| id.name(access_out),
                        out.reborrow(),
                    )?;
                    !(budget.compare_text(name, variant, out.reborrow()))?.is_eq()
                } else {
                    schema_access(
                        slots.access,
                        |access_out| id.name(access_out),
                        out.reborrow(),
                    )? != variant
                } {
                    return Err(out.fail(invalid()));
                }
                Ok(value)
            }
            (
                Member::VariantField { variant: id, field },
                ValueKind::Enumeration {
                    variant,
                    payload: ValuePayloadView::Structure(fields),
                },
            ) => {
                if if let Some(budget) = budget {
                    let name = schema_access(
                        slots.access,
                        |access_out| id.name(access_out),
                        out.reborrow(),
                    )?;
                    !(budget.compare_text(name, variant, out.reborrow()))?.is_eq()
                } else {
                    schema_access(
                        slots.access,
                        |access_out| id.name(access_out),
                        out.reborrow(),
                    )? != variant
                } {
                    return Err(out.fail(invalid()));
                }
                let slot = schema_access(
                    slots.access,
                    |access_out| field.slot(access_out),
                    out.reborrow(),
                )?;
                let Some(field) = fields.get(slot) else {
                    return Err(out.fail(invalid()));
                };
                Ok(field.value())
            }
            _ => Err(out.fail(invalid())),
        }
    }

    fn append_path<'slot>(
        &self,
        parent: ValueView<'_>,
        path: &mut InstancePath,
        slots: &LocationErrorSlots<'_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ()> {
        match self.member {
            Member::Root | Member::Optional => Ok(()),
            Member::Field(field) => {
                let name = schema_access(
                    slots.access,
                    |access_out| field.name(access_out),
                    out.reborrow(),
                )?;
                budget_error(
                    slots.budget,
                    |budget_out| path.push_field(name, budget_out),
                    out.reborrow(),
                )
            }
            Member::Index(index) => budget_error(
                slots.budget,
                |budget_out| path.push_index(index, budget_out),
                out.reborrow(),
            ),
            Member::MapItem(index) => {
                let ValueKind::Map(entries) = parent.as_ref() else {
                    return Err(out.fail(internal(self.span)));
                };
                let Some(entry) = entries.get(index) else {
                    return Err(out.fail(internal(self.span)));
                };
                let Some(key) = entry.name() else {
                    return Err(out.fail(internal(self.span)));
                };
                budget_error(
                    slots.budget,
                    |budget_out| path.push_map_key(key, budget_out),
                    out.reborrow(),
                )
            }
            Member::Payload(variant) => {
                let name = schema_access(
                    slots.access,
                    |access_out| variant.name(access_out),
                    out.reborrow(),
                )?;
                budget_error(
                    slots.budget,
                    |budget_out| path.push_payload(name, budget_out),
                    out.reborrow(),
                )
            }
            Member::VariantField { variant, field } => {
                let variant_name = schema_access(
                    slots.access,
                    |access_out| variant.name(access_out),
                    out.reborrow(),
                )?;
                budget_error(
                    slots.budget,
                    |budget_out| path.push_payload(variant_name, budget_out),
                    out.reborrow(),
                )?;
                let field_name = schema_access(
                    slots.access,
                    |access_out| field.name(access_out),
                    out.reborrow(),
                )?;
                budget_error(
                    slots.budget,
                    |budget_out| path.push_field(field_name, budget_out),
                    out.reborrow(),
                )
            }
        }
    }

    pub(crate) fn new(
        parent: Option<NodeId>,
        member: Member<'schema>,
        span: impl Into<Option<LocatedSpan>>,
        expected: TypeView<'schema>,
        path_depth: usize,
    ) -> Self {
        Self {
            parent,
            member,
            span: span.into(),
            expected,
            variant: None,
            path_depth,
            complete: false,
            children: ChargedVec::unmetered(),
            slots: ChargedVec::unmetered(),
        }
    }
}

fn internal(span: Option<LocatedSpan>) -> Diagnostic {
    DiagnosticFactory::internal(INSTANCE_LOCATION_INVARIANT, span, None, None)
}

impl Member<'_> {
    fn slot<'slot, 'borrow>(
        self,
        out: Out<'slot, 'borrow, SchemaAccessError>,
    ) -> OResult<'slot, usize> {
        match self {
            Self::Field(field) | Self::VariantField { field, .. } => field.slot(out),
            Self::Index(index) | Self::MapItem(index) => Ok(index),
            Self::Root | Self::Optional | Self::Payload(_) => Ok(0),
        }
    }
}

struct WalkFrame<'value> {
    id: NodeId,
    value: ValueView<'value>,
    depth: usize,
    next: usize,
    entered: bool,
    path_depth: usize,
}

/// Navigates each parent-child edge once; work frames and fallback paths grow only with depth.
pub(crate) struct InstanceWalk<'index, 'schema, 'value> {
    locations: &'index LocationIndex<'schema>,
    schema: &'schema CompiledSchema,
    frames: ChargedVec<WalkFrame<'value>>,
    path: InstancePath,
    limit: usize,
}

impl<'index, 'schema, 'value> InstanceWalk<'index, 'schema, 'value> {
    pub(crate) fn new<'slot>(
        locations: &'index LocationIndex<'schema>,
        schema: &'schema CompiledSchema,
        root: &'value ConfigValue,
        limit: usize,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Self> {
        let mut frames = ChargedVec::new(locations.budget(), out.reborrow())?;
        frames.try_push(
            WalkFrame {
                id: NodeId(0),
                value: root.root(),
                depth: 0,
                next: 0,
                entered: false,
                path_depth: 0,
            },
            out.reborrow(),
        )?;
        let path = match locations.budget() {
            Some(budget) => budget_error(
                &locations.budget_errors,
                |budget_out| InstancePath::new(Some(budget), budget_out),
                out.reborrow(),
            )?,
            None => InstancePath::unmetered(),
        };
        Ok(Self {
            locations,
            schema,
            frames,
            path,
            limit,
        })
    }

    pub(crate) fn hidden<'slot>(
        &self,
        primary: Option<LocatedSpan>,
        value_source: Option<ValueSource>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, bool> {
        SecretPolicy::hides(
            self.schema,
            &self.path,
            primary,
            value_source,
            out.reborrow(),
        )
    }

    pub(crate) fn copy_path<'slot>(
        &self,
        primary: Option<LocatedSpan>,
        value_source: Option<ValueSource>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, InstancePath> {
        SecretPolicy::copy_path(
            self.schema,
            &self.path,
            primary,
            value_source,
            out.reborrow(),
        )
    }

    pub(crate) fn next<'slot>(
        &mut self,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Option<(&'index LocationNode<'schema>, ValueView<'value>)>> {
        while let Some(frame) = self.frames.last_mut() {
            if let Some(budget) = self.locations.budget() {
                budget.consume(1, out.reborrow())?;
            }
            let Some(node) = self.locations.get(frame.id) else {
                return Err(out.fail(internal_fallback()));
            };
            if !node.complete {
                return Err(out.fail(internal(node.span)));
            }
            if !frame.entered {
                frame.entered = true;
                let structure_payload = matches!(
                    frame.value.as_ref(),
                    ValueKind::Enumeration {
                        payload: ValuePayloadView::Structure(_),
                        ..
                    }
                );
                if structure_payload
                    && !matches!(frame.depth.checked_add(1), Some(depth) if depth <= self.limit)
                {
                    return Err(out.fail(DiagnosticFactory::make_diagnostic(
                        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED,
                        "assertion instance traversal depth exceeded the limit",
                        node.span,
                        None,
                        None,
                    )));
                }
                return Ok(Some((node, frame.value)));
            }
            if let Some(id) = node.children.get(frame.next).copied() {
                frame.next += 1;
                let Some(child) = self.locations.get(id) else {
                    return Err(out.fail(internal(node.span)));
                };
                let slots = LocationErrorSlots {
                    access: &self.locations.access_errors,
                    budget: &self.locations.budget_errors,
                };
                let value =
                    child.descend(frame.value, self.locations.budget(), &slots, out.reborrow())?;
                let path_depth = self.path.len();
                child.append_path(frame.value, &mut self.path, &slots, out.reborrow())?;
                let extra = usize::from(matches!(
                    frame.value.as_ref(),
                    ValueKind::Enumeration {
                        payload: ValuePayloadView::Structure(_),
                        ..
                    }
                ));
                let Some(depth) = frame.depth.checked_add(1 + extra) else {
                    return Err(out.fail(DiagnosticFactory::make_diagnostic(
                        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED,
                        "assertion instance traversal depth exceeded the limit",
                        child.span,
                        None,
                        None,
                    )));
                };
                if depth > self.limit {
                    return Err(out.fail(DiagnosticFactory::make_diagnostic(
                        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED,
                        "assertion instance traversal depth exceeded the limit",
                        child.span,
                        None,
                        None,
                    )));
                }
                self.frames.try_push(
                    WalkFrame {
                        id,
                        value,
                        depth,
                        next: 0,
                        entered: false,
                        path_depth,
                    },
                    out.reborrow(),
                )?;
            } else {
                let depth = frame.path_depth;
                self.frames.pop();
                self.path.truncate(depth);
            }
        }
        Ok(None)
    }
}

fn internal_fallback() -> Diagnostic {
    DiagnosticFactory::internal(INSTANCE_LOCATION_NODE_MISSING, None, None, None)
}
