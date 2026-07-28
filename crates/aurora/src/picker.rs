//! aurora color picker: a floating HSVA picker - a saturation/value square, a
//! hue bar, an alpha bar, and a hex field - with the drag routing that makes it
//! work.
//!
//! The gradients are CPU-generated RGBA bitmaps that the app uploads as images,
//! because aurora never touches the GPU itself. That upload is the only thing
//! the app has to wire: register [`ColorPicker::initial_bitmaps`] once, then
//! after each interaction upload whatever [`ColorPicker::dirty_bitmaps`] hands
//! back. Only the bitmaps whose inputs actually changed are returned - dragging
//! inside the square never rebuilds the hue bar, and the square's gradient
//! itself is cached per hue, so a drag re-stamps a marker instead of recomputing
//! every pixel.
//!
//! The picker holds no notion of *what* is being edited. Keep it beside your own
//! target (a field reference, a variable name) and read [`ColorPicker::rgba`]
//! whenever it reports a change.

use glam::Vec2;

use crate::color::{Color, Hsva, hsv_to_rgb, to_hex};
use crate::draw::ImageHandle;
use crate::style::Style;
use crate::theme::Theme;
use crate::ui::Ui;
use crate::widget::WidgetId;

/// The SV square's side, the hue bar's width, and the alpha bar's height, in px.
pub const SV_SIZE: usize = 176;
pub const HUE_W: usize = 16;
pub const ALPHA_H: usize = 16;

/// One CPU-generated gradient, ready to be uploaded as an image.
pub struct Bitmap {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Which of the picker's three gradients a press landed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    Sv,
    Hue,
    Alpha,
}

/// The picker's widgets, recorded when it is built, so presses and drags can be
/// routed to the right gradient.
#[derive(Debug, Clone, Copy, Default)]
pub struct PickerRows {
    pub card: Option<WidgetId>,
    pub sv: Option<WidgetId>,
    pub hue: Option<WidgetId>,
    pub alpha: Option<WidgetId>,
    pub hex: Option<WidgetId>,
}

/// What a press on the picker meant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Press {
    /// The press started a drag on one of the gradients.
    Grabbed(Region),
    /// The press landed on the picker but not on a gradient (the hex field).
    Inside,
    /// The press landed outside the picker - the caller should close it.
    Outside,
}

/// An open color picker: its working color, where it floats, and any live drag.
pub struct ColorPicker {
    hsva: Hsva,
    anchor: Vec2,
    drag: Option<Region>,
    images: [ImageHandle; 3],
    /// The SV square's gradient for the current hue, kept so a drag inside the
    /// square only re-stamps the marker instead of recomputing every pixel.
    sv_field: Vec<u8>,
    sv_field_hue: Option<f32>,
    /// The color the images on the GPU were last generated for.
    uploaded: Option<Hsva>,
}

impl ColorPicker {
    /// The three gradients as they first look, for the app to register as images
    /// (in the order the handles are passed to [`ColorPicker::new`]: SV square,
    /// hue bar, alpha bar). They are drawn at their native size, so register
    /// them without mipmaps if the backend offers the choice.
    pub fn initial_bitmaps() -> [Bitmap; 3] {
        let hsva = Hsva::from_rgba([1.0, 1.0, 1.0, 1.0], 0.0);
        [
            Bitmap {
                rgba: sv_square(hsva.h, SV_SIZE, hsva.s, hsva.v),
                width: SV_SIZE as u32,
                height: SV_SIZE as u32,
            },
            Bitmap {
                rgba: hue_bar(HUE_W, SV_SIZE, hsva.h),
                width: HUE_W as u32,
                height: SV_SIZE as u32,
            },
            Bitmap {
                rgba: alpha_bar([0.0, 0.0, 0.0], SV_SIZE, ALPHA_H, 1.0),
                width: SV_SIZE as u32,
                height: ALPHA_H as u32,
            },
        ]
    }

    /// Open a picker on `rgba`, floating at `anchor`. `images` are the handles
    /// the app registered from [`initial_bitmaps`](Self::initial_bitmaps).
    pub fn new(images: [ImageHandle; 3], anchor: Vec2, rgba: [f32; 4]) -> Self {
        Self {
            hsva: Hsva::from_rgba(rgba, 0.0),
            anchor,
            drag: None,
            images,
            sv_field: Vec::new(),
            sv_field_hue: None,
            uploaded: None,
        }
    }

    /// The working color.
    pub fn rgba(&self) -> [f32; 4] {
        self.hsva.to_rgba()
    }

    /// Where the picker floats.
    pub fn anchor(&self) -> Vec2 {
        self.anchor
    }

    /// Whether a gradient drag is in progress.
    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// End any gradient drag (the picker stays open).
    pub fn end_drag(&mut self) {
        self.drag = None;
    }

