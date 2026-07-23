//! aurora rect: an axis-aligned rectangle in UI space (top-left origin, pixels).

use glam::Vec2;

/// An axis-aligned rectangle. `pos` is the top-left corner (absolute, in UI
/// pixels); `size` is the extent. Produced by the layout pass; also the unit of
/// hit-testing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub pos: Vec2,
    pub size: Vec2,
}

impl Rect {
    /// A rectangle at `pos` with extent `size`.
    pub fn new(pos: Vec2, size: Vec2) -> Self {
        Self { pos, size }
    }

    /// The bottom-right corner (`pos + size`).
    pub fn max(self) -> Vec2 {
        self.pos + self.size
    }

    /// Whether `point` lies inside the rectangle, top-left inclusive and
    /// bottom-right exclusive (so adjacent rects do not both claim a point).
    pub fn contains(self, point: Vec2) -> bool {
        point.x >= self.pos.x
            && point.y >= self.pos.y
            && point.x < self.pos.x + self.size.x
            && point.y < self.pos.y + self.size.y
    }
}
