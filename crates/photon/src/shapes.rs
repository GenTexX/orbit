//! Lines, outlines, circles and arrows, built out of the one thing photon can
//! draw.
//!
//! The renderer draws textured quads and nothing else, so every shape in the
//! editor - the grid, the axes, the selection outline, the gizmo - has been
//! hand-rolled as a pile of stretched sprites, three times over in two files.
//! This is that vocabulary, written once and shared, drawn with a 1x1 white
//! texture and tinted.
//!
//! **It is a vocabulary, not a new rendering path.** Every shape here is quads,
//! so edges are hard: a diagonal line is a rotated rectangle with aliased sides,
//! and a circle is a fan of short bars rather than a curve. Doing it properly
//! means a signed-distance shader and a second pipeline, which is worth having
//! and is not this. What this buys is that the shapes exist as an API - callers
//! stop reinventing them, and the day the shader lands there is one place to
//! change.

use glam::Vec2;

use crate::{Color, Sprite};

/// A line from `a` to `b`, `width` pixels thick.
///
/// A rotated quad: the sprite's rotation is about its top-left corner, so the
/// rectangle is offset by half its thickness along the perpendicular before
/// being turned.
pub fn line(a: Vec2, b: Vec2, width: f32, color: Color) -> Sprite {
    let along = b - a;
    let length = along.length();
    if length <= f32::EPSILON {
        return Sprite::new(a, Vec2::ZERO);
    }
    let angle = along.y.atan2(along.x);
    let perpendicular = Vec2::new(-along.y, along.x) / length;
    let mut sprite = Sprite::new(a - perpendicular * (width * 0.5), Vec2::new(length, width));
    sprite.rotation = angle;
    sprite.tint = color;
    sprite
}

/// A filled axis-aligned rectangle.
pub fn rect(min: Vec2, size: Vec2, color: Color) -> Sprite {
    let mut sprite = Sprite::new(min, size);
    sprite.tint = color;
    sprite
}

/// The four edges of an axis-aligned rectangle, drawn inside its bounds.
pub fn rect_outline(min: Vec2, size: Vec2, width: f32, color: Color) -> Vec<Sprite> {
    let max = min + size;
    vec![
        rect(min, Vec2::new(size.x, width), color),
        rect(
            Vec2::new(min.x, max.y - width),
            Vec2::new(size.x, width),
            color,
        ),
        rect(min, Vec2::new(width, size.y), color),
        rect(
            Vec2::new(max.x - width, min.y),
            Vec2::new(width, size.y),
            color,
        ),
    ]
}

/// An arc of `sweep` radians starting at `start`, as a fan of short segments.
///
/// `segments` is chosen by the caller because only it knows how large the arc
/// will be on screen - a handle-sized one needs a handful and a level-sized one
/// needs many.
pub fn arc(
    center: Vec2,
    radius: f32,
    start: f32,
    sweep: f32,
    width: f32,
    color: Color,
    segments: usize,
) -> Vec<Sprite> {
    let segments = segments.max(1);
    let step = sweep / segments as f32;
    (0..segments)
        .map(|i| {
            let a = start + step * i as f32;
            let b = a + step;
            line(
                on_circle(center, radius, a),
                on_circle(center, radius, b),
                width,
                color,
            )
        })
        .collect()
}

/// A whole circle, as an arc that goes all the way round.
pub fn circle(center: Vec2, radius: f32, width: f32, color: Color, segments: usize) -> Vec<Sprite> {
    arc(
        center,
        radius,
        0.0,
        std::f32::consts::TAU,
        width,
        color,
        segments,
    )
}

/// A filled pie slice, as a fan of bars from the centre outward.
///
/// What a rotation drag wants: the amount turned, shown as area rather than as
/// a line somebody has to compare against another line.
pub fn pie(
    center: Vec2,
    radius: f32,
    start: f32,
    sweep: f32,
    color: Color,
    segments: usize,
) -> Vec<Sprite> {
    let segments = segments.max(1);
    let step = sweep / segments as f32;
    // Each slice is a bar from the centre to the rim, wide enough to meet its
    // neighbours at the rim - which is where a gap would show.
    let width = (radius * step.abs()).max(1.0) * 1.2;
    (0..segments)
        .map(|i| {
            let a = start + step * (i as f32 + 0.5);
            line(center, on_circle(center, radius, a), width, color)
        })
        .collect()
}

