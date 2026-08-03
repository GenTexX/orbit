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

    fn field_names(&self) -> Vec<String> {
        ["texture", "tint", "size"]
            .into_iter()
            .map(String::from)
            .collect()
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
    /// The script's `@export`ed variables and what this instance holds for
    /// each, in the order the script declares them.
    ///
    /// This is what makes two nodes running one script behave differently, and
    /// it is the value that wins: the script's own initializer is only the
    /// default used when the field is first created or explicitly reverted.
    ///
    /// A `Vec` rather than a map so the inspector shows them in the order they
    /// were written, which is the order the author chose.
    pub exports: Vec<(String, Value)>,
    /// How the script asked for each exported value to be shown, by name.
    ///
    /// Not reflected: these describe a field rather than being one, so the
    /// inspector reads them beside `field_names` and the serializer never sees
    /// them - they come from the source, and the source is what is saved.
    pub hints: Vec<(String, Vec<comet::Hint>)>,
}

impl ScriptComponent {
    /// Bring the stored values into line with what `exports` says the script
    /// now declares: keep what is still there and still the right type, drop
    /// what is gone, and add what is new at its default.
    ///
    /// How the script asked for `field` to be shown.
    pub fn hints_for(&self, field: &str) -> &[comet::Hint] {
        self.hints
            .iter()
            .find(|(name, _)| name == field)
            .map_or(&[], |(_, hints)| hints.as_slice())
    }

    /// Bring the stored values into line with what `declared` says the script
    /// now exports: keep what is still there and still the right type, drop
    /// what is gone, and add what is new at its default.
    ///
    /// Keeping by name and type is what makes an edit to the source preserve
    /// tuning - the migration ADR 0008 describes, in the one place a script's
    /// fields can change without the component being touched.
    pub fn reconcile(&mut self, declared: &[(String, Value)]) {
        let mut next = Vec::with_capacity(declared.len());
        for (name, default) in declared {
            let kept = self
                .exports
                .iter()
                .find(|(existing, value)| {
                    existing == name
                        && std::mem::discriminant(value) == std::mem::discriminant(default)
                })
                .map(|(_, value)| value.clone());
            next.push((name.clone(), kept.unwrap_or_else(|| default.clone())));
        }
        self.exports = next;
    }
}

impl Reflect for ScriptComponent {
    fn type_name(&self) -> &'static str {
        "Script"
    }

    fn field_names(&self) -> Vec<String> {
        // The path, then whatever the script exports. The inspector and the
        // serializer both walk this, so an exported variable becomes an
        // editable, saved field without either of them being told.
        std::iter::once("source".to_string())
            .chain(self.exports.iter().map(|(name, _)| name.clone()))
            .collect()
    }

    fn get(&self, field: &str) -> Option<Value> {
        match field {
            // An Asset, not a Str: a script is a file in the project like a
            // texture is, so the inspector gives it the same path field and the
            // same drop target for free.
            "source" => Some(Value::Asset(self.source.clone())),
            _ => self
                .exports
                .iter()
                .find(|(name, _)| name == field)
                .map(|(_, value)| value.clone()),
        }
    }

    fn set(&mut self, field: &str, value: Value) -> bool {
        // `source` is a real field of a fixed type, so a mismatch is refused
        // rather than falling through to the declare-on-write path below - a
        // script does not get to export a variable called `source`.
        if field == "source" {
            let Value::Asset(s) = &value else {
                return false;
            };
            self.source = s.clone();
            return true;
        }
        // An exported value, and only if the type still matches - a stale
        // stored value from an older version of the script must not be written
        // back into a field that has since changed type.
        for (name, held) in &mut self.exports {
            if name == field {
                if std::mem::discriminant(held) != std::mem::discriminant(&value) {
                    return false;
                }
                *held = value;
                return true;
            }
        }
        // A name this component has never held is *declared* by writing it.
        //
        // Every other component has a fixed field set, so `set` on an unknown
        // name is a mistake; this one's fields are whatever its script exports,
        // and the serializer rebuilds a component from `from_type_name` - an
        // empty one - before replaying what it read. Refusing here meant every
        // tuned value in a scene file was silently dropped at load and the
        // field came back at its type default, which is the opposite of what
        // ADR 0022 promises. Nothing bogus survives: `reconcile` runs against
        // the source afterwards and drops any name the script does not declare.
        self.exports.push((field.to_string(), value));
        true
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

        // `source` has a fixed type, so a mismatch is refused.
        assert!(!script.set("source", Value::Str("not an asset".into())));
        assert_eq!(
            script.get("source"),
            Some(Value::Asset("scripts/bounce.cmt".into()))
        );
    }

    #[test]
    fn an_exported_name_this_component_never_held_is_declared_by_writing_it() {
        // This is what a load does: the serializer rebuilds an empty component
        // and replays the fields it read. Refusing an unknown name meant every
        // tuned value in a scene file was dropped and came back at its default.
        let mut script = ScriptComponent::default();
        assert!(script.set("speed", Value::F32(240.0)));
        assert_eq!(script.get("speed"), Some(Value::F32(240.0)));
        assert_eq!(script.field_names(), &["source", "speed"]);

        // A second write to a name it now holds still goes through the
        // type guard: same type overwrites, a different one is refused.
        assert!(script.set("speed", Value::F32(12.0)));
        assert!(!script.set("speed", Value::Bool(true)));
        assert_eq!(script.get("speed"), Some(Value::F32(12.0)));

        // And a name the script does not declare does not survive reconcile,
        // which is what keeps a stale scene file from accumulating fields.
        script.reconcile(&[("speed".to_string(), Value::F32(0.0))]);
        assert_eq!(script.field_names(), &["source", "speed"]);
        assert_eq!(script.get("speed"), Some(Value::F32(12.0)), "tuning kept");
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
