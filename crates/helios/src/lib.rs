//! helios - the Engine: scene tree (nodes + components), input, audio, physics, script host. A library; opens no windows.
//!
//! Milestone 3 brings the scene model online: a [`Scene`] is a tree of [`Node`]s
//! (ADR 0003), each with a [`Transform`] and a list of [`Component`]s. Every
//! component exposes its fields through one [`Reflect`] contract (ADR 0016), which
//! the inspector, the serializer, and hot-reload all walk. This core is GPU-free
//! and unit-tested headlessly; scene rendering (via photon) lives in a separate
//! module added later.

mod command;
mod component;
mod error;
mod input;
mod project;
mod reflect;
mod render;
mod scene;
mod script;
mod serialize;
mod transform;

/// Re-exported so callers can name the type helios' own public fields are made
/// of - a `Transform`'s translation, an `Input`'s mouse - without having to
/// guess which version of glam this crate was built against.
pub use glam::Vec2;

pub use command::{Edit, History};
pub use component::{CameraComponent, Component, ScriptComponent, SpriteComponent};
pub use error::HeliosError;
pub use input::Input;
pub use project::{Manifest, Project, write_atomic};
pub use reflect::{Reflect, Value};
pub use render::SpriteDraw;
pub use scene::{Node, NodeId, Scene};
pub use script::{Begin, ScriptError, ScriptHost, ScriptInstance, schema as script_schema};
pub use transform::Transform;