/// A solid arrowhead at `tip`, pointing along `direction`.
///
/// A triangle is not a quad, so it is a short stack of tapering bars. At the
/// size a gizmo handle is drawn this is indistinguishable from a triangle, and
/// it needs no pipeline that does not exist.
pub fn arrow_head(
    tip: Vec2,
    direction: Vec2,
    length: f32,
    half_width: f32,
    color: Color,
) -> Vec<Sprite> {
    let Some(forward) = direction.try_normalize() else {
        return Vec::new();
    };
    let across = Vec2::new(-forward.y, forward.x);
    const SLICES: usize = 8;
    (0..SLICES)
        .map(|i| {
            // 0 at the tip, 1 at the base: the bar is widest where the head
            // meets the shaft.
            let t = (i as f32 + 0.5) / SLICES as f32;
            let at = tip - forward * (length * t);
            let half = half_width * t;
            line(
                at - across * half,
                at + across * half,
                length / SLICES as f32 * 1.5,
                color,
            )
        })
        .collect()
}

fn on_circle(center: Vec2, radius: f32, angle: f32) -> Vec2 {
    center + Vec2::new(angle.cos(), angle.sin()) * radius
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f32 = 1e-3;

    #[test]
    fn a_horizontal_line_is_the_rectangle_you_would_have_written() {
        let s = line(
            Vec2::new(10.0, 20.0),
            Vec2::new(40.0, 20.0),
            2.0,
            Color::WHITE,
        );
        assert!((s.rotation).abs() < EPS, "no rotation: {}", s.rotation);
        assert!((s.size.x - 30.0).abs() < EPS, "as long as the run");
        assert!((s.size.y - 2.0).abs() < EPS, "as thick as asked");
        // Centred on the line rather than hanging off it.
        assert!((s.position.y - 19.0).abs() < EPS, "{}", s.position.y);
    }

    #[test]
    fn a_diagonal_line_keeps_its_length_and_faces_the_right_way() {
        let a = Vec2::new(0.0, 0.0);
        let b = Vec2::new(3.0, 4.0);
        let s = line(a, b, 1.0, Color::WHITE);
        assert!((s.size.x - 5.0).abs() < EPS, "3-4-5: {}", s.size.x);
        assert!((s.rotation - (4.0f32).atan2(3.0)).abs() < EPS);
    }

    #[test]
    fn a_degenerate_line_is_empty_rather_than_nan() {
        // Two identical points have no direction, and normalizing that is where
        // a NaN would get into the instance buffer and take the whole draw with
        // it.
        let s = line(Vec2::ONE, Vec2::ONE, 2.0, Color::WHITE);
        assert_eq!(s.size, Vec2::ZERO);
        assert!(s.rotation.is_finite());
    }

    #[test]
    fn an_outline_is_four_edges_inside_the_bounds() {
        let out = rect_outline(Vec2::ZERO, Vec2::new(100.0, 50.0), 2.0, Color::WHITE);
        assert_eq!(out.len(), 4);
        for edge in &out {
            assert!(edge.position.x >= -EPS && edge.position.y >= -EPS);
            assert!(edge.position.x + edge.size.x <= 100.0 + EPS);
            assert!(edge.position.y + edge.size.y <= 50.0 + EPS);
        }
    }

    #[test]
    fn an_arc_lands_on_the_circle_at_both_ends() {
        let center = Vec2::new(5.0, -5.0);
        let radius = 20.0;
        let segments = arc(
            center,
            radius,
            0.0,
            std::f32::consts::FRAC_PI_2,
            1.0,
            Color::WHITE,
            8,
        );
        assert_eq!(segments.len(), 8);
        // The first segment starts at angle 0, which is `center + (radius, 0)`,
        // give or take the half-width the quad is offset by to sit on the line.
        let first = &segments[0];
        let want = center + Vec2::new(radius, 0.0);
        assert!(
            (first.position - want).length() <= 0.51,
            "{}",
            first.position
        );

        // And the last one ends a quarter turn round, at `center + (0, radius)`
        // - downward, because y grows down (ADR 0012).
        let last = segments.last().expect("a segment");
        let end = last.position + Vec2::from_angle(last.rotation) * last.size.x;
        assert!(
            (end - (center + Vec2::new(0.0, radius))).length() <= 0.51,
            "{end}"
        );
    }

    #[test]
    fn an_arrow_head_tapers_to_its_tip() {
        let head = arrow_head(Vec2::new(100.0, 0.0), Vec2::X, 12.0, 6.0, Color::WHITE);
        assert!(!head.is_empty());
        // The slice nearest the tip is the narrowest, which is what makes it
        // read as a triangle rather than as a bar.
        let first = head.first().expect("a slice");
        let last = head.last().expect("a slice");
        assert!(
            first.size.x < last.size.x,
            "{} vs {}",
            first.size.x,
            last.size.x
        );
    }

    #[test]
    fn an_arrow_head_with_no_direction_draws_nothing() {
        assert!(arrow_head(Vec2::ZERO, Vec2::ZERO, 10.0, 4.0, Color::WHITE).is_empty());
    }
}
