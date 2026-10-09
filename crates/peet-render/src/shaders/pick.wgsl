// GPU picking: writes object ids, element ids and depth as packed u32 values into three
// Rgba8Unorm attachments (see pick.rs for the encoding). Ids are flat-interpolated with
// `either` sampling, which WebGL2 supports (`first` is not available there); all vertices
// of a face or edge segment carry the same id, so the provoking vertex does not matter.

struct PickIn {
    @location(0) position: vec3<f32>,
    @location(1) id: u32,
};

struct PickOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) @interpolate(flat, either) id: u32,
    @location(1) @interpolate(flat, either) object: u32,
};

struct PickTargets {
    @location(0) object: vec4<f32>,
    @location(1) element: vec4<f32>,
    @location(2) depth: vec4<f32>,
};

@vertex
fn vs_pick_face(v: PickIn, instance: InstanceIn) -> PickOut {
    var out: PickOut;
    out.clip = globals.view_proj * (instance_model(instance) * vec4<f32>(v.position, 1.0));
    out.id = v.id;
    out.object = instance.pick;
    return out;
}

// Same depth bias as the visible edges, so exactly the visible edges are pickable.
@vertex
fn vs_pick_edge(v: PickIn, instance: InstanceIn) -> PickOut {
    var out: PickOut;
    let world = instance_model(instance) * vec4<f32>(v.position, 1.0);
    out.clip = biased_line_position(world.xyz);
    out.id = v.id;
    out.object = instance.pick;
    return out;
}

// Little-endian bytes of `v` as unorm channels (R = lowest byte); exact through Rgba8Unorm.
fn pack_u32(v: u32) -> vec4<f32> {
    let bytes = vec4<u32>(v, v >> 8u, v >> 16u, v >> 24u) & vec4<u32>(255u);
    return vec4<f32>(bytes) / 255.0;
}

@fragment
fn fs_pick(in: PickOut) -> PickTargets {
    var out: PickTargets;
    out.object = pack_u32(in.object);
    out.element = pack_u32(in.id);
    // Window depth, the value the depth test used (reversed-Z).
    out.depth = pack_u32(bitcast<u32>(in.clip.z));
    return out;
}
