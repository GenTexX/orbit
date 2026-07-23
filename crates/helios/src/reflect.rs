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
    fn field_names(&self) -> &'static [&'static str];

    /// The current value of `field`, or `None` if the name is unknown.
    fn get(&self, field: &str) -> Option<Value>;

    /// Set `field` from `value`. Returns `false` on an unknown name or a value
    /// whose type does not match the field.
    fn set(&mut self, field: &str, value: Value) -> bool;
}
