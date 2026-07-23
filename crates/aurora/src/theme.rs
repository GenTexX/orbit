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

/// Default splitter bar fill (used when a splitter's style sets no background).
pub(crate) const SPLITTER: Color = Color::rgb(0.18, 0.19, 0.23);
/// The smallest a splitter will resize its target down to, in pixels.
pub(crate) const SPLITTER_MIN_TARGET: f32 = 40.0;
/// Extra grab zone per side beyond the splitter's visual bar, in pixels - the
/// bar is thin, the hit area should not be.
pub(crate) const SPLITTER_GRAB_EXTRA: f32 = 4.0;

/// Scrollbar thumb width, its inset from the right edge, and its minimum
/// length, in pixels.
pub(crate) const SCROLLBAR_WIDTH: f32 = 6.0;
pub(crate) const SCROLLBAR_INSET: f32 = 2.0;
pub(crate) const SCROLLBAR_MIN_THUMB: f32 = 24.0;
/// The scrollbar thumb fill (drawn only while content overflows).
pub(crate) const SCROLLBAR_THUMB: Color = Color::rgba(0.75, 0.78, 0.85, 0.45);
