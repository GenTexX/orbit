//! aurora style: layout (delegated to taffy) plus Aurora's own visual style, with a fluent builder.

use taffy::prelude::{length, percent};
use taffy::{Display, FlexDirection, Rect as TaffyRect, Size};

use crate::color::Color;

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
}

impl Default for Style {
    fn default() -> Self {
        Self {
            layout: taffy::Style::default(),
            background: Color::TRANSPARENT,
            foreground: Color::WHITE,
            clip: false,
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

    /// Clip this widget's children to its rectangle.
    pub fn clip(mut self) -> Self {
        self.clip = true;
        self
    }
}
