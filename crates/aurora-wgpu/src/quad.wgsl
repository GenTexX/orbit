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
    @location(6) radius: vec4<f32>,       // per-corner radius px [TL,TR,BR,BL]
    @location(7) sides: vec4<f32>,        // border edges [top,right,bottom,left] 1=draw
    @location(8) border: f32,             // border width, px (0 = no border)
};

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) border_color: vec4<f32>,
    @location(3) local: vec2<f32>,  // position within the rect, px (0..size)
    @location(4) half: vec2<f32>,   // half the rect size, px
    @location(5) radius: vec4<f32>, // per-corner radius px [TL,TR,BR,BL]
    @location(6) sides: vec4<f32>,  // border edges [top,right,bottom,left]
    @location(7) border: f32,       // border width, px
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
    out.radius = inst.radius;
    out.sides = inst.sides;
    out.border = inst.border;
    return out;
}

// Signed distance from `p` (centered in the rect) to a rounded rectangle of
// half-extent `half`, each corner rounded to its own radius in `radii`
// (`[top_left, top_right, bottom_right, bottom_left]`). Negative inside, 0 on
// the edge. `p.y > 0` is the bottom (Y-down space, matching photon/ADR 0012).
fn sdf_round_rect(p: vec2<f32>, half: vec2<f32>, radii: vec4<f32>) -> f32 {
    // Pick the radius of the corner this fragment sits nearest.
    let top = select(radii.x, radii.y, p.x > 0.0); // left=TL, right=TR
    let bot = select(radii.w, radii.z, p.x > 0.0); // left=BL, right=BR
    let r = select(top, bot, p.y > 0.0);
    let q = abs(p) - half + vec2<f32>(r, r);
    return length(max(q, vec2<f32>(0.0, 0.0))) + min(max(q.x, q.y), 0.0) - r;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let coverage = textureSample(atlas, samp, in.uv).r;
    let border = in.border;

    // Fast path: plain fills and glyphs (no radius, no border) - unchanged.
    if all(in.radius <= vec4<f32>(0.0)) && border <= 0.0 {
        return vec4<f32>(in.color.rgb, in.color.a * coverage);
    }

    // Rounded-rect / border path: an anti-aliased SDF over the rect. Both edges
    // are a ~1px coverage ramp centered on the true isosurface (0.5 exactly on
    // the edge), so neither the outer edge nor the border's inner edge is pushed
    // half a pixel out - the border reads as one even, constant-width stroke.
    let p = in.local - in.half;
    let d = sdf_round_rect(p, in.half, in.radius);
    let aa = max(fwidth(d), 1e-4);
    // Coverage: 1 inside, 0 outside, edge (d = 0) at 0.5.
    let shape_alpha = clamp(0.5 - d / aa, 0.0, 1.0);

    var col = in.color;
    if border > 0.0 {
        // The border is the outer `border` px (d in [-border, 0]); paint the
        // border color there, fading to the fill across the inner edge
        // (d = -border) with the same centered ramp.
        let t = clamp(0.5 + (d + border) / aa, 0.0, 1.0);
        // Gate the stroke on the nearest edge's flag, so an open side (e.g. the
        // bottom of a folder-style tab) keeps its fill but draws no line.
        let dl = p.x + in.half.x;
        let dr = in.half.x - p.x;
        let dt = p.y + in.half.y;
        let db = in.half.y - p.y;
        var side_on = in.sides.w; // left
        var md = dl;
        if dr < md { md = dr; side_on = in.sides.y; } // right
        if dt < md { md = dt; side_on = in.sides.x; } // top
        if db < md { md = db; side_on = in.sides.z; } // bottom
        col = mix(in.color, in.border_color, t * side_on);
    }
    return vec4<f32>(col.rgb, col.a * coverage * shape_alpha);
}
