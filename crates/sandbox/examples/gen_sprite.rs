//! Generates `crates/sandbox/assets/sprite.png`, the sample texture the sandbox draws.
//! Run once with: `cargo run -p sandbox --example gen_sprite`.

use image::{Rgba, RgbaImage};

fn main() {
    let size = 128u32;
    let center = (size as f32 - 1.0) / 2.0;
    let radius = 56.0;

    let mut img = RgbaImage::new(size, size);
    for y in 0..size {
        for x in 0..size {
            let (dx, dy) = (x as f32 - center, y as f32 - center);
            let dist = (dx * dx + dy * dy).sqrt();
            let pixel = if x < 24 && y < 24 {
                // Opaque red marker in the texture's top-left, so on-screen
                // orientation is unmistakable (red must end up top-left).
                Rgba([220, 60, 60, 255])
            } else if dist <= radius {
                // A disc shaded top-to-bottom, so vertical flips are visible.
                let t = y as f32 / size as f32;
                Rgba([60, (170.0 - 90.0 * t) as u8, 220, 255])
            } else {
                // Transparent outside the disc - exercises alpha blending.
                Rgba([0, 0, 0, 0])
            };
            img.put_pixel(x, y, pixel);
        }
    }

    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/assets");
    std::fs::create_dir_all(dir).expect("create assets dir");
    let path = format!("{dir}/sprite.png");
    img.save(&path).expect("write sprite.png");
    println!("wrote {path}");
}
