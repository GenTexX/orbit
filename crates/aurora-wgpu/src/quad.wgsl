// Quad shader - expands a unit quad per instance to a pixel-space rectangle,
// projects it to clip space, and samples the coverage atlas. Filled rects point
// their UVs at a solid-white texel (coverage 1); glyphs point at their bitmap in
// the atlas (coverage = the rasterized alpha). Same Y-down, top-left pixel space
// as photon (ADR 0012); the projection is uploaded per frame.
//
// A rect may also carry a corner `radius` and/or a `border` (width + color): the
// fragment shader then evaluates a rounded-rect signed distance field for
// anti-aliased corners and paints the outer band in the border color. When both
// are zero (the common case - plain fills and every glyph) the SDF path is
// skipped, so those draws are byte-for-byte what they were before.

struct Screen {
    proj: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> screen: Screen;
@group(1) @binding(0) var atlas: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

struct Instance {
    @location(0) pos: vec2<f32>,          // top-left, in pixels
    @location(1) size: vec2<f32>,         // extent, in pixels
    @location(2) uv_min: vec2<f32>,       // atlas UV at the top-left corner
    @location(3) uv_max: vec2<f32>,       // atlas UV at the bottom-right corner
    @location(4) color: vec4<f32>,        // fill color
    @location(5) border_color: vec4<f32>, // border color (used when border > 0)
    @location(6) radius: f32,             // corner radius, px (0 = square)
    @location(7) border: f32,             // border width, px (0 = no border)
};

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) border_color: vec4<f32>,
    @location(3) local: vec2<f32>, // position within the rect, px (0..size)
    @location(4) half: vec2<f32>,  // half the rect size, px
    @location(5) shape: vec2<f32>, // radius, border
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, inst: Instance) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    );
    let corner = corners[vi];
    let px = inst.pos + corner * inst.size;

    var out: VsOut;
    out.clip_pos = screen.proj * vec4<f32>(px, 0.0, 1.0);
    out.uv = mix(inst.uv_min, inst.uv_max, corner);
    out.color = inst.color;
    out.border_color = inst.border_color;
    out.local = corner * inst.size;
    out.half = inst.size * 0.5;
    out.shape = vec2<f32>(inst.radius, inst.border);
    return out;
}

// Signed distance from `p` (centered in the rect) to a rounded rectangle of
// half-extent `half` and corner radius `r`. Negative inside, 0 on the edge.
fn sdf_round_rect(p: vec2<f32>, half: vec2<f32>, r: f32) -> f32 {
    let q = abs(p) - half + vec2<f32>(r, r);
    return length(max(q, vec2<f32>(0.0, 0.0))) + min(max(q.x, q.y), 0.0) - r;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let coverage = textureSample(atlas, samp, in.uv).r;
    let radius = in.shape.x;
    let border = in.shape.y;

    // Fast path: plain fills and glyphs (no radius, no border) - unchanged.
    if radius <= 0.0 && border <= 0.0 {
        return vec4<f32>(in.color.rgb, in.color.a * coverage);
    }

    // Rounded-rect / border path: an anti-aliased SDF over the rect. Both edges
    // are a ~1px coverage ramp centered on the true isosurface (0.5 exactly on
    // the edge), so neither the outer edge nor the border's inner edge is pushed
    // half a pixel out - the border reads as one even, constant-width stroke.
    let p = in.local - in.half;
    let d = sdf_round_rect(p, in.half, radius);
    let aa = max(fwidth(d), 1e-4);
    // Coverage: 1 inside, 0 outside, edge (d = 0) at 0.5.
    let shape_alpha = clamp(0.5 - d / aa, 0.0, 1.0);

    var col = in.color;
    if border > 0.0 {
        // The border is the outer `border` px (d in [-border, 0]); paint the
        // border color there, fading to the fill across the inner edge
        // (d = -border) with the same centered ramp.
        let t = clamp(0.5 + (d + border) / aa, 0.0, 1.0);
        col = mix(in.color, in.border_color, t);
    }
    return vec4<f32>(col.rgb, col.a * coverage * shape_alpha);
}