    /// Set the working color (e.g. after the hex field was submitted).
    pub fn set_rgba(&mut self, rgba: [f32; 4]) {
        self.hsva = Hsva::from_rgba(rgba, self.hsva.h);
    }

    /// Parse `text` as a hex color into the working color, reporting whether it
    /// was valid.
    pub fn commit_hex(&mut self, text: &str) -> bool {
        match crate::color::from_hex(text) {
            Some(rgba) => {
                self.set_rgba(rgba);
                true
            }
            None => false,
        }
    }

    /// Route a press at `cursor`. On a gradient this starts a drag and applies
    /// it immediately, so a click jumps the marker to where it landed.
    pub fn press(&mut self, ui: &Ui, rows: &PickerRows, cursor: Vec2) -> Press {
        if let Some(region) = self.region_at(ui, rows, cursor) {
            self.drag = Some(region);
            self.drag_to(ui, rows, cursor);
            return Press::Grabbed(region);
        }
        let inside = rows
            .card
            .and_then(|c| ui.rect(c))
            .is_some_and(|r| r.contains(cursor));
        if inside {
            Press::Inside
        } else {
            Press::Outside
        }
    }

    /// Apply a live gradient drag to `cursor`, reporting whether the color
    /// changed (so the caller can skip work when it did not).
    pub fn drag_to(&mut self, ui: &Ui, rows: &PickerRows, cursor: Vec2) -> bool {
        let Some(region) = self.drag else {
            return false;
        };
        let widget = match region {
            Region::Sv => rows.sv,
            Region::Hue => rows.hue,
            Region::Alpha => rows.alpha,
        };
        let Some(rect) = widget.and_then(|id| ui.rect(id)) else {
            return false;
        };
        let fx = ((cursor.x - rect.pos.x) / rect.size.x).clamp(0.0, 1.0);
        let fy = ((cursor.y - rect.pos.y) / rect.size.y).clamp(0.0, 1.0);
        let before = self.hsva;
        match region {
            Region::Sv => {
                self.hsva.s = fx;
                self.hsva.v = 1.0 - fy;
            }
            Region::Hue => self.hsva.h = fy,
            Region::Alpha => self.hsva.a = fx,
        }
        self.hsva != before
    }

    /// Which gradient `p` is over, if any.
    fn region_at(&self, ui: &Ui, rows: &PickerRows, p: Vec2) -> Option<Region> {
        let over =
            |id: Option<WidgetId>| id.and_then(|i| ui.rect(i)).is_some_and(|r| r.contains(p));
        if over(rows.sv) {
            Some(Region::Sv)
        } else if over(rows.hue) {
            Some(Region::Hue)
        } else if over(rows.alpha) {
            Some(Region::Alpha)
        } else {
            None
        }
    }

    /// The gradients that need re-uploading since the last call, each paired
    /// with the handle to upload it to.
    ///
    /// Each bitmap depends on a different slice of the color, so most of the
    /// time this returns one or two, not three: the square's gradient is a
    /// function of hue alone (s and v only place its marker), and the hue bar
    /// depends on nothing else.
    pub fn dirty_bitmaps(&mut self) -> Vec<(ImageHandle, Bitmap)> {
        let hsva = self.hsva;
        let was = self.uploaded;
        let moved = |f: fn(&Hsva) -> f32| was.is_none_or(|w| f(&w) != f(&hsva));
        let hue_moved = moved(|c| c.h);
        let sv_moved = moved(|c| c.s) || moved(|c| c.v);
        let mut out = Vec::new();
        if hue_moved || sv_moved {
            if hue_moved || self.sv_field_hue != Some(hsva.h) {
                self.sv_field = sv_field(hsva.h, SV_SIZE);
                self.sv_field_hue = Some(hsva.h);
            }
            out.push((
                self.images[0],
                Bitmap {
                    rgba: sv_square_from(&self.sv_field, SV_SIZE, hsva.s, hsva.v),
                    width: SV_SIZE as u32,
                    height: SV_SIZE as u32,
                },
            ));
        }
        if hue_moved {
            out.push((
                self.images[1],
                Bitmap {
                    rgba: hue_bar(HUE_W, SV_SIZE, hsva.h),
                    width: HUE_W as u32,
                    height: SV_SIZE as u32,
                },
            ));
        }
        if hue_moved || sv_moved || moved(|c| c.a) {
            let [r, g, b, _] = hsva.to_rgba();
            out.push((
                self.images[2],
                Bitmap {
                    rgba: alpha_bar([r, g, b], SV_SIZE, ALPHA_H, hsva.a),
                    width: SV_SIZE as u32,
                    height: ALPHA_H as u32,
                },
            ));
        }
        self.uploaded = Some(hsva);
        out
    }

