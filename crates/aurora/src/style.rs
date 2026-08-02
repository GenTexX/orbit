//! aurora style: layout (delegated to taffy) plus Aurora's own visual style, with a fluent builder.

use taffy::prelude::{length, percent};
use taffy::{AlignItems, Display, FlexDirection, Overflow, Rect as TaffyRect, Size};

use glam::Vec2;

use crate::color::Color;
use crate::widget::{FontFamily, FontWeight, Mask};

/// How a widget lays out and looks. Layout is a taffy `Style` (size, padding,
/// flex, gap); the visual properties (background, clipping) are Aurora's own.
///
/// Build it fluently:
/// `Style::new().column().padding(8.0).gap(4.0).background(Color::rgb(0.1, 0.1, 0.12))`.
#[derive(Debug, Clone)]
pub struct Style {
    /// The taffy layout style.
    pub layout: taffy::Style,
    /// Background fill. `Color::TRANSPARENT` (the default) draws nothing.
    pub background: Color,
    /// Text color, for widgets that draw text.
    pub foreground: Color,
    /// Clip children to this widget's rectangle.
    pub clip: bool,
    /// Scroll overflowing content vertically (implies clip). Children of a
    /// scroll container keep their natural size instead of flex-shrinking.
    pub scroll: bool,
    /// Render a button "flat": no default fill when idle, only a subtle
    /// highlight on hover/press (plus its own background if set). For list and
    /// tree rows that should not read as raised buttons.
    pub flat: bool,
    /// Ignore this widget in hit-testing (the cursor passes through to whatever
    /// is behind it). For decorative overlays like a drag insertion line.
    pub hit_transparent: bool,
    /// Render this widget (and its subtree) dimmed and inert: it takes no hover,
    /// press, click, or focus. For controls that are unavailable right now.
    pub disabled: bool,
    /// Placeholder text a text input shows, dimmed, while empty. Ignored by
    /// other widget kinds.
    pub placeholder: Option<String>,
    /// The input mask a text input enforces as you type. Ignored by other kinds.
    pub mask: Mask,
    /// Font size in pixels for this widget's text, or `None` for the UI default.
    pub font_size: Option<f32>,
    /// Font weight for this widget's text.
    pub font_weight: FontWeight,
    /// Font family for this widget's text.
    pub font_family: FontFamily,
    /// Whether a text input is multi-line (a text area). Ignored by other kinds.
    pub multiline: bool,
    /// Center this widget's text horizontally within its content box (labels and
    /// button captions). The default is left-aligned.
    pub text_center: bool,
    /// Render an interactive button as an "icon button": no fill at all; a child
    /// icon recolors on hover/press instead of the button drawing a background.
    pub icon_button: bool,
    /// Truncate this widget's text with a trailing "..." when it does not fit
    /// the widget's width (a single line; needs a fixed or grow width).
    pub ellipsis: bool,
    /// Per-corner radius in px for this widget's background,
    /// `[top_left, top_right, bottom_right, bottom_left]` (0 = square corners).
    pub corner_radii: [f32; 4],
    /// Border width in px around this widget's background (0 = no border).
    pub border_width: f32,
    /// Border color (drawn when `border_width` > 0).
    pub border_color: Color,
    /// Which edges the border draws on, `[top, right, bottom, left]`; an open
    /// edge leaves the fill reaching it with no stroke (a folder-style tab).
    pub border_sides: [bool; 4],
    /// A draw-time offset for this widget and its subtree, in px. Layout and
    /// hit-testing ignore it, so a widget can be slid around - a tab following
    /// the pointer, or easing into a new position - without disturbing the
    /// layout it will settle into.
    pub translate: Vec2,
    /// A bottom-edge-only border (0 = none), drawn beneath this widget's children.
    pub border_bottom_width: f32,
    pub border_bottom_color: Color,
    /// Whether a multi-line text input draws a line-number gutter down its left
    /// edge. Ignored by other kinds.
    pub gutter: bool,
    /// Whether typing an opening bracket or quote inserts its partner.
    pub auto_pairs: bool,
    /// Whether a text area lets long lines run off the right edge and scrolls
    /// horizontally, instead of wrapping them.
    pub no_wrap: bool,
    /// Whether this widget is a drag source: pressing and moving past a small
    /// threshold starts a drag carrying this widget (see `Event::DragStarted`).
    pub draggable: bool,
    /// Whether this widget accepts drops: a drag released over it reports it as
    /// the target (see `Event::Dropped`).
    pub drop_target: bool,
    /// What a drag of this widget carries, as a caller-defined bitmask. A target
    /// takes the drop only if this and its [`accepts`](Self::accepts) share a
    /// bit. Both default to every bit set, so a drag with nothing to say lands
    /// anywhere that takes drops.
    pub drag_kind: u32,
    /// Which [`drag_kind`](Self::drag_kind)s this target takes.
    pub accepts: u32,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            layout: taffy::Style::default(),
            background: Color::TRANSPARENT,
            foreground: Color::WHITE,
            clip: false,
            scroll: false,
            flat: false,
            hit_transparent: false,
            disabled: false,
            placeholder: None,
            mask: Mask::None,
            font_size: None,
            font_weight: FontWeight::Normal,
            font_family: FontFamily::Sans,
            multiline: false,
            text_center: false,
            icon_button: false,
            ellipsis: false,
            corner_radii: [0.0; 4],
            border_width: 0.0,
            border_color: Color::TRANSPARENT,
            border_sides: [true; 4],
            translate: Vec2::ZERO,
            border_bottom_width: 0.0,
            border_bottom_color: Color::TRANSPARENT,
            gutter: false,
            auto_pairs: false,
            no_wrap: false,
            draggable: false,
            drop_target: false,
            drag_kind: u32::MAX,
            accepts: u32::MAX,
        }
    }
}

