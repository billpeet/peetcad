//! Drawing on a real GPU: objects that share a mesh are each drawn in their own place
//! and picked as themselves, and what is outside the view is left out. Skipped (with a
//! note) on a machine with no graphics adapter.

use peet_math::{Aabb, DMat4, DVec3};
use peet_render::{
    Camera, FrameInput, MeshData, MeshVertex, ObjectDraw, Overlay, PickRequest, PickResult,
    Projection, StandardView, ViewStyle, ViewportRenderer, wgpu,
};

const SIZE: [u32; 2] = [400, 300];

fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
    gpu_with_info().map(|(device, queue, _)| (device, queue))
}

/// The device, with what kind of adapter it is on.
fn gpu_with_info() -> Option<(wgpu::Device, wgpu::Queue, wgpu::AdapterInfo)> {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .ok()?;
    let info = adapter.get_info();
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()?;
    Some((device, queue, info))
}

/// A ball of about `2 * around * (around / 2)` triangles with lines along its
/// parallels: a stand-in for a part with curved faces.
fn ball(radius: f32, around: u32) -> MeshData {
    let up = around / 2;
    let mut mesh = MeshData::default();
    for j in 0..=up {
        let v = std::f32::consts::PI * j as f32 / up as f32;
        for i in 0..=around {
            let u = std::f32::consts::TAU * i as f32 / around as f32;
            let n = [v.sin() * u.cos(), v.sin() * u.sin(), v.cos()];
            mesh.vertices.push(MeshVertex {
                position: n.map(|c| c * radius),
                normal: n,
                color: [190, 196, 204, 255],
            });
            mesh.pick_ids.push(j);
        }
    }
    let row = around + 1;
    for j in 0..up {
        for i in 0..around {
            let (a, b) = (j * row + i, (j + 1) * row + i);
            mesh.indices.extend([a, b, a + 1, a + 1, b, b + 1]);
            let (p, q) = (mesh.vertices[a as usize], mesh.vertices[a as usize + 1]);
            mesh.edges.extend([p.position, q.position]);
            mesh.edge_pick_ids.push(j);
        }
    }
    mesh
}

/// A square of side 20 in the XY plane, facing up, whose face has the pick id 7.
fn square() -> MeshData {
    let corners = [[-10.0, -10.0], [10.0, -10.0], [10.0, 10.0], [-10.0, 10.0]];
    MeshData {
        vertices: corners
            .iter()
            .map(|[x, y]| MeshVertex {
                position: [*x, *y, 0.0],
                normal: [0.0, 0.0, 1.0],
                color: [200, 200, 200, 255],
            })
            .collect(),
        indices: vec![0, 1, 2, 0, 2, 3],
        edges: Vec::new(),
        pick_ids: vec![7; 4],
        edge_pick_ids: Vec::new(),
    }
}

struct Scene {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: ViewportRenderer,
    camera: Camera,
    bounds: Aabb,
}

impl Scene {
    /// Where a world point is on screen, in pixels.
    fn pixel(&self, world: DVec3) -> [f32; 2] {
        let aspect = f64::from(SIZE[0]) / f64::from(SIZE[1]);
        let clip = self.camera.view_projection(aspect, &self.bounds) * world.extend(1.0);
        let ndc = clip.truncate() / clip.w;
        [
            ((ndc.x * 0.5 + 0.5) * f64::from(SIZE[0])) as f32,
            ((0.5 - ndc.y * 0.5) * f64::from(SIZE[1])) as f32,
        ]
    }

    /// Draws `objects` and picks at a world point. Returns the pick and how many
    /// objects were drawn and left out.
    fn pick(&mut self, objects: &[ObjectDraw], at: DVec3) -> (PickResult, usize, usize) {
        let cursor_px = self.pixel(at);
        let style = ViewStyle::dark();
        let overlay = Overlay::default();
        for _ in 0..200 {
            let stats = self
                .renderer
                .render(
                    &self.device,
                    &self.queue,
                    &FrameInput {
                        size_px: SIZE,
                        camera: &self.camera,
                        style: &style,
                        show_grid: false,
                        objects,
                        overlay: &overlay,
                        scene_bounds: self.bounds,
                        pick: Some(PickRequest {
                            cursor_px,
                            radius_px: 4.0,
                        }),
                    },
                )
                .stats;
            let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
            if let Some(result) = self.renderer.pick_result() {
                return (result, stats.objects, stats.culled);
            }
        }
        panic!("no pick result arrived");
    }
}

