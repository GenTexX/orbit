// Quad shader - expands a unit quad per instance to a pixel-space rectangle,
// projects it to clip space, and samples the coverage atlas. Filled rects point
// their UVs at a solid-white texel (coverage 1); glyphs point at their bitmap in
// the atlas (coverage = the rasterized alpha). Same Y-down, top-left pixel space
// as photon (ADR 0012); the projection is uploaded per frame.

struct Screen {
    proj: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> screen: Screen;
@group(1) @binding(0) var atlas: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

struct Instance {
    @location(0) pos: vec2<f32>,     // top-left, in pixels
    @location(1) size: vec2<f32>,    // extent, in pixels
    @location(2) uv_min: vec2<f32>,  // atlas UV at the top-left corner
    @location(3) uv_max: vec2<f32>,  // atlas UV at the bottom-right corner
    @location(4) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
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
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let coverage = textureSample(atlas, samp, in.uv).r;
    return vec4<f32>(in.color.rgb, in.color.a * coverage);
}
