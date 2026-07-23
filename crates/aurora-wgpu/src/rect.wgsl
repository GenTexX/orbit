// Rect shader - expands a unit quad per instance to a pixel-space rectangle,
// projects it to clip space, and fills it with a flat color. Same Y-down,
// top-left pixel space as photon (ADR 0012); the projection is uploaded per frame.

struct Screen {
    proj: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> screen: Screen;

struct Instance {
    @location(0) pos: vec2<f32>,   // top-left, in pixels
    @location(1) size: vec2<f32>,  // extent, in pixels
    @location(2) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, inst: Instance) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    );
    let px = inst.pos + corners[vi] * inst.size;

    var out: VsOut;
    out.clip_pos = screen.proj * vec4<f32>(px, 0.0, 1.0);
    out.color = inst.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return in.color;
}
