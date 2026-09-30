// Infinite-looking ground grid on the XY plane.
//
// Lines are computed analytically per pixel (anti-aliased with screen-space derivatives).
// Three decades of spacing are drawn at once and cross-faded as the user zooms, so the
// grid never pops when it switches spacing.

struct GridOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world_xy: vec2<f32>,
};

@vertex
fn vs_grid(@builtin(vertex_index) i: u32) -> GridOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
    );
    let xy = globals.grid_center.xy + corners[i] * globals.grid_params.z;
    var out: GridOut;
    out.clip = globals.view_proj * vec4<f32>(xy, 0.0, 1.0);
    out.world_xy = xy;
    return out;
}

// Coverage (0..1) of grid lines with the given spacing at this pixel, about 1 px wide.
fn grid_lines(coord: vec2<f32>, deriv: vec2<f32>, spacing: f32) -> f32 {
    let c = coord / spacing;
    let d = max(deriv / spacing, vec2<f32>(1e-7));
    let g = abs(fract(c - 0.5) - 0.5) / d;
    return 1.0 - min(min(g.x, g.y), 1.0);
}

// Coverage of the line where `v` is zero, about 2 px wide.
fn axis_line(v: f32, deriv: f32) -> f32 {
    return 1.0 - min(abs(v) / max(deriv * 1.5, 1e-7), 1.0);
}

@fragment
fn fs_grid(in: GridOut) -> @location(0) vec4<f32> {
    // Derivatives first, in uniform control flow.
    let deriv = fwidth(in.world_xy);

    let spacing = globals.grid_params.x;
    let t = globals.grid_params.y;
    let minor = globals.grid_params.w;
    let major = globals.grid_color.a;

    let l0 = grid_lines(in.world_xy, deriv, spacing) * t * minor;
    let l1 = grid_lines(in.world_xy, deriv, spacing * 10.0) * mix(minor, major, t);
    let l2 = grid_lines(in.world_xy, deriv, spacing * 100.0) * major;
    var alpha = max(l0, max(l1, l2));
    var rgb = globals.grid_color.rgb;

    // The X axis is the line y = 0, the Y axis is x = 0.
    let ax = axis_line(in.world_xy.y, deriv.y);
    let ay = axis_line(in.world_xy.x, deriv.x);
    rgb = mix(rgb, globals.axis_x_color.rgb, ax);
    alpha = max(alpha, ax * globals.axis_x_color.a);
    rgb = mix(rgb, globals.axis_y_color.rgb, ay);
    alpha = max(alpha, ay * globals.axis_y_color.a);

    // Fade out towards the edge of the grid and at grazing angles (where lines alias).
    let extent = globals.grid_params.z;
    let dist = length(in.world_xy - globals.grid_center.xy);
    alpha = alpha * (1.0 - smoothstep(0.3 * extent, extent, dist));
    let graze = abs(view_dir_to(vec3<f32>(in.world_xy, 0.0)).z);
    alpha = alpha * smoothstep(0.02, 0.2, graze);

    if alpha < 0.003 {
        discard;
    }
    return vec4<f32>(rgb, alpha);
}
