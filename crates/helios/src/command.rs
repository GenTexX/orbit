//! helios edit commands: every scene edit as an undoable [`Edit`], driven through
//! a [`History`]. This is the layer every editor mutation goes through so undo
//! and redo come for free.
//!
//! `Edit` is a closed enum with `apply`/`revert`, matching the enum-over-trait
//! choice made for widgets and components (ADR 0014, ADR 0016): the edit set is
//! first-party and small. Each variant stores enough to both do and undo itself;
//! [`History`]'s methods capture the prior state so callers never build a
//! malformed edit.

use crate::reflect::Value;
use crate::scene::{Node, NodeId, Scene};
use crate::transform::Transform;

/// A single undoable edit to a [`Scene`].
#[derive(Debug, Clone)]
pub enum Edit {
    /// Attach an already-created node under a parent at an index (an add).
    Link {
        child: NodeId,
        parent: NodeId,
        index: usize,
    },
    /// Detach a node from its parent (a delete). The node stays in the arena so
    /// its handle survives undo.
    Unlink {
        child: NodeId,
        parent: NodeId,
        index: usize,
    },
    /// Move a node from one parent/index to another.
    Reparent {
        child: NodeId,
        old_parent: NodeId,
        old_index: usize,
        new_parent: NodeId,
        new_index: usize,
    },
    /// Replace a node's transform.
    SetTransform {
        node: NodeId,
        old: Transform,
        new: Transform,
    },
    /// Toggle a node's visibility (show/hide).
    SetVisible { node: NodeId, old: bool, new: bool },
    /// Replace one reflected field of one component on a node.
    SetField {
        node: NodeId,
        component: usize,
        field: String,
        old: Value,
        new: Value,
    },
}

impl Edit {
    fn apply(&self, scene: &mut Scene) {
        match self {
            Edit::Link {
                child,
                parent,
                index,
            } => scene.link(*child, *parent, *index),
            Edit::Unlink { child, .. } => scene.unlink(*child),
            Edit::Reparent {
                child,
                new_parent,
                new_index,
                ..
            } => {
                scene.unlink(*child);
                scene.link(*child, *new_parent, *new_index);
            }
            Edit::SetTransform { node, new, .. } => scene.node_mut(*node).transform = *new,
            Edit::SetVisible { node, new, .. } => scene.node_mut(*node).visible = *new,
            Edit::SetField {
                node,
                component,
                field,
                new,
                ..
            } => set_field(scene, *node, *component, field, new.clone()),
        }
    }

    fn revert(&self, scene: &mut Scene) {
        match self {
            Edit::Link { child, .. } => scene.unlink(*child),
            Edit::Unlink {
                child,
                parent,
                index,
            } => scene.link(*child, *parent, *index),
            Edit::Reparent {
                child,
                old_parent,
                old_index,
                ..
            } => {
                scene.unlink(*child);
                scene.link(*child, *old_parent, *old_index);
            }
            Edit::SetTransform { node, old, .. } => scene.node_mut(*node).transform = *old,
            Edit::SetVisible { node, old, .. } => scene.node_mut(*node).visible = *old,
            Edit::SetField {
                node,
                component,
                field,
                old,
                ..
            } => set_field(scene, *node, *component, field, old.clone()),
        }
    }
}

fn set_field(scene: &mut Scene, node: NodeId, component: usize, field: &str, value: Value) {
    if let Some(c) = scene.node_mut(node).components.get_mut(component) {
        c.as_reflect_mut().set(field, value);
    }
}

/// An undo/redo history of [`Edit`]s applied to a Scene. Applying a fresh edit
/// clears the redo stack, as usual.
#[derive(Debug, Default)]
pub struct History {
    done: Vec<Edit>,
    undone: Vec<Edit>,
}

impl History {
    /// An empty history.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether there is an edit to undo.
    pub fn can_undo(&self) -> bool {
        !self.done.is_empty()
    }

    /// Whether there is an edit to redo.
    pub fn can_redo(&self) -> bool {
        !self.undone.is_empty()
    }

    fn push(&mut self, scene: &mut Scene, edit: Edit) {
        edit.apply(scene);
        self.done.push(edit);
        self.undone.clear();
    }

