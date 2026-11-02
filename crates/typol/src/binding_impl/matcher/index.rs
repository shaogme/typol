use crate::{
    diagnostics::{CodecErrors, CodecStage, Failure},
    session::SessionBudget,
};
use core::{any::TypeId, cmp::Ordering};
use opeo::{OResult, Out, ResultOutExt};
use typol_core::{ChargedVec, DiagnosticContext, diagnostic_context};
use typol_semantic::TypeView;

static INDEX_INVARIANT: DiagnosticContext =
    diagnostic_context!("typol", "binding", "binding-type-index-invariant");

// AVL balancing keeps index lookups and inserts logarithmic; node storage is charged to the binding budget.
struct IndexNode<'schema> {
    identity: TypeId,
    ty: TypeView<'schema>,
    left: Option<usize>,
    right: Option<usize>,
    parent: Option<usize>,
    height: usize,
}

pub(super) struct BindingIndex<'schema> {
    nodes: ChargedVec<IndexNode<'schema>>,
    root: Option<usize>,
}

impl<'schema> BindingIndex<'schema> {
    pub(super) fn new<'slot>(
        session: &SessionBudget,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, Self> {
        let nodes = session.budget_codec(
            CodecStage::Binding,
            |budget_out| session.workspace(budget_out),
            out.reborrow(),
        )?;
        Ok(Self { nodes, root: None })
    }

    pub(super) fn contains<'slot>(
        &self,
        identity: TypeId,
        ty: TypeView<'schema>,
        session: &SessionBudget,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, bool> {
        let mut current = self.root;
        while let Some(index) = current {
            session.budget_codec(
                CodecStage::Binding,
                |budget_out| session.visit(budget_out),
                out.reborrow(),
            )?;
            let node = self
                .nodes
                .get(index)
                .ok_or_else(index_invariant)
                .store_err_into(out.reborrow())?;
            match identity.cmp(&node.identity).then_with(|| ty.cmp(&node.ty)) {
                Ordering::Less => current = node.left,
                Ordering::Greater => current = node.right,
                Ordering::Equal => return Ok(true),
            }
        }
        Ok(false)
    }

    pub(super) fn insert<'slot>(
        &mut self,
        identity: TypeId,
        ty: TypeView<'schema>,
        session: &SessionBudget,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, bool> {
        let mut parent = None;
        let mut side = Ordering::Equal;
        let mut current = self.root;
        while let Some(index) = current {
            session.budget_codec(
                CodecStage::Binding,
                |budget_out| session.visit(budget_out),
                out.reborrow(),
            )?;
            let node = self
                .nodes
                .get(index)
                .ok_or_else(index_invariant)
                .store_err_into(out.reborrow())?;
            side = identity.cmp(&node.identity).then_with(|| ty.cmp(&node.ty));
            match side {
                Ordering::Less => current = node.left,
                Ordering::Greater => current = node.right,
                Ordering::Equal => return Ok(false),
            }
            parent = Some(index);
        }

        let index = self.nodes.len();
        session.budget_codec(
            CodecStage::Binding,
            |budget_out| {
                self.nodes.try_push(
                    IndexNode {
                        identity,
                        ty,
                        left: None,
                        right: None,
                        parent,
                        height: 1,
                    },
                    budget_out,
                )
            },
            out.reborrow(),
        )?;
        match parent {
            Some(parent) => {
                let parent_node = self
                    .nodes
                    .get_mut(parent)
                    .ok_or_else(index_invariant)
                    .store_err_into(out.reborrow())?;
                match side {
                    Ordering::Less => parent_node.left = Some(index),
                    Ordering::Greater => parent_node.right = Some(index),
                    Ordering::Equal => return Err(out.fail(index_invariant())),
                }
            }
            None => self.root = Some(index),
        }
        self.rebalance_from(parent, session, out.reborrow())?;
        Ok(true)
    }

