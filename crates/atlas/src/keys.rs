//! The keyboard as a game sees it: which keys are down, and what they mean.
//!
//! A second path out of winit, deliberately. Game input cannot route through
//! aurora's focus model: aurora's `InputEvent` has no release at all, and its
//! `Key` enum is twenty-two text-editing verbs - "move the caret a word left",
//! not "the A key is down". Those are different questions and one type cannot
//! answer both.

use std::collections::HashSet;

use glam::Vec2;
use helios::Input;
use winit::keyboard::KeyCode;

/// Which physical keys each action is bound to.
///
/// Physical (`KeyCode`) rather than logical, which is the whole reason winit
/// reports both. WASD is a shape under three fingers; on an AZERTY keyboard the
/// logical letters under those fingers are QZSD, and a character controller that
/// used them would be unplayable on half the keyboards in Europe. A text field
/// wants the opposite and gets it from the logical key.
///
/// A fixed set of bindings rather than a configurable action map. Naming your
/// own actions and binding them is a real feature; it is not this one.
const BINDINGS: &[(&str, &[KeyCode])] = &[
    ("left", &[KeyCode::ArrowLeft, KeyCode::KeyA]),
    ("right", &[KeyCode::ArrowRight, KeyCode::KeyD]),
    ("up", &[KeyCode::ArrowUp, KeyCode::KeyW]),
    ("down", &[KeyCode::ArrowDown, KeyCode::KeyS]),
    ("jump", &[KeyCode::Space]),
];

/// Every action and the keys bound to it, for the generated manual.
#[cfg(test)]
pub fn bindings() -> Vec<(&'static str, String)> {
    BINDINGS
        .iter()
        .map(|(action, codes)| {
            let keys: Vec<String> = codes.iter().map(|code| format!("{code:?}")).collect();
            (*action, keys.join(" or "))
        })
        .collect()
}

/// The keys currently held down.
///
/// The held set is the truth and [`Input`] is derived from it, rather than the
/// two being kept in step. Two keys can mean one action - the arrows and WASD
/// both do - so a flag flipped on press and off on release would clear `left`
/// when you let go of A while still holding the left arrow.
#[derive(Debug, Default)]
pub struct GameKeys {
    held: HashSet<KeyCode>,
}

impl GameKeys {
    /// Take a key event from winit.
    ///
    /// `taking_text` is whether something on screen is currently accepting
    /// typing - a focused field, an open dialog. A press is ignored while it is,
    /// because typing "a" into a rename box must not also walk the player left.
    ///
    /// A **release always lands**, whatever has focus now, and that asymmetry is
    /// the point of this function. Hold A, click into a text field, let go: the
    /// press was seen and the release was not, so without this the player walks
    /// left until the end of the session. The same reasoning is why a lost
    /// window needs [`release_all`](Self::release_all) - that release is
    /// delivered to whoever has focus now, which is not us.
    ///
    /// A `repeat` is the keyboard re-reporting a key that never came up. Held is
    /// held, so it changes nothing here; it is ignored explicitly so that a
    /// later edge-triggered reading of this state does not fire ten times a
    /// second for one press.
    pub fn observe(&mut self, code: KeyCode, pressed: bool, repeat: bool, taking_text: bool) {
        if repeat {
            return;
        }
        match pressed {
            true if !taking_text => {
                self.held.insert(code);
            }
            true => {}
            false => {
                self.held.remove(&code);
            }
        }
    }

    /// Forget every held key - what a window does when it loses focus.
    pub fn release_all(&mut self) {
        self.held.clear();
    }

    /// What a script reads this frame, with the pointer at `mouse` in world
    /// coordinates.
    pub fn input(&self, mouse: Vec2) -> Input {
        let held = |action: &str| {
            BINDINGS
                .iter()
                .find(|(name, _)| *name == action)
                .is_some_and(|(_, codes)| codes.iter().any(|code| self.held.contains(code)))
        };
        Input {
            left: held("left"),
            right: held("right"),
            up: held("up"),
            down: held("down"),
            jump: held("jump"),
            mouse,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_keys_can_mean_one_action() {
        // The arrows and WASD are both bound to the same five actions, so the
        // held set has to be the truth: a flag flipped on press and off on
        // release would clear `left` when you let go of one of them.
        let mut keys = GameKeys::default();
        keys.observe(KeyCode::KeyA, true, false, false);
        keys.observe(KeyCode::ArrowLeft, true, false, false);
        assert!(keys.input(Vec2::ZERO).left);

        keys.observe(KeyCode::KeyA, false, false, false);
        assert!(keys.input(Vec2::ZERO).left, "the left arrow is still down");
        keys.observe(KeyCode::ArrowLeft, false, false, false);
        assert!(!keys.input(Vec2::ZERO).left);
    }

    #[test]
    fn a_release_lands_even_when_something_is_taking_text() {
        // Hold A, click into a rename box, let go. The press was seen and the
        // release was not, so a symmetric guard leaves the player walking left
        // until the end of the session.
        let mut keys = GameKeys::default();
        keys.observe(KeyCode::KeyA, true, false, false);
        assert!(keys.input(Vec2::ZERO).left);

        keys.observe(KeyCode::KeyA, false, false, true);
        assert!(!keys.input(Vec2::ZERO).left, "the release still landed");
    }

    #[test]
    fn typing_does_not_drive_the_game() {
        let mut keys = GameKeys::default();
        keys.observe(KeyCode::KeyD, true, false, true);
        keys.observe(KeyCode::Space, true, false, true);
        let input = keys.input(Vec2::ZERO);
        assert!(!input.right, "a `d` typed into a field is a letter");
        assert!(!input.jump);
    }

    #[test]
    fn a_repeat_is_not_a_second_press() {
        // Held is held. This changes nothing about the flags today; it is
        // ignored explicitly so an edge-triggered reading later does not fire
        // ten times a second for one press.
        let mut keys = GameKeys::default();
        keys.observe(KeyCode::Space, true, false, false);
        keys.observe(KeyCode::Space, true, true, false);
        assert!(keys.input(Vec2::ZERO).jump);

        // And a repeat cannot sneak a key in past the typing guard.
        let mut keys = GameKeys::default();
        keys.observe(KeyCode::Space, true, true, false);
        assert!(!keys.input(Vec2::ZERO).jump);
    }

    #[test]
    fn losing_the_window_releases_everything() {
        // The release is delivered to whoever has focus now, which is not us.
        let mut keys = GameKeys::default();
        for code in [KeyCode::KeyW, KeyCode::Space, KeyCode::ArrowRight] {
            keys.observe(code, true, false, false);
        }
        keys.release_all();
        let input = keys.input(Vec2::ZERO);
        assert_eq!(input, Input::default());
    }

    #[test]
    fn every_action_the_input_carries_is_bound_to_something() {
        // The five bools on `Input` and the five rows in BINDINGS are two lists
        // that have to agree, and nothing but this checks that they do: a
        // renamed field would leave `input` reading a name no row has, which
        // reports as "never held" rather than as an error.
        let mut keys = GameKeys::default();
        for (_, codes) in BINDINGS {
            for code in *codes {
                keys.observe(*code, true, false, false);
            }
        }
        let input = keys.input(Vec2::new(1.0, 2.0));
        assert_eq!(
            input,
            Input {
                left: true,
                right: true,
                up: true,
                down: true,
                jump: true,
                mouse: Vec2::new(1.0, 2.0),
            }
        );
    }
}