    /// Build the picker on the popup layer, recording its widgets for routing.
    /// Call this after the rest of the tree so it floats above it.
    pub fn build(&self, ui: &mut Ui, theme: &Theme) -> PickerRows {
        let card = ui.popup(
            self.anchor,
            Style::new()
                .column()
                .gap(6.0)
                .padding(8.0)
                .background(theme.menu_bg)
                .corner_radius(theme.card_radius)
                .border(theme.border_width, theme.card_border)
                .clip(),
        );
        let sv_px = SV_SIZE as f32;
        let top = ui.panel(card, Style::new().row().gap(6.0));
        let sv = ui.image(top, self.images[0], Style::new().size(sv_px, sv_px));
        let hue = ui.image(top, self.images[1], Style::new().size(HUE_W as f32, sv_px));
        let alpha = ui.image(
            card,
            self.images[2],
            Style::new().size(sv_px, ALPHA_H as f32),
        );

        let rgba = self.hsva.to_rgba();
        let bottom = ui.panel(card, Style::new().row().gap(6.0).align_center());
        let hex = ui.text_input(
            bottom,
            to_hex(rgba),
            Style::new()
                .grow(1.0)
                .padding(4.0)
                .corner_radius(theme.control_radius)
                .placeholder("#RRGGBBAA"),
        );
        let [r, g, b, a] = rgba;
        ui.panel(
            bottom,
            Style::new()
                .size(28.0, 24.0)
                .background(Color::rgba(r, g, b, a))
                .corner_radius(theme.control_radius),
        );

        PickerRows {
            card: Some(card),
            sv: Some(sv),
            hue: Some(hue),
            alpha: Some(alpha),
            hex: Some(hex),
        }
    }
}

// --- gradient bitmaps (RGBA8, row-major) ---

/// The saturation/value square for a given `hue`: saturation left->right,
/// value bottom->top. A ring marks the current `(s, v)`.
///
/// The gradient itself depends only on `hue`, so a caller dragging inside the
/// square (which changes `s`/`v` but not the hue) should keep the field from
/// [`sv_field`] and re-stamp it with [`sv_square_from`] instead of calling this -
/// that turns a `size * size` recompute per mouse move into a copy.
fn sv_square(hue: f32, size: usize, s: f32, v: f32) -> Vec<u8> {
    sv_square_from(&sv_field(hue, size), size, s, v)
}

/// The saturation/value square's gradient for `hue`, with no marker: a reusable
/// field for every `(s, v)` at that hue.
fn sv_field(hue: f32, size: usize) -> Vec<u8> {
    let mut px = vec![0u8; size * size * 4];
    for y in 0..size {
        let val = 1.0 - y as f32 / (size - 1) as f32;
        for x in 0..size {
            let sat = x as f32 / (size - 1) as f32;
            let [r, g, b] = hsv_to_rgb(hue, sat, val);
            put(&mut px, size, x, y, [byte(r), byte(g), byte(b), 255]);
        }
    }
    px
}

/// A copy of `field` (from [`sv_field`]) with the selection ring stamped at
/// `(s, v)`.
fn sv_square_from(field: &[u8], size: usize, s: f32, v: f32) -> Vec<u8> {
    let mut px = field.to_vec();
    let mx = (s * (size - 1) as f32) as i32;
    let my = ((1.0 - v) * (size - 1) as f32) as i32;
    draw_ring(&mut px, size, size, mx, my, 4.0);
    px
}

/// A vertical hue bar (full spectrum top->bottom), `w` by `h`, with a marker
/// at the current `hue`.
fn hue_bar(w: usize, h: usize, hue: f32) -> Vec<u8> {
    let mut px = vec![0u8; w * h * 4];
    for y in 0..h {
        let [r, g, b] = hsv_to_rgb(y as f32 / (h - 1) as f32, 1.0, 1.0);
        for x in 0..w {
            put(&mut px, w, x, y, [byte(r), byte(g), byte(b), 255]);
        }
    }
    draw_hline_marker(&mut px, w, h, (hue * (h - 1) as f32) as i32);
    px
}

/// A horizontal alpha bar for `rgb` over a checkerboard (transparent left ->
/// opaque right), `w` by `h`, with a marker at the current alpha.
fn alpha_bar(rgb: [f32; 3], w: usize, h: usize, alpha: f32) -> Vec<u8> {
    let mut px = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let a = x as f32 / (w - 1) as f32;
            // Checkerboard so transparency reads.
            let check = if (x / 6 + y / 6) % 2 == 0 { 0.55 } else { 0.4 };
            let blend = |c: f32| byte(c * a + check * (1.0 - a));
            put(
                &mut px,
                w,
                x,
                y,
                [blend(rgb[0]), blend(rgb[1]), blend(rgb[2]), 255],
            );
        }
    }
    draw_vline_marker(&mut px, w, h, (alpha * (w - 1) as f32) as i32);
    px
}