    /// Undo the most recent edit. Returns `false` if there was nothing to undo.
    pub fn undo(&mut self, scene: &mut Scene) -> bool {
        match self.done.pop() {
            Some(edit) => {
                edit.revert(scene);
                self.undone.push(edit);
                true
            }
            None => false,
        }
    }

    /// Redo the most recently undone edit. Returns `false` if there was nothing.
    pub fn redo(&mut self, scene: &mut Scene) -> bool {
        match self.undone.pop() {
            Some(edit) => {
                edit.apply(scene);
                self.done.push(edit);
                true
            }
            None => false,
        }
    }

    /// Add `node` as the last child of `parent`, returning its handle. Undoable.
    pub fn add_node(&mut self, scene: &mut Scene, parent: NodeId, node: Node) -> NodeId {
        let child = scene.insert_detached(node);
        let index = scene.children(parent).len();
        self.push(
            scene,
            Edit::Link {
                child,
                parent,
                index,
            },
        );
        child
    }

    /// Remove `node` from the tree (it keeps its handle for undo). A no-op on the
    /// root, which has no parent.
    pub fn remove_node(&mut self, scene: &mut Scene, node: NodeId) {
        let (Some(parent), Some(index)) = (scene.parent(node), scene.child_index(node)) else {
            return;
        };
        self.push(
            scene,
            Edit::Unlink {
                child: node,
                parent,
                index,
            },
        );
    }

    /// Move `node` under `new_parent` at `new_index`. A no-op on the root, or if
    /// it would create a cycle (moving a node into its own subtree).
    pub fn reparent(
        &mut self,
        scene: &mut Scene,
        node: NodeId,
        new_parent: NodeId,
        new_index: usize,
    ) {
        if node == new_parent || scene.is_ancestor(node, new_parent) {
            return;
        }
        let (Some(old_parent), Some(old_index)) = (scene.parent(node), scene.child_index(node))
        else {
            return;
        };
        self.push(
            scene,
            Edit::Reparent {
                child: node,
                old_parent,
                old_index,
                new_parent,
                new_index,
            },
        );
    }

    /// Set `node`'s transform. Undoable.
    pub fn set_transform(&mut self, scene: &mut Scene, node: NodeId, transform: Transform) {
        let old = scene.node(node).transform;
        self.push(
            scene,
            Edit::SetTransform {
                node,
                old,
                new: transform,
            },
        );
    }

    /// Set `node`'s visibility (show/hide its subtree at render time). Undoable;
    /// a no-op that records nothing when the value is unchanged.
    pub fn set_visible(&mut self, scene: &mut Scene, node: NodeId, visible: bool) {
        let old = scene.node(node).visible;
        if old == visible {
            return;
        }
        self.push(
            scene,
            Edit::SetVisible {
                node,
                old,
                new: visible,
            },
        );
    }

