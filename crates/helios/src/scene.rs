//! helios scene: the Node tree (ADR 0003) as a slotmap arena of Nodes with handles.

use glam::{Affine2, Vec2};
use slotmap::{SlotMap, new_key_type};

use crate::component::{CameraComponent, Component};
use crate::transform::Transform;

new_key_type! {
    /// A typed handle to a Node in a [`Scene`]. Cheap to copy; stays valid across
    /// edits until the node is removed.
    pub struct NodeId;
}

/// A node in the scene tree: a name, a transform, its children, and its attached
/// components. All capability lives in the components (ADR 0003); the node itself
/// only positions things and holds the tree together.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    /// The node's display name (shown in the scene-tree panel).
    pub name: String,
    /// The node's local transform, relative to its parent.
    pub transform: Transform,
    /// Whether this node (and its subtree) is drawn. A hidden node keeps its
    /// place in the tree and stays editable; it is only skipped at render time.
    pub visible: bool,
    /// The capabilities attached to this node.
    pub components: Vec<Component>,
    pub(crate) parent: Option<NodeId>,
    pub(crate) children: Vec<NodeId>,
}

impl Node {
    /// A named node at the identity transform, with no components.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            transform: Transform::IDENTITY,
            visible: true,
            components: Vec::new(),
            parent: None,
            children: Vec::new(),
        }
    }
}

/// A tree of Nodes with a single root (ADR 0003): the unit the editor edits and
/// the serializer walks. Nodes live in an arena and are addressed by [`NodeId`].
#[derive(Debug, Clone)]
pub struct Scene {
    nodes: SlotMap<NodeId, Node>,
    root: NodeId,
}

impl Scene {
    /// A new scene with a single root node named `root_name`.
    pub fn new(root_name: impl Into<String>) -> Self {
        let mut nodes = SlotMap::with_key();
        let root = nodes.insert(Node::new(root_name));
        Self { nodes, root }
    }

    /// The scene's root node handle.
    pub fn root(&self) -> NodeId {
        self.root
    }

    /// Insert `node` as a child of `parent`, returning its handle.
    pub fn add_child(&mut self, parent: NodeId, node: Node) -> NodeId {
        let id = self.nodes.insert(node);
        self.nodes[id].parent = Some(parent);
        self.nodes[parent].children.push(id);
        id
    }

