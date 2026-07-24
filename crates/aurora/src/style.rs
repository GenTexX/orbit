//! aurora style: layout (delegated to taffy) plus Aurora's own visual style, with a fluent builder.

use taffy::prelude::{length, percent};
use taffy::{AlignItems, Display, FlexDirection, Overflow, Rect as TaffyRect, Size};

use crate::color::Color;
use crate::widget::Mask;

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
        self.layout.size = Size {
            width: percent(1.0),
            height: percent(1.0),
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
