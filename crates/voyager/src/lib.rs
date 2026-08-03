//! voyager - loads a Game Package and plays it with the Engine; linked by the
//! Editor for in-process Play (ADR 0002).
//!
//! [`Runtime`] is the whole crate: it owns a script host, the instances running
//! for a scene's nodes, and the input they read, and it steps them a frame at a
//! time. It opens no window and touches no GPU, which is what makes the loop
//! testable headlessly and what makes the editor's Play and a shipped game the
//! same code.

mod runtime;

pub use runtime::Runtime;
