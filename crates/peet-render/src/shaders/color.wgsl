// Flat coloured geometry: edge lines, overlay lines and translucent overlay triangles.
// Overlays are given in world coordinates; mesh edges are placed by their instance.

struct ColorIn {
    @location(0) position: vec3<f32>,
    @location(1) color: vec4<f32>,
};

struct ColorOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_color(v: ColorIn) -> ColorOut {
    var out: ColorOut;
    out.clip = globals.view_proj * vec4<f32>(v.position, 1.0);
    out.color = v.color;
    return out;
}

// Lines are pulled slightly towards the eye so edges lying on a surface win the depth test.
@vertex
fn vs_line(v: ColorIn) -> ColorOut {
    var out: ColorOut;
    out.clip = biased_line_position(v.position);
    out.color = v.color;
    return out;
}

// Mesh edges: coloured by the view style, and tinted along with a highlighted object.
@vertex
fn vs_edge(v: ColorIn, instance: InstanceIn) -> ColorOut {
    var out: ColorOut;
    let world = instance_model(instance) * vec4<f32>(v.position, 1.0);
    out.clip = biased_line_position(world.xyz);
    let rgb = mix(globals.edge_color.rgb, instance.tint.rgb, min(instance.tint.a * 1.6, 1.0));
    out.color = vec4<f32>(rgb, globals.edge_color.a);
    return out;
}

@fragment
fn fs_color(in: ColorOut) -> @location(0) vec4<f32> {
    return in.color;
}
