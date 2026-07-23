//! aurora theme: default colors and metrics for the built-in controls.
//!
//! A seed for a fuller theming pass (Milestone 2, step 6). For now it is just the
//! handful of constants the widgets need to look like controls rather than plain
//! rectangles; hover and pressed states are derived from these at draw time.

use crate::color::Color;

/// Default button fill (used when a button's style sets no background).
pub(crate) const BUTTON: Color = Color::rgb(0.26, 0.28, 0.34);
/// The checkbox box fill.
pub(crate) const CHECKBOX_BOX: Color = Color::rgb(0.20, 0.21, 0.26);
/// The checkmark (inner square) fill when a checkbox is checked.
pub(crate) const CHECKBOX_MARK: Color = Color::rgb(0.40, 0.70, 1.0);
/// The intrinsic side length of a checkbox, in pixels.
pub(crate) const CHECKBOX_SIZE: f32 = 18.0;

/// Text-input field fill.
pub(crate) const FIELD: Color = Color::rgb(0.10, 0.11, 0.14);
/// The accent frame drawn around a focused field.
pub(crate) const FOCUS: Color = Color::rgb(0.30, 0.55, 0.90);
/// The text caret color.
pub(crate) const CARET: Color = Color::rgb(0.90, 0.92, 0.96);
