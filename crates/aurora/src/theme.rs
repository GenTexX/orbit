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