    fn rebalance_from<'slot>(
        &mut self,
        mut current: Option<usize>,
        session: &SessionBudget,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ()> {
        while let Some(index) = current {
            session.budget_codec(
                CodecStage::Binding,
                |budget_out| session.visit(budget_out),
                out.reborrow(),
            )?;
            self.update_height(index, out.reborrow())?;
            let balance = self.balance(index, out.reborrow())?;
            let subtree_root = if balance > 0 {
                let (left, _) = self.children(index, out.reborrow())?;
                let left = left
                    .ok_or_else(index_invariant)
                    .store_err_into(out.reborrow())?;
                if self.balance(left, out.reborrow())? < 0 {
                    self.rotate_left(left, out.reborrow())?;
                }
                self.rotate_right(index, out.reborrow())?
            } else if balance < 0 {
                let (_, right) = self.children(index, out.reborrow())?;
                let right = right
                    .ok_or_else(index_invariant)
                    .store_err_into(out.reborrow())?;
                if self.balance(right, out.reborrow())? > 0 {
                    self.rotate_right(right, out.reborrow())?;
                }
                self.rotate_left(index, out.reborrow())?
            } else {
                index
            };
            current = self.parent(subtree_root, out.reborrow())?;
        }
        Ok(())
    }

    fn rotate_left<'slot>(
        &mut self,
        pivot: usize,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, usize> {
        let (_, right) = self.children(pivot, out.reborrow())?;
        let right = right
            .ok_or_else(index_invariant)
            .store_err_into(out.reborrow())?;
        let (middle, _) = self.children(right, out.reborrow())?;
        let parent = self.parent(pivot, out.reborrow())?;
        self.replace_parent_link(parent, pivot, right, out.reborrow())?;
        self.set_parent(right, parent, out.reborrow())?;
        self.set_left(right, Some(pivot), out.reborrow())?;
        self.set_parent(pivot, Some(right), out.reborrow())?;
        self.set_right(pivot, middle, out.reborrow())?;
        if let Some(middle) = middle {
            self.set_parent(middle, Some(pivot), out.reborrow())?;
        }
        self.update_height(pivot, out.reborrow())?;
        self.update_height(right, out.reborrow())?;
        Ok(right)
    }

    fn rotate_right<'slot>(
        &mut self,
        pivot: usize,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, usize> {
        let (left, _) = self.children(pivot, out.reborrow())?;
        let left = left
            .ok_or_else(index_invariant)
            .store_err_into(out.reborrow())?;
        let (_, middle) = self.children(left, out.reborrow())?;
        let parent = self.parent(pivot, out.reborrow())?;
        self.replace_parent_link(parent, pivot, left, out.reborrow())?;
        self.set_parent(left, parent, out.reborrow())?;
        self.set_right(left, Some(pivot), out.reborrow())?;
        self.set_parent(pivot, Some(left), out.reborrow())?;
        self.set_left(pivot, middle, out.reborrow())?;
        if let Some(middle) = middle {
            self.set_parent(middle, Some(pivot), out.reborrow())?;
        }
        self.update_height(pivot, out.reborrow())?;
        self.update_height(left, out.reborrow())?;
        Ok(left)
    }

    fn replace_parent_link<'slot>(
        &mut self,
        parent: Option<usize>,
        old: usize,
        new: usize,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ()> {
        match parent {
            Some(parent) => {
                let node = self
                    .nodes
                    .get_mut(parent)
                    .ok_or_else(index_invariant)
                    .store_err_into(out.reborrow())?;
                if node.left == Some(old) {
                    node.left = Some(new);
                } else if node.right == Some(old) {
                    node.right = Some(new);
                } else {
                    return Err(out.fail(index_invariant()));
                }
            }
            None => self.root = Some(new),
        }
        Ok(())
    }

    fn update_height<'slot>(
        &mut self,
        index: usize,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ()> {
        let (left, right) = self.children(index, out.reborrow())?;
        let height = self
            .height(left, out.reborrow())?
            .max(self.height(right, out.reborrow())?)
            .checked_add(1)
            .ok_or_else(index_invariant)
            .store_err_into(out.reborrow())?;
        let node = self
            .nodes
            .get_mut(index)
            .ok_or_else(index_invariant)
            .store_err_into(out.reborrow())?;
        node.height = height;
        Ok(())
    }

