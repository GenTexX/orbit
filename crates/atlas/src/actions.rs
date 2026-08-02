//! atlas edit actions: the editor's higher-level scene edits, built on
//! helios's undoable History (M3 step 8). Pure scene logic - headless and
//! unit-tested; main.rs only decides when to call them.

use glam::Vec2;
use helios::{
    Component, History, Node, NodeId, Scene, ScriptComponent, SpriteComponent, Transform,
};

/// Create a new sprite node under the scene root, centered at `world` (the
/// node origin is the sprite's center, ADR 0019), showing `texture` at `size`
/// pixels. Undoable: one history step. Returns the new node for selection.
pub fn spawn_sprite(
    scene: &mut Scene,
    history: &mut History,
    world: Vec2,
    texture: &str,
    size: Vec2,
) -> NodeId {
    let root = scene.root();
    // `world` is a world-space point (a drop position or the view center); the
    // node's translation is in the root's local space, so map it through the
    // inverse of the root's world transform (identity in practice, but roots
    // are transformable nodes like any other).
    let local = scene
        .world_transform(root)
        .inverse()
        .transform_point2(world);

    let mut node = Node::new(format!("Sprite {}", scene.len()));
    node.transform = Transform::from_translation(local);
    node.components.push(Component::Sprite(SpriteComponent {
        texture: texture.to_string(),
        tint: [1.0, 1.0, 1.0, 1.0],
        size,
    }));
    history.add_node(scene, root, node)
}

/// Attach an empty [`ScriptComponent`] to `node`, returning its index. Undoable:
/// one history step. Returns `None` if the node already has a script - a node
/// runs one script, so a second one would just be an inspector row nothing ever
/// reads.
///
/// The source path starts empty. Picking the `.cmt` file is the inspector's job,
/// through the same asset field a sprite's texture uses.
/// A node may run several scripts: one for movement, one for what it collides
/// with, one for what it says. They are separate files with separate exported
/// values, and nothing about the component or the host needs them to be one.
pub fn attach_script(scene: &mut Scene, history: &mut History, node: NodeId) -> usize {
    history.add_component(scene, node, Component::Script(ScriptComponent::default()))
}

/// The first of `node`'s script components with no file picked yet.
///
/// What a dropped `.cmt` fills: adding a script and then dropping one on the
/// node should put it in the empty slot rather than leaving that behind and
/// making a second.
pub fn empty_script_index(scene: &Scene, node: NodeId) -> Option<usize> {
    scene
        .node(node)
        .components
        .iter()
        .position(|c| matches!(c, Component::Script(s) if s.source.is_empty()))
}

/// Deep-clone the subtree rooted at `src` as a new sibling right after it,
/// through the history as one undo step (the whole subtree, plus the " copy"
/// rename of the new root). Returns the new root, or `None` for the scene root
/// (which has no parent to be a sibling of).
pub fn duplicate_node(scene: &mut Scene, history: &mut History, src: NodeId) -> Option<NodeId> {
    let parent = scene.parent(src)?;
    let index = scene.children(parent).iter().position(|&c| c == src)? + 1;
    history.begin_group();
    let new = clone_into(scene, history, src, parent, index);
    let name = format!("{} copy", scene.node(new).name);
    history.set_name(scene, new, name);
    history.end_group(scene);
    Some(new)
}

/// Recursively clone the subtree at `src` under `parent` at `index`, creating a
/// fresh node for each (new handles, same names/transforms/components), and
/// return the new subtree root. Each link is an edit on the open history group.
fn clone_into(
    scene: &mut Scene,
    history: &mut History,
    src: NodeId,
    parent: NodeId,
    index: usize,
) -> NodeId {
    // Clone the node's data; `insert_detached` (via `add_node_at`) drops the
    // stale parent/child handles, and we rebuild the children as we recurse.
    let node = scene.node(src).clone();
    let children = scene.children(src).to_vec();
    let new = history.add_node_at(scene, parent, node, index);
    for child in children {
        let at = scene.children(new).len();
        clone_into(scene, history, child, new, at);
    }
    new
}

