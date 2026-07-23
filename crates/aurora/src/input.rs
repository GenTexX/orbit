//! aurora input: pointer input in Aurora's own vocabulary, so the framework
//! stays windowing-agnostic. A backend (or app) translates winit - or any -
//! events into these and feeds them to [`Ui::handle_input`](crate::Ui::handle_input).

use glam::Vec2;

/// A single pointer input event. Aurora tracks only the primary button for now
/// (all Milestone 2 needs); button state is implicit, at the last moved position.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InputEvent {
    /// The pointer moved to `position`, in physical pixels (UI space, the same
    /// coordinates layout produces).
    PointerMoved(Vec2),
    /// The pointer left the surface; nothing is hovered any more.
    PointerLeft,
    /// The primary pointer button went down (at the last moved position).
    PointerPressed,
    /// The primary pointer button came up.
    PointerReleased,
}
