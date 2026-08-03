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
    /// The view the game is played through, centred on the owning node.
    Camera(CameraComponent),
}

impl Component {
    /// This component's data as a reflectable value (for the inspector and the
    /// serializer, which never name the concrete component type).
    pub fn as_reflect(&self) -> &dyn Reflect {
        match self {
            Component::Sprite(s) => s,
            Component::Script(s) => s,
            Component::Camera(c) => c,
        }
    }

    /// This component's data as a mutable reflectable value.
    pub fn as_reflect_mut(&mut self) -> &mut dyn Reflect {
        match self {
            Component::Sprite(s) => s,
            Component::Script(s) => s,
            Component::Camera(c) => c,
        }
    }

    /// Construct a default component of the given [`type_name`](Reflect::type_name),
    /// or `None` if the kind is unknown. Deserialization uses this, then applies
    /// the saved fields through [`Reflect::set`].
    /// Every kind a component can be, with a sentence about what it does.
    ///
    /// The list a picker offers, and the list `from_type_name` accepts, from
    /// one place - so a fourth component kind cannot ship with a UI that does
    /// not know about it. A test asserts the two agree.
    pub const KINDS: &'static [(&'static str, &'static str)] = &[
        ("Sprite", "Draws a texture at this node."),
        ("Script", "Runs a Comet script for this node."),
        ("Camera", "The view the game is played through."),
    ];

    pub fn from_type_name(kind: &str) -> Option<Component> {
        match kind {
            "Sprite" => Some(Component::Sprite(SpriteComponent::default())),
            "Script" => Some(Component::Script(ScriptComponent::default())),
            "Camera" => Some(Component::Camera(CameraComponent::default())),
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

/// The view a game is played through: where the viewport is centred, and how
/// much of the world it shows.
///
/// The position comes from the owning node's *world* transform rather than from
/// a field here, which is the whole point of it being a component: parent a
/// camera to the player and it follows, with no code and no script.
#[derive(Debug, Clone, PartialEq)]
pub struct CameraComponent {
    /// Whether this is the camera to look through.
    ///
    /// A scene may hold several - one per level section, one for a cutscene -
    /// and only one can be looked through at a time. Turning one off is how you
    /// switch, and it is a field rather than a scene-level setting so it can be
    /// saved with the node it belongs to.
    pub active: bool,
    /// How much the view is magnified. Above 1 shows less of the world, larger.
    pub zoom: f32,
}

impl Default for CameraComponent {
    fn default() -> Self {
        Self {
            active: true,
            zoom: 1.0,
        }
    }
}

impl CameraComponent {
    /// The smallest zoom that can be set.
    ///
    /// Not zero, and not merely because dividing by it is undefined: a zoom of
    /// zero asks to show an infinite amount of world, and the projection that
    /// comes out of it is degenerate rather than empty. The floor is the same
    /// kind of guard the gizmo puts on a node's scale.
    pub const MIN_ZOOM: f32 = 0.01;

    /// The view this camera gives of a `viewport`-sized render target, with
    /// `center` (the owning node's world position) in the middle of it.
    ///
    /// Centred, unlike the editor's own camera, which anchors at the top left
    /// because that is what a pannable canvas wants. A game camera is a thing
    /// that follows something, and what it follows belongs in the middle.
    pub fn view(&self, center: Vec2, viewport: Vec2) -> photon::Camera {
        let extent = viewport / self.zoom.max(Self::MIN_ZOOM);
        photon::Camera::new(center - extent * 0.5, extent)
    }
}

impl Reflect for CameraComponent {
    fn type_name(&self) -> &'static str {
        "Camera"
    }

    fn field_names(&self) -> Vec<String> {
        ["active", "zoom"].into_iter().map(String::from).collect()
    }

    fn get(&self, field: &str) -> Option<Value> {
        match field {
            "active" => Some(Value::Bool(self.active)),
            "zoom" => Some(Value::F32(self.zoom)),
            _ => None,
        }
    }

    fn set(&mut self, field: &str, value: Value) -> bool {
        match (field, value) {
            ("active", Value::Bool(b)) => {
                self.active = b;
                true
            }
            ("zoom", Value::F32(z)) => {
                // Clamped rather than refused: a zero typed into the inspector
                // is a number on its way to being another number, and refusing
                // it mid-keystroke makes the field fight the person using it.
                self.zoom = z.max(Self::MIN_ZOOM);
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
    fn every_component_kind_round_trips_through_its_type_name() {
        // What deserialization relies on: a saved kind tag reconstructs the
        // right variant, and an unknown one is rejected rather than guessed.
        for kind in ["Sprite", "Script", "Camera"] {
            let component = Component::from_type_name(kind).expect("a known kind");
            assert_eq!(component.as_reflect().type_name(), kind);
        }
        assert!(Component::from_type_name("Light").is_none());
    }

    #[test]
    fn a_camera_reflects_its_fields_and_refuses_a_degenerate_zoom() {
        let mut camera = CameraComponent::default();
        assert_eq!(camera.type_name(), "Camera");
        assert_eq!(camera.field_names(), &["active", "zoom"]);
        assert_eq!(camera.get("active"), Some(Value::Bool(true)));
        assert_eq!(camera.get("zoom"), Some(Value::F32(1.0)));

        assert!(camera.set("zoom", Value::F32(2.5)));
        assert_eq!(camera.get("zoom"), Some(Value::F32(2.5)));

        // A zoom of zero asks to show an infinite amount of world. Clamped
        // rather than refused, so a field being typed into does not fight back.
        assert!(camera.set("zoom", Value::F32(0.0)));
        assert_eq!(
            camera.get("zoom"),
            Some(Value::F32(CameraComponent::MIN_ZOOM))
        );
        assert!(camera.set("zoom", Value::F32(-4.0)));
        assert_eq!(
            camera.get("zoom"),
            Some(Value::F32(CameraComponent::MIN_ZOOM))
        );

        assert!(
            !camera.set("zoom", Value::Bool(true)),
            "and a zoom is a number"
        );
    }

    #[test]
    fn a_cameras_view_is_centred_on_the_node_and_scaled_by_its_zoom() {
        // The editor's own camera anchors at the top left, because that is what
        // a pannable canvas wants. A game camera follows something, and what it
        // follows belongs in the middle - so these two must not be confused.
        let camera = CameraComponent::default();
        let view = camera.view(Vec2::new(100.0, 50.0), Vec2::new(800.0, 600.0));
        assert_eq!(view.viewport, Vec2::new(800.0, 600.0));
        assert_eq!(
            view.position,
            Vec2::new(-300.0, -250.0),
            "the node is in the middle of what the view shows"
        );

        let closer = CameraComponent {
            zoom: 2.0,
            ..CameraComponent::default()
        };
        let view = closer.view(Vec2::new(100.0, 50.0), Vec2::new(800.0, 600.0));
        assert_eq!(
            view.viewport,
            Vec2::new(400.0, 300.0),
            "zooming in shows less world"
        );
        assert_eq!(
            view.position,
            Vec2::new(-100.0, -100.0),
            "and still centred on the node"
        );
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

    #[test]
    fn reconcile_is_derived_state_and_says_the_same_thing_twice() {
        // The editor runs this outside the undo history and outside its
        // modified flag, which is only defensible if what it writes is a
        // function of the file on disk. Running it again must therefore change
        // nothing - otherwise "losing it costs nothing" would be false.
        let declared = [
            ("speed".to_string(), Value::F32(0.0)),
            ("jump".to_string(), Value::F32(0.0)),
        ];
        let mut script = ScriptComponent {
            source: "player.cmt".to_string(),
            exports: vec![
                ("speed".to_string(), Value::F32(220.0)),
                ("gone".to_string(), Value::F32(9.0)),
            ],
            hints: Vec::new(),
        };
        script.reconcile(&declared);
        let once = script.exports.clone();
        assert_eq!(
            once,
            vec![
                ("speed".to_string(), Value::F32(220.0)),
                ("jump".to_string(), Value::F32(0.0)),
            ],
            "tuning kept, a new field defaulted, and the one that is gone dropped"
        );

        script.reconcile(&declared);
        assert_eq!(script.exports, once, "and again changes nothing");

        // The loss that is real and has no undo: `gone` took its 9.0 with it.
        // ADR 0008 says drop what is gone, and keeping orphans would
        // accumulate dead data in every scene file forever - but a misspelled
        // export name costs somebody their tuned number, and that is worth
        // knowing rather than discovering.
        assert!(!script.exports.iter().any(|(name, _)| name == "gone"));
    }

    #[test]
    fn the_kinds_a_picker_offers_are_the_kinds_that_can_be_built() {
        // Two lists that have to agree, and nothing but this checks it: a
        // fourth component kind could otherwise ship with a picker that does
        // not offer it, or a picker offering one that cannot be constructed.
        for (kind, description) in Component::KINDS {
            let built = Component::from_type_name(kind)
                .unwrap_or_else(|| panic!("the picker offers {kind}, which cannot be built"));
            assert_eq!(built.as_reflect().type_name(), *kind);
            assert!(!description.is_empty(), "{kind} says what it does");
        }
        // And the other direction: every kind that can be built is offered.
        for kind in ["Sprite", "Script", "Camera"] {
            assert!(
                Component::KINDS.iter().any(|(name, _)| name == &kind),
                "{kind} can be built and is not offered"
            );
        }
    }
}