    /// Set one reflected field of one component on `node`. Returns `false` (and
    /// records nothing) if the component index or field name is invalid.
    pub fn set_field(
        &mut self,
        scene: &mut Scene,
        node: NodeId,
        component: usize,
        field: &str,
        value: Value,
    ) -> bool {
        let Some(old) = scene
            .node(node)
            .components
            .get(component)
            .and_then(|c| c.as_reflect().get(field))
        else {
            return false;
        };
        self.push(
            scene,
            Edit::SetField {
                node,
                component,
                field: field.to_string(),
                old,
                new: value,
            },
        );
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::{Component, SpriteComponent};
    use glam::Vec2;

    fn scene_with_root() -> (Scene, NodeId) {
        let scene = Scene::new("root");
        let root = scene.root();
        (scene, root)
    }

    #[test]
    fn add_node_undo_redo_keeps_the_same_handle() {
        let (mut scene, root) = scene_with_root();
        let mut history = History::new();

        let node = history.add_node(&mut scene, root, Node::new("child"));
        assert_eq!(scene.children(root), &[node]);
        assert_eq!(scene.parent(node), Some(root));

        assert!(history.undo(&mut scene));
        assert!(scene.children(root).is_empty());
        assert_eq!(scene.parent(node), None); // detached, but still in the arena

        assert!(history.redo(&mut scene));
        // The very same handle is back in the tree.
        assert_eq!(scene.children(root), &[node]);
        assert_eq!(scene.parent(node), Some(root));
    }

    #[test]
    fn remove_node_is_undoable() {
        let (mut scene, root) = scene_with_root();
        let mut history = History::new();
        let node = history.add_node(&mut scene, root, Node::new("child"));

        history.remove_node(&mut scene, node);
        assert!(scene.children(root).is_empty());

        history.undo(&mut scene);
        assert_eq!(scene.children(root), &[node]);
    }

    #[test]
    fn set_visible_toggles_and_undoes() {
        let (mut scene, root) = scene_with_root();
        let mut history = History::new();
        let node = history.add_node(&mut scene, root, Node::new("child"));
        assert!(scene.node(node).visible);

        history.set_visible(&mut scene, node, false);
        assert!(!scene.node(node).visible);
        // Setting the current value records nothing: no phantom edit, so a
        // single undo (below) is enough to restore visibility.
        history.set_visible(&mut scene, node, false);

        history.undo(&mut scene); // one undo restores the only real change
        assert!(scene.node(node).visible);
    }

    #[test]
    fn set_transform_and_set_field_undo_to_their_old_values() {
        let (mut scene, root) = scene_with_root();
        let mut history = History::new();
        let mut node = Node::new("sprite");
        node.components
            .push(Component::Sprite(SpriteComponent::default()));
        let node = history.add_node(&mut scene, root, node);

        history.set_transform(
            &mut scene,
            node,
            Transform::from_translation(Vec2::new(9.0, 9.0)),
        );
        assert!(history.set_field(
            &mut scene,
            node,
            0,
            "texture",
            Value::Asset("hero.png".into())
        ));

        assert_eq!(scene.node(node).transform.translation, Vec2::new(9.0, 9.0));
        let Component::Sprite(s) = &scene.node(node).components[0];
        assert_eq!(s.texture, "hero.png");

        history.undo(&mut scene); // undo set_field
        let Component::Sprite(s) = &scene.node(node).components[0];
        assert_eq!(s.texture, ""); // back to the default

        history.undo(&mut scene); // undo set_transform
        assert_eq!(scene.node(node).transform.translation, Vec2::ZERO);
    }

    #[test]
    fn reparent_moves_and_undoes() {
        let (mut scene, root) = scene_with_root();
        let mut history = History::new();
        let a = history.add_node(&mut scene, root, Node::new("a"));
        let b = history.add_node(&mut scene, root, Node::new("b"));

        history.reparent(&mut scene, b, a, 0);
        assert_eq!(scene.parent(b), Some(a));
        assert_eq!(scene.children(root), &[a]);

        history.undo(&mut scene);
        assert_eq!(scene.parent(b), Some(root));
        assert_eq!(scene.children(root), &[a, b]);

        // Reparenting a node into its own subtree is refused.
        history.reparent(&mut scene, root, a, 0);
        assert_eq!(scene.parent(root), None);
    }

    #[test]
    fn a_new_edit_clears_the_redo_stack() {
        let (mut scene, root) = scene_with_root();
        let mut history = History::new();
        history.add_node(&mut scene, root, Node::new("a"));

        history.undo(&mut scene);
        assert!(history.can_redo());
        history.add_node(&mut scene, root, Node::new("b")); // a fresh edit
        assert!(!history.can_redo());
    }

    #[test]
    fn undoing_a_whole_session_restores_the_original_scene() {
        // The keystone: any sequence of edits, fully undone, yields exactly the
        // starting scene (compared by its serialized form).
        let (mut scene, root) = scene_with_root();
        let before = scene.to_ron().unwrap();
        let mut history = History::new();

        let mut sprite = Node::new("sprite");
        sprite
            .components
            .push(Component::Sprite(SpriteComponent::default()));
        let sprite = history.add_node(&mut scene, root, sprite);
        let child = history.add_node(&mut scene, sprite, Node::new("child"));
        history.set_transform(
            &mut scene,
            sprite,
            Transform::from_translation(Vec2::new(3.0, 4.0)),
        );
        history.set_field(
            &mut scene,
            sprite,
            0,
            "texture",
            Value::Asset("x.png".into()),
        );
        history.reparent(&mut scene, child, root, 0);
        history.remove_node(&mut scene, child);

        let after = scene.to_ron().unwrap();
        assert_ne!(after, before);

        while history.undo(&mut scene) {}
        assert_eq!(scene.to_ron().unwrap(), before, "fully undone == original");

        while history.redo(&mut scene) {}
        assert_eq!(scene.to_ron().unwrap(), after, "fully redone == edited");
    }
}