    fn balance<'slot>(
        &self,
        index: usize,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, i8> {
        let (left, right) = self.children(index, out.reborrow())?;
        let left_height = self.height(left, out.reborrow())?;
        let right_height = self.height(right, out.reborrow())?;
        Ok(if left_height.saturating_sub(right_height) > 1 {
            1
        } else if right_height.saturating_sub(left_height) > 1 {
            -1
        } else {
            0
        })
    }

    fn children<'slot>(
        &self,
        index: usize,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, (Option<usize>, Option<usize>)> {
        let node = self
            .nodes
            .get(index)
            .ok_or_else(index_invariant)
            .store_err_into(out.reborrow())?;
        Ok((node.left, node.right))
    }

    fn parent<'slot>(
        &self,
        index: usize,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, Option<usize>> {
        self.nodes
            .get(index)
            .map(|node| node.parent)
            .ok_or_else(index_invariant)
            .store_err_into(out.reborrow())
    }

    fn height<'slot>(
        &self,
        index: Option<usize>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, usize> {
        index.map_or(Ok(0), |index| {
            self.nodes
                .get(index)
                .map(|node| node.height)
                .ok_or_else(index_invariant)
                .store_err_into(out.reborrow())
        })
    }

    fn set_parent<'slot>(
        &mut self,
        index: usize,
        parent: Option<usize>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ()> {
        let node = self
            .nodes
            .get_mut(index)
            .ok_or_else(index_invariant)
            .store_err_into(out.reborrow())?;
        node.parent = parent;
        Ok(())
    }

    fn set_left<'slot>(
        &mut self,
        index: usize,
        left: Option<usize>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ()> {
        let node = self
            .nodes
            .get_mut(index)
            .ok_or_else(index_invariant)
            .store_err_into(out.reborrow())?;
        node.left = left;
        Ok(())
    }

    fn set_right<'slot>(
        &mut self,
        index: usize,
        right: Option<usize>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ()> {
        let node = self
            .nodes
            .get_mut(index)
            .ok_or_else(index_invariant)
            .store_err_into(out.reborrow())?;
        node.right = right;
        Ok(())
    }
}

fn index_invariant() -> CodecErrors {
    CodecErrors::emergency(CodecStage::Binding, Failure::internal(INDEX_INVARIANT))
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::BindingIndex;
    use crate::BindingTestSupport;
    use std::any::TypeId;
    use typol_test_support::{TestResult, test_verify as assert, test_verify_eq as assert_eq};

    struct IndexIdentity<const INDEX: usize>;

    macro_rules! identities {
        ($($index:literal),+ $(,)?) => {
            [$(TypeId::of::<IndexIdentity<$index>>(),)+]
        };
    }

    #[test]
    fn distinct_rust_types_sharing_a_schema_view_use_budgeted_logarithmic_indexing() -> TestResult {
        let schema = BindingTestSupport::compile("entry struct Root {}")?;
        let session = BindingTestSupport::codec_session()?;
        let mut index = BindingTestSupport::codec_value(|out| BindingIndex::new(&session, out))?;
        let identities = identities![
            0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23,
            24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45,
            46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63,
        ];
        let ty = schema.entry();
        for identity in identities {
            assert!(BindingTestSupport::codec_value(|out| {
                index.insert(identity, ty, &session, out)
            })?);
        }
        for identity in identities.into_iter().rev() {
            assert!(BindingTestSupport::codec_value(|out| {
                index.contains(identity, ty, &session, out)
            })?);
        }
        assert!(!BindingTestSupport::codec_value(|out| {
            index.insert(identities[0], ty, &session, out)
        })?);
        assert_eq!(index.nodes.len(), identities.len());
        assert!(session.execution().steps() < identities.len() * 32);
        Ok(())
    }
}
