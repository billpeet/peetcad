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

// Per-object data. Must match `ObjectUniform` in renderer.rs exactly.
// Only used by the pipelines that bind group 1.
struct Object {
    model: mat4x4<f32>,
    // rgb = highlight colour, a = how much to blend it in (0 = none)
    tint: vec4<f32>,
    // x = pick object id + 1 (0 = not pickable), yzw unused
    pick: vec4<u32>,
};

@group(1) @binding(0) var<uniform> object: Object;

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

// Line vertex position, pulled slightly towards the eye so edges lying on a surface win
// the depth test. Shared by the visible edges and the pick pass so they agree exactly.
fn biased_line_position(position: vec3<f32>) -> vec4<f32> {
    var world = (object.model * vec4<f32>(position, 1.0)).xyz;
    if is_ortho() {
        world = world - globals.forward.xyz * globals.line_bias.y;
    } else {
        let to_eye = globals.eye.xyz - world;
        world = world + to_eye * globals.line_bias.x;
    }
    return globals.view_proj * vec4<f32>(world, 1.0);
}
