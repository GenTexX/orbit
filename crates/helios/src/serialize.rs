//! helios scene serialization: RON for syntax, `Reflect` for the field walk.
//!
//! The domain types (`Scene`/`Node`/`Component`) are never serde-derived. These
//! on-disk DTOs are the bridge (ADR 0017): on save, a component's fields come
//! from [`Reflect::get`]; on load, they go back through [`Reflect::set`] - so
//! what is saved is exactly what the inspector edits, and the `ron` crate only
//! supplies the text syntax.

use glam::Vec2;
use ron::ser::PrettyConfig;
use serde::{Deserialize, Serialize};

use crate::component::Component;
use crate::error::HeliosError;
use crate::reflect::Value;
use crate::scene::{Node, NodeId, Scene};
use crate::transform::Transform;

/// The format this build writes.
///
/// One line, and it cannot be added honestly later: a file with no version
/// could have been written by any build, so the first version number is the
/// only one whose absence means something specific. Bumped when a change to the
/// shape would make an older build read a newer file wrongly - which is not the
/// same as adding a field, since an unknown field is already ignored.
const FORMAT: u32 = 1;

#[derive(Serialize, Deserialize)]
struct SceneDoc {
    /// Absent in files written before this existed, which are format 1 by
    /// definition - nothing incompatible had happened yet.
    #[serde(default = "first_format")]
    version: u32,
    root: NodeDoc,
}

fn first_format() -> u32 {
    1
}

#[derive(Serialize, Deserialize)]
struct NodeDoc {
    name: String,
    transform: TransformDoc,
    // Older scenes predate node visibility; default to visible so they load.
    // Only hidden nodes serialize a `visible: false`, keeping files clean.
    #[serde(default = "visible_default", skip_serializing_if = "is_visible")]
    visible: bool,
    #[serde(default)]
    components: Vec<ComponentDoc>,
    #[serde(default)]
    children: Vec<NodeDoc>,
}

fn visible_default() -> bool {
    true
}

fn is_visible(visible: &bool) -> bool {
    *visible
}

#[derive(Serialize, Deserialize)]
struct TransformDoc {
    translation: [f32; 2],
    rotation: f32,
    scale: [f32; 2],
}

#[derive(Serialize, Deserialize)]
struct ComponentDoc {
    kind: String,
    fields: Vec<(String, Value)>,
}

impl Scene {
    /// Serialize this scene to pretty, diff-friendly RON (ADR 0017).
    pub fn to_ron(&self) -> Result<String, HeliosError> {
        let doc = SceneDoc {
            version: FORMAT,
            root: node_to_doc(self, self.root()),
        };
        Ok(ron::ser::to_string_pretty(&doc, PrettyConfig::default())?)
    }

    /// Rebuild a scene from RON produced by [`to_ron`](Self::to_ron).
    ///
    /// The recursion limit is raised because a `NodeDoc` nests structurally:
    /// `ron`'s default of 128 levels divided by the four serde levels each node
    /// costs meant a tree deeper than about 30 saved fine and then could not be
    /// read back - the editor writing a file it refuses to open. Nothing in the
    /// editor builds trees that deep on its own, but reparenting by hand can,
    /// and losing a scene to that is not a trade worth keeping.
    pub fn from_ron(text: &str) -> Result<Scene, HeliosError> {
        let doc: SceneDoc = ron::Options::default()
            .with_recursion_limit(1024)
            .from_str(text)?;
        // A file from a newer build says so, rather than being read as if the
        // shape had not changed. There is nothing to migrate yet - this is
        // format 1 - but a reader that cannot tell has no way to start.
        if doc.version > FORMAT {
            return Err(HeliosError::Format(format!(
                "this scene was written in format {} and this build reads {FORMAT} - \
                 it needs a newer Orbit",
                doc.version
            )));
        }
        let mut scene = Scene::new(&doc.root.name);
        let root = scene.root();
        apply_doc(&mut scene, root, &doc.root)?;
        Ok(scene)
    }
}