    /// A node by handle.
    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id]
    }

    /// A node by handle, mutably.
    pub fn node_mut(&mut self, id: NodeId) -> &mut Node {
        &mut self.nodes[id]
    }

    /// The children of a node, in order.
    pub fn children(&self, id: NodeId) -> &[NodeId] {
        &self.nodes[id].children
    }

    /// The parent of a node, or `None` for the root.
    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.nodes[id].parent
    }

    /// The number of nodes in the scene, including the root.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// A scene always has its root, so it is never empty; provided to satisfy the
    /// `len`-without-`is_empty` lint honestly.
    pub fn is_empty(&self) -> bool {
        false
    }

    /// The node's world transform: its local transform composed with all of its
    /// ancestors', from the root down (world = parent world x local).
    pub fn world_transform(&self, id: NodeId) -> Affine2 {
        let local = self.nodes[id].transform.to_affine();
        match self.nodes[id].parent {
            Some(parent) => self.world_transform(parent) * local,
            None => local,
        }
    }

    /// The camera the game is played through, and where it is in the world.
    ///
    /// The first active [`CameraComponent`](crate::CameraComponent) in scene
    /// pre-order - the same order scripts run in, so "first" means the same
    /// thing everywhere - together with the world position of the node carrying
    /// it. `None` when the scene has no camera, which a host answers however it
    /// likes; the editor falls back to the view you were already looking at.
    ///
    /// Pre-order rather than "the one marked default", because a scene with two
    /// active cameras is a mistake rather than a configuration, and the rule
    /// that resolves it should be one a person can predict from the panel
    /// rather than one they have to go looking for.
    pub fn active_camera(&self) -> Option<(&CameraComponent, Vec2)> {
        let mut stack = vec![self.root()];
        while let Some(id) = stack.pop() {
            for component in &self.nodes[id].components {
                if let Component::Camera(camera) = component
                    && camera.active
                {
                    return Some((camera, self.world_transform(id).translation));
                }
            }
            stack.extend(self.children(id).iter().rev().copied());
        }
        None
    }

    // ---- low-level tree ops, used by the edit commands (command.rs) ----
    //
    // Delete detaches rather than removes, so a node keeps its `NodeId` while it
    // sits in the undo history (slotmap cannot re-insert a freed key). A node
    // detached and never re-linked stays in the arena until the scene is dropped
    // or re-saved (it is not walked from the root); harmless for an editor.

    /// Insert a node into the arena without attaching it to the tree, returning
    /// its handle. Both tree links are cleared: a detached node has no parent and
    /// no children until the caller rebuilds them (see [`link`](Self::link)), so
    /// a node cloned from an existing one does not carry over stale child
    /// handles. The caller links it and, for a subtree, its children.
    pub(crate) fn insert_detached(&mut self, mut node: Node) -> NodeId {
        node.parent = None;
        node.children.clear();
        self.nodes.insert(node)
    }

    /// Attach `child` under `parent` at `index` (clamped to the child count).
    pub(crate) fn link(&mut self, child: NodeId, parent: NodeId, index: usize) {
        self.nodes[child].parent = Some(parent);
        let children = &mut self.nodes[parent].children;
        let index = index.min(children.len());
        children.insert(index, child);
    }

    /// Detach `child` from its parent, leaving it in the arena.
    pub(crate) fn unlink(&mut self, child: NodeId) {
        if let Some(parent) = self.nodes[child].parent.take()
            && let Some(pos) = self.nodes[parent].children.iter().position(|&c| c == child)
        {
            self.nodes[parent].children.remove(pos);
        }
    }

    /// The index of `child` within its parent's children, or `None` if detached.
    pub(crate) fn child_index(&self, child: NodeId) -> Option<usize> {
        let parent = self.nodes[child].parent?;
        self.nodes[parent].children.iter().position(|&c| c == child)
    }

    /// Whether `ancestor` lies on the path from `of` up to the root (so
    /// reparenting `of` under `ancestor`'s subtree would create a cycle).
    pub fn is_ancestor(&self, ancestor: NodeId, of: NodeId) -> bool {
        let mut cur = self.nodes[of].parent;
        while let Some(id) = cur {
            if id == ancestor {
                return true;
            }
            cur = self.nodes[id].parent;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec2;

    #[test]
    fn a_new_scene_has_just_its_root() {
        let scene = Scene::new("root");
        assert_eq!(scene.len(), 1);
        assert_eq!(scene.node(scene.root()).name, "root");
        assert_eq!(scene.parent(scene.root()), None);
        assert!(scene.children(scene.root()).is_empty());
    }

    #[test]
    fn children_link_to_their_parent_both_ways() {
        let mut scene = Scene::new("root");
        let root = scene.root();
        let a = scene.add_child(root, Node::new("a"));
        let b = scene.add_child(root, Node::new("b"));
        let grandchild = scene.add_child(a, Node::new("a.child"));

        assert_eq!(scene.children(root), &[a, b]);
        assert_eq!(scene.parent(a), Some(root));
        assert_eq!(scene.parent(grandchild), Some(a));
        assert_eq!(scene.children(a), &[grandchild]);
    }

    #[test]
    fn world_transform_composes_translations_down_the_tree() {
        let mut scene = Scene::new("root");
        let root = scene.root();
        scene.node_mut(root).transform = Transform::from_translation(Vec2::new(10.0, 0.0));

        let child = scene.add_child(root, {
            let mut n = Node::new("child");
            n.transform = Transform::from_translation(Vec2::new(5.0, 2.0));
            n
        });

        // The child sits at the sum of the two translations in world space.
        let world = scene.world_transform(child);
        assert_eq!(world.transform_point2(Vec2::ZERO), Vec2::new(15.0, 2.0));
    }

    #[test]
    fn a_parent_scale_scales_its_children_offsets() {
        let mut scene = Scene::new("root");
        let root = scene.root();
        scene.node_mut(root).transform = Transform {
            scale: Vec2::splat(2.0),
            ..Transform::IDENTITY
        };
        let child = scene.add_child(root, {
            let mut n = Node::new("child");
            n.transform = Transform::from_translation(Vec2::new(5.0, 0.0));
            n
        });

        // The parent's 2x scale doubles the child's local offset in world space.
        let world = scene.world_transform(child);
        assert_eq!(world.transform_point2(Vec2::ZERO), Vec2::new(10.0, 0.0));
    }

    #[test]
    fn the_active_camera_is_the_first_one_in_pre_order() {
        // Pre-order, the same order scripts run in, so "first" means the same
        // thing everywhere in the engine. Two active cameras is a mistake
        // rather than a configuration, and the rule that resolves it should be
        // one a person can read off the panel.
        let mut scene = Scene::new("root");
        let a = scene.add_child(scene.root(), Node::new("a"));
        let child = scene.add_child(a, Node::new("a-child"));
        let b = scene.add_child(scene.root(), Node::new("b"));

        for node in [child, b] {
            scene
                .node_mut(node)
                .components
                .push(Component::Camera(CameraComponent::default()));
        }
        scene.node_mut(child).transform.translation = Vec2::new(7.0, 0.0);
        scene.node_mut(b).transform.translation = Vec2::new(99.0, 0.0);

        let (_, at) = scene.active_camera().expect("a camera");
        assert_eq!(at, Vec2::new(7.0, 0.0), "the deeper one comes first");

        // Turning it off is how you switch, without moving anything.
        let Some(Component::Camera(camera)) = scene.node_mut(child).components.first_mut() else {
            panic!("it is a camera");
        };
        camera.active = false;
        let (_, at) = scene.active_camera().expect("the other camera");
        assert_eq!(at, Vec2::new(99.0, 0.0));
    }

    #[test]
    fn a_camera_reports_where_it_is_in_the_world_not_where_it_is_in_its_parent() {
        // The whole point of a camera being a component: parent it to the
        // player and it follows, with no code and no script.
        let mut scene = Scene::new("root");
        let player = scene.add_child(scene.root(), Node::new("player"));
        scene.node_mut(player).transform.translation = Vec2::new(500.0, 300.0);
        let eye = scene.add_child(player, Node::new("camera"));
        scene.node_mut(eye).transform.translation = Vec2::new(0.0, -40.0);
        scene
            .node_mut(eye)
            .components
            .push(Component::Camera(CameraComponent::default()));

        let (_, at) = scene.active_camera().expect("a camera");
        assert_eq!(at, Vec2::new(500.0, 260.0));
    }

    #[test]
    fn a_scene_with_no_camera_has_none() {
        let scene = Scene::new("root");
        assert!(scene.active_camera().is_none());
    }
}
