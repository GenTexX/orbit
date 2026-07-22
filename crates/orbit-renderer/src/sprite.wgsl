// Sprite shader - expands a unit quad per instance, applies the instance's 2D
// affine transform (mat3x2) and the camera view-projection, then samples the
// bound texture modulated by the instance tint. Coordinate space: ADR 0012.

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
    @location(1) tint: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, inst: Instance) -> VsOut {
    // Unit-quad corners in [0,1], two triangles. A local `var` so the dynamic
    // index is well-defined in WGSL.
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
    out.tint = inst.tint;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return textureSample(sprite_tex, sprite_sampler, in.uv) * in.tint;
}