#[test]
fn objects_sharing_a_mesh_are_each_drawn_and_picked_where_they_are() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no graphics adapter: skipped");
        return;
    };
    let mut renderer = ViewportRenderer::new(&device, 1);
    let mesh = renderer.upload_mesh(&device, &square());
    let bounds = Aabb::from_points([DVec3::new(-60.0, -30.0, -1.0), DVec3::new(60.0, 30.0, 1.0)]);
    let mut camera = Camera {
        rotation: StandardView::Top.rotation(),
        projection: Projection::Orthographic,
        ..Camera::default()
    };
    camera.fit(&bounds, f64::from(SIZE[0]) / f64::from(SIZE[1]));
    let mut scene = Scene {
        device,
        queue,
        renderer,
        camera,
        bounds,
    };
    let place = |x: f64, pick| ObjectDraw {
        mesh,
        transform: DMat4::from_translation(DVec3::new(x, 0.0, 0.0)),
        pick_object: Some(pick),
        ..ObjectDraw::default()
    };
    // Three of one mesh: left, right, and one far outside the view.
    let objects = [place(-40.0, 0), place(40.0, 1), place(5000.0, 2)];

    let (left, drawn, culled) = scene.pick(&objects, DVec3::new(-40.0, 0.0, 0.0));
    assert_eq!((drawn, culled), (2, 1));
    let hit = left.face.expect("the left square is under the cursor");
    assert_eq!((hit.object, hit.id), (0, 7));
    assert!(
        hit.world.distance(DVec3::new(-40.0, 0.0, 0.0)) < 1.0,
        "{}",
        hit.world
    );

    let (right, ..) = scene.pick(&objects, DVec3::new(40.0, 0.0, 0.0));
    let hit = right.face.expect("the right square is under the cursor");
    assert_eq!((hit.object, hit.id), (1, 7));

    // Between them there is nothing: neither was drawn where the other is.
    let (between, ..) = scene.pick(&objects, DVec3::ZERO);
    assert_eq!(between.face, None);

    // The same in perspective.
    scene.camera.projection = Projection::Perspective;
    let (right, drawn, culled) = scene.pick(&objects, DVec3::new(40.0, 0.0, 0.0));
    assert_eq!((drawn, culled), (2, 1));
    assert_eq!(right.face.map(|h| h.object), Some(1));
}

/// The phase's exit criterion for drawing: 1,000 instances of 50 parts at 60 frames a
/// second. Each frame is drawn and waited for, so the time is the CPU's and the GPU's.
#[test]
fn a_thousand_instances_of_fifty_parts_draw_in_a_frame() {
    let Some((device, queue, info)) = gpu_with_info() else {
        eprintln!("no graphics adapter: skipped");
        return;
    };
    let size = [1600, 1000];
    let mut renderer = ViewportRenderer::new(&device, 4);
    // Fifty different parts of about 2,000 triangles and 1,000 edge segments.
    let meshes: Vec<_> = (0..50)
        .map(|i| renderer.upload_mesh(&device, &ball(20.0 + i as f32 * 0.3, 44)))
        .collect();
    let objects: Vec<ObjectDraw> = (0..1000u32)
        .map(|i| ObjectDraw {
            mesh: meshes[i as usize % 50],
            transform: DMat4::from_translation(
                DVec3::new(
                    f64::from(i % 10),
                    f64::from(i / 10 % 10),
                    f64::from(i / 100),
                ) * 80.0,
            ),
            pick_object: Some(i),
            ..ObjectDraw::default()
        })
        .collect();
    let bounds = Aabb::from_points([DVec3::splat(-40.0), DVec3::splat(760.0)]);
    let aspect = f64::from(size[0]) / f64::from(size[1]);
    let mut camera = Camera::default();
    camera.fit(&bounds, aspect);
    let style = ViewStyle::dark();
    let overlay = Overlay::default();
    let mut frame = |camera: &Camera, pick: bool| {
        let stats = renderer
            .render(
                &device,
                &queue,
                &FrameInput {
                    size_px: size,
                    camera,
                    style: &style,
                    show_grid: true,
                    objects: &objects,
                    overlay: &overlay,
                    scene_bounds: bounds,
                    pick: pick.then_some(PickRequest {
                        cursor_px: [800.0, 500.0],
                        radius_px: 6.0,
                    }),
                },
            )
            .stats;
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        stats
    };
    // The first frames create pipelines and buffers.
    for _ in 0..5 {
        frame(&camera, true);
    }
    let frames = 60;
    let start = std::time::Instant::now();
    let mut stats = frame(&camera, true);
    for i in 1..frames {
        // Turning, as a user would: every frame is different.
        camera.orbit(
            peet_math::DVec2::new(0.01, 0.004),
            peet_render::OrbitStyle::Turntable,
        );
        stats = frame(&camera, i % 2 == 0);
    }
    let per_frame = start.elapsed() / frames;
    println!(
        "1000 instances of 50 parts on {} ({:?}): {per_frame:?} a frame, {} draw calls, {} triangles, {} lines",
        info.name, info.device_type, stats.draw_calls, stats.triangles, stats.lines
    );
    assert_eq!((stats.objects, stats.culled), (1000, 0));
    // A draw for the faces and one for the edges of each part, and a few for the rest.
    assert!(stats.draw_calls <= 2 * 50 + 4, "{}", stats.draw_calls);
    assert!(stats.triangles > 1_500_000, "{}", stats.triangles);
    // On real graphics hardware, inside a 60 Hz frame. (A software adapter is not held
    // to it.)
    if matches!(
        info.device_type,
        wgpu::DeviceType::DiscreteGpu | wgpu::DeviceType::IntegratedGpu
    ) && !cfg!(debug_assertions)
    {
        assert!(per_frame.as_secs_f64() < 1.0 / 60.0, "{per_frame:?}");
    }
}
