// Shaded, two-sided solid meshes lit by a camera-attached key light.

struct MeshIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
};

struct MeshOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
};

@vertex
fn vs_mesh(v: MeshIn) -> MeshOut {
    let world = object.model * vec4<f32>(v.position, 1.0);
    var out: MeshOut;
    out.clip = globals.view_proj * world;
    out.world_pos = world.xyz;
    // Object transforms are rigid, so the model matrix transforms normals correctly.
    out.normal = (object.model * vec4<f32>(v.normal, 0.0)).xyz;
    out.color = v.color;
    return out;
}

@fragment
fn fs_mesh(in: MeshOut) -> @location(0) vec4<f32> {
    let v = view_dir_to(in.world_pos);
    var n = normalize(in.normal);
    // Two-sided lighting: always shade the side facing the viewer.
    if dot(n, v) > 0.0 {
        n = -n;
    }
    let base = srgb_to_linear(mix(in.color.rgb, object.tint.rgb, object.tint.a));

    // Key light slightly above and to the left of the camera, so faces square to the
    // view still differ in brightness from their neighbours.
    let key = normalize(-v + globals.up.xyz * 0.45 - globals.right.xyz * 0.25);
    let diffuse = max(dot(n, key), 0.0);
    // Soft fill from the opposite side and a hemisphere ambient term (world Z is the sky).
    let fill = max(dot(n, normalize(-v - globals.up.xyz * 0.3 + globals.right.xyz * 0.6)), 0.0);
    let ambient = 0.22 + 0.08 * n.z;
    let half_vec = normalize(key - v);
    let specular = pow(max(dot(n, half_vec), 0.0), 48.0) * 0.18;

    let lit = base * (ambient + 0.68 * diffuse + 0.15 * fill) + vec3<f32>(specular);
    return vec4<f32>(linear_to_srgb(clamp(lit, vec3<f32>(0.0), vec3<f32>(1.0))), 1.0);
}
