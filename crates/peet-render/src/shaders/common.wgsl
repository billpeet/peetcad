// Shared declarations. Must match `Globals` in renderer.rs exactly.

struct Globals {
    view_proj: mat4x4<f32>,
    // xyz = eye position, w = 1.0 for orthographic projection, 0.0 for perspective.
    eye: vec4<f32>,
    // xyz = viewing direction, w = world units per pixel at the target.
    forward: vec4<f32>,
    right: vec4<f32>,
    up: vec4<f32>,
    // width, height, 1/width, 1/height (pixels)
    viewport: vec4<f32>,
    bg_top: vec4<f32>,
    bg_bottom: vec4<f32>,
    // rgb = grid line colour, a = major line opacity
    grid_color: vec4<f32>,
    // x = finest line spacing, y = level blend (0..1), z = half-extent, w = minor line opacity
    grid_params: vec4<f32>,
    // xy = grid centre, zw unused
    grid_center: vec4<f32>,
    axis_x_color: vec4<f32>,
    axis_y_color: vec4<f32>,
    // x = line bias as a fraction of eye distance (perspective), y = line bias in world units (orthographic)
    line_bias: vec4<f32>,
    // sRGB colour of mesh edge lines
    edge_color: vec4<f32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;

// Per-object data. Only used by the pipelines that bind group 1.
struct Object {
    model: mat4x4<f32>,
    // rgb = highlight colour, a = how much to blend it in (0 = none)
    tint: vec4<f32>,
};

fn is_ortho() -> bool {
    return globals.eye.w > 0.5;
}

// Direction from the eye towards a world point.
fn view_dir_to(p: vec3<f32>) -> vec3<f32> {
    if is_ortho() {
        return globals.forward.xyz;
    }
    return normalize(p - globals.eye.xyz);
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}