fn node_to_doc(scene: &Scene, id: NodeId) -> NodeDoc {
    let node = scene.node(id);
    NodeDoc {
        name: node.name.clone(),
        transform: transform_to_doc(node.transform),
        visible: node.visible,
        components: node.components.iter().map(component_to_doc).collect(),
        children: scene
            .children(id)
            .iter()
            .map(|&c| node_to_doc(scene, c))
            .collect(),
    }
}

fn transform_to_doc(t: Transform) -> TransformDoc {
    TransformDoc {
        translation: t.translation.to_array(),
        rotation: t.rotation,
        scale: t.scale.to_array(),
    }
}

fn component_to_doc(component: &Component) -> ComponentDoc {
    let reflect = component.as_reflect();
    let fields = reflect
        .field_names()
        .into_iter()
        .filter_map(|name| reflect.get(&name).map(|v| (name, v)))
        .collect();
    ComponentDoc {
        kind: reflect.type_name().to_string(),
        fields,
    }
}

/// Fill an already-created node (its name is set) from a `NodeDoc`: its
/// transform, then its components (built by kind, fields applied via `Reflect`),
/// then its children, recursively.
fn apply_doc(scene: &mut Scene, id: NodeId, doc: &NodeDoc) -> Result<(), HeliosError> {
    scene.node_mut(id).transform = doc_to_transform(&doc.transform);
    scene.node_mut(id).visible = doc.visible;
    for cd in &doc.components {
        let mut component = Component::from_type_name(&cd.kind)
            .ok_or_else(|| HeliosError::UnknownComponent(cd.kind.clone()))?;
        let reflect = component.as_reflect_mut();
        for (field, value) in &cd.fields {
            // Unknown or mismatched fields are skipped, so a scene saved by a
            // newer build still loads in an older one (forward-compatible).
            reflect.set(field, value.clone());
        }
        scene.node_mut(id).components.push(component);
    }
    for child_doc in &doc.children {
        let child = scene.add_child(id, Node::new(&child_doc.name));
        apply_doc(scene, child, child_doc)?;
    }
    Ok(())
}

fn doc_to_transform(d: &TransformDoc) -> Transform {
    Transform {
        translation: Vec2::from_array(d.translation),
        rotation: d.rotation,
        scale: Vec2::from_array(d.scale),
    }
}

#[cfg(test)]
mod tests {
    use crate::component::{Component, ScriptComponent, SpriteComponent};
    use crate::reflect::Value;
    use crate::scene::{Node, Scene};
    use crate::transform::Transform;
    use glam::Vec2;

    fn demo_scene() -> Scene {
        let mut scene = Scene::new("root");
        let root = scene.root();

        let mut player = Node::new("player");
        player.transform = Transform::from_translation(Vec2::new(100.0, 50.0));
        let sprite = SpriteComponent {
            texture: "player.png".into(),
            tint: [1.0, 0.5, 0.25, 1.0],
            size: Vec2::new(32.0, 48.0),
            // A sheet cell, mirrored: the fields added for sprite sheets have
            // to survive a save and a load like every other one.
            uv_offset: Vec2::new(0.5, 0.0),
            uv_size: Vec2::new(0.25, 0.5),
            flip_x: true,
            flip_y: false,
        };
        player.components.push(Component::Sprite(sprite));
        // Tuned exported values, deliberately: a Script whose `exports` is empty
        // is the one shape where a broken load path is a no-op, and this fixture
        // used to have exactly that - so every serialization test passed while
        // every tuned value in a real project was being dropped on load.
        player.components.push(Component::Script(ScriptComponent {
            source: "scripts/player.cmt".into(),
            exports: vec![
                ("speed".to_string(), Value::F32(240.0)),
                ("home".to_string(), Value::Vec2(Vec2::new(12.0, -3.0))),
                ("chases".to_string(), Value::Bool(true)),
            ],
            ..Default::default()
        }));
        let player = scene.add_child(root, player);

        scene.add_child(player, Node::new("weapon"));
        scene
    }