impl Style {
    /// A default style: auto layout, transparent background, white text, no clip.
    pub fn new() -> Self {
        Self::default()
    }

    /// Fill the parent (100% width and height). For the root, this fills the
    /// available space passed to [`Ui::layout`](crate::Ui::layout).
    pub fn fill(mut self) -> Self {
        // Suffixed: `percent` is generic over `Into<f32>`, so a bare literal
        // falls back to f32 rather than being inferred as one - which newer
        // toolchains warn about and will eventually reject.
        self.layout.size = Size {
            width: percent(1.0_f32),
            height: percent(1.0_f32),
        };
        self
    }

    /// Fix the widget's size in pixels. Sets the minimum size too, so a fixed
    /// size is not flex-shrunk when it overflows its container.
    pub fn size(mut self, width: f32, height: f32) -> Self {
        let s = Size {
            width: length(width),
            height: length(height),
        };
        self.layout.size = s;
        self.layout.min_size = s;
        self
    }

    /// Fix the width in pixels, leaving the height to layout (e.g. stretched to
    /// the parent's cross axis). Pins the minimum width too, so it is not shrunk.
    pub fn width(mut self, width: f32) -> Self {
        self.layout.size.width = length(width);
        self.layout.min_size.width = length(width);
        self
    }

    /// Fix the height in pixels, leaving the width to layout.
    pub fn height(mut self, height: f32) -> Self {
        self.layout.size.height = length(height);
        self.layout.min_size.height = length(height);
        self
    }

    /// Uniform padding on all four sides, in pixels.
    pub fn padding(mut self, pad: f32) -> Self {
        self.layout.padding = TaffyRect {
            left: length(pad),
            right: length(pad),
            top: length(pad),
            bottom: length(pad),
        };
        self
    }

    /// Horizontal padding (left and right), in pixels. Composes with the
    /// vertical and per-side setters.
    pub fn padding_x(mut self, pad: f32) -> Self {
        self.layout.padding.left = length(pad);
        self.layout.padding.right = length(pad);
        self
    }

    /// Vertical padding (top and bottom), in pixels.
    pub fn padding_y(mut self, pad: f32) -> Self {
        self.layout.padding.top = length(pad);
        self.layout.padding.bottom = length(pad);
        self
    }

