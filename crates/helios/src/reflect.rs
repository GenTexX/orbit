//! helios reflection: the one contract - enumerate a component's fields - that the
//! inspector, the serializer, and (later) hot-reload all walk (ADR 0016).

use glam::Vec2;
use serde::{Deserialize, Serialize};

/// A reflected field value: the closed set of field types the engine supports.
/// A genuinely new field type is a deliberate addition here (ADR 0016).
///
/// `Value` derives serde so it can be persisted, but that is orthogonal to
/// reflection: the inspector still walks fields through [`Reflect`], never
/// through serde's data model (ADR 0016).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Value {
    F32(f32),
    /// A whole number: comet's `int`, and a payload-free enum's tag.
    ///
    /// Added because pretending it was an `f32` was not free. atlas mapped
    /// comet's `Int` onto `F32`, and the script host then derived the wasm type
    /// from the `Value` it happened to hold - so it wrote an f32 into the
    /// module's i32 global, wasmtime refused it, and the error was discarded.
    /// An `@export let lives: int;` tuned to 5 in the inspector ran at 0. Two
    /// layers each working around a missing variant, which is exactly what a
    /// closed set is meant to prevent (ADR 0016).
    Int(i32),
    Bool(bool),
    Str(String),
    Vec2(Vec2),
    /// An RGBA color, each component in `0.0..=1.0`.
    Color([f32; 4]),
    /// A reference to a project asset (a project-relative path, for now).
    Asset(String),
}

/// A component's editable fields, exposed uniformly.
///
/// Every component implements this by hand - no derive macro yet (ADR 0016).
/// The inspector reads and writes fields through it, and serialization walks it,
/// so what is edited, saved, and reloaded is one and the same set of fields.
pub trait Reflect {
    /// The component's stable type name - a serialization tag and the inspector
    /// header (e.g. `"Sprite"`).
    fn type_name(&self) -> &'static str;

    /// This component's field names, in a stable order.
    ///
    /// Owned rather than `&'static`, because a component's fields can depend on
    /// something outside the type: a `Script`'s are the variables its source
    /// file marks `@export`. Special-casing that in the inspector and the
    /// serializer would make ADR 0016's actual claim - that one contract serves
    /// all three consumers with no component hardcoded - untrue.
    fn field_names(&self) -> Vec<String>;

    /// The current value of `field`, or `None` if the name is unknown.
    fn get(&self, field: &str) -> Option<Value>;

    /// Set `field` from `value`. Returns `false` on an unknown name or a value
    /// whose type does not match the field.
    fn set(&mut self, field: &str, value: Value) -> bool;
}