    #[test]
    fn a_scene_survives_a_ron_round_trip() {
        let scene = demo_scene();
        let ron = scene.to_ron().unwrap();
        let reloaded = Scene::from_ron(&ron).unwrap();

        // Re-serializing the reloaded scene yields identical text: nothing was
        // lost or reordered by save-then-load.
        assert_eq!(reloaded.to_ron().unwrap(), ron);
    }

    #[test]
    fn reloaded_values_match_the_original() {
        let scene = demo_scene();
        let reloaded = Scene::from_ron(&scene.to_ron().unwrap()).unwrap();

        let player = reloaded.children(reloaded.root())[0];
        assert_eq!(reloaded.node(player).name, "player");
        assert_eq!(
            reloaded.node(player).transform.translation,
            Vec2::new(100.0, 50.0)
        );
        assert_eq!(reloaded.children(player).len(), 1); // the weapon child

        let Component::Sprite(sprite) = &reloaded.node(player).components[0] else {
            panic!("the test saved a sprite");
        };
        assert_eq!(sprite.texture, "player.png");
        assert_eq!(sprite.tint, [1.0, 0.5, 0.25, 1.0]);
        assert_eq!(sprite.size, Vec2::new(32.0, 48.0));

        // A second component kind on the same node: both survive, in order.
        let Component::Script(script) = &reloaded.node(player).components[1] else {
            panic!("the test saved a script after the sprite");
        };
        assert_eq!(script.source, "scripts/player.cmt");
        // The tuning, which is the whole point of an exported variable: what
        // the inspector owns has to be what comes back (ADR 0022).
        assert_eq!(
            script.exports,
            vec![
                ("speed".to_string(), Value::F32(240.0)),
                ("home".to_string(), Value::Vec2(Vec2::new(12.0, -3.0))),
                ("chases".to_string(), Value::Bool(true)),
            ]
        );
    }

    #[test]
    fn a_deep_tree_can_be_read_back_after_it_is_written() {
        // NodeDoc nests structurally, and ron's default recursion limit of 128
        // divided by the levels serde spends per node stopped a load at about
        // 30 deep - so the editor wrote a file it then refused to open, which
        // is the one failure a save format must not have.
        let mut scene = Scene::new("root");
        let mut parent = scene.root();
        for i in 0..200 {
            parent = scene.add_child(parent, Node::new(format!("deep{i}")));
        }
        let ron = scene.to_ron().expect("it writes");
        let reloaded = Scene::from_ron(&ron).expect("and reads back");
        assert_eq!(reloaded.to_ron().unwrap(), ron);
    }

    #[test]
    fn an_unknown_component_kind_is_an_error() {
        // Take real output and rename the only component kind to one this build
        // does not know; loading it must fail cleanly, not panic.
        let ron = demo_scene().to_ron().unwrap().replace("Sprite", "Mystery");
        assert!(matches!(
            Scene::from_ron(&ron),
            Err(crate::error::HeliosError::UnknownComponent(_))
        ));
    }

    #[test]
    fn a_scene_carries_its_format_version() {
        let scene = Scene::new("root");
        let text = scene.to_ron().expect("it serializes");
        assert!(text.contains("version: 1"), "{text}");
    }

    #[test]
    fn a_file_from_a_newer_build_says_so_rather_than_being_guessed_at() {
        let text = "(version: 99, root: (name: \"root\", transform: (translation: (0.0, 0.0), \
                    rotation: 0.0, scale: (1.0, 1.0)), components: [], children: []))";
        let err = Scene::from_ron(text).expect_err("this build cannot read format 99");
        assert!(err.to_string().contains("needs a newer Orbit"), "{err}");
    }

    #[test]
    fn a_file_written_before_versions_existed_still_loads() {
        // The reason the first version number is the only one whose absence
        // means something: every file written until now has none.
        let text = "(root: (name: \"root\", transform: (translation: (0.0, 0.0), \
                    rotation: 0.0, scale: (1.0, 1.0)), components: [], children: []))";
        let scene = Scene::from_ron(text).expect("an unversioned file is format 1");
        assert_eq!(scene.node(scene.root()).name, "root");
    }
}
