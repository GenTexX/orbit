//! What a running script can ask about the keyboard and the mouse.

use glam::Vec2;

/// The input a script sees, as of the frame it is being stepped in.
///
/// **Polled state, not events.** `left` is true for as long as the key is held,
/// rather than there being an `on_key_down` to define. That is what a character
/// controller wants - "am I being pushed left this frame", not "a key went down
/// at some point" - and it is what a fixed schema can express: an event surface
/// would need a callback mechanism comet does not have.
///
/// **A fixed set of names, not an action map.** Letting a project name its own
/// actions and bind them is a real feature; it is not this one. These six are
/// what a one-screen platformer needs, which is Milestone 5's scope filter.
///
/// Nothing reads this yet. The rows in the host schema and the accessor arms
/// that serve them are step 12 of the milestone, and the winit seam that fills
/// it in is step 11; this is the type both of them agree on, owned by the crate
/// the accessors live in.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Input {
    /// Held this frame, whichever key or stick the host decided means "left".
    pub left: bool,
    pub right: bool,
    pub up: bool,
    pub down: bool,
    /// The one action a platformer cannot be written without.
    pub jump: bool,
    /// Where the pointer is, in the same world coordinates a transform uses -
    /// so a script can compare it against its own position without knowing
    /// anything about windows, scale factors or viewport rectangles.
    pub mouse: Vec2,
}

impl Input {
    /// Forget every held key, keeping the pointer position.
    ///
    /// What a host does when the window loses focus. Without it, alt-tabbing
    /// mid-jump leaves the key held forever: the release arrives at whatever
    /// has focus now, which is not us, so the run never ends.
    pub fn release_all(&mut self) {
        let mouse = self.mouse;
        *self = Input {
            mouse,
            ..Input::default()
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn losing_focus_releases_every_key_but_keeps_the_pointer() {
        let mut input = Input {
            left: true,
            jump: true,
            mouse: Vec2::new(3.0, 4.0),
            ..Input::default()
        };
        input.release_all();
        assert!(!input.left, "a key held across a focus loss never releases");
        assert!(!input.jump);
        assert_eq!(
            input.mouse,
            Vec2::new(3.0, 4.0),
            "the pointer is a position, not a press - it has nothing to release"
        );
    }
}
