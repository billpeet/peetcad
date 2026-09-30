// Vertical gradient background, drawn as one full-screen triangle.

struct BgOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) t: f32,
};

@vertex
fn vs_background(@builtin(vertex_index) i: u32) -> BgOut {
    // (0,0), (2,0), (0,2): covers the whole screen.
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var out: BgOut;
    // Depth 0 is the far plane with reversed depth.
    out.clip = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    out.t = uv.y;
    return out;
}

@fragment
fn fs_background(in: BgOut) -> @location(0) vec4<f32> {
    let t = clamp(in.t, 0.0, 1.0);
    return vec4<f32>(mix(globals.bg_bottom.rgb, globals.bg_top.rgb, t), 1.0);
}
