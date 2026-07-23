// Pick shader - renders each sprite's instance index (plus one) into an
// R32Uint id buffer, for editor selection (ADR 0018). The vertex path matches
// sprite.wgsl exactly, so what you click is what you saw; the fragment
// discards mostly-transparent texels (a click passes through the holes in a
// sprite) and writes the id flat - no blending exists for uint targets, so
// the last-drawn (topmost) sprite simply wins, matching paint order.

struct Camera {
    view_proj: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var sprite_tex: texture_2d<f32>;
@group(1) @binding(1) var sprite_sampler: sampler;

// Per-instance data; layout matches `RawInstance` in sprite.rs.
struct Instance {
    @location(0) t_col0: vec2<f32>,   // 2D affine, column 0
    @location(1) t_col1: vec2<f32>,   // 2D affine, column 1
    @location(2) t_col2: vec2<f32>,   // 2D affine, translation column
    @location(3) uv_rect: vec4<f32>,  // (min_u, min_v, max_u, max_v)
    @location(4) tint: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) tint_a: f32,
    @location(2) @interpolate(flat) id: u32,
};

@vertex
fn vs_main(
    @builtin(vertex_index) vi: u32,
    @builtin(instance_index) ii: u32,
    inst: Instance,
) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    );
    let corner = corners[vi];
    let transform = mat3x2<f32>(inst.t_col0, inst.t_col1, inst.t_col2);
    let world = transform * vec3<f32>(corner, 1.0);

    var out: VsOut;
    out.clip_pos = camera.view_proj * vec4<f32>(world, 0.0, 1.0);
    out.uv = mix(inst.uv_rect.xy, inst.uv_rect.zw, corner);
    out.tint_a = inst.tint.a;
    out.id = ii + 1u; // 0 is reserved for "nothing here"
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) u32 {
    let alpha = textureSample(sprite_tex, sprite_sampler, in.uv).a * in.tint_a;
    if alpha < 0.1 {
        discard;
    }
    return in.id;
}
