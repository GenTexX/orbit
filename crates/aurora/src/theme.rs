//! aurora theme: the colors and shape metrics the built-in controls and the
//! composite widgets draw with, as a swappable [`Theme`], plus the fixed
//! geometric metrics (sizes, insets).
//!
//! Values live in `Theme` so a whole UI can be restyled at once with
//! [`Ui::set_theme`](crate::Ui::set_theme); a widget still overrides individual
//! colors locally through its [`Style`](crate::Style) (background, foreground,
//! border). Hover and pressed shades are derived from these at draw time. The
//! geometric metrics at the bottom of this file are layout, not style, so they
//! stay constants.
//!
//! The palette has two halves. **Controls** are the colors a single built-in
//! widget draws itself with (a button's fill, a field's border). **Chrome** is
//! the surrounding surfaces, text, borders, and corner radii that the composite
//! widgets - tabs, cards, menus, dialogs, the color picker - are built from. An
//! app that only uses buttons and fields can ignore the chrome half; one that
//! builds a whole shell will theme all of it.

use crate::color::Color;

/// The palette the built-in and composite widgets draw with. Swap it globally
/// with [`Ui::set_theme`](crate::Ui::set_theme); [`Theme::dark`] is the default
/// and [`Theme::light`] a bundled alternative. Per-widget overrides still come
/// from each widget's [`Style`](crate::Style).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Theme {
    // --- controls: what one built-in widget paints itself with ---
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

    // --- chrome: the surfaces, text, and borders a shell is built from ---
    /// A panel's background - the main working surface.
    pub panel_bg: Color,
    /// The window background behind the panels, darker than `panel_bg` so the
    /// gaps between panels read as separation.
    pub root_bg: Color,
    /// A bar's background (a toolbar, a tab strip, a breadcrumb bar): a
    /// distinct shade so a bar reads as a strip, not part of the panel body.
    pub bar_bg: Color,
    /// A section header's background (the title bar of a collapsible card).
    pub header_bg: Color,
    /// A card's fill (a collapsible section, a grouped set of fields).
    pub card_bg: Color,
    /// The surface of a floating menu, popup, or dialog.
    pub menu_bg: Color,
    /// The translucent backdrop that dims the UI behind a modal dialog.
    pub scrim: Color,
    /// Primary text.
    pub heading: Color,
    /// Secondary, dimmed text.
    pub subhead: Color,
    /// The general divider color: splitters between panels, and bar underlines.
    pub panel_border: Color,
    /// A card's outline.
    pub card_border: Color,
    /// A text input's resting outline (its focused outline is `focus`).
    pub field_border: Color,
    /// The active tab's outline.
    pub tab_border: Color,
    /// A selected row's fill in a list or tree.
    pub row_selected: Color,
    /// The icon color on an active (toggled) icon button; an inactive one draws
    /// its icon untinted.
    pub icon_active: Color,

    // --- shape: corner radii and line widths, in pixels ---
    /// Corner radius of a popup surface: a menu, the color picker, a dialog.
    pub card_radius: f32,
    /// Corner radius of a collapsible section card.
    pub component_radius: f32,
    /// Corner radius of a tab's top corners.
    pub tab_radius: f32,
    /// Corner radius of a control: a button, a field.
    pub control_radius: f32,
    /// Corner radius of a small inset field (an inline rename box).
    pub inset_radius: f32,
    /// Thickness of a drawn border.
    pub border_width: f32,
    /// Thickness of a draggable splitter between panels.
    pub splitter_width: f32,
}

impl Theme {
    /// The default dark theme.
    pub const fn dark() -> Self {
        Self {
            button: Color::rgb(0.19, 0.21, 0.26),
            row_hover: Color::rgba(1.0, 1.0, 1.0, 0.06),
            row_pressed: Color::rgba(1.0, 1.0, 1.0, 0.11),
            checkbox_box: Color::rgb(0.14, 0.15, 0.19),
            checkbox_mark: Color::rgb(0.40, 0.70, 1.0),
            field: Color::rgb(0.06, 0.065, 0.085),
            focus: Color::rgb(0.30, 0.55, 0.90),
            caret: Color::rgb(0.90, 0.92, 0.96),
            placeholder: Color::rgb(0.42, 0.44, 0.50),
            icon_hover: Color::rgb(0.45, 0.70, 1.0),
            selection: Color::rgba(0.30, 0.55, 0.90, 0.40),
            slider_track: Color::rgb(0.14, 0.15, 0.19),
            slider_fill: Color::rgb(0.30, 0.55, 0.90),
            splitter: Color::rgb(0.12, 0.13, 0.16),
            scrollbar_thumb: Color::rgba(0.75, 0.78, 0.85, 0.45),
            disabled_fade: 0.38,
            panel_bg: Color::rgb(0.095, 0.10, 0.122),
            root_bg: Color::rgb(0.038, 0.04, 0.05),
            bar_bg: Color::rgb(0.062, 0.066, 0.082),
            header_bg: Color::rgb(0.078, 0.082, 0.105),
            card_bg: Color::rgb(0.128, 0.135, 0.165),
            menu_bg: Color::rgb(0.11, 0.12, 0.15),
            scrim: Color::rgba(0.0, 0.0, 0.0, 0.5),
            heading: Color::rgb(0.95, 0.96, 1.0),
            subhead: Color::rgb(0.52, 0.57, 0.68),
            panel_border: Color::rgb(0.24, 0.26, 0.32),
            card_border: Color::rgb(0.20, 0.22, 0.27),
            field_border: Color::rgb(0.24, 0.26, 0.32),
            tab_border: Color::rgb(0.24, 0.26, 0.32),
            row_selected: Color::rgb(0.19, 0.27, 0.42),
            icon_active: Color::WHITE,
            card_radius: 6.0,
            component_radius: 6.0,
            tab_radius: 5.0,
            control_radius: 4.0,
            inset_radius: 3.0,
            border_width: 1.0,
            splitter_width: 2.0,
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
            panel_bg: Color::rgb(0.90, 0.91, 0.94),
            root_bg: Color::rgb(0.82, 0.83, 0.86),
            bar_bg: Color::rgb(0.86, 0.87, 0.90),
            header_bg: Color::rgb(0.80, 0.82, 0.86),
            card_bg: Color::rgb(0.85, 0.86, 0.90),
            menu_bg: Color::rgb(0.95, 0.96, 0.98),
            scrim: Color::rgba(0.05, 0.05, 0.08, 0.45),
            heading: Color::rgb(0.10, 0.12, 0.16),
            subhead: Color::rgb(0.38, 0.42, 0.50),
            panel_border: Color::rgb(0.66, 0.68, 0.74),
            card_border: Color::rgb(0.72, 0.74, 0.80),
            field_border: Color::rgb(0.66, 0.68, 0.74),
            tab_border: Color::rgb(0.66, 0.68, 0.74),
            row_selected: Color::rgb(0.72, 0.80, 0.94),
            icon_active: Color::WHITE,
            card_radius: 6.0,
            component_radius: 6.0,
            tab_radius: 5.0,
            control_radius: 4.0,
            inset_radius: 3.0,
            border_width: 1.0,
            splitter_width: 2.0,
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

/// The side length of a checkbox's box, in pixels.
pub(crate) const CHECKBOX_SIZE: f32 = 18.0;
/// The corner radius of the checkbox box (a little rounded).
pub(crate) const CHECKBOX_RADIUS: f32 = 4.0;
/// The gap between the checkbox box and its caption, in pixels.
pub(crate) const CHECKBOX_GAP: f32 = 8.0;

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
