//! photon 2D camera: maps Y-down, top-left, pixel-unit world space (ADR 0012) into clip space.

use glam::{Mat4, Vec2, Vec4};

/// A 2D camera over Y-down, top-left, pixel-unit world space (ADR 0012).
///
/// The camera views an axis-aligned rectangle of world space and produces the
/// view-projection matrix that maps it into wgpu clip space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    /// World coordinate mapped to the **top-left** of the viewport. Panning the
    /// camera moves this point; `(0, 0)` places the world origin at the
    /// top-left corner, matching screen space. A center-anchored convenience can
    /// be layered on later, when gameplay needs it.
    pub position: Vec2,
    /// Size of the viewport in physical pixels. One world unit is one pixel, so
    /// this is also the extent of world space the camera shows at 1x.
    pub viewport: Vec2,
}

impl Camera {
    /// Create a camera showing a `viewport`-sized region of world space with its
    /// top-left corner at `position`.
    pub fn new(position: Vec2, viewport: Vec2) -> Self {
        Self { position, viewport }
    }

    /// The view-projection matrix mapping world space into wgpu clip space
    /// (normalized device coordinates: X right, Y up, Z in `0..1`).
    pub fn view_projection(&self) -> Mat4 {
        let left = self.position.x;
        let top = self.position.y;
        let right = left + self.viewport.x;
        let bottom = top + self.viewport.y;

        // Orthographic projection into wgpu clip space (X/Y in -1..1, Z in
        // 0..1), built by hand so the Y-flip from ADR 0012 is explicit and
        // self-contained. Because Y grows downward, `top < bottom`, which makes
        // the Y scale negative - that negative scale *is* the flip from Y-down
        // world space to Y-up NDC, living in this one spot.
        let near = -1.0;
        let far = 1.0;
        let sx = 2.0 / (right - left);
        let sy = 2.0 / (top - bottom);
        let sz = 1.0 / (near - far); // right-handed, 0..1 depth
        let tx = -(right + left) / (right - left);
        let ty = -(top + bottom) / (top - bottom);
        let tz = near / (near - far);

        Mat4::from_cols(
            Vec4::new(sx, 0.0, 0.0, 0.0),
            Vec4::new(0.0, sy, 0.0, 0.0),
            Vec4::new(0.0, 0.0, sz, 0.0),
            Vec4::new(tx, ty, tz, 1.0),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Project a world point through the camera to NDC (w is 1 for an
    /// orthographic projection, but we divide anyway to stay honest).
    fn project(camera: &Camera, world: Vec2) -> Vec2 {
        let clip = camera.view_projection() * Vec4::new(world.x, world.y, 0.0, 1.0);
        Vec2::new(clip.x / clip.w, clip.y / clip.w)
    }

    const EPS: f32 = 1e-5;

    #[test]
    fn origin_maps_to_top_left() {
        let cam = Camera::new(Vec2::ZERO, Vec2::new(800.0, 600.0));
        let ndc = project(&cam, Vec2::ZERO);
        assert!((ndc.x - -1.0).abs() < EPS, "x = {}", ndc.x);
        assert!((ndc.y - 1.0).abs() < EPS, "y = {}", ndc.y);
    }

    #[test]
    fn far_corner_maps_to_bottom_right() {
        let cam = Camera::new(Vec2::ZERO, Vec2::new(800.0, 600.0));
        let ndc = project(&cam, Vec2::new(800.0, 600.0));
        assert!((ndc.x - 1.0).abs() < EPS, "x = {}", ndc.x);
        assert!((ndc.y - -1.0).abs() < EPS, "y = {}", ndc.y);
    }

    #[test]
    fn viewport_center_maps_to_ndc_origin() {
        let cam = Camera::new(Vec2::ZERO, Vec2::new(800.0, 600.0));
        let ndc = project(&cam, Vec2::new(400.0, 300.0));
        assert!(ndc.x.abs() < EPS, "x = {}", ndc.x);
        assert!(ndc.y.abs() < EPS, "y = {}", ndc.y);
    }

    #[test]
    fn panning_shifts_the_visible_region() {
        // With the camera's top-left at (100, 50), that world point should now
        // sit at the top-left of the view.
        let cam = Camera::new(Vec2::new(100.0, 50.0), Vec2::new(800.0, 600.0));
        let ndc = project(&cam, Vec2::new(100.0, 50.0));
        assert!((ndc.x - -1.0).abs() < EPS, "x = {}", ndc.x);
        assert!((ndc.y - 1.0).abs() < EPS, "y = {}", ndc.y);
    }

    #[test]
    fn y_increases_downward_on_screen() {
        // A larger world Y must map to a lower NDC Y - the ADR 0012 Y-down
        // guarantee.
        let cam = Camera::new(Vec2::ZERO, Vec2::new(800.0, 600.0));
        let upper = project(&cam, Vec2::new(400.0, 100.0));
        let lower = project(&cam, Vec2::new(400.0, 500.0));
        assert!(
            lower.y < upper.y,
            "upper.y = {}, lower.y = {}",
            upper.y,
            lower.y
        );
    }
}
