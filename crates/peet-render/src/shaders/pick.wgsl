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
    @location(2) @interpolate(flat, either) band: u32,
};

// Puts a depth in its band (see `Band` in batch.rs): opaque objects in the nearer half of
// the reversed depth range, see-through ones in the farther half, so what is opaque wins
// wherever there is something opaque. Applied in clip space: z / w is the window depth.
fn in_band(clip: vec4<f32>, band: u32) -> vec4<f32> {
    if band == 1u {
        return vec4<f32>(clip.x, clip.y, 0.5 * clip.z + 0.5 * clip.w, clip.w);
    }
    if band == 2u {
        return vec4<f32>(clip.x, clip.y, 0.5 * clip.z, clip.w);
    }
    return clip;
}

// The window depth before `in_band`, for unprojecting the hit.
fn out_of_band(depth: f32, band: u32) -> f32 {
    if band == 1u {
        return 2.0 * depth - 1.0;
    }
    if band == 2u {
        return 2.0 * depth;
    }
    return depth;
}

struct PickTargets {
    @location(0) object: vec4<f32>,
    @location(1) element: vec4<f32>,
    @location(2) depth: vec4<f32>,
};

@vertex
fn vs_pick_face(v: PickIn, instance: InstanceIn) -> PickOut {
    var out: PickOut;
    out.clip = in_band(globals.view_proj * (instance_model(instance) * vec4<f32>(v.position, 1.0)), instance.pick.y);
    out.id = v.id;
    out.object = instance.pick.x;
    out.band = instance.pick.y;
    return out;
}

// Same depth bias as the visible edges, so exactly the visible edges are pickable.
@vertex
fn vs_pick_edge(v: PickIn, instance: InstanceIn) -> PickOut {
    var out: PickOut;
    let world = instance_model(instance) * vec4<f32>(v.position, 1.0);
    out.clip = in_band(biased_line_position(world.xyz), instance.pick.y);
    out.id = v.id;
    out.object = instance.pick.x;
    out.band = instance.pick.y;
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
    out.depth = pack_u32(bitcast<u32>(out_of_band(in.clip.z, in.band)));
    return out;
}