    /// Left padding only, in pixels.
    pub fn padding_left(mut self, pad: f32) -> Self {
        self.layout.padding.left = length(pad);
        self
    }

    /// Right padding only, in pixels.
    pub fn padding_right(mut self, pad: f32) -> Self {
        self.layout.padding.right = length(pad);
        self
    }

    /// Top padding only, in pixels.
    pub fn padding_top(mut self, pad: f32) -> Self {
        self.layout.padding.top = length(pad);
        self
    }

    /// Bottom padding only, in pixels.
    pub fn padding_bottom(mut self, pad: f32) -> Self {
        self.layout.padding.bottom = length(pad);
        self
    }

    /// Uniform margin on all four sides, in pixels. Margin is space *outside*
    /// the box, between it and its siblings (it does not fill with background).
    pub fn margin(mut self, m: f32) -> Self {
        self.layout.margin = TaffyRect {
            left: length(m),
            right: length(m),
            top: length(m),
            bottom: length(m),
        };
        self
    }

    /// Horizontal margin (left and right), in pixels.
    pub fn margin_x(mut self, m: f32) -> Self {
        self.layout.margin.left = length(m);
        self.layout.margin.right = length(m);
        self
    }

    /// Vertical margin (top and bottom), in pixels.
    pub fn margin_y(mut self, m: f32) -> Self {
        self.layout.margin.top = length(m);
        self.layout.margin.bottom = length(m);
        self
    }

    /// Left margin only, in pixels.
    pub fn margin_left(mut self, m: f32) -> Self {
        self.layout.margin.left = length(m);
        self
    }

    /// Right margin only, in pixels.
    pub fn margin_right(mut self, m: f32) -> Self {
        self.layout.margin.right = length(m);
        self
    }

    /// Top margin only, in pixels.
    pub fn margin_top(mut self, m: f32) -> Self {
        self.layout.margin.top = length(m);
        self
    }

    /// Bottom margin only, in pixels.
    pub fn margin_bottom(mut self, m: f32) -> Self {
        self.layout.margin.bottom = length(m);
        self
    }

    /// Gap between flex children, in pixels.
    pub fn gap(mut self, gap: f32) -> Self {
        self.layout.gap = Size {
            width: length(gap),
            height: length(gap),
        };
        self
    }

    /// Lay children out in a horizontal flex row.
    pub fn row(mut self) -> Self {
        self.layout.display = Display::Flex;
        self.layout.flex_direction = FlexDirection::Row;
        self
    }

    /// Lay children out in a vertical flex column.
    pub fn column(mut self) -> Self {
        self.layout.display = Display::Flex;
        self.layout.flex_direction = FlexDirection::Column;
        self
    }

    /// Flex grow factor: how much of the leftover space along the parent's main
    /// axis this widget claims.
    pub fn grow(mut self, factor: f32) -> Self {
        self.layout.flex_grow = factor;
        self
    }

    /// Center children along the cross axis (e.g. vertically center a row's
    /// items). Handy for label-plus-control rows.
    pub fn align_center(mut self) -> Self {
        self.layout.align_items = Some(AlignItems::CENTER);
        self
    }

    /// Set the background fill color.
    pub fn background(mut self, color: Color) -> Self {
        self.background = color;
        self
    }

    /// Set the text (foreground) color.
    pub fn foreground(mut self, color: Color) -> Self {
        self.foreground = color;
        self
    }

    /// Render a button flat (no raised fill; only a subtle hover/press
    /// highlight over its background) - for list and tree rows.
    pub fn flat(mut self) -> Self {
        self.flat = true;
        self
    }

    /// Make this widget invisible to hit-testing (the cursor passes through) -
    /// for decorative overlays like a drag insertion line.
    pub fn hit_transparent(mut self) -> Self {
        self.hit_transparent = true;
        self
    }

    /// Mark this widget disabled: dimmed and inert (no hover, press, click, or
    /// focus). For a control that is unavailable in the current context.
    pub fn disabled(mut self) -> Self {
        self.disabled = true;
        self
    }