/// A detached copy of a node and its subtree, independent of any scene - the
/// unit the editor's node clipboard holds so copy/paste survives across scene
/// edits (and, unlike a bare `NodeId`, refers to no live node).
#[derive(Debug, Clone)]
pub struct NodeSubtree {
    node: Node,
    children: Vec<NodeSubtree>,
}

/// Capture the subtree rooted at `src` into a detached [`NodeSubtree`] (for the
/// node clipboard). Copies names, transforms, and components; drops the live
/// parent/child handles, which [`paste_subtree`] rebuilds.
pub fn capture_subtree(scene: &Scene, src: NodeId) -> NodeSubtree {
    let node = scene.node(src).clone();
    let children = scene
        .children(src)
        .iter()
        .map(|&c| capture_subtree(scene, c))
        .collect();
    NodeSubtree { node, children }
}

/// Instantiate a captured [`NodeSubtree`] under `parent` at `index`, through the
/// history as one undo step. Returns the new subtree root.
pub fn paste_subtree(
    scene: &mut Scene,
    history: &mut History,
    tree: &NodeSubtree,
    parent: NodeId,
    index: usize,
) -> NodeId {
    history.begin_group();
    let new = instantiate(scene, history, tree, parent, index);
    history.end_group(scene);
    new
}

fn instantiate(
    scene: &mut Scene,
    history: &mut History,
    tree: &NodeSubtree,
    parent: NodeId,
    index: usize,
) -> NodeId {
    let new = history.add_node_at(scene, parent, tree.node.clone(), index);
    for child in &tree.children {
        let at = scene.children(new).len();
        instantiate(scene, history, child, new, at);
    }
    new
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A root with `parent -> child -> grandchild`, for the subtree tests.
    fn nested_scene() -> (Scene, History, NodeId) {
        let mut scene = Scene::new("Root");
        let mut history = History::new();
        let root = scene.root();
        let parent = history.add_node(&mut scene, root, Node::new("Parent"));
        let child = history.add_node(&mut scene, parent, Node::new("Child"));
        history.add_node(&mut scene, child, Node::new("Grandchild"));
        (scene, history, parent)
    }

    #[test]
    fn duplicate_deep_clones_the_subtree_beside_the_original() {
        let (mut scene, mut history, parent) = nested_scene();
        let root = scene.root();

        let copy = duplicate_node(&mut scene, &mut history, parent).unwrap();
        // The copy lands right after the original, as a new sibling.
        assert_eq!(scene.children(root), &[parent, copy]);
        assert_eq!(scene.node(copy).name, "Parent copy");
        // Fresh handles all the way down, but the same shape.
        let orig_child = scene.children(parent)[0];
        let copy_child = scene.children(copy)[0];
        assert_ne!(orig_child, copy_child);
        assert_eq!(scene.node(copy_child).name, "Child");
        assert_eq!(scene.children(copy_child).len(), 1);
        assert_eq!(scene.node(scene.children(copy_child)[0]).name, "Grandchild");

        // The whole duplicate is one undo step.
        assert!(history.undo(&mut scene));
        assert_eq!(scene.children(root), &[parent]);
    }

    #[test]
    fn copy_then_paste_round_trips_the_subtree() {
        let (mut scene, mut history, parent) = nested_scene();
        let root = scene.root();

        let clip = capture_subtree(&scene, parent);
        // Paste under the root as a new last child.
        let at = scene.children(root).len();
        let pasted = paste_subtree(&mut scene, &mut history, &clip, root, at);

        assert_eq!(scene.children(root), &[parent, pasted]);
        assert_ne!(pasted, parent);
        assert_eq!(scene.node(pasted).name, "Parent");
        let pasted_child = scene.children(pasted)[0];
        assert_eq!(scene.node(pasted_child).name, "Child");
        assert_eq!(scene.children(pasted_child).len(), 1);

        // The captured clip is independent: pasting again yields another copy.
        let again = paste_subtree(&mut scene, &mut history, &clip, root, 3);
        assert_ne!(again, pasted);
        assert_eq!(scene.children(root).len(), 3);

        // Each paste is its own undo step.
        assert!(history.undo(&mut scene));
        assert_eq!(scene.children(root), &[parent, pasted]);
    }

    #[test]
    fn spawn_places_the_sprite_at_the_world_point_and_is_undoable() {
        let mut scene = Scene::new("Root");
        let mut history = History::new();

        let node = spawn_sprite(
            &mut scene,
            &mut history,
            Vec2::new(120.0, 80.0),
            "assets/sprite.png",
            Vec2::new(64.0, 64.0),
        );

        assert_eq!(scene.children(scene.root()), &[node]);
        assert_eq!(
            scene.node(node).transform.translation,
            Vec2::new(120.0, 80.0)
        );
        let Component::Sprite(s) = &scene.node(node).components[0] else {
            panic!("spawn_sprite attaches a sprite");
        };
        assert_eq!(s.texture, "assets/sprite.png");
        assert_eq!(s.size, Vec2::new(64.0, 64.0));

        // One undo removes it again.
        assert!(history.undo(&mut scene));
        assert!(scene.children(scene.root()).is_empty());
    }

    #[test]
    fn attach_script_adds_one_script_and_only_one() {
        let mut scene = Scene::new("Root");
        let mut history = History::new();
        let node = spawn_sprite(
            &mut scene,
            &mut history,
            Vec2::ZERO,
            "assets/sprite.png",
            Vec2::splat(64.0),
        );

        assert_eq!(
            attach_script(&mut scene, &mut history, node),
            1,
            "the script lands after the sprite already there"
        );
        assert!(matches!(
            scene.node(node).components[1],
            Component::Script(_)
        ));

        // A node may run several: one for movement, one for what it says.
        assert_eq!(attach_script(&mut scene, &mut history, node), 2);
        assert_eq!(scene.node(node).components.len(), 3);

        // A dropped file fills the first slot with no file picked yet, rather
        // than leaving it empty and making a fourth.
        assert_eq!(empty_script_index(&scene, node), Some(1));

        // One undo takes the second one back off.
        assert!(history.undo(&mut scene));
        assert_eq!(scene.node(node).components.len(), 2);

        // One undo takes it back off.
        assert!(history.undo(&mut scene));
        assert_eq!(scene.node(node).components.len(), 1);
    }

    #[test]
    fn an_empty_script_slot_is_found_wherever_it_sits() {
        let mut scene = Scene::new("Root");
        let mut history = History::new();
        let root = scene.root();
        let bare = history.add_node(&mut scene, root, Node::new("Bare"));
        assert_eq!(empty_script_index(&scene, bare), None);

        // Behind a sprite, not at index 0 - which is exactly the case that
        // would break a lookup written as "the first component".
        let node = spawn_sprite(
            &mut scene,
            &mut history,
            Vec2::ZERO,
            "assets/sprite.png",
            Vec2::splat(64.0),
        );
        attach_script(&mut scene, &mut history, node);
        assert_eq!(empty_script_index(&scene, node), Some(1));
    }

    #[test]
    fn spawn_maps_the_world_point_through_a_transformed_root() {
        let mut scene = Scene::new("Root");
        let root = scene.root();
        scene.node_mut(root).transform = Transform {
            translation: Vec2::new(100.0, 0.0),
            scale: Vec2::splat(2.0),
            ..Transform::IDENTITY
        };
        let mut history = History::new();

        // World (140, 40) under a root at +100 with 2x scale is local (20, 20),
        // so the spawned sprite lands exactly under the drop point on screen.
        let node = spawn_sprite(
            &mut scene,
            &mut history,
            Vec2::new(140.0, 40.0),
            "a.png",
            Vec2::ONE,
        );
        assert!((scene.node(node).transform.translation - Vec2::new(20.0, 20.0)).length() < 1.0e-4);
        // And its world position round-trips back to the drop point.
        let world = scene.world_transform(node).transform_point2(Vec2::ZERO);
        assert!((world - Vec2::new(140.0, 40.0)).length() < 1.0e-4);
    }
}
