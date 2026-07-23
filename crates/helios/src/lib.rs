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
mod project;
mod reflect;
mod render;
mod scene;
mod serialize;
mod transform;

pub use command::{Edit, History};
pub use component::{Component, SpriteComponent};
pub use error::HeliosError;
pub use project::{Manifest, Project};
pub use reflect::{Reflect, Value};
pub use scene::{Node, NodeId, Scene};
pub use transform::Transform;