fn byte(f: f32) -> u8 {
    (f.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn put(px: &mut [u8], stride: usize, x: usize, y: usize, rgba: [u8; 4]) {
    let i = (y * stride + x) * 4;
    px[i..i + 4].copy_from_slice(&rgba);
}

/// A hollow ring marker (white core, dark edge for contrast on any color).
///
/// Only pixels within `r + 1.6` of the centre can be painted, so the scan is
/// clamped to that bounding box rather than sweeping the whole image - for the
/// picker's SV square that is a ~13x13 box instead of 176x176.
fn draw_ring(px: &mut [u8], w: usize, h: usize, cx: i32, cy: i32, r: f32) {
    let reach = (r + 1.6).ceil() as i32;
    let y0 = (cy - reach).max(0);
    let y1 = (cy + reach).min(h as i32 - 1);
    let x0 = (cx - reach).max(0);
    let x1 = (cx + reach).min(w as i32 - 1);
    for y in y0..=y1 {
        for x in x0..=x1 {
            let d = (((x - cx) * (x - cx) + (y - cy) * (y - cy)) as f32).sqrt();
            let ring = (d - r).abs();
            if ring <= 1.6 {
                let shade = if ring <= 0.8 { 255 } else { 20 };
                put(px, w, x as usize, y as usize, [shade, shade, shade, 255]);
            }
        }
    }
}

/// A short marker line across the left/right edges of a vertical bar at row `y`.
fn draw_hline_marker(px: &mut [u8], w: usize, h: usize, my: i32) {
    for y in (my - 1).max(0)..=(my + 1).min(h as i32 - 1) {
        let shade = if y == my { 255 } else { 20 };
        for x in 0..w {
            put(px, w, x, y as usize, [shade, shade, shade, 255]);
        }
    }
}

/// A marker line down a horizontal bar at column `x`.
fn draw_vline_marker(px: &mut [u8], w: usize, h: usize, mx: i32) {
    for x in (mx - 1).max(0)..=(mx + 1).min(w as i32 - 1) {
        let shade = if x == mx { 255 } else { 20 };
        for y in 0..h {
            put(px, w, x as usize, y, [shade, shade, shade, 255]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_picker_lays_out_in_its_popup_and_routes_presses_and_drags() {
        let mut ui = Ui::new();
        ui.root_panel(Style::new().fill());
        let h = ImageHandle::default();
        let mut picker = ColorPicker::new([h, h, h], Vec2::new(200.0, 150.0), [1.0, 0.5, 0.2, 1.0]);
        let rows = picker.build(&mut ui, &Theme::dark());
        ui.layout(Vec2::new(1200.0, 800.0)).unwrap();

        // Every part sits inside the popup card.
        let card = rows.card.expect("card");
        for part in [rows.sv, rows.hue, rows.alpha, rows.hex] {
            assert!(ui.is_within(part.expect("part"), card));
        }

        // A press on the SV square grabs it, and dragging across it changes the
        // color (saturation rises to the right, value falls downward).
        let sv = ui.rect(rows.sv.expect("sv")).expect("sv rect");
        assert_eq!(
            picker.press(&ui, &rows, sv.pos + sv.size * 0.5),
            Press::Grabbed(Region::Sv)
        );
        let middle = picker.rgba();
        assert!(picker.drag_to(&ui, &rows, sv.pos + Vec2::new(sv.size.x, 0.0)));
        assert_ne!(picker.rgba(), middle, "dragging moved the color");

        // A press away from the popup asks the caller to close it.
        assert_eq!(picker.press(&ui, &rows, Vec2::ZERO), Press::Outside);
    }

    #[test]
    fn only_the_gradients_whose_inputs_moved_are_re_uploaded() {
        let h = ImageHandle::default();
        let mut picker = ColorPicker::new([h, h, h], Vec2::ZERO, [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(picker.dirty_bitmaps().len(), 3, "all three on first upload");
        assert!(picker.dirty_bitmaps().is_empty(), "nothing changed since");

        // Alpha alone leaves the square and the hue bar byte-identical.
        let mut rgba = picker.rgba();
        rgba[3] = 0.5;
        picker.set_rgba(rgba);
        assert_eq!(picker.dirty_bitmaps().len(), 1, "only the alpha bar");
    }

    #[test]
    fn generators_produce_right_sized_opaque_buffers() {
        assert_eq!(sv_square(0.3, 32, 0.5, 0.5).len(), 32 * 32 * 4);
        assert_eq!(hue_bar(8, 40, 0.5).len(), 8 * 40 * 4);
        assert_eq!(alpha_bar([1.0, 0.0, 0.0], 40, 8, 0.5).len(), 40 * 8 * 4);
    }
}
