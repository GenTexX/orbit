//! aurora theme: the colors the built-in controls draw with, as a swappable
//! [`Theme`], plus the fixed geometric metrics (sizes, insets).
//!
//! Colors live in `Theme` so a whole UI can be recolored at once with
//! [`Ui::set_theme`](crate::Ui::set_theme); a widget still overrides individual
//! colors locally through its [`Style`](crate::Style) (background, foreground,
//! border). Hover and pressed shades are derived from these at draw time. The
//! geometric metrics below are layout, not color, so they stay constants.

use crate::color::Color;

/// The color palette the built-in widgets draw with. Swap it globally with
/// [`Ui::set_theme`](crate::Ui::set_theme); [`Theme::dark`] is the default and
/// [`Theme::light`] a bundled alternative. Per-widget overrides still come from
/// each widget's [`Style`](crate::Style).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    /// Button fill (when a button's style sets no background).
    pub button: Color,
    /// Hover/press overlays for flat (list/tree) rows.
    pub row_hover: Color,
    pub row_pressed: Color,
    /// Checkbox box fill and the check-glyph color when checked.
    pub checkbox_box: Color,
    pub checkbox_mark: Color,
    /// Text-input field fill.
    pub field: Color,
    /// Accent for a focused field's border, the slider fill, and selection.
    pub focus: Color,
    /// Text caret color.
    pub caret: Color,
    /// Placeholder text shown, dimmed, in an empty field.
    pub placeholder: Color,
    /// Color an icon-button's icon takes on hover/press.
    pub icon_hover: Color,
    /// Text-selection highlight (drawn behind the selected glyphs).
    pub selection: Color,
    /// Slider track (unfilled) and its filled portion / thumb.
    pub slider_track: Color,
    pub slider_fill: Color,
    /// Splitter bar fill (when its style sets no background).
    pub splitter: Color,
    /// Scrollbar thumb fill (drawn only while content overflows).
    pub scrollbar_thumb: Color,
    /// Alpha multiplier applied to a disabled widget's fills and text, fading
    /// the whole subtree toward the background so it reads as unavailable.
    pub disabled_fade: f32,
}

impl Theme {
    /// The default dark theme.
    pub const fn dark() -> Self {
        Self {
            button: Color::rgb(0.26, 0.28, 0.34),
            row_hover: Color::rgba(1.0, 1.0, 1.0, 0.06),
            row_pressed: Color::rgba(1.0, 1.0, 1.0, 0.11),
            checkbox_box: Color::rgb(0.20, 0.21, 0.26),
            checkbox_mark: Color::rgb(0.40, 0.70, 1.0),
            field: Color::rgb(0.10, 0.11, 0.14),
            focus: Color::rgb(0.30, 0.55, 0.90),
            caret: Color::rgb(0.90, 0.92, 0.96),
            placeholder: Color::rgb(0.45, 0.47, 0.53),
            icon_hover: Color::rgb(0.45, 0.70, 1.0),
            selection: Color::rgba(0.30, 0.55, 0.90, 0.40),
            slider_track: Color::rgb(0.20, 0.21, 0.26),
            slider_fill: Color::rgb(0.30, 0.55, 0.90),
            splitter: Color::rgb(0.18, 0.19, 0.23),
            scrollbar_thumb: Color::rgba(0.75, 0.78, 0.85, 0.45),
            disabled_fade: 0.38,
        }
    }

    /// A bundled light theme (dark text on light surfaces).
    pub const fn light() -> Self {
        Self {
            button: Color::rgb(0.88, 0.89, 0.92),
            row_hover: Color::rgba(0.0, 0.0, 0.0, 0.06),
            row_pressed: Color::rgba(0.0, 0.0, 0.0, 0.12),
            checkbox_box: Color::rgb(0.82, 0.83, 0.87),
            checkbox_mark: Color::rgb(0.15, 0.45, 0.85),
            field: Color::rgb(0.98, 0.98, 1.0),
            focus: Color::rgb(0.20, 0.50, 0.90),
            caret: Color::rgb(0.10, 0.12, 0.16),
            placeholder: Color::rgb(0.55, 0.57, 0.62),
            icon_hover: Color::rgb(0.20, 0.50, 0.90),
            selection: Color::rgba(0.20, 0.50, 0.90, 0.30),
            slider_track: Color::rgb(0.80, 0.81, 0.85),
            slider_fill: Color::rgb(0.20, 0.50, 0.90),
            splitter: Color::rgb(0.78, 0.79, 0.83),
            scrollbar_thumb: Color::rgba(0.20, 0.22, 0.28, 0.45),
            disabled_fade: 0.45,
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

/// The intrinsic side length of a checkbox, in pixels.
pub(crate) const CHECKBOX_SIZE: f32 = 18.0;

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
