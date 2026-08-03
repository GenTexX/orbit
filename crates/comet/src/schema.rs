//! The host surface a script is compiled against.
//!
//! comet does not know what a `Transform` is, or that `Sprite` exists. The
//! engine describes what a script can reach - named groups of typed properties -
//! and comet compiles against that description (ADR 0002's separation, applied
//! to the language). Adding a scriptable property becomes a change to the
//! description rather than to the compiler, and the language service gets
//! completions for it without being told separately.
//!
//! Before this, `pos` was a magic identifier with dedicated variants in the
//! checker, the typed IR and codegen. Every new property would have cost another
//! set of special cases in all three.

use crate::tir::Type;

/// The types a host property can have.
///
/// Deliberately narrower than [`Type`]: it is exactly what codegen can emit an
/// accessor for, so a schema cannot describe something the compiler would then
/// have to refuse. `String` is absent because a host property that owns a
/// refcounted value needs an ownership rule across the host boundary, which is
/// its own decision and not this one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostType {
    F32,
    Bool,
    Vec2,
}

impl HostType {
    /// The language type a property of this kind has.
    pub fn ty(self) -> Type {
        match self {
            HostType::F32 => Type::F32,
            HostType::Bool => Type::Bool,
            HostType::Vec2 => Type::Vec2,
        }
    }
}

/// Whether a script may assign to a property, or only read it.
///
/// Most of what a host will ever expose is read-only - the mouse, elapsed time,
/// a parent's world position - so a schema that can only describe writable
/// scalars can only describe the first few properties honestly. Without this,
/// `input.jump = true` compiles, emits a `set_bool`, and the host drops it on
/// the floor: no diagnostic, no effect, and no way for a learner to find out
/// why. That is the worst shape a gap can take in a teaching tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Access {
    /// A script may read it and assign to it.
    #[default]
    ReadWrite,
    /// The engine sets it; a script may only read it.
    ReadOnly,
}

/// One property: the name a script writes, its type, and whether it can be
/// assigned to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostField {
    pub name: String,
    pub ty: HostType,
    pub access: Access,
}

/// A writable property, which is the common case and the one every existing
/// caller declares.
impl From<(&'static str, HostType)> for HostField {
    fn from((name, ty): (&'static str, HostType)) -> Self {
        HostField {
            name: name.to_string(),
            ty,
            access: Access::ReadWrite,
        }
    }
}

impl From<(&'static str, HostType, Access)> for HostField {
    fn from((name, ty, access): (&'static str, HostType, Access)) -> Self {
        HostField {
            name: name.to_string(),
            ty,
            access,
        }
    }
}

/// A named group of properties - `transform`, or a component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostObject {
    pub name: String,
    pub fields: Vec<HostField>,
}

/// Everything a script can reach outside itself.
///
/// Properties are numbered in declaration order, flat across objects, and that
/// number is what codegen passes to the host accessors. The host builds the same
/// schema and so agrees on the numbering without either side hardcoding it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostSchema {
    objects: Vec<HostObject>,
}

/// Where a resolved property lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldRef {
    /// The property's index, flat across the whole schema. What codegen emits.
    pub id: u32,
    pub ty: HostType,
    pub access: Access,
}

impl HostSchema {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an object and its properties, in order. A repeated object name is
    /// ignored rather than merged: the engine builds this from one place, so a
    /// duplicate is a mistake, and silently merging would hide it.
    pub fn object(
        mut self,
        name: impl Into<String>,
        fields: impl IntoIterator<Item = impl Into<HostField>>,
    ) -> Self {
        let name = name.into();
        if self.objects.iter().any(|o| o.name == name) {
            return self;
        }
        self.objects.push(HostObject {
            name,
            fields: fields.into_iter().map(Into::into).collect(),
        });
        self
    }

    /// Whether `name` is one of the objects a script can reach.
    pub fn has_object(&self, name: &str) -> bool {
        self.objects.iter().any(|o| o.name == name)
    }

    /// The names of every object, for completions and diagnostics.
    pub fn object_names(&self) -> impl Iterator<Item = &str> {
        self.objects.iter().map(|o| o.name.as_str())
    }

    /// The properties of `object`, or an empty slice if there is no such object.
    pub fn fields_of(&self, object: &str) -> &[HostField] {
        self.objects
            .iter()
            .find(|o| o.name == object)
            .map_or(&[], |o| &o.fields)
    }

    /// Resolve `object.field` to the number codegen emits and the type it has.
    pub fn resolve(&self, object: &str, field: &str) -> Option<FieldRef> {
        let mut id = 0u32;
        for candidate in &self.objects {
            for property in &candidate.fields {
                if candidate.name == object && property.name == field {
                    return Some(FieldRef {
                        id,
                        ty: property.ty,
                        access: property.access,
                    });
                }
                id += 1;
            }
        }
        None
    }

    /// Every property in numbering order, as `(object, field, type)`. What a
    /// host walks to build its accessor table, so both sides derive the same
    /// numbering from the same declaration order.
    pub fn properties(&self) -> impl Iterator<Item = (&str, &str, HostType)> {
        self.objects.iter().flat_map(|object| {
            object
                .fields
                .iter()
                .map(move |field| (object.name.as_str(), field.name.as_str(), field.ty))
        })
    }
}

/// The schema comet's own tests and its example runner compile against.
///
/// Deliberately *not* the engine's schema - helios builds that from what it
/// actually has, and comet naming `Transform` is exactly what decision 2 exists
/// to prevent. This is a fixture that happens to have the same shape, so a test
/// written against it exercises the same paths a real script does. It lives here
/// rather than in each test module so four copies cannot drift apart.
pub fn example_schema() -> HostSchema {
    HostSchema::new().object(
        "transform",
        [
            ("position", HostType::Vec2),
            ("rotation", HostType::F32),
            ("scale", HostType::Vec2),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> HostSchema {
        HostSchema::new()
            .object(
                "transform",
                [
                    ("position", HostType::Vec2),
                    ("rotation", HostType::F32),
                    ("scale", HostType::Vec2),
                ],
            )
            .object("sprite", [("visible", HostType::Bool)])
    }

    #[test]
    fn properties_are_numbered_flat_across_objects_in_declaration_order() {
        // The host walks the same schema to build its accessor table, so the
        // numbering has to come from the declaration order and nothing else.
        let schema = schema();
        assert_eq!(schema.resolve("transform", "position").unwrap().id, 0);
        assert_eq!(schema.resolve("transform", "rotation").unwrap().id, 1);
        assert_eq!(schema.resolve("transform", "scale").unwrap().id, 2);
        assert_eq!(schema.resolve("sprite", "visible").unwrap().id, 3);

        let listed: Vec<&str> = schema.properties().map(|(_, field, _)| field).collect();
        assert_eq!(listed, ["position", "rotation", "scale", "visible"]);
    }

    #[test]
    fn an_unknown_object_or_field_resolves_to_nothing() {
        let schema = schema();
        assert_eq!(schema.resolve("transform", "colour"), None);
        assert_eq!(schema.resolve("audio", "volume"), None);
        // And a field belonging to another object does not leak across.
        assert_eq!(schema.resolve("sprite", "rotation"), None);
    }

    #[test]
    fn a_repeated_object_is_ignored_rather_than_merged() {
        let schema = schema().object("transform", [("bogus", HostType::F32)]);
        assert_eq!(schema.resolve("transform", "bogus"), None);
        assert_eq!(schema.fields_of("transform").len(), 3);
    }
}
