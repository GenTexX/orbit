//! atlas edit actions: the editor's higher-level scene edits, built on
//! helios's undoable History (M3 step 8). Pure scene logic - headless and
//! unit-tested; main.rs only decides when to call them.

use glam::Vec2;
use helios::{Component, History, Node, NodeId, Scene, SpriteComponent, Transform};

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

#[cfg(test)]
mod tests {
    use super::*;

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
        let Component::Sprite(s) = &scene.node(node).components[0];
        assert_eq!(s.texture, "assets/sprite.png");
        assert_eq!(s.size, Vec2::new(64.0, 64.0));

        // One undo removes it again.
        assert!(history.undo(&mut scene));
        assert!(scene.children(scene.root()).is_empty());
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
