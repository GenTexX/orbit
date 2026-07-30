//! helios components: the capabilities attached to a Node. One closed enum, each
//! variant's data reflectable (ADR 0016).

use glam::Vec2;

use crate::reflect::{Reflect, Value};

/// A unit of capability attached to a Node (ADR 0003). Adding a component means
/// adding a variant here and one [`Reflect`] impl.
#[derive(Debug, Clone, PartialEq)]
pub enum Component {
    /// Draws a texture at the owning node's transform.
    Sprite(SpriteComponent),
    /// Runs a Comet script for the owning node.
    Script(ScriptComponent),
}

impl Component {
    /// This component's data as a reflectable value (for the inspector and the
    /// serializer, which never name the concrete component type).
    pub fn as_reflect(&self) -> &dyn Reflect {
        match self {
            Component::Sprite(s) => s,
            Component::Script(s) => s,
        }
    }

    /// This component's data as a mutable reflectable value.
    pub fn as_reflect_mut(&mut self) -> &mut dyn Reflect {
        match self {
            Component::Sprite(s) => s,
            Component::Script(s) => s,
        }
    }

    /// Construct a default component of the given [`type_name`](Reflect::type_name),
    /// or `None` if the kind is unknown. Deserialization uses this, then applies
    /// the saved fields through [`Reflect::set`].
    pub fn from_type_name(kind: &str) -> Option<Component> {
        match kind {
            "Sprite" => Some(Component::Sprite(SpriteComponent::default())),
            "Script" => Some(Component::Script(ScriptComponent::default())),
            _ => None,
        }
    }
}

/// A drawable sprite: which texture to draw, and how to tint and size it. Where
/// it draws comes from the owning Node's transform, not from here.
#[derive(Debug, Clone, PartialEq)]
pub struct SpriteComponent {
    /// The texture asset to draw (a project-relative path, for now).
    pub texture: String,
    /// Multiplied into the texture's color; opaque white leaves it untinted.
    pub tint: [f32; 4],
    /// The sprite's size in pixels, before the node's scale is applied.
    pub size: Vec2,
}

impl Default for SpriteComponent {
    fn default() -> Self {
        Self {
            texture: String::new(),
            tint: [1.0, 1.0, 1.0, 1.0],
            size: Vec2::splat(100.0),
        }
    }
}

impl Reflect for SpriteComponent {
    fn type_name(&self) -> &'static str {
        "Sprite"
    }

    fn field_names(&self) -> &'static [&'static str] {
        &["texture", "tint", "size"]
    }

    fn get(&self, field: &str) -> Option<Value> {
        match field {
            "texture" => Some(Value::Asset(self.texture.clone())),
            "tint" => Some(Value::Color(self.tint)),
            "size" => Some(Value::Vec2(self.size)),
            _ => None,
        }
    }

    fn set(&mut self, field: &str, value: Value) -> bool {
        match (field, value) {
            ("texture", Value::Asset(s)) => {
                self.texture = s;
                true
            }
            ("tint", Value::Color(c)) => {
                self.tint = c;
                true
            }
            ("size", Value::Vec2(v)) => {
                self.size = v;
                true
            }
            _ => false,
        }
    }
}

/// A Comet script attached to a node: which `.cmt` source file runs for it.
///
/// Only the path lives here. Compiling it, instantiating it, and binding it to
/// this node's [`Transform`](crate::Transform) is the script host's job - a
/// component is data the inspector edits and the serializer walks, and nothing
/// more (ADR 0016).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ScriptComponent {
    /// The `.cmt` source file to run (a project-relative path). Empty means the
    /// component exists but has no script yet, which is what "Add Script" leaves
    /// behind until a file is picked.
    pub source: String,
}

impl Reflect for ScriptComponent {
    fn type_name(&self) -> &'static str {
        "Script"
    }

    fn field_names(&self) -> &'static [&'static str] {
        &["source"]
    }

    fn get(&self, field: &str) -> Option<Value> {
        match field {
            // An Asset, not a Str: a script is a file in the project like a
            // texture is, so the inspector gives it the same path field and the
            // same drop target for free.
            "source" => Some(Value::Asset(self.source.clone())),
            _ => None,
        }
    }

    fn set(&mut self, field: &str, value: Value) -> bool {
        match (field, value) {
            ("source", Value::Asset(s)) => {
                self.source = s;
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_reflects_its_source_path() {
        let mut script = ScriptComponent::default();
        assert_eq!(script.type_name(), "Script");
        assert_eq!(script.field_names(), &["source"]);
        assert_eq!(script.get("source"), Some(Value::Asset(String::new())));

        assert!(script.set("source", Value::Asset("scripts/bounce.cmt".into())));
        assert_eq!(
            script.get("source"),
            Some(Value::Asset("scripts/bounce.cmt".into()))
        );

        assert!(!script.set("source", Value::Str("not an asset".into())));
        assert!(!script.set("nope", Value::Bool(true)));
    }

    #[test]
    fn both_component_kinds_round_trip_through_their_type_name() {
        // What deserialization relies on: a saved kind tag reconstructs the
        // right variant, and an unknown one is rejected rather than guessed.
        for kind in ["Sprite", "Script"] {
            let component = Component::from_type_name(kind).expect("a known kind");
            assert_eq!(component.as_reflect().type_name(), kind);
        }
        assert!(Component::from_type_name("Camera").is_none());
    }

    #[test]
    fn sprite_reflects_its_fields() {
        let mut sprite = SpriteComponent::default();
        assert_eq!(sprite.type_name(), "Sprite");
        assert_eq!(sprite.field_names(), &["texture", "tint", "size"]);
        assert_eq!(sprite.get("tint"), Some(Value::Color([1.0, 1.0, 1.0, 1.0])));
        assert_eq!(sprite.get("missing"), None);

        // Setting through reflection round-trips through get.
        assert!(sprite.set("size", Value::Vec2(Vec2::new(32.0, 48.0))));
        assert_eq!(sprite.get("size"), Some(Value::Vec2(Vec2::new(32.0, 48.0))));

        // A type mismatch and an unknown field are both rejected.
        assert!(!sprite.set("size", Value::F32(5.0)));
        assert!(!sprite.set("nope", Value::Bool(true)));
    }

    #[test]
    fn component_dispatches_reflection_to_its_variant() {
        let component = Component::Sprite(SpriteComponent::default());
        assert_eq!(component.as_reflect().type_name(), "Sprite");
    }
}