    /// Set placeholder text a text input shows, dimmed, while it is empty.
    pub fn placeholder(mut self, text: impl Into<String>) -> Self {
        self.placeholder = Some(text.into());
        self
    }

    /// Set the input mask a text input enforces as you type (rejecting edits
    /// that would break it). See [`Mask`](crate::Mask).
    pub fn mask(mut self, mask: Mask) -> Self {
        self.mask = mask;
        self
    }

    /// Set this widget's text size in pixels (headings, captions, and the like).
    /// Text is re-shaped at this size and the widget measures to match.
    pub fn font_size(mut self, px: f32) -> Self {
        self.font_size = Some(px);
        self
    }

    /// Set this widget's text weight (light / normal / bold).
    pub fn weight(mut self, weight: FontWeight) -> Self {
        self.font_weight = weight;
        self
    }

    /// Give a multi-line text input a line-number gutter down its left edge.
    ///
    /// Aurora sizes it to the widest line number, keeps it aligned as the text
    /// wraps and scrolls, marks the line the caret is on, and turns a click in
    /// it into [`Event::GutterClicked`](crate::Event) - so an app never has to
    /// reproduce the text area's line layout beside it.
    pub fn gutter(mut self) -> Self {
        self.gutter = true;
        self
    }

    /// Auto-close brackets and quotes in a text area.
    ///
    /// Typing an opener inserts its partner and leaves the caret between them;
    /// typing the closer that is already there steps over it; backspace between
    /// an empty pair removes both; and typing an opener with a selection wraps
    /// the selection rather than replacing it. It stays out of the way where it
    /// would be wrong: not inside a string or a comment, and not when the next
    /// character is a word - typing `(` before a name means wrapping it.
    pub fn auto_pairs(mut self) -> Self {
        self.auto_pairs = true;
        self
    }

    /// Stop a text area wrapping: long lines run off the right edge and the
    /// field scrolls horizontally to follow the caret.
    ///
    /// Code should not wrap. While it does, a line number no longer names a row
    /// and a column ruler or an indent guide means nothing, because one logical
    /// line occupies several of them.
    pub fn no_wrap(mut self) -> Self {
        self.no_wrap = true;
        self
    }

    /// Shape this widget's text with the bundled fixed-pitch face, for code and
    /// anything whose columns have to line up.
    pub fn monospace(mut self) -> Self {
        self.font_family = FontFamily::Mono;
        self
    }

    /// Shorthand for bold text (`weight(FontWeight::Bold)`).
    pub fn bold(self) -> Self {
        self.weight(FontWeight::Bold)
    }

    /// Make a text input multi-line (a text area): Enter inserts a newline
    /// instead of submitting, and Up/Down move the caret between lines. The
    /// field measures to its wrapped content height (it grows as lines are
    /// added).
    /// `multiline()` when `yes`, unchanged otherwise - for a caller deciding
    /// from data rather than at the call site.
    pub fn multiline_if(self, yes: bool) -> Self {
        if yes { self.multiline() } else { self }
    }

    pub fn multiline(mut self) -> Self {
        self.multiline = true;
        self
    }

    /// Make this button an "icon button": it draws no fill; its child icon
    /// recolors on hover/press instead (see the visibility eye). The button
    /// stays interactive.
    pub fn icon_button(mut self) -> Self {
        self.icon_button = true;
        self
    }

    /// Truncate this widget's text with a trailing "..." when it overflows the
    /// widget's width, keeping it on one line. Needs a constrained width (a fixed
    /// `width`/`size`, or `grow` inside a bounded parent).
    pub fn ellipsis(mut self) -> Self {
        self.ellipsis = true;
        self
    }

    /// Round all four of this widget's background corners to `radius` px
    /// (anti-aliased).
    pub fn corner_radius(mut self, radius: f32) -> Self {
        self.corner_radii = [radius; 4];
        self
    }

    /// Round only the two top corners to `radius` px, leaving the bottom square
    /// (a tab that meets the content below it).
    pub fn round_top(mut self, radius: f32) -> Self {
        self.corner_radii = [radius, radius, 0.0, 0.0];
        self
    }

