//! aurora input: pointer and keyboard input in Aurora's own vocabulary, so the
//! framework stays windowing-agnostic. A backend (or app) translates winit - or
//! any - events into these and feeds them to
//! [`Ui::handle_input`](crate::Ui::handle_input).

use glam::Vec2;

/// A single input event. Aurora tracks only the primary pointer button; button
/// state is implicit, at the last moved position. Keyboard events act on the
/// focused widget (a text input).
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
    /// A character was typed. The backend should filter out control characters;
    /// the UI ignores them defensively too.
    Text(char),
    /// A named editing key was pressed.
    Key(Key),
}

/// A named key that drives text editing (as opposed to a typed character).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// Delete the character before the caret.
    Backspace,
    /// Delete the character after the caret.
    Delete,
    /// Move the caret one character left.
    Left,
    /// Move the caret one character right.
    Right,
    /// Move the caret to the start of the text.
    Home,
    /// Move the caret to the end of the text.
    End,
    /// Submit the focused field's current text (e.g. commit an edit).
    Enter,
}

/// What the mouse cursor should look like over the current hover target. The
/// app maps this to its windowing system's cursor (Aurora never names one).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorHint {
    /// The ordinary arrow.
    #[default]
    Default,
    /// A text beam (over editable text).
    Text,
    /// Horizontal resize arrows (over a vertical splitter, dragged left-right).
    ResizeHorizontal,
    /// Vertical resize arrows (over a horizontal splitter, dragged up-down).
    ResizeVertical,
}