    /// Set each corner radius independently, `[top_left, top_right,
    /// bottom_right, bottom_left]` px.
    pub fn corner_radii(mut self, radii: [f32; 4]) -> Self {
        self.corner_radii = radii;
        self
    }

    /// Draw a `width`-px border in `color` around this widget's background.
    pub fn border(mut self, width: f32, color: Color) -> Self {
        self.border_width = width;
        self.border_color = color;
        self
    }

    /// Offset this widget and its subtree by `delta` px when drawing, without
    /// moving it for layout or hit-testing. See [`Style::translate`].
    pub fn translate(mut self, delta: Vec2) -> Self {
        self.translate = delta;
        self
    }

    /// Restrict which edges the border draws on, `[top, right, bottom, left]`;
    /// an open edge leaves the fill reaching it with no stroke. Pairs with
    /// [`border`](Self::border) (which sets the width and color).
    pub fn border_sides(mut self, sides: [bool; 4]) -> Self {
        self.border_sides = sides;
        self
    }

    /// Draw a `width`-px line in `color` along this widget's bottom edge only,
    /// under its children (so a child reaching the bottom - an active tab -
    /// covers the segment beneath it). A bar underline that the active tab
    /// interrupts.
    pub fn border_bottom(mut self, width: f32, color: Color) -> Self {
        self.border_bottom_width = width;
        self.border_bottom_color = color;
        self
    }

    /// Center this widget's text horizontally in its content box (e.g. a short
    /// button caption like an "X"/"Y" axis label). Default is left-aligned.
    pub fn text_center(mut self) -> Self {
        self.text_center = true;
        self
    }

    /// Make this widget a drag source: a press-and-drag past a small threshold
    /// starts a drag carrying it (see [`Event::DragStarted`](crate::Event)).
    pub fn draggable(mut self) -> Self {
        self.draggable = true;
        self
    }

    /// Make this widget a drop target: a drag released over it reports it as the
    /// target (see [`Event::Dropped`](crate::Event)).
    pub fn drop_target(mut self) -> Self {
        self.drop_target = true;
        self
    }

    /// Tag what a drag of this widget carries, so targets can turn it down.
    ///
    /// Only the app knows whether a particular drag can land on a particular
    /// target - aurora sees widget ids. Without a tag it offers every drop
    /// target under the cursor, which overstates what a release would actually
    /// do. `mask` is caller-defined; a target takes the drop when its
    /// [`accepts`](Self::accepts) shares a bit with it.
    pub fn drag_kind(mut self, mask: u32) -> Self {
        self.drag_kind = mask;
        self
    }

    /// Restrict which [`drag_kind`](Self::drag_kind)s this target takes. A drag
    /// that shares no bit with `mask` passes over it as if it were not a target
    /// at all: no highlight, and no `Dropped` naming it.
    pub fn accepts(mut self, mask: u32) -> Self {
        self.accepts = mask;
        self
    }

    /// Set the disabled state from a flag, for `style.enabled(cond)`-style call
    /// sites that toggle a control on a condition.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.disabled = !enabled;
        self
    }

    /// Clip this widget's children to its rectangle. Also maps to taffy
    /// `Overflow::Hidden` on both axes, which zeroes the widget's automatic
    /// minimum size - so clipped containers take the size their parent gives
    /// them instead of ballooning to their content (the CSS overflow:hidden
    /// behavior, and what makes nested scroll layouts work).
    pub fn clip(mut self) -> Self {
        self.clip = true;
        self.layout.overflow = taffy::Point {
            x: Overflow::Hidden,
            y: Overflow::Hidden,
        };
        self
    }

    /// Scroll overflowing content vertically (mouse wheel; implies clip).
    /// Children keep their natural size instead of flex-shrinking to fit, so
    /// tall content overflows and scrolls rather than squashing. The
    /// container's own automatic minimum height drops to zero (taffy
    /// `Overflow::Hidden`), so tall content cannot balloon the container
    /// itself - it stays the size its parent gives it.
    pub fn scroll(mut self) -> Self {
        self.scroll = true;
        self = self.clip();
        self
    }
}
